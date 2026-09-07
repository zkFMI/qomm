use qomm_harness::{parse_value, write_pretty_json, HarnessResult};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const STAGES: [(&str, &str); 4] = [
    ("price", "inputs, reference lookup and price arithmetic"),
    ("direction", "+ direction selection"),
    ("gates", "+ eligibility gates"),
    ("tournament", "+ binary tournament"),
];

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    n_mm: usize,
    n_assets: usize,
    bit_length: usize,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let common = vec![
        "--mode".to_string(),
        "rfq".to_string(),
        "--n-mm".to_string(),
        options.n_mm.to_string(),
        "--n-parties".to_string(),
        "7".to_string(),
        "--threshold".to_string(),
        "2".to_string(),
        "--disclose".to_string(),
        "none".to_string(),
        "--bit-length".to_string(),
        options.bit_length.to_string(),
        "--argmin-arity".to_string(),
        "2".to_string(),
        "--n-assets".to_string(),
        options.n_assets.to_string(),
        "--user-asset".to_string(),
        "0".to_string(),
        "--n-requests".to_string(),
        "1".to_string(),
        "--is-real".to_string(),
        "1".to_string(),
    ];
    let mut rows = Vec::new();
    let mut previous = None;
    for (stage, description) in STAGES {
        let mut row = compile_stage(&options.mp_spdz_root, stage, &common)?;
        let rounds = row["rounds"]
            .as_i64()
            .ok_or("stage compiler returned no round count")?;
        row["description"] = json!(description);
        row["increment"] = previous.map_or(Value::Null, |value| json!(rounds - value));
        previous = Some(rounds);
        let increment = row["increment"]
            .as_i64()
            .map_or_else(|| "---".into(), |value| format!("+{value}"));
        println!("  {description:44} {rounds:4} rounds  {increment:>5}");
        rows.push(row);
    }
    let total = rows
        .last()
        .and_then(|row| row["rounds"].as_i64())
        .ok_or("no stages")?;
    for row in &mut rows {
        row["share_of_rounds"] = row["increment"]
            .as_f64()
            .map(|increment| json!(increment / total as f64))
            .unwrap_or(Value::Null);
    }
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "n_mm": options.n_mm,
        "n_assets": options.n_assets,
        "bit_length": options.bit_length,
        "counted_by": "compiler",
        "total_rounds": total,
        "stages": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn compile_stage(root: &std::path::Path, stage: &str, extra: &[String]) -> HarnessResult<Value> {
    let executable = std::env::current_exe()?
        .parent()
        .ok_or("run_stages executable has no parent")?
        .join("run_qomm");
    let output = Command::new(executable)
        .args(["--prepare-only", "--mp-spdz-root"])
        .arg(root)
        .args(["--stop-after", stage])
        .args(extra)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "stage {stage} did not compile:\n{}",
            tail(&String::from_utf8_lossy(&output.stderr), 3_000)
        )
        .into());
    }
    let payload: Value = serde_json::from_slice(&output.stdout)?;
    let circuit = &payload["circuit"];
    Ok(json!({
        "stage": stage,
        "rounds": circuit["vm_rounds"],
        "opens": circuit["integer_opens"],
        "triples": circuit["integer_triples"],
        "bits": circuit["integer_bits"],
    }))
}

fn tail(text: &str, chars: usize) -> String {
    let values = text.chars().collect::<Vec<_>>();
    values[values.len().saturating_sub(chars)..]
        .iter()
        .collect()
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_default(),
        out: PathBuf::from("artifacts/stages.json"),
        n_mm: 16,
        n_assets: 4,
        bit_length: 31,
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
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
