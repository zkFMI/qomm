//! Measure bit-decomposition circuit rounds through the official MP-SPDZ toolchain.
//!
//! MP-SPDZ compiles and executes the generated program. When qomm-mpc is linked
//! against libSPDZ, each party is a child invocation of this binary and the
//! crate reads the engine counters directly. A build without that optional
//! engine uses the explicitly named stock `--binary`, whose output is the
//! engine's measurement interface.

use qomm_harness::{parse_value, write_pretty_json, HarnessResult};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::Protocol;
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const ARMS: [(&str, &str); 4] = [
    ("inputs_only", "    keep.append(a + b)"),
    ("compare", "    keep.append((a < b).if_else(a, b))"),
    (
        "bitdec",
        "    keep.append(sum((a - b).bit_decompose(BITS)))",
    ),
    (
        "compare_and_bits",
        "    bits = (a - b).bit_decompose(BITS)\n    keep.append((a < b).if_else(a, b) + sum(bits))",
    ),
];

struct Options {
    root: PathBuf,
    out: PathBuf,
    counts: Vec<usize>,
    bits: Vec<usize>,
    bitlen: usize,
    parties: usize,
    threshold: usize,
    repeats: usize,
    binary: String,
}

fn main() {
    let mut args = std::env::args_os();
    let _ = args.next();
    let result = if args.next().as_deref() == Some(std::ffi::OsStr::new("__party")) {
        party_main(args.collect())
    } else {
        run_main()
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    validate(&options)?;
    if !qomm_mpc::available() {
        eprintln!(
            "qomm-mpc was built without libSPDZ; using the explicitly requested stock --binary {}",
            options.binary
        );
    }
    write_inputs(
        &options.root,
        options.parties,
        *options.counts.iter().max().expect("validated counts"),
    )?;

    let mut rows = Vec::new();
    for (arm, body) in ARMS {
        for &bits in &options.bits {
            for &count in &options.counts {
                let name = format!("bdr_{arm}_{count}_{bits}_noeda");
                let source = program_source(arm, body, count, bits, options.bitlen, false);
                let got = (0..options.repeats)
                    .map(|_| one(&options, &name, &source))
                    .collect::<Vec<_>>();
                let ok = got
                    .iter()
                    .filter(|row| row.get("rounds").is_some())
                    .collect::<Vec<_>>();
                let mut row = Map::new();
                row.insert("arm".into(), json!(arm));
                row.insert("n".into(), json!(count));
                row.insert("edabit".into(), json!(false));
                row.insert("bits".into(), json!(bits));
                row.insert("repeats".into(), json!(ok.len()));
                if !ok.is_empty() {
                    let mut rounds = numeric_values(&ok, "rounds")?;
                    let mut mb = float_values(&ok, "mb")?;
                    let mut seconds = float_values(&ok, "seconds")?;
                    rounds.sort_unstable();
                    mb.sort_by(f64::total_cmp);
                    seconds.sort_by(f64::total_cmp);
                    row.insert("rounds".into(), json!(rounds[rounds.len() / 2]));
                    row.insert("mb".into(), json!(mb[mb.len() / 2]));
                    row.insert("seconds".into(), json!(seconds[seconds.len() / 2]));
                    row.insert(
                        "rounds_spread".into(),
                        json!([rounds[0], rounds[rounds.len() - 1]]),
                    );
                } else {
                    row.insert(
                        "error".into(),
                        got.first()
                            .and_then(|value| value.get("error"))
                            .cloned()
                            .unwrap_or_else(|| json!("no round count")),
                    );
                }
                row.insert(
                    "vm_rounds".into(),
                    got.first()
                        .and_then(|value| value.get("vm_rounds"))
                        .cloned()
                        .unwrap_or(Value::Null),
                );
                println!(
                    "{arm:17} N={count:3} bits={bits:3} -> rounds={} mb={} s={} {}",
                    display(&row, "rounds"),
                    display(&row, "mb"),
                    display(&row, "seconds"),
                    row.get("error").and_then(Value::as_str).unwrap_or("")
                );
                rows.push(Value::Object(row));
            }
        }
    }

    let mut marginal = Map::new();
    for &bits in &options.bits {
        for &count in &options.counts {
            let base = find_row(&rows, "compare", count, bits);
            let both = find_row(&rows, "compare_and_bits", count, bits);
            if let (Some(base), Some(both)) = (base, both) {
                if let (Some(base_rounds), Some(both_rounds), Some(base_mb), Some(both_mb)) = (
                    base["rounds"].as_i64(),
                    both["rounds"].as_i64(),
                    base["mb"].as_f64(),
                    both["mb"].as_f64(),
                ) {
                    marginal.insert(
                        format!("{bits}bit_n{count}"),
                        json!({
                            "comparison_alone": base_rounds,
                            "comparison_and_bits": both_rounds,
                            "extra_rounds": both_rounds - base_rounds,
                            "extra_mb": round_half_even_places(both_mb - base_mb, 3),
                        }),
                    );
                }
            }
        }
    }

    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "bits": options.bits,
        "parties": options.parties,
        "threshold": options.threshold,
        "binary": options.binary,
        "marginal": marginal,
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn one(options: &Options, name: &str, source: &str) -> Value {
    match one_result(options, name, source) {
        Ok(value) => value,
        Err(error) => json!({"error": error.to_string()}),
    }
}

fn one_result(options: &Options, name: &str, source: &str) -> HarnessResult<Value> {
    let source_path = options
        .root
        .join("Programs/Source")
        .join(format!("{name}.mpc"));
    fs::create_dir_all(source_path.parent().expect("source path has a parent"))?;
    fs::write(&source_path, source)?;

    let compiler = OfficialCompiler::from_checkout(&options.root)?;
    let compile = compiler.compile_field(128, name)?;
    let compile_text = format!(
        "{}{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    if !compile.status.success() {
        return Ok(json!({
            "error": format!("compile: {}", last_line(&compile_text)),
        }));
    }
    let vm_rounds = phrase_integer(&compile_text, "virtual machine rounds");

    let started = Instant::now();
    let execution = execute_parties(options, name)?;
    let wall_seconds = started.elapsed().as_secs_f64();
    if let Some(error) = execution.error {
        return Ok(json!({
            "wall_seconds": wall_seconds,
            "error": format!("run: {}", last_line(&error).chars().take(200).collect::<String>()),
            "vm_rounds": vm_rounds,
        }));
    }
    Ok(json!({
        "wall_seconds": wall_seconds,
        "mb": execution.mb,
        "rounds": execution.rounds,
        "seconds": execution.seconds,
        "vm_rounds": vm_rounds,
    }))
}

struct Execution {
    rounds: u64,
    mb: f64,
    seconds: f64,
    error: Option<String>,
}

fn execute_parties(options: &Options, name: &str) -> HarnessResult<Execution> {
    let protocol = binary_protocol(&options.binary);
    let use_embedded = qomm_mpc::available() && protocol.is_some();
    let executable = std::env::current_exe()?;
    let stock = if Path::new(&options.binary).is_absolute() {
        PathBuf::from(&options.binary)
    } else {
        options.root.join(&options.binary)
    };
    let mut children = Vec::new();
    for party in 0..options.parties {
        let mut command = if use_embedded {
            let mut command = Command::new(&executable);
            command
                .arg("__party")
                .arg(protocol.expect("checked protocol").as_str());
            command
        } else {
            Command::new(&stock)
        };
        command
            .current_dir(&options.root)
            .arg(party.to_string())
            .arg(name)
            .args(["-N", &options.parties.to_string()])
            .args(["-T", &options.threshold.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match command.spawn() {
            Ok(child) => children.push(child),
            Err(error) => {
                return Ok(Execution {
                    rounds: 0,
                    mb: 0.0,
                    seconds: 0.0,
                    error: Some(format!("could not start party {party}: {error}")),
                })
            }
        }
    }

    let mut outputs = Vec::new();
    let mut failure = None;
    for (party, child) in children.into_iter().enumerate() {
        let output = child.wait_with_output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            failure.get_or_insert_with(|| format!("party {party}: {text}"));
        }
        outputs.push(text);
    }
    if let Some(error) = failure {
        return Ok(Execution {
            rounds: 0,
            mb: 0.0,
            seconds: 0.0,
            error: Some(error),
        });
    }
    let first = outputs.first().ok_or("no party output")?;
    if use_embedded {
        parse_embedded(first)
    } else {
        parse_stock(first)
    }
}

fn parse_embedded(text: &str) -> HarnessResult<Execution> {
    for line in text.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() >= 7 && fields[0] == "QOMM" && fields[1] == "total" {
            let rounds = fields[2].parse::<u64>()?;
            let sent = fields[4].parse::<u64>()?;
            let seconds = fields[6].parse::<f64>()?;
            return Ok(Execution {
                rounds,
                mb: six_significant(sent as f64 / 1_000_000.0),
                seconds,
                error: None,
            });
        }
    }
    Err("an embedded party returned no QOMM total line".into())
}

fn parse_stock(text: &str) -> HarnessResult<Execution> {
    let mut rounds = None;
    let mut mb = None;
    let mut seconds = None;
    for line in text.lines() {
        if let Some(at) = line.find("Data sent") {
            let line = &line[at..];
            if let Some((_, after_equals)) = line.split_once('=') {
                mb = after_equals
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok());
            }
            if let Some((_, after_rounds)) = line.split_once("in ~") {
                rounds = after_rounds
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.replace(',', "").parse().ok());
            }
        }
        if let Some(after) = line.strip_prefix("Time") {
            seconds = after
                .split_once('=')
                .and_then(|(_, value)| value.split_whitespace().next())
                .and_then(|value| value.parse().ok());
        }
    }
    match (rounds, mb, seconds) {
        (Some(rounds), Some(mb), Some(seconds)) => Ok(Execution {
            rounds,
            mb,
            seconds,
            error: None,
        }),
        _ => Err("party 0 output contained no complete runtime counters".into()),
    }
}

