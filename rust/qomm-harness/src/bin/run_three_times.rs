use ed25519_dalek::SigningKey;
use qomm_audit::receipts::{digest, sign_receipt, AuditLedger, SlotSpec, GENESIS};
use qomm_harness::{parse_value, timing_summary, write_pretty_json, HarnessResult};
use qomm_proofs::quote_proof::{MakerWitness, QuoteCircuit, Registered};
use qomm_sim::deterministic_random::DeterministicRng;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const SENTINEL: i64 = 1 << 20;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    n_mm: usize,
    bit_length: usize,
    delays: Vec<f64>,
    slots: u64,
    nodes: u32,
    quorum: usize,
    rfs_interval_ms: f64,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut values = DeterministicRng::new(5);
    let mut rng = OsRng;
    let makers = (0..options.n_mm)
        .map(|_| MakerWitness {
            ask_level: 100_000 + values.randint(-15, 15),
            spread: values.randint(10, 80),
            slope: *values.choice(&[0, 1, 2]),
            invcoef: *values.choice(&[0, 1, 2]),
            inv: values.randint(0, 50),
            maxqty: *values.choice(&[200, 400]),
            expiry: 1_000 + values.randint(1, 600),
            active: true,
            blindings: Registered::fresh(&mut rng),
        })
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    for &delay in &options.delays {
        let samples = (0..options.slots)
            .map(|slot| one_slot(&options, delay, slot, &makers, &mut rng))
            .collect::<Vec<_>>();
        let good = samples
            .iter()
            .filter(|sample| sample.get("error").is_none())
            .collect::<Vec<_>>();
        if good.is_empty() {
            println!(
                "  delay {delay}ms: all slots failed: {}",
                samples
                    .first()
                    .map_or_else(|| "None".into(), qomm_harness::value_display)
            );
            continue;
        }
        let price = summary_field(&good, "price_ms");
        let proof = summary_field(&good, "proof_ms");
        let settle = summary_field(&good, "settle_ms");
        let total = summary_field(&good, "total_ms");
        let audited_rfs_met =
            total["mean"].as_f64().unwrap_or(f64::INFINITY) <= options.rfs_interval_ms;
        let row = json!({
            "delay_ms": delay,
            "price": price,
            "proof": proof,
            "settle": settle,
            "total": total,
            "proof_verified": good.iter().all(|sample| sample["proof_verified"] == true),
            "receipts_settled": good.iter().all(|sample| sample["receipts_settled"] == true),
            "mpc_rounds": good[0]["mpc_rounds"],
            "audited_rfs_met": audited_rfs_met,
        });
        println!(
            "  {delay}ms one way: priced {} ms | proved +{} ms | settleable +{} ms | total {} ms | meets an audited {}ms RFS slot={}",
            render(&row["price"], 1),
            render(&row["proof"], 1),
            render(&row["settle"], 1),
            render(&row["total"], 1),
            options.rfs_interval_ms,
            py_bool(audited_rfs_met),
        );
        rows.push(row);
    }

    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "config": {
            "mp_spdz_root": options.mp_spdz_root.display().to_string(),
            "out": options.out.display().to_string(),
            "n_mm": options.n_mm,
            "bit_length": options.bit_length,
            "delays": options.delays,
            "slots": options.slots,
            "nodes": options.nodes,
            "quorum": options.quorum,
            "rfs_interval_ms": options.rfs_interval_ms,
        },
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn one_slot(
    options: &Options,
    delay_ms: f64,
    slot: u64,
    makers: &[MakerWitness],
    rng: &mut OsRng,
) -> Value {
    let request = Instant::now();
    let quote = run_qomm(options, delay_ms);
    let price = Instant::now();
    let Ok(quote) = quote else {
        let (stderr, _) = quote.err().unwrap();
        return json!({"error": "mpc failed", "stderr": stderr});
    };
    if quote["verified"] != true {
        return json!({"error": "mpc result did not match the cleartext reference"});
    }

    let circuit = QuoteCircuit::new(24, span_bits(options.n_mm));
    let proved = circuit.prove(
        makers,
        100,
        0,
        1_000,
        SENTINEL,
        options.n_mm as i64,
        b"",
        rng,
        [0u8; 32],
        0,
    );
    let Ok((proof, public)) = proved else {
        return json!({"error": "proof failed"});
    };
    let proof_time = Instant::now();
    let verification = circuit.verify(&proof, &public, b"");
    let proof_verified = verification.is_ok();
    let proof_message = if proof_verified {
        "ok".to_string()
    } else {
        verification
            .err()
            .map(|error| format!("{error:?}"))
            .unwrap_or_else(|| "verification failed".to_string())
    };

    let keys = (0..options.nodes)
        .map(|node| (node, SigningKey::generate(&mut *rng)))
        .collect::<BTreeMap<_, _>>();
    let mut ledger = AuditLedger::new(
        keys.iter()
            .map(|(node, key)| (*node, key.verifying_key()))
            .collect(),
    );
    let maker_names = (0..options.n_mm)
        .map(|index| format!("MM-{index}"))
        .collect::<Vec<_>>();
    let mut parts = vec![b"makers".as_slice()];
    parts.extend(maker_names.iter().map(String::as_bytes));
    let makers_digest = digest(&parts);
    let spec = SlotSpec {
        slot,
        mm_set_digest: makers_digest,
        market_digest: digest(&[
            b"market",
            &u32::try_from(slot).unwrap_or(u32::MAX).to_be_bytes(),
        ]),
        deadline: 10_000,
        required_receipts: options.quorum,
    };
    ledger.open_slot(spec.clone());
    let winner_value = proof.winner_value.to_string();
    let result_digest = digest(&[b"result", winner_value.as_bytes()]);
    let new_state = digest(&[b"state", &result_digest]);
    for node in 0..options.nodes {
        ledger.record(
            sign_receipt(
                &keys[&node],
                node,
                &spec,
                GENESIS,
                new_state,
                result_digest,
                1,
                None,
            ),
            None,
        );
    }
    let settled = ledger.settle(slot, 2);
    let settle_time = Instant::now();
    let (receipts_settled, findings) = match settled {
        Ok((state, findings)) => (state.is_some(), findings.len()),
        Err(_) => (false, 0),
    };
    json!({
        "mpc_rounds": quote["measured_rounds"],
        "mpc_mb": quote["measured_mb"],
        "proof_verified": proof_verified,
        "proof_message": proof_message,
        "receipts_settled": receipts_settled,
        "findings": findings,
        "price_ms": price.duration_since(request).as_secs_f64() * 1e3,
        "proof_ms": proof_time.duration_since(price).as_secs_f64() * 1e3,
        "settle_ms": settle_time.duration_since(proof_time).as_secs_f64() * 1e3,
        "total_ms": settle_time.duration_since(request).as_secs_f64() * 1e3,
    })
}

