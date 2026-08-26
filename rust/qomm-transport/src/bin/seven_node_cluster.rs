//! Seven real mutually-authenticated listeners with one durable-node restart.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use curve25519_dalek::ristretto::RistrettoPoint;
use openssl::pkey::{PKey, Private};
use openssl::x509::X509;
use qomm_dsl::registry::CircuitRegistry;
use qomm_mpc::program::{build_program, ProgramConfig};
use qomm_proofs::kyb::{cohort_id, present, BusinessAttributes, KybIssuer, KybPresentation};
use qomm_transport::executor::{
    circuit_shape_digest, write_source_bound_executable, ProgramRegistry, RegisteredProgram,
};
use qomm_transport::key_management::{create_ca, issue_mutual_tls_certificate, write_tls_bundle};
use qomm_transport::node_service::{
    certificate_fingerprint, client_ssl_context, server_ssl_context, KybPolicy, NodeStore,
    Principal, RateLimitPolicy, ResidentNodeClient, ResidentNodeLocalClient, ResidentNodeServer,
};
use qomm_transport::wire::{share_request, Frame, FRAME_BYTES};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RULE: &str = "\
param mid[99000,101000] half[1,200] slope[0,16]\n\
input qty[1,1000]\n\
ask = mid + half + slope * qty\n";

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone)]
struct Bundle {
    key: PathBuf,
    cert: PathBuf,
    ca: PathBuf,
    der: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Transport {
    Tcp,
    Local,
}

enum ClusterClient {
    Tcp(ResidentNodeClient),
    Local(ResidentNodeLocalClient),
}

impl ClusterClient {
    fn call(&mut self, request: &Value) -> Result<Value, String> {
        match self {
            Self::Tcp(client) => client.call(request),
            Self::Local(client) => client.call(request),
        }
    }

