use qomm_audit::receipts::{
    digest, sign_receipt, AuditLedger, BondLedger, Evidence, Fault, SlotSpec, GENESIS,
};
use qomm_harness::{parse_value, write_pretty_json, HarnessResult};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use zkfmi_crypto::{hybrid::signature::HybridSigner, traits::Signer};

#[derive(Clone)]
struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    mode: String,
    n_mm: usize,
    bit_length: usize,
    delay_ms: f64,
    repeats: usize,
    trials: usize,
    seed: u64,
    nodes: u32,
    slots: u64,
    quorum: usize,
    skip_mpc: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "config": {
            "mp_spdz_root": options.mp_spdz_root.display().to_string(),
            "out": options.out.display().to_string(),
            "mode": options.mode,
            "n_mm": options.n_mm,
            "bit_length": options.bit_length,
            "delay_ms": options.delay_ms,
            "repeats": options.repeats,
            "trials": options.trials,
            "seed": options.seed,
            "nodes": options.nodes,
            "slots": options.slots,
            "quorum": options.quorum,
            "skip_mpc": options.skip_mpc,
        }
    });

    if !options.skip_mpc {
        println!("== cover slot vs real slot on the wire ==");
        let summary = indistinguishability(&options)?;
        if let (Some(identical), Some(gap), Some(spread)) = (
            summary.get("identical"),
            summary.get("timing_gap_s").and_then(Value::as_f64),
            summary.get("timing_spread_s").and_then(Value::as_f64),
        ) {
            println!(
                "  identical: {}  timing gap {:.4}s vs spread {:.4}s",
                py_dict(identical),
                gap,
                spread
            );
        }
        payload["indistinguishability"] = summary;
    }

    println!("== audit drill ==");
    let drill = audit_drill(options.nodes, options.slots, options.quorum)?;
    println!(
        "  injected {} faults, detected_all={}, missed={}, wrongful={} (consequential={})",
        drill["injected"].as_array().map_or(0, Vec::len),
        py_bool(drill["detected_all_injected"].as_bool().unwrap_or(false)),
        qomm_harness::value_display(&drill["missed"]),
        drill["wrongful_findings"].as_array().map_or(0, Vec::len),
        drill["consequential_findings"]
            .as_array()
            .map_or(0, Vec::len),
    );
    if let Some(records) = drill["slashing"].as_array() {
        for record in records {
            println!(
                "    slashed node {} slot {} {}: {}",
                record["node"], record["slot"], record["fault"], record["amount"]
            );
        }
    }
    payload["audit_drill"] = drill;
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn run_slot(options: &Options, is_real: u8, seed: u64) -> Value {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("run_qomm")))
        .unwrap_or_else(|| PathBuf::from("run_qomm"));
    let output = Command::new(executable)
        .args([
            OsString::from("--mp-spdz-root"),
            options.mp_spdz_root.as_os_str().to_owned(),
            OsString::from("--mode"),
            OsString::from(&options.mode),
            OsString::from("--n-mm"),
            OsString::from(options.n_mm.to_string()),
            OsString::from("--bit-length"),
            OsString::from(options.bit_length.to_string()),
            OsString::from("--delay-ms"),
            OsString::from(options.delay_ms.to_string()),
            OsString::from("--repeats"),
            OsString::from(options.repeats.to_string()),
            OsString::from("--is-real"),
            OsString::from(is_real.to_string()),
            OsString::from("--seed"),
            OsString::from(seed.to_string()),
        ])
        .output();
    let Ok(output) = output else {
        return json!({"error": "unparseable", "stderr": "could not launch run_qomm", "is_real": is_real});
    };
    let mut payload: Value = match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(_) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = stderr
                .char_indices()
                .rev()
                .nth(1499)
                .map_or(0, |(index, _)| index);
            return json!({"error": "unparseable", "stderr": &stderr[tail..], "is_real": is_real});
        }
    };
    if let Some(circuit) = payload.get_mut("circuit").and_then(Value::as_object_mut) {
        circuit.remove("compile_log");
    }
    payload
}

