use qomm_demo::participant_node::{serve_participant_node, ParticipantNodeConfig, ParticipantRole};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

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
    let role = match value(&arguments, "--role")?.as_str() {
        "maker" => ParticipantRole::Maker,
        "taker" => ParticipantRole::Taker,
        "mpc-operator" => ParticipantRole::MpcOperator,
        _ => return Err("--role must be maker, taker, or mpc-operator".into()),
    };
    let label = value(&arguments, "--label")?;
    let participant_id: [u8; 32] = Sha256::new()
        .chain_update(b"QOMM:DEMO:PARTICIPANT:v1")
        .chain_update(role_string(role))
        .chain_update(label.as_bytes())
        .finalize()
        .into();
    serve_participant_node(ParticipantNodeConfig {
        role,
        label,
        participant_id,
        listen_host: value(&arguments, "--host")?,
        port: value(&arguments, "--port")?
            .parse()
            .map_err(|_| "--port must be an unsigned 16-bit integer".to_string())?,
        state_root: PathBuf::from(value(&arguments, "--state-root")?),
        initial_cash: value(&arguments, "--cash")?
            .parse()
            .map_err(|_| "--cash must be an unsigned integer".to_string())?,
        initial_inventory: value(&arguments, "--inventory")?
            .parse()
            .map_err(|_| "--inventory must be an unsigned integer".to_string())?,
        defmi_endpoint: value(&arguments, "--defmi-endpoint")?,
    })
}

fn role_string(role: ParticipantRole) -> &'static [u8] {
    match role {
        ParticipantRole::Maker => b"maker",
        ParticipantRole::Taker => b"taker",
        ParticipantRole::MpcOperator => b"mpc_operator",
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("qomm-participant-node failed: {error}");
        std::process::exit(1);
    }
}
