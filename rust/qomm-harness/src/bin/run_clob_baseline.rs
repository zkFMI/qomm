//! Same-host control for the existing nine-order MP-SPDZ CLOB fixture.

use qomm_harness::{next_value, parse_value, unique_temp_dir, write_pretty_json, HarnessResult};
use serde_json::{json, Map, Value};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PARTIES: usize = 7;
const THRESHOLD: usize = 2;
const COMPILE_KEYS: [(&str, &str); 4] = [
    ("integer_bits", "integer bits"),
    ("integer_opens", "integer opens"),
    ("integer_triples", "integer triples"),
    ("vm_rounds", "virtual machine rounds"),
];

struct Options {
    mp_spdz_root: PathBuf,
    clob_dir: PathBuf,
    delay_ms: f64,
    repeats: usize,
    out: Option<PathBuf>,
}

struct InstalledRun {
    root: PathBuf,
    program: String,
    run_dir: PathBuf,
    saved_inputs: Vec<(PathBuf, Option<Vec<u8>>)>,
    source_dest: Option<PathBuf>,
    port_base: Option<u16>,
}

struct Execution {
    ok: bool,
    wall_seconds: f64,
    party0_seconds: Option<f64>,
    party0_mb: Option<f64>,
    party0_rounds: Option<u64>,
    global_mb: Option<f64>,
    log: String,
}

impl InstalledRun {
    fn new(root: PathBuf, program: String) -> HarnessResult<Self> {
        Ok(Self {
            root,
            program,
            run_dir: unique_temp_dir("qomm-clob")?,
            saved_inputs: Vec::new(),
            source_dest: None,
            port_base: None,
        })
    }

    fn install(&mut self, clob_dir: &Path) -> HarnessResult<()> {
        let player_data = self.root.join("Player-Data");
        fs::create_dir_all(&player_data)?;
        for party in 0..PARTIES {
            let target = player_data.join(format!("Input-P{party}-0"));
            self.saved_inputs
                .push((target.clone(), fs::read(&target).ok()));
            fs::copy(
                clob_dir.join("inputs_7").join(format!("Input-P{party}-0")),
                &target,
            )?;
            let private_output = player_data.join(format!("Private-Output-P{party}"));
            if private_output.exists() {
                fs::remove_file(private_output)?;
            }
        }
        let source_dest = self
            .root
            .join("Programs/Source")
            .join(format!("{}.mpc", self.program));
        fs::create_dir_all(
            source_dest
                .parent()
                .expect("source destination has a parent"),
        )?;
        fs::copy(clob_dir.join("continuous_clob_7.mpc"), &source_dest)?;
        self.source_dest = Some(source_dest);
        Ok(())
    }

