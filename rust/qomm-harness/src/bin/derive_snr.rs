use qomm_harness::{median, repo_root, write_pretty_json, HarnessResult};
use qomm_sim::deterministic_random::DeterministicRng;
use qomm_sim::experiment::DpParams;
use qomm_sim::market::{SimConfig, SIZE_BUCKETS};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

const MEDIAN_OVER_SIGMA: f64 = 0.6745;

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let out = parse_args()?;
    let cfg = SimConfig::default();
    let dp = DpParams::default();
    let cap = dp.volume_cap as f64;
    let epsilon_per_field = dp.epsilon_per_window / 4.0;
    let noise = cap / epsilon_per_field;

    let weights = [0.55, 0.33, 0.12];
    let mean_size = qomm_sim::fsum::nsum(
        weights
            .iter()
            .zip(SIZE_BUCKETS)
            .map(|(weight, (low, high))| weight * (low + high) as f64 / 2.0),
    );
    let trades_per_firm_per_s =
        cfg.arrival_rate * (1_000.0 / cfg.step_ms as f64) / cfg.n_entities as f64;
    let clip_saturation_s = (cap / mean_size).powi(2) / trades_per_firm_per_s;
    let ceiling = |firms: f64| MEDIAN_OVER_SIGMA * epsilon_per_field * firms.sqrt();

    let firm_counts = [
        (
            "generated market, entities configured",
            cfg.n_entities as f64,
        ),
        (
            "UniswapX: requests per window in the run (4485/2400)",
            4_485.0 / 2_400.0,
        ),
        (
            "UniswapX: distinct swappers per 150-block window, raw tape median",
            11.0,
        ),
    ];

    println!(
        "noise scale        = cap/eps_field = {}/{epsilon_per_field} = {noise:.0}",
        dp.volume_cap
    );
    println!(
        "clip saturates at  T* = {clip_saturation_s:.0} s ({:.1} min), where the standard deviation reaches the cap --- about 32% of firms clip there, not all of them",
        clip_saturation_s / 60.0
    );
    println!("signal-to-noise ceiling (median basis) = 0.6745 * eps_field * sqrt(N),");
    println!("  where N is the firms contributing to ONE window:");
    for (label, firms) in firm_counts {
        println!("  N = {firms:8.2}  {label:60}  {:6.3}", ceiling(firms));
    }
    let firms_for_snr_1 = (1.0 / (MEDIAN_OVER_SIGMA * epsilon_per_field)).powi(2);
    println!("  SNR = 1 needs N = {firms_for_snr_1:.1} firms in the same window");
    println!(
        "  the whole-tape count 2526 would give {:.2}, which is what this reported before; it aggregates 2,400 windows into one.",
        ceiling(2_526.0)
    );

    println!();
    println!(
        "cross-check 1: the disclosure runs used a noise scale of 1200; derived here, {noise:.0}"
    );
    if (noise - 1_200.0).abs() > 1.0 {
        return Err("derived noise scale does not match the measured one".into());
    }

    let mut rng = DeterministicRng::new(7);
    let saturated = drawn_median(&mut rng, cfg.n_entities, 10.0 * cap, cap, 20_000);
    let closed_form = MEDIAN_OVER_SIGMA * cap * (cfg.n_entities as f64).sqrt();
    println!(
        "cross-check 2: saturated median imbalance drawn {saturated:.0} against closed form {closed_form:.0}"
    );
    if (saturated - closed_form).abs() > 0.1 * closed_form {
        return Err("the closed form does not reproduce the drawn sums".into());
    }

    let at_60s = mean_size * (trades_per_firm_per_s * 60.0).sqrt();
    let predicted = drawn_median(&mut rng, cfg.n_entities, at_60s, cap, 20_000);
    println!(
        "cross-check 3: at a 60 s window this model predicts a median imbalance of {predicted:.0}; the disclosure runs measured 428"
    );
    if !(0.6 * 428.0 < predicted && predicted < 1.6 * 428.0) {
        return Err("the model does not reproduce the measured imbalance".into());
    }

    println!();
    println!("a mean instead of a sum: noise = k*R/(eps_field*n)");
    let requests_per_window = cfg.arrival_rate * cfg.window_steps as f64;
    let mean_fields = [
        ("winning half-spread (ticks)", 60.0 - 4.0),
        (
            "trade size (lots)",
            SIZE_BUCKETS[SIZE_BUCKETS.len() - 1].1 as f64,
        ),
        ("per-fill markout (ticks)", 2.0 * cfg.informed_edge_ticks),
    ];
    let mut mean_field_noise = Map::new();
    for (field, span) in mean_fields {
        let mut row = Map::new();
        for (label, multiplier) in [("1 min", 1.0), ("10 min", 10.0), ("1 hour", 60.0)] {
            let observations = requests_per_window * multiplier;
            row.insert(
                label.to_string(),
                json!(dp.request_cap as f64 * span / (epsilon_per_field * observations)),
            );
        }
        println!(
            "  {field:28} 1 min {:7.3}  10 min {:7.3}  1 hour {:7.3}",
            row["1 min"].as_f64().unwrap(),
            row["10 min"].as_f64().unwrap(),
            row["1 hour"].as_f64().unwrap()
        );
        mean_field_noise.insert(field.to_string(), Value::Object(row));
    }
    println!(
        "  for scale: half-spreads run 6-18 ticks, break-even at {:.1}, mean size {:.1} lots",
        cfg.informed_base * cfg.informed_edge_ticks,
        mean_size
    );

    let firm_counts_json = [
        (
            "generated market, entities configured".to_string(),
            json!(cfg.n_entities),
        ),
        (
            "UniswapX: requests per window in the run (4485/2400)".to_string(),
            json!(4_485.0 / 2_400.0),
        ),
        (
            "UniswapX: distinct swappers per 150-block window, raw tape median".to_string(),
            json!(11),
        ),
    ]
    .into_iter()
    .collect::<Map<_, _>>();
    let ceilings = firm_counts
        .into_iter()
        .map(|(label, count)| (label.to_string(), json!(ceiling(count))))
        .collect::<Map<_, _>>();
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "volume_cap": dp.volume_cap,
        "epsilon_per_window": dp.epsilon_per_window,
        "epsilon_per_field": epsilon_per_field,
        "noise_scale": noise,
        "mean_size_lots": mean_size,
        "trades_per_firm_per_s": trades_per_firm_per_s,
        "clip_saturation_s": clip_saturation_s,
        "ceiling": ceilings,
        "firms_per_window_for_snr_1": firms_for_snr_1,
        "ceiling_if_whole_tape_pooled": ceiling(2_526.0),
        "firm_counts": firm_counts_json,
        "mean_field_noise": mean_field_noise,
    });
    write_pretty_json(Some(&out), &payload)?;
    println!(
        "wrote {}",
        out.file_name().unwrap_or_default().to_string_lossy()
    );
    Ok(())
}

