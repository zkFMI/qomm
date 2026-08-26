//! Rust port of `scripts/run_multi_asset.py`.

use qomm_harness::{parse_value, write_pretty_json, HarnessResult};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    n_mm: usize,
    bit_length: usize,
    assets: Vec<usize>,
    delay_ms: f64,
    repeats: usize,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.assets.is_empty() {
        return Err("--assets expects at least one value".into());
    }
    println!("== cost of serving A assets from one circuit ==");
    let mut scaling = Vec::new();
    for &assets in &options.assets {
        let row = run_qomm(&options, assets, 0);
        println!(
            "  assets={assets:3}  rounds={}  mb={}  median={}  ok={}",
            get(&row, "measured_rounds"),
            get(&row, "measured_mb"),
            get(&row, "wall_median"),
            get(&row, "verified"),
        );
        scaling.push(row);
    }

    println!("== does the trace change with which asset was requested? ==");
    let assets = *options
        .assets
        .iter()
        .max()
        .expect("non-empty checked above");
    let mut probes = Vec::new();
    for requested in 0..assets.min(8) {
        let row = run_qomm(&options, assets, requested);
        println!(
            "  requested asset {requested}: rounds={} mb={} median={} ok={}",
            get(&row, "measured_rounds"),
            get(&row, "measured_mb"),
            get(&row, "wall_median"),
            get(&row, "verified"),
        );
        probes.push(row);
    }

    let good = probes
        .iter()
        .filter(|probe| probe["verified"] == true)
        .collect::<Vec<_>>();
    let rounds = unique_sorted(&good, "measured_rounds");
    let megabytes = unique_sorted(&good, "measured_mb");
    let times = good
        .iter()
        .filter_map(|probe| probe["wall_median"].as_f64())
        .collect::<Vec<_>>();
    let winners = unique_sorted(&good, "verify_detail");
    let timing_gap = (!times.is_empty()).then(|| {
        times.iter().copied().max_by(f64::total_cmp).unwrap()
            - times.iter().copied().min_by(f64::total_cmp).unwrap()
    });
    let summary = json!({
        "assets_probed": good.len(),
        "identical_rounds": rounds.len() == 1,
        "identical_bytes": megabytes.len() == 1,
        "rounds": rounds,
        "megabytes": megabytes,
        "timing_gap_s": timing_gap,
        "distinct_answers": winners.len(),
        "all_verified": good.len() == probes.len(),
    });
    if let Some(gap) = timing_gap {
        println!(
            "  identical rounds={} bytes={} timing spread={gap:.4}s distinct answers={}",
            py_bool(summary["identical_rounds"].as_bool().unwrap_or(false)),
            py_bool(summary["identical_bytes"].as_bool().unwrap_or(false)),
            summary["distinct_answers"],
        );
    }

    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "config": {
            "mp_spdz_root": options.mp_spdz_root.display().to_string(),
            "out": options.out.display().to_string(),
            "n_mm": options.n_mm,
            "bit_length": options.bit_length,
            "assets": options.assets,
            "delay_ms": options.delay_ms,
            "repeats": options.repeats,
        },
        "scaling": scaling,
        "asset_probes": probes,
        "obliviousness": summary,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn run_qomm(options: &Options, assets: usize, requested: usize) -> Value {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("run_qomm")))
        .unwrap_or_else(|| PathBuf::from("run_qomm"));
    let kwargs = json!({
        "n_mm": options.n_mm,
        "bit_length": options.bit_length,
        "n_assets": assets,
        "user_asset": requested,
        "delay_ms": options.delay_ms,
        "repeats": options.repeats,
        "mode": "rfq",
    });
    let output = Command::new(executable)
        .args([
            OsString::from("--mp-spdz-root"),
            options.mp_spdz_root.as_os_str().to_owned(),
            OsString::from("--n-mm"),
            OsString::from(options.n_mm.to_string()),
            OsString::from("--bit-length"),
            OsString::from(options.bit_length.to_string()),
            OsString::from("--n-assets"),
            OsString::from(assets.to_string()),
            OsString::from("--user-asset"),
            OsString::from(requested.to_string()),
            OsString::from("--delay-ms"),
            OsString::from(options.delay_ms.to_string()),
            OsString::from("--repeats"),
            OsString::from(options.repeats.to_string()),
            OsString::from("--mode"),
            OsString::from("rfq"),
        ])
        .output();
    let Ok(output) = output else {
        let mut error = kwargs;
        error["error"] = json!("unparseable");
        error["stderr"] = json!("could not launch run_qomm");
        return error;
    };
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(mut payload) => {
            if let Some(circuit) = payload.get_mut("circuit").and_then(Value::as_object_mut) {
                circuit.remove("compile_log");
            }
            payload
        }
        Err(_) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let start = stderr
                .char_indices()
                .rev()
                .nth(799)
                .map_or(0, |(index, _)| index);
            let mut error = kwargs;
            error["error"] = json!("unparseable");
            error["stderr"] = json!(&stderr[start..]);
            error
        }
    }
}

fn unique_sorted(rows: &[&Value], key: &str) -> Vec<Value> {
    let mut values = Vec::new();
    for row in rows {
        if !values.contains(&row[key]) {
            values.push(row[key].clone());
        }
    }
    values.sort_by(|left, right| match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        _ => left.to_string().cmp(&right.to_string()),
    });
    values
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        out: PathBuf::new(),
        n_mm: 16,
        bit_length: 31,
        assets: vec![1, 2, 4, 8, 16],
        delay_ms: 1.0,
        repeats: 3,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        let arg = raw[index].to_string_lossy();
        match arg.as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root = PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--n-mm" => options.n_mm = parse_value(value(&raw, &mut index, "--n-mm")?, "--n-mm")?,
            "--bit-length" => {
                options.bit_length =
                    parse_value(value(&raw, &mut index, "--bit-length")?, "--bit-length")?
            }
            "--delay-ms" => {
                options.delay_ms =
                    parse_value(value(&raw, &mut index, "--delay-ms")?, "--delay-ms")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--assets" => {
                options.assets.clear();
                index += 1;
                while index < raw.len() && !raw[index].to_string_lossy().starts_with("--") {
                    options
                        .assets
                        .push(parse_value(raw[index].clone(), "--assets")?);
                    index += 1;
                }
                continue;
            }
            _ => return Err(format!("unknown argument {arg}").into()),
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

fn get(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map_or_else(|| "None".into(), qomm_harness::py_display)
}

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}
