use qomm_harness::{fmean, parse_value, rustc_version, write_pretty_json, HarnessResult};
use qomm_sim::attackers;
use qomm_sim::engine::{run_arm, ArmOptions};
use qomm_sim::experiment::{build_probes, make_disclosure, DpParams};
use qomm_sim::market::{
    build_market_makers, build_requests, PricePath, ReferenceMarket, SimConfig,
};
use qomm_sim::tapes::{load_bybit, requests_from_tape, Entities, TapeMarket};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

struct Options {
    out: PathBuf,
    tape: Option<PathBuf>,
    tape_step_ms: u64,
    tape_entities: Option<usize>,
    rhos: Vec<f64>,
    protocols: Vec<String>,
    seeds: usize,
    seed0: u64,
    steps: usize,
    window_steps: usize,
}

enum Market {
    Generated(ReferenceMarket),
    Tape(Box<TapeMarket>),
}

impl PricePath for Market {
    fn mid(&self) -> &[i64] {
        match self {
            Self::Generated(value) => value.mid(),
            Self::Tape(value) => value.mid(),
        }
    }
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.rhos.is_empty() || options.protocols.is_empty() {
        return Err("--rhos and --protocols each expect at least one value".into());
    }
    let seeds = (0..options.seeds)
        .map(|offset| options.seed0 + offset as u64)
        .collect::<Vec<_>>();
    let generated = sweep(
        None,
        &options.protocols,
        &options.rhos,
        &seeds,
        options.steps,
        options.window_steps,
        options.tape_step_ms,
        options.tape_entities,
    )?;
    println!("generated market:");
    print_rows(&generated, true);

