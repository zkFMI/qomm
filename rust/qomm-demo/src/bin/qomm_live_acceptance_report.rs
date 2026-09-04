//! Offline aggregation for the live network acceptance evidence.
//!
//! The per-scenario `judge` command is authoritative for its scenario verdict.
//! This module independently rebuilds the request identity chain, hold/receipt
//! agreement, pool movement and resident-Maker generation accounting from the
//! raw evidence. A disagreement makes the aggregate record fail closed.

use super::{unix_now, Args};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const SCENARIOS: &[&str] = &[
    "expired-seq7",
    "outage-queue",
    "outage-dispatching",
    "abort-1",
    "abort-2",
    "abort-3",
    "concurrent-facility",
    "concurrent-pool",
    "pool-sum",
    "pool-race",
    "pool-replay",
    "restart-all",
];

fn followed(scenario: &str) -> (&'static str, &'static str) {
    match scenario {
        "expired-seq7" => ("01-entry", "01-entry"),
        "outage-queue" => ("02-queued", "05-replayed"),
        "outage-dispatching" => ("02-hold-active", "04-replayed"),
        "abort-1" | "abort-2" | "abort-3" => ("02-hold-active", "05-replayed"),
        "concurrent-facility" => ("04-a-finalized", "04-a-finalized"),
        "concurrent-pool" => ("06-a-finalized", "08-b"),
        "pool-sum" => ("04-a-hold-active", "06-a-finalized"),
        "pool-race" => ("04-a-queued", "08-a-finalized"),
        "pool-replay" => ("04-wait", "04-wait"),
        "restart-all" => ("03-wait", "03-wait"),
        _ => ("00-before", "99-after"),
    }
}

fn rounds(scenario: &str) -> Vec<(&'static str, &'static str, Option<&'static str>)> {
    match scenario {
        "outage-queue" => vec![("00-before", "99-after", Some("05-replayed"))],
        "outage-dispatching" => vec![("00-before", "99-after", Some("04-replayed"))],
        "abort-1" | "abort-2" | "abort-3" => {
            vec![("00-before", "99-after", Some("05-replayed"))]
        }
        "concurrent-facility" => vec![
            ("00-before", "05-after-a", Some("04-a-finalized")),
            ("05-after-a", "99-after", None),
        ],
        "pool-sum" => vec![
            ("02-policies", "07-after-a", Some("06-a-finalized")),
            ("07-after-a", "11-after-b", None),
        ],
        "pool-race" => vec![
            ("02-policies", "09-after-a", Some("08-a-finalized")),
            ("09-after-a", "13-after-b-later", Some("10-b-finalized")),
        ],
        "pool-replay" => vec![
            ("02-policies", "05-after-settle", Some("04-wait")),
            ("05-after-settle", "08-after-replay", None),
        ],
        "restart-all" => vec![("01-after-restart", "99-after", Some("03-wait"))],
        _ => Vec::new(),
    }
}

fn projections(scenario: &str) -> &'static [&'static str] {
    match scenario {
        "outage-queue" => &["01-rfq-while-down"],
        "outage-dispatching" | "abort-1" | "abort-2" | "abort-3" => &["01-rfq"],
        "concurrent-facility" => &["01-rfq-a", "02-rfq-b", "06-rfq-c"],
        "concurrent-pool" => &["03-rfq-a", "04-rfq-b"],
        "pool-sum" => &["03-rfq-a", "05-rfq-b-concurrent", "08-rfq-b"],
        "pool-race" => &["03-rfq-a", "05-rfq-b"],
        "pool-replay" => &["03-rfq"],
        "restart-all" => &["02-rfq"],
        _ => &[],
    }
}

struct Evidence {
    directory: PathBuf,
    files: Vec<String>,
}

