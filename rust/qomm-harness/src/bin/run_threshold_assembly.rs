use curve25519_dalek::scalar::Scalar;
use qomm_harness::{median, parse_value, repo_root, sample_sd, write_pretty_json, HarnessResult};
use qomm_proofs::quote_proof::{MakerWitness, QuoteCircuit, Registered};
use qomm_proofs::threshold_quote::{deal_quote_shares, joint_prove_quote};
use qomm_proofs::threshold_range::{
    deal_bits, joint_prove_range_from_contributions, verify_threshold_range,
};
use qomm_zk::bitrange::{prove_range, verify_range};
use qomm_zk::pedersen::Pedersen;
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

struct Options {
    out: PathBuf,
    widths: Vec<usize>,
    parties: usize,
    threshold: usize,
    repeats: usize,
    makers: Vec<usize>,
    group: String,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    validate(&options)?;
    let key = Pedersen::new(b"qomm:quote:v1");
    let parties = (1..=options.parties).collect::<Vec<_>>();
    let quorum = parties[..=options.threshold].to_vec();
    let mut rng = OsRng;

    let mut rows = Vec::new();
    for &width in &options.widths {
        let value = if width < 40 {
            (1u64 << width) - 1
        } else {
            12_345
        };
        let blinding = Scalar::random(&mut rng);
        let shares = deal_bits(
            &key,
            value,
            &blinding,
            width,
            &parties,
            options.threshold,
            &mut rng,
        )?;
        let contributions = quorum
            .iter()
            .map(|party| {
                shares
                    .node_contribution(*party)
                    .ok_or_else(|| format!("missing range contribution from party {party}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (assembled, _) =
            joint_prove_range_from_contributions(&key, &contributions, &quorum, b"ctx", &mut rng)?;
        let local = prove_range(
            &key,
            &shares.commitment,
            value,
            &blinding,
            width,
            b"ctx",
            &mut rng,
        )?;
        if !verify_threshold_range(&key, &shares.commitment, &assembled, b"ctx") {
            return Err(format!("assembled {width}-bit range proof did not verify").into());
        }
        if !verify_range(&key, &shares.commitment, &local, b"ctx") {
            return Err(format!("local {width}-bit range proof did not verify").into());
        }

        let assemble = timed(options.repeats, || {
            joint_prove_range_from_contributions(&key, &contributions, &quorum, b"ctx", &mut rng)?;
            Ok(())
        })?;
        let verify_assembled = timed(options.repeats, || {
            std::hint::black_box(verify_threshold_range(
                &key,
                &shares.commitment,
                &assembled,
                b"ctx",
            ));
            Ok(())
        })?;
        let prove_local = timed(options.repeats, || {
            prove_range(
                &key,
                &shares.commitment,
                value,
                &blinding,
                width,
                b"ctx",
                &mut rng,
            )
            .map(|_| ())
            .map_err(Into::into)
        })?;
        let verify_local = timed(options.repeats, || {
            std::hint::black_box(verify_range(&key, &shares.commitment, &local, b"ctx"));
            Ok(())
        })?;
        let size_assembled = proof_size(width, false);
        let size_local = proof_size(width, true);
        let assemble_median = assemble["median_ms"].as_f64().unwrap();
        let local_median = prove_local["median_ms"].as_f64().unwrap();
        let assemble_per_node_ms = assemble_median / quorum.len() as f64;
        let assemble_over_local = assemble_median / local_median;
        let per_node_over_local = assemble_per_node_ms / local_median;
        let bytes_over_local =
            size_assembled["bytes"].as_f64().unwrap() / size_local["bytes"].as_f64().unwrap();
        let no_node_holds_the_value = shares
            .value
            .values()
            .all(|share| *share != Scalar::from(value));
        rows.push(json!({
            "width": width,
            "quorum": quorum.len(),
            "assemble": assemble,
            "verify_assembled": verify_assembled,
            "prove_local": prove_local,
            "verify_local": verify_local,
            "size_assembled": size_assembled,
            "size_local": size_local,
            "no_node_holds_the_value": no_node_holds_the_value,
            "verified_by_ordinary_verifier": true,
            "assemble_over_local": assemble_over_local,
            "assemble_per_node_ms": assemble_per_node_ms,
            "per_node_over_local": per_node_over_local,
            "bytes_over_local": bytes_over_local,
        }));
        println!(
            "width {width:3}: assemble {assemble_median:7.1} ms vs local {local_median:7.1} ms ({assemble_over_local:.2}x total, {per_node_over_local:.2}x per node)   {:5} B vs {:5} B ({bytes_over_local:.3}x)",
            size_assembled["bytes"], size_local["bytes"]
        );
    }

    let mut quote_rows = Vec::new();
    for &maker_count in &options.makers {
        let makers = (0..maker_count)
            .map(|index| MakerWitness {
                ask_level: 10_000,
                spread: 40 + 2 * index as i64,
                slope: 1 + index as i64,
                invcoef: 1,
                inv: 3,
                maxqty: 500,
                expiry: 2_000,
                active: true,
                blindings: Registered::fresh(&mut rng),
            })
            .collect::<Vec<_>>();
        let sentinel = 1 << 20;
        // A slot for every maker, and the same slot count for every row so the
        // said, and the Makefile asks for 16: the ranking key is
        // `(gated + sentinel) * n_slots + index`, so at sixteen makers in eight
        // slots maker 8 at cost c and maker 0 at cost c+1 derive the *same* key
        // commitment and the winner is no longer determined. See
        // `qomm-proofs/tests/slot_collision.rs`.
        let n_slots = slot_count(&options.makers);
        let span_bits = bit_length((sentinel * n_slots * 2) as u64);
        let circuit = QuoteCircuit::new(24, span_bits);
        let (shares, public) = deal_quote_shares(
            &circuit,
            &makers,
            100,
            0,
            1_000,
            sentinel,
            n_slots,
            &parties,
            options.threshold,
            [0; 32],
            0,
            &mut rng,
        )?;
        let (assembled, _) = joint_prove_quote(&circuit, &shares, &public, &quorum, b"", &mut rng)?;
        circuit
            .verify(&assembled, &public, b"")
            .map_err(|why| format!("assembled quote did not verify: {why:?}"))?;
        let (local, local_public) = circuit.prove(
            &makers, 100, 0, 1_000, sentinel, n_slots, b"", &mut rng, [0; 32], 0,
        )?;
        circuit
            .verify(&local, &local_public, b"")
            .map_err(|why| format!("local quote did not verify: {why:?}"))?;

        let assemble = timed(options.repeats, || {
            joint_prove_quote(&circuit, &shares, &public, &quorum, b"", &mut rng)?;
            Ok(())
        })?;
        let prove_local = timed(options.repeats, || {
            circuit
                .prove(
                    &makers, 100, 0, 1_000, sentinel, n_slots, b"", &mut rng, [0; 32], 0,
                )
                .map(|_| ())
                .map_err(Into::into)
        })?;
        let verify_assembled = timed(options.repeats, || {
            let _ = std::hint::black_box(circuit.verify(&assembled, &public, b""));
            Ok(())
        })?;
        let verify_local = timed(options.repeats, || {
            let _ = std::hint::black_box(circuit.verify(&local, &local_public, b""));
            Ok(())
        })?;
        let assemble_per_node_ms = assemble["median_ms"].as_f64().unwrap() / quorum.len() as f64;
        let per_node_over_local = assemble_per_node_ms / prove_local["median_ms"].as_f64().unwrap();
        let verify_over_local = verify_assembled["median_ms"].as_f64().unwrap()
            / verify_local["median_ms"].as_f64().unwrap();
        println!(
            "quote, {maker_count:2} makers: per node {assemble_per_node_ms:7.1} ms vs local {:7.1} ms ({per_node_over_local:.2}x)   verify {:7.1} vs {:7.1} ms ({verify_over_local:.2}x)",
            prove_local["median_ms"].as_f64().unwrap(),
            verify_assembled["median_ms"].as_f64().unwrap(),
            verify_local["median_ms"].as_f64().unwrap(),
        );
        quote_rows.push(json!({
            "makers": maker_count,
            "assemble": assemble,
            "prove_local": prove_local,
            "verify_assembled": verify_assembled,
            "verify_local": verify_local,
            "assemble_per_node_ms": assemble_per_node_ms,
            "per_node_over_local": per_node_over_local,
            "verify_over_local": verify_over_local,
        }));
    }

    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "group": options.group,
        "quote_rows": quote_rows,
        "assemble_is": "total CPU over recipient-scoped node contributions, run serially in one process; a deployment runs the members at once, so assemble_per_node_ms is what a node waits for",
        "parties": options.parties,
        "threshold": options.threshold,
        // Recorded because it is a soundness parameter, not a tuning one: the
        // ranking key packs the maker index into `n_slots`, so an artifact
        // whose maker count exceeds it has measured a run in which two makers
        // at two costs share one key. See `qomm-proofs/tests/slot_collision.rs`.
        "n_slots": slot_count(&options.makers),
        "bit_proof_assembled": "square: b*b = b, linear in the witness",
        "bit_proof_local": "disjunction: the branch is chosen from the bit",
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn proof_size(width: usize, local: bool) -> Value {
    let points = 3 * width + 1;
    // Preserve the source artifact's explicit two-branch accounting. The Rust
    // `BitProof` derives c1 from c0, but this benchmark compares constructions.
    let scalars = (if local { 4 } else { 3 }) * width + 2;
    json!({"points": points, "scalars": scalars, "bytes": (points + scalars) * 32})
}

fn timed<F>(repeats: usize, mut operation: F) -> HarnessResult<Value>
where
    F: FnMut() -> HarnessResult<()>,
{
    let mut samples = Vec::with_capacity(repeats);
    for _ in 0..repeats {
        let started = Instant::now();
        operation()?;
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    Ok(json!({
        "median_ms": median(&samples),
        "n": samples.len(),
        "min_ms": samples.iter().copied().min_by(f64::total_cmp),
        "max_ms": samples.iter().copied().max_by(f64::total_cmp),
        "sd_ms": sample_sd(&samples).unwrap_or(0.0),
    }))
}

fn bit_length(value: u64) -> usize {
    (u64::BITS - value.leading_zeros()) as usize
}

fn validate(options: &Options) -> HarnessResult<()> {
    if options.group != "ed25519" {
        return Err("the Rust proof crates implement the ed25519 scalar field as ristretto255; --group must be ed25519".into());
    }
    if options.repeats == 0 {
        return Err("--repeats must be positive".into());
    }
    if options.parties <= options.threshold {
        return Err("--parties must be greater than --threshold".into());
    }
    if options.widths.iter().any(|width| !(1..=64).contains(width)) {
        return Err("--widths must be between 1 and 64".into());
    }
    if options.makers.contains(&0) {
        return Err("--makers must be at least 1".into());
    }
    Ok(())
}

/// One slot per maker, floored at the eight the earlier runs used so a small
/// row is not silently measured on a narrower span than it used to be.
fn slot_count(makers: &[usize]) -> i64 {
    makers.iter().copied().max().unwrap_or(8).max(8) as i64
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: repo_root().join("artifacts/threshold_assembly.json"),
        widths: vec![8, 16, 24, 26, 32],
        parties: 7,
        threshold: 2,
        repeats: 5,
        makers: vec![2, 4, 8],
        group: "ed25519".into(),
    };
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        match flag.as_ref() {
            "--out" => options.out = PathBuf::from(value(&args, &mut index, "--out")?),
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
            "--group" => {
                options.group = value(&args, &mut index, "--group")?
                    .into_string()
                    .map_err(|_| "--group is not UTF-8")?
            }
            "--widths" => options.widths = list(&args, &mut index, "--widths")?,
            "--makers" => options.makers = list(&args, &mut index, "--makers")?,
            "-h" | "--help" => {
                println!("usage: run_threshold_assembly [--out PATH] [--widths N...] [--parties N] [--threshold N] [--repeats N] [--makers N...] [--group ed25519]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {flag}").into()),
        }
        index += 1;
    }
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
    let mut result = Vec::new();
    while *index + 1 < args.len() && !args[*index + 1].to_string_lossy().starts_with("--") {
        *index += 1;
        result.push(parse_value(args[*index].clone(), flag)?);
    }
    if result.is_empty() {
        return Err(format!("argument {flag} expects at least one value").into());
    }
    Ok(result)
}

#[cfg(test)]
mod source_boundary_tests {
    const THRESHOLD_ASSEMBLY: &str = include_str!("run_threshold_assembly.rs");
    const CIRCUIT_BOUND_PROOF: &str = include_str!("run_circuit_bound_proof.rs");

    #[test]
    fn paper_artifact_provers_only_call_node_scoped_assembly_apis() {
        let binaries = [
            ("run_threshold_assembly", THRESHOLD_ASSEMBLY),
            ("run_circuit_bound_proof", CIRCUIT_BOUND_PROOF),
        ];
        let allowed = [
            "joint_prove_quote",
            "joint_prove_range_from_contributions",
            "joint_prove_product_from_contributions",
        ];
        let mut violations = Vec::new();

        for (binary, source) in binaries {
            let source = source.split("#[cfg(test)]").next().unwrap_or(source);
            for (index, line) in source.lines().enumerate() {
                if line.contains("joint_prove_")
                    && !allowed.iter().any(|entry| line.contains(entry))
                {
                    violations.push(format!("{binary}:{}: {}", index + 1, line.trim()));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "paper artifact path crossed a whole-party proving API:\n{}",
            violations.join("\n")
        );
        assert!(
            THRESHOLD_ASSEMBLY.contains("joint_prove_range_from_contributions"),
            "threshold assembly no longer proves ranges from node contributions"
        );
        assert!(
            CIRCUIT_BOUND_PROOF.contains("joint_prove_product_from_contributions"),
            "circuit-bound proof no longer proves products from node contributions"
        );
    }
}