    fn compile(&self) -> HarnessResult<Value> {
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
        let started = Instant::now();
        let output = Command::new(python)
            .current_dir(&self.root)
            .args(["./compile.py", "-F", "128", &self.program])
            .output()?;
        let elapsed = started.elapsed().as_secs_f64();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            return Err(format!("compile failed:\n{}", tail(&text, 4_000)).into());
        }
        let mut stats = Map::new();
        stats.insert("compile_seconds".into(), json!(elapsed));
        for (key, phrase) in COMPILE_KEYS {
            let value = text
                .lines()
                .find_map(|line| parse_number_before(line, phrase));
            stats.insert(key.into(), value.map_or(Value::Null, |value| json!(value)));
        }
        stats.insert("compile_log".into(), json!(tail(&text, 2_000)));
        Ok(Value::Object(stats))
    }

    fn execute(&mut self, delay_ms: f64) -> HarnessResult<Execution> {
        let count = PARTIES * (PARTIES + 2);
        let actual_base = *self
            .port_base
            .get_or_insert_with(|| free_port_block(count, 21_000));
        let proxy_base = actual_base + PARTIES as u16 + 1;
        let proxies = self.write_host_files(actual_base, proxy_base, delay_ms)?;
        eprintln!(
            "run_clob_baseline party port block: {}..{}",
            actual_base,
            actual_base + count as u16 - 1
        );

        let mut proxy = if proxies.is_empty() {
            None
        } else {
            Some(self.start_proxy(delay_ms, &proxies)?)
        };
        let started = Instant::now();
        let mut children = Vec::with_capacity(PARTIES);
        for party in 0..PARTIES {
            let log = File::create(self.run_dir.join(format!("party-{party}.log")))?;
            let stderr = log.try_clone()?;
            let child = Command::new(self.root.join("malicious-shamir-party.x"))
                .arg(party.to_string())
                .arg(&self.program)
                .args(["-N", &PARTIES.to_string(), "-T", &THRESHOLD.to_string()])
                .arg("-ip")
                .arg(self.run_dir.join(format!("hosts-P{party}")))
                .current_dir(&self.root)
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr))
                .spawn()?;
            children.push(child);
        }

        let deadline = Instant::now() + Duration::from_secs(1_800);
        let mut failed = false;
        for child in &mut children {
            loop {
                if let Some(status) = child.try_wait()? {
                    failed |= !status.success();
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill()?;
                    let _ = child.wait();
                    failed = true;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        for child in &mut children {
            if child.try_wait()?.is_none() {
                child.kill()?;
                let _ = child.wait();
                failed = true;
            }
        }
        let wall_seconds = started.elapsed().as_secs_f64();
        if let Some(child) = proxy.as_mut() {
            child.kill()?;
            let _ = child.wait();
        }

        let logs = (0..PARTIES)
            .map(|party| fs::read_to_string(self.run_dir.join(format!("party-{party}.log"))))
            .collect::<Result<Vec<_>, _>>()?;
        let combined = logs
            .iter()
            .enumerate()
            .map(|(party, text)| format!("===== PARTY {party} =====\n{text}"))
            .collect::<Vec<_>>()
            .join("\n");
        let party0 = logs.first().map(String::as_str).unwrap_or("");
        let (party0_mb, party0_rounds) = parse_data_sent(party0).unwrap_or((None, None));
        Ok(Execution {
            ok: !failed,
            wall_seconds,
            party0_seconds: parse_seconds(party0),
            party0_mb,
            party0_rounds,
            global_mb: parse_global_sent(party0),
            log: combined,
        })
    }

    fn write_host_files(
        &self,
        actual_base: u16,
        proxy_base: u16,
        delay_ms: f64,
    ) -> HarnessResult<Vec<Value>> {
        let mut proxies = Vec::new();
        for source in 0..PARTIES {
            let mut lines = String::new();
            for target in 0..PARTIES {
                let port = if delay_ms == 0.0 || source == target {
                    actual_base + target as u16
                } else {
                    let port = proxy_base + (source * PARTIES + target) as u16;
                    proxies.push(json!({
                        "source": source,
                        "target": target,
                        "listen_port": port,
                        "target_port": actual_base + target as u16,
                        "one_way_delay_ms": delay_ms,
                    }));
                    port
                };
                lines.push_str(&format!("127.0.0.1:{port}\n"));
            }
            fs::write(self.run_dir.join(format!("hosts-P{source}")), lines)?;
        }
        Ok(proxies)
    }

    fn start_proxy(&self, delay_ms: f64, proxies: &[Value]) -> HarnessResult<Child> {
        let config = self.run_dir.join("proxy.json");
        let ready = self.run_dir.join("ready.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "one_way_delay_ms": delay_ms,
                "proxies": proxies,
            }))?,
        )?;
        let current = std::env::current_exe()?;
        let executable = std::env::var_os("QOMM_WAN_PROXY")
            .map(PathBuf::from)
            .unwrap_or_else(|| current.with_file_name("wan_proxy"));
        if !executable.is_file() {
            return Err(format!(
                "{} is missing; build qomm-transport --bin wan_proxy for delayed runs",
                executable.display()
            )
            .into());
        }
        let mut child = Command::new(executable)
            .args([
                "--config",
                config.to_str().ok_or("config path is not UTF-8")?,
            ])
            .args(["--ready", ready.to_str().ok_or("ready path is not UTF-8")?])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() && Instant::now() < deadline {
            if child.try_wait()?.is_some() {
                return Err("wan_proxy exited before becoming ready".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            child.kill()?;
            let _ = child.wait();
            return Err("wan_proxy did not become ready".into());
        }
        Ok(child)
    }

    fn cleanup(&mut self) {
        for (target, saved) in self.saved_inputs.drain(..) {
            if let Some(bytes) = saved {
                let _ = fs::write(target, bytes);
            } else {
                let _ = fs::remove_file(target);
            }
        }
        remove_matching(
            &self.root.join("Programs/Bytecode"),
            &format!("{}-", self.program),
            Some(".bc"),
        );
        let _ = fs::remove_file(
            self.root
                .join("Programs/Schedules")
                .join(format!("{}.sch", self.program)),
        );
        let _ = fs::remove_file(self.root.join("Programs/Public-Input").join(&self.program));
        if let Some(source) = self.source_dest.take() {
            let _ = fs::remove_file(source);
        }
        let _ = fs::remove_dir_all(&self.run_dir);
    }
}