fn party_main(args: Vec<OsString>) -> HarnessResult<()> {
    let strings = args
        .into_iter()
        .map(|arg| arg.into_string().map_err(|_| "party argument is not UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    let (protocol_name, party_args) = strings
        .split_first()
        .ok_or("internal party mode expects a protocol")?;
    let protocol = Protocol::parse(protocol_name)
        .ok_or_else(|| format!("unknown embedded protocol {protocol_name}"))?;
    let me = std::env::current_exe()?.to_string_lossy().to_string();
    let mut argv = vec![me.as_str()];
    argv.extend(party_args.iter().map(String::as_str));
    let run = qomm_mpc::run(protocol, &argv)?;
    println!(
        "QOMM total {} {} {} {} {:.6}",
        run.rounds, run.raw_rounds, run.sent, run.payload, run.seconds
    );
    Ok(())
}

fn write_inputs(root: &Path, parties: usize, count: usize) -> HarnessResult<()> {
    let directory = root.join("Player-Data");
    fs::create_dir_all(&directory)?;
    for party in 0..parties {
        let values = (0..count)
            .map(|index| {
                if party == 0 {
                    (1_000 + 7 * index).to_string()
                } else {
                    (500 + 3 * index).to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            directory.join(format!("Input-P{party}-0")),
            format!("{values}\n"),
        )?;
    }
    Ok(())
}

fn program_source(
    arm: &str,
    body: &str,
    count: usize,
    bits: usize,
    bitlen: usize,
    edabit: bool,
) -> String {
    format!(
        "# generated by the QOMM Rust harness --- {arm}, N={count}, {bits}-bit values\nprogram.set_bit_length({bitlen})\nprogram.use_edabit({})\n\nN = {count}\nBITS = {bits}\nkeep = []\nfor i in range(N):\n    a = sint.get_input_from(0)\n    b = sint.get_input_from(1)\n{body}\n\ntotal = keep[0]\nfor x in keep[1:]:\n    total = total + x\nprint_ln('%s', total.reveal())\n",
        if edabit { "True" } else { "False" }
    )
}

fn find_row<'a>(rows: &'a [Value], arm: &str, count: usize, bits: usize) -> Option<&'a Value> {
    rows.iter().find(|row| {
        row["arm"] == arm
            && row["n"].as_u64() == Some(count as u64)
            && row["bits"].as_u64() == Some(bits as u64)
    })
}

fn numeric_values(rows: &[&Value], key: &str) -> HarnessResult<Vec<i64>> {
    rows.iter()
        .map(|row| {
            row[key]
                .as_i64()
                .ok_or_else(|| format!("{key} is not an integer").into())
        })
        .collect()
}

fn float_values(rows: &[&Value], key: &str) -> HarnessResult<Vec<f64>> {
    rows.iter()
        .map(|row| {
            row[key]
                .as_f64()
                .ok_or_else(|| format!("{key} is not a number").into())
        })
        .collect()
}

fn phrase_integer(text: &str, phrase: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let at = line.find(phrase)?;
        line[..at]
            .split_whitespace()
            .last()?
            .replace(',', "")
            .parse()
            .ok()
    })
}

