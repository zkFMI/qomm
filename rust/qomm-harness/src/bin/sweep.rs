//! Rust port of `scripts/sweep.py`.

use qomm_harness::{parse_value, HarnessResult};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    repeats: usize,
    mms: Vec<usize>,
    delays: Vec<f64>,
    modes: Vec<String>,
    rfs_steps: usize,
    n_parties: usize,
    threshold: usize,
    with_threshold_disclosure: bool,
}

#[derive(Clone)]
struct Job {
    mode: String,
    n_mm: usize,
    delay_ms: f64,
    disclose: String,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut jobs = Vec::new();
    for mode in &options.modes {
        for &n_mm in &options.mms {
            for &delay_ms in &options.delays {
                jobs.push(Job {
                    mode: mode.clone(),
                    n_mm,
                    delay_ms,
                    disclose: "none".into(),
                });
                if options.with_threshold_disclosure && mode == "rfq" {
                    jobs.push(Job {
                        mode: mode.clone(),
                        n_mm,
                        delay_ms,
                        disclose: "threshold".into(),
                    });
                }
            }
        }
    }
    let total = jobs.len();
    let mut handle = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&options.out)?;
    for (offset, job) in jobs.iter().enumerate() {
        let index = offset + 1;
        let started = Instant::now();
        let mut payload = run_one(&options, job, index);
        payload["sweep_index"] = json!(index);
        payload["sweep_seconds"] = json!(started.elapsed().as_secs_f64());
        serde_json::to_writer(&mut handle, &payload)?;
        handle.write_all(b"\n")?;
        handle.flush()?;
        println!(
            "[{index}/{total}] {} M={} d={}ms {} -> rounds={} median={} verified={}",
            job.mode,
            job.n_mm,
            qomm_harness::py_display(&json!(job.delay_ms)),
            job.disclose,
            get(&payload, "measured_rounds"),
            get(&payload, "wall_median"),
            get(&payload, "verified"),
        );
    }
    Ok(())
}

fn run_one(options: &Options, job: &Job, index: usize) -> Value {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("run_qomm")))
        .unwrap_or_else(|| PathBuf::from("run_qomm"));
    let kwargs = json!({
        "repeats": options.repeats,
        "n_parties": options.n_parties,
        "threshold": options.threshold,
        "rfs_steps": options.rfs_steps,
        "tag": format!("sweep-{index}"),
        "mode": job.mode,
        "n_mm": job.n_mm,
        "delay_ms": job.delay_ms,
        "disclose": job.disclose,
    });
    let output = Command::new(executable)
        .args(["--mp-spdz-root", &options.mp_spdz_root.to_string_lossy()])
        .args(["--repeats", &options.repeats.to_string()])
        .args(["--n-parties", &options.n_parties.to_string()])
        .args(["--threshold", &options.threshold.to_string()])
        .args(["--rfs-steps", &options.rfs_steps.to_string()])
        .args(["--tag", &format!("sweep-{index}")])
        .args(["--mode", &job.mode])
        .args(["--n-mm", &job.n_mm.to_string()])
        .args(["--delay-ms", &job.delay_ms.to_string()])
        .args(["--disclose", &job.disclose])
        .output();
    let Ok(output) = output else {
        let mut payload = kwargs;
        payload["error"] = json!("unparseable");
        payload["stdout"] = json!("");
        payload["stderr"] = json!("could not launch run_qomm");
        payload["circuit"] = json!({});
        payload["returncode"] = json!(-1);
        return payload;
    };
    let mut payload = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(payload) => payload,
        Err(_) => {
            let mut payload = kwargs;
            payload["error"] = json!("unparseable");
            payload["stdout"] = json!(tail(&String::from_utf8_lossy(&output.stdout), 2_000));
            payload["stderr"] = json!(tail(&String::from_utf8_lossy(&output.stderr), 2_000));
            payload
        }
    };
    if payload.get("circuit").is_none() {
        payload["circuit"] = Value::Object(Map::new());
    }
    if let Some(circuit) = payload["circuit"].as_object_mut() {
        circuit.remove("compile_log");
    }
    payload["returncode"] = json!(output.status.code().unwrap_or(-1));
    payload
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
        .map_or_else(|| "None".into(), qomm_harness::py_display)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_default(),
        out: PathBuf::new(),
        repeats: 5,
        mms: vec![4, 8, 16, 32, 64],
        delays: vec![0.0, 1.0, 5.0, 15.0],
        modes: vec!["rfq".into(), "rfm".into(), "rfs".into()],
        rfs_steps: 5,
        n_parties: 7,
        threshold: 2,
        with_threshold_disclosure: false,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root = PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--rfs-steps" => {
                options.rfs_steps =
                    parse_value(value(&raw, &mut index, "--rfs-steps")?, "--rfs-steps")?
            }
            "--n-parties" => {
                options.n_parties =
                    parse_value(value(&raw, &mut index, "--n-parties")?, "--n-parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--with-threshold-disclosure" => options.with_threshold_disclosure = true,
            "--mms" => {
                options.mms = parse_many(&raw, &mut index, "--mms")?;
                continue;
            }
            "--delays" => {
                options.delays = parse_many(&raw, &mut index, "--delays")?;
                continue;
            }
            "--modes" => {
                options.modes = parse_many(&raw, &mut index, "--modes")?;
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
