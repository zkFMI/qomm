//! Measure the MPC compiler cost of secret conditionals in maker price rules.
//!
//! QOMM owns program generation, orchestration, metric parsing, and the
//! artifact. The only non-Rust step is the official compiler shipped inside a
//! verified MP-SPDZ checkout.

use qomm_harness::{next_value, parse_value, write_pretty_json, HarnessResult};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::program::{build_program, CheckMode, Mode, ProgramConfig};
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct Options {
    mp_spdz_root: PathBuf,
    makers: Vec<usize>,
    conditionals: Vec<usize>,
    bit_length: u32,
    out: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct CompileCost {
    vm_rounds: u64,
    integer_triples: u64,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let root = options.mp_spdz_root.canonicalize()?;
    let compiler = OfficialCompiler::from_checkout(&root)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut rows = Vec::with_capacity(options.makers.len() * options.conditionals.len());

    for makers in &options.makers {
        if !makers.is_power_of_two() {
            return Err(format!("--makers value {makers} is not a power of two").into());
        }
        for conditionals in &options.conditionals {
            let program = format!(
                "qomm_price_conditionals_{}_{}_m{}_c{}",
                std::process::id(),
                nonce,
                makers,
                conditionals
            );
            let source = root.join("Programs/Source").join(format!("{program}.mpc"));
            let config = ProgramConfig {
                n_mm: *makers,
                mode: Mode::Rfq,
                bit_length: options.bit_length,
                price_conditionals: *conditionals,
                check_mode: CheckMode::PerParty,
                ..ProgramConfig::default()
            };
            fs::create_dir_all(source.parent().expect("source has a parent"))?;
            fs::write(&source, build_program(&config)?)?;

            let compile = compiler.compile_field(128, &program);
            let cleanup_result = cleanup_program(&root, &program);
            let output = compile?;
            cleanup_result?;
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                return Err(
                    format!("official MP-SPDZ compile failed for {program}:\n{text}").into(),
                );
            }
            let cost = parse_compile_cost(&text)?;
            println!(
                "makers={makers} conditionals={conditionals} rounds={} triples={}",
                cost.vm_rounds, cost.integer_triples
            );
            rows.push(json!({
                "n_mm": makers,
                "conditionals": conditionals,
                "vm_rounds": cost.vm_rounds,
                "integer_triples": cost.integer_triples,
            }));
        }
    }

    write_pretty_json(Some(&options.out), &Value::Array(rows))?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn parse_compile_cost(text: &str) -> HarnessResult<CompileCost> {
    let vm_rounds = capture_metric(text, r"(?m)([0-9][0-9,]*)\s+virtual machine rounds")?;
    let integer_triples = capture_metric(text, r"(?m)([0-9][0-9,]*)\s+integer triples")?;
    Ok(CompileCost {
        vm_rounds,
        integer_triples,
    })
}

fn capture_metric(text: &str, pattern: &str) -> HarnessResult<u64> {
    let regex = Regex::new(pattern)?;
    let raw = regex
        .captures(text)
        .and_then(|captures| captures.get(1))
        .ok_or_else(|| format!("compiler output did not contain metric {pattern:?}"))?
        .as_str()
        .replace(',', "");
    Ok(raw.parse()?)
}

fn cleanup_program(root: &Path, program: &str) -> HarnessResult<()> {
    for path in [
        root.join("Programs/Source").join(format!("{program}.mpc")),
        root.join("Programs/Schedules")
            .join(format!("{program}.sch")),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let bytecode = root.join("Programs/Bytecode");
    if bytecode.is_dir() {
        for entry in fs::read_dir(bytecode)? {
            let path = entry?.path();
            let is_ours = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{program}-")) && name.ends_with(".bc")
                });
            if is_ours {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

fn parse_args() -> HarnessResult<Options> {
    let mut mp_spdz_root = std::env::var_os("MP_SPDZ_ROOT").map(PathBuf::from);
    let mut makers = Vec::new();
    let mut conditionals = Vec::new();
    let mut bit_length = 31_u32;
    let mut out = None;
    let mut args = std::env::args_os().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--mp-spdz-root") => {
                mp_spdz_root = Some(PathBuf::from(next_value(&mut args, "--mp-spdz-root")?));
            }
            Some("--makers") => {
                makers = parse_list(&mut args, "--makers")?;
            }
            Some("--conditionals") => {
                conditionals = parse_list(&mut args, "--conditionals")?;
            }
            Some("--bit-length") => {
                bit_length = parse_value(next_value(&mut args, "--bit-length")?, "--bit-length")?;
            }
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("-h" | "--help") => {
                println!(
                    "usage: run_price_conditionals --mp-spdz-root PATH --makers N... \\\n  --conditionals N... [--bit-length N] --out PATH"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    if makers.is_empty() || conditionals.is_empty() {
        return Err("--makers and --conditionals each require at least one value".into());
    }
    if !(1..=127).contains(&bit_length) {
        return Err("--bit-length must be in 1..=127".into());
    }
    Ok(Options {
        mp_spdz_root: mp_spdz_root.ok_or("set MP_SPDZ_ROOT or pass --mp-spdz-root")?,
        makers,
        conditionals,
        bit_length,
        out: out.ok_or("--out is required")?,
    })
}

fn parse_list<I>(args: &mut std::iter::Peekable<I>, flag: &str) -> HarnessResult<Vec<usize>>
where
    I: Iterator<Item = std::ffi::OsString>,
{
    let mut values = Vec::new();
    while args
        .peek()
        .is_some_and(|value| !value.to_string_lossy().starts_with('-'))
    {
        values.push(parse_value(next_value(args, flag)?, flag)?);
    }
    if values.is_empty() {
        return Err(format!("{flag} requires at least one integer").into());
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commas_and_both_required_metrics() {
        let text = "53 virtual machine rounds\n3,222 integer triples\n";
        assert_eq!(
            parse_compile_cost(text).unwrap(),
            CompileCost {
                vm_rounds: 53,
                integer_triples: 3_222,
            }
        );
    }

    #[test]
    fn missing_metric_fails_closed() {
        assert!(parse_compile_cost("53 virtual machine rounds\n").is_err());
    }
}
