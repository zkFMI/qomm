//! End-to-end DP publication acceptance.
//!
//! Seven stock MP-SPDZ parties produce one distributed-noise release. Seven
//! process-isolated proof parties then read only their own private runtime
//! evidence and sign the public statement. A separate governance committee
//! allocates the legal-entity privacy budget. The persistent ledger commits
//! the 3-of-7 certificate, budget debit and replay marker atomically.

use qomm_audit::distributed_dp::DpMechanism;
use qomm_audit::publication::{NodePublicationEvidence, NodeSignature};
use qomm_audit::publication_ledger::{BudgetAllocation, PublicationLedger, PublicationRequest};
use qomm_harness::local_mpc::LocalMpcRun;
use qomm_harness::{unique_temp_dir, write_pretty_json, HarnessResult};
use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;
use zkfmi_crypto::{hybrid::signature::HybridSigner, key::KeyPurpose, traits::Signer};
use zkpi_committee::proof_party::{
    verify_peer_identity, FrostPeerEntry, ProofRequest, ProofResponse,
};

const PARTIES: usize = 7;
const THRESHOLD: usize = 3;
const PROTOCOL: &str = "malicious-shamir";
const BINARY: &str = "malicious-shamir-party.x";

struct Options {
    mp_spdz_root: PathBuf,
    proof_party_bin: PathBuf,
    ledger: PathBuf,
    out: PathBuf,
    epsilon_micros: u64,
    sensitivity: u64,
    support: u16,
    budget_total_micros: u64,
    epoch: u64,
}

struct PartyChild {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl PartyChild {
    fn spawn(binary: &Path, node: u16, root: &Path) -> Result<Self, String> {
        let node_state = root.join("proof-parties").join(format!("node-{node}"));
        let mut child = Command::new(binary)
            .arg("--proof-party")
            .arg("--node")
            .arg(node.to_string())
            .arg("--proof-root")
            .arg(root)
            .arg("--proof-state")
            .arg(node_state.join("state.qps"))
            .arg("--proof-passphrase-file")
            .arg(node_state.join("passphrase.bin"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("failed to start DP publication party {node}: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "publication-party stdin was not created".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "publication-party stdout was not created".to_string())?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let request = ProofRequest {
            id,
            method: method.into(),
            params,
        };
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "publication party is already closed".to_string())?;
        serde_json::to_writer(&mut *stdin, &request).map_err(|error| error.to_string())?;
        stdin.write_all(b"\n").map_err(|error| error.to_string())?;
        stdin.flush().map_err(|error| error.to_string())?;
        let mut line = String::new();
        if self
            .stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?
            == 0
        {
            return Err("publication party closed without a response".into());
        }
        let response: ProofResponse =
            serde_json::from_str(&line).map_err(|_| "publication party returned invalid JSON")?;
        if response.id != id {
            return Err("publication-party response identifier differs".into());
        }
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "publication party rejected the request".into()));
        }
        response
            .result
            .ok_or_else(|| "publication-party response omitted its result".into())
    }

    fn finish(mut self) -> Result<(), String> {
        self.stdin.take();
        let status = self.child.wait().map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("publication party exited with {status}"))
        }
    }
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().into()
}

fn private_write(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())
}

