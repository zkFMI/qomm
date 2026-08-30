//!
//! Program generation is provided by `qomm-mpc`, and every party is a fresh
//! process invoking `qomm_mpc::run`, which enters the linked libSPDZ engine.
//! The harness only owns orchestration, parsing, restoration, and the artifact.

use qomm_harness::{unique_temp_dir, write_pretty_json, HarnessResult};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::inputs::{build_inputs, finish_reference, DvpInputs, InputConfig};
use qomm_mpc::program::{
    build_program, ed25519_lagrange_at_zero, pow2_ceil, sentinel_for, CheckMode, Disclosure, Mode,
    ProgramConfig, Reference, StopAfter, ED25519_ORDER,
};
use qomm_mpc::Protocol;
use serde_json::{json, Map, Value};
use std::fs::{self, File};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const COMPILE_KEYS: [(&str, &str); 4] = [
    ("integer_bits", "integer bits"),
    ("integer_opens", "integer opens"),
    ("integer_triples", "integer triples"),
    ("vm_rounds", "virtual machine rounds"),
];

#[derive(Clone)]
struct Options {
    mp_spdz_root: Option<PathBuf>,
    n_mm: usize,
    n_parties: usize,
    threshold: usize,
    mode: Mode,
    rfs_steps: usize,
    disclose: Disclosure,
    delay_ms: f64,
    per_party_ms: Option<Vec<f64>>,
    repeats: usize,
    user_qty: i128,
    user_dir: i128,
    seed: i128,
    field_bits: i128,
    bit_length: u32,
    argmin_arity: usize,
    edabit: bool,
    input_check: bool,
    trunc_pr: bool,
    protocol: String,
    stop_after: StopAfter,
    prepare_only: bool,
    is_real: i128,
    n_assets: usize,
    batch_size: Option<usize>,
    n_requests: usize,
    public_maker_assets: bool,
    audit_gates: bool,
    binding_limit: bool,
    user_limit: i128,
    check_mode: CheckMode,
    unsound_check_for_measurement: bool,
    file_prep: bool,
    user_asset: usize,
    prime: Option<String>,
    reference: Reference,
    use_ref: i128,
    persist_wires: bool,
    persist_zkpi_wires: bool,
    persist_dvp_wires: bool,
    zkpi_amount_bits: usize,
    zkpi_price_bits: usize,
    dvp_remainder_bits: usize,
    taker_securities_reserve: Option<i128>,
    taker_securities_blinding: Option<i128>,
    taker_cash_reserve: Option<i128>,
    taker_cash_blinding: Option<i128>,
    maker_securities_reserve: Option<i128>,
    maker_securities_blinding: Option<i128>,
    maker_cash_reserve: Option<i128>,
    maker_cash_blinding: Option<i128>,
    shamir_inputs: bool,
    tag: String,
    out: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mp_spdz_root: std::env::var_os("MP_SPDZ_ROOT").map(PathBuf::from),
            n_mm: 16,
            n_parties: 7,
            threshold: 2,
            mode: Mode::Rfq,
            rfs_steps: 5,
            disclose: Disclosure::None,
            delay_ms: 0.0,
            per_party_ms: None,
            repeats: 3,
            user_qty: 100,
            user_dir: 0,
            seed: 7,
            field_bits: 128,
            bit_length: 63,
            argmin_arity: 2,
            edabit: false,
            input_check: false,
            trunc_pr: false,
            protocol: "malicious-shamir-party.x".into(),
            stop_after: StopAfter::Tournament,
            prepare_only: false,
            is_real: 1,
            n_assets: 1,
            batch_size: None,
            n_requests: 1,
            public_maker_assets: false,
            audit_gates: false,
            binding_limit: false,
            user_limit: 100_000,
            check_mode: CheckMode::PerParty,
            unsound_check_for_measurement: false,
            file_prep: false,
            user_asset: 0,
            prime: None,
            reference: Reference::Anchored,
            use_ref: 1,
            persist_wires: false,
            persist_zkpi_wires: false,
            persist_dvp_wires: false,
            zkpi_amount_bits: 32,
            zkpi_price_bits: 32,
            dvp_remainder_bits: 32,
            taker_securities_reserve: None,
            taker_securities_blinding: None,
            taker_cash_reserve: None,
            taker_cash_blinding: None,
            maker_securities_reserve: None,
            maker_securities_blinding: None,
            maker_cash_reserve: None,
            maker_cash_blinding: None,
            shamir_inputs: false,
            tag: String::new(),
            out: None,
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("__party") {
        party_main(args.collect());
        return;
    }
    let code = match run_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            2
        }
    };
    std::process::exit(code);
}

fn party_main(args: Vec<String>) {
    let Some((protocol_name, tail)) = args.split_first() else {
        eprintln!("missing qomm-mpc protocol");
        std::process::exit(2);
    };
    let Some(protocol) = Protocol::parse(protocol_name) else {
        eprintln!("unsupported qomm-mpc protocol {protocol_name}");
        std::process::exit(2);
    };
    let me = std::env::args().next().unwrap_or_else(|| "run_qomm".into());
    let mut argv = vec![me.as_str()];
    argv.extend(tail.iter().map(String::as_str));
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
}