impl Evidence {
    fn open(directory: &str) -> Result<Self, String> {
        let directory = PathBuf::from(directory);
        if !directory.is_dir() {
            return Err(format!(
                "evidence directory does not exist: {}",
                directory.display()
            ));
        }
        let mut files = fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .map(|entry| {
                entry
                    .map_err(|error| error.to_string())?
                    .file_name()
                    .into_string()
                    .map_err(|_| "evidence file name is not valid UTF-8".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        files.sort();
        Ok(Self { directory, files })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    fn contains(&self, name: &str) -> bool {
        self.files.iter().any(|item| item == name)
    }

    fn load(&self, scenario: &str, suffix: &str) -> Result<Option<Value>, String> {
        self.load_name(&format!("{scenario}.{suffix}.json"))
    }

    fn load_name(&self, name: &str) -> Result<Option<Value>, String> {
        if !self.contains(name) {
            return Ok(None);
        }
        let path = self.path(name);
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: invalid JSON: {error}", path.display()))
    }

    fn scenario_files(&self, scenario: &str) -> Vec<String> {
        let prefix = format!("{scenario}.");
        self.files
            .iter()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect()
    }

    fn text(&self, name: &str) -> Result<Option<String>, String> {
        if !self.contains(name) {
            return Ok(None);
        }
        let path = self.path(name);
        fs::read_to_string(&path)
            .map(|text| Some(text.trim().to_string()))
            .map_err(|error| format!("{}: {error}", path.display()))
    }
}

fn pointer<'a>(value: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    value.and_then(|document| document.pointer(path))
}

fn clone_or_null(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or(Value::Null)
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut chunk = [0_u8; 1 << 20];
    loop {
        let count = file
            .read(&mut chunk)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn file_record(path: &Path) -> Result<Value, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular evidence file", path.display()));
    }
    Ok(json!({"sha256": sha256_of(path)?, "bytes": metadata.len()}))
}

fn hold_of(snapshot: Option<&Value>, sequence: i64) -> Option<&Value> {
    pointer(snapshot, "/holds")
        .and_then(Value::as_array)
        .and_then(|holds| {
            holds
                .iter()
                .find(|hold| hold.get("sequence").and_then(Value::as_i64) == Some(sequence))
        })
}

fn pool_sequences(snapshot: Option<&Value>) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    if let Some(pools) = pointer(snapshot, "/pools").and_then(Value::as_array) {
        for pool in pools {
            if let (Some(id), Some(sequence)) = (
                pool.get("pool_id").and_then(Value::as_str),
                pool.pointer("/pool/sequence").and_then(Value::as_i64),
            ) {
                out.insert(id.to_string(), sequence);
            }
        }
    }
    out
}

fn node_generations(snapshot: Option<&Value>) -> Vec<Value> {
    pointer(snapshot, "/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| clone_or_null(node.pointer("/maker_state/generation")))
                .collect()
        })
        .unwrap_or_default()
}

fn node_health(snapshot: Option<&Value>) -> Vec<bool> {
    pointer(snapshot, "/nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| node.pointer("/health/ok") == Some(&Value::Bool(true)))
                .collect()
        })
        .unwrap_or_default()
}