    fn close(&mut self) {
        match self {
            Self::Tcp(client) => client.close(),
            Self::Local(client) => client.close(),
        }
    }
}

fn issue(
    root: &Path,
    name: &str,
    ca_key: &PKey<Private>,
    ca_cert: &X509,
) -> Result<Bundle, String> {
    let (key, cert) =
        issue_mutual_tls_certificate(ca_key, ca_cert, name, &[name], &["127.0.0.1"], 30)?;
    let der = cert.to_der().map_err(|error| error.to_string())?;
    let (key, certificate, ca) = write_tls_bundle(root.join(name), name, &key, &cert, ca_cert)?;
    Ok(Bundle {
        key,
        cert: certificate,
        ca,
        der,
    })
}

fn approved_registry(node: u16, root: &Path) -> Result<(Arc<ProgramRegistry>, String), String> {
    let config = ProgramConfig::default();
    let source = build_program(&config).map_err(|error| error.to_string())?;
    let shape = [
        config.n_mm as u64,
        config.n_parties as u64,
        u64::from(config.bit_length),
    ];
    let mut circuits = CircuitRegistry::default();
    circuits
        .approve("quote", RULE, &source, &shape)
        .map_err(|error| error.to_string())?;
    let executable = root.join(format!("node-{node}-approved-compute"));
    write_source_bound_executable(&executable, &source)?;
    let executable = fs::canonicalize(executable).map_err(|error| error.to_string())?;
    let shape_digest = circuit_shape_digest(&shape);
    let program = RegisteredProgram {
        shape_digest: shape_digest.clone(),
        argv: vec![
            executable.display().to_string(),
            "{node}".into(),
            "{slot}".into(),
            "{batch_digest}".into(),
        ],
        cwd: std::env::current_dir().map_err(|error| error.to_string())?,
        executable_sha256: hex::encode(Sha256::digest(
            fs::read(&executable).map_err(|error| error.to_string())?,
        )),
        timeout_seconds: 5.0,
    };
    let registry = ProgramRegistry::from_approved_mpc(node, program, &circuits, &config, &shape)?;
    Ok((Arc::new(registry), shape_digest))
}

#[allow(clippy::too_many_arguments)]
fn start_node(
    node: u16,
    bundle: &Bundle,
    store: Arc<NodeStore>,
    client_fingerprint: &str,
    coordinator_fingerprint: &str,
    frame_key: &[u8],
    scope_nullifier: &RistrettoPoint,
    presentation: &KybPresentation,
    kyb_policy: Arc<KybPolicy>,
    delay_ms: u64,
) -> Result<(ResidentNodeServer, Transport), String> {
    let mut principals = BTreeMap::new();
    principals.insert(
        client_fingerprint.to_string(),
        Principal::client(frame_key.to_vec(), scope_nullifier, presentation.clone())?,
    );
    principals.insert(
        coordinator_fingerprint.to_string(),
        Principal::coordinator(),
    );
    let program_root = store
        .path
        .parent()
        .ok_or_else(|| "node store has no parent directory".to_string())?;
    let (registry, _) = approved_registry(node, program_root)?;
    let mut server = ResidentNodeServer::new(
        node,
        "127.0.0.1",
        0,
        server_ssl_context(&bundle.cert, &bundle.key, &bundle.ca)?,
        principals,
        Some(kyb_policy),
        store,
        Some(registry),
        RateLimitPolicy::default(),
        Duration::from_secs(30),
        Duration::from_millis(delay_ms),
    )?;
    let transport = match server.start() {
        Ok(_) => Transport::Tcp,
        Err(error) if error.contains("Operation not permitted") => Transport::Local,
        Err(error) => return Err(error),
    };
    Ok((server, transport))
}

fn client_for(
    bundle: &Bundle,
    server: &ResidentNodeServer,
    transport: Transport,
) -> Result<ClusterClient, String> {
    let tls = client_ssl_context(&bundle.cert, &bundle.key, &bundle.ca)?;
    let server_name = format!("node-{}", server.node);
    Ok(match transport {
        Transport::Tcp => ClusterClient::Tcp(ResidentNodeClient::new(
            "127.0.0.1",
            server.port,
            tls,
            server_name,
            3,
        )),
        Transport::Local => ClusterClient::Local(server.local_client(tls, server_name, 3)),
    })
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn run(slots: u32) -> Result<bool, String> {
    if slots < 2 {
        return Err("at least two slots are required".into());
    }
    let root = std::env::temp_dir().join(format!(
        "qomm-seven-node-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let _root_guard = TempRoot(root.clone());
    let (ca_key, ca_cert) = create_ca("QOMM seven-node acceptance CA", 3650)?;
    let client_bundle = issue(&root, "client", &ca_key, &ca_cert)?;
    let coordinator_bundle = issue(&root, "coordinator", &ca_key, &ca_cert)?;
    let node_bundles = (0..7)
        .map(|node| issue(&root, &format!("node-{node}"), &ca_key, &ca_cert))
        .collect::<Result<Vec<_>, _>>()?;
    let client_fingerprint = certificate_fingerprint(&client_bundle.der);
    let coordinator_fingerprint = certificate_fingerprint(&coordinator_bundle.der);
    let frame_key: Vec<u8> = Sha256::digest(b"qomm-seven-node-frame-key-v1").to_vec();
    let venue_scope = b"qomm-seven-node/venue/orders";
    let mut issuer = KybIssuer::new(5, &mut rand_core::OsRng);
    let credential = issuer
        .enroll(
            "seven-node-client",
            BusinessAttributes {
                jurisdiction: "JP".into(),
                entity_type: "bank".into(),
                collateral_tier: 3,
            },
            &mut rand_core::OsRng,
        )
        .map_err(str::to_string)?;
    let cohort = cohort_id("JP", "bank", 2);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let kyb_registry = issuer
        .publish(&cohort, 1, now + 3_600)
        .map_err(str::to_string)?;
    let presentation = present(
        &credential,
        &kyb_registry,
        venue_scope,
        client_fingerprint.as_bytes(),
        &mut rand_core::OsRng,
    )
    .map_err(str::to_string)?;
    let scope_nullifier = credential.scope_nullifier(venue_scope);
    let kyb_policy = Arc::new(KybPolicy::new(
        venue_scope.to_vec(),
        cohort,
        kyb_registry,
        issuer.public_key(),
    )?);
    let delays = [8_u64, 13, 21, 34, 55, 70, 85];
    let mut stores = (0..7)
        .map(|node| NodeStore::open(root.join(format!("node-{node}.sqlite3"))).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;
    let started = (0..7)
        .map(|node| {
            start_node(
                node as u16,
                &node_bundles[node],
                Arc::clone(&stores[node]),
                &client_fingerprint,
                &coordinator_fingerprint,
                &frame_key,
                &scope_nullifier,
                &presentation,
                Arc::clone(&kyb_policy),
                delays[node],
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (mut servers, mut transports): (Vec<_>, Vec<_>) = started.into_iter().unzip();
    let mut clients = servers
        .iter()
        .zip(transports.iter().copied())
        .map(|(server, transport)| client_for(&client_bundle, server, transport))
        .collect::<Result<Vec<_>, _>>()?;

    let mut sizes = Vec::new();
    let mut last_requests = Vec::new();
    let mut last_responses = Vec::new();
    for slot in 0..slots {
        let real = slot == slots / 2;
        let values = if real {
            [3_u128, 100, 0, 42]
        } else {
            [0_u128; 4]
        };
        let payloads = share_request(&values, 7).map_err(|error| error.to_string())?;
        let mut requests = Vec::new();
        let mut responses = Vec::new();
        for node in 0..7 {
            let frame = Frame::new(slot, node, payloads[node], &frame_key)
                .map_err(|error| error.to_string())?;
            let raw = frame.encode();
            sizes.push((real, raw.len()));
            let request = json!({
                "version": 1,
                "request_id": format!("slot-{slot}-node-{node}"),
                "operation": "submit",
                "slot": slot,
                "frame": BASE64.encode(raw),
            });
            let response = clients[node].call(&request)?;
            if response.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(format!("node {node} refused slot {slot}: {response}"));
            }
            requests.push(request);
            responses.push(response);
        }
        last_requests = requests;
        last_responses = responses;
    }

    let node = 3_usize;
    clients[node].close();
    servers[node].stop();
    let frames_before = stores[node].frame_count()?;
    let requests_before = stores[node].request_count()?;
    // Drop every object that owns the original SQLite handle.  Recovery below
    // therefore comes from reopening the on-disk database, rather than from a
    // live Arc that happened to survive the listener restart.
    drop(clients.remove(node));
    drop(servers.remove(node));
    transports.remove(node);
    drop(stores.remove(node));
    let reopened_store = Arc::new(NodeStore::open(root.join(format!("node-{node}.sqlite3")))?);
    let (restarted, transport) = start_node(
        node as u16,
        &node_bundles[node],
        Arc::clone(&reopened_store),
        &client_fingerprint,
        &coordinator_fingerprint,
        &frame_key,
        &scope_nullifier,
        &presentation,
        Arc::clone(&kyb_policy),
        delays[node],
    )?;
    servers.insert(node, restarted);
    transports.insert(node, transport);
    stores.insert(node, reopened_store);
    clients.insert(
        node,
        client_for(&client_bundle, &servers[node], transports[node])?,
    );
    let recovered = clients[node].call(&last_requests[node])?;
    let recovery_works = recovered == last_responses[node]
        && stores[node].frame_count()? == frames_before
        && stores[node].request_count()? == requests_before;

    let mut changed = last_requests[node].clone();
    let mut payload = [0_u8; qomm_transport::wire::PAYLOAD_BYTES];
    payload[0] = 0x5a;
    let replacement =
        Frame::new(slots - 1, node, payload, &frame_key).map_err(|error| error.to_string())?;
    changed["frame"] = Value::String(BASE64.encode(replacement.encode()));
    let changed_reply = clients[node].call(&changed)?;
    let different_body_refused = changed_reply.get("ok").and_then(Value::as_bool) == Some(false)
        && changed_reply
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("reused"));

    let slot_counts = (0..slots)
        .map(|slot| {
            stores
                .iter()
                .map(|store| store.frames_for_slot(slot).map(|frames| frames.len()))
                .sum::<Result<usize, String>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let frames_constant = slot_counts.iter().all(|count| *count == 7);
    let cover_real_same_size = sizes.iter().all(|(_, size)| *size == FRAME_BYTES)
        && sizes.iter().any(|(real, _)| *real)
        && sizes.iter().any(|(real, _)| !*real);

    let (_, shape_digest) = approved_registry(0, &root)?;
    let mut coordinators = servers
        .iter()
        .zip(transports.iter().copied())
        .map(|(server, transport)| client_for(&coordinator_bundle, server, transport))
        .collect::<Result<Vec<_>, _>>()?;
    for (node, coordinator) in coordinators.iter_mut().enumerate() {
        let closed = coordinator.call(&json!({
            "version": 1,
            "request_id": "close-final",
            "operation": "close_slot",
            "slot": slots - 1,
        }))?;
        if closed.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!("node {node} slot close failed: {closed}"));
        }
        let response = coordinator.call(&json!({
            "version": 1,
            "request_id": "compute-final",
            "operation": "compute",
            "slot": slots - 1,
            "shape_digest": shape_digest,
        }))?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!("node {node} computation failed: {response}"));
        }
    }

    let transport_mode = if transports
        .iter()
        .all(|transport| matches!(transport, Transport::Tcp))
    {
        "tcp-mtls"
    } else {
        "socket-pair-mtls (bind unavailable)"
    };
    println!("transport: {transport_mode}");
    println!("frames per slot: {slot_counts:?}");
    println!("frames per slot constant: {}", yes_no(frames_constant));
    println!("recovery works: {}", yes_no(recovery_works));
    println!(
        "repeated id with a different body refused: {}",
        yes_no(different_body_refused)
    );
    println!(
        "cover and real frames the same size: {}",
        yes_no(cover_real_same_size)
    );

    for client in &mut clients {
        client.close();
    }
    for coordinator in &mut coordinators {
        coordinator.close();
    }
    for server in &mut servers {
        server.stop();
    }
    Ok(frames_constant && recovery_works && different_body_refused && cover_real_same_size)
}

fn main() {
    let mut slots = 5_u32;
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(position) = arguments.iter().position(|argument| argument == "--slots") {
        slots = arguments
            .get(position + 1)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("--slots requires an unsigned integer");
                std::process::exit(2);
            });
    }
    match run(slots) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("seven-node acceptance failed: {error}");
            std::process::exit(1);
        }
    }
}