fn run_main() -> HarnessResult<i32> {
    let mut options = parse_args()?;
    let Some(root) = options.mp_spdz_root.clone() else {
        eprintln!("set MP_SPDZ_ROOT or pass --mp-spdz-root");
        return Ok(2);
    };
    let root = root.canonicalize()?;
    if !root.join(&options.protocol).exists() {
        eprintln!("{} missing under {}", options.protocol, root.display());
        return Ok(2);
    }
    let mpc_protocol = stock_protocol(&options.protocol)?;

    if options.shamir_inputs {
        if options
            .prime
            .as_deref()
            .is_some_and(|prime| prime != ED25519_ORDER)
        {
            eprintln!("--shamir-inputs fixes the prime; do not also pass --prime");
            return Ok(2);
        }
        options.prime = Some(ED25519_ORDER.into());
        options.field_bits = decimal_bit_length(ED25519_ORDER) as i128;
    }
    if options.persist_dvp_wires {
        options.persist_zkpi_wires = true;
        options.persist_wires = true;
    }
    if options.persist_zkpi_wires {
        options.persist_wires = true;
        if !options.shamir_inputs || options.mode != Mode::Rfq {
            return Err("threshold proof persistence requires RFQ and --shamir-inputs".into());
        }
    }
    if options.input_check
        && options.check_mode == CheckMode::Aggregate
        && !options.unsound_check_for_measurement
    {
        eprintln!(
            "the aggregate input check is unsound; pass --unsound-check-for-measurement only to reproduce its cost baseline"
        );
        return Ok(7);
    }
    if let Some(delays) = &options.per_party_ms {
        if delays.len() != options.n_parties {
            return Err(format!(
                "--per-party-ms needs {} values, got {}",
                options.n_parties,
                delays.len()
            )
            .into());
        }
    }

    let work = unique_temp_dir("qomm-gen")?;
    let program = format!(
        "qomm_{}_m{}_{}_{}",
        options.mode.as_str(),
        options.n_mm,
        options.disclose.as_str(),
        std::process::id()
    );
    let source = work.join(format!("{program}.mpc"));
    let inputs = work.join("inputs");
    let reference_path = work.join("reference.json");
    let reference = generate(&options, &source, &inputs, &reference_path)?;

    let mut installed = InstalledRun::new(root.clone(), program.clone(), options.n_parties)?;
    let mut result = initial_result(&options, &reference);
    let outcome = (|| -> HarnessResult<i32> {
        installed.install(&source, &inputs)?;
        let compile = compile_program(
            &root,
            &program,
            options.prime.as_deref(),
            options.field_bits,
        )?;
        result.insert("circuit".into(), compile);

        if options.prepare_only {
            result.insert("program".into(), json!(program));
            result.insert(
                "reference".into(),
                json!(reference_path.to_string_lossy().to_string()),
            );
            result.insert("work_dir".into(), json!(work.to_string_lossy().to_string()));
            result.insert("prepared".into(), json!(true));
            installed.keep = true;
            // The name of the compiled program, on its own, next to the JSON.
            // `rounds-by-channel` needs exactly this one field and used to get
            // substitution. Writing what the next step reads is cheaper than
            // parsing, and it is the producer that knows the field's name.
            if let Some(out) = options.out.as_deref() {
                fs::write(out.with_extension("program"), format!("{program}\n"))?;
            }
            let text = write_pretty_json(options.out.as_deref(), &Value::Object(result.clone()))?;
            println!("{text}");
            return Ok(0);
        }

        let mut samples = Vec::new();
        let mut verified: Option<bool> = None;
        let mut detail = String::new();
        for _ in 0..options.repeats {
            let execution = installed.execute(&options, mpc_protocol)?;
            if !execution.ok {
                result.insert("error".into(), json!("party failure"));
                result.insert("log_tail".into(), json!(tail_chars(&execution.log, 4000)));
                break;
            }
            let (ok, why) = verify(options.mode, &execution.log, &reference)?;
            verified = Some(verified.unwrap_or(true) && ok);
            detail = why;
            samples.push(json!({
                "wall_seconds": execution.wall_seconds,
                "party0_seconds": execution.party0_seconds,
                "party0_mb": execution.party0_mb,
                "party0_rounds": execution.party0_rounds,
                "global_mb": execution.global_mb,
            }));
        }
        result.insert("samples".into(), Value::Array(samples.clone()));
        result.insert("verified".into(), verified.map_or(Value::Null, Value::Bool));
        result.insert("verify_detail".into(), json!(detail));
        if !samples.is_empty() {
            let walls = numeric_field(&samples, "wall_seconds");
            let p0 = numeric_field(&samples, "party0_seconds");
            result.insert("wall_median".into(), json!(median(&walls)));
            result.insert("wall_min".into(), json!(minimum(&walls)));
            result.insert(
                "party0_median".into(),
                if p0.is_empty() {
                    Value::Null
                } else {
                    json!(median(&p0))
                },
            );
            result.insert(
                "measured_rounds".into(),
                samples[0]["party0_rounds"].clone(),
            );
            result.insert("measured_mb".into(), samples[0]["party0_mb"].clone());
        }
        let text = write_pretty_json(options.out.as_deref(), &Value::Object(result.clone()))?;
        println!("{text}");
        Ok(if verified == Some(true) { 0 } else { 1 })
    })();

    if !options.prepare_only {
        let _ = fs::remove_dir_all(&work);
    }
    outcome
}

