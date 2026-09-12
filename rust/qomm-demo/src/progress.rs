//! Public execution metadata. Never store request values, shares, proofs or keys.
//!
//! This lock is independent of the room lock, so a slow RPC cannot prevent the
//! browser from receiving the stage and elapsed time of that very RPC.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default)]
pub struct ExecutionProgress(Arc<Mutex<State>>, Arc<Mutex<Option<Value>>>);

#[derive(Debug, Default)]
struct State {
    run: u64,
    revision: u64,
    active: bool,
    status: &'static str,
    stage: &'static str,
    started: Option<Instant>,
    ended: Option<Instant>,
    stage_started: Option<Instant>,
    nodes: Vec<Node>,
    events: Vec<Value>,
    transfers: Vec<Transfer>,
    started_at_ms: u64,
    history_dir: Option<PathBuf>,
    history: BTreeMap<u64, Value>,
    history_error: bool,
    optimistic: Option<Value>,
}

#[derive(Debug)]
struct Transfer {
    stage: &'static str,
    step: &'static str,
    status: &'static str,
    nodes: BTreeSet<usize>,
    count: u64,
    first_ms: u64,
    last_ms: u64,
}

#[derive(Debug, Default)]
struct Node {
    revision: u64,
    status: &'static str,
    step: &'static str,
    started: Option<Instant>,
    ended: Option<Instant>,
}

impl ExecutionProgress {
    pub fn load_history(&self, directory: &Path) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let mut state = self.0.lock().expect("execution progress lock");
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.extension().and_then(|part| part.to_str()) != Some("json") { continue; }
            let mut value: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("invalid execution history: {error}"))?;
            let run = value["run"].as_u64().ok_or("execution history is missing its run id")?;
            if value["active"] == true {
                value["active"] = json!(false);
                value["status"] = json!("interrupted");
            }
            state.run = state.run.max(run);
            state.history.insert(run, value);
        }
        state.history_dir = Some(directory.to_path_buf());
        Ok(())
    }

    pub fn history(&self, run: u64) -> Option<Value> {
        self.0.lock().expect("execution progress lock").history.get(&run).cloned()
    }

    pub fn provisional(&self, value:Value) { *self.1.lock().expect("private progress lock")=Some(value); }
    pub fn private_provisional(&self)->Option<Value>{ self.1.lock().expect("private progress lock").clone() }

    pub fn start(&self, nodes: usize) {
        *self.1.lock().expect("private progress lock")=None;
        let mut state = self.0.lock().expect("execution progress lock");
        let now = Instant::now();
        let history_dir = state.history_dir.clone();
        let history = std::mem::take(&mut state.history);
        *state = State {
            run: state.run + 1,
            revision: state.revision + 1,
            active: true,
            status: "running",
            stage: "policy",
            started: Some(now),
            stage_started: Some(now),
            started_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            history_dir,
            history,
            nodes: (0..nodes)
                .map(|_| Node {
                    status: "waiting",
                    step: "waiting",
                    ..Node::default()
                })
                .collect(),
            ..State::default()
        };
        archive(&mut state);
    }

    pub fn stage(&self, stage: &'static str) {
        let mut state = self.0.lock().expect("execution progress lock");
        if state.active && state.stage != stage {
            state.stage = stage;
            state.stage_started = Some(Instant::now());
            state.revision += 1;
        }
    }

    pub fn optimistic_claim(&self, claim: &zkpi_committee::optimistic::Claim) {
        let mut state = self.0.lock().expect("execution progress lock");
        state.optimistic = Some(json!({
            "claim_id":claim.proposal.id().map(hex::encode).ok(),
            "challenge_deadline":claim.challenge_deadline,
            "status":claim.status,
        }));
        state.revision += 1;
        archive(&mut state);
    }

    pub fn node_started(&self, node: usize, step: &'static str) -> Option<u64> {
        let mut state = self.0.lock().expect("execution progress lock");
        if !state.active || node >= state.nodes.len() {
            return None;
        }
        state.revision += 1;
        let revision = state.revision;
        state.nodes[node] = Node {
            revision,
            status: "running",
            step,
            started: Some(Instant::now()),
            ended: None,
        };
        record_transfer(&mut state, node, step, "sent");
        Some(revision)
    }

    pub fn node_finished(&self, node: usize, revision: Option<u64>, status: &'static str) {
        let Some(revision) = revision else {
            return;
        };
        let mut state = self.0.lock().expect("execution progress lock");
        if !state.active {
            return;
        }
        if let Some(entry) = state.nodes.get_mut(node) {
            // A delayed response from an earlier call must not replace a newer call.
            if entry.revision == revision {
                entry.status = status;
                entry.ended = Some(Instant::now());
                let step = entry.step;
                state.revision += 1;
                record_transfer(&mut state, node, step, if status == "done" { "received" } else { status });
            }
        }
    }

    pub fn finish(&self, succeeded: bool) {
        let mut state = self.0.lock().expect("execution progress lock");
        if !state.active {
            return;
        }
        let now = Instant::now();
        state.active = false;
        state.status = if succeeded { "done" } else { "failed" };
        state.ended = Some(now);
        state.revision += 1;
        for entry in &mut state.nodes {
            if entry.status == "running" {
                entry.status = "interrupted";
                entry.ended = Some(now);
            }
        }
        archive(&mut state);
    }

    pub fn snapshot(&self) -> Value {
        let state = self.0.lock().expect("execution progress lock");
        let mut snapshot = state_snapshot(&state);
        snapshot["runs"] = json!(state.history.values().rev().map(|run| json!({
            "run": run["run"], "started_at_ms": run["started_at_ms"],
            "status": if run["run"] == state.run && state.active { &snapshot["status"] } else { &run["status"] },
            "elapsed_ms": if run["run"] == state.run && state.active { &snapshot["elapsed_ms"] } else { &run["elapsed_ms"] },
        })).collect::<Vec<_>>());
        snapshot["history_error"] = json!(state.history_error);
        snapshot
    }
}

