//! The browser demo's full-circuit MP-SPDZ engine, without a Python runtime.
//!
//! `qomm-mpc` owns program and input generation.  MP-SPDZ remains the protocol
//! implementation: this module compiles one shape, starts all parties for each
//! round, reads the opened masked key, and checks it against the same cleartext
//! model the browser's simulation path uses.

use crate::model::{evaluate, Outcome, Policy, Request};
use qomm_mpc::inputs::{build_inputs, finish_reference, parse_policies, InputConfig};
use qomm_mpc::program::{build_program, pow2_ceil, sentinel_for, CheckMode, Mode, ProgramConfig};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct MpcRound {
    pub outcome: Outcome,
    pub masked_key: i128,
    pub mask: u64,
    pub node_shares: BTreeMap<usize, Vec<String>>,
    pub named: BTreeMap<usize, usize>,
    pub verified: bool,
    pub detail: String,
    pub stats: Value,
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
        let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
        if !root.join("compile.py").is_file() {
            return Err(format!("{} is not an MP-SPDZ checkout", root.display()));
        }
        let atlas = root.join("atlas-party.x");
        let malicious = root.join("malicious-shamir-party.x");
        let robust = atlas.is_file() && n_parties >= 4 * threshold + 1;
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
        let mut config = ProgramConfig::default();
        config.n_mm = padded;
        config.n_parties = n_parties;
        config.n_assets = references.len();
        config.ref_table = references.iter().copied().map(i128::from).collect();
        config.maker_assets = (0..padded)
            .map(|maker| maker % references.len().max(1))
            .collect();
        config.bit_length = bit_length;
        config.input_check = input_check;
        config.check_mode = CheckMode::PerParty;
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
        let compiled = Command::new("python3")
            .current_dir(&root)
            .args(["./compile.py", "-F", "128", &program])
            .output()
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
        now: i64,
        corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        if policies.len() != self.n_makers {
            return Err("the live policy count differs from the compiled shape".into());
        }
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
            user_limit: 100_000,
            check_coefficients: &self.config.check_coefficients,
            check_repeats: self.config.check_repeats,
            policies: Some(&mpc_policies),
            shamir_inputs: false,
            shamir_threshold: self.threshold,
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
        let opened = logs
            .iter()
            .filter_map(|log| opened_key(log))
            .collect::<Vec<_>>();
        let Some(masked_key) = opened.first().copied() else {
            return Err("the MP-SPDZ parties emitted no QOMM_MASKED_KEY".into());
        };
        if opened.len() != self.n_parties || opened.iter().any(|value| *value != masked_key) {
            return Err("the MP-SPDZ parties disagreed on the opened masked key".into());
        }
        let references = self
            .references
            .iter()
            .copied()
            .map(|value| value as i64)
            .collect::<Vec<_>>();
        let outcome = evaluate(policies, request, &references, now);
        let (verified, detail) = if let (Some(cost), Some(winner)) = (outcome.cost, outcome.winner)
        {
            let unpacked = unpack_key(masked_key - i128::from(mask), self.config.n_mm);
            let wanted = (i128::from(cost), winner);
            (
                unpacked == wanted,
                format!("got={unpacked:?} want={wanted:?}"),
            )
        } else {
            (
                reference.get("no_eligible_maker").and_then(Value::as_bool) == Some(true),
                "no eligible maker; the circuit opened its masked sentinel".into(),
            )
        };
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
        })
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
    use super::{corrected_players, low_72_hex, opened_key, unpack_key};

    #[test]
    fn log_and_field_helpers_fail_closed_and_match_the_packing() {
        assert_eq!(opened_key("x\nQOMM_MASKED_KEY=-17\n"), Some(-17));
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
}
