//! Rust port of `scripts/run_dp_audit.py`.

use qomm_harness::{write_pretty_json, HarnessResult};
use qomm_sim::audit::{audit_window, AuditResult, AuditSettings, Field};
use qomm_sim::disclosure::WindowObservation;
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::experiment::{make_disclosure, DpParams};
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

#[derive(Clone)]
struct Options {
    out: PathBuf,
    steps: usize,
    window_steps: usize,
    n_entities: usize,
    arrival_rate: f64,
    epsilons: Vec<f64>,
    trials: usize,
    windows: usize,
    entities: usize,
    fields: Vec<String>,
    seed: u64,
    workers: usize,
}

#[derive(Serialize)]
struct Row {
    window: usize,
    entity: usize,
    trials: usize,
    declared_epsilon: f64,
    field_epsilon: f64,
    empirical_epsilon: f64,
    best_threshold: Value,
    within_claim: bool,
    entity_requests: i64,
    entity_volume: i64,
    field: String,
}

struct Job {
    observation: WindowObservation,
    entity: usize,
    settings: AuditSettings,
    field: String,
}

impl Row {
    fn from_result(result: AuditResult, field: String) -> Self {
        // Python's `round()` produces an integer threshold after the first
        // positive result, but the untouched sentinel is the float `0.0`.
        let best_threshold = if result.empirical_epsilon > 0.0 {
            json!(result.best_threshold as i64)
        } else {
            json!(0.0)
        };
        Self {
            window: result.window,
            entity: result.entity,
            trials: result.trials,
            declared_epsilon: result.declared_epsilon,
            field_epsilon: result.field_epsilon,
            empirical_epsilon: result.empirical_epsilon,
            best_threshold,
            within_claim: result.within_claim,
            entity_requests: result.entity_requests,
            entity_volume: result.entity_volume,
            field,
        }
    }
}

#[derive(Default, Serialize)]
struct FieldBucket {
    cells: usize,
    max_empirical_epsilon: f64,
    violations: usize,
}

#[derive(Serialize)]
struct Bucket {
    declared_epsilon: f64,
    field_epsilon: f64,
    cells: usize,
    max_empirical_epsilon: f64,
    violations: usize,
    by_field: BTreeMap<String, FieldBucket>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    let cfg = SimConfig {
        steps: options.steps,
        window_steps: options.window_steps,
        n_entities: options.n_entities,
        arrival_rate: options.arrival_rate,
        seed: options.seed,
        ..SimConfig::default()
    };
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let requests = build_requests(&cfg, &market, cfg.seed + 2);
    let dp = DpParams {
        epsilon_per_window: 1.0,
        epsilon_total: 1e9,
        ..DpParams::default()
    };
    let mut disclosure = make_disclosure("C_dp", &cfg, &dp);
    let arm_options = ArmOptions::new("qomm_rfq", cfg.seed + 5);
    let result = run_arm(
        &cfg,
        &market,
        &requests,
        &makers,
        &mut disclosure,
        &arm_options,
    );

    // Python dictionaries retain the first-seen entity order. The simulation
    // core uses BTreeMap for deterministic keyed state, so reconstruct that
    // insertion order from the attempt stream for tie-breaking below.
    let mut first_seen_by_window: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
    for attempt in &result.truth {
        let order = first_seen_by_window
            .entry(attempt.step / options.window_steps)
            .or_default();
        let rank = order.len();
        order.entry(attempt.entity).or_insert(rank);
    }

    let mut windows = result.windows;
    windows.sort_by(|left, right| right.requests.cmp(&left.requests));
    windows.truncate(options.windows);

