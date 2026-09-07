use qomm_harness::smallsample::mean_ci;
use qomm_harness::{rustc_version, write_pretty_json, HarnessResult};
use qomm_sim::disclosure::{Disclosure, DpDisclosure};
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::experiment::{build_probes, DpParams};
use qomm_sim::lab::LabMarket;
use qomm_sim::market::{build_market_makers, build_requests, ReferenceMarket, SimConfig};
use qomm_sim::tapes::{load_bybit, requests_from_tape, Entities, TapeMarket};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const ARMS: [&str; 3] = ["none", "dp_uncorrected", "dp_corrected"];
const METRICS: [&str; 2] = ["fill_rate", "mm_pnl_per_fill"];

struct Options {
    out: PathBuf,
    tape: Option<PathBuf>,
    tape_step_ms: u64,
    tape_entities: Option<usize>,
    seeds: usize,
    seed0: u64,
    steps: usize,
    window_steps: usize,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let seeds: Vec<u64> = (0..options.seeds)
        .map(|index| options.seed0 + index as u64)
        .collect();
    let mut arms = Map::new();
    arms.insert(
        "generated".into(),
        run_arm_set(
            None,
            &seeds,
            options.steps,
            options.window_steps,
            options.tape_step_ms,
            options.tape_entities,
            "reactive",
        )?,
    );
    if let Some(tape) = &options.tape {
        arms.insert(
            "tape".into(),
            run_arm_set(
                Some(tape),
                &seeds,
                2_400,
                60,
                options.tape_step_ms,
                options.tape_entities,
                "reactive",
            )?,
        );
    }
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "rustc": rustc_version(),
        "seeds": options.seeds,
        "arms": arms,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn mechanism(kind: &str, cfg: &SimConfig, dp: &DpParams) -> Disclosure {
    if kind == "none" {
        return Disclosure::None;
    }
    let old = kind == "dp_uncorrected";
    let mut disclosure = DpDisclosure::new(
        dp.epsilon_per_window,
        dp.request_cap,
        dp.volume_cap,
        cfg.n_entities,
        dp.epsilon_total,
        !old,
    );
    disclosure.signed_sensitivity_factor = if old { 2.0 } else { 1.0 };
    Disclosure::Dp(Box::new(disclosure))
}

fn run_arm_set(
    tape_path: Option<&Path>,
    seeds: &[u64],
    steps: usize,
    window_steps: usize,
    step_ms: u64,
    entities: Option<usize>,
    layer: &str,
) -> HarnessResult<Value> {
    let mut per_arm: BTreeMap<&str, BTreeMap<&str, Vec<f64>>> = ARMS
        .into_iter()
        .map(|arm| {
            (
                arm,
                METRICS
                    .into_iter()
                    .map(|metric| (metric, Vec::new()))
                    .collect(),
            )
        })
        .collect();
    let mut paired: BTreeMap<&str, BTreeMap<&str, Vec<f64>>> = ARMS[1..]
        .iter()
        .copied()
        .map(|arm| {
            (
                arm,
                METRICS
                    .into_iter()
                    .map(|metric| (metric, Vec::new()))
                    .collect(),
            )
        })
        .collect();
    let mut meta = json!({"source": "generated"});

    for seed in seeds {
        let base_cfg = SimConfig {
            steps,
            window_steps,
            seed: *seed,
            ..SimConfig::default()
        };
        let (cfg, market, requests, tape_meta) = if let Some(path) = tape_path {
            let text = fs::read_to_string(path)?;
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("tape.csv");
            let tape = load_bybit(&text, &base_cfg, name, Some(steps), Some(step_ms), None)?;
            let tape_market = TapeMarket::new(&base_cfg, &tape, 20, 60.0, 200, base_cfg.seed);
            let loaded = requests_from_tape(
                &base_cfg,
                &tape_market,
                &tape,
                entities.map_or(Entities::PerAddress, Entities::RoundRobin),
                1,
                base_cfg.seed + 2,
            );
            let mut values = Map::new();
            values.insert("source".into(), json!(tape.source));
            values.insert("entity_kind".into(), json!(loaded.entity_kind));
            for (key, value) in &loaded.meta {
                values.insert(key.clone(), json!(value));
            }
            for (key, value) in &tape.meta_text {
                values.insert(key.clone(), json!(value));
            }
            (
                loaded.cfg,
                LabMarket::Tape(Box::new(tape_market)),
                loaded.requests,
                Value::Object(values),
            )
        } else {
            let market = ReferenceMarket::new(&base_cfg, base_cfg.seed);
            let requests = build_requests(&base_cfg, &market, base_cfg.seed + 2);
            (
                base_cfg,
                LabMarket::Generated(market),
                requests,
                json!({"source": "generated"}),
            )
        };
        meta = tape_meta;
        let dp = DpParams::default();
        let makers = build_market_makers(&cfg, cfg.seed + 1);
        let probes = build_probes(&cfg, 6, 50);
        let mut summaries: BTreeMap<&str, BTreeMap<&str, f64>> = BTreeMap::new();
        for arm in ARMS {
            let mut disclosure = mechanism(arm, &cfg, &dp);
            let mut options = ArmOptions::new("plain_rfq", cfg.seed + 5);
            options.probes = probes.clone();
            options.reactive = layer == "reactive";
            let result = run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
            let values = [
                ("fill_rate", result.fill_rate()),
                (
                    "mm_pnl_per_fill",
                    result
                        .mm_pnl_per_fill()
                        .ok_or("an arm produced no fill, so its PnL difference is undefined")?,
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
            for metric in METRICS {
                per_arm
                    .get_mut(arm)
                    .unwrap()
                    .get_mut(metric)
                    .unwrap()
                    .push(values[metric]);
            }
            summaries.insert(arm, values);
        }
        for arm in &ARMS[1..] {
            for metric in METRICS {
                paired
                    .get_mut(arm)
                    .unwrap()
                    .get_mut(metric)
                    .unwrap()
                    .push(summaries[arm][metric] - summaries["none"][metric]);
            }
        }
    }

    let levels = per_arm
        .into_iter()
        .map(|(arm, metrics)| {
            (
                arm.to_string(),
                Value::Object(
                    metrics
                        .into_iter()
                        .map(|(metric, values)| (metric.to_string(), mean_ci(&values, 0.05)))
                        .collect(),
                ),
            )
        })
        .collect::<Map<_, _>>();
    let paired = paired
        .into_iter()
        .map(|(arm, metrics)| {
            (
                arm.to_string(),
                Value::Object(
                    metrics
                        .into_iter()
                        .map(|(metric, values)| (metric.to_string(), mean_ci(&values, 0.05)))
                        .collect(),
                ),
            )
        })
        .collect::<Map<_, _>>();
    Ok(json!({
        "meta": meta,
        "layer": layer,
        "levels": levels,
        "paired_against_no_disclosure": paired,
    }))
}

fn parse_args() -> HarnessResult<Options> {
    let root = qomm_harness::repo_root();
    let mut options = Options {
        out: root.join("artifacts/dp_effect.json"),
        tape: None,
        tape_step_ms: 1_000,
        tape_entities: None,
        seeds: 12,
        seed0: 20_260_818,
        steps: 48_000,
        window_steps: 1_200,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        index += 1;
        let value = || {
            args.get(index)
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--out" => options.out = PathBuf::from(value()?),
            "--tape" => options.tape = Some(PathBuf::from(value()?)),
            "--tape-step-ms" => options.tape_step_ms = value()?.parse()?,
            "--tape-entities" => options.tape_entities = Some(value()?.parse()?),
            "--seeds" => options.seeds = value()?.parse()?,
            "--seed0" => options.seed0 = value()?.parse()?,
            "--steps" => options.steps = value()?.parse()?,
            "--window-steps" => options.window_steps = value()?.parse()?,
            "-h" | "--help" => {
                println!("run_dp_effect [--out PATH] [--tape PATH] [simulation options]");
                return Err("help requested".into());
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
        index += 1;
    }
    if options.seeds == 0 {
        return Err("--seeds must be positive".into());
    }
    Ok(options)
}
