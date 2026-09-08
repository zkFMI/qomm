use qomm_demo::distributed_mpc::{serve_mpc_node, MpcNodeConfig};
use std::path::PathBuf;
use std::time::Duration;

fn value(arguments: &[String], name: &str) -> Result<String, String> {
    let position = arguments
        .iter()
        .position(|argument| argument == name)
        .ok_or_else(|| format!("missing {name}"))?;
    arguments
        .get(position + 1)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn run() -> Result<(), String> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let node: usize = value(&arguments, "--node")?
        .parse()
        .map_err(|_| "--node must be a non-negative integer".to_string())?;
    let api_port: u16 = value(&arguments, "--api-port")?
        .parse()
        .map_err(|_| "--api-port must be an unsigned 16-bit integer".to_string())?;
    let references = value(&arguments, "--references")?
        .split(',')
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| "--references must contain comma-separated integers".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let party_hosts = value(&arguments, "--party-hosts")?
        .split(',')
        .map(str::to_string)
        .collect::<Vec<_>>();
    serve_mpc_node(MpcNodeConfig {
        recipient_participants: std::env::var("QOMM_RECIPIENT_PARTICIPANTS")
            .map_err(|_| "QOMM_RECIPIENT_PARTICIPANTS must enroll the corporate services")?
            .split(',')
            .map(str::to_string)
            .collect(),
        node,
        n_parties: 7,
        threshold: 2,
        n_makers: value(&arguments, "--makers")?
            .parse()
            .map_err(|_| "--makers must be a non-negative integer".to_string())?,
        references,
        bit_length: value(&arguments, "--bit-length")?
            .parse()
            .map_err(|_| "--bit-length must be an unsigned integer".to_string())?,
        input_check: !arguments
            .iter()
            .any(|argument| argument == "--no-input-check"),
        listen_host: value(&arguments, "--listen-host")?,
        api_port,
        mp_spdz_root: PathBuf::from(value(&arguments, "--mp-spdz-root")?),
        state_root: PathBuf::from(value(&arguments, "--state-root")?),
        party_hosts,
        timeout: Duration::from_secs(300),
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qomm-mpc-node failed: {error}");
        std::process::exit(1);
    }
}