fn generate(
    options: &Options,
    source: &Path,
    input_dir: &Path,
    reference_path: &Path,
) -> HarnessResult<Value> {
    let padded = pow2_ceil(options.n_mm)?;
    let ref_table: Vec<i128> = (0..options.n_assets)
        .map(|asset| 100_000 + 5_000 * asset as i128)
        .collect();
    if options.user_asset >= options.n_assets {
        return Err("--user-asset must be below --n-assets".into());
    }
    let sentinel = sentinel_for(
        options.bit_length,
        padded,
        8 * *ref_table
            .iter()
            .max()
            .ok_or("--n-assets must be positive")?,
    )?;
    let lagrange = if options.shamir_inputs {
        Some(ed25519_lagrange_at_zero(options.n_parties)?)
    } else {
        None
    };
    let config = ProgramConfig {
        n_mm: padded,
        n_parties: options.n_parties,
        mode: options.mode,
        rfs_steps: options.rfs_steps,
        disclose: options.disclose,
        n_requests: options.n_requests,
        n_assets: options.n_assets,
        ref_table: ref_table.clone(),
        maker_assets: (0..padded).map(|maker| maker % options.n_assets).collect(),
        public_maker_assets: options.public_maker_assets,
        audit_gates: options.audit_gates,
        bit_length: options.bit_length,
        argmin_arity: if options.argmin_arity == 0 {
            padded
        } else {
            options.argmin_arity
        },
        lagrange,
        edabit: options.edabit,
        trunc_pr: options.trunc_pr,
        input_check: options.input_check,
        check_mode: options.check_mode,
        binding_limit: options.binding_limit,
        stop_after: options.stop_after,
        persist_wires: options.persist_wires,
        persist_zkpi_wires: options.persist_zkpi_wires,
        persist_dvp_wires: options.persist_dvp_wires,
        zkpi_amount_bits: options.zkpi_amount_bits,
        zkpi_price_bits: options.zkpi_price_bits,
        dvp_remainder_bits: options.dvp_remainder_bits,
        reference: options.reference,
        ..ProgramConfig::default()
    };
    let program_text = build_program(&config)?;
    if let Some(parent) = source.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(source, program_text)?;

    let input_config = InputConfig {
        n_mm: padded,
        n_real_mm: options.n_mm,
        n_parties: options.n_parties,
        is_real: options.is_real,
        n_requests: options.n_requests,
        n_assets: options.n_assets,
        ref_table: &ref_table,
        user_asset: options.user_asset,
        user_qty: options.user_qty,
        user_dir: options.user_dir,
        user_entity: 42,
        now_t: config.now_t,
        seed: options.seed,
        audit_gates: options.audit_gates,
        value_bits: options.bit_length + 1,
        field_bits: options.field_bits,
        use_ref: options.use_ref,
        reference: options.reference,
        input_check: options.input_check,
        check_mode: options.check_mode,
        binding_limit: options.binding_limit,
        user_limit: options.user_limit,
        user_limit_blinding: 1,
        user_qty_blinding: 1,
        check_coefficients: &config.check_coefficients,
        check_repeats: config.check_repeats,
        policies: None,
        shamir_inputs: options.shamir_inputs,
        shamir_threshold: options.threshold,
        dvp: if options.persist_dvp_wires {
            Some(DvpInputs {
                taker_securities_reserve: options
                    .taker_securities_reserve
                    .ok_or("--persist-dvp-wires requires --taker-securities-reserve")?,
                taker_securities_blinding: options
                    .taker_securities_blinding
                    .ok_or("--persist-dvp-wires requires --taker-securities-blinding")?,
                taker_cash_reserve: options
                    .taker_cash_reserve
                    .ok_or("--persist-dvp-wires requires --taker-cash-reserve")?,
                taker_cash_blinding: options
                    .taker_cash_blinding
                    .ok_or("--persist-dvp-wires requires --taker-cash-blinding")?,
                maker_securities_reserves: vec![
                    options.maker_securities_reserve.ok_or(
                        "--persist-dvp-wires requires --maker-securities-reserve"
                    )?;
                    padded
                ],
                maker_securities_blindings: vec![
                    options.maker_securities_blinding.ok_or(
                        "--persist-dvp-wires requires --maker-securities-blinding"
                    )?;
                    padded
                ],
                maker_cash_reserves: vec![
                    options.maker_cash_reserve.ok_or(
                        "--persist-dvp-wires requires --maker-cash-reserve"
                    )?;
                    padded
                ],
                maker_cash_blindings: vec![
                    options.maker_cash_blinding.ok_or(
                        "--persist-dvp-wires requires --maker-cash-blinding"
                    )?;
                    padded
                ],
                maker_handle_scalars: (0..padded).map(|maker| 21_i128 + maker as i128).collect(),
            })
        } else {
            None
        },
        quote_proof: None,
    };
    let mut generated = build_inputs(&input_config)?;
    finish_reference(&mut generated, &input_config, sentinel, options.mode)?;
    fs::create_dir_all(input_dir)?;
    for (party, text) in generated.party_files().into_iter().enumerate() {
        fs::write(input_dir.join(format!("Input-P{party}-0")), text)?;
    }
    let reference_text = generated.reference_json();
    fs::write(reference_path, &reference_text)?;
    Ok(serde_json::from_str(&reference_text)?)
}

