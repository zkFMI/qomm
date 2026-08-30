use qomm_demo::mpc::MpcEngine;
use qomm_demo::room::{DemoConfig, Room};
use qomm_demo::web::DemoServer;

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
    let mut mpc_smoke = false;
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
                if !matches!(engine.as_str(), "sim" | "mpc") {
                    return Err("--engine must be sim or mpc".into());
                }
            }
            "--mp-spdz-root" => {
                mp_spdz_root = Some(take(&args, &mut index, "--mp-spdz-root")?.into())
            }
            "--mpc-smoke" => mpc_smoke = true,
            "-h" | "--help" => {
                println!("usage: qomm-demo [--host H] [--port P] [--makers N] [--nodes N] [--threshold T] [--round-seconds S] [--step-ms N] [--no-auto-rounds] [--no-input-check] [--seed N] [--engine sim|mpc] [--mp-spdz-root PATH] [--mpc-smoke]");
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
    }
    if mpc_smoke {
        if engine != "mpc" {
            return Err("--mpc-smoke requires --engine mpc".into());
        }
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

fn take<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}