fn run_qomm(options: &Options, delay_ms: f64) -> Result<Value, (String, String)> {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("run_qomm")))
        .unwrap_or_else(|| PathBuf::from("run_qomm"));
    let output = Command::new(executable)
        .args([
            OsString::from("--mp-spdz-root"),
            options.mp_spdz_root.as_os_str().to_owned(),
            OsString::from("--mode"),
            OsString::from("rfq"),
            OsString::from("--n-mm"),
            OsString::from(options.n_mm.to_string()),
            OsString::from("--bit-length"),
            OsString::from(options.bit_length.to_string()),
            OsString::from("--delay-ms"),
            OsString::from(delay_ms.to_string()),
            OsString::from("--repeats"),
            OsString::from("1"),
        ])
        .output()
        .map_err(|error| (error.to_string(), String::new()))?;
    serde_json::from_slice(&output.stdout).map_err(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let start = stderr
            .char_indices()
            .rev()
            .nth(799)
            .map_or(0, |(index, _)| index);
        (
            stderr[start..].to_string(),
            String::from_utf8_lossy(&output.stdout).into(),
        )
    })
}

fn span_bits(n_slots: usize) -> usize {
    let upper = (SENTINEL as u64) * n_slots as u64 * 2;
    (u64::BITS - upper.leading_zeros()) as usize
}

fn summary_field(rows: &[&Value], field: &str) -> Value {
    timing_summary(
        &rows
            .iter()
            .filter_map(|row| row[field].as_f64())
            .collect::<Vec<_>>(),
    )
}

fn render(summary: &Value, places: usize) -> String {
    let n = summary["n"].as_u64().unwrap_or(0);
    if n == 0 {
        return "—".into();
    }
    let mean = summary["mean"].as_f64().unwrap_or(0.0);
    match summary["sd"].as_f64() {
        Some(sd) => format!("{mean:.places$} ± {sd:.places$} (n={n})"),
        None => format!("{mean:.places$} (n=1)"),
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        out: PathBuf::new(),
        n_mm: 16,
        bit_length: 31,
        delays: vec![1.0, 15.0],
        slots: 5,
        nodes: 7,
        quorum: 5,
        rfs_interval_ms: 1_000.0,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root = PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--n-mm" => options.n_mm = parse_value(value(&raw, &mut index, "--n-mm")?, "--n-mm")?,
            "--bit-length" => {
                options.bit_length =
                    parse_value(value(&raw, &mut index, "--bit-length")?, "--bit-length")?
            }
            "--slots" => {
                options.slots = parse_value(value(&raw, &mut index, "--slots")?, "--slots")?
            }
            "--nodes" => {
                options.nodes = parse_value(value(&raw, &mut index, "--nodes")?, "--nodes")?
            }
            "--quorum" => {
                options.quorum = parse_value(value(&raw, &mut index, "--quorum")?, "--quorum")?
            }
            "--rfs-interval-ms" => {
                options.rfs_interval_ms = parse_value(
                    value(&raw, &mut index, "--rfs-interval-ms")?,
                    "--rfs-interval-ms",
                )?
            }
            "--delays" => {
                options.delays.clear();
                index += 1;
                while index < raw.len() && !raw[index].to_string_lossy().starts_with("--") {
                    options
                        .delays
                        .push(parse_value(raw[index].clone(), "--delays")?);
                    index += 1;
                }
                continue;
            }
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    if options.out.as_os_str().is_empty() {
        return Err("--out is required".into());
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}
