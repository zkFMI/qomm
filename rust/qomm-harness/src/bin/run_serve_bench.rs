//! Measure cold, resident, and socket-served QOMM quote paths.

use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
use curve25519_dalek::scalar::Scalar;
use qomm_harness::{rustc_version, unique_temp_dir, write_pretty_json, HarnessResult};
use qomm_transport::resident_quote::{serve, CircuitCache, Quote};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

struct Options {
    mp_spdz_root: PathBuf,
    batches: Vec<usize>,
    repeats: usize,
    n_mm: usize,
    n_parties: usize,
    threshold: usize,
    mode: String,
    bit_length: u32,
    delay_ms: f64,
    skip_transport: bool,
    out: PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            mp_spdz_root: home.join("work/qomm/MP-SPDZ"),
            batches: vec![1, 4, 16, 32],
            repeats: 5,
            n_mm: 16,
            n_parties: 7,
            threshold: 2,
            mode: "rfq".into(),
            bit_length: 31,
            delay_ms: 0.0,
            skip_transport: false,
            out: qomm_harness::repo_root().join("artifacts/mpc_resident.json"),
        }
    }
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

fn run() -> HarnessResult<i32> {
    let raw = std::env::args_os().collect::<Vec<_>>();
    if raw.get(1).and_then(|value| value.to_str()) == Some("__serve") {
        return hidden_server(&raw[2..]);
    }
    let options = parse_args(&raw[1..])?;
    let mut result = Map::new();
    result.insert("host".into(), json!(zkfmi_measure::hosts::this_host()));
    result.insert("rustc".into(), json!(rustc_version()));
    result.insert("protocol".into(), json!("malicious-shamir-party.x"));
    result.insert("n_parties".into(), json!(options.n_parties));
    result.insert("threshold".into(), json!(options.threshold));
    result.insert("n_mm".into(), json!(options.n_mm));
    result.insert("mode".into(), json!(options.mode));
    result.insert("bit_length".into(), json!(options.bit_length));
    result.insert("repeats".into(), json!(options.repeats));

    let calibration_us = calibration(200);
    result.insert(
        "calibration".into(),
        json!({"scalar_mult_us": calibration_us}),
    );
    println!("calibration: scalar mult {calibration_us:.1} us");

    result.insert("cold".into(), Value::Array(cold_arm(&options)?));
    result.insert("resident".into(), Value::Array(resident_arm(&options)?));
    if !options.skip_transport {
        let transport = transport_arm(&options, 3)?;
        println!(
            "  socket adds {:.2} ms",
            transport["socket_ms"].as_f64().unwrap_or(f64::NAN)
        );
        result.insert("transport".into(), transport);
    }
    write_pretty_json(Some(&options.out), &Value::Object(result))?;
    println!("wrote {}", options.out.display());
    Ok(0)
}

fn calibration(repeats: usize) -> f64 {
    let scalar = Scalar::from(12_345_u64);
    let mut samples = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let started = Instant::now();
        std::hint::black_box(RISTRETTO_BASEPOINT_POINT * scalar);
        samples.push(started.elapsed().as_secs_f64() * 1e6);
    }
    median(&samples)
}

fn request_for(batch: usize, options: &Options) -> Value {
    json!({
        "n_mm": options.n_mm,
        "n_parties": options.n_parties,
        "threshold": options.threshold,
        "mode": options.mode,
        "bit_length": options.bit_length,
        "n_requests": batch,
        "delay_ms": options.delay_ms,
    })
}

fn cold_arm(options: &Options) -> HarnessResult<Vec<Value>> {
    let mut rows = Vec::new();
    for &batch in &options.batches {
        let request = request_for(batch, options);
        let mut samples = Vec::new();
        let mut compiles = Vec::new();
        for _ in 0..options.repeats {
            let workdir = unique_temp_dir("qomm-cold")?;
            let mut cache = CircuitCache::new(&options.mp_spdz_root, workdir, None)?;
            let started = Instant::now();
            let mut result = cache.quote(&request)?;
            result.wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
            compiles.push(result.compiled_once_ms);
            samples.push(result);
        }
        let row = summarise(batch, &samples, median(&compiles))?;
        println!(
            "  cold     batch {batch:3}  {:8.1} ms/quote",
            row["ms_per_quote"].as_f64().unwrap_or(f64::NAN)
        );
        rows.push(row);
    }
    Ok(rows)
}

