use qomm_harness::{parse_value, HarnessResult};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    mode: String,
    mms: Vec<usize>,
    delays: Vec<f64>,
    bit_lengths: Vec<usize>,
    arities: Vec<usize>,
    edabits: Vec<String>,
    protocols: Vec<String>,
    repeats: usize,
    n_parties: usize,
    threshold: usize,
}

struct Job {
    n_mm: usize,
    delay_ms: f64,
    bit_length: usize,
    argmin_arity: usize,
    edabit: String,
    protocol: String,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut jobs = Vec::new();
    for &n_mm in &options.mms {
        for &delay_ms in &options.delays {
            for &bit_length in &options.bit_lengths {
                for &argmin_arity in &options.arities {
                    for edabit in &options.edabits {
                        for protocol in &options.protocols {
                            if argmin_arity > n_mm {
                                continue;
                            }
                            jobs.push(Job {
                                n_mm,
                                delay_ms,
                                bit_length,
                                argmin_arity,
                                edabit: edabit.clone(),
                                protocol: protocol.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
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
        payload["edabit"] = json!(job.edabit == "on");
        payload["sweep_seconds"] = json!(started.elapsed().as_secs_f64());
        serde_json::to_writer(&mut handle, &payload)?;
        handle.write_all(b"\n")?;
        handle.flush()?;
        println!(
            "[{index}/{total}] M={} d={}ms bits={} arity={} eda={} {} -> rounds={} mb={} median={} ok={}",
            job.n_mm,
            qomm_harness::value_display(&json!(job.delay_ms)),
            job.bit_length,
            job.argmin_arity,
            job.edabit,
            job.protocol.replace("-party.x", ""),
            get(&payload, "measured_rounds"),
            get(&payload, "measured_mb"),
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
        "mode": options.mode,
        "repeats": options.repeats,
        "n_parties": options.n_parties,
        "threshold": options.threshold,
        "tag": format!("opt-{index}"),
        "n_mm": job.n_mm,
        "delay_ms": job.delay_ms,
        "bit_length": job.bit_length,
        "argmin_arity": job.argmin_arity,
        "protocol": job.protocol,
    });
    let mut command = Command::new(executable);
    command
        .args(["--mp-spdz-root", &options.mp_spdz_root.to_string_lossy()])
        .args(["--mode", &options.mode])
        .args(["--repeats", &options.repeats.to_string()])
        .args(["--n-parties", &options.n_parties.to_string()])
        .args(["--threshold", &options.threshold.to_string()])
        .args(["--tag", &format!("opt-{index}")])
        .args(["--n-mm", &job.n_mm.to_string()])
        .args(["--delay-ms", &job.delay_ms.to_string()])
        .args(["--bit-length", &job.bit_length.to_string()])
        .args(["--argmin-arity", &job.argmin_arity.to_string()])
        .args(["--protocol", &job.protocol]);
    if job.edabit == "on" {
        command.arg("--edabit");
    }
    let Ok(output) = command.output() else {
        let mut payload = kwargs;
        payload["error"] = json!("unparseable");
        payload["stderr"] = json!("could not launch run_qomm");
        return payload;
    };
    let mut payload = match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(payload) => payload,
        Err(_) => {
            let mut payload = kwargs;
            payload["error"] = json!("unparseable");
            payload["stderr"] = json!(tail(&String::from_utf8_lossy(&output.stderr), 1_500));
            payload
        }
    };
    if let Some(circuit) = payload.get_mut("circuit").and_then(Value::as_object_mut) {
        circuit.remove("compile_log");
    }
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
        .map_or_else(|| "None".into(), qomm_harness::value_display)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_default(),
        out: PathBuf::new(),
        mode: "rfq".into(),
        mms: vec![16, 64],
        delays: vec![0.0, 5.0, 15.0],
        bit_lengths: vec![63, 31],
        arities: vec![2, 4, 8],
        edabits: vec!["off".into(), "on".into()],
        protocols: vec!["malicious-shamir-party.x".into()],
        repeats: 5,
        n_parties: 7,
        threshold: 2,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root = PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--mode" => {
                options.mode = value(&raw, &mut index, "--mode")?
                    .to_string_lossy()
                    .into_owned()
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--n-parties" => {
                options.n_parties =
                    parse_value(value(&raw, &mut index, "--n-parties")?, "--n-parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--mms" => {
                options.mms = parse_many(&raw, &mut index, "--mms")?;
                continue;
            }
            "--delays" => {
                options.delays = parse_many(&raw, &mut index, "--delays")?;
                continue;
            }
            "--bit-lengths" => {
                options.bit_lengths = parse_many(&raw, &mut index, "--bit-lengths")?;
                continue;
            }
            "--arities" => {
                options.arities = parse_many(&raw, &mut index, "--arities")?;
                continue;
            }
            "--edabits" => {
                options.edabits = parse_many(&raw, &mut index, "--edabits")?;
                continue;
            }
            "--protocols" => {
                options.protocols = parse_many(&raw, &mut index, "--protocols")?;
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
