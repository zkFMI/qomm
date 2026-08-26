//! Rust port of `scripts/run_sim_matrix.py`.

use qomm_harness::smallsample::mean_ci;
use qomm_harness::{median, sum_mean, write_pretty_json, HarnessResult};
use qomm_sim::attackers::{self as atk, AttackReport};
use qomm_sim::engine::{run_arm, ArmOptions, ArmResult};
use qomm_sim::experiment::{build_probes, make_disclosure, DpParams};
use qomm_sim::lab::LabMarket;
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use qomm_sim::tapes::{load_bybit, load_uniswapx, requests_from_tape, Entities, TapeMarket};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

#[derive(Clone)]
struct TapeSpec {
    kind: String,
    path: PathBuf,
    step_ms: Option<u64>,
    step_blocks: usize,
    max_rows: Option<usize>,
    entities: Option<usize>,
}

#[derive(Clone)]
struct Job {
    cfg: SimConfig,
    dp: DpParams,
    protocol: String,
    disclosure: String,
    layer: String,
    probes_per_window: usize,
    tape: Option<TapeSpec>,
}

struct Options {
    out: PathBuf,
    seeds: usize,
    seed0: u64,
    steps: usize,
    n_mm: usize,
    n_entities: usize,
    arrival_rate: f64,
    window_steps: usize,
    epsilons: Vec<f64>,
    epsilon_total: f64,
    protocols: Vec<String>,
    disclosures: Vec<String>,
    layers: Vec<String>,
    probes_per_window: usize,
    workers: usize,
    tape: Option<String>,
    tape_paths: Vec<PathBuf>,
    tape_step_ms: Option<u64>,
    tape_step_blocks: usize,
    tape_max_rows: Option<usize>,
    tape_entities: Option<usize>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.seeds == 0 {
        return Err("--seeds must be positive".into());
    }
    if options.epsilons.is_empty()
        || options.protocols.is_empty()
        || options.disclosures.is_empty()
        || options.layers.is_empty()
    {
        return Err("matrix list arguments must not be empty".into());
    }
    if options.tape.is_some() && options.tape_paths.is_empty() {
        return Err("--tape needs --tape-paths".into());
    }

    let mut jobs = Vec::new();
    for seed_index in 0..options.seeds {
        let cfg = SimConfig {
            steps: options.steps,
            n_mm: options.n_mm,
            n_entities: options.n_entities,
            arrival_rate: options.arrival_rate,
            window_steps: options.window_steps,
            seed: options.seed0 + 1_000 * seed_index as u64,
            ..SimConfig::default()
        };
        for (epsilon_index, epsilon) in options.epsilons.iter().enumerate() {
            let dp = DpParams {
                epsilon_per_window: *epsilon,
                epsilon_total: options.epsilon_total,
                request_cap: 3,
                volume_cap: 300,
                ..DpParams::default()
            };
            for layer in &options.layers {
                for protocol in &options.protocols {
                    for disclosure in &options.disclosures {
                        if disclosure != "C_dp" && epsilon_index != 0 {
                            continue;
                        }
                        let tapes: Vec<Option<TapeSpec>> = match &options.tape {
                            None => vec![None],
                            Some(kind) => options
                                .tape_paths
                                .iter()
                                .map(|path| {
                                    Some(TapeSpec {
                                        kind: kind.clone(),
                                        path: path.clone(),
                                        step_ms: options.tape_step_ms,
                                        step_blocks: options.tape_step_blocks,
                                        max_rows: options.tape_max_rows,
                                        entities: options.tape_entities,
                                    })
                                })
                                .collect(),
                        };
                        for tape in tapes {
                            jobs.push(Job {
                                cfg,
                                dp,
                                protocol: protocol.clone(),
                                disclosure: disclosure.clone(),
                                layer: layer.clone(),
                                probes_per_window: options.probes_per_window,
                                tape,
                            });
                        }
                    }
                }
            }
        }
    }