fn resident_arm(options: &Options) -> HarnessResult<Vec<Value>> {
    let workdir = unique_temp_dir("qomm-resident")?;
    let mut cache = CircuitCache::new(&options.mp_spdz_root, workdir, None)?;
    let mut rows = Vec::new();
    for &batch in &options.batches {
        let request = request_for(batch, options);
        cache.quote(&request)?;
        let compile_ms = cache.compile_ms(&request)?;
        let mut samples = Vec::new();
        for _ in 0..options.repeats {
            let started = Instant::now();
            let mut result = cache.quote(&request)?;
            result.wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
            samples.push(result);
        }
        let row = summarise(batch, &samples, compile_ms)?;
        println!(
            "  resident batch {batch:3}  {:8.1} ms/quote",
            row["ms_per_quote"].as_f64().unwrap_or(f64::NAN)
        );
        rows.push(row);
    }
    Ok(rows)
}

fn summarise(batch: usize, samples: &[Quote], compile_ms: f64) -> HarnessResult<Value> {
    let wall = median(
        &samples
            .iter()
            .map(|sample| sample.wall_ms)
            .collect::<Vec<_>>(),
    );
    let protocol = median(
        &samples
            .iter()
            .map(|sample| sample.protocol_ms)
            .collect::<Vec<_>>(),
    );
    let rounds = median(
        &samples
            .iter()
            .map(|sample| {
                sample
                    .rounds
                    .map(|value| value as f64)
                    .ok_or("MP-SPDZ did not report rounds")
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mb = median(
        &samples
            .iter()
            .map(|sample| sample.mb.ok_or("MP-SPDZ did not report MB"))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(json!({
        "batch": batch,
        "quotes": samples.len() * batch,
        "wall_ms": wall,
        "ms_per_quote": wall / batch as f64,
        "protocol_ms_per_quote": protocol / batch as f64,
        "overhead_ms_per_quote": (wall - protocol) / batch as f64,
        "rounds_per_quote": rounds / batch as f64,
        "mb_per_quote": mb / batch as f64,
        "compile_ms": compile_ms,
        "verified": samples.iter().all(|sample| sample.verified),
    }))
}

fn transport_arm(options: &Options, repeats: usize) -> HarnessResult<Value> {
    let port = 8_899_u16;
    TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| format!("transport port {port} is already in use: {error}"))?;
    let request = request_for(1, options);
    let current = std::env::current_exe()?;
    let mut child = Command::new(current)
        .arg("__serve")
        .args(["--port", &port.to_string()])
        .arg("--mp-spdz-root")
        .arg(&options.mp_spdz_root)
        .args(["--warm", &serde_json::to_string(&request)?])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("service stdout is unavailable")?);
    wait_for_server(&mut child, &mut stdout)?;
    let result = (|| -> HarnessResult<Value> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(600)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(600)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = BufWriter::new(stream);
        let mut samples = Vec::new();
        for _ in 0..repeats {
            let started = Instant::now();
            serde_json::to_writer(&mut writer, &request)?;
            writer.write_all(b"\n")?;
            writer.flush()?;
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err("service closed the socket before replying".into());
            }
            let reply: Value = serde_json::from_str(&line)?;
            let client_ms = started.elapsed().as_secs_f64() * 1_000.0;
            if reply.get("ok") != Some(&Value::Bool(true)) {
                return Err(reply
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("the service refused")
                    .into());
            }
            samples.push((
                client_ms,
                reply["service_ms"]
                    .as_f64()
                    .ok_or("service response has no service_ms")?,
            ));
        }
        let client = median(&samples.iter().map(|sample| sample.0).collect::<Vec<_>>());
        let service = median(&samples.iter().map(|sample| sample.1).collect::<Vec<_>>());
        Ok(json!({
            "client_ms": client,
            "service_ms": service,
            "socket_ms": client - service,
            "repeats": repeats,
        }))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn wait_for_server(
    child: &mut Child,
    stdout: &mut BufReader<impl std::io::Read>,
) -> HarnessResult<()> {
    let deadline = Instant::now() + std::time::Duration::from_secs(300);
    loop {
        if Instant::now() >= deadline {
            return Err("the service did not come up".into());
        }
        let mut line = String::new();
        if stdout.read_line(&mut line)? == 0 {
            return Err("the service exited before it was ready".into());
        }
        println!("    [service] {}", line.trim());
        if line.starts_with("serving on") {
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            return Err("the service exited before it was ready".into());
        }
    }
}

fn hidden_server(raw: &[OsString]) -> HarnessResult<i32> {
    let mut root = None;
    let mut port = 8_899_u16;
    let mut warm = None;
    let mut index = 0;
    while index < raw.len() {
        let name = raw[index].to_string_lossy();
        let take = |index: &mut usize| -> HarnessResult<String> {
            *index += 1;
            raw.get(*index)
                .ok_or_else(|| format!("argument {name} expects one value"))?
                .clone()
                .into_string()
                .map_err(|_| format!("argument {name} is not UTF-8").into())
        };
        match name.as_ref() {
            "--mp-spdz-root" => root = Some(PathBuf::from(take(&mut index)?)),
            "--port" => port = take(&mut index)?.parse()?,
            "--warm" => warm = Some(serde_json::from_str(&take(&mut index)?)?),
            _ => return Err(format!("unknown argument {name}").into()),
        }
        index += 1;
    }
    let root = root.ok_or("--mp-spdz-root is required")?;
    let workdir = unique_temp_dir("qomm-serve-bench")?;
    let mut cache = CircuitCache::new(root, workdir, None)?;
    if let Some(request) = warm {
        let compile_ms = cache.warm(&request)?;
        println!("warmed one shape in {compile_ms:.1} ms");
    }
    serve(cache, "127.0.0.1", port)?;
    Ok(0)
}

fn parse_args(raw: &[OsString]) -> HarnessResult<Options> {
    let mut options = Options::default();
    let mut index = 0;
    while index < raw.len() {
        let argument = raw[index]
            .clone()
            .into_string()
            .map_err(|_| "argument is not valid UTF-8")?;
        if argument == "-h" || argument == "--help" {
            println!("{}", usage());
            std::process::exit(0);
        }
        let (name, attached) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(name, value)| {
                (name, Some(value))
            });
        let take = |index: &mut usize| -> HarnessResult<String> {
            if let Some(value) = attached {
                Ok(value.to_string())
            } else {
                *index += 1;
                raw.get(*index)
                    .ok_or_else(|| format!("argument {name} expects one value"))?
                    .clone()
                    .into_string()
                    .map_err(|_| format!("argument {name} is not valid UTF-8").into())
            }
        };
        match name {
            "--mp-spdz-root" => options.mp_spdz_root = PathBuf::from(take(&mut index)?),
            "--batches" => {
                let mut batches = Vec::new();
                if let Some(value) = attached {
                    batches.push(value.parse()?);
                }
                while raw
                    .get(index + 1)
                    .is_some_and(|next| !next.to_string_lossy().starts_with('-'))
                {
                    index += 1;
                    batches.push(raw[index].to_string_lossy().parse()?);
                }
                if batches.is_empty() {
                    return Err("--batches expects at least one value".into());
                }
                options.batches = batches;
            }
            "--repeats" => options.repeats = take(&mut index)?.parse()?,
            "--n-mm" => options.n_mm = take(&mut index)?.parse()?,
            "--n-parties" => options.n_parties = take(&mut index)?.parse()?,
            "--threshold" => options.threshold = take(&mut index)?.parse()?,
            "--mode" => options.mode = take(&mut index)?,
            "--bit-length" => options.bit_length = take(&mut index)?.parse()?,
            "--delay-ms" => options.delay_ms = take(&mut index)?.parse()?,
            "--skip-transport" => options.skip_transport = true,
            "--out" => options.out = PathBuf::from(take(&mut index)?),
            _ => return Err(format!("unknown argument {name}").into()),
        }
        index += 1;
    }
    if options.batches.contains(&0) {
        return Err("--batches values must be positive".into());
    }
    if options.repeats == 0 {
        return Err("--repeats must be positive".into());
    }
    Ok(options)
}

fn median(values: &[f64]) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    }
}

fn usage() -> &'static str {
    "usage: run_serve_bench [--mp-spdz-root PATH] [--batches N ...] [--repeats N]\n\
     [--n-mm N] [--n-parties N] [--threshold N] [--mode MODE]\n\
     [--bit-length N] [--delay-ms MS] [--skip-transport] [--out PATH]"
}