fn receipts_for_slot(snapshot: &Value, slot: i64) -> Vec<Vec<Value>> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    node.get("last_receipts")
                        .and_then(Value::as_array)
                        .map(|receipts| {
                            receipts
                                .iter()
                                .filter(|receipt| {
                                    receipt.get("slot").and_then(Value::as_i64) == Some(slot)
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn bindings_of(snapshot: &Value, pool_id: &str) -> Vec<Option<(i64, Value)>> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    node.pointer("/maker_state/bindings")
                        .and_then(Value::as_array)
                        .and_then(|bindings| {
                            bindings.iter().find(|binding| {
                                binding.get("pool_id").and_then(Value::as_str) == Some(pool_id)
                            })
                        })
                        .and_then(|binding| {
                            Some((
                                binding.get("pool_sequence")?.as_i64()?,
                                clone_or_null(binding.get("partial_commitment")),
                            ))
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn selected_fields(value: Option<&Value>, fields: &[&str]) -> Value {
    let mut out = Map::new();
    for field in fields {
        out.insert(
            (*field).to_string(),
            clone_or_null(value.and_then(|document| document.get(*field))),
        );
    }
    Value::Object(out)
}

fn identity(wait: Option<&Value>) -> Value {
    let entry = pointer(wait, "/entry");
    let reconciliation = entry.and_then(|value| value.get("reconciliation"));
    let reservation = entry.and_then(|value| value.get("defmi_reservation"));
    let reservation = reservation
        .filter(|value| value.as_object().is_some_and(|object| !object.is_empty()))
        .map(|value| {
            selected_fields(
                Some(value),
                &[
                    "holdID",
                    "escrowNoteID",
                    "amountCommitment",
                    "settlementDigest",
                    "reserveReceiptDigest",
                    "acceptedHeight",
                    "status",
                ],
            )
        })
        .unwrap_or(Value::Null);
    json!({
        "sequence": clone_or_null(entry.and_then(|value| value.get("sequence"))),
        "request_id": clone_or_null(entry.and_then(|value| value.get("request_id"))),
        "request_digest": clone_or_null(entry.and_then(|value| value.get("request_digest"))),
        "accepted_at": clone_or_null(entry.and_then(|value| value.get("accepted_at"))),
        "expires_at": clone_or_null(entry.and_then(|value| value.get("expires_at"))),
        "outbox_state": clone_or_null(entry.and_then(|value| value.get("outbox_state"))),
        "hold_id": clone_or_null(reconciliation.and_then(|value| value.get("hold_id"))),
        "defmi_status": clone_or_null(reconciliation.and_then(|value| value.get("status"))),
        "canonical_receipt": clone_or_null(reconciliation.and_then(|value| value.get("canonical_receipt"))),
        "defmi_reservation": reservation,
    })
}

fn final_view(snapshot: Option<&Value>, sequence: i64) -> Value {
    hold_of(snapshot, sequence)
        .map(|hold| identity(Some(&json!({"entry": hold}))))
        .unwrap_or_else(|| json!({}))
}

struct GenerationAccounting {
    document: Value,
    accounted: bool,
    advanced_pools: Vec<Value>,
}

fn generation_accounting(
    before: &Value,
    after: &Value,
    fill_commits: i64,
    skip: &[usize],
) -> GenerationAccounting {
    let pools_before = pool_sequences(Some(before));
    let pools_after = pool_sequences(Some(after));
    let newly_bound = pools_after
        .iter()
        .filter(|(pool, _)| !pools_before.contains_key(*pool))
        .map(|(pool, sequence)| json!({"pool_id": pool, "sequence": sequence}))
        .collect::<Vec<_>>();
    let advanced_pools = pools_after
        .iter()
        .filter_map(|(pool, sequence)| {
            pools_before.get(pool).and_then(|before_sequence| {
                (before_sequence != sequence)
                    .then(|| json!({"pool_id": pool, "from": before_sequence, "to": sequence}))
            })
        })
        .collect::<Vec<_>>();
    let generations_before = node_generations(Some(before));
    let generations_after = node_generations(Some(after));
    let deltas = generations_before
        .iter()
        .zip(&generations_after)
        .enumerate()
        .filter(|(index, _)| !skip.contains(index))
        .map(|(_, (before_generation, after_generation))| {
            match (before_generation.as_i64(), after_generation.as_i64()) {
                (Some(before), Some(after)) => json!(after - before),
                _ => Value::Null,
            }
        })
        .collect::<Vec<_>>();
    let expected = i64::try_from(newly_bound.len()).unwrap_or(i64::MAX) + fill_commits;
    let accounted = !deltas.is_empty()
        && node_health(Some(before)).into_iter().any(|healthy| healthy)
        && deltas.iter().all(|delta| delta.as_i64() == Some(expected));
    let document = json!({
        "generations_before": generations_before,
        "generations_after": generations_after,
        "deltas_on_live_nodes": deltas,
        "registration_steps": newly_bound.len(),
        "newly_bound_pools": newly_bound,
        "fill_commit_steps": fill_commits,
        "advanced_pools": advanced_pools,
        "expected_delta": expected,
        "accounted": accounted,
        "explanation": "a generation step is one compare-and-swap on a node's resident Maker state: re-dealing the registration opening of a pool the node had not bound (registration, non-economic; the pool's DeFMI sequence does not move) or the commit that follows a DeFMI settlement (one per fill; the pool sequence moved by one)",
    });
    GenerationAccounting {
        document,
        accounted,
        advanced_pools,
    }
}

fn projection(rfq: Option<&Value>, side: &str, asset: i64) -> Value {
    let path = format!("/{side}/taker/portfolio");
    let portfolio = pointer(rfq, &path);
    let inventory = portfolio
        .and_then(|value| value.get("inventory"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("asset").and_then(Value::as_i64) == Some(asset))
        });
    json!({
        "cash_available": clone_or_null(pointer(portfolio, "/cash/available")),
        "cash_reserved": clone_or_null(pointer(portfolio, "/cash/reserved")),
        "inventory_available": clone_or_null(inventory.and_then(|item| item.get("available"))),
        "inventory_reserved": clone_or_null(inventory.and_then(|item| item.get("reserved"))),
    })
}

fn winning_pool_from_judge(judge: Option<&Value>, label_fragment: &str) -> Option<Value> {
    pointer(judge, "/checks")
        .and_then(Value::as_array)
        .and_then(|checks| {
            checks.iter().find_map(|check| {
                let name = check
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if name.contains(label_fragment) {
                    check.pointer("/observed/winning_pool").cloned()
                } else {
                    None
                }
            })
        })
}

fn scenario_record(evidence: &Evidence, scenario: &str) -> Result<Value, String> {
    let judge = evidence.load(scenario, "judge")?;
    let before = evidence.load(scenario, "00-before")?;
    let after = evidence.load(scenario, "99-after")?;
    let (first_name, last_name) = followed(scenario);
    let first = evidence.load(scenario, first_name)?;
    let last = evidence.load(scenario, last_name)?;
    let request = if pointer(first.as_ref(), "/entry")
        .and_then(Value::as_object)
        .is_some_and(|entry| !entry.is_empty())
    {
        identity(first.as_ref())
    } else {
        json!({})
    };
    let terminal = if pointer(last.as_ref(), "/entry")
        .and_then(Value::as_object)
        .is_some_and(|entry| !entry.is_empty())
    {
        identity(last.as_ref())
    } else {
        json!({})
    };
    let sequence = request.get("sequence").and_then(Value::as_i64);
    let final_snapshot_view = sequence
        .map(|value| final_view(after.as_ref(), value))
        .unwrap_or_else(|| json!({}));
    let mut problems = Vec::<String>::new();

    for name in evidence.scenario_files(scenario) {
        if !name.ends_with(".json") || name.contains(".judge.") {
            continue;
        }
        let prefix_length = scenario.len() + 1;
        let suffix = &name[prefix_length..name.len() - ".json".len()];
        let document = evidence.load(scenario, suffix)?;
        let entry = pointer(document.as_ref(), "/entry");
        if sequence.is_some()
            && entry
                .and_then(|value| value.get("sequence"))
                .and_then(Value::as_i64)
                == sequence
        {
            for key in ["request_id", "request_digest"] {
                if entry.and_then(|value| value.get(key)) != request.get(key) {
                    problems.push(format!("{name}: {key} differs from the followed request"));
                }
            }
        }
    }

    if final_snapshot_view
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        if matches!(
            final_snapshot_view
                .get("outbox_state")
                .and_then(Value::as_str),
            Some("settled" | "released")
        ) {
            let receipt = final_snapshot_view.pointer("/canonical_receipt/transaction_id");
            let settlement = final_snapshot_view.pointer("/defmi_reservation/settlementDigest");
            if receipt.is_none() || receipt != settlement {
                problems.push(
                    "final receipt transaction id differs from the DeFMI settlement digest".into(),
                );
            }
        }
        let first_hold = request.get("hold_id").filter(|value| !value.is_null());
        let final_hold = final_snapshot_view
            .get("hold_id")
            .filter(|value| !value.is_null());
        if first_hold.is_some() && final_hold.is_some() && first_hold != final_hold {
            problems
                .push("hold id changed between the followed record and the final snapshot".into());
        }
    }

    if let Some(after_document) = after.as_ref() {
        let holds = after_document
            .get("holds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(sequence) = sequence {
            let count = holds
                .iter()
                .filter(|hold| hold.get("sequence").and_then(Value::as_i64) == Some(sequence))
                .count();
            if count != 1 {
                problems.push(
                    "the final snapshot does not hold exactly one entry for the sequence".into(),
                );
            }
        }
        let hold_ids = holds
            .iter()
            .filter_map(|hold| {
                hold.pointer("/reconciliation/hold_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        if hold_ids.iter().collect::<BTreeSet<_>>().len() != hold_ids.len() {
            problems.push("two outbox entries share one DeFMI hold in the final snapshot".into());
        }
    }

    let accepted_execution = match (after.as_ref(), sequence) {
        (Some(after), Some(sequence)) => {
            let receipts = receipts_for_slot(after, sequence);
            let selected = receipts
                .iter()
                .map(|per_node| {
                    per_node
                        .iter()
                        .map(|receipt| {
                            selected_fields(
                                Some(receipt),
                                &[
                                    "round_id",
                                    "execution_generation",
                                    "maker_state_generation",
                                    "sequence",
                                    "persistence_sha256",
                                    "input_sha256",
                                    "order_digest",
                                ],
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            json!({
                "slot": sequence,
                "nodes_holding_a_receipt": receipts.iter().filter(|items| !items.is_empty()).count(),
                "receipts": selected,
            })
        }
        _ => Value::Null,
    };

    let mut round_records = Vec::new();
    for (before_name, after_name, wait_name) in rounds(scenario) {
        let round_before = evidence.load(scenario, before_name)?;
        let round_after = evidence.load(scenario, after_name)?;
        let wait = match wait_name {
            Some(name) => evidence.load(scenario, name)?,
            None => None,
        };
        let mut filled = wait_name.is_some()
            && pointer(wait.as_ref(), "/entry/reconciliation/status").and_then(Value::as_str)
                == Some("consumed");
        if scenario == "concurrent-facility" && after_name == "99-after" {
            let start = evidence.load(scenario, "00-before")?;
            let last_sequence = pointer(start.as_ref(), "/next_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                - 1;
            filled = round_after
                .as_ref()
                .and_then(|snapshot| hold_of(Some(snapshot), last_sequence + 2))
                .and_then(|hold| hold.pointer("/reconciliation/status"))
                .and_then(Value::as_str)
                == Some("consumed");
        }
        if scenario == "pool-sum" && after_name == "11-after-b" {
            let start = evidence.load(scenario, "02-policies")?;
            let last_sequence = pointer(start.as_ref(), "/next_sequence")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                - 1;
            filled = round_after
                .as_ref()
                .and_then(|snapshot| hold_of(Some(snapshot), last_sequence + 2))
                .and_then(|hold| hold.pointer("/reconciliation/status"))
                .and_then(Value::as_str)
                == Some("consumed");
        }
        let (Some(round_before), Some(round_after)) = (round_before.as_ref(), round_after.as_ref())
        else {
            problems.push(format!("missing snapshot pair {before_name}/{after_name}"));
            continue;
        };
        let accounting =
            generation_accounting(round_before, round_after, if filled { 1 } else { 0 }, &[]);
        if !accounting.accounted {
            problems.push(format!(
                "generation arithmetic between {before_name} and {after_name} is not accounted for"
            ));
        }
        let pools_before = pool_sequences(Some(round_before));
        let pools_after = pool_sequences(Some(round_after));
        let mut candidates = pools_after
            .iter()
            .filter(|(pool, sequence)| {
                **sequence >= 1
                    && (!pools_before.contains_key(*pool)
                        || pools_before.get(*pool) == Some(&(**sequence - 1)))
            })
            .map(|(pool, _)| pool.clone())
            .collect::<Vec<_>>();
        let mut winner = filled
            .then(|| {
                winning_pool_from_judge(judge.as_ref(), "the fill allocated from exactly one pool")
            })
            .flatten();
        if filled && winner.is_none() && scenario == "pool-race" {
            let index = if after_name == "09-after-a" { 1 } else { 2 };
            let journal = evidence.load(scenario, &format!("11b-journal-{index}"))?;
            if let Some(pool) =
                pointer(journal.as_ref(), "/params/allocation/poolID").and_then(Value::as_str)
            {
                let expected = pointer(journal.as_ref(), "/params/allocation/expectedPoolSequence")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                winner = Some(json!({
                    "pool_id": pool,
                    "sequence": expected + 1,
                    "remainder_note_id": clone_or_null(pointer(journal.as_ref(), "/params/allocation/remainderNote/noteID")),
                    "source": "gateway DeFMI journal",
                }));
                if !candidates.iter().any(|candidate| candidate == pool) {
                    candidates.push(pool.to_string());
                }
            }
        }
        let winner_id = winner
            .as_ref()
            .and_then(|value| value.get("pool_id"))
            .and_then(Value::as_str);
        if filled
            && winner_id
                .map(|pool| !candidates.iter().any(|candidate| candidate == pool))
                .unwrap_or(true)
        {
            problems.push(format!(
                "the judge's winning pool for {after_name} is not a pool that could have advanced once"
            ));
        }
        let journal_source = winner
            .as_ref()
            .and_then(|value| value.get("source"))
            .and_then(Value::as_str)
            == Some("gateway DeFMI journal");
        if filled && !journal_source {
            if let Some(pool) = winner_id {
                let expected = pools_after.get(pool).copied();
                let held = bindings_of(round_after, pool);
                if held
                    .iter()
                    .any(|binding| binding.as_ref().map(|value| value.0) != expected)
                {
                    problems.push(
                        "not every node binds the winning pool at its canonical sequence".into(),
                    );
                }
            }
        }
        if !filled && !accounting.advanced_pools.is_empty() {
            problems.push(format!(
                "a pool sequence advanced between {before_name} and {after_name} without a fill"
            ));
        }
        round_records.push(json!({
            "between": [before_name, after_name],
            "filled": filled,
            "pools_before": pools_before,
            "pools_after": pools_after,
            "winning_pool": winner,
            "generations": accounting.document,
            "taker_facility_before": clone_or_null(round_before.pointer("/taker/portfolio")),
            "taker_facility_after": clone_or_null(round_after.pointer("/taker/portfolio")),
        }));
    }

    let judge_pass = pointer(judge.as_ref(), "/pass").and_then(Value::as_bool) == Some(true);
    let checks = pointer(judge.as_ref(), "/checks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|check| selected_fields(Some(check), &["name", "pass"]))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if judge.is_none() {
        problems.push("no judge verdict recorded".into());
    }

    let mut taker_projection = Map::new();
    for name in projections(scenario) {
        if let Some(document) = evidence.load(scenario, name)? {
            taker_projection.insert(
                (*name).to_string(),
                json!({
                    "before": projection(Some(&document), "before", 0),
                    "after": projection(Some(&document), "after", 0),
                    "status": clone_or_null(document.get("status")),
                    "settlement": clone_or_null(document.get("settlement")),
                }),
            );
        }
    }

    let mut files = Map::new();
    for name in evidence.scenario_files(scenario) {
        files.insert(name.clone(), file_record(&evidence.path(&name))?);
    }
    let pass = judge_pass && problems.is_empty();
    Ok(json!({
        "judge": {
            "pass": judge_pass,
            "recorded_at": clone_or_null(pointer(judge.as_ref(), "/recorded_at")),
            "checks": checks,
        },
        "request": request,
        "terminal_record": terminal,
        "final_snapshot_view": final_snapshot_view,
        "accepted_execution": accepted_execution,
        "rounds": round_records,
        "taker_facility_before": clone_or_null(pointer(before.as_ref(), "/taker/portfolio")),
        "taker_facility_after": clone_or_null(pointer(after.as_ref(), "/taker/portfolio")),
        "taker_projection": taker_projection,
        "outbox_metrics_after": clone_or_null(pointer(after.as_ref(), "/taker/outbox_metrics")),
        "defmi_state_root_after": clone_or_null(pointer(after.as_ref(), "/defmi/state_root/stateRoot")),
        "node_generations_before": node_generations(before.as_ref()),
        "node_generations_after": node_generations(after.as_ref()),
        "pool_sequences_before": pool_sequences(before.as_ref()),
        "pool_sequences_after": pool_sequences(after.as_ref()),
        "report_problems": problems,
        "pass": pass,
        "files": files,
    }))
}

fn split_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn cmd_report(args: &Args) -> Result<Value, String> {
    let directory = args
        .options
        .get("dir")
        .ok_or_else(|| "report requires --dir".to_string())?;
    if !args.options.contains_key("out") {
        return Err("report requires --out".into());
    }
    let evidence = Evidence::open(directory)?;
    let present = SCENARIOS
        .iter()
        .filter(|scenario| {
            let prefix = format!("{scenario}.");
            evidence.files.iter().any(|name| name.starts_with(&prefix))
        })
        .map(|scenario| (*scenario).to_string())
        .collect::<Vec<_>>();
    let selected = split_values(args.texts("scenario"));
    let wanted = if selected.is_empty() {
        present.clone()
    } else {
        selected
    };
    let mut scenarios = Map::new();
    for scenario in &wanted {
        let record = if present.contains(scenario) {
            scenario_record(&evidence, scenario)?
        } else {
            json!({"pass": false, "report_problems": ["no evidence recorded"]})
        };
        scenarios.insert(scenario.clone(), record);
    }
    let explicitly_required = split_values(args.texts("require"));
    let required = if explicitly_required.is_empty() {
        wanted.clone()
    } else {
        explicitly_required
    };
    let overall = required.iter().all(|scenario| {
        scenarios
            .get(scenario)
            .and_then(|record| record.get("pass"))
            .and_then(Value::as_bool)
            == Some(true)
    });

    let mut events = Vec::new();
    if evidence.contains("events.jsonl") {
        let path = evidence.path("events.jsonl");
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<Value>(line) {
                Ok(event) => events.push(event),
                Err(error) => events.push(json!({
                    "unparsable_event_line": line,
                    "error": error.to_string(),
                })),
            }
        }
    }

    let common_names = [
        "events.jsonl",
        "acceptance.log",
        "app-image-id.txt",
        "containers-at-start.txt",
        "app-image-id.first-run.txt",
        "containers-at-start.first-run.txt",
        "findings.json",
        "judge-image-id.txt",
    ];
    let mut common = Map::new();
    for name in common_names {
        if evidence.contains(name) {
            common.insert(name.to_string(), file_record(&evidence.path(name))?);
        }
    }

    let mut runs = Map::new();
    for (label, image_name, containers_name) in [
        (
            "first-run",
            "app-image-id.first-run.txt",
            "containers-at-start.first-run.txt",
        ),
        ("last-run", "app-image-id.txt", "containers-at-start.txt"),
    ] {
        if let Some(app_image_id) = evidence.text(image_name)? {
            let containers = evidence
                .text(containers_name)?
                .map(|text| text.lines().map(str::to_string).collect::<Vec<String>>());
            runs.insert(
                label.to_string(),
                json!({
                    "app_image_id": app_image_id,
                    "containers_at_start": containers,
                }),
            );
        }
    }

    let findings = evidence.load_name("findings.json")?.unwrap_or(Value::Null);
    let attempts = evidence
        .files
        .iter()
        .filter(|name| name.contains('-') && evidence.path(name).is_dir())
        .cloned()
        .collect::<Vec<_>>();
    let mut superseded = Map::new();
    for attempt in attempts {
        let mut names = fs::read_dir(evidence.path(&attempt))
            .map_err(|error| format!("{}: {error}", evidence.path(&attempt).display()))?
            .map(|entry| {
                entry
                    .map_err(|error| error.to_string())?
                    .file_name()
                    .into_string()
                    .map_err(|_| "attempt file name is not valid UTF-8".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        superseded.insert(attempt, json!(names));
    }

    let evidence_dir = fs::canonicalize(&evidence.directory)
        .map_err(|error| format!("{}: {error}", evidence.directory.display()))?;
    Ok(json!({
        "generated_at": unix_now(),
        "evidence_dir": evidence_dir,
        "app_image_id": evidence.text("app-image-id.txt")?,
        "containers_at_start": evidence.text("containers-at-start.txt")?.map(|text| text.lines().map(str::to_string).collect::<Vec<_>>()),
        "required_scenarios": required,
        "pass": overall,
        "judge_image_id": evidence.text("judge-image-id.txt")?,
        "runs": runs,
        "scenarios": scenarios,
        "findings": findings,
        "superseded_attempts": superseded,
        "events": events,
        "files": common,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_and_comma_separated_filters_are_flattened() {
        assert_eq!(
            split_values(vec!["abort-1, abort-2".into(), "restart-all".into()]),
            vec!["abort-1", "abort-2", "restart-all"]
        );
    }

    #[test]
    fn projection_uses_the_requested_asset() {
        let document = json!({
            "before": {"taker": {"portfolio": {
                "cash": {"available": 11, "reserved": 3},
                "inventory": [
                    {"asset": 0, "available": 5, "reserved": 1},
                    {"asset": 2, "available": 9, "reserved": 4}
                ]
            }}}
        });
        assert_eq!(
            projection(Some(&document), "before", 2),
            json!({
                "cash_available": 11,
                "cash_reserved": 3,
                "inventory_available": 9,
                "inventory_reserved": 4,
            })
        );
    }
}