    let mut arms = Map::new();
    arms.insert("generated".into(), generated);
    if let Some(tape) = options.tape.as_deref() {
        let result = sweep(
            Some(tape),
            &options.protocols,
            &options.rhos,
            &seeds,
            2_400,
            60,
            options.tape_step_ms,
            options.tape_entities,
        )?;
        println!(
            "\n{}:",
            tape.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
        );
        print_rows(&result, false);
        arms.insert("tape".into(), result);
    }
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "rustc": rustc_version(),
        "rhos": options.rhos,
        "seeds": options.seeds,
        "arms": arms,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("\nwrote {}", options.out.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sweep(
    tape_path: Option<&Path>,
    protocols: &[String],
    rhos: &[f64],
    seeds: &[u64],
    steps: usize,
    window_steps: usize,
    step_ms: u64,
    entities: Option<usize>,
) -> HarnessResult<Value> {
    let mut cells = protocols
        .iter()
        .flat_map(|protocol| {
            rhos.iter()
                .map(move |&rho| (protocol.clone(), rho, Vec::new()))
        })
        .collect::<Vec<(String, f64, Vec<(f64, f64, usize)>)>>();
    let mut meta = json!({});
    for &seed in seeds {
        let base_cfg = SimConfig {
            steps,
            window_steps,
            seed,
            ..SimConfig::default()
        };
        let (cfg, market, requests, current_meta) =
            one_market(&base_cfg, tape_path, step_ms, entities)?;
        meta = current_meta;
        let makers = build_market_makers(&cfg, cfg.seed + 1);
        let probes = build_probes(&cfg, 6, 50);
        for protocol in protocols {
            let mut disclosure = make_disclosure("A_none", &cfg, &DpParams::default());
            let mut options = ArmOptions::new(protocol, cfg.seed + 5);
            options.probes = probes.clone();
            options.reactive = false;
            let result = run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
            for &rho in rhos {
                let report = attackers::passive_observer(&result, &cfg, rho, cfg.seed);
                if let Some(auc) = report.auc {
                    let covered = report
                        .extra
                        .get("entities_covered")
                        .copied()
                        .flatten()
                        .unwrap_or(0.0);
                    cells
                        .iter_mut()
                        .find(|(cell_protocol, cell_rho, _)| {
                            cell_protocol == protocol && *cell_rho == rho
                        })
                        .expect("all cells initialized")
                        .2
                        .push((auc, covered, report.n_examples));
                }
            }
        }
    }
    cells.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
    });
    let rows = cells
        .into_iter()
        .filter_map(|(protocol, rho, values)| {
            if values.is_empty() {
                return None;
            }
            let aucs = values.iter().map(|value| value.0).collect::<Vec<_>>();
            Some(json!({
                "protocol": protocol,
                "linkage_rho": rho,
                "auc_mean": fmean(&aucs),
                "auc_min": aucs.iter().copied().min_by(f64::total_cmp),
                "auc_max": aucs.iter().copied().max_by(f64::total_cmp),
                "entities_covered": fmean(&values.iter().map(|value| value.1).collect::<Vec<_>>()),
                "cells_scored": fmean(&values.iter().map(|value| value.2 as f64).collect::<Vec<_>>()),
                "seeds": aucs.len(),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({"meta": meta, "rows": rows}))
}

fn one_market(
    cfg: &SimConfig,
    tape_path: Option<&Path>,
    step_ms: u64,
    entities: Option<usize>,
) -> HarnessResult<(SimConfig, Market, Vec<qomm_sim::market::Request>, Value)> {
    let Some(path) = tape_path else {
        let market = ReferenceMarket::new(cfg, cfg.seed);
        let requests = build_requests(cfg, &market, cfg.seed + 2);
        return Ok((
            *cfg,
            Market::Generated(market),
            requests,
            json!({"source": "generated"}),
        ));
    };
    let text = fs::read_to_string(path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tape.csv");
    let tape = load_bybit(&text, cfg, name, Some(cfg.steps), Some(step_ms), None)?;
    let market = TapeMarket::new(cfg, &tape, 20, 60.0, 200, cfg.seed);
    let loaded = requests_from_tape(
        cfg,
        &market,
        &tape,
        entities.map_or(Entities::PerAddress, Entities::RoundRobin),
        1,
        cfg.seed + 2,
    );
    let mut meta = Map::new();
    meta.insert("source".into(), json!(tape.source));
    meta.insert("entity_kind".into(), json!(loaded.entity_kind));
    for (key, value) in loaded.meta {
        meta.insert(key, json!(value));
    }
    for (key, value) in tape.meta_text {
        meta.insert(key, json!(value));
    }
    Ok((
        loaded.cfg,
        Market::Tape(Box::new(market)),
        loaded.requests,
        Value::Object(meta),
    ))
}

fn print_rows(result: &Value, generated: bool) {
    for row in result["rows"].as_array().into_iter().flatten() {
        if matches!(row["protocol"].as_str(), Some("qomm_rfq" | "plain_rfq")) {
            if generated {
                println!(
                    "  {:10} rho={:<5} auc {:.4}  firms covered {:.1}",
                    row["protocol"].as_str().unwrap_or_default(),
                    qomm_harness::value_display(&row["linkage_rho"]),
                    row["auc_mean"].as_f64().unwrap_or(0.0),
                    row["entities_covered"].as_f64().unwrap_or(0.0),
                );
            } else {
                println!(
                    "  {:10} rho={:<5} auc {:.4}",
                    row["protocol"].as_str().unwrap_or_default(),
                    qomm_harness::value_display(&row["linkage_rho"]),
                    row["auc_mean"].as_f64().unwrap_or(0.0),
                );
            }
        }
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: qomm_harness::repo_root().join("artifacts/rho_sweep.json"),
        tape: None,
        tape_step_ms: 1_000,
        tape_entities: None,
        rhos: vec![0.0, 0.05, 0.12, 0.25, 0.5, 0.75, 1.0],
        protocols: vec![
            "qomm_rfq".into(),
            "plain_rfq".into(),
            "plain_rfm".into(),
            "plain_rfs".into(),
        ],
        seeds: 5,
        seed0: 20_260_818,
        steps: 48_000,
        window_steps: 1_200,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--tape" => options.tape = Some(PathBuf::from(value(&raw, &mut index, "--tape")?)),
            "--tape-step-ms" => {
                options.tape_step_ms =
                    parse_value(value(&raw, &mut index, "--tape-step-ms")?, "--tape-step-ms")?
            }
            "--tape-entities" => {
                options.tape_entities = Some(parse_value(
                    value(&raw, &mut index, "--tape-entities")?,
                    "--tape-entities",
                )?)
            }
            "--seeds" => {
                options.seeds = parse_value(value(&raw, &mut index, "--seeds")?, "--seeds")?
            }
            "--seed0" => {
                options.seed0 = parse_value(value(&raw, &mut index, "--seed0")?, "--seed0")?
            }
            "--steps" => {
                options.steps = parse_value(value(&raw, &mut index, "--steps")?, "--steps")?
            }
            "--window-steps" => {
                options.window_steps =
                    parse_value(value(&raw, &mut index, "--window-steps")?, "--window-steps")?
            }
            "--rhos" => {
                options.rhos = parse_many(&raw, &mut index, "--rhos")?;
                continue;
            }
            "--protocols" => {
                options.protocols = parse_many(&raw, &mut index, "--protocols")?;
                continue;
            }
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_many<T>(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let mut values = Vec::new();
    *index += 1;
    while *index < raw.len() && !raw[*index].to_string_lossy().starts_with("--") {
        values.push(parse_value(raw[*index].clone(), name)?);
        *index += 1;
    }
    Ok(values)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
