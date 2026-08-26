//! Rust port of `scripts/run_probe_budget.py`.

use qomm_harness::{median, parse_value, write_pretty_json, HarnessResult};
use qomm_sim::attackers;
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::experiment::{build_probes, make_disclosure, DpParams};
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use serde_json::json;
use std::ffi::OsString;
use std::path::PathBuf;

const BUDGETS: [usize; 14] = [4, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96, 128, 192, 240];

struct Options {
    seeds: usize,
    probes_per_window: usize,
    out: PathBuf,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut by_budget = BUDGETS
        .iter()
        .map(|&budget| (budget, Vec::<f64>::new()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for seed in 0..options.seeds {
        for (budget, value) in one_seed(seed as u64, options.probes_per_window) {
            by_budget.entry(budget).or_default().push(value);
        }
    }

    println!(
        "{:>7} {:>11} {:>7} {:>7} {:>12} {:>6}",
        "probes", "median |r|", "p25", "p75", "significant", "seeds"
    );
    let mut rows = Vec::new();
    for budget in BUDGETS {
        let values = by_budget.get_mut(&budget).expect("all budgets initialized");
        values.sort_by(f64::total_cmp);
        if values.is_empty() {
            continue;
        }
        let middle = median(values).expect("non-empty checked");
        let low = values[values.len() / 4];
        let high = values[3 * values.len() / 4];
        let share = values
            .iter()
            .filter(|&&value| significant(value, budget))
            .count() as f64
            / values.len() as f64;
        println!(
            "{budget:>7} {middle:>11.3} {low:>7.3} {high:>7.3} {:>11.0}% {:>6}",
            100.0 * share,
            values.len()
        );
        rows.push(json!({
            "probes": budget,
            "median_abs_corr": round_places(middle, 4),
            "p25": round_places(low, 4),
            "p75": round_places(high, 4),
            "share_significant": round_places(share, 3),
            "seeds": values.len(),
        }));
    }

    let usable = rows
        .iter()
        .find(|row| row["share_significant"].as_f64().unwrap_or(0.0) >= 0.5)
        .and_then(|row| row["probes"].as_u64());
    println!(
        "\nfirst budget at which most seeds give a correlation distinguishable from zero: {}",
        usable.map_or_else(|| "none in the range".into(), |value| value.to_string())
    );
    let payload = json!({
        "what": "how many probes recover a maker's net inventory from its own two-sided quotes",
        "why": "DEPLOYMENT.md tells an operator to set the per-entity cap from this number and the number was not measured anywhere",
        "attack": "correlate the midpoint of a firm two-sided quote with the maker's net inventory; the half spread cancels",
        "verdict": "a correlation distinguishable from zero at 95% for most seeds",
        "first_usable_budget": usable,
        "seeds": options.seeds,
        "probes_per_window": options.probes_per_window,
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn significant(r: f64, n: usize) -> bool {
    if n < 4 || r.abs() >= 1.0 {
        return false;
    }
    let t_stat = r.abs() * ((n - 2) as f64).sqrt() / (1.0 - r * r).sqrt();
    let critical = 1.96 * (1.0 + 2.0 / (n - 2) as f64);
    t_stat > critical
}

fn one_seed(seed: u64, probes_per_window: usize) -> Vec<(usize, f64)> {
    let cfg = SimConfig {
        seed,
        ..SimConfig::default()
    };
    let market = ReferenceMarket::new(&cfg, seed);
    let makers = build_market_makers(&cfg, seed + 1);
    let requests = build_requests(&cfg, &market, seed + 2);
    let probes = build_probes(&cfg, probes_per_window, 50);
    let mut disclosure = make_disclosure("A_none", &cfg, &DpParams::default());
    let mut options = ArmOptions::new("qomm", seed + 5);
    options.probes = probes;
    options.reactive = false;
    let result = run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
    BUDGETS
        .iter()
        .filter_map(|&budget| {
            attackers::probing_entity(&result, budget)
                .extra
                .get("net_inventory_corr_from_best_quote")
                .copied()
                .flatten()
                .map(|value| (budget, value.abs()))
        })
        .collect()
}

fn round_places(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    qomm_sim::market::py_round(value * scale) as f64 / scale
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        seeds: 24,
        probes_per_window: 4,
        out: qomm_harness::repo_root().join("artifacts/probe_budget.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--seeds" => {
                options.seeds = parse_value(value(&raw, &mut index, "--seeds")?, "--seeds")?
            }
            "--probes-per-window" => {
                options.probes_per_window = parse_value(
                    value(&raw, &mut index, "--probes-per-window")?,
                    "--probes-per-window",
                )?
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