impl Drop for InstalledRun {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT")
            .map(PathBuf::from)
            .unwrap_or_default(),
        clob_dir: PathBuf::new(),
        delay_ms: 0.0,
        repeats: 3,
        out: None,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--mp-spdz-root") => {
                options.mp_spdz_root = PathBuf::from(next_value(&mut args, "--mp-spdz-root")?);
            }
            Some("--clob-dir") => {
                options.clob_dir = PathBuf::from(next_value(&mut args, "--clob-dir")?);
            }
            Some("--delay-ms") => {
                options.delay_ms = parse_value(next_value(&mut args, "--delay-ms")?, "--delay-ms")?;
            }
            Some("--repeats") => {
                options.repeats = parse_value(next_value(&mut args, "--repeats")?, "--repeats")?;
            }
            Some("--out") => {
                options.out = Some(PathBuf::from(next_value(&mut args, "--out")?));
            }
            Some("-h" | "--help") => {
                println!("usage: run_clob_baseline [-h] [--mp-spdz-root MP_SPDZ_ROOT] --clob-dir CLOB_DIR [--delay-ms DELAY_MS] [--repeats REPEATS] [--out OUT]");
                std::process::exit(0);
            }
            Some(value) => return Err(format!("unrecognized argument: {value}").into()),
            None => return Err("argument is not valid UTF-8".into()),
        }
    }
    if options.clob_dir.as_os_str().is_empty() {
        return Err("the following arguments are required: --clob-dir".into());
    }
    if !options.delay_ms.is_finite() || options.delay_ms < 0.0 {
        return Err("--delay-ms must be a finite non-negative number".into());
    }
    Ok(options)
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("run_clob_baseline: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> HarnessResult<bool> {
    let options = parse_args()?;
    let root = options.mp_spdz_root.canonicalize()?;
    let clob_dir = options.clob_dir.canonicalize()?;
    let mut installed = InstalledRun::new(root, format!("clob_baseline_{}", std::process::id()))?;
    installed.install(&clob_dir)?;

    let mut circuit = installed.compile()?;
    circuit
        .as_object_mut()
        .expect("compile stats are an object")
        .remove("compile_log");
    let mut result = Map::from_iter([
        (
            "fixture".into(),
            json!("continuous_clob_7 (9 orders, MAX_FILLS=4)"),
        ),
        ("delay_ms".into(), json!(options.delay_ms)),
        ("repeats".into(), json!(options.repeats)),
        ("host".into(), json!(qomm_measure::hosts::this_host())),
        ("n_parties".into(), json!(PARTIES)),
        ("threshold".into(), json!(THRESHOLD)),
        ("circuit".into(), circuit),
    ]);
    let mut samples = Vec::with_capacity(options.repeats);
    let mut verified = true;
    for _ in 0..options.repeats {
        let execution = installed.execute(options.delay_ms)?;
        if !execution.ok {
            result.insert("error".into(), json!("party failure"));
            result.insert("log_tail".into(), json!(tail(&execution.log, 3_000)));
            verified = false;
            break;
        }
        let got = (
            named_u64(&execution.log, "MPC7_MATCH_EVENTS="),
            named_u64(&execution.log, "MPC7_MATCH_VOLUME="),
        );
        verified &= got == (Some(5), Some(7));
        result.insert(
            "verify_detail".into(),
            json!(format!(
                "got=({}, {}) want=(5, 7)",
                py_optional(got.0),
                py_optional(got.1)
            )),
        );
        samples.push(json!({
            "wall_seconds": execution.wall_seconds,
            "party0_seconds": execution.party0_seconds,
            "party0_mb": execution.party0_mb,
            "party0_rounds": execution.party0_rounds,
            "global_mb": execution.global_mb,
        }));
    }
    result.insert("samples".into(), Value::Array(samples.clone()));
    result.insert("verified".into(), json!(verified));
    if !samples.is_empty() {
        let wall = samples
            .iter()
            .filter_map(|sample| sample["wall_seconds"].as_f64())
            .collect::<Vec<_>>();
        let party0 = samples
            .iter()
            .filter_map(|sample| sample["party0_seconds"].as_f64())
            .collect::<Vec<_>>();
        result.insert("wall_median".into(), json!(median(&wall)));
        result.insert(
            "party0_median".into(),
            party0
                .is_empty()
                .then_some(Value::Null)
                .unwrap_or_else(|| json!(median(&party0))),
        );
        result.insert(
            "measured_rounds".into(),
            samples[0]["party0_rounds"].clone(),
        );
        result.insert("measured_mb".into(), samples[0]["party0_mb"].clone());
    }

    let value = Value::Object(result);
    let text = write_pretty_json(options.out.as_deref(), &value)?;
    println!("{text}");
    Ok(verified)
}

