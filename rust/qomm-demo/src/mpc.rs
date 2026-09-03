//!
//! `qomm-mpc` owns program and input generation.  MP-SPDZ remains the protocol
//! implementation: this module compiles one shape, starts all parties for each
//! round, reads the opened masked key, and checks it against the same cleartext
//! model the browser's simulation path uses.

use crate::model::{evaluate, Outcome, Policy, Request};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::inputs::{build_inputs, finish_reference, parse_policies, InputConfig};
use qomm_mpc::program::{build_program, pow2_ceil, sentinel_for, CheckMode, Mode, ProgramConfig};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const CORPORATE_QUEUE_UNAVAILABLE: &str =
    "MPC nodes are unavailable; the signed RFQ is durably queued and no local execution was attempted";
pub const CORPORATE_QUEUE_WAITING_SLOT: &str =
    "the signed RFQ is durably queued and is waiting for its fixed-rate dispatch slot";
pub const CORPORATE_QUEUE_RECONCILING: &str =
    "the signed RFQ remains reserved in DeFMI and is awaiting canonical reconciliation";

pub fn is_corporate_queue_pending(error: &str) -> bool {
    matches!(
        error,
        CORPORATE_QUEUE_UNAVAILABLE | CORPORATE_QUEUE_WAITING_SLOT
    ) || error.starts_with(CORPORATE_QUEUE_RECONCILING)
}

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct MpcRound {
    pub outcome: Outcome,
    /// True only when the MPC fill bit is one. Verification success and trade
    /// execution are separate states: a price-limit rejection is valid but is
    /// not a fill.
    pub filled: bool,
    pub masked_key: i128,
    pub mask: u64,
    pub node_shares: BTreeMap<usize, Vec<String>>,
    pub named: BTreeMap<usize, usize>,
    pub verified: bool,
    pub detail: String,
    pub stats: Value,
    /// Public handoff to the settlement coordinator.  It contains only
    /// commitments to each node-local persistence file, never the persisted
    /// Shamir evaluations themselves.
    pub product_handoff: Option<MpcProductHandoff>,
}