fn last_integer(text: &str) -> Result<i64, String> {
    text.lines()
        .rev()
        .find_map(|line| line.trim().parse::<i64>().ok())
        .ok_or_else(|| "MP-SPDZ party log contains no released integer".into())
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let started = Instant::now();
    let mechanism = DpMechanism::new(options.epsilon_micros, options.sensitivity, options.support)?;
    let source = mechanism.mp_spdz_source(PARTIES, options.budget_total_micros, 0, "published")?;
    let private_root = unique_temp_dir("qomm-dp-publication")?;
    fs::set_permissions(&private_root, fs::Permissions::from_mode(0o700))?;
    let source_path = private_root.join("distributed-publication.mpc");
    fs::write(&source_path, &source)?;

    let counts = (0..PARTIES)
        .map(|party| 10_i64 + party as i64)
        .collect::<Vec<_>>();
    let mut input_commitments = Vec::new();
    for (party, count) in counts.iter().enumerate() {
        let mut salt = [0_u8; 32];
        OsRng.fill_bytes(&mut salt);
        input_commitments.push(hash(&[
            b"QOMM:DP:PRIVATE-INPUT-COMMITMENT:v1",
            &(party as u64).to_be_bytes(),
            &count.to_be_bytes(),
            &salt,
        ]));
    }
    let private_input_commitment = hash(
        &std::iter::once(b"QOMM:DP:INPUT-SET:v1".as_slice())
            .chain(input_commitments.iter().map(<[u8; 32]>::as_slice))
            .collect::<Vec<_>>(),
    );
    let program = format!("qomm_dp_publication_{}", std::process::id());
    let mut mpc = LocalMpcRun::new(
        options.mp_spdz_root.canonicalize()?,
        program,
        PARTIES,
        2,
        PROTOCOL,
        None,
    )?;
    mpc.install(
        &source_path,
        &counts
            .iter()
            .map(|count| format!("{count}\n"))
            .collect::<Vec<_>>(),
    )?;
    let compile_rounds = mpc.compile(128)?;
    // MP-SPDZ normally prints public output only on party 0. `-OF .` is the
    // stock runtime switch that emits the same already-public release on every
    // party, so each publication signer can bind its own local observation.
    let stock = mpc.execute_stock(BINARY, &["-OF".into(), ".".into()])?;
    if stock.party_logs.len() != PARTIES {
        return Err("MP-SPDZ did not return seven node-local runtime logs".into());
    }
    let outputs = stock
        .party_logs
        .iter()
        .map(|log| last_integer(log))
        .collect::<Result<Vec<_>, _>>()?;
    if outputs.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err("MP-SPDZ parties disagree on the DP release".into());
    }
    let output_value = outputs[0];
    let source_digest: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    let rule_digest = hash(&[b"QOMM:DP:ONE-CONTRIBUTION-PER-LEGAL-ENTITY:v1"]);
    let log_digests = stock
        .party_logs
        .iter()
        .map(|log| Sha256::digest(log.as_bytes()).into())
        .collect::<Vec<[u8; 32]>>();
    let transcript_digest = hash(
        &std::iter::once(b"QOMM:DP:MALICIOUS-SHAMIR-TRANSCRIPT:v1".as_slice())
            .chain(std::iter::once(source_digest.as_slice()))
            .chain(std::iter::once(mechanism.digest().as_slice()))
            .chain(log_digests.iter().map(<[u8; 32]>::as_slice))
            .collect::<Vec<_>>(),
    );
    let budget_scope = hash(&[b"QOMM:DP:LEGAL-ENTITY-COHORT:v1"]);
    let operation_id = hash(&[
        b"QOMM:DP:PUBLICATION-OPERATION:v1",
        &options.epoch.to_be_bytes(),
        &transcript_digest,
    ]);
    let request = PublicationRequest {
        operation_id,
        budget_scope,
        venue: "QOMM".into(),
        epoch: options.epoch,
        slot_start: options.epoch.saturating_mul(10),
        slot_end: options.epoch.saturating_mul(10).saturating_add(9),
        source_digest,
        rule_digest,
        private_input_commitment,
        transcript_digest,
        output_name: "request_count".into(),
        output_value,
    };
    for node in 0..PARTIES {
        private_write(
            &private_root.join(format!("dp-publication-P{node}.json")),
            &NodePublicationEvidence {
                version: 1,
                node_id: format!("node-{node}"),
                operation_id,
                epoch: request.epoch,
                slot_start: request.slot_start,
                slot_end: request.slot_end,
                source_digest,
                rule_digest,
                mechanism_digest: mechanism.digest(),
                private_input_commitment,
                transcript_digest,
                output_name: request.output_name.clone(),
                output_value,
            },
        )?;
    }

    let mut parties = (0..PARTIES)
        .map(|node| PartyChild::spawn(&options.proof_party_bin, node as u16, &private_root))
        .collect::<Result<Vec<_>, _>>()?;
    let identity_session = hash(&[b"QOMM:DP:PUBLICATION-IDENTITIES:v1", &operation_id]);
    let mut publication_registry = BTreeMap::new();
    for (node, party) in parties.iter_mut().enumerate() {
        let identity = party.call(
            "frost_identity",
            json!({"session": hex::encode(identity_session)}),
        )?;
        // Pin nothing unsigned: both identity self-signatures (Ed25519 and
        // ML-DSA-65) must cover the v3 identity body, which binds the
        // publication key to this child's FROST identity for this session.
        let entry: FrostPeerEntry = serde_json::from_value(identity)
            .map_err(|_| "proof party emitted a malformed FROST identity")?;
        if entry.party as usize != node + 1 {
            return Err("proof party answered for another FROST party".into());
        }
        let verified = verify_peer_identity(&entry, &identity_session)?;
        if verified.publication_public.len() != 1984 {
            return Err("proof-party publication identity has the wrong width".into());
        }
        publication_registry.insert(format!("node-{node}"), verified.publication_public);
    }

    let governance_keys = (0..PARTIES)
        .map(|node| HybridSigner::generate().map(|key| (format!("governance-{node}"), key)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let governance_registry = governance_keys
        .iter()
        .map(|(node, key)| (node.clone(), key.public_key()))
        .collect();
    let ledger = PublicationLedger::open_with_registries(
        &options.ledger,
        publication_registry,
        THRESHOLD,
        governance_registry,
        THRESHOLD,
    )?;
    let allocation = BudgetAllocation {
        budget_scope,
        venue: request.venue.clone(),
        output_name: request.output_name.clone(),
        total_micros: options.budget_total_micros,
        policy_version: 1,
    };
    let allocation_body = allocation.body()?;
    let allocation_signatures = governance_keys
        .iter()
        .take(THRESHOLD)
        .map(|(node_id, key)| {
            Ok::<_, zkfmi_crypto::error::CryptoError>(NodeSignature {
                node_id: node_id.clone(),
                signature: key.sign(KeyPurpose::AuditCheckpoint, &allocation_body)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    ledger.configure_budget(allocation, &allocation_signatures)?;

    let certificate = ledger.publish_with(request, &mechanism, |statement| {
        parties
            .iter_mut()
            .enumerate()
            .map(|(node, party)| {
                let result = party.call(
                    "sign_publication",
                    json!({
                        "statement": statement,
                        "sensitivity": options.sensitivity,
                        "support": options.support,
                        "evidence_path": format!("dp-publication-P{node}.json"),
                    }),
                )?;
                let node_id = result
                    .get("node_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "publication signer omitted node_id".to_string())?
                    .to_string();
                let raw = hex::decode(
                    result
                        .get("signature")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "publication signer omitted signature".to_string())?,
                )
                .map_err(|_| "publication signature is malformed")?;
                if raw.len() != 3373 {
                    return Err("publication signature has the wrong width".into());
                }
                Ok(NodeSignature {
                    node_id,
                    signature: raw,
                })
            })
            .collect::<Result<Vec<_>, String>>()
    })?;
    let replay_refused = parties[0]
        .call(
            "sign_publication",
            json!({
                "statement": &certificate.statement,
                "sensitivity": options.sensitivity,
                "support": options.support,
                "evidence_path": "dp-publication-P0.json",
            }),
        )
        .is_err();
    let mut altered = certificate.statement.clone();
    altered.operation_id = hash(&[
        b"QOMM:DP:ALTERED-PUBLICATION:v1",
        &certificate.statement.operation_id,
    ]);
    altered.output_value = altered.output_value.saturating_add(1);
    let mismatch_refused = parties[1]
        .call(
            "sign_publication",
            json!({
                "statement": altered,
                "sensitivity": options.sensitivity,
                "support": options.support,
                "evidence_path": "dp-publication-P1.json",
            }),
        )
        .is_err();
    if !replay_refused || !mismatch_refused {
        return Err("publication signer accepted a replay or altered MPC output".into());
    }
    for party in parties {
        party.finish()?;
    }
    let budget_state = ledger
        .budget_state(&budget_scope, "QOMM", "request_count")?
        .ok_or("privacy budget disappeared after publication")?;
    let result = json!({
        "version": 1,
        "passed": true,
        "environment": "7 MP-SPDZ processes and 7 process-isolated publication signers on one host; not a seven-host WAN",
        "mpc": {
            "protocol": PROTOCOL,
            "parties": PARTIES,
            "threshold": 2,
            "compile_rounds": compile_rounds,
            "runtime_rounds": stock.party0_rounds,
            "global_mb": stock.global_mb,
            "wall_seconds": stock.wall_seconds,
        },
        "publication": {
            "operation_id": hex::encode(operation_id),
            "certificate_digest": hex::encode(certificate.digest()?),
            "signatures": certificate.signatures.len(),
            "required_signatures": THRESHOLD,
            "output_name": certificate.statement.output_name,
            "output_value": certificate.statement.output_value,
            "epsilon_micros": certificate.statement.epsilon_micros,
            "delta_numerator": certificate.statement.delta_numerator,
            "delta_denominator": certificate.statement.delta_denominator.to_string(),
            "transcript_digest": hex::encode(transcript_digest),
            "node_replay_refused": replay_refused,
            "altered_output_refused": mismatch_refused,
        },
        "privacy_budget": {
            "scope": hex::encode(budget_scope),
            "total_micros": budget_state.0,
            "spent_micros": budget_state.1,
            "last_epoch": budget_state.2,
            "governance_registry_separate_from_publication_registry": true,
            "atomic_certificate_budget_and_replay_commit": true,
        },
        "elapsed_seconds": started.elapsed().as_secs_f64(),
    });
    write_pretty_json(Some(&options.out), &result)?;
    fs::remove_dir_all(&private_root)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn parse_args() -> HarnessResult<Options> {
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    let required = |name: &str| -> HarnessResult<String> {
        raw.iter()
            .position(|argument| argument == name)
            .and_then(|position| raw.get(position + 1))
            .cloned()
            .ok_or_else(|| format!("{name} is required").into())
    };
    let value = |name: &str, default: &str| -> HarnessResult<String> {
        Ok(raw
            .iter()
            .position(|argument| argument == name)
            .and_then(|position| raw.get(position + 1))
            .cloned()
            .unwrap_or_else(|| default.into()))
    };
    Ok(Options {
        mp_spdz_root: PathBuf::from(required("--mp-spdz-root")?),
        proof_party_bin: PathBuf::from(required("--proof-party-bin")?),
        ledger: PathBuf::from(required("--ledger")?),
        out: PathBuf::from(required("--out")?),
        epsilon_micros: value("--epsilon-micros", "500000")?.parse()?,
        sensitivity: value("--sensitivity", "1")?.parse()?,
        support: value("--support", "16")?.parse()?,
        budget_total_micros: value("--budget-total-micros", "1000000")?.parse()?,
        epoch: value("--epoch", "1")?.parse()?,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("run_distributed_publication: {error}");
        std::process::exit(1);
    }
}