fn initial_result(options: &Options, reference: &Value) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("tag".into(), json!(options.tag));
    result.insert("mode".into(), json!(options.mode.as_str()));
    result.insert("n_mm".into(), json!(options.n_mm));
    result.insert("padded_mm".into(), reference["padded_mm"].clone());
    result.insert("n_parties".into(), json!(options.n_parties));
    result.insert("threshold".into(), json!(options.threshold));
    result.insert("disclose".into(), json!(options.disclose.as_str()));
    result.insert(
        "rfs_steps".into(),
        if options.mode == Mode::Rfs {
            json!(options.rfs_steps)
        } else {
            Value::Null
        },
    );
    result.insert("delay_ms".into(), json!(options.delay_ms));
    result.insert("repeats".into(), json!(options.repeats));
    result.insert("host".into(), json!(qomm_measure::hosts::this_host()));
    result.insert("bit_length".into(), json!(options.bit_length));
    result.insert("argmin_arity".into(), json!(options.argmin_arity));
    result.insert("edabit".into(), json!(options.edabit));
    result.insert("trunc_pr".into(), json!(options.trunc_pr));
    result.insert("input_check".into(), json!(options.input_check));
    result.insert(
        "check_mode".into(),
        json!(match options.check_mode {
            CheckMode::Aggregate => "aggregate",
            CheckMode::PerParty => "per-party",
        }),
    );
    result.insert("binding_limit".into(), json!(options.binding_limit));
    result.insert("field_bits".into(), json!(options.field_bits));
    result.insert("protocol".into(), json!(options.protocol));
    result.insert("is_real".into(), json!(options.is_real));
    result.insert(
        "prime".into(),
        options
            .prime
            .as_ref()
            .map_or(Value::Null, |prime| json!(prime)),
    );
    result.insert("n_assets".into(), json!(options.n_assets));
    result.insert("user_asset".into(), json!(options.user_asset));
    result.insert(
        "batch_size".into(),
        options.batch_size.map_or(Value::Null, |value| json!(value)),
    );
    result.insert("file_prep".into(), json!(options.file_prep));
    result.insert(
        "public_maker_assets".into(),
        json!(options.public_maker_assets),
    );
    result.insert("audit_gates".into(), json!(options.audit_gates));
    result.insert("n_requests".into(), json!(options.n_requests));
    result
}

struct InstalledRun {
    root: PathBuf,
    program: String,
    parties: usize,
    run_dir: PathBuf,
    saved: Vec<(PathBuf, Option<Vec<u8>>)>,
    source_dest: Option<PathBuf>,
    port_base: Option<u16>,
    keep: bool,
}

impl InstalledRun {
    fn new(root: PathBuf, program: String, parties: usize) -> HarnessResult<Self> {
        let run_dir = unique_temp_dir("qomm")?;
        fs::create_dir_all(run_dir.join("backup"))?;
        Ok(Self {
            root,
            program,
            parties,
            run_dir,
            saved: Vec::new(),
            source_dest: None,
            port_base: None,
            keep: false,
        })
    }

    fn install(&mut self, source: &Path, input_dir: &Path) -> HarnessResult<()> {
        let player_data = self.root.join("Player-Data");
        fs::create_dir_all(&player_data)?;
        for party in 0..self.parties {
            let target = player_data.join(format!("Input-P{party}-0"));
            let saved = fs::read(&target).ok();
            self.saved.push((target.clone(), saved));
            fs::copy(input_dir.join(format!("Input-P{party}-0")), &target)?;
            let output = player_data.join(format!("Private-Output-P{party}"));
            if output.exists() {
                fs::remove_file(output)?;
            }
        }
        let destination = self
            .root
            .join("Programs/Source")
            .join(format!("{}.mpc", self.program));
        fs::create_dir_all(destination.parent().expect("a source parent"))?;
        fs::copy(source, &destination)?;
        self.source_dest = Some(destination);
        Ok(())
    }