/// A corporate request recovered from its durable participant-owned outbox and
/// executed after the MPC committee becomes healthy again.
pub struct QueuedMpcRound {
    pub policies: Vec<Policy>,
    pub request: Request,
    pub settlement: MpcSettlementInputs,
    pub market_time: i64,
    pub round: MpcRound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpcProductHandoff {
    pub round_id: String,
    pub source_sha256: String,
    pub persistence_sha256: BTreeMap<usize, String>,
    /// Public fingerprint of the durable 3-of-7 signing group.  The key shares
    /// remain encrypted inside the seven node-local proof-party states.
    pub frost_public_sha256: String,
    /// Present for real requests after all seven proof nodes have loaded their
    /// own persistence file and jointly proved the winning registered quote.
    pub proof_job_id: Option<String>,
    pub quote_proof_digest: Option<String>,
    /// Canonical private verifier-complete record.  It contains commitments,
    /// threshold proofs and recipient-encrypted openings, but no clear amount,
    /// price, reserve or policy opening.  DeFMI typing and execution receipts
    /// are attached before it becomes an admissible settlement transaction.
    pub settlement_record: Option<Vec<u8>>,
    /// Seven node-identity-signed receipts over the exact MP-SPDZ inputs,
    /// stdout/stderr and node-local persistence used by this proof job.
    pub execution_attestations: Option<Vec<u8>>,
    pub execution_node_keys: Option<Vec<[u8; 32]>>,
    /// Seven node-identity signatures over the pre-MPC legal-entity claim,
    /// content-independent order and each node's distinct input-share batch.
    pub admission_attestations: Option<Vec<u8>>,
    pub admission_node_keys: Option<Vec<[u8; 32]>>,
    /// Present only for a real request: canonical Taker mandate body followed
    /// by its Ed25519 signature.  It was signed before any MPC node executed.
    pub signed_taker_mandate: Option<Vec<u8>>,
    /// Canonical standing Maker mandates.  Each active two-sided policy has
    /// one inventory-backed sell authorization and one cash-backed buy
    /// authorization, both signed before the RFQ reaches the MPC service.
    pub signed_maker_mandates: Vec<Vec<u8>>,
}

/// Pre-trade amounts already reserved under Maker and Taker mandates.
///
/// The matching circuit persists the selected price and the remainders against
/// these exact limits.  A later zkPI/DvP proof therefore cannot substitute a
/// larger reserve or a different Taker limit after seeing the match.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MpcSettlementInputs {
    pub user_limit: i64,
    pub taker_securities_reserve: i64,
    pub taker_cash_reserve: i64,
    pub maker_securities_reserves: Vec<i64>,
    pub maker_cash_reserves: Vec<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaskedExecutionOpening {
    pub padded_makers: usize,
    pub masked_key: i128,
    pub key_mask: u64,
    pub masked_fill: i128,
    pub fill_mask: u64,
}

/// Verify the two Taker-masked public outputs.  The fill bit is checked
/// separately from the packed quote so an executable zero-price quote cannot
/// be confused with a non-fill.
pub(crate) fn verify_masked_execution(
    outcome: &Outcome,
    request: &Request,
    user_limit: i64,
    opening: MaskedExecutionOpening,
) -> Result<(bool, String, bool), String> {
    let MaskedExecutionOpening {
        padded_makers,
        masked_key,
        key_mask,
        masked_fill,
        fill_mask,
    } = opening;
    let opened_fill = masked_fill
        .checked_sub(i128::from(fill_mask))
        .ok_or_else(|| "masked fill underflowed its Taker mask".to_string())?;
    let filled = match opened_fill {
        0 => false,
        1 => true,
        _ => return Err("MPC opened a fill value outside zero or one".into()),
    };
    let opened_key = masked_key
        .checked_sub(i128::from(key_mask))
        .ok_or_else(|| "masked quote underflowed its Taker mask".to_string())?;
    let expected_fill = if request.is_real == 0 {
        false
    } else {
        match (request.direction, outcome.price) {
            (_, None) => false,
            (0, Some(price)) => price <= user_limit,
            (1, Some(price)) => price >= user_limit,
            _ => return Err("request direction is outside buy or sell".into()),
        }
    };
    if !filled {
        return Ok((
            !expected_fill && opened_key == 0,
            if expected_fill {
                "MPC reported no fill for an executable best quote".into()
            } else if opened_key != 0 {
                "MPC exposed a packed quote despite reporting no fill".into()
            } else {
                "no executable quote; the Taker-masked fill and key agree".into()
            },
            false,
        ));
    }
    let Some((cost, winner)) = outcome.cost.zip(outcome.winner) else {
        return Ok((
            false,
            "MPC reported a fill when no eligible Maker exists".into(),
            true,
        ));
    };
    let unpacked = unpack_key(opened_key, padded_makers);
    let wanted = (i128::from(cost), winner);
    Ok((
        expected_fill && unpacked == wanted,
        if !expected_fill {
            "MPC reported a fill outside the Taker's signed price limit".into()
        } else {
            format!("got={unpacked:?} want={wanted:?}")
        },
        true,
    ))
}

impl MpcSettlementInputs {
    pub fn validate(&self, makers: usize) -> Result<(), String> {
        if self.user_limit < 0
            || self.taker_securities_reserve < 0
            || self.taker_cash_reserve < 0
            || self.maker_securities_reserves.len() != makers
            || self.maker_cash_reserves.len() != makers
            || self
                .maker_securities_reserves
                .iter()
                .chain(&self.maker_cash_reserves)
                .any(|value| *value < 0)
        {
            return Err("pre-trade settlement inputs do not match the Maker population".into());
        }
        Ok(())
    }
}

/// Execution boundary used by the browser room.  The local implementation
/// starts every stock MP-SPDZ party on one host, while the Docker deployment
/// installs an implementation that sends only one party's share to each
/// independently running node service.  Both implementations return the same
/// fail-closed round receipt to the UI and settlement layer.
pub trait MpcQuoteEngine: Send {
    fn name(&self) -> &'static str;
    fn note(&self) -> String;
    fn robust(&self) -> bool;
    fn robust_reason(&self) -> &str;
    fn input_check(&self) -> bool;
    /// Register or refresh standing Maker policy authority before an RFQ can
    /// run. Local/simulation engines have no external custody boundary and use
    /// the default no-op; the distributed engine fails closed on stale policy
    /// authority inside `quote`.
    fn preauthorize_maker_policies(
        &mut self,
        _policies: &[Policy],
        _settlement: &MpcSettlementInputs,
        _now: i64,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Poll one fixed-rate corporate outbox slot. Engines without an external
    /// participant package have no queue and therefore return `None`.
    fn replay_queued(&mut self) -> Result<Option<QueuedMpcRound>, String> {
        Ok(None)
    }
    /// When a round ended with its corporate request retained for a replay,
    /// report the request's canonical status once the participant module has
    /// finalized it (`released` or `consumed`) without any replay, so the
    /// room can drop the Taker reserve it kept for that replay.  Engines
    /// without a corporate queue never retain anything.
    fn retained_corporate_finalized(&mut self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn quote(
        &mut self,
        policies: &[Policy],
        request: &Request,
        settlement: &MpcSettlementInputs,
        now: i64,
        corrupt: &[usize],
    ) -> Result<MpcRound, String>;
}

pub struct MpcEngine {
    root: PathBuf,
    binary: PathBuf,
    n_parties: usize,
    threshold: usize,
    n_makers: usize,
    bit_length: u32,
    references: Vec<i128>,
    input_check: bool,
    program: String,
    config: ProgramConfig,
    compile_ms: f64,
    served: u64,
    robust: bool,
    robust_reason: String,
    work: TempRoot,
}

impl MpcEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: impl AsRef<Path>,
        n_parties: usize,
        threshold: usize,
        n_makers: usize,
        references: &[i64],
        bit_length: u32,
        input_check: bool,
    ) -> Result<Self, String> {
        if n_parties < 2 * threshold + 1 {
            return Err(format!(
                "{n_parties} parties cannot carry threshold {threshold}"
            ));
        }
        let compiler = OfficialCompiler::from_checkout(root).map_err(|error| error.to_string())?;
        let root = compiler.root().to_path_buf();
        let atlas = root.join("atlas-party.x");
        let malicious = root.join("malicious-shamir-party.x");
        let robust = atlas.is_file() && n_parties > 4 * threshold;
        let (binary, robust_reason) = if robust {
            (atlas, String::new())
        } else if !atlas.is_file() {
            (
                malicious,
                "no atlas-party.x in the MP-SPDZ checkout; real party corruption is disabled"
                    .into(),
            )
        } else {
            (
                malicious,
                format!(
                    "n={n_parties}, T={threshold} is below n >= 4T+1; real party corruption is disabled"
                ),
            )
        };
        if !binary.is_file() {
            return Err(format!(
                "MP-SPDZ party binary {} is absent",
                binary.display()
            ));
        }
        let padded = pow2_ceil(n_makers).map_err(|error| error.to_string())?;
        let config = ProgramConfig {
            n_mm: padded,
            n_parties,
            n_assets: references.len(),
            ref_table: references.iter().copied().map(i128::from).collect(),
            maker_assets: (0..padded)
                .map(|maker| maker % references.len().max(1))
                .collect(),
            bit_length,
            input_check,
            check_mode: CheckMode::PerParty,
            // A real Taker reserve is authorized against `user_limit`.  The
            // MPC circuit must therefore compute the masked fill bit against
            // that same limit; otherwise the verifier asks for an output the
            // circuit never emits and, more importantly, matching ignores the
            // signed maximum/minimum price.
            binding_limit: true,
            ..ProgramConfig::default()
        };
        let source = build_program(&config).map_err(|error| error.to_string())?;
        let work_path = std::env::temp_dir().join(format!(
            "qomm-demo-mpc-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&work_path).map_err(|error| error.to_string())?;
        let work = TempRoot(work_path);
        let program = format!(
            "qomm_demo_rust_{}_{:016x}",
            std::process::id(),
            rand::random::<u64>()
        );
        let source_path = root
            .join("Programs")
            .join("Source")
            .join(format!("{program}.mpc"));
        fs::create_dir_all(
            source_path
                .parent()
                .ok_or_else(|| "program source has no parent".to_string())?,
        )
        .map_err(|error| error.to_string())?;
        fs::write(&source_path, source).map_err(|error| error.to_string())?;
        let started = Instant::now();
        let compiled = compiler
            .compile_field(128, &program)
            .map_err(|error| error.to_string())?;
        if !compiled.status.success() {
            let _ = fs::remove_file(&source_path);
            return Err(format!(
                "MP-SPDZ compile failed: {}",
                String::from_utf8_lossy(&compiled.stderr)
                    .chars()
                    .rev()
                    .take(2_000)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ));
        }
        Ok(Self {
            root,
            binary,
            n_parties,
            threshold,
            n_makers,
            bit_length,
            references: references.iter().copied().map(i128::from).collect(),
            input_check,
            program,
            config,
            compile_ms: started.elapsed().as_secs_f64() * 1_000.0,
            served: 0,
            robust,
            robust_reason,
            work,
        })
    }

    pub const fn name(&self) -> &'static str {
        "mpc"
    }

    pub fn note(&self) -> String {
        format!(
            "MP-SPDZ {}, n={}, T={}{}",
            self.binary
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("party"),
            self.n_parties,
            self.threshold,
            if self.robust { ", robust" } else { "" }
        )
    }

    pub const fn robust(&self) -> bool {
        self.robust
    }

    pub fn robust_reason(&self) -> &str {
        &self.robust_reason
    }

    pub const fn input_check(&self) -> bool {
        self.input_check
    }

    pub fn quote(
        &mut self,
        policies: &[Policy],
        request: &Request,
        settlement: &MpcSettlementInputs,
        now: i64,
        corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        if policies.len() != self.n_makers {
            return Err("the live policy count differs from the compiled shape".into());
        }
        settlement.validate(self.n_makers)?;
        let policy_json = serde_json::to_string(policies).map_err(|error| error.to_string())?;
        let mpc_policies = parse_policies(&policy_json).map_err(|error| error.to_string())?;
        let seed = i128::from(self.served.saturating_add(7));
        let config = InputConfig {
            n_mm: self.config.n_mm,
            n_real_mm: self.n_makers,
            n_parties: self.n_parties,
            is_real: i128::from(request.is_real),
            n_requests: 1,
            n_assets: self.references.len(),
            ref_table: &self.references,
            user_asset: usize::try_from(request.asset)
                .map_err(|_| "request asset is negative".to_string())?,
            user_qty: i128::from(request.qty),
            user_dir: i128::from(request.direction),
            user_entity: i128::from(request.entity),
            now_t: i128::from(now),
            seed,
            audit_gates: self.config.audit_gates,
            value_bits: self.bit_length + 1,
            field_bits: 128,
            use_ref: 1,
            reference: self.config.reference,
            input_check: self.input_check,
            check_mode: self.config.check_mode,
            binding_limit: self.config.binding_limit,
            user_limit: i128::from(settlement.user_limit),
            user_limit_blinding: i128::from(self.served.saturating_add(101)),
            user_qty_blinding: i128::from(self.served.saturating_add(151)),
            response_mask: None,
            fill_mask: None,
            check_coefficients: &self.config.check_coefficients,
            check_repeats: self.config.check_repeats,
            policies: Some(&mpc_policies),
            shamir_inputs: false,
            shamir_threshold: self.threshold,
            dvp: None,
            quote_proof: None,
        };
        let mut generated = build_inputs(&config).map_err(|error| error.to_string())?;
        let max_reference = self.references.iter().copied().max().unwrap_or(0);
        let sentinel = sentinel_for(self.bit_length, self.config.n_mm, 8 * max_reference)
            .map_err(|error| error.to_string())?;
        finish_reference(&mut generated, &config, sentinel, Mode::Rfq)
            .map_err(|error| error.to_string())?;
        let reference: Value =
            serde_json::from_str(&generated.reference_json()).map_err(|error| error.to_string())?;
        let mask = json_u64(
            reference
                .get("mask")
                .ok_or_else(|| "generated reference has no mask".to_string())?,
        )?;
        let fill_mask = json_u64(
            reference
                .get("fill_mask")
                .ok_or_else(|| "generated reference has no fill mask".to_string())?,
        )?;

        let round_dir = self.work.0.join(format!("round-{}", self.served));
        fs::create_dir_all(&round_dir).map_err(|error| error.to_string())?;
        let input_prefix = round_dir.join("Input");
        let party_files = generated.party_files();
        let mut node_shares = BTreeMap::new();
        for (party, contents) in party_files.iter().enumerate() {
            fs::write(round_dir.join(format!("Input-P{party}-0")), contents)
                .map_err(|error| error.to_string())?;
            node_shares.insert(
                party,
                contents
                    .split_whitespace()
                    .take(6)
                    .map(low_72_hex)
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let port = free_port_block(self.n_parties)?;
        let hosts = (0..self.n_parties)
            .map(|party| format!("127.0.0.1:{}\n", port + party as u16))
            .collect::<String>();
        for party in 0..self.n_parties {
            fs::write(round_dir.join(format!("hosts-P{party}")), &hosts)
                .map_err(|error| error.to_string())?;
        }

        let started = Instant::now();
        let corruption = corrupt
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let mut children = Vec::new();
        let mut log_paths = Vec::new();
        for party in 0..self.n_parties {
            let log_path = round_dir.join(format!("party-{party}.log"));
            let log = File::create(&log_path).map_err(|error| error.to_string())?;
            let stderr = log.try_clone().map_err(|error| error.to_string())?;
            let mut command = Command::new(&self.binary);
            command
                .current_dir(&self.root)
                .arg(party.to_string())
                .arg(&self.program)
                .args(["-N", &self.n_parties.to_string()])
                .args(["-T", &self.threshold.to_string()]);
            if self.robust {
                command.args(["--options", "robust"]);
            }
            command
                .arg("-ip")
                .arg(round_dir.join(format!("hosts-P{party}")))
                .arg("-IF")
                .arg(&input_prefix)
                // MP-SPDZ prints public output only for party 0 by default.
                // The demo deliberately observes every party so it can fail
                // closed if the opened value ever differs between nodes.
                .args(["-OF", "."])
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr));
            if self.robust && !corruption.is_empty() {
                command.env("QOMM_CORRUPT_PLAYER", &corruption);
            } else {
                command.env_remove("QOMM_CORRUPT_PLAYER");
            }
            children.push(
                command
                    .spawn()
                    .map_err(|error| format!("party {party} did not start: {error}"))?,
            );
            log_paths.push(log_path);
        }
        wait_all(&mut children, Duration::from_secs(300))?;
        let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let logs = log_paths
            .iter()
            .map(|path| fs::read_to_string(path).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let opened = logs.iter().map(|log| opened_key(log)).collect::<Vec<_>>();
        let Some(masked_key) = opened.first().copied().flatten() else {
            return Err("the MP-SPDZ parties emitted no QOMM_MASKED_KEY".into());
        };
        let missing = opened
            .iter()
            .enumerate()
            .filter_map(|(party, value)| value.is_none().then_some(party))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "MP-SPDZ public output was missing for parties {missing:?}"
            ));
        }
        let disagreements = opened
            .iter()
            .enumerate()
            .filter_map(|(party, value)| (*value != Some(masked_key)).then_some((party, *value)))
            .collect::<Vec<_>>();
        if !disagreements.is_empty() {
            return Err(format!(
                "the MP-SPDZ parties disagreed on the opened masked key: party 0={masked_key}, others={disagreements:?}"
            ));
        }
        let opened_fills = logs.iter().map(|log| opened_fill(log)).collect::<Vec<_>>();
        let Some(masked_fill) = opened_fills.first().copied().flatten() else {
            return Err("the MP-SPDZ parties emitted no QOMM_MASKED_FILL".into());
        };
        if opened_fills.iter().any(|value| *value != Some(masked_fill)) {
            return Err("the MP-SPDZ parties disagreed on the opened masked fill".into());
        }
        let references = self
            .references
            .iter()
            .copied()
            .map(|value| value as i64)
            .collect::<Vec<_>>();
        let outcome = evaluate(policies, request, &references, now);
        let (verified, detail, filled) = verify_masked_execution(
            &outcome,
            request,
            settlement.user_limit,
            MaskedExecutionOpening {
                padded_makers: self.config.n_mm,
                masked_key,
                key_mask: mask,
                masked_fill,
                fill_mask,
            },
        )?;
        let named = logs
            .iter()
            .flat_map(|log| corrected_players(log))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|culprit| (culprit, 1))
            .collect();
        self.served += 1;
        Ok(MpcRound {
            outcome,
            filled,
            masked_key,
            mask,
            node_shares,
            named,
            verified,
            detail,
            stats: json!({
                "robust": self.robust,
                "wall_ms": wall_ms,
                "compiled_once_ms": (self.compile_ms * 10.0).round() / 10.0,
                "parties": self.n_parties,
                "binary": self.binary.file_name().and_then(|name| name.to_str()),
            }),
            product_handoff: None,
        })
    }
}