    let job_count =
        options.epsilons.len() * windows.len() * options.entities * options.fields.len();
    let mut jobs = Vec::with_capacity(job_count);
    for epsilon in &options.epsilons {
        for (index, observation) in windows.iter().enumerate() {
            let mut busiest: Vec<(usize, i64)> = observation
                .requests_by_entity
                .iter()
                .map(|(entity, count)| (*entity, *count))
                .collect();
            let first_seen = first_seen_by_window.get(&observation.window);
            busiest.sort_by(|left, right| {
                right.1.cmp(&left.1).then_with(|| {
                    let left_rank = first_seen
                        .and_then(|order| order.get(&left.0))
                        .copied()
                        .unwrap_or(usize::MAX);
                    let right_rank = first_seen
                        .and_then(|order| order.get(&right.0))
                        .copied()
                        .unwrap_or(usize::MAX);
                    left_rank.cmp(&right_rank)
                })
            });
            for (entity, _) in busiest.into_iter().take(options.entities) {
                for field_name in &options.fields {
                    let settings = AuditSettings {
                        epsilon_per_window: *epsilon,
                        request_cap: dp.request_cap,
                        volume_cap: dp.volume_cap,
                        trials: options.trials,
                        seed: options.seed + 17 * index as u64,
                        n_entities: cfg.n_entities,
                        field: parse_field(field_name)?,
                        signed_sensitivity_factor: dp.signed_sensitivity_factor,
                        ..AuditSettings::default()
                    };
                    jobs.push(Job {
                        observation: observation.clone(),
                        entity,
                        settings,
                        field: field_name.clone(),
                    });
                }
            }
        }
    }

    let requested_workers = if options.workers == 0 {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    } else {
        options.workers
    };
    let worker_count = requested_workers.max(1).min(job_count.max(1));
    println!("auditing {job_count} (window, entity, epsilon) cells with {worker_count} workers");
    let next = AtomicUsize::new(0);
    let output_rows = std::thread::scope(|scope| -> HarnessResult<Vec<Row>> {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..worker_count {
            let sender = sender.clone();
            let jobs = &jobs;
            let next = &next;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(index) else {
                    break;
                };
                let row = Row::from_result(
                    audit_window(&job.observation, job.entity, &job.settings),
                    job.field.clone(),
                );
                if sender.send((index, row)).is_err() {
                    break;
                }
            });
        }
        drop(sender);

        let mut ordered: Vec<Option<Row>> = (0..job_count).map(|_| None).collect();
        for completed in 1..=job_count {
            let (index, row) = receiver
                .recv()
                .map_err(|error| format!("DP audit worker stopped early: {error}"))?;
            ordered[index] = Some(row);
            if completed % 10 == 0 || completed == job_count {
                println!("  {completed}/{job_count}");
            }
        }
        ordered
            .into_iter()
            .enumerate()
            .map(|(index, row)| row.ok_or_else(|| format!("missing DP audit row {index}").into()))
            .collect()
    })?;

    let mut buckets: Vec<Bucket> = Vec::new();
    for row in &output_rows {
        let index = buckets
            .iter()
            .position(|bucket| bucket.declared_epsilon.to_bits() == row.declared_epsilon.to_bits());
        let bucket = match index {
            Some(index) => &mut buckets[index],
            None => {
                buckets.push(Bucket {
                    declared_epsilon: row.declared_epsilon,
                    field_epsilon: row.field_epsilon,
                    cells: 0,
                    max_empirical_epsilon: 0.0,
                    violations: 0,
                    by_field: BTreeMap::new(),
                });
                buckets.last_mut().unwrap()
            }
        };
        bucket.cells += 1;
        bucket.max_empirical_epsilon = bucket.max_empirical_epsilon.max(row.empirical_epsilon);
        bucket.violations += usize::from(!row.within_claim);
        let field = bucket.by_field.entry(row.field.clone()).or_default();
        field.cells += 1;
        field.max_empirical_epsilon = field.max_empirical_epsilon.max(row.empirical_epsilon);
        field.violations += usize::from(!row.within_claim);
    }
    buckets.sort_by(f64_bucket_order);

    let fields: Vec<Value> = options.fields.iter().map(|field| json!(field)).collect();
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "config": {
            "steps": options.steps,
            "window_steps": options.window_steps,
            "n_entities": options.n_entities,
            "arrival_rate": options.arrival_rate,
            "trials": options.trials,
            "request_cap": dp.request_cap,
            "volume_cap": dp.volume_cap,
            "fields": fields,
            "note": concat!(
                "epsilon is split across 4 released fields, and every one of them is audited: ",
                "the empirical lower bound for each is compared against epsilon/4, the claim ",
                "that actually binds that field. Only the request count used to be audited, and ",
                "the fill count --- the one whose sensitivity was wrong --- was the one nothing ",
                "looked at"
            ),
        },
        "rows": output_rows,
        "by_epsilon": buckets,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("{}", serde_json::to_string_pretty(&payload["by_epsilon"])?);
    Ok(())
}