    fn execute(&mut self, options: &Options, protocol: Protocol) -> HarnessResult<Execution> {
        let block = self.parties * (self.parties + 2);
        let actual_base = *self
            .port_base
            .get_or_insert_with(|| free_port_block(block, 21_000));
        let proxy_base = actual_base + self.parties as u16 + 1;
        let proxies = self.write_host_files(
            actual_base,
            proxy_base,
            options.delay_ms,
            options.per_party_ms.as_deref(),
        )?;
        let mut proxy = if proxies.is_empty() {
            None
        } else {
            Some(start_proxy(&self.run_dir, options.delay_ms, &proxies)?)
        };

        let started = Instant::now();
        let executable = std::env::current_exe()?;
        let mut children = Vec::new();
        for party in 0..self.parties {
            let log_path = self.run_dir.join(format!("party-{party}.log"));
            let log = File::create(&log_path)?;
            let stderr = log.try_clone()?;
            let mut command = Command::new(&executable);
            command
                .arg("__party")
                .arg(protocol.as_str())
                .arg(party.to_string())
                .arg(&self.program)
                .args(["-N", &self.parties.to_string()])
                .args(["-T", &options.threshold.to_string()]);
            if let Some(prime) = &options.prime {
                command.args(["-P", prime]);
            }
            if let Some(batch) = options.batch_size {
                command.args(["-b", &batch.to_string()]);
            }
            if options.file_prep {
                command.arg("-F");
            }
            command
                .arg("-ip")
                .arg(self.run_dir.join(format!("hosts-P{party}")))
                .current_dir(&self.root)
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr));
            children.push(command.spawn()?);
        }

        let deadline = started + Duration::from_secs(1800);
        let mut failed = false;
        for child in &mut children {
            loop {
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        failed = true;
                    }
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
            let _ = child.kill();
            let _ = child.wait();
        }

        let mut logs = Vec::new();
        let mut parsed = Vec::new();
        for party in 0..self.parties {
            let text = fs::read_to_string(self.run_dir.join(format!("party-{party}.log")))?;
            parsed.push(parse_party_total(&text));
            logs.push(format!("===== PARTY {party} =====\n{text}"));
        }
        let combined = logs.join("\n");
        let p0 = parsed.first().and_then(|row| row.as_ref());
        if parsed.iter().any(Option::is_none) {
            failed = true;
        }
        let global_bytes: u64 = parsed
            .iter()
            .filter_map(|row| row.as_ref().map(|row| row.sent))
            .sum();
        Ok(Execution {
            ok: !failed,
            wall_seconds,
            party0_seconds: p0.map(|row| row.seconds),
            party0_mb: p0.map(|row| six_significant(row.sent as f64 / 1_000_000.0)),
            party0_rounds: p0.map(|row| row.rounds),
            global_mb: (!parsed.is_empty())
                .then(|| six_significant(global_bytes as f64 / 1_000_000.0)),
            log: combined,
        })
    }

    fn write_host_files(
        &self,
        actual_base: u16,
        proxy_base: u16,
        delay_ms: f64,
        per_party_ms: Option<&[f64]>,
    ) -> HarnessResult<Vec<Value>> {
        let mut proxies = Vec::new();
        for source in 0..self.parties {
            let mut lines = String::new();
            for target in 0..self.parties {
                let uniform = delay_ms == 0.0 && per_party_ms.is_none();
                let port = if uniform || source == target {
                    actual_base + target as u16
                } else {
                    let port = proxy_base + (source * self.parties + target) as u16;
                    let link_ms = per_party_ms
                        .map(|delays| delays[source].max(delays[target]))
                        .unwrap_or(delay_ms);
                    proxies.push(json!({
                        "source": source,
                        "target": target,
                        "listen_port": port,
                        "target_port": actual_base + target as u16,
                        "one_way_delay_ms": link_ms,
                    }));
                    port
                };
                lines.push_str(&format!("127.0.0.1:{port}\n"));
            }
            fs::write(self.run_dir.join(format!("hosts-P{source}")), lines)?;
        }
        fs::write(
            self.run_dir.join("proxy.json"),
            serde_json::to_vec(&json!({
                "one_way_delay_ms": delay_ms,
                "proxies": proxies,
            }))?,
        )?;
        Ok(proxies)
    }

    fn cleanup(&mut self) {
        for (target, saved) in self.saved.drain(..) {
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

impl Drop for InstalledRun {
    fn drop(&mut self) {
        if !self.keep {
            self.cleanup();
        }
    }
}

#[derive(Clone, Copy)]
struct PartyTotal {
    rounds: u64,
    sent: u64,
    seconds: f64,
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

fn compile_program(
    root: &Path,
    program: &str,
    prime: Option<&str>,
    field_bits: i128,
) -> HarnessResult<Value> {
    let compiler = OfficialCompiler::from_checkout(root)?;
    let mut command = compiler.command();
    command.arg("-F");
    if let Some(prime) = prime {
        command.arg(decimal_bit_length(prime).to_string());
    } else {
        command.arg(field_bits.to_string());
    }
    command.arg(program);
    let started = Instant::now();
    let output = command.output()?;
    let elapsed = started.elapsed().as_secs_f64();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!("compile failed:\n{}", tail_chars(&text, 4000)).into());
    }
    let mut stats = Map::new();
    stats.insert("compile_seconds".into(), json!(elapsed));
    for (key, phrase) in COMPILE_KEYS {
        let value = text.lines().find_map(|line| {
            line.find(phrase).and_then(|at| {
                line[..at]
                    .split_whitespace()
                    .last()
                    .and_then(|raw| raw.replace(',', "").parse::<u64>().ok())
            })
        });
        stats.insert(key.into(), value.map_or(Value::Null, |value| json!(value)));
    }
    stats.insert("compile_log".into(), json!(tail_chars(&text, 2000)));
    Ok(Value::Object(stats))
}

fn start_proxy(run_dir: &Path, delay_ms: f64, proxies: &[Value]) -> HarnessResult<Child> {
    let ready = run_dir.join("ready.json");
    fs::write(
        run_dir.join("proxy.json"),
        serde_json::to_vec(&json!({
            "one_way_delay_ms": delay_ms,
            "proxies": proxies,
        }))?,
    )?;
    let proxy = if let Some(path) = std::env::var_os("QOMM_WAN_PROXY_BIN") {
        PathBuf::from(path)
    } else {
        let executable = std::env::current_exe()?;
        let directory = executable
            .parent()
            .ok_or("run_qomm executable has no parent directory")?;
        directory.join(format!("wan_proxy{}", std::env::consts::EXE_SUFFIX))
    };
    if !proxy.is_file() {
        return Err(format!(
            "Rust WAN proxy is missing at {}; build the workspace binaries or set QOMM_WAN_PROXY_BIN",
            proxy.display()
        )
        .into());
    }
    let mut child = Command::new(proxy)
        .arg("--config")
        .arg(run_dir.join("proxy.json"))
        .arg("--ready")
        .arg(&ready)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Err("proxy died before becoming ready".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    if !ready.exists() {
        let _ = child.kill();
        return Err("proxy did not become ready".into());
    }
    Ok(child)
}

fn verify(mode: Mode, log: &str, reference: &Value) -> HarnessResult<(bool, String)> {
    let padded = integer(reference, "padded_mm")?;
    let mask = if reference.get("mask").is_some() {
        integer(reference, "mask")?
    } else {
        0
    };
    match mode {
        Mode::Rfq => {
            let Some(masked) = named_integer(log, "QOMM_MASKED_KEY=") else {
                return Ok((false, "no masked quote in log".into()));
            };
            let got = unpack_key(masked - mask, padded);
            let want = (
                integer(reference, "best_cost")?,
                integer(reference, "best_mm")?,
            );
            Ok((
                got == want,
                format!("got=({}, {}) want=({}, {})", got.0, got.1, want.0, want.1),
            ))
        }
        Mode::Rfm => {
            let (Some(ask), Some(bid)) = (
                named_integer(log, "QOMM_MASKED_ASK="),
                named_integer(log, "QOMM_MASKED_BID="),
            ) else {
                return Ok((false, "no two-sided quote in log".into()));
            };
            let got = (
                unpack_key(ask - mask, padded),
                unpack_key(bid - mask, padded),
            );
            let want = (
                (
                    integer(reference, "best_ask")?,
                    integer(reference, "best_ask_mm")?,
                ),
                (
                    -integer(reference, "best_bid")?,
                    integer(reference, "best_bid_mm")?,
                ),
            );
            Ok((
                got == want,
                format!(
                    "got=(({}, {}), ({}, {})) want=(({}, {}), ({}, {}))",
                    got.0 .0,
                    got.0 .1,
                    got.1 .0,
                    got.1 .1,
                    want.0 .0,
                    want.0 .1,
                    want.1 .0,
                    want.1 .1
                ),
            ))
        }
        Mode::Rfs => {
            let mut steps = Vec::new();
            for line in log.lines() {
                if let Some(rest) = line.strip_prefix("QOMM_RFS_STEP_") {
                    if let Some((index, key)) = rest.split_once("_KEY=") {
                        if let (Ok(index), Ok(key)) = (index.parse::<usize>(), key.parse::<i128>())
                        {
                            steps.push((index, key));
                        }
                    }
                }
            }
            steps.sort_by_key(|row| row.0);
            let Some((_, key)) = steps.first() else {
                return Ok((false, "no RFS price series in log".into()));
            };
            let first = unpack_key(*key, padded);
            let want = (
                integer(reference, "best_cost")?,
                integer(reference, "best_mm")?,
            );
            Ok((
                first == want,
                format!(
                    "steps={} first=({}, {}) want=({}, {})",
                    steps.len(),
                    first.0,
                    first.1,
                    want.0,
                    want.1
                ),
            ))
        }
    }
}

fn parse_party_total(text: &str) -> Option<PartyTotal> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("QOMM") || fields.next() != Some("total") {
            continue;
        }
        let rounds = fields.next()?.parse().ok()?;
        let _raw_rounds: u64 = fields.next()?.parse().ok()?;
        let sent = fields.next()?.parse().ok()?;
        let _payload: u64 = fields.next()?.parse().ok()?;
        let seconds = fields.next()?.parse().ok()?;
        return Some(PartyTotal {
            rounds,
            sent,
            seconds,
        });
    }
    None
}

