//! Smoke comparison across maker count and requested order size.

use qomm_harness::{next_value, parse_value, write_pretty_json, HarnessResult};
use qomm_sim::attackers::{passive_observer, pretrade_attributes};
use qomm_sim::disclosure::Disclosure;
use qomm_sim::engine::{run_arm, ArmOptions, ArmResult, ProbeResult};
use qomm_sim::experiment::build_probes;
use qomm_sim::fsum::{fsum, nsum};
use qomm_sim::market::{build_market_makers, ReferenceMarket, Request, SimConfig, SIZE_BUCKETS};
use qomm_sim::pyrandom::PyRandom;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

const REGIMES: [(&str, i64, i64, i64); 3] = [
    ("small", 1, 20, 10),
    ("medium", 21, 100, 50),
    ("large", 101, 400, 200),
];
const METRICS: [&str; 6] = [
    "fill_rate",
    "no_quote_rate",
    "user_cost_mean_ticks",
    "mm_pnl_per_fill",
    "unsettled_request_auc",
    "individual_inventory_correlation",
];

fn pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() < 4 || xs.len() != ys.len() {
        return None;
    }
    // statistics.fmean is math.fsum and one division.
    let xbar = fsum(xs.iter().copied()) / xs.len() as f64;
    let ybar = fsum(ys.iter().copied()) / ys.len() as f64;
    // These three are Python builtin sum(), which uses Neumaier
    // compensation and is deliberately distinct from fmean/fsum.
    let numerator = nsum(xs.iter().zip(ys).map(|(x, y)| (x - xbar) * (y - ybar)));
    let xden = nsum(xs.iter().map(|x| (x - xbar).powi(2)));
    let yden = nsum(ys.iter().map(|y| (y - ybar).powi(2)));
    if xden <= 0.0 || yden <= 0.0 {
        return None;
    }
    Some(numerator / (xden * yden).sqrt())
}

fn force_regime(requests: &[Request], low: i64, high: i64) -> Vec<Request> {
    let width = high - low + 1;
    requests
        .iter()
        .map(|request| Request {
            size: low + (request.size - 1).rem_euclid(width),
            ..*request
        })
        .collect()
}

fn individual_inventory_correlation(probes: &[ProbeResult]) -> Option<f64> {
    if probes.len() < 4 {
        return None;
    }
    let maker_ids = probes[0]
        .per_mm_inventory
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let mut correlations = Vec::new();
    for maker_id in maker_ids {
        let mut scores = Vec::new();
        let mut truth = Vec::new();
        for probe in probes {
            let score = if let Some((ask, bid)) = probe
                .per_mm_quotes
                .as_ref()
                .and_then(|quotes| quotes.get(&maker_id))
            {
                Some(0.5 * (*ask + *bid) as f64 - probe.ref_mid as f64)
            } else if let (Some(ask), Some(bid)) = (probe.best_ask, probe.best_bid) {
                Some(0.5 * (ask + bid) as f64 - probe.ref_mid as f64)
            } else {
                None
            };
            if let Some(score) = score {
                scores.push(score);
                truth.push(probe.per_mm_inventory[&maker_id] as f64);
            }
        }
        if let Some(value) = pearson(&scores, &truth) {
            correlations.push(value.abs());
        }
    }
    (!correlations.is_empty())
        .then(|| fsum(correlations.iter().copied()) / correlations.len() as f64)
}

fn fmean_optional(values: impl IntoIterator<Item = Option<f64>>) -> Option<f64> {
    let finite = values
        .into_iter()
        .flatten()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    (!finite.is_empty()).then(|| fsum(finite.iter().copied()) / finite.len() as f64)
}

/// Python calls SimConfig with only six keyword overrides. Start with the
/// library default and change exactly those fields; copying the other current
/// values here would silently diverge when the library default moves.
fn config(steps: usize, n_mm: usize, seed: u64) -> SimConfig {
    let mut cfg = SimConfig::default();
    cfg.steps = steps;
    cfg.n_mm = n_mm;
    cfg.n_entities = 24;
    cfg.arrival_rate = 0.15;
    cfg.window_steps = 200.max(steps / 8);
    cfg.seed = seed;
    cfg
}