    println!("running {} cells", jobs.len());
    let worker_count = if options.workers == 0 {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
    } else {
        options.workers
    }
    .max(1)
    .min(jobs.len().max(1));
    let next = AtomicUsize::new(0);
    let rows = std::thread::scope(|scope| -> HarnessResult<Vec<Value>> {
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
                let result = one_cell(job).map_err(|error| error.to_string());
                if sender.send((index, result)).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        let mut ordered = vec![None; jobs.len()];
        for (completed, (index, result)) in receiver.into_iter().enumerate() {
            ordered[index] = Some(result.map_err(|error| format!("cell {index}: {error}"))?);
            let count = completed + 1;
            if count % 25 == 0 || count == jobs.len() {
                println!("  {count}/{}", jobs.len());
            }
        }
        ordered
            .into_iter()
            .map(|row| row.ok_or_else(|| "a simulation worker omitted its result".into()))
            .collect()
    })?;

    let payload = json!({
        "config": {
            "steps": options.steps,
            "n_mm": options.n_mm,
            "n_entities": options.n_entities,
            "arrival_rate": options.arrival_rate,
            "window_steps": options.window_steps,
            "seeds": options.seeds,
            "epsilons": options.epsilons,
            "epsilon_total": options.epsilon_total,
            "probes_per_window": options.probes_per_window,
        },
        "rows": rows,
        "aggregate": aggregate(&rows),
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn one_cell(job: &Job) -> HarnessResult<Value> {
    let (cfg, market, requests, tape_meta) = if let Some(spec) = &job.tape {
        let text = fs::read_to_string(&spec.path)?;
        let name = spec
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tape");
        let tape = match spec.kind.as_str() {
            "bybit" => load_bybit(
                &text,
                &job.cfg,
                name,
                Some(job.cfg.steps),
                spec.step_ms,
                spec.max_rows,
            )?,
            "uniswapx" => load_uniswapx(
                &text,
                &job.cfg,
                name,
                Some(job.cfg.steps),
                spec.step_blocks,
                1,
                None,
            )?,
            other => return Err(format!("unknown tape kind {other:?}").into()),
        };
        let tape_market = TapeMarket::new(&job.cfg, &tape, 20, 60.0, 200, job.cfg.seed);
        let entities = spec
            .entities
            .map_or(Entities::PerAddress, Entities::RoundRobin);
        let loaded =
            requests_from_tape(&job.cfg, &tape_market, &tape, entities, 1, job.cfg.seed + 2);
        let mut meta = Map::new();
        meta.insert("source".into(), json!(tape.source));
        meta.insert("entity_kind".into(), json!(loaded.entity_kind));
        for (key, value) in &loaded.meta {
            meta.insert(key.clone(), json!(value));
        }
        for (key, value) in &tape.meta_text {
            meta.insert(key.clone(), json!(value));
        }
        (
            loaded.cfg,
            LabMarket::Tape(tape_market),
            loaded.requests,
            Some(Value::Object(meta)),
        )
    } else {
        let market = ReferenceMarket::new(&job.cfg, job.cfg.seed);
        let requests = build_requests(&job.cfg, &market, job.cfg.seed + 2);
        (job.cfg, LabMarket::Generated(market), requests, None)
    };

    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let probes = build_probes(&cfg, job.probes_per_window, 50);
    let mut disclosure = make_disclosure(&job.disclosure, &cfg, &job.dp);
    let mut arm_options = ArmOptions::new(&job.protocol, cfg.seed + 5);
    arm_options.probes = probes.clone();
    arm_options.reactive = job.layer == "reactive";
    let result = run_arm(
        &cfg,
        &market,
        &requests,
        &makers,
        &mut disclosure,
        &arm_options,
    );
    let attacks = vec![
        atk::passive_observer(&result, &cfg, 0.5, cfg.seed),
        atk::pretrade_attributes(&result, &cfg),
        atk::window_shift_observer(&result, &cfg),
        atk::probing_entity(&result, probes.len()),
        atk::colluding_wallets(&result, &cfg, 4, 4),
        atk::external_info_observer(&result, &cfg, &market),
    ];
    let mut row = result_summary(&result);
    if let Some(meta) = tape_meta {
        row.insert("tape".into(), meta);
    }
    row.insert("layer".into(), json!(job.layer));
    row.insert("seed".into(), json!(cfg.seed));
    row.insert(
        "epsilon_per_window".into(),
        json!(job.dp.epsilon_per_window),
    );
    row.insert("epsilon_total".into(), json!(job.dp.epsilon_total));
    row.insert(
        "attacks".into(),
        Value::Array(
            attacks
                .iter()
                .map(|report| attack_value(report, &result))
                .collect(),
        ),
    );
    Ok(Value::Object(row))
}

fn result_summary(result: &ArmResult) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("protocol".into(), json!(result.protocol));
    out.insert("disclosure".into(), json!(result.disclosure));
    out.insert("requests".into(), json!(result.requests));
    out.insert("fills".into(), json!(result.fills));
    out.insert("fill_rate".into(), json!(result.fill_rate()));
    out.insert(
        "no_quote_rate".into(),
        json!(if result.requests == 0 {
            0.0
        } else {
            result.no_quote as f64 / result.requests as f64
        }),
    );
    out.insert(
        "user_cost_mean_ticks".into(),
        json!(sum_mean(&result.user_cost_ticks)),
    );
    out.insert(
        "user_cost_median_ticks".into(),
        json!(median(&result.user_cost_ticks)),
    );
    out.insert("mm_pnl_total_ticklots".into(), json!(result.mm_pnl_total()));
    out.insert("mm_pnl_per_fill".into(), json!(result.mm_pnl_per_fill()));
    out.insert(
        "quote_continuation".into(),
        json!(result.quote_continuation),
    );
    out.insert("suppression_rate".into(), json!(result.suppression_rate));
    out.insert("epsilon_spent_max".into(), json!(result.epsilon_spent_max));
    for (key, values) in &result.mm_markouts {
        out.insert(format!("mm_{key}_mean"), json!(sum_mean(values)));
    }
    for (key, values) in &result.release_errors {
        out.insert(format!("release_{key}_mae"), json!(sum_mean(values)));
    }
    out
}

fn attack_value(report: &AttackReport, result: &ArmResult) -> Value {
    let mut out = Map::new();
    out.insert("attacker".into(), json!(report.name));
    out.insert("target".into(), json!(report.target));
    out.insert("auc".into(), json!(report.auc));
    out.insert("tpr_at_5pct_fpr".into(), json!(report.tpr_at_5pct_fpr));
    out.insert("base_rate".into(), json!(report.base_rate));
    out.insert("advantage_over_prior".into(), json!(report.advantage));
    out.insert("n".into(), json!(report.n_examples));
    for (key, value) in &report.extra {
        out.insert((*key).into(), json!(value));
    }
    if report.name == "A3_probing_entity" && report.n_examples < 4 {
        out.insert("note".into(), json!("insufficient probes"));
    }
    if report.name == "A4_colluding_wallets" {
        let curve = atk::probe_cost_curve(result)
            .into_iter()
            .map(|(budget, (net, per_mm))| {
                (budget.to_string(), json!({"net": net, "per_mm": per_mm}))
            })
            .collect::<Map<_, _>>();
        out.insert("probe_cost_curve".into(), Value::Object(curve));
    }
    Value::Object(out)
}

fn aggregate(rows: &[Value]) -> Vec<Value> {
    type Key = (String, String, String, String, u64);
    let mut groups: BTreeMap<Key, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        let source = row
            .get("tape")
            .and_then(|tape| tape.get("source"))
            .and_then(Value::as_str)
            .unwrap_or("generated");
        let epsilon = row["epsilon_per_window"].as_f64().unwrap();
        let key = (
            source.to_string(),
            row["layer"].as_str().unwrap().to_string(),
            row["protocol"].as_str().unwrap().to_string(),
            row["disclosure"].as_str().unwrap().to_string(),
            epsilon.to_bits(),
        );
        groups.entry(key).or_default().push(row);
    }

    let numeric = [
        "fill_rate",
        "no_quote_rate",
        "user_cost_mean_ticks",
        "mm_pnl_per_fill",
        "mm_markout_50ms_mean",
        "mm_markout_1s_mean",
        "mm_markout_10s_mean",
        "quote_continuation",
        "suppression_rate",
        "epsilon_spent_max",
        "release_requests_mae",
        "release_signed_volume_mae",
    ];
    let attack_numeric: [(&str, &[&str]); 6] = [
        (
            "A1_passive_observer",
            &["auc", "advantage_over_prior", "tpr_at_5pct_fpr"],
        ),
        (
            "A1b_pretrade_attributes",
            &[
                "direction_accuracy",
                "direction_prior",
                "size_bucket_accuracy",
                "size_bucket_prior",
            ],
        ),
        ("A2_window_shift", &["auc", "advantage_over_prior"]),
        (
            "A3_probing_entity",
            &[
                "net_inventory_corr_from_best_quote",
                "own_inventory_corr_from_per_mm_quotes",
            ],
        ),
        (
            "A4_colluding_wallets",
            &[
                "corr_under_wallet_limit",
                "corr_under_entity_limit",
                "probes_needed_net_corr_0.8",
                "probes_needed_per_mm_corr_0.8",
            ],
        ),
        ("A5_external_info", &["auc", "advantage_over_prior"]),
    ];

    groups
        .into_iter()
        .map(
            |((source, layer, protocol, disclosure, epsilon_bits), items)| {
                let mut summary = Map::new();
                summary.insert("source".into(), json!(source));
                summary.insert("layer".into(), json!(layer));
                summary.insert("protocol".into(), json!(protocol));
                summary.insert("disclosure".into(), json!(disclosure));
                summary.insert(
                    "epsilon_per_window".into(),
                    json!(f64::from_bits(epsilon_bits)),
                );
                summary.insert("n_seeds".into(), json!(items.len()));
                for field in numeric {
                    let values: Vec<f64> = items
                        .iter()
                        .filter_map(|item| item.get(field).and_then(Value::as_f64))
                        .collect();
                    summary.insert(field.into(), aggregate_ci(&values));
                }
                for (name, fields) in attack_numeric {
                    for field in fields {
                        let values: Vec<f64> = items
                            .iter()
                            .filter_map(|item| item["attacks"].as_array())
                            .flatten()
                            .filter(|attack| attack["attacker"].as_str() == Some(name))
                            .filter_map(|attack| attack.get(*field).and_then(Value::as_f64))
                            .collect();
                        summary.insert(format!("{name}.{field}"), aggregate_ci(&values));
                    }
                }
                Value::Object(summary)
            },
        )
        .collect()
}

fn aggregate_ci(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let ci = mean_ci(values, 0.05);
    let mut out = Map::new();
    out.insert("mean".into(), ci["mean"].clone());
    out.insert(
        "ci95".into(),
        ci["half_width"]
            .as_f64()
            .map_or(json!(0.0), |value| json!(value)),
    );
    out.insert("n".into(), ci["n"].clone());
    if !ci["multiplier"].is_null() {
        out.insert("multiplier".into(), ci["multiplier"].clone());
    }
    Value::Object(out)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: PathBuf::new(),
        seeds: 20,
        seed0: 20_260_818,
        steps: 48_000,
        n_mm: 16,
        n_entities: 24,
        arrival_rate: 0.15,
        window_steps: 1_200,
        epsilons: vec![1.0],
        epsilon_total: 40.0,
        protocols: [
            "plain_rfq",
            "plain_rfm",
            "plain_rfs",
            "qomm_rfq",
            "qomm_rfm",
            "qomm_rfs",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        disclosures: ["A_none", "B_threshold", "C_dp"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        layers: ["replay", "reactive"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        probes_per_window: 6,
        workers: 0,
        tape: None,
        tape_paths: Vec::new(),
        tape_step_ms: None,
        tape_step_blocks: 1,
        tape_max_rows: None,
        tape_entities: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        index += 1;
        macro_rules! scalar {
            ($field:ident, $type:ty) => {{
                let raw = args
                    .get(index)
                    .ok_or_else(|| format!("{flag} needs a value"))?;
                options.$field = raw.parse::<$type>().map_err(|_| format!("bad {flag}"))?;
                index += 1;
            }};
        }
        match flag.as_str() {
            "--out" => {
                options.out = args.get(index).ok_or("--out needs a value")?.into();
                index += 1;
            }
            "--seeds" => scalar!(seeds, usize),
            "--seed0" => scalar!(seed0, u64),
            "--steps" => scalar!(steps, usize),
            "--n-mm" => scalar!(n_mm, usize),
            "--n-entities" => scalar!(n_entities, usize),
            "--arrival-rate" => scalar!(arrival_rate, f64),
            "--window-steps" => scalar!(window_steps, usize),
            "--epsilon-total" => scalar!(epsilon_total, f64),
            "--probes-per-window" => scalar!(probes_per_window, usize),
            "--workers" => scalar!(workers, usize),
            "--tape-step-blocks" => scalar!(tape_step_blocks, usize),
            "--tape" => {
                let value = args.get(index).ok_or("--tape needs a value")?;
                if value != "bybit" && value != "uniswapx" {
                    return Err("--tape must be bybit or uniswapx".into());
                }
                options.tape = Some(value.clone());
                index += 1;
            }
            "--tape-step-ms" => {
                let raw = args.get(index).ok_or("--tape-step-ms needs a value")?;
                options.tape_step_ms = Some(raw.parse().map_err(|_| "bad --tape-step-ms")?);
                index += 1;
            }
            "--tape-max-rows" => {
                let raw = args.get(index).ok_or("--tape-max-rows needs a value")?;
                options.tape_max_rows = Some(raw.parse().map_err(|_| "bad --tape-max-rows")?);
                index += 1;
            }
            "--tape-entities" => {
                let raw = args.get(index).ok_or("--tape-entities needs a value")?;
                options.tape_entities = Some(raw.parse().map_err(|_| "bad --tape-entities")?);
                index += 1;
            }
            "--epsilons" => options.epsilons = take_numbers(&args, &mut index, flag)?,
            "--protocols" => options.protocols = take_strings(&args, &mut index, flag)?,
            "--disclosures" => options.disclosures = take_strings(&args, &mut index, flag)?,
            "--layers" => options.layers = take_strings(&args, &mut index, flag)?,
            "--tape-paths" => {
                options.tape_paths = take_strings(&args, &mut index, flag)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect()
            }
            "-h" | "--help" => {
                println!("run_sim_matrix --out PATH [matrix and tape options]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    if options.out.as_os_str().is_empty() {
        return Err("--out is required".into());
    }
    Ok(options)
}

fn take_strings(args: &[String], index: &mut usize, flag: &str) -> HarnessResult<Vec<String>> {
    let start = *index;
    while args
        .get(*index)
        .is_some_and(|value| !value.starts_with("--"))
    {
        *index += 1;
    }
    if *index == start {
        return Err(format!("{flag} needs at least one value").into());
    }
    Ok(args[start..*index].to_vec())
}

fn take_numbers(args: &[String], index: &mut usize, flag: &str) -> HarnessResult<Vec<f64>> {
    take_strings(args, index, flag)?
        .into_iter()
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| format!("bad value for {flag}: {value}").into())
        })
        .collect()
}
