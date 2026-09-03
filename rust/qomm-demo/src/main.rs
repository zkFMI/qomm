use qomm_demo::asset_ids::traded_asset_id;
use qomm_demo::defmi_bootstrap::{bootstrap, DefmiBootstrapConfig};
use qomm_demo::distributed_mpc::{DistributedMpcEngine, TakerPretradeSignerConfig};
use qomm_demo::mpc::MpcEngine;
use qomm_demo::participant_client::ParticipantClient;
use qomm_demo::room::{DemoConfig, Room};
use qomm_demo::web::DemoServer;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("qomm-demo: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut host = "0.0.0.0".to_string();
    let mut port = 8_800_u16;
    let mut makers = 8_usize;
    let mut nodes = 9_usize;
    let mut threshold = 2_usize;
    let mut seed = 1_u64;
    let mut input_check = true;
    let mut engine = "sim".to_string();
    let mut mp_spdz_root = None::<String>;
    let mut mpc_endpoints = None::<String>;
    let mut mpc_timeout_seconds = 300_u64;
    let mut mpc_smoke = false;
    let mut defmi_endpoint = None::<String>;
    let mut maker_endpoints = None::<String>;
    let mut taker_endpoint = None::<String>;
    let mut mpc_operator_endpoints = None::<String>;
    let mut config = DemoConfig::default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--host" => host = take(&args, &mut index, "--host")?.into(),
            "--port" => {
                port = take(&args, &mut index, "--port")?
                    .parse()
                    .map_err(|_| "invalid --port")?
            }
            "--makers" => {
                makers = take(&args, &mut index, "--makers")?
                    .parse()
                    .map_err(|_| "invalid --makers")?
            }
            "--nodes" => {
                nodes = take(&args, &mut index, "--nodes")?
                    .parse()
                    .map_err(|_| "invalid --nodes")?
            }
            "--threshold" => {
                threshold = take(&args, &mut index, "--threshold")?
                    .parse()
                    .map_err(|_| "invalid --threshold")?
            }
            "--seed" => {
                seed = take(&args, &mut index, "--seed")?
                    .parse()
                    .map_err(|_| "invalid --seed")?
            }
            "--round-seconds" => {
                config.round_seconds = take(&args, &mut index, "--round-seconds")?
                    .parse()
                    .map_err(|_| "invalid --round-seconds")?
            }
            "--step-ms" => {
                config.step_ms = take(&args, &mut index, "--step-ms")?
                    .parse()
                    .map_err(|_| "invalid --step-ms")?
            }
            "--no-auto-rounds" => config.auto_rounds = false,
            "--no-input-check" => input_check = false,
            "--engine" => {
                engine = take(&args, &mut index, "--engine")?.into();
                if !matches!(engine.as_str(), "sim" | "mpc" | "distributed") {
                    return Err("--engine must be sim, mpc, or distributed".into());
                }
            }
            "--mp-spdz-root" => {
                mp_spdz_root = Some(take(&args, &mut index, "--mp-spdz-root")?.into())
            }
            "--mpc-endpoints" => {
                mpc_endpoints = Some(take(&args, &mut index, "--mpc-endpoints")?.into())
            }
            "--mpc-timeout-seconds" => {
                mpc_timeout_seconds = take(&args, &mut index, "--mpc-timeout-seconds")?
                    .parse()
                    .map_err(|_| "invalid --mpc-timeout-seconds")?;
            }
            "--defmi-endpoint" => {
                defmi_endpoint = Some(take(&args, &mut index, "--defmi-endpoint")?.into())
            }
            "--maker-endpoints" => {
                maker_endpoints = Some(take(&args, &mut index, "--maker-endpoints")?.into())
            }
            "--taker-endpoint" => {
                taker_endpoint = Some(take(&args, &mut index, "--taker-endpoint")?.into())
            }
            "--mpc-operator-endpoints" => {
                mpc_operator_endpoints =
                    Some(take(&args, &mut index, "--mpc-operator-endpoints")?.into())
            }
            "--mpc-smoke" => mpc_smoke = true,
            "-h" | "--help" => {
                println!("usage: qomm-demo [--host H] [--port P] [--makers N] [--nodes N] [--threshold T] [--round-seconds S] [--step-ms N] [--no-auto-rounds] [--no-input-check] [--seed N] [--engine sim|mpc|distributed] [--mp-spdz-root PATH] [--mpc-endpoints URL,...] [--mpc-timeout-seconds N] [--defmi-endpoint URL --maker-endpoints URL,... --taker-endpoint URL --mpc-operator-endpoints URL,...] [--mpc-smoke]");
                return Ok(());
            }
            other => return Err(format!("unknown argument {other}")),
        }
        index += 1;
    }
    if config.round_seconds <= 0.0 {
        return Err("round interval must be positive".into());
    }
    if nodes < 4 * threshold + 1 {
        eprintln!(
            "note: N={nodes}, T={threshold} cannot run the optional n >= 4T+1 Atlas identify-and-correct demonstration; MP-SPDZ malicious-Shamir remains the execution protocol"
        );
    }
    let mut room = Room::new(makers, nodes, threshold, input_check, seed)?;
    if engine == "mpc" {
        let root = mp_spdz_root
            .or_else(|| std::env::var("MP_SPDZ_ROOT").ok())
            .ok_or_else(|| {
                "--engine mpc requires --mp-spdz-root or MP_SPDZ_ROOT; no sim substitution was made"
                    .to_string()
            })?;
        let references = room
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let mpc = MpcEngine::new(root, nodes, threshold, makers, &references, 63, input_check)?;
        room.install_mpc_engine(mpc)?;
    } else if engine == "distributed" {
        let endpoints = mpc_endpoints
            .or_else(|| std::env::var("QOMM_MPC_ENDPOINTS").ok())
            .ok_or_else(|| {
                "--engine distributed requires --mpc-endpoints or QOMM_MPC_ENDPOINTS".to_string()
            })?
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let references = room
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let mut mpc = DistributedMpcEngine::new(
            &endpoints,
            threshold,
            makers,
            &references,
            63,
            input_check,
            Duration::from_secs(mpc_timeout_seconds),
        )?;
        if let Some(defmi_endpoint) = defmi_endpoint {
            let taker_endpoint = taker_endpoint
                .as_ref()
                .ok_or_else(|| "--defmi-endpoint requires --taker-endpoint".to_string())?
                .clone();
            let maker_endpoint_list = endpoint_list(
                maker_endpoints
                    .as_deref()
                    .ok_or_else(|| "--defmi-endpoint requires --maker-endpoints".to_string())?,
            );
            let program_digest: [u8; 32] = hex::decode(mpc.source_sha256())
                .map_err(|_| "distributed MPC source digest is not hexadecimal")?
                .try_into()
                .map_err(|_| "distributed MPC source digest is not 32 bytes")?;
            let report = bootstrap(DefmiBootstrapConfig {
                rpc_endpoint: defmi_endpoint.clone(),
                maker_endpoints: maker_endpoint_list.clone(),
                taker_endpoint: taker_endpoint.clone(),
                mpc_operator_endpoints: endpoint_list(
                    mpc_operator_endpoints.as_deref().ok_or_else(|| {
                        "--defmi-endpoint requires --mpc-operator-endpoints".to_string()
                    })?,
                ),
                program_digest,
            })?;
            room.configure_participant_capacities(&report.maker_capacities, report.taker_capacity)?;
            println!(
                "DeFMI participant domain {} ready on chain {}; {} new accepted registrations",
                report.domain_id,
                report.chain_id,
                report.registered_transactions.len()
            );
            let maker_participant_ids = report
                .maker_participant_ids
                .iter()
                .map(|value| {
                    hex::decode(value)
                        .map_err(|_| "Maker participant id is not hexadecimal")?
                        .try_into()
                        .map_err(|_| "Maker participant id is not 32 bytes".to_string())
                })
                .collect::<Result<Vec<[u8; 32]>, String>>()?;
            let taker_participant_id: [u8; 32] = hex::decode(&report.taker_participant_id)
                .map_err(|_| "Taker participant id is not hexadecimal")?
                .try_into()
                .map_err(|_| "Taker participant id is not 32 bytes".to_string())?;
            let taker_client = ParticipantClient::new(
                &taker_endpoint,
                Duration::from_secs(mpc_timeout_seconds.min(60)),
            )?;
            let taker_snapshot = taker_client.snapshot()?;
            if taker_snapshot.participant_id != taker_participant_id {
                return Err("canonical portfolio came from another Taker participant".into());
            }
            let asset_ids = (0..room.assets.len())
                .map(|asset| {
                    i64::try_from(asset)
                        .map(traded_asset_id)
                        .map_err(|_| "room asset index exceeds the signed range".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let canonical_portfolio =
                taker_client.canonical_portfolio(&taker_snapshot, &asset_ids)?;
            let canonical_inventory = asset_ids
                .iter()
                .map(|asset_id| {
                    canonical_portfolio
                        .inventory
                        .get(asset_id)
                        .copied()
                        .ok_or_else(|| "canonical Taker portfolio omitted a room asset".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            room.configure_taker_canonical_portfolio(
                canonical_portfolio.cash,
                &canonical_inventory,
            )?;
            mpc.bind_participants(&maker_participant_ids, taker_participant_id)?;
            let venue_id: [u8; 32] = hex::decode(&report.service_id)
                .map_err(|_| "DeFMI service id is not hexadecimal")?
                .try_into()
                .map_err(|_| "DeFMI service id is not 32 bytes".to_string())?;
            let defmi_id: [u8; 32] = hex::decode(&report.defmi_id)
                .map_err(|_| "DeFMI id is not hexadecimal")?
                .try_into()
                .map_err(|_| "DeFMI id is not 32 bytes".to_string())?;
            let taker_presentation = report
                .kyb
                .presentations
                .get(&taker_participant_id)
                .cloned()
                .ok_or_else(|| "DeFMI bootstrap omitted the Taker KYB proof".to_string())?;
            mpc.bind_taker_pretrade_signer(TakerPretradeSignerConfig {
                endpoint: taker_endpoint,
                taker_participant_id,
                venue_id,
                defmi_id,
                presentation: taker_presentation,
                registry: report.kyb.registry.clone(),
                trusted_issuer: report.kyb.trusted_issuer,
                identity_scope: report.kyb.scope.clone(),
                identity_context: report.kyb.context.clone(),
                required_cohort: report.kyb.required_cohort.clone(),
            })?;
            let maker_presentations = maker_participant_ids
                .iter()
                .map(|participant_id| {
                    report
                        .kyb
                        .presentations
                        .get(participant_id)
                        .cloned()
                        .ok_or_else(|| "DeFMI bootstrap omitted a Maker KYB proof".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            mpc.bind_maker_pretrade_signers(
                &maker_endpoint_list,
                &maker_participant_ids,
                &maker_presentations,
            )?;
            mpc.bind_defmi_market(&defmi_endpoint, &report.chain_id, venue_id, defmi_id)?;
            room.set_infrastructure(serde_json::json!({
                "mode": "docker",
                "defmi": true,
                "participant_registry": true,
                "mpc_committee": "3-of-7",
                "chain_id": report.chain_id,
                "defmi_id": report.defmi_id,
                "domain_id": report.domain_id,
                "service_id": report.service_id,
                "participant_count": report.participant_count,
                "operator_count": report.operator_count,
                "maker_participant_ids": report.maker_participant_ids,
                "taker_participant_id": report.taker_participant_id,
                "state_root": report.state_root,
                "portfolio_state_root": hex::encode(canonical_portfolio.state_root),
                "prior_taker_settlements": canonical_portfolio.consumed_settlements,
                "taker_portfolio_source": "defmi_final_claims_and_corporate_outbox",
                "governance": report.governance,
                "anonymous_kyb": true,
                "kyb_cohort": report.kyb.required_cohort,
                "raw_legal_entity_id_disclosed": false,
            }))?;
        }
        room.install_mpc_engine(mpc)?;
    }
    if mpc_smoke {
        if !matches!(engine.as_str(), "mpc" | "distributed") {
            return Err("--mpc-smoke requires --engine mpc or distributed".into());
        }
        room.prepare_taker_reservation()?;
        let result = room.run_round()?;
        println!(
            "mpc smoke: verified={} aborted={} winner={:?} price={:?} wall_ms={:.1}",
            result.verified == Some(true),
            result.aborted,
            result.outcome.winner,
            result.outcome.price,
            result.elapsed_ms,
        );
        return if result.verified == Some(true) && !result.aborted {
            Ok(())
        } else {
            Err(result.verified_detail)
        };
    }
    println!("QOMM demo --- {makers} makers, {nodes} nodes, threshold {threshold}, Rust {engine}");
    DemoServer::new(room, config).serve(&host, port)
}

fn endpoint_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn take<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}