fn state_snapshot(state: &State) -> Value {
        let now = state.ended.unwrap_or_else(Instant::now);
        let elapsed = |started: Option<Instant>, ended: Option<Instant>| {
            started
                .map(|start| {
                    ended
                        .unwrap_or(now)
                        .saturating_duration_since(start)
                        .as_millis() as u64
                })
                .unwrap_or(0)
        };
        json!({
            "type": "progress", "run": state.run, "revision": state.revision,
            "active": state.active, "status": state.status, "stage": state.stage,
            "started_at_ms": state.started_at_ms,
            "elapsed_ms": elapsed(state.started, state.ended),
            "stage_ms": elapsed(state.stage_started, state.ended),
            "events": state.events,
            "optimistic": state.optimistic,
            "transfers": state.transfers.iter().map(|entry| json!({
                "stage": entry.stage, "step": entry.step, "status": entry.status,
                "nodes": entry.nodes, "count": entry.count,
                "first_ms": entry.first_ms, "last_ms": entry.last_ms,
            })).collect::<Vec<_>>(),
            "nodes": state.nodes.iter().enumerate().map(|(node, entry)| json!({
                "node": node, "status": entry.status, "step": entry.step,
                "elapsed_ms": elapsed(entry.started, entry.ended),
            })).collect::<Vec<_>>()
        })
}

fn archive(state: &mut State) {
    let snapshot = state_snapshot(state);
    state.history.insert(state.run, snapshot.clone());
    if let Some(directory) = &state.history_dir {
        let path = directory.join(format!("run-{}.json", state.run));
        let temporary = directory.join(format!("run-{}.tmp", state.run));
        let saved = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&serde_json::to_vec(&snapshot)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)?;
            std::fs::File::open(directory)?.sync_all()
        })();
        state.history_error = saved.is_err();
        if let Err(error) = saved { eprintln!("execution history could not be saved: {error}"); }
    }
}

fn record_transfer(state: &mut State, node: usize, step: &'static str, status: &'static str) {
    let elapsed_ms = state.started.map(|start| start.elapsed().as_millis() as u64).unwrap_or(0);
    if let Some(entry) = state.transfers.iter_mut().find(|entry| entry.stage == state.stage && entry.step == step && entry.status == status) {
        entry.nodes.insert(node);
        entry.count += 1;
        entry.last_ms = elapsed_ms;
    } else {
        state.transfers.push(Transfer {
            stage: state.stage, step, status, nodes: BTreeSet::from([node]),
            count: 1, first_ms: elapsed_ms, last_ms: elapsed_ms,
        });
    }
    state.events.push(json!({
        "id": state.revision, "at_ms": elapsed_ms, "node": node,
        "step": step, "status": status,
        "from": if status == "sent" { "gateway".to_string() } else { format!("node:{node}") },
        "to": if status == "sent" { format!("node:{node}") } else { "gateway".to_string() },
    }));
    if state.events.len() > 96 {
        state.events.remove(0);
    }
}