/// `qomm_sim.market.build_requests` ported at the call boundary so the current
/// CPython builtin-sum semantics are explicit. All other market defaults and
/// algorithms remain owned by qomm-sim.
fn build_requests_exact(cfg: &SimConfig, market: &ReferenceMarket, seed: u64) -> Vec<Request> {
    let mut rng = PyRandom::new(seed);
    let raw = (0..cfg.n_entities)
        .map(|_| rng.paretovariate(1.6))
        .collect::<Vec<_>>();
    let total = nsum(raw.iter().copied());
    let weights = raw.iter().map(|weight| weight / total).collect::<Vec<_>>();
    let mut requests = Vec::new();
    for step in 0..cfg.steps {
        if rng.random() >= cfg.arrival_rate {
            continue;
        }
        let draw = rng.random();
        let mut running = 0.0;
        let entity = weights
            .iter()
            .enumerate()
            .find_map(|(index, weight)| {
                running += weight;
                (draw <= running).then_some(index)
            })
            .unwrap_or(weights.len() - 1);
        let wallet = entity * cfg.wallets_per_entity
            + rng.randrange(0, cfg.wallets_per_entity as i64) as usize;
        let bucket = rng.choices(&[0.55, 0.33, 0.12]);
        let (low, high) = SIZE_BUCKETS[bucket];
        let size = rng.randint(low, high);
        let informed = rng.random() < market.phi[step];
        let (direction, signal) = if informed {
            let future = market.move_over(step, 20);
            (u8::from(future <= 0), future)
        } else {
            (rng.randrange(0, 2) as u8, 0)
        };
        requests.push(Request {
            step,
            entity,
            wallet,
            size,
            direction,
            informed,
            signal,
        });
    }
    requests
}

fn summary(result: &ArmResult) -> Map<String, Value> {
    let pnl_total = nsum(result.mm_pnl.values().copied());
    Map::from_iter([
        ("requests".into(), json!(result.requests)),
        ("fills".into(), json!(result.fills)),
        ("fill_rate".into(), json!(result.fill_rate())),
        (
            "no_quote_rate".into(),
            json!(if result.requests == 0 {
                0.0
            } else {
                result.no_quote as f64 / result.requests as f64
            }),
        ),
        (
            "user_cost_mean_ticks".into(),
            json!((!result.user_cost_ticks.is_empty()).then(|| {
                nsum(result.user_cost_ticks.iter().copied()) / result.user_cost_ticks.len() as f64
            })),
        ),
        (
            "mm_pnl_per_fill".into(),
            json!((result.fills > 0).then(|| pnl_total / result.fills as f64)),
        ),
        (
            "quote_continuation".into(),
            json!(result.quote_continuation),
        ),
    ])
}

fn row_metric(row: &Value, name: &str) -> Option<f64> {
    row.get(name).and_then(Value::as_f64)
}