impl MpcQuoteEngine for MpcEngine {
    fn name(&self) -> &'static str {
        MpcEngine::name(self)
    }

    fn note(&self) -> String {
        MpcEngine::note(self)
    }

    fn robust(&self) -> bool {
        MpcEngine::robust(self)
    }

    fn robust_reason(&self) -> &str {
        MpcEngine::robust_reason(self)
    }

    fn input_check(&self) -> bool {
        MpcEngine::input_check(self)
    }

    fn quote(
        &mut self,
        policies: &[Policy],
        request: &Request,
        settlement: &MpcSettlementInputs,
        now: i64,
        corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        MpcEngine::quote(self, policies, request, settlement, now, corrupt)
    }
}

impl Drop for MpcEngine {
    fn drop(&mut self) {
        let _ = fs::remove_file(
            self.root
                .join("Programs/Source")
                .join(format!("{}.mpc", self.program)),
        );
        let _ = fs::remove_file(
            self.root
                .join("Programs/Schedules")
                .join(format!("{}.sch", self.program)),
        );
        let _ = fs::remove_file(self.root.join("Programs/Public-Input").join(&self.program));
        if let Ok(entries) = fs::read_dir(self.root.join("Programs/Bytecode")) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&format!("{}-", self.program)) && name.ends_with(".bc") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn wait_all(children: &mut [Child], timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut done = vec![false; children.len()];
    loop {
        let mut remaining = 0;
        for (party, child) in children.iter_mut().enumerate() {
            if done[party] {
                continue;
            }
            match child.try_wait().map_err(|error| error.to_string())? {
                Some(status) if status.success() => done[party] = true,
                Some(status) => {
                    for child in children.iter_mut() {
                        let _ = child.kill();
                    }
                    return Err(format!(
                        "MP-SPDZ party {party} exited {}",
                        status.code().unwrap_or(-1)
                    ));
                }
                None => remaining += 1,
            }
        }
        if remaining == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            for child in children.iter_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err("MP-SPDZ round exceeded 300 seconds".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn free_port_block(parties: usize) -> Result<u16, String> {
    for _ in 0..128 {
        let upper = 60_000_u16.saturating_sub(parties as u16 + 1);
        let base = rand::random::<u16>() % upper.saturating_sub(21_000).max(1) + 21_000;
        let mut listeners = Vec::new();
        for offset in 0..parties {
            match TcpListener::bind(("127.0.0.1", base + offset as u16)) {
                Ok(listener) => listeners.push(listener),
                Err(_) => break,
            }
        }
        if listeners.len() == parties {
            return Ok(base);
        }
    }
    Err("no free consecutive port block for the MP-SPDZ parties".into())
}

fn opened_key(log: &str) -> Option<i128> {
    log.lines()
        .find_map(|line| line.trim().strip_prefix("QOMM_MASKED_KEY="))?
        .trim()
        .parse()
        .ok()
}

fn opened_fill(log: &str) -> Option<i128> {
    log.lines()
        .find_map(|line| line.trim().strip_prefix("QOMM_MASKED_FILL="))?
        .trim()
        .parse()
        .ok()
}

fn corrected_players(log: &str) -> BTreeSet<usize> {
    const PREFIX: &str = "ROBUST_ATLAS_CORRECTED player ";
    log.lines()
        .filter_map(|line| {
            let rest = line.split_once(PREFIX)?.1;
            rest.split_whitespace().next()?.parse().ok()
        })
        .collect()
}

fn unpack_key(key: i128, padded: usize) -> (i128, usize) {
    let width = padded as i128;
    let maker = key.rem_euclid(width) as usize;
    ((key - maker as i128) / width, maker)
}

fn json_u64(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.to_string().parse().ok())
        .ok_or_else(|| "generated mask is outside the unsigned 64-bit range".to_string())
}

fn low_72_hex(decimal: &str) -> Result<String, String> {
    const MASK: u128 = (1_u128 << 72) - 1;
    let (negative, digits) = decimal
        .strip_prefix('-')
        .map_or((false, decimal), |digits| (true, digits));
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err("MP-SPDZ input is not a decimal integer".into());
    }
    let mut value = 0_u128;
    for digit in digits.bytes() {
        value = (value * 10 + u128::from(digit - b'0')) & MASK;
    }
    if negative {
        value = value.wrapping_neg() & MASK;
    }
    Ok(format!("{value:018x}"))
}

#[cfg(test)]
mod tests {
    use super::{
        corrected_players, is_corporate_queue_pending, low_72_hex, opened_fill, opened_key,
        unpack_key, verify_masked_execution, MaskedExecutionOpening, CORPORATE_QUEUE_RECONCILING,
    };
    use crate::model::{Outcome, Request};

    #[test]
    fn log_and_field_helpers_fail_closed_and_match_the_packing() {
        assert_eq!(opened_key("x\nQOMM_MASKED_KEY=-17\n"), Some(-17));
        assert_eq!(opened_fill("x\nQOMM_MASKED_FILL=19\n"), Some(19));
        assert_eq!(opened_key("QOMM_MASKED_KEY=no"), None);
        assert_eq!(unpack_key(-17, 8), (-3, 7));
        assert_eq!(low_72_hex("-1").unwrap(), "ffffffffffffffffff");
        assert!(low_72_hex("1x").is_err());
        assert_eq!(
            corrected_players(
                "ROBUST_ATLAS_CORRECTED player 4 bad\nROBUST_ATLAS_CORRECTED player 1 bad\nROBUST_ATLAS_CORRECTED player 4 bad"
            )
            .into_iter()
            .collect::<Vec<_>>(),
            vec![1, 4]
        );
    }

    #[test]
    fn canonical_reconciliation_errors_keep_the_corporate_reserve_pending() {
        assert!(is_corporate_queue_pending(&format!(
            "{CORPORATE_QUEUE_RECONCILING}: consensus rejected the settlement"
        )));
        assert!(!is_corporate_queue_pending(
            "consensus rejected before the DeFMI reserve"
        ));
    }

    #[test]
    fn masked_fill_distinguishes_price_limit_rejection_from_a_zero_quote() {
        let mut outcome = Outcome {
            winner: Some(0),
            price: Some(15_912),
            cost: Some(15_912),
            ..Outcome::default()
        };
        let request = Request::default();
        let no_fill = verify_masked_execution(
            &outcome,
            &request,
            15_907,
            MaskedExecutionOpening {
                padded_makers: 4,
                masked_key: 500,
                key_mask: 500,
                masked_fill: 701,
                fill_mask: 701,
            },
        )
        .unwrap();
        assert!(no_fill.0);
        assert!(!no_fill.2);

        outcome.price = Some(0);
        outcome.cost = Some(0);
        let zero_fill = verify_masked_execution(
            &outcome,
            &request,
            15_907,
            MaskedExecutionOpening {
                padded_makers: 4,
                masked_key: 500,
                key_mask: 500,
                masked_fill: 702,
                fill_mask: 701,
            },
        )
        .unwrap();
        assert!(zero_fill.0);
        assert!(zero_fill.2);
    }

    #[test]
    fn sell_limit_is_a_minimum_price() {
        let request = Request {
            direction: 1,
            ..Request::default()
        };
        let outcome = Outcome {
            winner: Some(1),
            price: Some(15_500),
            cost: Some(-15_500),
            ..Outcome::default()
        };
        let rejected = verify_masked_execution(
            &outcome,
            &request,
            15_600,
            MaskedExecutionOpening {
                padded_makers: 4,
                masked_key: 900,
                key_mask: 900,
                masked_fill: 1000,
                fill_mask: 1000,
            },
        )
        .unwrap();
        assert!(rejected.0);
        assert!(!rejected.2);
    }
}
