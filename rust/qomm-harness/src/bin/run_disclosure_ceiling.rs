//! Rust port of `scripts/run_disclosure_ceiling.py`.

use qomm_harness::smallsample::{fsum, mean_ci};
use qomm_harness::{write_pretty_json, HarnessResult};
use qomm_sim::disclosure::Disclosure;
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::experiment::{make_disclosure, DpParams};
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const FIELDS: [&str; 6] = [
    "fill_rate",
    "mm_pnl_per_fill",
    "mm_pnl_total",
    "user_cost_ticks",
    "mm0_pnl",
    "others_pnl",
];

struct Options {
    out: PathBuf,
    seeds: usize,
    seed0: u64,
    steps: usize,
    window_steps: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let seeds: Vec<u64> = (0..options.seeds)
        .map(|index| options.seed0 + 7 * index as u64)
        .collect();
    let mut paired: BTreeMap<&str, Vec<f64>> = FIELDS
        .into_iter()
        .map(|field| (field, Vec::new()))
        .collect();
    let mut paired_one = paired.clone();
    let mut rows = Vec::new();

    for seed in seeds {
        let none = one(seed, "A_none", options.steps, options.window_steps)?;
        let oracle = one(seed, "oracle", options.steps, options.window_steps)?;
        let single = one(seed, "oracle_one", options.steps, options.window_steps)?;
        for field in FIELDS {
            paired
                .get_mut(field)
                .unwrap()
                .push(number(&oracle, field)? - number(&none, field)?);
            paired_one
                .get_mut(field)
                .unwrap()
                .push(number(&single, field)? - number(&none, field)?);
        }
        println!(
            "seed {seed}: pnl/fill {:8.1} -> {:8.1}   user {:6.1} -> {:6.1}",
            number(&none, "mm_pnl_per_fill")?,
            number(&oracle, "mm_pnl_per_fill")?,
            number(&none, "user_cost_ticks")?,
            number(&oracle, "user_cost_ticks")?,
        );
        rows.push(json!({
            "seed": seed,
            "none": none,
            "oracle": oracle,
            "oracle_one": single,
        }));
    }

    let summary = intervals(&paired);
    let summary_one = intervals(&paired_one);
    let phi_values: Vec<f64> = rows
        .iter()
        .filter_map(|row| row["none"]["true_phi"].as_f64())
        .collect();
    let true_phi_mean = if phi_values.is_empty() {
        0.0
    } else {
        fsum(phi_values.iter().copied()) / phi_values.len() as f64
    };
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "question": "what the best possible disclosure of the informed fraction is worth to a market maker",
        "arm": "the true phi, exact and free, which no mechanism beats",
        "seeds": options.seeds,
        "steps": options.steps,
        "true_phi_mean": true_phi_mean,
        "everyone_informed_minus_none": summary,
        "one_maker_informed_minus_none": summary_one,
        "what_this_measures": "naive substitution of the market-wide figure for the maker's own conditional estimate, which is what BeliefState.combined prescribes; not the value of the information to a maker free to use it as it likes",
        "reading": "a maker's own estimate is formed on the flow it won, which is more informed than the market's average; the market-wide figure makes it quote for average flow and win worse flow",
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn one(seed: u64, arm: &str, steps: usize, window_steps: usize) -> HarnessResult<Value> {
    let cfg = SimConfig {
        steps,
        window_steps,
        seed,
        ..SimConfig::default()
    };
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let requests = build_requests(&cfg, &market, cfg.seed + 2);
    let mut by_window: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for request in &requests {
        let entry = by_window
            .entry(request.step / cfg.window_steps)
            .or_default();
        entry.0 += usize::from(request.informed);
        entry.1 += 1;
    }
    let phi: BTreeMap<usize, f64> = by_window
        .into_iter()
        .filter_map(|(window, (informed, count))| {
            (count > 0).then_some((window, informed as f64 / count as f64))
        })
        .collect();
    let mut disclosure = match arm {
        "oracle" => Disclosure::Oracle {
            phi_by_window: phi.clone(),
            reaches: None,
        },
        "oracle_one" => Disclosure::Oracle {
            phi_by_window: phi.clone(),
            reaches: Some(BTreeSet::from([0])),
        },
        name => make_disclosure(name, &cfg, &DpParams::default()),
    };
    let options = ArmOptions::new("qomm_rfq", cfg.seed + 5);
    let result = run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
    let total = result.mm_pnl_total();
    let true_phi = if phi.is_empty() {
        0.0
    } else {
        fsum(phi.values().copied()) / phi.len() as f64
    };
    Ok(json!({
        "mm0_pnl": result.mm_pnl.get(&0).copied().unwrap_or(0.0),
        "others_pnl": qomm_sim::fsum::nsum(
            result.mm_pnl.iter().filter(|(maker, _)| **maker != 0).map(|(_, value)| *value),
        ),
        "fill_rate": result.fills as f64 / result.requests.max(1) as f64,
        "mm_pnl_per_fill": total / result.fills.max(1) as f64,
        "mm_pnl_total": total,
        "user_cost_ticks": if result.user_cost_ticks.is_empty() { 0.0 } else { fsum(result.user_cost_ticks.iter().copied()) / result.user_cost_ticks.len() as f64 },
        "true_phi": true_phi,
    }))
}

fn number(value: &Value, field: &str) -> HarnessResult<f64> {
    value
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("missing numerical field {field}").into())
}

fn intervals(values: &BTreeMap<&str, Vec<f64>>) -> Value {
    Value::Object(
        FIELDS
            .into_iter()
            .map(|field| (field.to_string(), mean_ci(&values[field], 0.05)))
            .collect::<Map<_, _>>(),
    )
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: qomm_harness::repo_root().join("artifacts/disclosure_ceiling.json"),
        seeds: 12,
        seed0: 11,
        steps: 48_000,
        window_steps: 1_200,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        index += 1;
        let raw = args
            .get(index)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--out" => options.out = raw.into(),
            "--seeds" => options.seeds = raw.parse()?,
            "--seed0" => options.seed0 = raw.parse()?,
            "--steps" => options.steps = raw.parse()?,
            "--window-steps" => options.window_steps = raw.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
        index += 1;
    }
    if options.seeds == 0 {
        return Err("--seeds must be positive".into());
    }
    Ok(options)
}