fn stock_protocol(binary: &str) -> HarnessResult<Protocol> {
    match binary {
        "malicious-shamir-party.x" => Ok(Protocol::MaliciousShamir),
        "shamir-party.x" => Ok(Protocol::SemiHonestShamir),
        _ => Err(format!(
            "{binary} is not linked by qomm-mpc; supported: malicious-shamir-party.x, shamir-party.x"
        )
        .into()),
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

fn named_integer(log: &str, prefix: &str) -> Option<i128> {
    log.lines()
        .find_map(|line| line.strip_prefix(prefix)?.trim().parse().ok())
}

fn integer(value: &Value, key: &str) -> HarnessResult<i128> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .map(i128::from)
        .or_else(|| value.get(key).and_then(Value::as_u64).map(i128::from))
        .ok_or_else(|| format!("reference has no integer {key}").into())
}

fn unpack_key(key: i128, padded: i128) -> (i128, i128) {
    let index = key.rem_euclid(padded);
    ((key - index) / padded, index)
}

fn numeric_field(rows: &[Value], key: &str) -> Vec<f64> {
    rows.iter().filter_map(|row| row[key].as_f64()).collect()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.len() % 2 == 1 {
        sorted[sorted.len() / 2]
    } else {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
    }
}

fn minimum(values: &[f64]) -> f64 {
    values.iter().copied().fold(f64::INFINITY, f64::min)
}

/// Six significant digits, the way MP-SPDZ's own `%g` output would have them.
///
/// rounds it. Computing it from the byte count instead means reproducing what
/// the engine would have printed, and C's formatted output rounds half to even
/// while Rust's `f64::round` rounds half away from zero. On an exact tie those
/// disagree in the last digit reported.
fn six_significant(value: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    let digits = value.abs().log10().floor() + 1.0;
    let scale = 10_f64.powf(6.0 - digits);
    let scaled = value * scale;
    let rounded = if scaled < 0.0 {
        -(qomm_sim::market::round_half_even(-scaled) as f64)
    } else {
        qomm_sim::market::round_half_even(scaled) as f64
    };
    rounded / scale
}

fn tail_chars(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

fn decimal_bit_length(decimal: &str) -> usize {
    let mut digits: Vec<u8> = decimal
        .trim_start_matches('+')
        .bytes()
        .map(|byte| byte - b'0')
        .collect();
    while digits.first() == Some(&0) {
        digits.remove(0);
    }
    let mut bits = 0;
    while !digits.is_empty() {
        let mut carry = 0u16;
        for digit in &mut digits {
            let value = carry * 10 + *digit as u16;
            *digit = (value / 2) as u8;
            carry = value % 2;
        }
        while digits.first() == Some(&0) {
            digits.remove(0);
        }
        bits += 1;
    }
    bits
}

fn parse_args() -> HarnessResult<Options> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut options = Options::default();
    let mut index = 0;
    while index < raw.len() {
        let argument = &raw[index];
        if argument == "-h" || argument == "--help" {
            println!("{}", usage());
            std::process::exit(0);
        }
        let (name, attached) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(name, value)| {
                (name, Some(value.to_string()))
            });
        let take = |index: &mut usize| -> HarnessResult<String> {
            if let Some(value) = attached.clone() {
                Ok(value)
            } else {
                *index += 1;
                raw.get(*index)
                    .cloned()
                    .ok_or_else(|| format!("argument {name} expects one value").into())
            }
        };
        macro_rules! number {
            ($field:expr, $kind:ty) => {{
                let text = take(&mut index)?;
                $field = text
                    .parse::<$kind>()
                    .map_err(|error| format!("invalid {name}: {error}"))?;
            }};
        }
        match name {
            "--mp-spdz-root" => options.mp_spdz_root = Some(PathBuf::from(take(&mut index)?)),
            "--n-mm" => number!(options.n_mm, usize),
            "--n-parties" => number!(options.n_parties, usize),
            "--threshold" => number!(options.threshold, usize),
            "--mode" => {
                let value = take(&mut index)?;
                options.mode = Mode::parse(&value).ok_or("--mode expects rfq, rfm, or rfs")?;
            }
            "--rfs-steps" => number!(options.rfs_steps, usize),
            "--disclose" => {
                let value = take(&mut index)?;
                options.disclose =
                    Disclosure::parse(&value).ok_or("--disclose expects none or threshold")?;
            }
            "--delay-ms" => number!(options.delay_ms, f64),
            "--per-party-ms" => {
                let mut values = Vec::new();
                if let Some(value) = attached {
                    values.push(value.parse()?);
                }
                while raw
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with("--"))
                {
                    index += 1;
                    values.push(raw[index].parse()?);
                }
                options.per_party_ms = Some(values);
            }
            "--repeats" => number!(options.repeats, usize),
            "--user-qty" => number!(options.user_qty, i128),
            "--user-dir" => number!(options.user_dir, i128),
            "--seed" => number!(options.seed, i128),
            "--field-bits" => number!(options.field_bits, i128),
            "--bit-length" => number!(options.bit_length, u32),
            "--argmin-arity" => number!(options.argmin_arity, usize),
            "--edabit" => options.edabit = true,
            "--input-check" => options.input_check = true,
            "--trunc-pr" => options.trunc_pr = true,
            "--protocol" => options.protocol = take(&mut index)?,
            "--stop-after" => {
                let value = take(&mut index)?;
                options.stop_after = StopAfter::parse(&value)
                    .ok_or("--stop-after expects price, direction, gates, or tournament")?;
            }
            "--prepare-only" => options.prepare_only = true,
            "--is-real" => number!(options.is_real, i128),
            "--n-assets" => number!(options.n_assets, usize),
            "--batch-size" => {
                let text = take(&mut index)?;
                options.batch_size = Some(
                    text.parse::<usize>()
                        .map_err(|error| format!("invalid {name}: {error}"))?,
                );
            }
            "--n-requests" => number!(options.n_requests, usize),
            "--public-maker-assets" => options.public_maker_assets = true,
            "--audit-gates" => options.audit_gates = true,
            "--binding-limit" => options.binding_limit = true,
            "--user-limit" => number!(options.user_limit, i128),
            "--check-mode" => {
                let value = take(&mut index)?;
                options.check_mode = CheckMode::parse(&value)
                    .ok_or("--check-mode expects aggregate or per-party")?;
            }
            "--unsound-check-for-measurement" => options.unsound_check_for_measurement = true,
            "--file-prep" => options.file_prep = true,
            "--user-asset" => number!(options.user_asset, usize),
            "--prime" => options.prime = Some(take(&mut index)?),
            "--reference" => {
                let value = take(&mut index)?;
                options.reference =
                    Reference::parse(&value).ok_or("--reference expects anchored or none")?;
            }
            "--use-ref" => number!(options.use_ref, i128),
            "--persist-wires" => options.persist_wires = true,
            "--persist-zkpi-wires" => options.persist_zkpi_wires = true,
            "--persist-dvp-wires" => options.persist_dvp_wires = true,
            "--zkpi-amount-bits" => number!(options.zkpi_amount_bits, usize),
            "--zkpi-price-bits" => number!(options.zkpi_price_bits, usize),
            "--dvp-remainder-bits" => number!(options.dvp_remainder_bits, usize),
            "--taker-securities-reserve" => {
                let text = take(&mut index)?;
                options.taker_securities_reserve = Some(text.parse()?);
            }
            "--taker-securities-blinding" => {
                let text = take(&mut index)?;
                options.taker_securities_blinding = Some(text.parse()?);
            }
            "--taker-cash-reserve" => {
                let text = take(&mut index)?;
                options.taker_cash_reserve = Some(text.parse()?);
            }
            "--taker-cash-blinding" => {
                let text = take(&mut index)?;
                options.taker_cash_blinding = Some(text.parse()?);
            }
            "--maker-securities-reserve" => {
                let text = take(&mut index)?;
                options.maker_securities_reserve = Some(text.parse()?);
            }
            "--maker-securities-blinding" => {
                let text = take(&mut index)?;
                options.maker_securities_blinding = Some(text.parse()?);
            }
            "--maker-cash-reserve" => {
                let text = take(&mut index)?;
                options.maker_cash_reserve = Some(text.parse()?);
            }
            "--maker-cash-blinding" => {
                let text = take(&mut index)?;
                options.maker_cash_blinding = Some(text.parse()?);
            }
            "--shamir-inputs" => options.shamir_inputs = true,
            "--tag" => options.tag = take(&mut index)?,
            "--out" => options.out = Some(PathBuf::from(take(&mut index)?)),
            other => return Err(format!("unknown argument {other}").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn usage() -> &'static str {
    "usage: run_qomm [OPTIONS]\n\
     The default protocol is malicious-shamir-party.x; execution is through qomm-mpc/libSPDZ."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_packed_cost_uses_floor_division() {
        assert_eq!(unpack_key(-399, 16), (-25, 1));
    }

    #[test]
    fn rfq_verification_subtracts_a_mask_above_i64_max() {
        let mask = 18_408_253_335_210_568_581_u64;
        let best_cost = 1_i128 << 60;
        let opened = i128::from(mask) + best_cost * 2;
        let reference = json!({
            "padded_mm": 2,
            "mask": mask,
            "best_cost": best_cost,
            "best_mm": 0,
        });

        let (verified, detail) = verify(
            Mode::Rfq,
            &format!("QOMM_MASKED_KEY={opened}\n"),
            &reference,
        )
        .expect("RFQ verification");

        assert!(verified, "{detail}");
    }

    #[test]
    fn ed25519_order_has_the_required_bit_length() {
        assert_eq!(decimal_bit_length(ED25519_ORDER), 253);
    }

    #[test]
    fn mp_spdz_megabytes_keep_six_significant_digits() {
        assert_eq!(six_significant(3.312688), 3.31269);
        assert_eq!(six_significant(19.261776), 19.2618);
    }
}