fn drawn_median(
    rng: &mut DeterministicRng,
    firms: usize,
    per_firm: f64,
    cap: f64,
    trials: usize,
) -> f64 {
    let values = (0..trials)
        .map(|_| {
            qomm_sim::fsum::nsum((0..firms).map(|_| rng.gauss(0.0, per_firm).clamp(-cap, cap)))
                .abs()
        })
        .collect::<Vec<_>>();
    median(&values).expect("the caller asks for positive trials")
}

fn parse_args() -> HarnessResult<PathBuf> {
    let mut out = repo_root().join("artifacts/snr_model.json");
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--out") => {
                out = PathBuf::from(args.next().ok_or("argument --out expects one value")?)
            }
            Some("-h" | "--help") => {
                println!("usage: derive_snr [--out PATH]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", arg.to_string_lossy()).into()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snr_model_matches_the_mechanism_it_describes() {
        let dp = DpParams::default();
        let cfg = SimConfig::default();
        let epsilon_per_field = dp.epsilon_per_window / 4.0;
        assert_eq!(dp.volume_cap as f64 / epsilon_per_field, 1_200.0);

        let mean_size = qomm_sim::fsum::nsum(
            [0.55, 0.33, 0.12]
                .into_iter()
                .zip(SIZE_BUCKETS)
                .map(|(weight, (low, high))| weight * (low + high) as f64 / 2.0),
        );
        let per_firm_per_s =
            cfg.arrival_rate * (1_000.0 / cfg.step_ms as f64) / cfg.n_entities as f64;
        let saturation = (dp.volume_cap as f64 / mean_size).powi(2) / per_firm_per_s;
        assert!((225.0..240.0).contains(&saturation));

        let ceiling = MEDIAN_OVER_SIGMA * epsilon_per_field * (cfg.n_entities as f64).sqrt();
        assert!((0.80..0.86).contains(&ceiling));
        assert!(ceiling > 0.36);
    }
}
