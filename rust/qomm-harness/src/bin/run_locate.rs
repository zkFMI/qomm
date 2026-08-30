//! Measure the exact attribution boundary of QOMM's Shamir share decoder.

use curve25519_dalek::scalar::Scalar;
use qomm_audit::locate::{capacity, locate, points, reconstruct, share, Verdict};
use qomm_harness::{
    next_value, parse_value, rustc_version, timing_summary, write_pretty_json, HarnessResult,
};
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug)]
struct Options {
    trials: usize,
    timing_repeats: usize,
    out: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let degree_t = grid(2, options.trials)?;
    let degree_2t = grid(4, options.trials)?;
    validate_grid(
        &degree_t,
        &[options.trials, options.trials, options.trials, 0],
    )?;
    validate_grid(&degree_2t, &[options.trials, options.trials, 0, 0])?;
    let timings = measure(options.timing_repeats)?;
    let payload = json!({
        "schema": "qomm-share-attribution-v1",
        "host": qomm_measure::hosts::this_host(),
        "rustc": rustc_version(),
        "question": "Which malformed Shamir shares can be attributed without guessing?",
        "implementation": "rust/qomm-zk/src/shamir.rs, re-exported by rust/qomm-audit/src/locate.rs",
        "deployment": {
            "parties": 7,
            "threshold": 2,
            "degree_t_capacity": capacity(7, 2),
            "degree_2t_capacity": capacity(7, 4),
        },
        "grid": {
            "degree_t": {"degree": 2, "capacity": capacity(7, 2), "rows": degree_t},
            "degree_2t_product": {"degree": 4, "capacity": capacity(7, 4), "rows": degree_2t},
        },
        "timing_us": timings,
        "communication": {
            "ordinary_opening_shares": 5,
            "attribution_shares": 7,
            "extra_share_traffic_fraction": 0.4,
            "extra_rounds": 0,
        },
        "claim_boundary": [
            "At degree t, zero through two malformed shares are corrected and named exactly; three are refused.",
            "Before degree reduction at degree 2t, one malformed product share is attributable; two are refused.",
            "A valid sharing of a substituted input is not a malformed codeword and must be caught by transport binding and the circuit input check."
        ],
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn grid(degree: usize, trials: usize) -> HarnessResult<Vec<Value>> {
    let xs = points(7);
    let mut rng = OsRng;
    let mut rows = Vec::new();
    for wrong in 0..=3 {
        let mut recovered = 0usize;
        let mut named_exactly = 0usize;
        for trial in 0..trials {
            let secret = Scalar::from((trial + 1) as u64);
            let mut shares = share(&secret, degree, &xs, &mut rng);
            let culprits = culprits_for(trial, wrong);
            corrupt(&mut shares, &culprits);
            if let Verdict::Decoded {
                secret: decoded,
                culprits: named,
            } = locate(&xs, &shares, degree)
            {
                if decoded == secret {
                    recovered += 1;
                }
                if decoded == secret && named == culprits {
                    named_exactly += 1;
                }
            }
        }
        rows.push(json!({
            "wrong_shares": wrong,
            "recovered": recovered,
            "named_exactly": named_exactly,
            "trials": trials,
        }));
    }
    Ok(rows)
}

fn culprits_for(trial: usize, count: usize) -> Vec<usize> {
    let mut values = (0..count)
        .map(|offset| (trial + offset * 3) % 7)
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    while values.len() < count {
        let candidate = (trial + values.len() * 5 + 1) % 7;
        if !values.contains(&candidate) {
            values.push(candidate);
            values.sort_unstable();
        }
    }
    values
}

fn corrupt(shares: &mut [Scalar], culprits: &[usize]) {
    for (offset, culprit) in culprits.iter().enumerate() {
        shares[*culprit] += Scalar::from(10_000 + offset as u64);
    }
}

fn validate_grid(rows: &[Value], expected_recovered: &[usize]) -> HarnessResult<()> {
    for (row, expected) in rows.iter().zip(expected_recovered) {
        let recovered = row["recovered"]
            .as_u64()
            .ok_or("grid row has no recovered count")? as usize;
        let exact = row["named_exactly"]
            .as_u64()
            .ok_or("grid row has no exact attribution count")? as usize;
        if recovered != *expected || exact != *expected {
            return Err(format!(
                "decoder boundary changed at {} wrong shares: recovered={recovered}, exact={exact}, expected={expected}",
                row["wrong_shares"]
            )
            .into());
        }
    }
    Ok(())
}

fn measure(repeats: usize) -> HarnessResult<Value> {
    let xs = points(7);
    let mut rng = OsRng;
    let secret = Scalar::from(23_u64);
    let honest = share(&secret, 2, &xs, &mut rng);
    let mut one_bad = honest.clone();
    corrupt(&mut one_bad, &[3]);
    let mut two_bad = honest.clone();
    corrupt(&mut two_bad, &[1, 5]);

    let plain = timings(repeats, || {
        std::hint::black_box(reconstruct(&xs[..3], &honest[..3]));
    });
    let no_errors = timings(repeats, || {
        std::hint::black_box(locate(&xs, &honest, 2));
    });
    let one_error = timings(repeats, || {
        std::hint::black_box(locate(&xs, &one_bad, 2));
    });
    let two_errors = timings(repeats, || {
        std::hint::black_box(locate(&xs, &two_bad, 2));
    });
    Ok(json!({
        "plain_lagrange_3_of_7": timing_summary(&plain),
        "locate_no_errors": timing_summary(&no_errors),
        "locate_one_error": timing_summary(&one_error),
        "locate_two_errors": timing_summary(&two_errors),
    }))
}

fn timings(mut repeats: usize, mut operation: impl FnMut()) -> Vec<f64> {
    repeats = repeats.max(1);
    (0..repeats)
        .map(|_| {
            let started = Instant::now();
            operation();
            started.elapsed().as_secs_f64() * 1_000_000.0
        })
        .collect()
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        trials: 300,
        timing_repeats: 200,
        out: qomm_harness::repo_root().join("artifacts/locate.json"),
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--trials") => {
                options.trials = parse_value(next_value(&mut args, "--trials")?, "--trials")?
            }
            Some("--timing-repeats") => {
                options.timing_repeats = parse_value(
                    next_value(&mut args, "--timing-repeats")?,
                    "--timing-repeats",
                )?
            }
            Some("--out") => options.out = PathBuf::from(next_value(&mut args, "--out")?),
            Some("-h" | "--help") => {
                println!("usage: run_locate [--trials N] [--timing-repeats N] [--out PATH]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    if options.trials == 0 || options.timing_repeats == 0 {
        return Err("--trials and --timing-repeats must be positive".into());
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn culprit_sets_are_sorted_unique_and_have_requested_size() {
        for trial in 0..30 {
            for count in 0..=3 {
                let values = culprits_for(trial, count);
                assert_eq!(values.len(), count);
                assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
                assert!(values.iter().all(|value| *value < 7));
            }
        }
    }
}
