//! Execute the QOMM circuit at selected MP-SPDZ protocol sites.
//!
//! Orchestration and result handling are native Rust. Circuit compilation is
//! delegated only to the verified official compiler inside MP-SPDZ.

use qomm_harness::{next_value, parse_value, unique_temp_dir, write_pretty_json, HarnessResult};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::inputs::{build_inputs, finish_reference, InputConfig};
use qomm_mpc::program::{
    build_program, pow2_ceil, sentinel_for, CheckMode, Mode, ProgramConfig, Reference,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PARTIES: usize = 7;

#[derive(Clone, Debug)]
struct Site {
    label: String,
    ssh: Option<String>,
    root: String,
    parties: Vec<usize>,
}

impl Site {
    fn parse(spec: &str) -> HarnessResult<Self> {
        let fields = spec.splitn(4, ':').collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(format!(
                "invalid --site {spec:?}; expected label:ssh_or_local:mp_spdz_root:p1,p2"
            )
            .into());
        }
        let parties = fields[3]
            .split(',')
            .map(|party| {
                party
                    .parse::<usize>()
                    .map_err(|error| format!("invalid party {party:?} in --site: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            label: fields[0].into(),
            ssh: (fields[1] != "local").then(|| fields[1].into()),
            root: fields[2].into(),
            parties,
        })
    }

    fn local_root(&self) -> PathBuf {
        expand_tilde(&self.root)
    }
}

struct Options {
    sites: Vec<Site>,
    n_mm: usize,
    mode: Mode,
    bit_length: u32,
    base_port: u16,
    repeats: usize,
    compile_on: Option<String>,
    out: PathBuf,
}

fn main() {
    let code = match run_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}

fn run_main() -> HarnessResult<i32> {
    let options = parse_args()?;
    validate_sites(&options.sites)?;
    let local = options.sites.iter().find(|site| site.ssh.is_none());
    let builder = if let Some(label) = &options.compile_on {
        options.sites.iter().find(|site| &site.label == label)
    } else {
        local
    }
    .ok_or_else(|| format!("no site labelled {:?} to compile on", options.compile_on))?;

    let work = unique_temp_dir("qomm-sites")?;
    let program = format!("qomm_sites_{}_m{}", options.mode.as_str(), options.n_mm);
    println!("== generating {program} ==");
    let reference = generate_fixture(&options, &work, &program)?;

    println!("== certificates for seven parties, on {} ==", builder.label);
    setup_certificates(builder)?;
    println!("== compiling once, on {} ==", builder.label);
    let compile_out = compile_program(builder, &work, &program)?;
    for line in compile_out.lines() {
        if line.contains("rounds") || line.contains("triples") {
            println!("  {}", line.trim());
        }
    }

    let certs = work.join("certs");
    fs::create_dir_all(&certs)?;
    collect_certificates(builder, &certs)?;
    let bytecode = work.join("bytecode");
    fs::create_dir_all(&bytecode)?;
    collect_bytecode(builder, &bytecode, &program)?;

    let remotes = options
        .sites
        .iter()
        .filter(|site| site.ssh.is_some())
        .collect::<Vec<_>>();
    let mut run_dirs = BTreeMap::new();
    for site in &remotes {
        let target = create_remote_run_dir(site)?;
        run_dirs.insert(site.label.clone(), target.clone());
        println!(
            "== shipping to {} ({}) ==",
            site.label,
            site.ssh.as_deref().unwrap_or_default()
        );
        ship_site(site, &target, &work, &bytecode, &certs, &program)?;
    }

    println!("== measuring the round trip to each site ==");
    let mut round_trips = BTreeMap::new();
    for site in &remotes {
        let value = rtt_ms(site.ssh.as_deref().expect("remote site has ssh"));
        println!(
            "  {}: {} ms",
            site.label,
            value.map_or_else(|| "None".into(), |value| value.to_string())
        );
        round_trips.insert(site.label.clone(), value);
    }

    let hosts_file = work.join("hosts");
    let host_text = (0..PARTIES)
        .map(|party| format!("127.0.0.1:{}\n", options.base_port + party as u16))
        .collect::<String>();
    fs::write(&hosts_file, host_text)?;
    if let Some(local) = local {
        fs::copy(&hosts_file, local.local_root().join("qomm_sites_hosts"))?;
    }
    for site in &remotes {
        scp_file(
            &hosts_file,
            site,
            &format!("{}/qomm_hosts", run_dirs[&site.label]),
        )?;
    }

    println!("== opening tunnels ==");
    let mut tunnels = Vec::new();
    let execution = (|| -> HarnessResult<(Vec<f64>, PathBuf)> {
        for site in &remotes {
            tunnels.push(open_tunnel(site, options.base_port)?);
        }
        thread::sleep(Duration::from_secs(5));
        for (site, tunnel) in remotes.iter().zip(&mut tunnels) {
            if tunnel.try_wait()?.is_some() {
                return Err(format!("the tunnel to {} did not open", site.label).into());
            }
        }

        let mut samples = Vec::with_capacity(options.repeats);
        for attempt in 1..=options.repeats {
            println!("== run {attempt} ==");
            let started = Instant::now();
            let mut processes = Vec::with_capacity(PARTIES);
            let mut ordered = options
                .sites
                .iter()
                .flat_map(|site| site.parties.iter().map(move |party| (site, *party)))
                .collect::<Vec<_>>();
            ordered.sort_by_key(|(_, party)| *party);
            for (site, party) in ordered {
                processes.push(spawn_party(
                    site,
                    party,
                    &program,
                    &work,
                    run_dirs.get(&site.label).map(String::as_str),
                )?);
                if party == 0 {
                    thread::sleep(Duration::from_secs(3));
                }
            }
            let mut status = 0;
            for process in &mut processes {
                if !process.wait()?.success() {
                    status = 1;
                }
            }
            let elapsed = started.elapsed().as_secs_f64();
            println!("  {elapsed:.3} s status={status}");
            samples.push(elapsed);
        }
        Ok((samples, work.clone()))
    })();

    for tunnel in &mut tunnels {
        if tunnel.try_wait()?.is_none() {
            let _ = tunnel.kill();
            let _ = tunnel.wait();
        }
    }
    for site in &remotes {
        if let Some(target) = run_dirs.get(&site.label) {
            cleanup_remote_run_dir(site, target);
        }
    }
    let (samples, work) = execution?;

    let logs_by_party = (0..PARTIES)
        .map(|party| fs::read_to_string(work.join(format!("party-{party}.log"))))
        .collect::<Result<Vec<_>, _>>()?;
    let logs = logs_by_party.join("\n");
    if !logs.contains("QOMM_MASKED_KEY=") {
        println!("== no quote in the logs; the first lines each party wrote ==");
        for (party, log) in logs_by_party.iter().enumerate() {
            let first = log
                .trim()
                .lines()
                .next()
                .filter(|line| !line.is_empty())
                .unwrap_or("(nothing)");
            println!("  party {party}: {first}");
        }
    }

    let masked = logs
        .lines()
        .filter_map(|line| line.strip_prefix("QOMM_MASKED_KEY="))
        .filter_map(|value| value.parse::<i128>().ok())
        .next_back();
    let padded = json_i128(&reference["padded_mm"])?;
    let (verified, detail) = if let Some(masked) = masked {
        let key = masked - json_i128(&reference["mask"])?;
        let got = (key.div_euclid(padded), key.rem_euclid(padded));
        let want = (
            json_i128(&reference["best_cost"])?,
            json_i128(&reference["best_mm"])?,
        );
        (
            got == want,
            format!("got=({}, {}) want=({}, {})", got.0, got.1, want.0, want.1),
        )
    } else {
        (false, "no masked quote in the logs".into())
    };

    let engine = logs_by_party
        .iter()
        .flat_map(|log| log.lines())
        .filter_map(|line| line.strip_prefix("Time = "))
        .filter_map(|tail| tail.split_whitespace().next())
        .filter_map(|value| value.parse::<f64>().ok())
        .collect::<Vec<_>>();
    let mut bytes_sent = None;
    for line in logs.lines() {
        if line.contains("Data sent") && bytes_sent.is_none() {
            bytes_sent = Some(line.trim().to_string());
        }
        if line.contains("Global data sent") {
            bytes_sent = Some(line.trim().to_string());
        }
    }
    let sites = options
        .sites
        .iter()
        .map(|site| {
            json!({
                "label": site.label,
                "parties": site.parties,
                "round_trip_ms": round_trips.get(&site.label).copied().flatten(),
                "remote": site.ssh.is_some(),
            })
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "engine_seconds": if engine.is_empty() { Value::Null } else { qomm_harness::measure::summarise(&engine) },
        "data_sent": bytes_sent,
        "fixture": "seven parties across real sites, cross-site links carried through the machine running this script",
        "sites": sites,
        "n_mm": options.n_mm,
        "mode": options.mode.as_str(),
        "bit_length": options.bit_length,
        "wall_seconds": qomm_harness::measure::summarise(&samples),
        "verified": verified,
        "verify_detail": detail,
        "claim_boundary": "a link between two remote sites is the sum of two real round trips, not the direct one between them, because the sites cannot all reach each other",
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!(
        "wrote {}  verified={}  {}",
        options.out.display(),
        if verified { "True" } else { "False" },
        payload["verify_detail"].as_str().unwrap_or_default()
    );
    let _ = fs::remove_dir_all(&work);
    Ok(if verified { 0 } else { 6 })
}

fn validate_sites(sites: &[Site]) -> HarnessResult<()> {
    let mut owners = BTreeMap::new();
    for site in sites {
        for &party in &site.parties {
            if let Some(previous) = owners.insert(party, &site.label) {
                return Err(format!(
                    "party {party} is assigned to both {previous} and {}",
                    site.label
                )
                .into());
            }
        }
    }
    let parties = owners.keys().copied().collect::<Vec<_>>();
    if parties != (0..PARTIES).collect::<Vec<_>>() {
        return Err(format!("the sites hold {parties:?}, and there are seven parties").into());
    }
    Ok(())
}

fn generate_fixture(options: &Options, work: &Path, program: &str) -> HarnessResult<Value> {
    let padded = pow2_ceil(options.n_mm)?;
    let ref_table = vec![100_000_i128];
    let sentinel = sentinel_for(options.bit_length, padded, 800_000)?;
    let mut config = ProgramConfig {
        n_mm: padded,
        n_parties: PARTIES,
        mode: options.mode,
        bit_length: options.bit_length,
        maker_assets: vec![0; padded],
        check_mode: CheckMode::PerParty,
        ref_table: ref_table.clone(),
        ..ProgramConfig::default()
    };
    config.n_assets = 1;
    fs::write(work.join(format!("{program}.mpc")), build_program(&config)?)?;
    let input_config = InputConfig {
        n_mm: padded,
        n_real_mm: options.n_mm,
        n_parties: PARTIES,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        ref_table: &ref_table,
        user_asset: 0,
        user_qty: 100,
        user_dir: 0,
        user_entity: 42,
        now_t: config.now_t,
        seed: 7,
        audit_gates: false,
        value_bits: options.bit_length + 1,
        field_bits: 128,
        use_ref: 1,
        reference: Reference::Anchored,
        input_check: false,
        check_mode: CheckMode::PerParty,
        binding_limit: false,
        user_limit: 100_000,
        user_limit_blinding: 1,
        user_qty_blinding: 1,
        response_mask: None,
        fill_mask: None,
        check_coefficients: &config.check_coefficients,
        check_repeats: config.check_repeats,
        policies: None,
        shamir_inputs: false,
        shamir_threshold: 3,
        dvp: None,
        quote_proof: None,
    };
    let mut generated = build_inputs(&input_config)?;
    finish_reference(&mut generated, &input_config, sentinel, options.mode)?;
    let input_dir = work.join("inputs");
    fs::create_dir_all(&input_dir)?;
    for (party, contents) in generated.party_files().into_iter().enumerate() {
        fs::write(input_dir.join(format!("Input-P{party}-0")), contents)?;
    }
    let reference_text = generated.reference_json();
    fs::write(work.join("reference.json"), &reference_text)?;
    Ok(serde_json::from_str(&reference_text)?)
}

fn setup_certificates(builder: &Site) -> HarnessResult<()> {
    let output = if let Some(alias) = &builder.ssh {
        let command = format!(
            "cd {} && ./Scripts/setup-ssl.sh 7 >/dev/null 2>&1 && ls Player-Data/*.pem | wc -l",
            shell_quote(&builder.root)
        );
        let output = Command::new("ssh").arg(alias).arg(command).output()?;
        if output.status.success() {
            println!(
                "  {} certificates",
                String::from_utf8_lossy(&output.stdout).trim()
            );
        }
        output
    } else {
        Command::new("bash")
            .args(["Scripts/setup-ssl.sh", "7"])
            .current_dir(builder.local_root())
            .output()?
    };
    ensure_success(&output, "MP-SPDZ certificate setup")
}

fn compile_program(builder: &Site, work: &Path, program: &str) -> HarnessResult<String> {
    let source = work.join(format!("{program}.mpc"));
    let output = if let Some(alias) = &builder.ssh {
        let destination = format!("{}:{}/Programs/Source/", alias, builder.root);
        ensure_success(
            &Command::new("scp")
                .args([
                    OsString::from("-q"),
                    source.as_os_str().to_owned(),
                    destination.into(),
                ])
                .output()?,
            "copy generated program to compile site",
        )?;
        Command::new("ssh")
            .arg(alias)
            .arg(format!(
                "cd {} && ./compile.py -F 128 {}",
                shell_quote(&builder.root),
                shell_quote(program)
            ))
            .output()?
    } else {
        let root = builder.local_root();
        let target = root.join("Programs/Source").join(format!("{program}.mpc"));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
        OfficialCompiler::from_checkout(&root)?.compile_field(128, program)?
    };
    let text = combined_output(&output);
    if !output.status.success() {
        return Err(format!("MP-SPDZ compile failed:\n{}", tail(&text, 1_500)).into());
    }
    Ok(text)
}

fn collect_certificates(builder: &Site, target: &Path) -> HarnessResult<()> {
    if let Some(alias) = &builder.ssh {
        for suffix in ["*.pem", "*.key"] {
            let source = format!("{}:{}/Player-Data/{suffix}", alias, builder.root);
            ensure_success(
                &Command::new("scp")
                    .args([
                        OsString::from("-q"),
                        source.into(),
                        target.as_os_str().to_owned(),
                    ])
                    .output()?,
                "collect MP-SPDZ certificates",
            )?;
        }
    } else {
        let player_data = builder.local_root().join("Player-Data");
        for entry in fs::read_dir(player_data)? {
            let path = entry?.path();
            if matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("pem" | "key")
            ) {
                fs::copy(
                    &path,
                    target.join(path.file_name().expect("certificate filename")),
                )?;
            }
        }
    }
    Ok(())
}

fn collect_bytecode(builder: &Site, target: &Path, program: &str) -> HarnessResult<()> {
    if let Some(alias) = &builder.ssh {
        for source in [
            format!(
                "{}:{}/Programs/Schedules/{program}.sch",
                alias, builder.root
            ),
            format!(
                "{}:{}/Programs/Bytecode/{program}-*.bc",
                alias, builder.root
            ),
        ] {
            ensure_success(
                &Command::new("scp")
                    .args([
                        OsString::from("-q"),
                        source.into(),
                        target.as_os_str().to_owned(),
                    ])
                    .output()?,
                "collect MP-SPDZ bytecode",
            )?;
        }
    } else {
        let root = builder.local_root();
        fs::copy(
            root.join("Programs/Schedules")
                .join(format!("{program}.sch")),
            target.join(format!("{program}.sch")),
        )?;
        for path in files_with_prefix(
            &root.join("Programs/Bytecode"),
            &format!("{program}-"),
            ".bc",
        )? {
            fs::copy(
                &path,
                target.join(path.file_name().expect("bytecode filename")),
            )?;
        }
    }
    Ok(())
}

fn create_remote_run_dir(site: &Site) -> HarnessResult<String> {
    let alias = site.ssh.as_deref().expect("remote site");
    let command = format!(
        "root=$(cd {} && pwd) && d=$(mktemp -d \"$root/../qomm_sites_XXXX\") && mkdir -p \"$d/Programs/Bytecode\" \"$d/Programs/Schedules\" \"$d/Player-Data\" && ln -sf \"$root/malicious-shamir-party.x\" \"$d/\" && echo \"$d\"",
        shell_quote(&site.root)
    );
    let output = Command::new("ssh").arg(alias).arg(command).output()?;
    ensure_success(
        &output,
        &format!("create private run directory on {}", site.label),
    )?;
    let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !target.contains("qomm_sites_") || target.contains('\n') {
        return Err(format!("{} returned an unsafe run directory {target:?}", site.label).into());
    }
    Ok(target)
}

fn ship_site(
    site: &Site,
    target: &str,
    work: &Path,
    bytecode: &Path,
    certs: &Path,
    program: &str,
) -> HarnessResult<()> {
    scp_file(
        &bytecode.join(format!("{program}.sch")),
        site,
        &format!("{target}/Programs/Schedules/"),
    )?;
    for path in files_with_prefix(bytecode, &format!("{program}-"), ".bc")? {
        scp_file(&path, site, &format!("{target}/Programs/Bytecode/"))?;
    }
    for &party in &site.parties {
        scp_file(
            &work.join("inputs").join(format!("Input-P{party}-0")),
            site,
            &format!("{target}/Player-Data/"),
        )?;
    }
    for path in files_with_extension(certs, "pem")? {
        scp_file(&path, site, &format!("{target}/Player-Data/"))?;
    }
    for &party in &site.parties {
        let key = certs.join(format!("P{party}.key"));
        if key.exists() {
            scp_file(&key, site, &format!("{target}/Player-Data/"))?;
        }
    }
    let alias = site.ssh.as_deref().expect("remote site");
    for _ in 0..2 {
        let _ = Command::new("ssh")
            .arg(alias)
            .arg(format!(
                "cd {} && c_rehash . >/dev/null 2>&1 || true",
                shell_quote(&format!("{target}/Player-Data"))
            ))
            .output()?;
    }
    Ok(())
}

fn rtt_ms(alias: &str) -> Option<f64> {
    let output = Command::new("ssh").args(["-G", alias]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut host = None;
    let mut port = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("hostname ") {
            host = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("port ") {
            port = value.trim().parse::<u16>().ok();
        }
    }
    let measured = qomm_transport::rtt::tcp_handshake_median_ms(
        host.as_deref()?,
        port?,
        9,
        Duration::from_secs(5),
    )
    .ok()??;
    Some(qomm_sim::market::round_half_even(measured * 100.0) as f64 / 100.0)
}

fn open_tunnel(site: &Site, base_port: u16) -> HarnessResult<Child> {
    let mut command = Command::new("ssh");
    command.args([
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "ServerAliveInterval=15",
    ]);
    let owned = site.parties.iter().copied().collect::<BTreeSet<_>>();
    for party in &site.parties {
        let port = base_port + *party as u16;
        command.args(["-L", &format!("127.0.0.1:{port}:127.0.0.1:{port}")]);
    }
    for party in 0..PARTIES {
        if !owned.contains(&party) {
            let port = base_port + party as u16;
            command.args(["-R", &format!("127.0.0.1:{port}:127.0.0.1:{port}")]);
        }
    }
    command.arg(site.ssh.as_deref().expect("remote site"));
    Ok(command.spawn()?)
}

fn spawn_party(
    site: &Site,
    party: usize,
    program: &str,
    work: &Path,
    remote_run_dir: Option<&str>,
) -> HarnessResult<Child> {
    let log = File::create(work.join(format!("party-{party}.log")))?;
    let stderr = log.try_clone()?;
    let mut command = if let Some(alias) = &site.ssh {
        let target = remote_run_dir.ok_or("remote site has no run directory")?;
        let mut command = Command::new("ssh");
        command.arg(alias).arg(format!(
            "cd {} && ./malicious-shamir-party.x {party} {} -N 7 -T 2 -ip qomm_hosts",
            shell_quote(target),
            shell_quote(program),
        ));
        command
    } else {
        let mut command = Command::new("./malicious-shamir-party.x");
        command
            .args([
                party.to_string(),
                program.to_string(),
                "-N".into(),
                "7".into(),
                "-T".into(),
                "2".into(),
                "-ip".into(),
                "qomm_sites_hosts".into(),
            ])
            .current_dir(site.local_root());
        command
    };
    command.stdout(Stdio::from(log)).stderr(Stdio::from(stderr));
    Ok(command.spawn()?)
}

fn cleanup_remote_run_dir(site: &Site, target: &str) {
    if !target.contains("qomm_sites_") {
        return;
    }
    let _ = Command::new("ssh")
        .arg(site.ssh.as_deref().expect("remote site"))
        .arg(format!("rm -rf -- {}", shell_quote(target)))
        .output();
}

fn scp_file(source: &Path, site: &Site, remote_target: &str) -> HarnessResult<()> {
    let destination = format!(
        "{}:{remote_target}",
        site.ssh.as_deref().expect("remote site")
    );
    let output = Command::new("scp")
        .args([
            OsString::from("-q"),
            source.as_os_str().to_owned(),
            destination.into(),
        ])
        .output()?;
    ensure_success(
        &output,
        &format!("copy {} to {}", source.display(), site.label),
    )
}

fn files_with_prefix(directory: &Path, prefix: &str, suffix: &str) -> HarnessResult<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no {prefix}*{suffix} files under {}", directory.display()).into());
    }
    Ok(paths)
}

fn files_with_extension(directory: &Path, extension: &str) -> HarnessResult<Vec<PathBuf>> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(extension))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn ensure_success(output: &Output, what: &str) -> HarnessResult<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed:\n{}", tail(&combined_output(output), 1_500)).into())
    }
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn tail(text: &str, limit: usize) -> &str {
    let start = text.len().saturating_sub(limit);
    &text[text.ceil_char_boundary(start)..]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        return std::env::var_os("HOME").map_or_else(|| PathBuf::from(value), PathBuf::from);
    }
    value.strip_prefix("~/").map_or_else(
        || PathBuf::from(value),
        |tail| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(tail)
        },
    )
}