fn binary_protocol(binary: &str) -> Option<Protocol> {
    match Path::new(binary).file_name()?.to_str()? {
        "malicious-shamir-party.x" => Some(Protocol::MaliciousShamir),
        "shamir-party.x" => Some(Protocol::SemiHonestShamir),
        _ => None,
    }
}

fn six_significant(value: f64) -> f64 {
    if value == 0.0 {
        return 0.0;
    }
    let digits = value.abs().log10().floor() + 1.0;
    let scale = 10_f64.powf(6.0 - digits);
    round_half_even(value * scale) / scale
}

fn round_half_even_places(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    round_half_even(value * scale) / scale
}

fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    let fraction = value - floor;
    if fraction > 0.5 || (fraction == 0.5 && (floor as i128) % 2 != 0) {
        floor + 1.0
    } else {
        floor
    }
}

fn last_line(text: &str) -> &str {
    text.trim().lines().last().unwrap_or("")
}

fn display(row: &Map<String, Value>, key: &str) -> String {
    row.get(key).map_or_else(|| "None".into(), Value::to_string)
}

fn validate(options: &Options) -> HarnessResult<()> {
    if options.counts.is_empty() || options.counts.contains(&0) {
        return Err("--counts must contain positive values".into());
    }
    if options.bits.is_empty() || options.bits.contains(&0) {
        return Err("--bits must contain positive values".into());
    }
    if options.parties <= 2 * options.threshold {
        return Err("malicious Shamir requires --parties greater than twice --threshold".into());
    }
    if options.repeats == 0 {
        return Err("--repeats must be positive".into());
    }
    OfficialCompiler::from_checkout(&options.root)?;
    Ok(())
}