fn f64_bucket_order(left: &Bucket, right: &Bucket) -> std::cmp::Ordering {
    left.declared_epsilon.total_cmp(&right.declared_epsilon)
}

fn parse_field(name: &str) -> HarnessResult<Field> {
    match name {
        "noisy_requests" => Ok(Field::Requests),
        "noisy_volume" => Ok(Field::Volume),
        "noisy_signed_volume" => Ok(Field::SignedVolume),
        "noisy_fills" => Ok(Field::Fills),
        _ => Err(format!("unknown released field: {name}").into()),
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: PathBuf::new(),
        steps: 48_000,
        window_steps: 1_200,
        n_entities: 24,
        arrival_rate: 0.15,
        epsilons: vec![0.25, 1.0, 4.0],
        trials: 4_000,
        windows: 6,
        entities: 4,
        fields: vec![
            "noisy_requests".into(),
            "noisy_volume".into(),
            "noisy_signed_volume".into(),
            "noisy_fills".into(),
        ],
        seed: 20_260_818,
        workers: 0,
    };
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < raw.len() {
        let name = &raw[index];
        index += 1;
        let next = |index: &mut usize| -> HarnessResult<&str> {
            let value = raw
                .get(*index)
                .ok_or_else(|| format!("argument {name} expects one value"))?;
            *index += 1;
            Ok(value)
        };
        match name.as_str() {
            "--out" => options.out = PathBuf::from(next(&mut index)?),
            "--steps" => options.steps = next(&mut index)?.parse()?,
            "--window-steps" => options.window_steps = next(&mut index)?.parse()?,
            "--n-entities" => options.n_entities = next(&mut index)?.parse()?,
            "--arrival-rate" => options.arrival_rate = next(&mut index)?.parse()?,
            "--trials" => options.trials = next(&mut index)?.parse()?,
            "--windows" => options.windows = next(&mut index)?.parse()?,
            "--entities" => options.entities = next(&mut index)?.parse()?,
            "--seed" => options.seed = next(&mut index)?.parse()?,
            "--workers" => options.workers = next(&mut index)?.parse()?,
            "--epsilons" => {
                options.epsilons.clear();
                while index < raw.len() && !raw[index].starts_with("--") {
                    options.epsilons.push(raw[index].parse()?);
                    index += 1;
                }
                if options.epsilons.is_empty() {
                    return Err("--epsilons expects one or more values".into());
                }
            }
            "--fields" => {
                options.fields.clear();
                while index < raw.len() && !raw[index].starts_with("--") {
                    parse_field(&raw[index])?;
                    options.fields.push(raw[index].clone());
                    index += 1;
                }
                if options.fields.is_empty() {
                    return Err("--fields expects one or more values".into());
                }
            }
            "-h" | "--help" => {
                println!("usage: run_dp_audit --out PATH [--steps N] [--window-steps N] [--n-entities N] [--arrival-rate X] [--epsilons E ...] [--trials N] [--windows N] [--entities N] [--fields FIELD ...] [--seed N] [--workers N]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    if options.out.as_os_str().is_empty() {
        return Err("the following argument is required: --out".into());
    }
    if options.window_steps == 0 || options.n_entities == 0 || options.trials == 0 {
        return Err("window steps, entity count, and trials must be positive".into());
    }
    Ok(options)
}
