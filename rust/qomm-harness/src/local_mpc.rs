//! Minimal local MP-SPDZ orchestration shared by harnesses that inspect raw logs.

use crate::{unique_temp_dir, HarnessResult};
use qomm_mpc::Protocol;
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Enter the linked MP-SPDZ party engine when the binary was recursively
/// launched with `__party`; return `false` for the normal harness entrypoint.
pub fn maybe_run_party() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) != Some("__party") {
        return false;
    }
    let Some(protocol_name) = args.get(2) else {
        eprintln!("missing qomm-mpc protocol");
        std::process::exit(2);
    };
    let Some(protocol) = Protocol::parse(protocol_name) else {
        eprintln!("unsupported qomm-mpc protocol {protocol_name}");
        std::process::exit(2);
    };
    let argv = std::iter::once(args[0].as_str())
        .chain(args.iter().skip(3).map(String::as_str))
        .collect::<Vec<_>>();
    match qomm_mpc::run(protocol, &argv) {
        Ok(run) => println!(
            "QOMM total {} {} {} {} {:.6}",
            run.rounds, run.raw_rounds, run.sent, run.payload, run.seconds
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    true
}

pub struct LocalMpcRun {
    root: PathBuf,
    program: String,
    parties: usize,
    threshold: usize,
    protocol: String,
    prime: Option<String>,
    run_dir: PathBuf,
    saved_inputs: Vec<(PathBuf, Option<Vec<u8>>)>,
    source_dest: Option<PathBuf>,
}

impl LocalMpcRun {
    pub fn new(
        root: PathBuf,
        program: impl Into<String>,
        parties: usize,
        threshold: usize,
        protocol: impl Into<String>,
        prime: Option<String>,
    ) -> HarnessResult<Self> {
        let run_dir = unique_temp_dir("qomm-local-mpc")?;
        Ok(Self {
            root,
            program: program.into(),
            parties,
            threshold,
            protocol: protocol.into(),
            prime,
            run_dir,
            saved_inputs: Vec::new(),
            source_dest: None,
        })
    }

    pub fn install(&mut self, source: &Path, party_files: &[String]) -> HarnessResult<()> {
        if party_files.len() != self.parties {
            return Err(format!(
                "expected {} party input files, got {}",
                self.parties,
                party_files.len()
            )
            .into());
        }
        let player_data = self.root.join("Player-Data");
        fs::create_dir_all(&player_data)?;
        for (party, contents) in party_files.iter().enumerate() {
            let target = player_data.join(format!("Input-P{party}-0"));
            self.saved_inputs
                .push((target.clone(), fs::read(&target).ok()));
            fs::write(&target, contents)?;
            let output = player_data.join(format!("Private-Output-P{party}"));
            if output.exists() {
                fs::remove_file(output)?;
            }
        }
        let destination = self
            .root
            .join("Programs/Source")
            .join(format!("{}.mpc", self.program));
        fs::create_dir_all(destination.parent().expect("source has a parent"))?;
        fs::copy(source, &destination)?;
        self.source_dest = Some(destination);
        Ok(())
    }

    pub fn replace_inputs(&self, party_files: &[String]) -> HarnessResult<()> {
        if party_files.len() != self.parties {
            return Err("wrong number of replacement input files".into());
        }
        for (party, contents) in party_files.iter().enumerate() {
            fs::write(
                self.root
                    .join("Player-Data")
                    .join(format!("Input-P{party}-0")),
                contents,
            )?;
        }
        Ok(())
    }

    pub fn compile(&self, field_bits: usize) -> HarnessResult<Option<u64>> {
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
        let output = Command::new(python)
            .current_dir(&self.root)
            .arg("./compile.py")
            .args(["-F", &field_bits.to_string()])
            .arg(&self.program)
            .output()?;
        if !output.status.success() {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Err(format!("compile failed:\n{}", tail(&text, 4_000)).into());
        }
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let rounds = text.lines().find_map(|line| {
            let at = line.find("virtual machine rounds")?;
            line[..at]
                .split_whitespace()
                .last()?
                .replace(',', "")
                .parse()
                .ok()
        });
        Ok(rounds)
    }

    pub fn execute(&self) -> HarnessResult<String> {
        let protocol = Protocol::parse(&self.protocol)
            .ok_or_else(|| format!("unsupported linked protocol {}", self.protocol))?;
        if !self.root.join(protocol.stock_binary()).exists() {
            return Err(format!(
                "{} missing under {}",
                protocol.stock_binary(),
                self.root.display()
            )
            .into());
        }
        let base = free_port_block(self.parties * (self.parties + 2), 21_000);
        for source in 0..self.parties {
            let lines = (0..self.parties)
                .map(|target| format!("127.0.0.1:{}", base + target as u16))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(
                self.run_dir.join(format!("hosts-P{source}")),
                format!("{lines}\n"),
            )?;
        }
        let executable = std::env::current_exe()?;
        let mut children = Vec::new();
        for party in 0..self.parties {
            let log_path = self.run_dir.join(format!("party-{party}.log"));
            let log = File::create(&log_path)?;
            let stderr = log.try_clone()?;
            let mut command = Command::new(&executable);
            command
                .arg("__party")
                .arg(&self.protocol)
                .arg(party.to_string())
                .arg(&self.program)
                .args(["-N", &self.parties.to_string()]);
            if self.protocol.contains("shamir") || self.protocol.contains("atlas") {
                command.args(["-T", &self.threshold.to_string()]);
            }
            if let Some(prime) = &self.prime {
                command.args(["-P", prime]);
            }
            command
                .arg("-ip")
                .arg(self.run_dir.join(format!("hosts-P{party}")))
                .current_dir(&self.root)
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr));
            children.push(command.spawn()?);
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
        let logs = (0..self.parties)
            .map(|party| {
                fs::read_to_string(self.run_dir.join(format!("party-{party}.log")))
                    .map(|text| format!("===== PARTY {party} =====\n{text}"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        if failed {
            return Err(format!("MP-SPDZ party failure:\n{}", tail(&logs, 4_000)).into());
        }
        Ok(logs)
    }

    /// Execute a stock MP-SPDZ party binary. This keeps custom protocol and
    /// preprocessing options available to measurement scripts while the common
    /// malicious/semi-honest arms can use the linked engine above.
    pub fn execute_stock(&self, binary: &str, extra: &[String]) -> HarnessResult<StockRun> {
        let run = self.execute_stock_observed(binary, extra, &[])?;
        if !run.ok {
            return Err(format!("MP-SPDZ party failure:\n{}", tail(&run.combined, 4_000)).into());
        }
        Ok(run)
    }

    /// Execute a stock binary while preserving its logs even when a party
    /// exits unsuccessfully. Fault-injection harnesses need the refusal itself
    /// as an observed result instead of treating it as an orchestration error.
    pub fn execute_stock_observed(
        &self,
        binary: &str,
        extra: &[String],
        environment: &[(String, String)],
    ) -> HarnessResult<StockRun> {
        if !self.root.join(binary).exists() {
            return Err(format!("{binary} missing under {}", self.root.display()).into());
        }
        let base = free_port_block(self.parties * (self.parties + 2), 21_000);
        for source in 0..self.parties {
            let lines = (0..self.parties)
                .map(|target| format!("127.0.0.1:{}", base + target as u16))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(
                self.run_dir.join(format!("hosts-P{source}")),
                format!("{lines}\n"),
            )?;
        }
        let started = Instant::now();
        let mut children = Vec::new();
        for party in 0..self.parties {
            let log_path = self.run_dir.join(format!("party-{party}.log"));
            let log = File::create(&log_path)?;
            let stderr = log.try_clone()?;
            let mut command = Command::new(self.root.join(binary));
            command
                .arg(party.to_string())
                .arg(&self.program)
                .args(["-N", &self.parties.to_string()]);
            if binary.contains("shamir") || binary.contains("atlas") {
                command.args(["-T", &self.threshold.to_string()]);
            }
            command.args(extra);
            command.envs(environment.iter().map(|(key, value)| (key, value)));
            command
                .arg("-ip")
                .arg(self.run_dir.join(format!("hosts-P{party}")))
                .current_dir(&self.root)
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr));
            children.push(command.spawn()?);
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
        let logs = (0..self.parties)
            .map(|party| fs::read_to_string(self.run_dir.join(format!("party-{party}.log"))))
            .collect::<Result<Vec<_>, _>>()?;
        let combined = logs
            .iter()
            .enumerate()
            .map(|(party, text)| format!("===== PARTY {party} =====\n{text}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (party0_mb, party0_rounds) = logs
            .first()
            .and_then(|log| parse_data_sent(log))
            .map_or((None, None), |(mb, rounds)| (Some(mb), Some(rounds)));
        let global_mb = logs
            .first()
            .and_then(|log| parse_global_sent(log))
            .or_else(|| {
                let values = logs
                    .iter()
                    .filter_map(|log| parse_data_sent(log).map(|v| v.0))
                    .collect::<Vec<_>>();
                (!values.is_empty()).then(|| values.iter().sum())
            });
        Ok(StockRun {
            ok: !failed,
            wall_seconds: started.elapsed().as_secs_f64(),
            combined,
            party0_mb,
            party0_rounds,
            global_mb,
        })
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
        if let Some(path) = self.source_dest.take() {
            let _ = fs::remove_file(path);
        }
        let _ = fs::remove_dir_all(&self.run_dir);
    }
}

pub struct StockRun {
    pub ok: bool,
    pub wall_seconds: f64,
    pub combined: String,
    pub party0_mb: Option<f64>,
    pub party0_rounds: Option<u64>,
    pub global_mb: Option<f64>,
}

impl Drop for LocalMpcRun {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn free_port_block(count: usize, start: u16) -> u16 {
    let mut base = start;
    while (base as usize) < 60_000usize.saturating_sub(count) {
        let available =
            (0..count).all(|offset| TcpListener::bind(("127.0.0.1", base + offset as u16)).is_ok());
        if available {
            return base;
        }
        base = base.saturating_add(200);
    }
    panic!("no free port block after scan");
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

fn parse_data_sent(text: &str) -> Option<(f64, u64)> {
    for line in text.lines() {
        let marker = "Data sent =";
        let Some((_, rest)) = line.split_once(marker) else {
            continue;
        };
        let Some((mb, rest)) = rest.trim().split_once(" MB in ~") else {
            continue;
        };
        let rounds = rest.split_whitespace().next()?.replace(',', "");
        return Some((mb.trim().parse().ok()?, rounds.parse().ok()?));
    }
    None
}

fn parse_global_sent(text: &str) -> Option<f64> {
    for line in text.lines() {
        let Some((_, rest)) = line.split_once("Global data sent =") else {
            continue;
        };
        return rest.trim().split_whitespace().next()?.parse().ok();
    }
    None
}