fn run_experiment(seeds: usize, steps: usize) -> Value {
    let mut rows = Vec::new();
    for seed_offset in 0..seeds {
        let seed = 202_608_240 + seed_offset as u64;
        for n_mm in [4, 8, 16] {
            let cfg = config(steps, n_mm, seed);
            let market = ReferenceMarket::new(&cfg, seed);
            let makers = build_market_makers(&cfg, seed + 1);
            let base_requests = build_requests_exact(&cfg, &market, seed + 2);
            for (regime, low, high, probe_size) in REGIMES {
                let requests = force_regime(&base_requests, low, high);
                let probes = build_probes(&cfg, 6, probe_size);
                for protocol in ["plain_rfq", "qomm_rfq"] {
                    let mut disclosure = Disclosure::None;
                    let mut options = ArmOptions::new(protocol, seed + 5);
                    options.probes = probes.clone();
                    options.reactive = false;
                    let result =
                        run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
                    let passive = passive_observer(&result, &cfg, 0.5, seed + 7);
                    let attributes = pretrade_attributes(&result, &cfg);
                    let summary = summary(&result);
                    rows.push(json!({
                        "seed": seed,
                        "n_mm": n_mm,
                        "order_regime": regime,
                        "order_range": [low, high],
                        "protocol": protocol,
                        "requests": summary["requests"],
                        "fills": summary["fills"],
                        "fill_rate": summary["fill_rate"],
                        "no_quote_rate": summary["no_quote_rate"],
                        "user_cost_mean_ticks": summary["user_cost_mean_ticks"],
                        "mm_pnl_per_fill": summary["mm_pnl_per_fill"],
                        "quote_continuation": summary["quote_continuation"],
                        "unsettled_request_auc": passive.auc,
                        "unsettled_request_tpr_at_5pct_fpr": passive.tpr_at_5pct_fpr,
                        "direction_accuracy": attributes.extra["direction_accuracy"],
                        "direction_prior": attributes.extra["direction_prior"],
                        "size_bucket_accuracy": attributes.extra["size_bucket_accuracy"],
                        "size_bucket_prior": attributes.extra["size_bucket_prior"],
                        "individual_inventory_correlation": individual_inventory_correlation(&result.probe_results),
                        "probe_count": result.probe_results.len(),
                    }));
                }
            }
        }
    }

    let mut cells = Vec::new();
    for n_mm in [4_u64, 8, 16] {
        for (regime, _, _, _) in REGIMES {
            let cell = rows
                .iter()
                .filter(|row| {
                    row["n_mm"].as_u64() == Some(n_mm)
                        && row["order_regime"].as_str() == Some(regime)
                })
                .collect::<Vec<_>>();
            let plain = cell
                .iter()
                .copied()
                .filter(|row| row["protocol"] == "plain_rfq")
                .collect::<Vec<_>>();
            let qomm = cell
                .iter()
                .copied()
                .filter(|row| row["protocol"] == "qomm_rfq")
                .collect::<Vec<_>>();
            let metric_object = |items: &[&Value]| {
                Value::Object(Map::from_iter(METRICS.map(|metric| {
                    (
                        metric.into(),
                        json!(fmean_optional(
                            items.iter().map(|row| row_metric(row, metric))
                        )),
                    )
                })))
            };
            cells.push(json!({
                "n_mm": n_mm,
                "order_regime": regime,
                "paired_seeds": seeds,
                "plain": metric_object(&plain),
                "qomm": metric_object(&qomm),
                "paired_fill_rate_delta": fmean_optional(plain.iter().zip(&qomm).map(|(plain, qomm)| {
                    Some(row_metric(qomm, "fill_rate")? - row_metric(plain, "fill_rate")?)
                })),
            }));
        }
    }

    let plain_corr = fmean_optional(
        rows.iter()
            .filter(|row| row["protocol"] == "plain_rfq")
            .map(|row| row_metric(row, "individual_inventory_correlation")),
    );
    let qomm_corr = fmean_optional(
        rows.iter()
            .filter(|row| row["protocol"] == "qomm_rfq")
            .map(|row| row_metric(row, "individual_inventory_correlation")),
    );
    let reduction = match (plain_corr, qomm_corr) {
        (Some(plain), Some(qomm)) if plain != 0.0 => Some(1.0 - qomm / plain),
        _ => None,
    };
    json!({
        "host": qomm_measure::hosts::this_host(),
        "evidence_class": "smoke_only",
        "synthetic": true,
        "config": {
            "seeds": seeds,
            "steps": steps,
            "maker_counts": [4, 8, 16],
            "order_regimes": {
                "small": [1, 20],
                "medium": [21, 100],
                "large": [101, 400],
            },
        },
        "sealed_prediction": {
            "metric": "individual_inventory_correlation",
            "prediction": "QOMM is at least 30 percent lower than plain RFQ",
            "likely_failure": "large orders in four-maker markets lose utility first",
        },
        "prediction_readout": {
            "plain_mean": plain_corr,
            "qomm_mean": qomm_corr,
            "relative_reduction": reduction,
            "prediction_met": reduction.is_some_and(|value| value >= 0.30),
        },
        "cells": cells,
        "rows": rows,
        "limitations": [
            "Synthetic smoke run; no production promotion is permitted.",
            "Behavioral responses and disclosure are disabled to isolate request routing.",
            "Three paired seeds are not a powered confirmation cohort.",
        ],
    })
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("run_market_thickness: {error}");
        std::process::exit(1);
    }
}

fn execute() -> HarnessResult<()> {
    let mut out = None::<PathBuf>;
    let mut seeds = 3_usize;
    let mut steps = 6_000_usize;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("--seeds") => {
                seeds = parse_value(next_value(&mut args, "--seeds")?, "--seeds")?;
            }
            Some("--steps") => {
                steps = parse_value(next_value(&mut args, "--steps")?, "--steps")?;
            }
            Some("-h" | "--help") => {
                println!(
                    "usage: run_market_thickness [-h] --out OUT [--seeds SEEDS] [--steps STEPS]"
                );
                return Ok(());
            }
            Some(value) => return Err(format!("unrecognized argument: {value}").into()),
            None => return Err("argument is not valid UTF-8".into()),
        }
    }
    if seeds < 1 || steps < 1_000 {
        return Err("at least one seed and 1,000 steps are required".into());
    }
    let out = out.ok_or("the following arguments are required: --out")?;
    let payload = run_experiment(seeds, steps);
    write_pretty_json(Some(&out), &payload)?;
    let readout = &payload["prediction_readout"];
    println!(
        "{{\"plain_mean\": {}, \"prediction_met\": {}, \"qomm_mean\": {}, \"relative_reduction\": {}}}",
        readout["plain_mean"],
        readout["prediction_met"],
        readout["qomm_mean"],
        readout["relative_reduction"],
    );
    Ok(())
}