fn parse_number_before(line: &str, phrase: &str) -> Option<u64> {
    let at = line.find(phrase)?;
    line[..at]
        .split_whitespace()
        .last()?
        .replace(',', "")
        .parse()
        .ok()
}

fn parse_seconds(text: &str) -> Option<f64> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("Time")?.trim_start();
        let rest = rest.strip_prefix('=')?.trim_start();
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn parse_data_sent(text: &str) -> Option<(Option<f64>, Option<u64>)> {
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("Data sent =")?;
        let (mb, rest) = rest.trim().split_once(" MB in ~")?;
        let rounds = rest.split_whitespace().next()?.replace(',', "");
        Some((mb.trim().parse().ok(), rounds.parse().ok()))
    })
}

fn parse_global_sent(text: &str) -> Option<f64> {
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("Global data sent =")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn named_u64(text: &str, prefix: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix)?.trim().parse().ok())
}

fn py_optional(value: Option<u64>) -> String {
    value.map_or_else(|| "None".into(), |value| value.to_string())
}

fn median(values: &[f64]) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn free_port_block(count: usize, start: u16) -> u16 {
    let mut base = start;
    while (base as usize) < 60_000usize.saturating_sub(count) {
        let listeners = (0..count)
            .map(|offset| TcpListener::bind(("127.0.0.1", base + offset as u16)))
            .collect::<Result<Vec<_>, _>>();
        if listeners.is_ok() {
            return base;
        }
        base = base.saturating_add(200);
    }
    panic!("no free port block after scan")
}

fn remove_matching(directory: &Path, prefix: &str, suffix: Option<&str>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) && suffix.is_none_or(|suffix| name.ends_with(suffix)) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn tail(text: &str, chars: usize) -> String {
    let values = text.chars().collect::<Vec<_>>();
    values[values.len().saturating_sub(chars)..]
        .iter()
        .collect()
}
