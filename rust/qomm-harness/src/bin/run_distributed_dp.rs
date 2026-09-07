//! Run the distributed discrete-Laplace release inside MP-SPDZ.
//!
//! `DpMechanism::mp_spdz_source` has existed since the audit layer was written
//! and nothing ever compiled it. The noise that reached a release was drawn by
//! `discrete_laplace` from one RNG in one process, so the mechanism was
//! distributed on paper and central in fact. This runs it: the parties supply
//! their counts, the uniform comes from shared random bits nobody sees, and the
//! only thing revealed is the sum plus the noise.
//!
//! What it measures is whether the noise costs one comparison layer or
//! `2*support` of them, and whether what comes out is the distribution the
//! privacy argument is about. Predictions in
//! `artifacts/distributed_dp_prediction.json`, written before the first run.

use qomm_audit::distributed_dp::{DpMechanism, U64_SPACE};
use qomm_harness::local_mpc::LocalMpcRun;
use qomm_harness::{fmean, parse_value, unique_temp_dir, write_pretty_json, HarnessResult};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

struct Options {
    root: PathBuf,
    parties: usize,
    threshold: usize,
    supports: Vec<u16>,
    samples: usize,
    epsilon_micros: u64,
    sensitivity: u64,
    field_bits: usize,
    protocols: Vec<String>,
    out: PathBuf,
}

/// The engine this deployment runs. n=7 with T=2 is t < n/3, so a linear reveal
/// is robust without assuming a broadcast channel.
const DEPLOYED: &str = "malicious-shamir";

/// The release without a mechanism, for the share of the cost the noise is.
const PLAIN_SOURCE: &str = "\
from Compiler.types import sint
N_PARTIES = {n}
exact = sum(sint.get_input_from(p) for p in range(N_PARTIES))
print_ln('%s', exact.reveal())
";