/// Only fixed, public protocol labels leave the coordinator.
pub fn node_step(path: &str, request: &Value) -> &'static str {
    match path {
        "/v1/admit" => "admission",
        "/v1/execute" => "mpc",
        "/v1/proof" => match request.get("method").and_then(Value::as_str).unwrap_or("") {
            "frost_status" => "public_keys",
            "frost_identity"
            | "frost_configure_peers"
            | "frost_confirm_peers"
            | "frost_dkg_round1"
            | "frost_dkg_round2"
            | "frost_dkg_finalize" => "prepare",
            "frost_commit" | "frost_sign" | "authorize_typed"
            | "authorize_zkpi" | "authorize_reserve_payment" | "authorize_reserve_typed"
            | "authorize_standing_pool_allocation" | "sign_publication"
            | "sign_admission_attestation" | "sign_execution_attestation" => "signature",
            "quote_evaluations" | "quote_bind" | "quote_relation_bind" | "quote_finalize" | "maker_handle_evaluation" => "quote_proof",
            "zkpi_evaluations" | "zkpi_bind" => "zkpi_proof",
            "limit_evaluations" | "limit_bind" => "limit_proof",
            "pool_remainder_evaluations" | "pool_remainder_bind" => "pool_proof",
            "dvp_evaluations" | "dvp_bind" | "claim_opening_share" => "dvp_proof",
            "complete" | "complete_observer" => "save",
            "load" => "input",
            _ => "proof",
        },
        "/v1/maker-state/commit" => "save",
        _ => "prepare",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_history_survives_restart_and_is_not_replaced_by_a_new_run() {
        let directory = std::env::temp_dir().join(format!("qomm-progress-{}-{}", std::process::id(), rand::random::<u64>()));
        let first = ExecutionProgress::default();
        first.load_history(&directory).unwrap();
        first.start(7);
        let operation = first.node_started(0, "signature");
        first.node_finished(0, operation, "rejected");
        first.finish(true);
        let archived = first.history(1).unwrap();
        assert_eq!(archived["transfers"][1]["status"], "rejected");
        let restarted = ExecutionProgress::default();
        restarted.load_history(&directory).unwrap();
        assert_eq!(restarted.history(1).unwrap(), archived);
        restarted.start(7);
        assert_eq!(restarted.snapshot()["run"], 2);
        assert_eq!(restarted.history(1).unwrap(), archived);
        restarted.finish(false);
        assert_eq!(restarted.history(2).unwrap()["status"], "failed");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn late_response_cannot_finish_a_newer_node_operation() {
        let progress = ExecutionProgress::default();
        progress.start(7);
        let first = progress.node_started(0, "admission");
        let second = progress.node_started(0, "mpc");
        let delayed = progress.clone();
        std::thread::spawn(move || delayed.node_finished(0, first, "done"))
            .join()
            .unwrap();
        let snapshot = progress.snapshot();
        assert_eq!(snapshot["nodes"][0]["status"], "running");
        assert_eq!(snapshot["nodes"][0]["step"], "mpc");
        progress.node_finished(0, second, "failed");
        assert_eq!(progress.snapshot()["nodes"][0]["status"], "failed");
    }

    #[test]
    fn failed_run_retains_its_stage_and_cannot_be_reopened_by_late_responses() {
        let progress = ExecutionProgress::default();
        progress.start(7);
        progress.stage("proof");
        let pending = progress.node_started(2, "proof");
        progress.finish(false);
        progress.node_finished(2, pending, "done");
        progress.stage("settlement");
        let snapshot = progress.snapshot();
        assert_eq!(snapshot["active"], false);
        assert_eq!(snapshot["stage"], "proof");
        assert_eq!(snapshot["status"], "failed");
        assert_eq!(snapshot["nodes"][2]["status"], "interrupted");
        assert_eq!(snapshot["nodes"][0]["status"], "waiting");
        progress.start(7);
        assert_eq!(progress.snapshot()["run"], 2);
        assert_eq!(progress.snapshot()["nodes"][2]["status"], "waiting");
    }
}