fn parse_args() -> HarnessResult<Options> {
    let mut root = None;
    let mut out = None;
    let mut options = Options {
        root: PathBuf::new(),
        out: PathBuf::new(),
        counts: vec![1, 16, 48],
        bits: vec![26],
        bitlen: 128,
        parties: 7,
        threshold: 2,
        repeats: 5,
        binary: "malicious-shamir-party.x".into(),
    };
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        match flag.as_ref() {
            "--root" => root = Some(PathBuf::from(value(&args, &mut index, "--root")?)),
            "--out" => out = Some(PathBuf::from(value(&args, &mut index, "--out")?)),
            "--counts" => options.counts = list(&args, &mut index, "--counts")?,
            "--bits" => options.bits = list(&args, &mut index, "--bits")?,
            "--bitlen" => {
                options.bitlen = parse_value(value(&args, &mut index, "--bitlen")?, "--bitlen")?
            }
            "--parties" => {
                options.parties = parse_value(value(&args, &mut index, "--parties")?, "--parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&args, &mut index, "--threshold")?, "--threshold")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&args, &mut index, "--repeats")?, "--repeats")?
            }
            "--binary" => {
                options.binary = value(&args, &mut index, "--binary")?
                    .into_string()
                    .map_err(|_| "--binary is not UTF-8")?
            }
            "-h" | "--help" => {
                println!("usage: run_bitdec_rounds --root PATH --out PATH [--counts N...] [--bits N...] [--bitlen N] [--parties N] [--threshold N] [--repeats N] [--binary NAME]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {flag}").into()),
        }
        index += 1;
    }
    options.root = root.ok_or("--root is required")?;
    options.out = out.ok_or("--out is required")?;
    Ok(options)
}

fn value(args: &[OsString], index: &mut usize, flag: &str) -> HarnessResult<OsString> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("argument {flag} expects one value").into())
}

fn list<T>(args: &[OsString], index: &mut usize, flag: &str) -> HarnessResult<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let mut values = Vec::new();
    while *index + 1 < args.len() && !args[*index + 1].to_string_lossy().starts_with("--") {
        *index += 1;
        values.push(parse_value(args[*index].clone(), flag)?);
    }
    if values.is_empty() {
        return Err(format!("argument {flag} expects at least one value").into());
    }
    Ok(values)
}