fn indistinguishability(options: &Options) -> HarnessResult<Value> {
    let mut rows = Vec::new();
    for is_real in [1, 0] {
        for trial in 0..options.trials {
            let row = run_slot(options, is_real, options.seed + trial as u64);
            println!(
                "  slot is_real={is_real} trial={trial}: rounds={} mb={} median={} ok={}",
                display_get(&row, "measured_rounds"),
                display_get(&row, "measured_mb"),
                display_get(&row, "wall_median"),
                display_get(&row, "verified"),
            );
            rows.push(row);
        }
    }
    let real = rows
        .iter()
        .filter(|row| row["is_real"] == 1 && row["verified"] == true)
        .collect::<Vec<_>>();
    let cover = rows
        .iter()
        .filter(|row| row["is_real"] == 0 && row["verified"] == true)
        .collect::<Vec<_>>();
    if real.is_empty() || cover.is_empty() {
        return Ok(json!({"error": "a slot failed to verify", "rows": rows}));
    }

    let real_rounds = unique_fields(&real, &["measured_rounds"]);
    let cover_rounds = unique_fields(&cover, &["measured_rounds"]);
    let real_mb = unique_fields(&real, &["measured_mb"]);
    let cover_mb = unique_fields(&cover, &["measured_mb"]);
    let identical = json!({
        "vm_rounds": unique_fields(&real, &["circuit", "vm_rounds"])
            == unique_fields(&cover, &["circuit", "vm_rounds"]),
        "integer_triples": unique_fields(&real, &["circuit", "integer_triples"])
            == unique_fields(&cover, &["circuit", "integer_triples"]),
        "measured_rounds": real_rounds == cover_rounds,
        "measured_mb": real_mb == cover_mb,
    });
    let real_times = real
        .iter()
        .filter_map(|row| row["wall_median"].as_f64())
        .collect::<Vec<_>>();
    let cover_times = cover
        .iter()
        .filter_map(|row| row["wall_median"].as_f64())
        .collect::<Vec<_>>();
    let real_median = qomm_harness::median(&real_times).ok_or("missing real timing")?;
    let cover_median = qomm_harness::median(&cover_times).ok_or("missing cover timing")?;
    let spread = range(&real_times).max(range(&cover_times));
    let all_identical = identical
        .as_object()
        .is_some_and(|fields| fields.values().all(|value| value == true));
    Ok(json!({
        "identical": identical,
        "all_identical": all_identical,
        "real": {"rounds": real_rounds, "mb": real_mb, "median_s": real_median, "n": real.len()},
        "cover": {"rounds": cover_rounds, "mb": cover_mb, "median_s": cover_median, "n": cover.len()},
        "timing_gap_s": (real_median - cover_median).abs(),
        "timing_spread_s": spread,
        "rows": rows,
    }))
}

fn unique_fields(rows: &[&Value], path: &[&str]) -> Vec<Value> {
    let mut values = Vec::new();
    for row in rows {
        let mut value = *row;
        for key in path {
            value = &value[*key];
        }
        if !values.contains(value) {
            values.push(value.clone());
        }
    }
    values.sort_by(|left, right| match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => left.to_string().cmp(&right.to_string()),
    });
    values
}

fn range(values: &[f64]) -> f64 {
    let min = values.iter().copied().min_by(f64::total_cmp).unwrap_or(0.0);
    let max = values.iter().copied().max_by(f64::total_cmp).unwrap_or(0.0);
    max - min
}