fn json_i128(value: &Value) -> HarnessResult<i128> {
    match value {
        Value::Number(number) => number
            .to_string()
            .parse::<i128>()
            .map_err(|error| format!("JSON integer {number} is outside i128: {error}").into()),
        Value::String(number) => number
            .parse::<i128>()
            .map_err(|error| format!("JSON integer {number:?} is invalid: {error}").into()),
        _ => Err(format!("expected a JSON integer, got {value}").into()),
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut site_specs = Vec::new();
    let mut n_mm: usize = 16;
    let mut mode = Mode::Rfq;
    let mut bit_length: u32 = 31;
    let mut base_port: u16 = 24_100;
    let mut repeats: usize = 3;
    let mut compile_on = None;
    let mut out = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--site") => site_specs.push(
                next_value(&mut args, "--site")?
                    .into_string()
                    .map_err(|_| "--site is not valid UTF-8")?,
            ),
            Some("--n-mm") => n_mm = parse_value(next_value(&mut args, "--n-mm")?, "--n-mm")?,
            Some("--mode") => {
                let raw = next_value(&mut args, "--mode")?
                    .into_string()
                    .map_err(|_| "--mode is not valid UTF-8")?;
                mode = Mode::parse(&raw).ok_or_else(|| format!("invalid --mode {raw:?}"))?;
            }
            Some("--bit-length") => {
                bit_length = parse_value(next_value(&mut args, "--bit-length")?, "--bit-length")?
            }
            Some("--base-port") => {
                base_port = parse_value(next_value(&mut args, "--base-port")?, "--base-port")?
            }
            Some("--repeats") => {
                repeats = parse_value(next_value(&mut args, "--repeats")?, "--repeats")?
            }
            Some("--compile-on") => {
                compile_on = Some(
                    next_value(&mut args, "--compile-on")?
                        .into_string()
                        .map_err(|_| "--compile-on is not valid UTF-8")?,
                )
            }
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("-h" | "--help") => {
                println!("usage: run_sites --site label:ssh_or_local:root:p1,p2 [--site ...] --out PATH [--n-mm N] [--mode rfq|rfm|rfs] [--bit-length N] [--base-port PORT] [--repeats N] [--compile-on LABEL]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    if site_specs.is_empty() {
        return Err("at least one --site is required".into());
    }
    if n_mm == 0 || repeats == 0 {
        return Err("--n-mm and --repeats must be positive".into());
    }
    if usize::from(base_port) + PARTIES > usize::from(u16::MAX) {
        return Err("--base-port leaves no room for seven party ports".into());
    }
    Ok(Options {
        sites: site_specs
            .iter()
            .map(|spec| Site::parse(spec))
            .collect::<HarnessResult<Vec<_>>>()?,
        n_mm,
        mode,
        bit_length,
        base_port,
        repeats,
        compile_on,
        out: out.ok_or("--out is required")?,
    })
}
