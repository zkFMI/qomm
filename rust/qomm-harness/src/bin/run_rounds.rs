use qomm_harness::{parse_value, write_pretty_json, HarnessResult};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    n_mm: usize,
    n_assets: usize,
    bit_length: usize,
    delay_ms: f64,
    repeats: usize,
    batches: Vec<usize>,
    requests: Vec<usize>,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut payload = Map::new();
    payload.insert(
        "config".into(),
        json!({
            "mp_spdz_root": options.mp_spdz_root.display().to_string(),
            "out": options.out.display().to_string(),
            "n_mm": options.n_mm,
            "n_assets": options.n_assets,
            "bit_length": options.bit_length,
            "delay_ms": options.delay_ms,
            "repeats": options.repeats,
            "batches": options.batches,
            "requests": options.requests,
        }),
    );

    println!("== preprocessing batch size ==");
    let mut batch_rows = Vec::new();
    for &batch in &options.batches {
        let row = run_qomm(&options, &[("--batch-size", batch.to_string())], &[]);
        println!(
            "  batch={batch:6}  rounds={}  mb={}  median={}",
            get(&row, "measured_rounds"),
            get(&row, "measured_mb"),
            get(&row, "wall_median"),
        );
        batch_rows.push(row);
    }
    payload.insert("batch".into(), Value::Array(batch_rows));

    println!("== trimming work out of the gate layer ==");
    let gate_cases = [
        ("baseline", Vec::<&str>::new()),
        ("public maker assets", vec!["--public-maker-assets"]),
        ("gates moved to the audit", vec!["--audit-gates"]),
        ("both", vec!["--public-maker-assets", "--audit-gates"]),
    ];
    let mut gate_rows = Vec::new();
    for (label, flags) in gate_cases {
        let mut row = run_qomm(&options, &[], &flags);
        row["label"] = json!(label);
        println!(
            "  {label:26} rounds={}  mb={}  median={}",
            get(&row, "measured_rounds"),
            get(&row, "measured_mb"),
            get(&row, "wall_median"),
        );
        gate_rows.push(row);
    }
    payload.insert("gates".into(), Value::Array(gate_rows));

    println!("== batching requests into one job ==");
    let mut batching_rows = Vec::new();
    for &requests in &options.requests {
        let mut row = run_qomm(
            &options,
            &[("--n-requests", requests.to_string())],
            &["--public-maker-assets", "--audit-gates"],
        );
        if row["verified"].as_bool().unwrap_or(false) {
            row["rounds_per_quote"] = row["measured_rounds"]
                .as_f64()
                .map(|value| json!(value / requests as f64))
                .unwrap_or(Value::Null);
            row["ms_per_quote"] = row["wall_median"]
                .as_f64()
                .map(|value| json!(value / requests as f64 * 1_000.0))
                .unwrap_or(Value::Null);
        }
        println!(
            "  Q={requests:3}  rounds={}  rounds/quote={}  ms/quote={}",
            get(&row, "measured_rounds"),
            get(&row, "rounds_per_quote"),
            get(&row, "ms_per_quote"),
        );
        batching_rows.push(row);
    }
    payload.insert("batching".into(), Value::Array(batching_rows));
    write_pretty_json(Some(&options.out), &Value::Object(payload))?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn run_qomm(options: &Options, extra: &[(&str, String)], flags: &[&str]) -> Value {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("run_qomm")))
        .unwrap_or_else(|| PathBuf::from("run_qomm"));
    let mut command = Command::new(executable);
    command
        .args(["--mp-spdz-root", &options.mp_spdz_root.to_string_lossy()])
        .args(["--n-mm", &options.n_mm.to_string()])
        .args(["--n-assets", &options.n_assets.to_string()])
        .args(["--bit-length", &options.bit_length.to_string()])
        .args(["--delay-ms", &options.delay_ms.to_string()])
        .args(["--repeats", &options.repeats.to_string()]);
    for (name, value) in extra {
        command.arg(name).arg(value);
    }
    command.args(flags);
    let Ok(output) = command.output() else {
        return json!({"error": "unparseable", "stderr": "could not launch run_qomm"});
    };
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(mut payload) => {
            if let Some(circuit) = payload.get_mut("circuit").and_then(Value::as_object_mut) {
                circuit.remove("compile_log");
            }
            payload
        }
        Err(_) => {
            json!({"error": "unparseable", "stderr": tail(&String::from_utf8_lossy(&output.stderr), 600)})
        }
    }
}

fn tail(text: &str, chars: usize) -> String {
    let values = text.chars().collect::<Vec<_>>();
    values[values.len().saturating_sub(chars)..]
        .iter()
        .collect()
}

fn get(value: &Value, key: &str) -> String {
    value
        .get(key)
        .map_or_else(|| "None".into(), qomm_harness::value_display)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_default(),
        out: PathBuf::new(),
        n_mm: 16,
        n_assets: 4,
        bit_length: 31,
        delay_ms: 15.0,
        repeats: 3,
        batches: vec![10_000, 1_000, 100],
        requests: vec![1, 2, 4, 8, 16, 32],
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
            "--n-assets" => {
                options.n_assets =
                    parse_value(value(&raw, &mut index, "--n-assets")?, "--n-assets")?
            }
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
            "--batches" => {
                options.batches = parse_many(&raw, &mut index, "--batches")?;
                continue;
            }
            "--requests" => {
                options.requests = parse_many(&raw, &mut index, "--requests")?;
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

fn parse_many<T>(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let mut values = Vec::new();
    *index += 1;
    while *index < raw.len() && !raw[*index].to_string_lossy().starts_with("--") {
        values.push(parse_value(raw[*index].clone(), name)?);
        *index += 1;
    }
    Ok(values)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