fn audit_drill(n_nodes: u32, n_slots: u64, quorum: usize) -> HarnessResult<Value> {
    let keys = (0..n_nodes)
        .map(|node| HybridSigner::generate().map(|key| (node, key)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut ledger = AuditLedger::new(
        keys.iter()
            .map(|(node, key)| (*node, key.public_key()))
            .collect(),
    );
    let mut bonds = BondLedger::new((0..n_nodes).map(|node| (node, 2_000_000)).collect());
    let names = (0..16)
        .map(|index| format!("MM-{index}"))
        .collect::<Vec<_>>();
    let mut maker_parts = vec![b"makers".as_slice()];
    maker_parts.extend(names.iter().map(String::as_bytes));
    let fixed_makers = digest(&maker_parts);
    let injected = BTreeMap::from([
        ((2u32, 1u64), Fault::Equivocation),
        ((3, 2), Fault::OmittedMakers),
        ((4, 3), Fault::StaleState),
        ((5, 4), Fault::MissingReceipt),
    ]);
    let mut previous = GENESIS;
    let mut timeline = Vec::new();
    for slot in 0..n_slots {
        let market = digest(&[b"market", &u32::try_from(slot)?.to_be_bytes()]);
        let spec = SlotSpec {
            slot,
            mm_set_digest: fixed_makers,
            market_digest: market,
            deadline: 100 * slot + 50,
            required_receipts: quorum,
        };
        ledger.open_slot(spec.clone());
        let result = digest(&[b"result", &u32::try_from(slot)?.to_be_bytes()]);
        let state = digest(&[b"state", &previous, &result]);
        for node in 0..n_nodes {
            let fault = injected.get(&(node, slot)).copied();
            if fault == Some(Fault::MissingReceipt) {
                continue;
            }
            let maker_set = (fault == Some(Fault::OmittedMakers)).then(|| {
                let names = (0..15)
                    .map(|index| format!("MM-{index}"))
                    .collect::<Vec<_>>();
                let mut parts = vec![b"makers".as_slice()];
                parts.extend(names.iter().map(String::as_bytes));
                digest(&parts)
            });
            let parent = if fault == Some(Fault::StaleState) && slot >= 1 {
                GENESIS
            } else {
                previous
            };
            ledger.record(
                sign_receipt(
                    &keys[&node],
                    node,
                    &spec,
                    parent,
                    state,
                    result,
                    100 * slot + 10,
                    maker_set,
                )?,
                None,
            );
            if fault == Some(Fault::Equivocation) {
                let other_result = digest(&[b"result-other"]);
                let other = digest(&[b"state", &previous, &other_result]);
                ledger.record(
                    sign_receipt(
                        &keys[&node],
                        node,
                        &spec,
                        parent,
                        other,
                        other_result,
                        100 * slot + 11,
                        None,
                    )?,
                    None,
                );
            }
        }
        let (settled, found) = ledger.settle(slot, 100 * slot + 60)?;
        if let Some(settled) = settled {
            previous = settled;
        }
        timeline.push(json!({
            "slot": slot,
            "settled": settled.map(|bytes| hex::encode(bytes)[..16].to_string()),
            "new_findings": found.iter().map(evidence_json).collect::<Vec<_>>(),
        }));
    }

    let caught = ledger
        .evidence
        .iter()
        .map(|item| (item.node, item.slot, item.fault))
        .collect::<BTreeSet<_>>();
    let expected = injected
        .iter()
        .map(|((node, slot), fault)| (i64::from(*node), *slot, *fault))
        .collect::<BTreeSet<_>>();
    let guilty_slots = injected.keys().copied().collect::<BTreeSet<_>>();
    let consequential = ledger
        .evidence
        .iter()
        .filter(|item| {
            item.node >= 0
                && !expected.contains(&(item.node, item.slot, item.fault))
                && guilty_slots.contains(&(item.node as u32, item.slot))
        })
        .map(evidence_json)
        .collect::<Vec<_>>();
    let wrongful = ledger
        .evidence
        .iter()
        .filter(|item| item.node >= 0 && !guilty_slots.contains(&(item.node as u32, item.slot)))
        .map(evidence_json)
        .collect::<Vec<_>>();
    let slashing = bonds.apply(&ledger.evidence);
    Ok(json!({
        "nodes": n_nodes,
        "slots": n_slots,
        "quorum": quorum,
        "injected": injected.iter().map(|((node, slot), fault)| json!({
            "node": node, "slot": slot, "fault": fault.as_str()
        })).collect::<Vec<_>>(),
        "detected_all_injected": expected.is_subset(&caught),
        "missed": expected.difference(&caught).map(|(node, slot, fault)| json!({
            "node": node, "slot": slot, "fault": fault.as_str()
        })).collect::<Vec<_>>(),
        "evidence": ledger.evidence.iter().map(evidence_json).collect::<Vec<_>>(),
        "consequential_findings": consequential,
        "wrongful_findings": wrongful,
        "slashing": slashing.iter().map(|item| json!({
            "node": item.node,
            "slot": item.slot,
            "fault": item.fault.as_str(),
            "amount": item.amount,
            "remaining_bond": item.remaining_bond,
        })).collect::<Vec<_>>(),
        "remaining_bonds": bonds.bonds,
        "timeline": timeline,
    }))
}

fn evidence_json(evidence: &Evidence) -> Value {
    json!({
        "fault": evidence.fault.as_str(),
        "node": evidence.node,
        "slot": evidence.slot,
        "detail": evidence.detail,
    })
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        out: PathBuf::new(),
        mode: "rfs".into(),
        n_mm: 16,
        bit_length: 31,
        delay_ms: 1.0,
        repeats: 3,
        trials: 3,
        seed: 7,
        nodes: 7,
        slots: 6,
        quorum: 5,
        skip_mpc: false,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--mp-spdz-root") => {
                options.mp_spdz_root = PathBuf::from(next(&mut args, "--mp-spdz-root")?)
            }
            Some("--out") => options.out = PathBuf::from(next(&mut args, "--out")?),
            Some("--mode") => {
                options.mode = next(&mut args, "--mode")?
                    .into_string()
                    .map_err(|_| "invalid --mode")?
            }
            Some("--n-mm") => options.n_mm = parse_value(next(&mut args, "--n-mm")?, "--n-mm")?,
            Some("--bit-length") => {
                options.bit_length = parse_value(next(&mut args, "--bit-length")?, "--bit-length")?
            }
            Some("--delay-ms") => {
                options.delay_ms = parse_value(next(&mut args, "--delay-ms")?, "--delay-ms")?
            }
            Some("--repeats") => {
                options.repeats = parse_value(next(&mut args, "--repeats")?, "--repeats")?
            }
            Some("--trials") => {
                options.trials = parse_value(next(&mut args, "--trials")?, "--trials")?
            }
            Some("--seed") => options.seed = parse_value(next(&mut args, "--seed")?, "--seed")?,
            Some("--nodes") => options.nodes = parse_value(next(&mut args, "--nodes")?, "--nodes")?,
            Some("--slots") => options.slots = parse_value(next(&mut args, "--slots")?, "--slots")?,
            Some("--quorum") => {
                options.quorum = parse_value(next(&mut args, "--quorum")?, "--quorum")?
            }
            Some("--skip-mpc") => options.skip_mpc = true,
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    if options.out.as_os_str().is_empty() {
        return Err("--out is required".into());
    }
    Ok(options)
}

fn next(args: &mut impl Iterator<Item = OsString>, name: &str) -> HarnessResult<OsString> {
    args.next()
        .ok_or_else(|| format!("{name} expects a value").into())
}

fn display_get(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map_or_else(|| "None".into(), qomm_harness::value_display)
}

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

fn py_dict(value: &Value) -> String {
    qomm_harness::value_display(value)
}

#[allow(dead_code)]
fn _path_display(path: &Path) -> String {
    path.display().to_string()
}
