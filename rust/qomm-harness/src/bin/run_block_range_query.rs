use qomm_harness::smallsample::{fsum, population_sd};
use qomm_harness::{median, write_pretty_json, HarnessResult};
use qomm_sim::deterministic_random::DeterministicRng;
use qomm_sim::disclosure::{Disclosure, EntityAccountant, WindowObservation};
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::lab::LabMarket;
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use qomm_sim::queries::{
    answer_block_range_query, event_count_sensitivity, noise_scale, windows_in_range,
    BlockRangeQuery, SENSITIVITY,
};
use qomm_sim::tapes::{load_bybit, requests_from_tape, Entities, TapeMarket};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const SATURATION: f64 = 0.95;

struct Options {
    out: PathBuf,
    seeds: usize,
    epsilon: f64,
    draws: usize,
    tape: PathBuf,
    uniswapx: PathBuf,
    samples: usize,
}

struct Setup {
    cfg: SimConfig,
    market: LabMarket,
    makers: Vec<qomm_sim::market::MarketMaker>,
    requests: Vec<qomm_sim::market::Request>,
    meta: Value,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let generated = build_setup(None)?;
    let mut arms = vec![arm(
        "generated",
        &generated,
        options.seeds,
        options.epsilon,
        options.draws,
    )?];
    if options.tape.exists() {
        let tape = build_setup(Some(&options.tape))?;
        arms.push(arm(
            "tape",
            &tape,
            options.seeds,
            options.epsilon,
            options.draws,
        )?);
    }
    let real = if options.uniswapx.exists() {
        Some(real_identity_arm(
            &options.uniswapx,
            options.epsilon,
            &[100, 600, 3_000, 7_200, 50_000],
            options.samples,
            20_260_824,
        )?)
    } else {
        None
    };
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "question": "whether a block-range question about distinct requesting entities carries anything, and at what range width",
        "prediction": "artifacts/block_range_query_prediction.json",
        "saturation_definition": "width at which the true distinct count reaches 95% of the entities seen",
        "not_measured_on_purpose": "the fill count. Settlement publishes one on-chain instruction per trade, so that count is exact and free to anyone with a node; a noised version would protect nothing and be worse than the free answer.",
        "arms": arms,
        "real_identities": real,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    copy_prediction_next_to(&options.out)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn build_setup(tape_path: Option<&Path>) -> HarnessResult<Setup> {
    let cfg = SimConfig::default();
    if let Some(path) = tape_path {
        let text = fs::read_to_string(path)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tape.csv");
        let tape = load_bybit(&text, &cfg, name, Some(cfg.steps), Some(1_000), None)?;
        let market = TapeMarket::new(&cfg, &tape, 20, 60.0, 200, cfg.seed);
        let loaded = requests_from_tape(
            &cfg,
            &market,
            &tape,
            // this has to take the same one. Hardcoding `RoundRobin(24)` here
            // twenty-four; when that moved, this arm silently kept measuring a
            // different population.
            Entities::PerAddress,
            1,
            cfg.seed + 2,
        );
        let mut meta = Map::new();
        meta.insert("source".into(), json!(tape.source));
        meta.insert("entity_kind".into(), json!(loaded.entity_kind));
        for (key, value) in &loaded.meta {
            meta.insert(key.clone(), json!(value));
        }
        for (key, value) in &tape.meta_text {
            meta.insert(key.clone(), json!(value));
        }
        let makers = build_market_makers(&loaded.cfg, loaded.cfg.seed + 1);
        Ok(Setup {
            cfg: loaded.cfg,
            market: LabMarket::Tape(Box::new(market)),
            makers,
            requests: loaded.requests,
            meta: Value::Object(meta),
        })
    } else {
        let market = ReferenceMarket::new(&cfg, cfg.seed);
        let requests = build_requests(&cfg, &market, cfg.seed + 2);
        Ok(Setup {
            cfg,
            market: LabMarket::Generated(market),
            makers: build_market_makers(&cfg, cfg.seed + 1),
            requests,
            meta: json!({"source": "generated"}),
        })
    }
}

fn windows_of(setup: &Setup, seed: u64) -> Vec<WindowObservation> {
    let mut disclosure = Disclosure::None;
    let options = ArmOptions::new("qomm_rfq", seed);
    run_arm(
        &setup.cfg,
        &setup.market,
        &setup.requests,
        &setup.makers,
        &mut disclosure,
        &options,
    )
    .windows
}

fn curve(windows: &[WindowObservation], enrolled: usize) -> Vec<Value> {
    let mut rows = Vec::new();
    for width in 1..=windows.len() {
        let mut counts = Vec::new();
        for start in 0..=windows.len() - width {
            let entities: BTreeSet<usize> = windows[start..start + width]
                .iter()
                .flat_map(|window| window.requests_by_entity.keys().copied())
                .collect();
            counts.push(entities.len() as f64);
        }
        let mean = fsum(counts.iter().copied()) / counts.len() as f64;
        rows.push(json!({
            "width_windows": width,
            "true_distinct_mean": mean,
            "fraction_of_enrolment": if enrolled == 0 { 0.0 } else { mean / enrolled as f64 },
            "starts": counts.len(),
        }));
    }
    rows
}

fn saturation_width(rows: &[Value], enrolled: usize) -> Option<usize> {
    rows.iter().find_map(|row| {
        let mean = row["true_distinct_mean"].as_f64()?;
        (enrolled > 0 && mean >= SATURATION * enrolled as f64)
            .then(|| row["width_windows"].as_u64().unwrap() as usize)
    })
}

fn measured_sensitivity(windows: &[WindowObservation]) -> HarnessResult<Value> {
    let mut out = Vec::new();
    for width in [1, 5, 10, windows.len()] {
        if width > windows.len() {
            continue;
        }
        let span = &windows[..width];
        let present: BTreeSet<usize> = span
            .iter()
            .flat_map(|window| window.requests_by_entity.keys().copied())
            .collect();
        if present.is_empty() {
            continue;
        }
        let mut counted: BTreeMap<usize, i64> = BTreeMap::new();
        for window in span {
            for (entity, count) in &window.requests_by_entity {
                *counted.entry(*entity).or_default() += count;
            }
        }
        let (&busiest, &requests) = counted
            .iter()
            .max_by_key(|(_, count)| *count)
            .ok_or("a nonempty span had no entity counts")?;
        let without = present.iter().filter(|entity| **entity != busiest).count();
        out.push(json!({
            "width_windows": width,
            "entity_count_moves_by": present.len() - without,
            "that_entity_made_requests": requests,
            "event_count_would_move_by_at_most": event_count_sensitivity(width, requests)?,
        }));
    }
    let flat = out
        .iter()
        .all(|row| row["entity_count_moves_by"].as_i64() == Some(SENSITIVITY));
    Ok(json!({"rows": out, "entity_sensitivity_is_flat": flat}))
}

fn noise_check(
    windows: &[WindowObservation],
    epsilon: f64,
    draws: usize,
    seed: u64,
) -> HarnessResult<Value> {
    if windows.is_empty() || draws == 0 {
        return Err("noise check needs at least one window and one draw".into());
    }
    let mut rng = DeterministicRng::new(seed);
    let query = BlockRangeQuery::new(windows[0].start_step, windows[windows.len() - 1].end_step)?;
    let truth = windows_in_range(windows, &query)
        .into_iter()
        .flat_map(|window| window.requests_by_entity.keys().copied())
        .collect::<BTreeSet<_>>()
        .len() as i64;
    let mut errors = Vec::with_capacity(draws);
    for _ in 0..draws {
        let mut accountant = EntityAccountant::new(1e9);
        let answer = answer_block_range_query(windows, &query, epsilon, &mut accountant, &mut rng);
        errors.push(answer.count.ok_or("block query was unexpectedly refused")? - truth);
    }
    let p = (-epsilon / SENSITIVITY as f64).exp();
    Ok(json!({
        "epsilon": epsilon,
        "true_distinct": truth,
        "sd_measured": (errors.iter().map(|error| (*error as f64) * (*error as f64)).sum::<f64>() / errors.len() as f64).sqrt(),
        "sd_predicted": (2.0 * p).sqrt() / (1.0 - p),
        "noise_scale_entities": noise_scale(epsilon),
        "draws": draws,
    }))
}

fn arm(
    name: &str,
    setup: &Setup,
    seeds: usize,
    epsilon: f64,
    draws: usize,
) -> HarnessResult<Value> {
    let mut per_seed = Vec::new();
    for index in 0..seeds {
        let windows = windows_of(setup, setup.cfg.seed + 5 + index as u64);
        let enrolled = windows
            .iter()
            .flat_map(|window| window.requests_by_entity.keys().copied())
            .collect::<BTreeSet<_>>()
            .len();
        let rows = curve(&windows, enrolled);
        per_seed.push(json!({
            "enrolled_seen": enrolled,
            "windows": windows.len(),
            "saturation_width": saturation_width(&rows, enrolled),
            "curve": rows,
            "sensitivity": measured_sensitivity(&windows)?,
            "noise": noise_check(&windows, epsilon, draws, setup.cfg.seed + 900 + index as u64)?,
        }));
    }
    let enrolled: Vec<usize> = per_seed
        .iter()
        .map(|row| row["enrolled_seen"].as_u64().unwrap() as usize)
        .collect();
    let widths: Vec<usize> = per_seed
        .iter()
        .filter_map(|row| row["saturation_width"].as_u64().map(|value| value as usize))
        .collect();
    let measured: Vec<f64> = per_seed
        .iter()
        .filter_map(|row| row["noise"]["sd_measured"].as_f64())
        .collect();
    Ok(json!({
        "arm": name,
        "meta": setup.meta,
        "seeds": seeds,
        "enrolment_seen_median": median_usize(&enrolled),
        "saturation_width_median": if widths.is_empty() { Value::Null } else { median_usize(&widths) },
        "saturation_width_all": widths,
        "sensitivity_flat_every_seed": per_seed.iter().all(|row| row["sensitivity"]["entity_sensitivity_is_flat"].as_bool() == Some(true)),
        "sd_measured_median": median(&measured),
        "sd_predicted": per_seed.first().and_then(|row| row["noise"]["sd_predicted"].as_f64()),
        "per_seed": per_seed,
    }))
}

fn median_usize(values: &[usize]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let middle = ordered.len() / 2;
    if ordered.len() % 2 == 1 {
        json!(ordered[middle])
    } else {
        json!(0.5 * (ordered[middle - 1] + ordered[middle]) as f64)
    }
}

fn real_identity_arm(
    path: &Path,
    epsilon: f64,
    widths: &[usize],
    samples: usize,
    seed: u64,
) -> HarnessResult<Value> {
    let mut records = Vec::new();
    for (line_number, line) in fs::read_to_string(path)?.lines().enumerate() {
        if line.trim().is_empty() || line.contains("\"checkpoint\"") {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|error| format!("{}:{}: {error}", path.display(), line_number + 1))?;
        let block = value["block"].as_u64().ok_or_else(|| {
            format!(
                "{}:{} has no integer block",
                path.display(),
                line_number + 1
            )
        })? as usize;
        let swapper = value["swapper"]
            .as_str()
            .ok_or_else(|| format!("{}:{} has no swapper", path.display(), line_number + 1))?
            .to_string();
        records.push((block, swapper));
    }
    records.sort();
    let lo_block = records.first().ok_or("the identity tape is empty")?.0;
    let hi_block = records.last().unwrap().0;
    let blocks: Vec<usize> = records.iter().map(|record| record.0).collect();
    let swappers: Vec<&str> = records.iter().map(|record| record.1.as_str()).collect();
    let mut out = Vec::new();
    for &width in widths {
        if hi_block - lo_block <= width {
            continue;
        }
        let population = hi_block - width - lo_block;
        if samples > population {
            return Err(format!("cannot sample {samples} starts from {population}").into());
        }
        let starts: Vec<usize> = DeterministicRng::new(seed + width as u64)
            .sample(population, samples)
            .into_iter()
            .map(|offset| lo_block + offset)
            .collect();
        let mut pairs = Vec::with_capacity(samples);
        for &start in &starts {
            let left = blocks.partition_point(|block| *block < start);
            let right = blocks.partition_point(|block| *block < start + width);
            let segment = &swappers[left..right];
            pairs.push((
                segment.len() as f64,
                segment.iter().copied().collect::<BTreeSet<_>>().len() as f64,
            ));
        }
        let fills: Vec<f64> = pairs.iter().map(|pair| pair.0).collect();
        let distinct: Vec<f64> = pairs.iter().map(|pair| pair.1).collect();
        let mean_fills = fsum(fills.iter().copied()) / fills.len() as f64;
        let mean_distinct = fsum(distinct.iter().copied()) / distinct.len() as f64;
        // The metric contract uses compensated summation.
        let sxy = fsum(
            pairs
                .iter()
                .map(|(fills, distinct)| (fills - mean_fills) * (distinct - mean_distinct)),
        );
        let sxx = fsum(fills.iter().map(|fills| (fills - mean_fills).powi(2)));
        let syy = fsum(
            distinct
                .iter()
                .map(|distinct| (distinct - mean_distinct).powi(2)),
        );
        if sxx == 0.0 || syy == 0.0 {
            continue;
        }
        let slope = sxy / sxx;
        let residuals: Vec<f64> = pairs
            .iter()
            .map(|(fills, distinct)| distinct - (mean_distinct + slope * (fills - mean_fills)))
            .collect();
        let residual_sd = population_sd(&residuals).expect("the sample is nonempty");
        let p = (-epsilon / SENSITIVITY as f64).exp();
        let dp_sd = (2.0 * p).sqrt() / (1.0 - p);
        out.push(json!({
            "width_blocks": width,
            "width_hours_at_12s": width as f64 * 12.0 / 3_600.0,
            "mean_fills": mean_fills,
            "mean_distinct_swappers": mean_distinct,
            "r2_against_public_fill_count": (sxy * sxy) / (sxx * syy),
            "slope_distinct_per_fill": slope,
            "residual_sd_entities": residual_sd,
            "dp_noise_sd_entities": dp_sd,
            "signal_to_noise": residual_sd / dp_sd,
            "samples": samples,
        }));
    }

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for swapper in &swappers {
        *counts.entry(swapper).or_default() += 1;
    }
    let mut tally: Vec<usize> = counts.values().copied().collect();
    tally.sort_unstable();
    let top_count = (tally.len() / 100).max(1);
    Ok(json!({
        "arm": "uniswapx_real_identities",
        "source": path.file_name().and_then(|name| name.to_str()).unwrap_or("uniswapx"),
        "fills": records.len(),
        "distinct_swappers": counts.len(),
        "fills_per_swapper": records.len() as f64 / counts.len() as f64,
        "swappers_appearing_once": tally.iter().filter(|count| **count == 1).count() as f64 / tally.len() as f64,
        "top_1pct_share_of_fills": tally[tally.len() - top_count..].iter().sum::<usize>() as f64 / records.len() as f64,
        "block_span": hi_block - lo_block,
        "saturates": false,
        "why_not": "48,000 swappers over 1.43M blocks with 60% appearing once. The pool is unbounded against any range an asker names, so the count is close to linear in width. The one-window saturation on the simulator was the round-robin assignment.",
        "what_this_cannot_say": "UniswapX records fills, so these are settled requests. QOMM's denominator includes requests that settled nothing, and no chain records those. If they come from a different population the relation here does not carry.",
        "rows": out,
    }))
}

fn copy_prediction_next_to(out: &Path) -> HarnessResult<()> {
    let source = qomm_harness::repo_root().join("artifacts/block_range_query_prediction.json");
    let Some(parent) = out.parent() else {
        return Ok(());
    };
    let target = parent.join("block_range_query_prediction.json");
    if source != target {
        fs::create_dir_all(parent)?;
        fs::copy(source, target)?;
    }
    Ok(())
}

fn parse_args() -> HarnessResult<Options> {
    let root = qomm_harness::repo_root();
    let mut options = Options {
        out: root.join("artifacts/block_range_query.json"),
        seeds: 5,
        epsilon: 1.0,
        draws: 4_000,
        tape: root.join("artifacts/tapes/LTCUSDT2021-06-15.csv"),
        uniswapx: root.join("artifacts/tapes/uniswapx_amounts.jsonl"),
        samples: 400,
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
            "--epsilon" => options.epsilon = raw.parse()?,
            "--draws" => options.draws = raw.parse()?,
            "--tape" => options.tape = raw.into(),
            "--uniswapx" => options.uniswapx = raw.into(),
            "--samples" => options.samples = raw.parse()?,
            other => return Err(format!("unknown argument {other}").into()),
        }
        index += 1;
    }
    if options.seeds == 0 || options.draws == 0 || options.samples == 0 {
        return Err("--seeds, --draws and --samples must be positive".into());
    }
    Ok(options)
}