fn main() {
    if qomm_harness::local_mpc::maybe_run_party() {
        return;
    }
    if let Err(error) = run() {
        eprintln!("run_distributed_dp: {error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let root = options
        .root
        .canonicalize()
        .map_err(|_| format!("MP_SPDZ_ROOT does not exist: {}", options.root.display()))?;

    // One count per party. Fixed, so every sample differs only in its noise.
    let counts: Vec<i64> = (0..options.parties).map(|p| 10 + p as i64).collect();
    let exact: i64 = counts.iter().sum();

    let mut protocols = Map::new();
    for protocol in &options.protocols {
        let binary = binary_for(protocol)?;
        if !root.join(binary).exists() {
            return Err(format!("{binary} missing under {}", root.display()).into());
        }
        let plain_source = PLAIN_SOURCE.replace("{n}", &options.parties.to_string());
        let plain = measure(
            &options,
            &root,
            Measurement {
                protocol,
                binary,
                source_text: &plain_source,
                tag: "plain",
                counts: &counts,
                samples: 1,
            },
        )?;
        let mut arms = Vec::new();
        for support in &options.supports {
            let mechanism =
                DpMechanism::new(options.epsilon_micros, options.sensitivity, *support)?;
            let source = mechanism.mp_spdz_source(
                options.parties,
                options.epsilon_micros * 8,
                0,
                "published",
            )?;
            let tag = format!("support{support}");
            let mut arm = measure(
                &options,
                &root,
                Measurement {
                    protocol,
                    binary,
                    source_text: &source,
                    tag: &tag,
                    counts: &counts,
                    samples: options.samples,
                },
            )?;
            let revealed = arm["revealed"]
                .as_array()
                .ok_or("revealed is not an array")?
                .iter()
                .map(|v| v.as_i64().ok_or("revealed value is not an integer"))
                .collect::<Result<Vec<_>, _>>()?;
            let noise: Vec<i64> = revealed.iter().map(|value| value - exact).collect();
            let object = arm.as_object_mut().ok_or("arm is not an object")?;
            object.insert("support".into(), json!(support));
            object.insert("noise".into(), json!(noise));
            object.insert("fit".into(), fit(&mechanism, &noise)?);
            if let (Some(r), Some(p)) = (arm["rounds"].as_u64(), plain["rounds"].as_u64()) {
                let object = arm.as_object_mut().expect("checked above");
                object.insert("rounds_over_plain".into(), json!(r as f64 / p as f64));
                object.insert(
                    "noise_share_of_rounds".into(),
                    json!((r.saturating_sub(p)) as f64 / r as f64),
                );
                let plain_mb = plain["global_mb"].as_f64().unwrap_or(0.0);
                if let Some(mb) = arm["global_mb"].as_f64().filter(|mb| *mb > 0.0) {
                    let object = arm.as_object_mut().expect("checked above");
                    object.insert("noise_share_of_bytes".into(), json!((mb - plain_mb) / mb));
                }
            }
            arms.push(arm);
        }
        protocols.insert(
            protocol.clone(),
            json!({"binary": binary, "plain": plain, "arms": arms}),
        );
    }

    let mut result = Map::new();
    result.insert("host".into(), json!(zkfmi_measure::hosts::this_host()));
    result.insert("deployed".into(), json!(DEPLOYED));
    result.insert(
        "why_the_second_protocol".into(),
        json!(
            "The semi-honest arm is not a deployment option; it is there to price what \
malicious security costs on this circuit, so the round count can be attributed to the \
construction rather than to the threat model."
        ),
    );
    result.insert("n_parties".into(), json!(options.parties));
    result.insert("threshold".into(), json!(options.threshold));
    result.insert("field_bits".into(), json!(options.field_bits));
    result.insert("epsilon_micros".into(), json!(options.epsilon_micros));
    result.insert("sensitivity".into(), json!(options.sensitivity));
    result.insert("exact".into(), json!(exact));
    result.insert("samples".into(), json!(options.samples));
    result.insert("protocols".into(), Value::Object(protocols));
    result.insert(
        "randomness".into(),
        json!("64 sint.get_random_bit() per release; no party observes the uniform"),
    );
    write_pretty_json(Some(options.out.as_path()), &Value::Object(result))?;
    println!("wrote {}", options.out.display());
    Ok(())
}

/// Compile once, run `samples` times, and keep what each run revealed.
struct Measurement<'a> {
    protocol: &'a str,
    binary: &'a str,
    source_text: &'a str,
    tag: &'a str,
    counts: &'a [i64],
    samples: usize,
}

fn measure(
    options: &Options,
    root: &std::path::Path,
    measurement: Measurement<'_>,
) -> HarnessResult<Value> {
    let Measurement {
        protocol,
        binary,
        source_text,
        tag,
        counts,
        samples,
    } = measurement;
    let work = unique_temp_dir("qomm-dist-dp")?;
    let source = work.join("prog.mpc");
    fs::write(&source, source_text)?;
    let party_files = counts
        .iter()
        .map(|count| format!("{count}\n"))
        .collect::<Vec<_>>();
    let program = format!("distdp_{}_{tag}_{}", &protocol[..4], std::process::id());
    let mut run = LocalMpcRun::new(
        root.to_path_buf(),
        program,
        options.parties,
        options.threshold,
        protocol,
        None,
    )?;
    run.install(&source, &party_files)?;
    let compile_rounds = run.compile(options.field_bits)?;

    let mut revealed = Vec::new();
    let mut rounds = None;
    let mut party0_mb = Vec::new();
    let mut global_mb = Vec::new();
    let mut wall = Vec::new();
    for _ in 0..samples {
        let sample = run.execute_stock(binary, &[])?;
        revealed.push(last_integer(&sample.combined)?);
        rounds.get_or_insert(sample.party0_rounds.ok_or("MP-SPDZ reported no rounds")?);
        if let Some(mb) = sample.party0_mb {
            party0_mb.push(mb);
        }
        if let Some(mb) = sample.global_mb {
            global_mb.push(mb);
        }
        wall.push(sample.wall_seconds);
    }
    let _ = fs::remove_dir_all(work);
    Ok(json!({
        "tag": tag,
        "protocol": protocol,
        "compile_rounds": compile_rounds,
        "rounds": rounds,
        "party0_mb": fmean(&party0_mb),
        "global_mb": fmean(&global_mb),
        "wall_seconds": fmean(&wall),
        "revealed": revealed,
    }))
}

/// The last integer a party printed, which is what the program revealed.
fn last_integer(text: &str) -> HarnessResult<i64> {
    text.lines()
        .rev()
        .find_map(|line| line.trim().parse::<i64>().ok())
        .ok_or_else(|| "no revealed integer in the party output".into())
}

/// Chi-square of the observed noise against the mechanism's own cell
/// probabilities. Not against the unquantised geometric: the quantisation is
/// provably below 1e-17 in total variation, so a disagreement here is the
/// program and not the rounding.
fn fit(mechanism: &DpMechanism, noise: &[i64]) -> HarnessResult<Value> {
    let endpoints = mechanism.thresholds()?;
    let support = i64::from(mechanism.support);
    let mut expected = Vec::with_capacity(endpoints.len());
    let mut previous = 0_u128;
    for endpoint in &endpoints {
        expected.push((endpoint - previous) as f64 / U64_SPACE as f64);
        previous = *endpoint;
    }
    let mut observed = vec![0_usize; expected.len()];
    let mut outside = 0_usize;
    for value in noise {
        let index = value + support;
        if index < 0 || index as usize >= observed.len() {
            outside += 1;
        } else {
            observed[index as usize] += 1;
        }
    }
    // Cells whose expectation is below five are pooled, which is the condition
    // the chi-square approximation needs and the reason a small sample cannot
    // resolve the tails.
    let n = noise.len() as f64;
    let mut statistic = 0.0;
    let mut cells = 0_usize;
    let mut pooled_observed = 0.0;
    let mut pooled_expected = 0.0;
    for (count, probability) in observed.iter().zip(expected.iter()) {
        let e = probability * n;
        if e >= 5.0 {
            statistic += (*count as f64 - e).powi(2) / e;
            cells += 1;
        } else {
            pooled_observed += *count as f64;
            pooled_expected += e;
        }
    }
    if pooled_expected > 0.0 {
        statistic += (pooled_observed - pooled_expected).powi(2) / pooled_expected;
        cells += 1;
    }
    Ok(json!({
        "n": noise.len(),
        "outside_support": outside,
        "cells_after_pooling": cells,
        "degrees_of_freedom": cells.saturating_sub(1),
        "chi_square": statistic,
        "mean": fmean(&noise.iter().map(|v| *v as f64).collect::<Vec<_>>()),
    }))
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        root: PathBuf::from(
            std::env::var_os("MP_SPDZ_ROOT").unwrap_or_else(|| OsString::from("MP-SPDZ")),
        ),
        parties: 7,
        threshold: 2,
        supports: vec![16, 64],
        samples: 200,
        epsilon_micros: 1_000_000,
        sensitivity: 1,
        field_bits: 128,
        protocols: vec![DEPLOYED.into(), "semi-honest-shamir".into()],
        out: qomm_harness::repo_root().join("artifacts/distributed_dp.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--root" => options.root = PathBuf::from(value(&raw, &mut index, "--root")?),
            "--parties" => {
                options.parties = parse_value(value(&raw, &mut index, "--parties")?, "--parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--supports" => {
                options.supports = Vec::new();
                while index + 1 < raw.len() && !raw[index + 1].to_string_lossy().starts_with("--") {
                    index += 1;
                    options
                        .supports
                        .push(parse_value(raw[index].clone(), "--supports")?);
                }
            }
            "--samples" => {
                options.samples = parse_value(value(&raw, &mut index, "--samples")?, "--samples")?
            }
            "--epsilon-micros" => {
                options.epsilon_micros = parse_value(
                    value(&raw, &mut index, "--epsilon-micros")?,
                    "--epsilon-micros",
                )?
            }
            "--sensitivity" => {
                options.sensitivity =
                    parse_value(value(&raw, &mut index, "--sensitivity")?, "--sensitivity")?
            }
            "--field-bits" => {
                options.field_bits =
                    parse_value(value(&raw, &mut index, "--field-bits")?, "--field-bits")?
            }
            "--protocols" => {
                options.protocols = Vec::new();
                while index + 1 < raw.len() && !raw[index + 1].to_string_lossy().starts_with("--") {
                    index += 1;
                    let name = raw[index].to_string_lossy().into_owned();
                    binary_for(&name)?;
                    options.protocols.push(name);
                }
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            other => return Err(format!("unknown argument {other}").into()),
        }
        index += 1;
    }
    if options.supports.is_empty() {
        return Err("--supports needs at least one value".into());
    }
    if options.protocols.is_empty() {
        return Err("--protocols needs at least one value".into());
    }
    Ok(options)
}

/// Only Shamir arms: the threshold t < n/3 this deployment uses is what lets a
/// linear reveal be robust without a broadcast channel, and swapping to a
/// dishonest-majority engine would change the release, not just its cost.
fn binary_for(protocol: &str) -> HarnessResult<&'static str> {
    match protocol {
        "malicious-shamir" => Ok("malicious-shamir-party.x"),
        "semi-honest-shamir" => Ok("shamir-party.x"),
        other => Err(format!(
            "--protocol must be malicious-shamir or semi-honest-shamir, not {other}"
        )
        .into()),
    }
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
