//! Browser-free acceptance driver for the Docker demo network.
//!
//! The binary runs inside the `qomm-demo` Docker network and speaks only the
//! interfaces a browser or the gateway already use: the gateway WebSocket, the
//! participant module HTTP API, the MPC node inspection endpoints and the
//! DeFMI JSON-RPC.  It never signs, never touches an outbox file and never
//! executes anything locally; it submits, waits and records.
//!
//! Every command prints one JSON document to stdout and, with `--out FILE`,
//! writes the same document to disk so an outage test can be judged from the
//! recorded identifiers (request id, request digest, outbox sequence, DeFMI
//! hold id, receipts, node generations) rather than from a narrative.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use qomm_demo::distributed_mpc::remainder_note_id_of_execution;
use qomm_transport::resident_mpc::{combine_partial_commitments, InputSharing};
use rand_core::{OsRng, RngCore};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[path = "qomm_live_acceptance_report.rs"]
mod report;

const USAGE: &str = "usage: qomm-live-acceptance <command> [--key value ...]

commands
  snapshot   record the taker outbox, every hold's canonical DeFMI status, the
             seven nodes' health/maker-state/last receipt and the pool notes
  rfq        claim the taker seat over the gateway WebSocket, submit one real
             RFQ and wait for the round to finish (or to be queued/refused)
  policy     claim one maker seat and publish a policy change
  wait       poll one outbox entry (by --sequence or the first one after
             --after-sequence) until it reaches --until
  view       record one gateway view for --seat (default observer)
  judge      turn the files a scenario recorded (--dir, --scenario) into checks
  report     assemble all recorded scenarios into one independently re-derived
             acceptance record (--dir, --out; repeat --scenario/--require)
  force      pin --seat to manual mode (--manual 1) or release the pin (0)
  defmi-rpc  re-present a journaled DeFMI transition (--file) and record the answer
  pool       record one standing pool's canonical DeFMI state and current note (--id)
  pool-guard-probe  submit a journaled standing-pool allocation (--file) to the L1 with the current root and an over-large remainder, and record the canonical pool guard
  pool-cas-probe    re-sign a journaled allocation with fresh ids and a valid dev-committee approval over the current root, and record the standing-pool sequence/current-note compare-and-swap

common options
  --gateway URL   http://gateway:8801        --taker URL   http://taker:9200
  --defmi URL     http://defmi-network:9650/rpc
  --mpc URL,...   http://mpc-0:9100,...,http://mpc-6:9100
  --out FILE      also write the JSON document to FILE
  --timeout-secs N";

struct Args {
    command: String,
    options: BTreeMap<String, String>,
    repeated: BTreeMap<String, Vec<String>>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut raw = std::env::args().skip(1);
        let command = raw.next().ok_or_else(|| USAGE.to_string())?;
        let mut options = BTreeMap::new();
        let mut repeated = BTreeMap::<String, Vec<String>>::new();
        while let Some(key) = raw.next() {
            let name = key
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument {key}\n{USAGE}"))?
                .to_string();
            let value = raw
                .next()
                .ok_or_else(|| format!("--{name} needs a value\n{USAGE}"))?;
            repeated
                .entry(name.clone())
                .or_default()
                .push(value.clone());
            options.insert(name, value);
        }
        Ok(Self {
            command,
            options,
            repeated,
        })
    }

    fn text(&self, key: &str, default: &str) -> String {
        self.options
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    fn texts(&self, key: &str) -> Vec<String> {
        self.repeated.get(key).cloned().unwrap_or_default()
    }

    fn number(&self, key: &str, default: u64) -> Result<u64, String> {
        match self.options.get(key) {
            Some(value) => value
                .parse::<u64>()
                .map_err(|_| format!("--{key} must be an unsigned integer")),
            None => Ok(default),
        }
    }

    fn signed(&self, key: &str, default: i64) -> Result<i64, String> {
        match self.options.get(key) {
            Some(value) => value
                .parse::<i64>()
                .map_err(|_| format!("--{key} must be an integer")),
            None => Ok(default),
        }
    }

    fn gateway(&self) -> String {
        self.text("gateway", "http://gateway:8801")
    }

    fn taker(&self) -> String {
        self.text("taker", "http://taker:9200")
    }

    fn defmi(&self) -> String {
        self.text("defmi", "http://defmi-network:9650/rpc")
    }

    fn mpc(&self) -> Vec<String> {
        self.text(
            "mpc",
            "http://mpc-0:9100,http://mpc-1:9100,http://mpc-2:9100,http://mpc-3:9100,http://mpc-4:9100,http://mpc-5:9100,http://mpc-6:9100",
        )
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
    }

    fn timeout(&self) -> Result<Duration, String> {
        Ok(Duration::from_secs(self.number("timeout-secs", 900)?))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

/// `http://host:port[/path]` -> (`host:port`, `/path`).
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("{url} must start with http://"))?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || !authority.contains(':') {
        return Err(format!("{url} must name host:port"));
    }
    Ok((authority.to_string(), path.to_string()))
}

fn connect(authority: &str, timeout: Duration) -> Result<TcpStream, String> {
    let address = authority
        .to_socket_addrs()
        .map_err(|error| format!("{authority} did not resolve: {error}"))?
        .next()
        .ok_or_else(|| format!("{authority} did not resolve"))?;
    let stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("{authority} refused a connection: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|_| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

/// One HTTP/1.1 exchange with `Connection: close`.  Returns the status code
/// and the parsed JSON body (or the raw body as a JSON string).
fn http(
    method: &str,
    url: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let (authority, path) = split_url(url)?;
    let mut stream = connect(&authority, timeout)?;
    let payload = body
        .map(|value| serde_json::to_vec(value).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if body.is_some() {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        ));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&payload))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("{url}: write failed: {error}"))?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("{url}: read failed: {error}"))?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("{url}: response had no header terminator"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("{url}: response had no status line"))?;
    let mut body_bytes = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body_bytes = dechunk(&body_bytes);
    }
    let value = serde_json::from_slice::<Value>(&body_bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body_bytes).to_string()));
    Ok((status, value))
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(line_end) = raw[cursor..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_text = String::from_utf8_lossy(&raw[cursor..cursor + line_end]).to_string();
        let size = usize::from_str_radix(size_text.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        cursor += line_end + 2;
        if size == 0 || cursor + size > raw.len() {
            break;
        }
        out.extend_from_slice(&raw[cursor..cursor + size]);
        cursor += size + 2;
    }
    out
}

fn http_ok(
    method: &str,
    url: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<Value, String> {
    let (status, value) = http(method, url, body, timeout)?;
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        let detail = value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        Err(format!("{url} returned {status}: {detail}"))
    }
}

/// Captures a failure as data instead of aborting a snapshot: a stopped node
/// is a finding, not an error of the recorder.
fn captured(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => value,
        Err(error) => json!({ "error": error }),
    }
}

fn rpc(defmi: &str, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
    let value = http_ok(
        "POST",
        defmi,
        Some(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
        timeout,
    )?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string());
        return Err(format!("{method}: {message}"));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| format!("{method}: response had no result"))
}

fn digest_hex(entry: &Value) -> Option<String> {
    let bytes = entry
        .get("request_digest")?
        .as_array()?
        .iter()
        .map(|item| item.as_u64().and_then(|value| u8::try_from(value).ok()))
        .collect::<Option<Vec<u8>>>()?;
    Some(hex::encode(bytes))
}

fn state_name(entry: &Value) -> String {
    match entry.get("state") {
        Some(Value::Object(object)) => object.keys().next().cloned().unwrap_or_default(),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// Outbox entries with the digest rendered as hex next to the raw array.
fn outbox_entries(snapshot: &Value) -> Vec<Value> {
    snapshot
        .pointer("/mpc_outbox/entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let mut item = entry.clone();
                    if let (Some(object), Some(digest)) = (item.as_object_mut(), digest_hex(entry))
                    {
                        object.insert("request_digest_hex".into(), json!(digest));
                        object.insert("state_name".into(), json!(state_name(entry)));
                    }
                    item
                })
                .collect()
        })
        .unwrap_or_default()
}

fn reconcile(
    taker: &str,
    request_id: &str,
    digest: &str,
    timeout: Duration,
) -> Result<Value, String> {
    http_ok(
        "POST",
        &format!("{taker}/v1/outbox/reconcile"),
        Some(&json!({"request_id": request_id, "request_digest": digest})),
        timeout,
    )
}

fn hold_snapshot(defmi: &str, hold_id: &str, timeout: Duration) -> Value {
    captured(rpc(
        defmi,
        "defmivm.noteReservation",
        json!({"holdID": hold_id}),
        timeout,
    ))
}

struct WsClient {
    stream: TcpStream,
    buffer: Vec<u8>,
}

impl WsClient {
    fn connect(gateway: &str, query: &str, timeout: Duration) -> Result<Self, String> {
        let (authority, _) = split_url(gateway)?;
        let mut stream = connect(&authority, timeout)?;
        let mut nonce = [0_u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let request = format!(
            "GET /ws?{query} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            BASE64.encode(nonce)
        );
        stream
            .write_all(request.as_bytes())
            .and_then(|_| stream.flush())
            .map_err(|error| format!("websocket handshake write failed: {error}"))?;
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") && head.len() < 65_536 {
            stream
                .read_exact(&mut byte)
                .map_err(|error| format!("websocket handshake read failed: {error}"))?;
            head.push(byte[0]);
        }
        let status = String::from_utf8_lossy(&head)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        if !status.contains(" 101 ") {
            return Err(format!("gateway refused the websocket upgrade: {status}"));
        }
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            stream,
            buffer: Vec::new(),
        })
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mut key = [0_u8; 4];
        OsRng.fill_bytes(&mut key);
        let mut frame = vec![0x80 | opcode];
        let length = payload.len();
        if length < 126 {
            frame.push(0x80 | length as u8);
        } else if length < (1 << 16) {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
        frame.extend_from_slice(&key);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ key[index & 3]),
        );
        self.stream
            .write_all(&frame)
            .and_then(|_| self.stream.flush())
            .map_err(|error| format!("websocket write failed: {error}"))
    }

    fn send_json(&mut self, value: &Value) -> Result<(), String> {
        let text = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        self.send_frame(1, &text)
    }

    /// Returns the next complete server frame (opcode, payload) already
    /// buffered, without reading from the socket.
    fn buffered_frame(&mut self) -> Result<Option<(u8, Vec<u8>)>, String> {
        if self.buffer.len() < 2 {
            return Ok(None);
        }
        let opcode = self.buffer[0] & 0x0f;
        let masked = self.buffer[1] & 0x80 != 0;
        let mut length = usize::from(self.buffer[1] & 0x7f);
        let mut offset = 2;
        if length == 126 {
            if self.buffer.len() < 4 {
                return Ok(None);
            }
            length = usize::from(u16::from_be_bytes([self.buffer[2], self.buffer[3]]));
            offset = 4;
        } else if length == 127 {
            if self.buffer.len() < 10 {
                return Ok(None);
            }
            let mut raw = [0_u8; 8];
            raw.copy_from_slice(&self.buffer[2..10]);
            length = usize::try_from(u64::from_be_bytes(raw))
                .map_err(|_| "websocket frame length overflow".to_string())?;
            offset = 10;
        }
        if masked {
            return Err("gateway sent a masked frame".into());
        }
        if self.buffer.len() < offset + length {
            return Ok(None);
        }
        let payload = self.buffer[offset..offset + length].to_vec();
        self.buffer.drain(..offset + length);
        Ok(Some((opcode, payload)))
    }

    /// The next text message as JSON, or `None` when the deadline passes.
    fn next_json(&mut self, deadline: Instant) -> Result<Option<Value>, String> {
        let mut chunk = [0_u8; 65_536];
        loop {
            while let Some((opcode, payload)) = self.buffered_frame()? {
                match opcode {
                    1 => {
                        if let Ok(value) = serde_json::from_slice::<Value>(&payload) {
                            return Ok(Some(value));
                        }
                    }
                    8 => return Err("gateway closed the websocket".into()),
                    9 => self.send_frame(10, &payload)?,
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err("gateway hung up the websocket".into()),
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(error) => return Err(format!("websocket read failed: {error}")),
            }
        }
    }

    fn close(mut self) {
        let _ = self.send_frame(8, &[]);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn wait_for_view(client: &mut WsClient, deadline: Instant) -> Result<Value, String> {
    loop {
        match client.next_json(deadline)? {
            Some(message) if message.get("type").and_then(Value::as_str) == Some("view") => {
                return Ok(message)
            }
            Some(message) if message.get("type").and_then(Value::as_str) == Some("refused") => {
                return Err(format!(
                    "gateway refused: {}",
                    message.get("reason").and_then(Value::as_str).unwrap_or("")
                ))
            }
            Some(_) => {}
            None => return Err("no gateway view before the deadline".into()),
        }
    }
}

fn public_number(view: &Value) -> u64 {
    view.pointer("/public/number")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn trimmed_round(view: &Value) -> Value {
    let mut public = view.get("public").cloned().unwrap_or(Value::Null);
    if let Some(object) = public.as_object_mut() {
        // Node share strings are large and private to the node seats; the
        // acceptance record keeps the identifiers and receipts.
        object.remove("node_shares");
    }
    json!({
        "phase": view.get("phase"),
        "busy": view.get("busy"),
        "public": public,
        "taker": view.get("taker"),
        "notices": view.get("notices"),
        "infrastructure": view.get("infrastructure"),
    })
}

fn cmd_view(args: &Args) -> Result<Value, String> {
    let seat = args.text("seat", "observer");
    let timeout = args.timeout()?;
    let mut client = WsClient::connect(
        &args.gateway(),
        &format!("seat={seat}&label=acceptance"),
        Duration::from_secs(10),
    )?;
    let view = wait_for_view(
        &mut client,
        Instant::now() + timeout.min(Duration::from_secs(30)),
    );
    let _ = client.send_json(&json!({"type": "release"}));
    client.close();
    let view = view?;
    Ok(json!({
        "recorded_at": unix_now(),
        "seat": view.get("seat"),
        "view": trimmed_round(&view),
        "seats": view.get("seats"),
        "history": view.get("history"),
    }))
}

/// Pin a seat to manual mode (or release the pin).  With every Maker seat
/// pinned, the room's automatic quote refresh no longer rewrites policies at
/// the start of each round, so no pool re-registration --- which needs the
/// seven-node FROST committee --- is triggered by a submission while nodes
/// are down.  The pin is the same `force` message the observer page sends.
fn cmd_force(args: &Args) -> Result<Value, String> {
    let seat = args.text("seat", "");
    if seat.is_empty() {
        return Err("force needs --seat maker:N (or taker) and --manual 0|1".into());
    }
    let manual = args.number("manual", 1)? != 0;
    let mut client = WsClient::connect(
        &args.gateway(),
        "seat=observer&label=acceptance",
        Duration::from_secs(10),
    )?;
    let started = Instant::now();
    wait_for_view(&mut client, started + Duration::from_secs(30))?;
    client.send_json(&json!({"type": "force", "seat": seat, "manual": manual}))?;
    let view = wait_for_view(&mut client, started + Duration::from_secs(30));
    let _ = client.send_json(&json!({"type": "release"}));
    client.close();
    let view = view?;
    Ok(json!({
        "recorded_at": unix_now(),
        "seat": seat,
        "manual": manual,
        "seats": view.get("seats"),
    }))
}

fn cmd_policy(args: &Args) -> Result<Value, String> {
    let maker = args.number("maker", 0)?;
    let timeout = args.timeout()?;
    let mut values = Map::new();
    for field in [
        "asset",
        "ask_level",
        "spread",
        "slope",
        "invcoef",
        "inv",
        "maxqty",
        "expiry",
        "active",
        "use_ref",
    ] {
        if let Some(value) = args.options.get(field) {
            let number = value
                .parse::<i64>()
                .map_err(|_| format!("--{field} must be an integer"))?;
            values.insert(field.to_string(), json!(number));
        }
    }
    if values.is_empty() {
        return Err("policy needs at least one field such as --active 0 or --maxqty 200".into());
    }
    let mut client = WsClient::connect(
        &args.gateway(),
        &format!("seat=maker:{maker}&label=acceptance"),
        Duration::from_secs(10),
    )?;
    let started = Instant::now();
    let first = wait_for_view(&mut client, started + Duration::from_secs(30))?;
    if first.get("seat").and_then(Value::as_str) != Some(&format!("maker:{maker}")) {
        client.close();
        return Err(format!(
            "maker:{maker} seat was not granted: {}",
            first.get("seat").cloned().unwrap_or(Value::Null)
        ));
    }
    client.send_json(&json!({"type": "policy", "values": Value::Object(values.clone())}))?;
    let deadline = started + timeout;
    let mut outcome = json!({"status": "no_reply"});
    while let Some(message) = client.next_json(deadline)? {
        match message.get("type").and_then(Value::as_str) {
            Some("refused") => {
                outcome = json!({
                    "status": "refused",
                    "reason": message.get("reason"),
                });
                break;
            }
            Some("view") => {
                outcome = json!({
                    "status": "applied",
                    "maker": message.get("maker"),
                    "notices": message.get("notices"),
                    "infrastructure": message.get("infrastructure"),
                });
                break;
            }
            _ => {}
        }
    }
    let _ = client.send_json(&json!({"type": "release"}));
    client.close();
    Ok(json!({
        "recorded_at": unix_now(),
        "maker": maker,
        "values": values,
        "outcome": outcome,
        "elapsed_ms": started.elapsed().as_millis(),
    }))
}

fn cmd_rfq(args: &Args) -> Result<Value, String> {
    let timeout = args.timeout()?;
    let values = json!({
        "asset": args.signed("asset", 0)?,
        "direction": args.signed("direction", 0)?,
        "qty": args.signed("qty", 1)?,
        "limit_price": args.signed("limit", 0)?,
        "is_real": 1,
    });
    let submitted_at = unix_now();
    let started = Instant::now();
    let mut client = WsClient::connect(
        &args.gateway(),
        "seat=taker&label=acceptance",
        Duration::from_secs(10),
    )?;
    let first = wait_for_view(&mut client, started + Duration::from_secs(30))?;
    if first.get("seat").and_then(Value::as_str) != Some("taker") {
        client.close();
        return Err(format!(
            "taker seat was not granted: {}",
            first.get("seat").cloned().unwrap_or(Value::Null)
        ));
    }
    let baseline = public_number(&first);
    let before = trimmed_round(&first);
    let mut submit = values.clone();
    if !args.options.contains_key("limit") {
        // Keep the gateway's default limit for the asset/direction.
        submit
            .as_object_mut()
            .expect("object")
            .remove("limit_price");
    }
    client.send_json(&json!({"type": "submit", "values": submit}))?;
    let deadline = started + timeout;
    let mut status = "timeout".to_string();
    let mut refused = Value::Null;
    let mut latest = Value::Null;
    let mut seen_round = false;
    while let Some(message) = client.next_json(deadline)? {
        match message.get("type").and_then(Value::as_str) {
            Some("refused") => {
                refused = message.get("reason").cloned().unwrap_or(Value::Null);
                status = "refused".into();
                if !seen_round {
                    break;
                }
            }
            Some("view") if public_number(&message) > baseline => {
                seen_round = true;
                latest = message.clone();
                let busy = message.get("busy").and_then(Value::as_bool).unwrap_or(true);
                if !busy {
                    status = "finished".into();
                    break;
                }
            }
            _ => {}
        }
    }
    let _ = client.send_json(&json!({"type": "release"}));
    client.close();
    let public = latest.get("public").cloned().unwrap_or(Value::Null);
    let outbox = public
        .pointer("/engine_stats/corporate_outbox")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "submitted_at": submitted_at,
        "recorded_at": unix_now(),
        "elapsed_ms": started.elapsed().as_millis(),
        "status": status,
        "refused": refused,
        "submit": values,
        "baseline_round": baseline,
        "round_number": public.get("number"),
        "aborted": public.get("aborted"),
        "abort_code": public.get("abort_code"),
        "abort_reason": public.get("abort_reason"),
        "verified": public.get("verified"),
        "corporate_outbox": outbox,
        "engine_stats": public.get("engine_stats"),
        "settlement": public.get("settlement"),
        "before": before,
        "after": if latest.is_null() { Value::Null } else { trimmed_round(&latest) },
    }))
}

fn taker_snapshot(taker: &str, timeout: Duration) -> Result<Value, String> {
    http_ok("GET", &format!("{taker}/v1/snapshot"), None, timeout)
}

/// Re-present a journaled DeFMI transition (`{"method": ..., "params": ...}`
/// as the gateway wrote it under `QOMM_DEFMI_JOURNAL_DIR`) to the L1 and
/// record what it answers, following the transaction to its final status.
/// `--before-root current` rewrites the transition's `expectedBeforeRoot` to
/// the live state root first: the identical bytes are deduplicated by
/// transaction id and never reach the state machine, while the same content
/// under a fresh root is a new transaction that the pool-note
/// compare-and-swap must refuse.  A rejection is the expected outcome and is
/// recorded, not treated as a failure of this command.
fn cmd_defmi_rpc(args: &Args) -> Result<Value, String> {
    let path = args.text("file", "");
    if path.is_empty() {
        return Err("defmi-rpc needs --file <journaled transition>".into());
    }
    let record: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|error| format!("{path}: {error}"))?)
            .map_err(|error| format!("{path}: {error}"))?;
    let method = record
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "journaled transition has no method".to_string())?
        .to_string();
    let mut params = record.get("params").cloned().unwrap_or(Value::Null);
    let defmi = args.defmi();
    let root_before = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let before_root_mode = args.text("before-root", "journaled");
    if before_root_mode == "current" {
        let current = s(&root_before, "/stateRoot");
        if current.is_empty() {
            return Err("the live state root is unavailable for --before-root current".into());
        }
        if let Some(object) = params.as_object_mut() {
            object.insert("expectedBeforeRoot".into(), Value::String(current));
        }
    }
    let replay_method = args.text("method", &method);
    let started = Instant::now();
    let submission = rpc(
        &defmi,
        &replay_method,
        params.clone(),
        Duration::from_secs(60),
    );
    let tx_id = submission
        .as_ref()
        .ok()
        .and_then(|result| result.get("txID").and_then(Value::as_str))
        .map(str::to_string);
    // The RPC only admits the transaction; acceptance or rejection is the
    // state machine's verdict when a block carries it.
    let mut status = Value::Null;
    if let Some(tx_id) = &tx_id {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            status = captured(rpc(
                &defmi,
                "defmivm.txStatus",
                json!({"txID": tx_id}),
                Duration::from_secs(10),
            ));
            let state = s(&status, "/status");
            if !matches!(state.as_str(), "pending" | "processing" | "unknown" | "")
                || Instant::now() >= deadline
            {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
    }
    let root_after = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let original_height = args
        .options
        .get("original-height")
        .and_then(|value| value.parse::<u64>().ok());
    let final_status = s(&status, "/status");
    let applied_height = u(&status, "/height");
    Ok(json!({
        "recorded_at": unix_now(),
        "file": path,
        "method": replay_method,
        "before_root_mode": before_root_mode,
        "params_digest": hex::encode(Sha256::digest(serde_json::to_vec(&params).unwrap_or_default())),
        "params_summary": {
            "expected_before_root": params.get("expectedBeforeRoot").cloned(),
            "pool_id": params.pointer("/allocation/poolID").cloned(),
            "expected_pool_sequence": params.pointer("/allocation/expectedPoolSequence").cloned(),
            "previous_pool_note_id": params.pointer("/allocation/previousPoolNoteID").cloned(),
            "remainder_note_id": params.pointer("/allocation/remainderNote/noteID").cloned(),
        },
        "submitted": submission.is_ok(),
        "submission_error": submission.as_ref().err().cloned(),
        "tx_id": tx_id,
        "final_status": status,
        "original_height": original_height,
        // The identical transaction is the one the chain already holds: its
        // status names the original height and nothing is applied again.
        "deduplicated_by_transaction_id": original_height.is_some() && applied_height == original_height && final_status == "accepted",
        "rejected": final_status == "rejected",
        "rejection_reason": status.get("reason").cloned(),
        "state_root_before": root_before,
        "state_root_after": root_after,
        "state_root_unchanged": s(&root_before, "/stateRoot") == s(&root_after, "/stateRoot") && !s(&root_before, "/stateRoot").is_empty(),
        "elapsed_ms": started.elapsed().as_millis(),
    }))
}

/// The canonical state of one standing pool and its current note, whether or
/// not any node still binds it.
fn cmd_pool(args: &Args) -> Result<Value, String> {
    let pool_id = args.text("id", "");
    if pool_id.is_empty() {
        return Err("pool needs --id <pool id hex>".into());
    }
    let defmi = args.defmi();
    let pool = captured(rpc(
        &defmi,
        "defmivm.standingNotePool",
        json!({"poolID": pool_id}),
        Duration::from_secs(30),
    ));
    let note = pool
        .get("currentPoolNoteID")
        .and_then(Value::as_str)
        .map(|note_id| {
            captured(rpc(
                &defmi,
                "defmivm.note",
                json!({"noteID": note_id}),
                Duration::from_secs(30),
            ))
        })
        .unwrap_or(Value::Null);
    Ok(json!({
        "recorded_at": unix_now(),
        "pool_id": pool_id,
        "pool": pool,
        "current_pool_note": note,
        "state_root": captured(rpc(&defmi, "defmivm.stateRoot", json!({}), Duration::from_secs(10))),
    }))
}

/// Read a 32-byte hex field from a JSON object.
fn hex32(value: &Value, key: &str) -> Result<[u8; 32], String> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is missing or not a string"))?;
    hex::decode(text)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| format!("{key} is not a 32-byte hex string"))
}

/// A Ristretto point from a hex commitment.
fn point(value: &Value, key: &str) -> Result<curve25519_dalek::ristretto::RistrettoPoint, String> {
    curve25519_dalek::ristretto::CompressedRistretto(hex32(value, key)?)
        .decompress()
        .ok_or_else(|| format!("{key} is not a canonical Ristretto point"))
}

/// Rebuild a note-output DTO (camelCase, as the gateway journals it) with a
/// replaced value commitment, recomputing its note id through the canonical
/// `NoteOutput::derived_id` so the note stays internally consistent.
fn reencode_note_with_commitment(dto: &Value, commitment: [u8; 32]) -> Result<Value, String> {
    let mut note = qomm_defmi::note_chain::NoteOutput {
        note_id: [0u8; 32],
        asset_id: hex32(dto, "assetID")?,
        one_time: hex32(dto, "oneTime")?,
        value_commitment: commitment,
        ephemeral: hex32(dto, "ephemeral")?,
        masked_value: hex32(dto, "maskedValue")?,
        masked_blinding: hex32(dto, "maskedBlinding")?,
        lock_id: hex32(dto, "lockID")?,
    };
    note.note_id = note.derived_id()?;
    Ok(json!({
        "noteID": hex::encode(note.note_id),
        "assetID": hex::encode(note.asset_id),
        "oneTime": hex::encode(note.one_time),
        "valueCommitment": hex::encode(note.value_commitment),
        "ephemeral": hex::encode(note.ephemeral),
        "maskedValue": hex::encode(note.masked_value),
        "maskedBlinding": hex::encode(note.masked_blinding),
        "lockID": hex::encode(note.lock_id),
    }))
}

/// Submit a `defmivm.issue*` transition to the L1 and follow it to a terminal
/// status.  Shared by the replay and pool-guard probes.
fn submit_and_follow(
    defmi: &str,
    method: &str,
    params: &Value,
) -> (bool, Option<Value>, Option<String>, Value) {
    let submission = rpc(defmi, method, params.clone(), Duration::from_secs(60));
    let tx_id = submission
        .as_ref()
        .ok()
        .and_then(|result| result.get("txID").and_then(Value::as_str))
        .map(str::to_string);
    let mut status = Value::Null;
    if let Some(tx_id) = &tx_id {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            status = captured(rpc(
                defmi,
                "defmivm.txStatus",
                json!({"txID": tx_id}),
                Duration::from_secs(10),
            ));
            let state = s(&status, "/status");
            if !matches!(state.as_str(), "pending" | "processing" | "unknown" | "")
                || Instant::now() >= deadline
            {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
    }
    (
        submission.is_ok(),
        submission.as_ref().err().cloned().map(Value::String),
        tx_id,
        status,
    )
}

/// Present a journaled standing-pool allocation to the L1 as a first-class
/// `defmivm.issueStandingNotePoolAllocation` transition, built against the
/// live global state root, but with the winning Maker's remainder commitment
/// inflated so the allocation would take more from the pool than the parent
/// note holds.  This is the transaction a request that exceeded the pool
/// would carry; the honest gateway never sends it, so it is constructed here
/// from the gateway's own DeFMI journal.  The canonical VM's standing-pool
/// allocation body (`does not conserve its parent commitment`) rejects it in
/// consensus execution, before the k-of-n approval or the state root are
/// checked and before any node signature.  Nothing is forged: the approval,
/// proofs and committee signature are the gateway's own; only the remainder
/// commitment is made too large, which is exactly what the guard exists to
/// catch.  A rejection is the expected outcome and is recorded, not an error.
fn cmd_pool_guard_probe(args: &Args) -> Result<Value, String> {
    let path = args.text("file", "");
    if path.is_empty() {
        return Err(
            "pool-guard-probe needs --file <journaled allocation-bearing transition>".into(),
        );
    }
    let record: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|error| format!("{path}: {error}"))?)
            .map_err(|error| format!("{path}: {error}"))?;
    let params = record.get("params").cloned().unwrap_or(Value::Null);
    // The gateway journals the allocation inside a product-settlement
    // transition; the standalone allocation transition carries the same four
    // members under un-prefixed keys.
    let allocation = params
        .get("allocation")
        .cloned()
        .ok_or_else(|| "journaled transition has no allocation".to_string())?;
    let transition = params
        .get("allocationTransition")
        .or_else(|| params.get("transition"))
        .cloned()
        .ok_or_else(|| "journaled transition has no allocation transition".to_string())?;
    let authorization = params
        .get("allocationAuthorization")
        .or_else(|| params.get("authorization"))
        .cloned()
        .ok_or_else(|| "journaled transition has no allocation authorization".to_string())?;
    let approval = params
        .get("allocationApproval")
        .or_else(|| params.get("approval"))
        .cloned()
        .ok_or_else(|| "journaled transition has no allocation approval".to_string())?;
    let defmi = args.defmi();
    let root_before = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let current_root = s(&root_before, "/stateRoot");
    if current_root.is_empty() {
        return Err("the live state root is unavailable".into());
    }
    // Inflate the remainder: new remainder = remainder + escrow, so the parent
    // no longer equals child (escrow) + remainder.  Both operands are the
    // gateway's own canonical commitments.
    let escrow_point = point(
        allocation
            .get("escrowNote")
            .ok_or("allocation has no escrow note")?,
        "valueCommitment",
    )?;
    let remainder_dto = allocation
        .get("remainderNote")
        .cloned()
        .ok_or("allocation has no remainder note")?;
    let remainder_point = point(&remainder_dto, "valueCommitment")?;
    let inflated = (remainder_point + escrow_point).compress().to_bytes();
    let mut mutated_allocation = allocation.clone();
    mutated_allocation
        .as_object_mut()
        .expect("allocation object")
        .insert(
            "remainderNote".into(),
            reencode_note_with_commitment(&remainder_dto, inflated)?,
        );
    let submit_params = json!({
        "transition": transition,
        "authorization": authorization,
        "allocation": mutated_allocation,
        "approval": approval,
        "expectedBeforeRoot": current_root,
    });
    let (submitted, submission_error, tx_id, status) = submit_and_follow(
        &defmi,
        "defmivm.issueStandingNotePoolAllocation",
        &submit_params,
    );
    let root_after = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let reason = status
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            submission_error
                .as_ref()
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let final_state = s(&status, "/status");
    let rejected = final_state == "rejected" || (!submitted && submission_error.is_some());
    Ok(json!({
        "recorded_at": unix_now(),
        "file": path,
        "mode": "over-remainder",
        "method": "defmivm.issueStandingNotePoolAllocation",
        "expected_before_root": current_root,
        "expected_before_root_is_live": true,
        "pool_id": allocation.get("poolID"),
        "expected_pool_sequence": allocation.get("expectedPoolSequence"),
        "previous_pool_note_id": allocation.get("previousPoolNoteID"),
        "original_remainder_note_id": remainder_dto.get("noteID"),
        "inflated_remainder_note_id": mutated_allocation.pointer("/remainderNote/noteID"),
        "submitted": submitted,
        "submission_error": submission_error,
        "tx_id": tx_id,
        "final_status": status,
        "rejected": rejected,
        "rejection_reason": reason,
        "rejected_by_pool_conservation_guard": reason.contains("does not conserve its parent commitment"),
        "state_root_before": root_before,
        "state_root_after": root_after,
        "state_root_unchanged": current_root == s(&root_after, "/stateRoot"),
    }))
}

/// One SHA-256 digest over a set of byte strings, used to mint fresh
/// operation/hold/nullifier identifiers for the compare-and-swap probe.
fn probe_digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// The seven-node development governance committee the demo DeFMI VM is
/// bootstrapped with (`development_committee` in defmi_bootstrap.rs): node-i
/// signs with the ed25519 key derived from sha256("key:{i}"), a 3-of-7
/// threshold at epoch 1 over the given chain-id domain.  A dev/test path can
/// re-sign an approval with a genuine quorum here; this is the same committee
/// the running VM verifies against, so no signature or approval check is
/// disabled and no production guard is weakened.
fn development_quorum(
    domain: &str,
) -> Result<
    (
        qomm_defmi::facility::QuorumAuthorizer,
        BTreeMap<String, qomm_defmi::governance::GovernanceSigner>,
    ),
    String,
> {
    let keys = qomm_defmi::governance::public_development_keys()?;
    let nodes = keys
        .iter()
        .map(|(name, key)| (name.clone(), key.verifying_key()))
        .collect::<BTreeMap<String, zkfmi_crypto::key::KeyRecord>>();
    let authorizer = qomm_defmi::facility::QuorumAuthorizer::new(nodes, 3, 1, domain)?;
    Ok((authorizer, keys))
}

fn h32(value: &Value, key: &str) -> Result<[u8; 32], String> {
    hex32(value, key)
}
fn hu64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key} is not an unsigned integer"))
}

fn parse_transition(dto: &Value) -> Result<qomm_defmi::facility::CreditFacilityTransition, String> {
    Ok(qomm_defmi::facility::CreditFacilityTransition {
        operation_id: h32(dto, "operationID")?,
        facility_id: h32(dto, "facilityID")?,
        hold_id: h32(dto, "holdID")?,
        kind: qomm_defmi::facility::CreditTransitionKind::Hold,
        query_commitment: h32(dto, "queryCommitment")?,
        amount_commitment: h32(dto, "amountCommitment")?,
        consumed_commitment: h32(dto, "consumedCommitment")?,
        refund_commitment: h32(dto, "refundCommitment")?,
        before_available_commitment: h32(dto, "beforeAvailableCommitment")?,
        after_available_commitment: h32(dto, "afterAvailableCommitment")?,
        before_held_commitment: h32(dto, "beforeHeldCommitment")?,
        after_held_commitment: h32(dto, "afterHeldCommitment")?,
        before_outstanding_commitment: h32(dto, "beforeOutstandingCommitment")?,
        after_outstanding_commitment: h32(dto, "afterOutstandingCommitment")?,
        before_sequence: hu64(dto, "beforeSequence")?,
        expires_at: hu64(dto, "expiresAt")?,
        settlement_digest: h32(dto, "settlementDigest")?,
        relation_proof_digest: h32(dto, "relationProofDigest")?,
    })
}

fn transition_json(t: &qomm_defmi::facility::CreditFacilityTransition) -> Value {
    json!({
        "operationID": hex::encode(t.operation_id),
        "facilityID": hex::encode(t.facility_id),
        "holdID": hex::encode(t.hold_id),
        "kind": "hold",
        "queryCommitment": hex::encode(t.query_commitment),
        "amountCommitment": hex::encode(t.amount_commitment),
        "consumedCommitment": hex::encode(t.consumed_commitment),
        "refundCommitment": hex::encode(t.refund_commitment),
        "beforeAvailableCommitment": hex::encode(t.before_available_commitment),
        "afterAvailableCommitment": hex::encode(t.after_available_commitment),
        "beforeHeldCommitment": hex::encode(t.before_held_commitment),
        "afterHeldCommitment": hex::encode(t.after_held_commitment),
        "beforeOutstandingCommitment": hex::encode(t.before_outstanding_commitment),
        "afterOutstandingCommitment": hex::encode(t.after_outstanding_commitment),
        "beforeSequence": t.before_sequence,
        "expiresAt": t.expires_at,
        "settlementDigest": hex::encode(t.settlement_digest),
        "relationProofDigest": hex::encode(t.relation_proof_digest),
    })
}

fn parse_authorization(
    dto: &Value,
) -> Result<qomm_defmi::facility::ReservationAuthorization, String> {
    Ok(qomm_defmi::facility::ReservationAuthorization {
        role: qomm_defmi::facility::ReservationRole::Maker,
        entity_commitment: h32(dto, "entityCommitment")?,
        asset_id: h32(dto, "assetID")?,
        direction: dto
            .get("direction")
            .and_then(Value::as_u64)
            .ok_or("direction is not an integer")? as u8,
        authorization_digest: h32(dto, "authorizationDigest")?,
        mandate_digest: h32(dto, "mandateDigest")?,
        typed_reserve_digest: h32(dto, "typedReserveDigest")?,
        reserve_nullifier: h32(dto, "reserveNullifier")?,
        asset_link_proof_digest: h32(dto, "assetLinkProofDigest")?,
        limit_price_commitment: h32(dto, "limitPriceCommitment")?,
        escrow_digest: h32(dto, "escrowDigest")?,
        rfq_nullifier: h32(dto, "rfqNullifier")?,
        policy_version: hu64(dto, "policyVersion")?,
        admission_ticket_id: h32(dto, "admissionTicketID")?,
        admission_slot: hu64(dto, "admissionSlot")?,
        admission_receipt_digest: h32(dto, "admissionReceiptDigest")?,
        admission_epoch: hu64(dto, "admissionEpoch")?,
        admission_sequence: hu64(dto, "admissionSequence")?,
        admission_batch_id: h32(dto, "admissionBatchID")?,
    })
}

fn authorization_json(a: &qomm_defmi::facility::ReservationAuthorization) -> Value {
    json!({
        "role": "maker",
        "entityCommitment": hex::encode(a.entity_commitment),
        "assetID": hex::encode(a.asset_id),
        "direction": a.direction,
        "authorizationDigest": hex::encode(a.authorization_digest),
        "mandateDigest": hex::encode(a.mandate_digest),
        "typedReserveDigest": hex::encode(a.typed_reserve_digest),
        "reserveNullifier": hex::encode(a.reserve_nullifier),
        "assetLinkProofDigest": hex::encode(a.asset_link_proof_digest),
        "limitPriceCommitment": hex::encode(a.limit_price_commitment),
        "escrowDigest": hex::encode(a.escrow_digest),
        "rfqNullifier": hex::encode(a.rfq_nullifier),
        "policyVersion": a.policy_version,
        "admissionTicketID": hex::encode(a.admission_ticket_id),
        "admissionSlot": a.admission_slot,
        "admissionReceiptDigest": hex::encode(a.admission_receipt_digest),
        "admissionEpoch": a.admission_epoch,
        "admissionSequence": a.admission_sequence,
        "admissionBatchID": hex::encode(a.admission_batch_id),
    })
}

fn parse_note(dto: &Value) -> Result<qomm_defmi::note_chain::NoteOutput, String> {
    Ok(qomm_defmi::note_chain::NoteOutput {
        note_id: h32(dto, "noteID")?,
        asset_id: h32(dto, "assetID")?,
        one_time: h32(dto, "oneTime")?,
        value_commitment: h32(dto, "valueCommitment")?,
        ephemeral: h32(dto, "ephemeral")?,
        masked_value: h32(dto, "maskedValue")?,
        masked_blinding: h32(dto, "maskedBlinding")?,
        lock_id: h32(dto, "lockID")?,
    })
}

fn note_json(n: &qomm_defmi::note_chain::NoteOutput) -> Value {
    json!({
        "noteID": hex::encode(n.note_id),
        "assetID": hex::encode(n.asset_id),
        "oneTime": hex::encode(n.one_time),
        "valueCommitment": hex::encode(n.value_commitment),
        "ephemeral": hex::encode(n.ephemeral),
        "maskedValue": hex::encode(n.masked_value),
        "maskedBlinding": hex::encode(n.masked_blinding),
        "lockID": hex::encode(n.lock_id),
    })
}

fn parse_allocation(
    dto: &Value,
) -> Result<qomm_defmi::note_chain::StandingNotePoolAllocation, String> {
    Ok(qomm_defmi::note_chain::StandingNotePoolAllocation {
        pq_authorization: Some(
            serde_json::from_value(
                dto.get("pqAuthorization")
                    .ok_or("allocation lacks PQ authorization")?
                    .clone(),
            )
            .map_err(|error| format!("allocation PQ authorization is malformed: {error}"))?,
        ),
        pool_id: h32(dto, "poolID")?,
        delegation_digest: h32(dto, "delegationDigest")?,
        committee_epoch: hu64(dto, "committeeEpoch")?,
        expected_pool_sequence: hu64(dto, "expectedPoolSequence")?,
        previous_pool_note_id: h32(dto, "previousPoolNoteID")?,
        previous_amount_commitment: h32(dto, "previousAmountCommitment")?,
        escrow_note: parse_note(
            dto.get("escrowNote")
                .ok_or("allocation has no escrowNote")?,
        )?,
        remainder_note: parse_note(
            dto.get("remainderNote")
                .ok_or("allocation has no remainderNote")?,
        )?,
        proof_job_id: h32(dto, "proofJobID")?,
        quote_proof_digest: h32(dto, "quoteProofDigest")?,
        dvp_proof_digest: h32(dto, "dvpProofDigest")?,
        remainder_range_proof_digest: h32(dto, "remainderRangeProofDigest")?,
        committee_signature: hex::decode(
            dto.get("committeeSignature")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .map_err(|_| "committeeSignature is not hexadecimal".to_string())?,
    })
}

fn allocation_json(a: &qomm_defmi::note_chain::StandingNotePoolAllocation) -> Value {
    json!({
        "poolID": hex::encode(a.pool_id),
        "delegationDigest": hex::encode(a.delegation_digest),
        "committeeEpoch": a.committee_epoch,
        "expectedPoolSequence": a.expected_pool_sequence,
        "previousPoolNoteID": hex::encode(a.previous_pool_note_id),
        "previousAmountCommitment": hex::encode(a.previous_amount_commitment),
        "escrowNote": note_json(&a.escrow_note),
        "remainderNote": note_json(&a.remainder_note),
        "proofJobID": hex::encode(a.proof_job_id),
        "quoteProofDigest": hex::encode(a.quote_proof_digest),
        "dvpProofDigest": hex::encode(a.dvp_proof_digest),
        "remainderRangeProofDigest": hex::encode(a.remainder_range_proof_digest),
        "committeeSignature": hex::encode(&a.committee_signature),
        "pqAuthorization": a.pq_authorization,
    })
}

fn approval_json(approval: &qomm_defmi::facility::QuorumApproval) -> Value {
    json!({
        "statement": hex::encode(approval.statement),
        "signerEpoch": approval.signer_epoch,
        "suite": approval.suite,
        "committeeDigest": hex::encode(approval.committee_digest),
        "domain": approval.domain,
        "beforeRoot": hex::encode(approval.before_root),
        "approvals": approval.approvals.iter().map(|signed| json!({
            "nodeID": signed.node_id,
            "signature": hex::encode(&signed.signature),
        })).collect::<Vec<_>>(),
    })
}

/// Query one standing pool's canonical sequence and current note.
fn pool_state(defmi: &str, pool_id: &str) -> Value {
    let pool = captured(rpc(
        defmi,
        "defmivm.standingNotePool",
        json!({"poolID": pool_id}),
        Duration::from_secs(30),
    ));
    json!({"sequence": pool.get("sequence"), "currentPoolNoteID": pool.get("currentPoolNoteID"), "status": pool.get("status")})
}

/// Directly exercise the canonical standing-pool sequence / current-note
/// compare-and-swap in the DeFMI VM.  The gateway's journaled, accepted
/// allocation is rebuilt with fresh operation/hold/nullifier identifiers (so
/// the already-used guard cannot pre-empt it) and a freshly escrowed note
/// locked to the new hold, keeping the parent commitment conserved, then
/// re-approved by a genuine 3-of-7 development-committee quorum over the LIVE
/// state root.  Three transitions are submitted to `issueStandingNotePoolAllocation`:
/// The cas case names the pool sequence/current-note the winning fill already
/// advanced past, so the VM must reject it at the compare-and-swap
/// (`standing allocation is stale or outside its Maker mandate`), which by the
/// VM's own check order can only be reached once the k-of-n approval verified
/// and the root matched.  The bad-approval control is the same transition with
/// one signature byte flipped, rejected earlier at `k-of-n DeFMI approval is
/// invalid`, which proves the approval gate precedes the compare-and-swap and
/// that the cas case's approval genuinely verified.  The stale-root control is
/// a valid approval bound to the pre-fill root, rejected at `transaction was
/// built against a stale state root`, which proves the root gate precedes the
/// compare-and-swap and that the cas case used the live root.
/// The pool sequence, current note and state root are recorded before and
/// after to show nothing advanced.
fn cmd_pool_cas_probe(args: &Args) -> Result<Value, String> {
    let path = args.text("file", "");
    if path.is_empty() {
        return Err("pool-cas-probe needs --file <journaled allocation-bearing transition>".into());
    }
    let record: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|error| format!("{path}: {error}"))?)
            .map_err(|error| format!("{path}: {error}"))?;
    let params = record.get("params").cloned().unwrap_or(Value::Null);
    let t_dto = params
        .get("allocationTransition")
        .or_else(|| params.get("transition"))
        .cloned()
        .ok_or("no allocation transition")?;
    let a_dto = params
        .get("allocationAuthorization")
        .or_else(|| params.get("authorization"))
        .cloned()
        .ok_or("no allocation authorization")?;
    let al_dto = params.get("allocation").cloned().ok_or("no allocation")?;
    let old_root = params
        .get("expectedBeforeRoot")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let domain = params
        .pointer("/allocationApproval/domain")
        .or_else(|| params.pointer("/approval/domain"))
        .and_then(Value::as_str)
        .ok_or("no approval domain (chain id) in the journaled transition")?
        .to_string();

    let mut transition = parse_transition(&t_dto)?;
    let mut authorization = parse_authorization(&a_dto)?;
    let mut allocation = parse_allocation(&al_dto)?;

    let defmi = args.defmi();
    let root_before = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let current_root_hex = s(&root_before, "/stateRoot");
    let current_root = hex32(&root_before, "stateRoot")
        .map_err(|_| "the live state root is unavailable".to_string())?;
    let pool_id_hex = hex::encode(allocation.pool_id);
    let pool_before = pool_state(&defmi, &pool_id_hex);

    // Fresh identifiers so the already-used guard cannot pre-empt the
    // compare-and-swap; the note stays locked to the new hold.
    let nonce = unix_now();
    let fresh_operation = probe_digest(&[
        b"qomm-cas-probe:operation",
        &transition.operation_id,
        &nonce.to_le_bytes(),
    ]);
    let fresh_hold = probe_digest(&[
        b"qomm-cas-probe:hold",
        &transition.hold_id,
        &nonce.to_le_bytes(),
    ]);
    transition.operation_id = fresh_operation;
    transition.hold_id = fresh_hold;
    allocation.escrow_note.lock_id = fresh_hold;
    allocation.escrow_note.note_id = allocation.escrow_note.derived_id()?;
    // The reservation nullifier and its typed/asset digests are *derived* from
    // the allocation and the transition statement, not free fields: the fresh
    // hold changes the transition statement, so the derived nullifier is a new,
    // never-used one (this is what lets the request clear the already-used
    // guard and reach the compare-and-swap), and it must be re-derived here or
    // the VM rejects the reservation metadata before the pointer is even read.
    let transition_statement = transition.statement()?;
    let metadata = qomm_transport::standing_pool::standing_pool_reservation_metadata(
        allocation.pool_id,
        allocation.delegation_digest,
        allocation.expected_pool_sequence,
        allocation.previous_pool_note_id,
        allocation.proof_job_id,
        allocation.quote_proof_digest,
        allocation.dvp_proof_digest,
        transition_statement,
        authorization.entity_commitment,
        authorization.asset_id,
        authorization.direction,
        authorization.authorization_digest,
        authorization.mandate_digest,
        authorization.policy_version,
    )?;
    authorization.typed_reserve_digest = metadata.typed_reserve_digest;
    authorization.reserve_nullifier = metadata.reserve_nullifier;
    authorization.asset_link_proof_digest = metadata.asset_link_proof_digest;
    let fresh_nullifier = metadata.reserve_nullifier;
    // Bind the fresh reservation: escrow digest = the allocation's own
    // statement over the rebuilt transition and authorization.
    let escrow_digest = allocation.statement(&transition, &authorization)?;
    authorization.escrow_digest = escrow_digest;

    // Sanity: the rebuilt allocation body must still validate and conserve,
    // and the escrow digest must bind (mirrors the VM's first two checks).
    allocation.body(&transition, &authorization)?;
    if allocation.statement(&transition, &authorization)? != authorization.escrow_digest {
        return Err("rebuilt allocation does not bind its own escrow digest".into());
    }

    let (authorizer, keys) = development_quorum(&domain)?;
    // A genuine, different 3-of-7 subset (nodes 1, 3, 5).
    let signer_ids = ["node-1", "node-3", "node-5"];
    let signers: BTreeMap<String, qomm_defmi::governance::GovernanceSigner> = signer_ids
        .iter()
        .map(|id| {
            (
                id.to_string(),
                keys.get(*id).expect("dev node present").clone(),
            )
        })
        .collect();
    let statement = authorization.statement(&transition)?;
    let approval_live = authorizer.approve(statement, current_root, &signers)?;
    if !authorizer.verify_now(&statement, &current_root, &approval_live) {
        return Err("the rebuilt approval does not verify under the development committee".into());
    }
    let old_root_bytes = hex::decode(&old_root)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok());
    let approval_stale_root = old_root_bytes
        .map(|root| authorizer.approve(statement, root, &signers))
        .transpose()?;

    let base = |approval: &Value, root_hex: &str| -> Value {
        json!({
            "transition": transition_json(&transition),
            "authorization": authorization_json(&authorization),
            "allocation": allocation_json(&allocation),
            "approval": approval,
            "expectedBeforeRoot": root_hex,
        })
    };

    // cas: stale pointer, valid approval, live root.
    let cas_params = base(&approval_json(&approval_live), &current_root_hex);
    let (cas_submitted, cas_err, cas_tx, cas_status) = submit_and_follow(
        &defmi,
        "defmivm.issueStandingNotePoolAllocation",
        &cas_params,
    );
    let cas_reason = cas_status
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| cas_err.as_ref().and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();

    // bad-approval: flip one signature byte.
    let mut bad_approval = approval_json(&approval_live);
    let bad_sig = bad_approval
        .pointer("/approvals/0/signature")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(sig) = bad_sig {
        let mut bytes = hex::decode(&sig).unwrap_or_default();
        if let Some(first) = bytes.first_mut() {
            *first ^= 0x01;
        }
        if let Some(slot) = bad_approval.pointer_mut("/approvals/0/signature") {
            *slot = Value::String(hex::encode(bytes));
        }
    }
    let bad_params = base(&bad_approval, &current_root_hex);
    let (_bad_submitted, bad_err, bad_tx, bad_status) = submit_and_follow(
        &defmi,
        "defmivm.issueStandingNotePoolAllocation",
        &bad_params,
    );
    let bad_reason = bad_status
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| bad_err.as_ref().and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();

    // stale-root: valid approval bound to the pre-fill root.
    let (stale_tx, stale_status, stale_reason) =
        if let (Some(approval), Some(_)) = (approval_stale_root.as_ref(), old_root_bytes) {
            let params = base(&approval_json(approval), &old_root);
            let (_submitted, err, tx, status) =
                submit_and_follow(&defmi, "defmivm.issueStandingNotePoolAllocation", &params);
            let reason = status
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| err.as_ref().and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            (tx, status, reason)
        } else {
            (None, Value::Null, String::new())
        };

    let root_after = captured(rpc(
        &defmi,
        "defmivm.stateRoot",
        json!({}),
        Duration::from_secs(10),
    ));
    let pool_after = pool_state(&defmi, &pool_id_hex);

    let cas_is_stale_pointer = allocation.expected_pool_sequence
        != pool_before
            .get("sequence")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
        || hex::encode(allocation.previous_pool_note_id) != s(&pool_before, "/currentPoolNoteID");
    let cas_rejected_by_pool_cas =
        cas_reason.contains("standing allocation is stale or outside its Maker mandate");
    let bad_rejected_by_approval = bad_reason.contains("k-of-n DeFMI approval is invalid");
    let stale_rejected_by_root = stale_reason.contains("stale state root");

    Ok(json!({
        "recorded_at": unix_now(),
        "file": path,
        "method": "defmivm.issueStandingNotePoolAllocation",
        "chain_domain": domain,
        "signer_subset": signer_ids,
        "expected_before_root_live": current_root_hex,
        "pool_id": pool_id_hex,
        "allocation_expected_pool_sequence": allocation.expected_pool_sequence,
        "allocation_previous_pool_note_id": hex::encode(allocation.previous_pool_note_id),
        "fresh_operation_id": hex::encode(fresh_operation),
        "fresh_hold_id": hex::encode(fresh_hold),
        "fresh_reserve_nullifier": hex::encode(fresh_nullifier),
        "escrow_note_id": hex::encode(allocation.escrow_note.note_id),
        "conserves_parent_commitment": true,
        "approval_verifies_locally": true,
        "pointer_is_stale_vs_canonical": cas_is_stale_pointer,
        "cas": {
            "submitted": cas_submitted, "tx_id": cas_tx, "final_status": cas_status,
            "rejection_reason": cas_reason, "rejected_by_pool_sequence_current_note_cas": cas_rejected_by_pool_cas,
        },
        "bad_approval_control": {
            "tx_id": bad_tx, "final_status": bad_status,
            "rejection_reason": bad_reason, "rejected_by_approval_gate": bad_rejected_by_approval,
        },
        "stale_root_control": {
            "tx_id": stale_tx, "final_status": stale_status,
            "rejection_reason": stale_reason, "rejected_by_root_gate": stale_rejected_by_root,
        },
        "pool_state_before": pool_before,
        "pool_state_after": pool_after,
        "state_root_before": root_before,
        "state_root_after": root_after,
        "state_root_unchanged": current_root_hex == s(&root_after, "/stateRoot"),
        "pool_sequence_unchanged": pool_before.get("sequence") == pool_after.get("sequence"),
        "pool_current_note_unchanged": s(&pool_before, "/currentPoolNoteID") == s(&pool_after, "/currentPoolNoteID"),
    }))
}

fn cmd_snapshot(args: &Args) -> Result<Value, String> {
    let timeout = Duration::from_secs(30);
    let taker = args.taker();
    let defmi = args.defmi();
    let snapshot = taker_snapshot(&taker, timeout)?;
    let entries = outbox_entries(&snapshot);
    let reconcile_all = args.text("reconcile", "1") == "1";
    let mut holds = Vec::new();
    for entry in &entries {
        let request_id = entry
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let digest = entry
            .get("request_digest_hex")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !reconcile_all
            && matches!(
                state_name(entry).as_str(),
                "settled" | "released" | "aborted_before_reserve"
            )
        {
            continue;
        }
        let reconciliation = captured(reconcile(&taker, &request_id, &digest, timeout));
        let hold = reconciliation
            .get("hold_id")
            .and_then(Value::as_str)
            .map(|hold_id| hold_snapshot(&defmi, hold_id, timeout))
            .unwrap_or(Value::Null);
        holds.push(json!({
            "sequence": entry.get("sequence"),
            "request_id": request_id,
            "request_digest": digest,
            "outbox_state": state_name(entry),
            "reconciliation": reconciliation,
            "defmi_reservation": hold,
        }));
    }
    let mut nodes = Vec::new();
    let mut pool_ids = BTreeSet::new();
    for (index, endpoint) in args.mpc().iter().enumerate() {
        let health = captured(http_ok(
            "GET",
            &format!("{endpoint}/health"),
            None,
            Duration::from_secs(5),
        ));
        let maker_state = captured(http_ok(
            "GET",
            &format!("{endpoint}/v1/maker-state"),
            None,
            Duration::from_secs(10),
        ));
        if let Some(bindings) = maker_state.get("bindings").and_then(Value::as_array) {
            for binding in bindings {
                if let Some(pool) = binding.get("pool_id").and_then(Value::as_str) {
                    pool_ids.insert(pool.to_string());
                }
            }
        }
        let rounds = captured(http_ok(
            "GET",
            &format!("{endpoint}/v1/rounds"),
            None,
            Duration::from_secs(10),
        ));
        let receipts = rounds
            .get("receipts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        nodes.push(json!({
            "node": index,
            "endpoint": endpoint,
            "health": health,
            "maker_state": maker_state,
            "receipt_count": receipts.len(),
            "last_receipts": receipts.iter().rev().take(3).cloned().collect::<Vec<_>>(),
        }));
    }
    let mut pools = Vec::new();
    for pool_id in &pool_ids {
        let pool = captured(rpc(
            &defmi,
            "defmivm.standingNotePool",
            json!({"poolID": pool_id}),
            timeout,
        ));
        let note = pool
            .get("currentPoolNoteID")
            .and_then(Value::as_str)
            .map(|note_id| {
                captured(rpc(
                    &defmi,
                    "defmivm.note",
                    json!({"noteID": note_id}),
                    timeout,
                ))
            })
            .unwrap_or(Value::Null);
        pools.push(json!({
            "pool_id": pool_id,
            "pool": pool,
            "current_pool_note": note,
        }));
    }
    let state_root = captured(rpc(&defmi, "defmivm.stateRoot", json!({}), timeout));
    let network = captured(rpc(&defmi, "defmivm.network", json!({}), timeout));
    let gateway = captured(
        http(
            "GET",
            &format!("{}/", args.gateway()),
            None,
            Duration::from_secs(5),
        )
        .map(|(status, _)| json!({"status": status})),
    );
    // The gateway's own record of the last rounds (engine stats with the
    // Maker-state reconciliation actions, the allocation and the settlement),
    // taken as an observer.  Absent while the gateway is down.
    let history = captured(
        WsClient::connect(
            &args.gateway(),
            "seat=observer&label=acceptance",
            Duration::from_secs(5),
        )
        .and_then(|mut client| {
            let view = wait_for_view(&mut client, Instant::now() + Duration::from_secs(20));
            let _ = client.send_json(&json!({"type": "release"}));
            client.close();
            view
        })
        .map(|view| {
            let mut rounds = view.get("history").cloned().unwrap_or(Value::Null);
            if let Some(items) = rounds.as_array_mut() {
                for item in items {
                    if let Some(object) = item.as_object_mut() {
                        object.remove("node_shares");
                    }
                }
            }
            json!({
                "round_number": view.pointer("/public/number"),
                "phase": view.get("phase"),
                "history": rounds,
            })
        }),
    );
    let manifest = captured(http_ok(
        "GET",
        &defmi.replace("/rpc", "/manifest"),
        None,
        Duration::from_secs(10),
    ));
    let next_sequence = entries
        .iter()
        .filter_map(|entry| entry.get("sequence").and_then(Value::as_u64))
        .max()
        .map_or(0, |value| value + 1);
    Ok(json!({
        "recorded_at": unix_now(),
        "next_sequence": next_sequence,
        "taker": {
            "participant_id": snapshot.get("participant_id"),
            "portfolio": snapshot.get("portfolio"),
            "post_match_signature": snapshot.get("post_match_signature"),
            "outbox_metrics": snapshot.pointer("/mpc_outbox/metrics"),
            "outbox_durable": snapshot.pointer("/mpc_outbox/durable"),
            "local_execution_fallback": snapshot.pointer("/mpc_outbox/local_execution_fallback"),
            "outbox_entries": entries,
        },
        "holds": holds,
        "nodes": nodes,
        "pools": pools,
        "gateway": gateway,
        "gateway_history": history,
        "defmi": {
            "state_root": state_root,
            "network": network,
            "manifest": manifest,
        },
    }))
}

fn cmd_wait(args: &Args) -> Result<Value, String> {
    let timeout = args.timeout()?;
    let interval = Duration::from_millis(args.number("interval-ms", 250)?);
    let until = args.text("until", "finalized");
    let taker = args.taker();
    let defmi = args.defmi();
    let exact = args
        .options
        .get("sequence")
        .map(|_| args.number("sequence", 0))
        .transpose()?;
    let after = args
        .options
        .get("after-sequence")
        .map(|_| args.number("after-sequence", 0))
        .transpose()?;
    if exact.is_none() && after.is_none() {
        return Err("wait needs --sequence N or --after-sequence N".into());
    }
    let started = Instant::now();
    let deadline = started + timeout;
    let mut polls = 0_u64;
    let mut last = Value::Null;
    loop {
        polls += 1;
        let snapshot = taker_snapshot(&taker, Duration::from_secs(10));
        let mut reached = false;
        if let Ok(snapshot) = &snapshot {
            let entries = outbox_entries(snapshot);
            let entry = entries
                .iter()
                .filter(|entry| {
                    let sequence = entry
                        .get("sequence")
                        .and_then(Value::as_u64)
                        .unwrap_or(u64::MAX);
                    match (exact, after) {
                        (Some(exact), _) => sequence == exact,
                        (None, Some(after)) => sequence > after,
                        (None, None) => false,
                    }
                })
                .min_by_key(|entry| {
                    entry
                        .get("sequence")
                        .and_then(Value::as_u64)
                        .unwrap_or(u64::MAX)
                })
                .cloned();
            if let Some(entry) = entry {
                let state = state_name(&entry);
                let request_id = entry
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let digest = entry
                    .get("request_digest_hex")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // Every poll records the canonical view too, so even a
                // `present` record carries the hold id the request will use.
                let reconciliation = captured(reconcile(
                    &taker,
                    &request_id,
                    &digest,
                    Duration::from_secs(30),
                ));
                let hold_status = reconciliation
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                reached = match until.as_str() {
                    "present" => true,
                    "dispatching" => state == "dispatching",
                    "mpc_admitted" => state == "mpc_admitted",
                    "settled" => state == "settled",
                    "released" => state == "released",
                    "aborted_before_reserve" => state == "aborted_before_reserve",
                    "finalized" => matches!(
                        state.as_str(),
                        "settled" | "released" | "aborted_before_reserve"
                    ),
                    "hold-active" => hold_status == "active",
                    "hold-consumed" => hold_status == "consumed",
                    "hold-released" => hold_status == "released",
                    "hold-terminal" => matches!(hold_status.as_str(), "consumed" | "released"),
                    other => return Err(format!("unknown --until {other}")),
                };
                let hold = reconciliation
                    .get("hold_id")
                    .and_then(Value::as_str)
                    .map(|hold_id| hold_snapshot(&defmi, hold_id, Duration::from_secs(10)))
                    .unwrap_or(Value::Null);
                last = json!({
                    "sequence": entry.get("sequence"),
                    "request_id": request_id,
                    "request_digest": digest,
                    "accepted_at": entry.get("accepted_at"),
                    "expires_at": entry.get("expires_at"),
                    "outbox_state": state,
                    "outbox_state_detail": entry.get("state"),
                    "reconciliation": reconciliation,
                    "defmi_reservation": hold,
                });
            }
        } else if let Err(error) = &snapshot {
            last = json!({"taker_error": error});
        }
        if reached || Instant::now() >= deadline {
            return Ok(json!({
                "recorded_at": unix_now(),
                "until": until,
                "reached": reached,
                "polls": polls,
                "elapsed_ms": started.elapsed().as_millis(),
                "entry": last,
            }));
        }
        thread::sleep(interval);
    }
}

// --- judge ------------------------------------------------------------------
//
// The judge reads the files a scenario of `demo-network/live_acceptance.sh`
// recorded and turns them into named pass/fail checks with the observed
// values next to them.  It never contacts the network, so a verdict can be
// recomputed from the artifact alone.

struct Judge {
    dir: String,
    scenario: String,
    checks: Vec<Value>,
}

impl Judge {
    fn load(&self, suffix: &str) -> Value {
        let path = format!("{}/{}.{}.json", self.dir, self.scenario, suffix);
        fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            .unwrap_or(Value::Null)
    }

    fn text(&self, suffix: &str) -> String {
        let path = format!("{}/{}.{}", self.dir, self.scenario, suffix);
        fs::read_to_string(&path).unwrap_or_default()
    }

    fn check(&mut self, name: &str, pass: bool, observed: Value) {
        self.checks
            .push(json!({"name": name, "pass": pass, "observed": observed}));
    }
}

fn s(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .map(|item| match item {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default()
}

fn u(value: &Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(Value::as_u64)
}

fn hold_of(snapshot: &Value, sequence: u64) -> Value {
    snapshot
        .get("holds")
        .and_then(Value::as_array)
        .and_then(|holds| {
            holds
                .iter()
                .find(|hold| hold.get("sequence").and_then(Value::as_u64) == Some(sequence))
        })
        .cloned()
        .unwrap_or(Value::Null)
}

fn entries_after(snapshot: &Value, sequence: u64) -> Vec<u64> {
    snapshot
        .pointer("/taker/outbox_entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("sequence").and_then(Value::as_u64))
                .filter(|value| *value > sequence)
                .collect()
        })
        .unwrap_or_default()
}

fn pool_sequences(snapshot: &Value) -> BTreeMap<String, u64> {
    snapshot
        .get("pools")
        .and_then(Value::as_array)
        .map(|pools| {
            pools
                .iter()
                .filter_map(|pool| {
                    Some((
                        pool.get("pool_id")?.as_str()?.to_string(),
                        pool.pointer("/pool/sequence")?.as_u64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn node_generations(snapshot: &Value) -> Vec<Option<u64>> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    node.pointer("/maker_state/generation")
                        .and_then(Value::as_u64)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn node_health(snapshot: &Value) -> Vec<bool> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| node.pointer("/health/ok").and_then(Value::as_bool) == Some(true))
                .collect()
        })
        .unwrap_or_default()
}

/// The identifiers of the request that the scenario followed, taken from a
/// `wait` document.
fn identity(wait: &Value) -> (u64, String, String, String) {
    (
        u(wait, "/entry/sequence").unwrap_or(u64::MAX),
        s(wait, "/entry/request_id"),
        s(wait, "/entry/request_digest"),
        s(wait, "/entry/reconciliation/hold_id"),
    )
}

/// True when at least one node answered `/health`, so the snapshot's pool
/// list (taken from the node bindings) is a view and not an empty set.
fn nodes_reachable(snapshot: &Value) -> bool {
    node_health(snapshot).iter().any(|ok| *ok)
}

/// Every node's `/v1/rounds` receipts for the corporate outbox slot (a
/// round that was aborted and replayed leaves one per execution attempt).
fn receipts_for_slot(snapshot: &Value, slot: u64) -> Vec<Vec<Value>> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    node.get("last_receipts")
                        .and_then(Value::as_array)
                        .map(|receipts| {
                            receipts
                                .iter()
                                .filter(|receipt| {
                                    receipt.get("slot").and_then(Value::as_u64) == Some(slot)
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The maker and direction the nodes bind `pool_id` for.
fn pool_owner(snapshot: &Value, pool_id: &str) -> Value {
    snapshot
        .pointer("/nodes/0/maker_state/bindings")
        .and_then(Value::as_array)
        .and_then(|bindings| {
            bindings
                .iter()
                .find(|binding| binding.get("pool_id").and_then(Value::as_str) == Some(pool_id))
        })
        .map(
            |binding| json!({"maker": binding.get("maker"), "direction": binding.get("direction")}),
        )
        .unwrap_or(Value::Null)
}

/// Every node's binding of `pool_id`: the pool sequence it holds shares for
/// and its partial commitment.
fn bindings_of(snapshot: &Value, pool_id: &str) -> Vec<Option<(u64, String)>> {
    snapshot
        .get("nodes")
        .and_then(Value::as_array)
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| {
                    node.pointer("/maker_state/bindings")
                        .and_then(Value::as_array)
                        .and_then(|bindings| {
                            bindings.iter().find(|binding| {
                                binding.get("pool_id").and_then(Value::as_str) == Some(pool_id)
                            })
                        })
                        .and_then(|binding| {
                            Some((
                                binding.get("pool_sequence")?.as_u64()?,
                                binding.get("partial_commitment")?.as_str()?.to_string(),
                            ))
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The seven partial commitments of `pool_id`, all at the pool's canonical
/// sequence, summed and compared with the current pool note's commitment:
/// the resident Maker state opens exactly the note DeFMI holds.
fn shares_open_pool(snapshot: &Value, pool: &Value) -> Value {
    let pool_id = s(pool, "/pool_id");
    let sequence = u(pool, "/pool/sequence");
    let note_commitment = s(pool, "/current_pool_note/valueCommitment");
    let bindings = bindings_of(snapshot, &pool_id);
    let at_sequence = bindings
        .iter()
        .all(|binding| binding.as_ref().map(|(held, _)| Some(*held)) == Some(sequence));
    let partials = bindings
        .iter()
        .filter_map(|binding| binding.as_ref())
        .filter_map(|(_, partial)| {
            hex::decode(partial)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        })
        .collect::<Vec<_>>();
    let combined = combine_partial_commitments(&partials, InputSharing::Additive, 7)
        .map(hex::encode)
        .unwrap_or_default();
    json!({
        "pool_id": pool_id,
        "sequence": sequence,
        "nodes_bound_at_sequence": at_sequence && bindings.len() == 7,
        "partial_commitments": bindings.len(),
        "combined_commitment": combined,
        "pool_note_commitment": note_commitment,
        "opens": at_sequence && bindings.len() == 7 && !combined.is_empty() && combined == note_commitment,
    })
}

/// Correlate the request at `slot` with the pool it allocated from.  The
/// seven nodes' receipts for the slot name one execution; its remainder note
/// id is recomputed from those public digests, and the pool whose canonical
/// current note carries that id is the one the fill consumed.  Nothing here
/// relies on pool ids staying bound between two snapshots.
fn fill_of(after: &Value, slot: u64) -> Value {
    let per_node = receipts_for_slot(after, slot);
    let source = after
        .pointer("/nodes/0/health/source_sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Each distinct (round, execution generation) node 0 holds for the slot
    // is a candidate execution; it counts only when all seven nodes hold it.
    let candidates = per_node
        .first()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|receipt| (s(receipt, "/round_id"), u(receipt, "/execution_generation")))
        .collect::<BTreeSet<_>>();
    let mut executions = Vec::new();
    let mut matched = Vec::new();
    for (round_id, generation) in candidates {
        let receipts = per_node
            .iter()
            .filter_map(|receipts| {
                receipts.iter().find(|receipt| {
                    s(receipt, "/round_id") == round_id
                        && u(receipt, "/execution_generation") == generation
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let complete = per_node.len() == 7 && receipts.len() == 7;
        let mut derivations = Vec::new();
        if complete {
            for pool in after
                .get("pools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let note = remainder_note_id_of_execution(
                    &source,
                    &receipts,
                    &s(&pool, "/pool/assetID"),
                    &s(&pool, "/pool_id"),
                    &s(&pool, "/current_pool_note/valueCommitment"),
                );
                let current = s(&pool, "/pool/currentPoolNoteID");
                derivations.push(json!({"pool_id": s(&pool, "/pool_id"), "remainder_note_id": note.clone().ok(), "current_pool_note_id": current, "error": note.clone().err()}));
                if note.ok().as_deref() == Some(current.as_str()) {
                    matched.push(json!({
                        "pool_id": s(&pool, "/pool_id"),
                        "owner": pool_owner(after, &s(&pool, "/pool_id")),
                        "sequence": u(&pool, "/pool/sequence"),
                        "remainder_note_id": current,
                        "round_id": round_id,
                        "execution_generation": generation,
                        "maker_state_generation_at_execution": receipts.iter().map(|receipt| u(receipt, "/maker_state_generation")).collect::<Vec<_>>(),
                        "shares": shares_open_pool(after, &pool),
                    }));
                }
            }
        }
        executions.push(json!({
            "round_id": round_id,
            "execution_generation": generation,
            "receipts_complete": complete,
            "maker_state_generation_at_execution": receipts.iter().map(|receipt| u(receipt, "/maker_state_generation")).collect::<Vec<_>>(),
            "derivations": derivations,
        }));
    }
    json!({
        "slot": slot,
        "source_sha256": source,
        "executions": executions,
        "pools_matched": matched,
    })
}

fn judge_final(judge: &mut Judge, label: &str, first: &Value, replayed: &Value, after: &Value) {
    let (sequence, request_id, digest, mut hold) = identity(first);
    let (sequence2, request_id2, digest2, hold2) = identity(replayed);
    if hold.is_empty() {
        // An older `present` record carried no canonical view; the hold the
        // request was bound to is still in the final snapshot's reconciliation.
        hold = s(&hold_of(after, sequence), "/reconciliation/hold_id");
    }
    judge.check(
        &format!("{label}: same sequence, request id, digest and DeFMI hold before and after the outage"),
        sequence == sequence2 && request_id == request_id2 && digest == digest2 && hold == hold2
            && !request_id.is_empty() && !hold.is_empty(),
        json!({"sequence": sequence, "request_id": request_id, "request_digest": digest, "hold_id": hold,
               "after": {"sequence": sequence2, "request_id": request_id2, "request_digest": digest2, "hold_id": hold2}}),
    );
    let state = s(replayed, "/entry/outbox_state");
    let status = s(replayed, "/entry/reconciliation/status");
    judge.check(
        &format!("{label}: replay reached a canonical final state"),
        replayed.get("reached") == Some(&Value::Bool(true))
            && matches!(state.as_str(), "settled" | "released")
            && matches!(status.as_str(), "consumed" | "released"),
        json!({"reached": replayed.get("reached"), "outbox_state": state, "defmi_status": status,
               "ledger_height": replayed.pointer("/entry/reconciliation/ledger_height")}),
    );
    let receipt_tx = s(
        replayed,
        "/entry/reconciliation/canonical_receipt/transaction_id",
    );
    let settlement_digest = s(replayed, "/entry/defmi_reservation/settlementDigest");
    judge.check(
        &format!("{label}: the outbox receipt is the DeFMI transition that closed the hold"),
        !receipt_tx.is_empty() && receipt_tx == settlement_digest,
        json!({"receipt_transaction_id": receipt_tx, "defmi_settlement_digest": settlement_digest,
               "receipt": replayed.pointer("/entry/reconciliation/canonical_receipt")}),
    );
    let extra = entries_after(after, sequence);
    judge.check(
        &format!("{label}: no other request was created to reach that state"),
        extra.is_empty(),
        json!({"entries_after_sequence": extra}),
    );
    let final_hold = hold_of(after, sequence);
    judge.check(
        &format!("{label}: the final snapshot agrees with the replay record"),
        s(&final_hold, "/reconciliation/status") == status
            && s(&final_hold, "/reconciliation/hold_id") == hold
            && s(
                &final_hold,
                "/reconciliation/canonical_receipt/transaction_id",
            ) == receipt_tx,
        json!({"final": final_hold}),
    );
}

/// Pools whose canonical sequence moved between two snapshots (any move is
/// an allocation, one step is the only legitimate one) and pools that were
/// not bound on the nodes before, with the sequence they were first seen at.
fn pool_movement(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
) -> (Vec<Value>, Vec<Value>) {
    let mut advanced = Vec::new();
    let mut newly_bound = Vec::new();
    for (pool, sequence) in after {
        match before.get(pool) {
            Some(previous) if sequence == previous => {}
            Some(previous) => advanced.push(json!({"pool_id": pool, "from": previous, "to": sequence, "one_step": *sequence == previous + 1})),
            None => newly_bound.push(json!({"pool_id": pool, "sequence": sequence})),
        }
    }
    (advanced, newly_bound)
}

/// Per-node generation movement between two snapshots for the nodes not in
/// `skip`; `None` where either side is unreachable.
fn generation_deltas(
    before: &[Option<u64>],
    after: &[Option<u64>],
    skip: &[usize],
) -> Vec<Option<u64>> {
    after
        .iter()
        .enumerate()
        .filter(|(node, _)| !skip.contains(node))
        .map(
            |(node, generation)| match (generation, before.get(node).copied().flatten()) {
                (Some(after), Some(before)) if *after >= before => Some(*after - before),
                _ => None,
            },
        )
        .collect()
}

/// Account for every generation step on the live nodes between two
/// snapshots one round apart.  Binding a pool the nodes had not seen (a
/// pool the round start re-registered under a new id, or one caught up from
/// persistence) re-deals the registration opening to all seven nodes and
/// costs one step; a commit after a settlement costs one more.  Nothing else
/// may move a generation, so the observed delta must equal
/// `newly_bound + fill_commits` on every live node.  Registration steps are
/// not economic: the pool sequence stays where DeFMI has it.
fn judge_generation_accounting(
    judge: &mut Judge,
    label: &str,
    before: &Value,
    after: &Value,
    fill_commits: u64,
    skip: &[usize],
) -> Value {
    let pools_before = pool_sequences(before);
    let pools_after = pool_sequences(after);
    let (_, newly_bound) = pool_movement(&pools_before, &pools_after);
    let expected = newly_bound.len() as u64 + fill_commits;
    let deltas = generation_deltas(&node_generations(before), &node_generations(after), skip);
    let observed = json!({
        "generations_before": node_generations(before),
        "generations_after": node_generations(after),
        "deltas_on_live_nodes": deltas,
        "newly_bound_pools": newly_bound,
        "registration_steps": newly_bound.len(),
        "fill_commit_steps": fill_commits,
        "expected_delta": expected,
        "explanation": "a generation step is one compare-and-swap on a node's resident Maker state: re-dealing the registration opening of a pool the node had not bound (registration, non-economic) or the commit that follows a DeFMI settlement (one per fill)",
    });
    judge.check(
        &format!("{label}: every generation step on the live nodes is a registration re-deal or the fill commit, and all live nodes took the same steps"),
        nodes_reachable(before) && !deltas.is_empty() && deltas.iter().all(|delta| *delta == Some(expected)),
        observed.clone(),
    );
    observed
}

fn judge_frozen(
    judge: &mut Judge,
    label: &str,
    before: &Value,
    frozen: &Value,
    still: Option<&Value>,
    sequence: u64,
    stopped: &[usize],
) {
    let hold = hold_of(frozen, sequence);
    let state = s(&hold, "/outbox_state");
    let status = s(&hold, "/reconciliation/status");
    judge.check(
        &format!("{label}: the request stayed reserved and unsettled while nodes were down"),
        matches!(state.as_str(), "dispatching" | "mpc_admitted" | "queued") && status == "active",
        json!({"outbox_state": state, "defmi_status": status, "hold_id": s(&hold, "/reconciliation/hold_id")}),
    );
    // No pool may be allocated from while the request is frozen: no bound
    // pool moves its canonical sequence, and no execution for this slot has
    // a remainder note DeFMI accepted.
    let pools_before = pool_sequences(before);
    let pools_frozen = pool_sequences(frozen);
    let (advanced, newly_bound) = pool_movement(&pools_before, &pools_frozen);
    let fill = fill_of(frozen, sequence);
    let matched = fill
        .get("pools_matched")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    judge.check(
        &format!("{label}: no Maker pool was allocated from while nodes were down"),
        advanced.is_empty() && matched == 0 && nodes_reachable(frozen)
            && newly_bound.iter().all(|pool| pool.get("sequence") == Some(&json!(0))),
        json!({"advanced": advanced, "newly_bound": newly_bound, "execution_matched_a_pool_note": matched, "fill": fill}),
    );
    let health = node_health(frozen);
    if !stopped.is_empty() {
        let stopped_down = stopped.iter().all(|node| health.get(*node) == Some(&false));
        judge.check(
            &format!("{label}: the stopped nodes were unreachable"),
            stopped_down
                && stopped
                    .iter()
                    .all(|node| node_generations(frozen).get(*node) == Some(&None)),
            json!({"health": health, "stopped": stopped, "generations": node_generations(frozen)}),
        );
    }
    judge_generation_accounting(judge, label, before, frozen, 0, stopped);
    // The second observation while still down: nothing moved at all.
    let Some(still) = still else { return };
    let gens_frozen = node_generations(frozen);
    let gens_still = node_generations(still);
    let quiet = gens_frozen == gens_still
        && pool_sequences(frozen) == pool_sequences(still)
        && s(&hold_of(still, sequence), "/reconciliation/status") == "active"
        && s(&hold_of(still, sequence), "/outbox_state") == state;
    judge.check(
        &format!("{label}: while the nodes stayed down no generation, pool sequence or hold status moved"),
        quiet,
        json!({"generations_first": gens_frozen, "generations_later": gens_still,
               "pools_first": pool_sequences(frozen), "pools_later": pool_sequences(still),
               "hold_later": hold_of(still, sequence).get("reconciliation")}),
    );
}

/// After a round that filled (`slot` settled) or did not, between two
/// snapshots one round apart: the fill allocated from exactly one pool,
/// identified through the accepted execution and the canonical remainder
/// note; that pool's sequence moved by one and every other bound pool kept
/// its sequence; the seven nodes' shares open the pool note; and the
/// generations account for the registrations plus one commit.
fn judge_advanced_once(
    judge: &mut Judge,
    label: &str,
    before: &Value,
    after: &Value,
    filled: bool,
    slot: u64,
) -> Value {
    let pools_before = pool_sequences(before);
    let pools_after = pool_sequences(after);
    let (advanced, newly_bound) = pool_movement(&pools_before, &pools_after);
    let fill = fill_of(after, slot);
    let matched = fill
        .get("pools_matched")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let winner = matched.first().cloned().unwrap_or(Value::Null);
    let winner_id = s(&winner, "/pool_id");
    let winner_sequence = u(&winner, "/sequence").unwrap_or(0);
    // The winning pool moved by exactly one: from its sequence in `before`
    // when it was bound there, otherwise from the sequence the nodes caught
    // it up to, which the receipts' pool sequence in the commit records.
    let winner_moved_once = if filled {
        match pools_before.get(&winner_id) {
            Some(previous) => winner_sequence == previous + 1,
            None => winner_sequence >= 1,
        }
    } else {
        true
    };
    let others_still = advanced
        .iter()
        .all(|pool| s(pool, "/pool_id") == winner_id && filled);
    let rebound = newly_bound
        .iter()
        .filter(|pool| pool.get("sequence") != Some(&json!(0)) && s(pool, "/pool_id") != winner_id)
        .cloned()
        .collect::<Vec<_>>();
    // A pool the nodes newly bound at a sequence above zero without this
    // slot's execution behind it was caught up from persistence, not
    // allocated: its shares must still open the canonical note.
    let rebound_open = rebound.iter().all(|pool| {
        after
            .get("pools")
            .and_then(Value::as_array)
            .and_then(|pools| {
                pools
                    .iter()
                    .find(|item| s(item, "/pool_id") == s(pool, "/pool_id"))
            })
            .is_some_and(|item| {
                shares_open_pool(after, item).get("opens") == Some(&Value::Bool(true))
            })
    });
    judge.check(
        &format!("{label}: {}", if filled {
            "the fill allocated from exactly one pool, found through the accepted execution's remainder note, and that pool moved by exactly one sequence while every other bound pool kept its sequence"
        } else {
            "no execution for the request produced a pool note DeFMI accepted, and every bound pool kept its sequence"
        }),
        matched.len() == usize::from(filled)
            && winner_moved_once
            && others_still
            && rebound_open
            && nodes_reachable(before)
            && nodes_reachable(after),
        json!({"filled": filled, "winning_pool": winner, "advanced": advanced, "newly_bound": newly_bound,
               "caught_up_not_allocated": rebound, "fill": fill}),
    );
    if filled {
        judge.check(
            &format!("{label}: the seven nodes' resident shares open the winning pool's current DeFMI note at its canonical sequence"),
            winner.pointer("/shares/opens") == Some(&Value::Bool(true)),
            json!({"shares": winner.get("shares")}),
        );
        let executed_at = winner
            .get("maker_state_generation_at_execution")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let after_generations = node_generations(after);
        let committed_once = executed_at.len() == 7
            && after_generations.len() == 7
            && executed_at
                .iter()
                .zip(&after_generations)
                .all(|(executed, now)| match (executed.as_u64(), now) {
                    (Some(executed), Some(now)) => *now == executed + 1,
                    _ => false,
                });
        judge.check(
            &format!("{label}: every node executed at the generation its receipt names and committed exactly once after the settlement"),
            committed_once,
            json!({"at_execution": executed_at, "after": after_generations}),
        );
    }
    let accounting =
        judge_generation_accounting(judge, label, before, after, u64::from(filled), &[]);
    json!({"fill": fill, "generations": accounting})
}

/// A refusal before any signature or hold: the room answered `refused`
/// and no corporate outbox entry exists for it.
fn refusal_text(rfq: &Value) -> String {
    format!("{}{}", s(rfq, "/abort_reason"), s(rfq, "/refused"))
}

/// The Taker legal entity's facility figures the participant module
/// reports (caps and outstanding reservations), for before/after comparison.
fn taker_balances(snapshot: &Value) -> Value {
    snapshot
        .pointer("/taker/portfolio")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The gateway's Taker balance projection as an `rfq` record saw it before
/// submitting (`before`) and after the round (`after`): available cash and
/// the available inventory of `asset`.
fn projected(rfq: &Value, side: &str, asset: u64) -> (Option<i64>, Option<i64>) {
    let portfolio = rfq.pointer(&format!("/{side}/taker/portfolio"));
    let cash = portfolio
        .and_then(|portfolio| portfolio.pointer("/cash/available"))
        .and_then(Value::as_i64);
    let inventory = portfolio
        .and_then(|portfolio| portfolio.get("inventory"))
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("asset").and_then(Value::as_u64) == Some(asset))
        })
        .and_then(|item| item.get("available"))
        .and_then(Value::as_i64);
    (cash, inventory)
}

fn cmd_judge(args: &Args) -> Result<Value, String> {
    let scenario = args.text("scenario", "");
    if scenario.is_empty() {
        return Err("judge needs --scenario NAME".into());
    }
    let mut judge = Judge {
        dir: args.text("dir", "/out"),
        scenario: scenario.clone(),
        checks: Vec::new(),
    };
    let before = judge.load("00-before");
    let after = judge.load("99-after");
    match scenario.as_str() {
        "expired-seq7" => {
            let entry = judge.load("01-entry");
            let hold = &entry["entry"];
            judge.check(
                "seq 7 is released in the corporate outbox with a canonical receipt bound to its digest",
                s(hold, "/outbox_state") == "released"
                    && s(hold, "/reconciliation/status") == "released"
                    && hold.pointer("/reconciliation/queue_finalized") == Some(&Value::Bool(true))
                    && !s(hold, "/reconciliation/canonical_receipt/request_digest").is_empty(),
                json!({"entry": hold}),
            );
            judge.check(
                "the DeFMI hold is released by the same transition the receipt names, never consumed",
                s(hold, "/defmi_reservation/status") == "released"
                    && s(hold, "/defmi_reservation/settlementDigest") == s(hold, "/reconciliation/canonical_receipt/transaction_id"),
                json!({"defmi": hold.get("defmi_reservation"), "receipt": hold.pointer("/reconciliation/canonical_receipt")}),
            );
            let expired = u(hold, "/expires_at").unwrap_or(0);
            let finalized = u(hold, "/reconciliation/canonical_receipt/finalized_at").unwrap_or(0);
            judge.check(
                "the release happened after the signed deadline, not before",
                expired > 0 && finalized > expired,
                json!({"expires_at": expired, "finalized_at": finalized}),
            );
            let reuse = judge.text("02-reuse-probe.txt");
            judge.check(
                "the participant module refuses another body under seq 7's request id",
                reuse.contains("409") && reuse.contains("\"error\""),
                json!({"response": reuse.trim()}),
            );
            let stale = judge.text("03-expired-probe.txt");
            judge.check(
                "the participant module refuses a request whose expiry already passed",
                stale.contains("409") && stale.to_ascii_lowercase().contains("expire"),
                json!({"response": stale.trim()}),
            );
        }
        "outage-queue" => {
            let rfq = judge.load("01-rfq-while-down");
            judge.check(
                "the RFQ signed while every node was stopped was accepted into the durable queue, not executed",
                s(&rfq, "/abort_code") == "queued" && s(&rfq, "/abort_reason").contains("durably queued"),
                json!({"abort_code": rfq.get("abort_code"), "abort_reason": rfq.get("abort_reason"), "status": rfq.get("status")}),
            );
            let queued = judge.load("02-queued");
            let (sequence, ..) = identity(&queued);
            let down = judge.load("03-all-down");
            // The `present` record may predate the canonical view being taken
            // on every poll; the snapshots always reconcile, so the canonical
            // "not reserved" comes from the all-down snapshot.
            judge.check(
                "the entry is `queued` with no DeFMI reservation yet",
                s(&queued, "/entry/outbox_state") == "queued"
                    && (s(&queued, "/entry/reconciliation/status") == "not_reserved"
                        || queued.pointer("/entry/reconciliation").is_none_or(Value::is_null))
                    && s(&hold_of(&down, sequence), "/reconciliation/status") == "not_reserved",
                json!({"entry": queued.get("entry"), "canonical_view_while_down": hold_of(&down, sequence).get("reconciliation")}),
            );
            let health = node_health(&down);
            judge.check(
                "with the gateway and all seven nodes stopped the queue still holds the entry",
                health.iter().all(|ok| !*ok) && health.len() == 7
                    && !s(&down, "/gateway/error").is_empty()
                    && s(&hold_of(&down, sequence), "/outbox_state") == "queued",
                json!({"node_health": health, "gateway": down.get("gateway"), "entry": hold_of(&down, sequence)}),
            );
            let still = judge.load("04-still-queued");
            judge.check(
                "twenty seconds later nothing moved: still queued, still not reserved",
                s(&hold_of(&still, sequence), "/outbox_state") == "queued"
                    && s(&hold_of(&still, sequence), "/reconciliation/status") == "not_reserved",
                json!({"entry": hold_of(&still, sequence)}),
            );
            let replayed = judge.load("05-replayed");
            judge_final(&mut judge, "after restart", &queued, &replayed, &after);
            let filled = s(&replayed, "/entry/reconciliation/status") == "consumed";
            judge_advanced_once(
                &mut judge,
                "after restart",
                &before,
                &after,
                filled,
                sequence,
            );
        }
        "outage-dispatching" => {
            let active = judge.load("02-hold-active");
            let (sequence, ..) = identity(&active);
            judge.check(
                "the gateway was killed after the DeFMI reserve existed and before settlement",
                s(&active, "/entry/reconciliation/status") == "active"
                    && matches!(
                        s(&active, "/entry/outbox_state").as_str(),
                        "dispatching" | "mpc_admitted"
                    ),
                json!({"entry": active.get("entry")}),
            );
            let down = judge.load("03-gateway-down");
            judge.check(
                "with the gateway down the reserve stayed active and the request unsettled",
                !s(&down, "/gateway/error").is_empty()
                    && s(&hold_of(&down, sequence), "/reconciliation/status") == "active",
                json!({"gateway": down.get("gateway"), "entry": hold_of(&down, sequence)}),
            );
            judge_frozen(
                &mut judge,
                "gateway down",
                &before,
                &down,
                None,
                sequence,
                &[],
            );
            let replayed = judge.load("04-replayed");
            judge_final(
                &mut judge,
                "after gateway restart",
                &active,
                &replayed,
                &after,
            );
            let filled = s(&replayed, "/entry/reconciliation/status") == "consumed";
            judge_advanced_once(
                &mut judge,
                "after gateway restart",
                &before,
                &after,
                filled,
                sequence,
            );
        }
        "abort-1" | "abort-2" | "abort-3" => {
            let count = scenario
                .trim_start_matches("abort-")
                .parse::<usize>()
                .unwrap_or(0);
            let stopped = (7 - count..7).collect::<Vec<_>>();
            let active = judge.load("02-hold-active");
            let (sequence, ..) = identity(&active);
            judge.check(
                "nodes were stopped after the DeFMI reserve existed, during execution",
                s(&active, "/entry/reconciliation/status") == "active",
                json!({"entry": active.get("entry"), "stopped": stopped, "threshold": 2,
                       "above_threshold": count > 2}),
            );
            let rfq = judge.load("01-rfq");
            judge.check(
                "the live round failed closed into reconciliation instead of settling with a partial committee",
                rfq.get("aborted") == Some(&Value::Bool(true))
                    && s(&rfq, "/abort_code") == "queued"
                    && s(&rfq, "/abort_reason").contains("awaiting canonical reconciliation"),
                json!({"aborted": rfq.get("aborted"), "abort_code": rfq.get("abort_code"), "abort_reason": rfq.get("abort_reason"), "status": rfq.get("status")}),
            );
            let frozen = judge.load("03-after-abort");
            let still = judge.load("04-still-reserved");
            judge_frozen(
                &mut judge,
                "after abort",
                &before,
                &frozen,
                Some(&still),
                sequence,
                &stopped,
            );
            let replayed = judge.load("05-replayed");
            judge_final(
                &mut judge,
                "after the nodes returned",
                &active,
                &replayed,
                &after,
            );
            let filled = s(&replayed, "/entry/reconciliation/status") == "consumed";
            judge_advanced_once(
                &mut judge,
                "after the nodes returned",
                &before,
                &after,
                filled,
                sequence,
            );
        }
        "concurrent-facility" => {
            let a = judge.load("01-rfq-a");
            let b = judge.load("02-rfq-b");
            let both = judge.load("03-both-submitted");
            let last = u(&before, "/next_sequence").unwrap_or(0).saturating_sub(1);
            let cap = u(&before, "/taker/portfolio/inventory").unwrap_or(0);
            let qty_a = u(&a, "/submit/qty").unwrap_or(0);
            let qty_b = u(&b, "/submit/qty").unwrap_or(0);
            judge.check(
                "each sell alone fits the Taker entity's inventory facility and the two together exceed it",
                qty_a > 0 && qty_a <= cap && qty_b > 0 && qty_b <= cap && qty_a + qty_b > cap,
                json!({"inventory_cap": cap, "qty_a": qty_a, "qty_b": qty_b, "balances_before": taker_balances(&before)}),
            );
            judge.check(
                "the first sell was accepted into the queue while the committee was down",
                s(&a, "/abort_code") == "queued",
                json!({"abort_code": a.get("abort_code"), "abort_reason": a.get("abort_reason")}),
            );
            // The corporate boundary admits one outstanding Taker reservation:
            // the room refuses the second signature while the first reserve is
            // live, and the participant module would refuse the sum at the
            // outbox cap behind it.  Either refusal is before any DeFMI hold.
            let refusal = refusal_text(&b);
            judge.check(
                "the second sell, which with the first exceeds the corporate inventory cap, was refused before any reserve",
                refusal.contains("exceed the corporate cash or inventory limit")
                    || refusal.contains("previous Taker reservation is still active"),
                json!({"abort_code": b.get("abort_code"), "abort_reason": b.get("abort_reason"), "refused": b.get("refused"), "status": b.get("status")}),
            );
            let created = entries_after(&both, last);
            judge.check(
                "exactly one new outbox entry exists after both submissions; the refused one left no entry and no hold",
                created.len() == 1,
                json!({"entries_after": created, "holds": both.get("holds")}),
            );
            let a_final = judge.load("04-a-finalized");
            let after_a = judge.load("05-after-a");
            let a_filled = s(&a_final, "/entry/reconciliation/status") == "consumed";
            judge.check(
                "the accepted sell reached a canonical final state once the committee returned",
                a_final.get("reached") == Some(&Value::Bool(true))
                    && matches!(
                        s(&a_final, "/entry/reconciliation/status").as_str(),
                        "consumed" | "released"
                    ),
                json!({"entry": a_final.get("entry")}),
            );
            judge_advanced_once(
                &mut judge,
                "after the first sell",
                &before,
                &after_a,
                a_filled,
                last + 1,
            );
            let c = judge.load("06-rfq-c");
            let c_hold = hold_of(&after, last + 2);
            let c_filled = s(&c_hold, "/reconciliation/status") == "consumed";
            judge.check(
                "a third sell after the first one is final was judged against outstanding reservations and canonical notes, and reached a canonical final state",
                !s(&c, "/status").is_empty()
                    && (s(&c, "/status") == "refused"
                        || matches!(s(&c_hold, "/reconciliation/status").as_str(), "consumed" | "released")),
                json!({"abort_code": c.get("abort_code"), "abort_reason": c.get("abort_reason"), "refused": c.get("refused"), "status": c.get("status"), "corporate_outbox": c.get("corporate_outbox"), "final_hold": c_hold}),
            );
            judge_advanced_once(
                &mut judge,
                "after the third sell",
                &after_a,
                &after,
                c_filled,
                last + 2,
            );
            // The gateway's Taker projection from the first submission to
            // the end of the third round moves only by the sells that
            // settled; the participant module's figures are the facility caps.
            let (cash_start, inventory_start) = projected(&a, "before", 0);
            let (cash_end, inventory_end) = projected(&c, "after", 0);
            let sold = (u64::from(a_filled) * qty_a + u64::from(c_filled) * qty_a) as i64;
            judge.check(
                "the Taker's projected inventory moved only by the sells that settled, never beyond the facility",
                matches!((inventory_start, inventory_end), (Some(start), Some(end)) if end == start - sold && sold <= cap as i64)
                    && (sold > 0 || cash_start == cash_end)
                    && u(&after, "/taker/portfolio/reserved_inventory") == Some(0),
                json!({"inventory_start": inventory_start, "inventory_end": inventory_end, "cash_start": cash_start, "cash_end": cash_end,
                       "settled_quantity": sold, "facility_before": taker_balances(&before), "facility_after": taker_balances(&after)}),
            );
        }
        "concurrent-pool" => {
            let policies = judge.load("02-policies");
            let last = u(&policies, "/next_sequence")
                .unwrap_or(0)
                .saturating_sub(1);
            let a = judge.load("03-rfq-a");
            let b = judge.load("04-rfq-b");
            let queued = judge.load("05-both-queued");
            let after_a = judge.load("07-after-a");
            let after_b = judge.load("09-after-b");
            let created = entries_after(&queued, last);
            if s(&a, "/status") == "refused" && s(&b, "/status") == "refused" {
                // The recorded run: both 150-unit buys were refused at the
                // Taker entity's cash facility before any signature.  That
                // is a facility refusal, not a pool contention; the pool
                // aggregate is judged in `pool-sum`.
                judge.check(
                    "both buys were refused at the Taker cash facility before any signature or hold (the pool aggregate is judged in pool-sum)",
                    refusal_text(&a).contains("cash units available") && refusal_text(&b).contains("cash units available"),
                    json!({"a": a.get("refused"), "b": b.get("refused"), "qty": a.pointer("/submit/qty"), "balances": taker_balances(&policies)}),
                );
                judge.check(
                    "no corporate outbox entry, hold, pool sequence or node generation came from the refused buys",
                    created.is_empty()
                        && entries_after(&after, last).is_empty()
                        && pool_sequences(&policies) == pool_sequences(&after_b)
                        && node_generations(&policies) == node_generations(&after_b),
                    json!({"entries_after": created, "pools_before": pool_sequences(&policies), "pools_after": pool_sequences(&after_b),
                           "generations_before": node_generations(&policies), "generations_after": node_generations(&after_b)}),
                );
            } else {
                judge.check(
                    "two buys against the same Maker pool were both durable before either executed",
                    created == vec![last + 1, last + 2]
                        && s(&hold_of(&queued, last + 1), "/outbox_state") == "queued"
                        && s(&hold_of(&queued, last + 2), "/outbox_state") == "queued",
                    json!({"entries_after": created, "holds": queued.get("holds")}),
                );
                let a_final = judge.load("06-a-finalized");
                let a_filled = s(&a_final, "/entry/reconciliation/status") == "consumed";
                judge.check(
                    "the first buy reached a canonical final state",
                    a_final.get("reached") == Some(&Value::Bool(true)),
                    json!({"entry": a_final.get("entry")}),
                );
                judge_advanced_once(
                    &mut judge,
                    "after the first buy",
                    &policies,
                    &after_a,
                    a_filled,
                    last + 1,
                );
                let b_wait = judge.load("08-b");
                let b_filled = s(&b_wait, "/entry/reconciliation/status") == "consumed";
                judge.check(
                    "the second buy's outcome is recorded (it must not be filled beyond the pool remainder)",
                    !s(&b_wait, "/entry/outbox_state").is_empty(),
                    json!({"entry": b_wait.get("entry"), "reached": b_wait.get("reached")}),
                );
                judge_advanced_once(
                    &mut judge,
                    "after the second buy",
                    &after_a,
                    &after_b,
                    b_filled,
                    last + 2,
                );
            }
        }
        "pool-sum" => {
            let policies = judge.load("02-policies");
            let last = u(&policies, "/next_sequence")
                .unwrap_or(0)
                .saturating_sub(1);
            let on = (0..4)
                .map(|maker| judge.load(&format!("01-policy-maker{maker}-on")))
                .find(|policy| !policy.is_null())
                .unwrap_or(Value::Null);
            let maxqty = u(&on, "/values/maxqty").unwrap_or(0);
            let others_off = (0..4)
                .map(|maker| judge.load(&format!("01-policy-maker{maker}-off")))
                .filter(|policy| !policy.is_null())
                .collect::<Vec<_>>();
            judge.check(
                "one Maker is registered with a small standing pool and the other Makers are inactive, so both buys meet the same pool",
                s(&on, "/outcome/status") == "applied" && maxqty > 0 && others_off.len() == 3
                    && others_off.iter().all(|policy| s(policy, "/outcome/status") == "applied" && u(policy, "/values/active") == Some(0)),
                json!({"maker": on.get("maker"), "policy": on.get("values"), "others_off": others_off.iter().map(|policy| policy.get("maker").cloned()).collect::<Vec<_>>()}),
            );
            let a = judge.load("03-rfq-a");
            let qty_a = u(&a, "/submit/qty").unwrap_or(0);
            let a_active = judge.load("04-a-hold-active");
            let (sequence_a, ..) = identity(&a_active);
            judge.check(
                "the first buy was reserved in DeFMI (hold active) before the second one was signed",
                s(&a_active, "/entry/reconciliation/status") == "active" && sequence_a == last + 1,
                json!({"entry": a_active.get("entry")}),
            );
            let b_concurrent = judge.load("05-rfq-b-concurrent");
            // The concurrent attempt may have ended before submitting (no
            // view while the round ran); the sequential attempt always
            // records the quantity it signed for.
            let qty_b = u(&judge.load("08-rfq-b"), "/submit/qty")
                .or(u(&b_concurrent, "/submit/qty"))
                .unwrap_or(0);
            judge.check(
                "each buy alone fits the pool and the two together exceed it",
                qty_a > 0
                    && qty_a <= maxqty
                    && qty_b > 0
                    && qty_b <= maxqty
                    && qty_a + qty_b > maxqty,
                json!({"pool_maxqty": maxqty, "qty_a": qty_a, "qty_b": qty_b}),
            );
            // While the first round runs the room serves no second Taker
            // request: it either refuses the signature because the first
            // reserve is live, or (the round being in progress) hands the
            // second client no view before its deadline.  Either way nothing
            // is signed and no second hold exists; the snapshot after the
            // first buy proves the latter.
            let refusal = format!(
                "{}{}",
                refusal_text(&b_concurrent),
                s(&b_concurrent, "/error")
            );
            judge.check(
                "the second buy, submitted while the first was reserved, was not admitted: refused before any hold or given no view while the round ran",
                (s(&b_concurrent, "/status") == "refused"
                    && (refusal.contains("previous Taker reservation is still active")
                        || refusal.contains("exceed the corporate cash or inventory limit")))
                    || refusal.contains("no gateway view before the deadline")
                    || refusal.contains("seat was not granted"),
                json!({"refused": b_concurrent.get("refused"), "status": b_concurrent.get("status"), "error": b_concurrent.get("error")}),
            );
            let a_final = judge.load("06-a-finalized");
            let after_a = judge.load("07-after-a");
            let a_filled = s(&a_final, "/entry/reconciliation/status") == "consumed";
            judge.check(
                "the first buy filled and settled canonically, and it is the only outbox entry the pair created",
                a_final.get("reached") == Some(&Value::Bool(true)) && a_filled
                    && s(&a_final, "/entry/outbox_state") == "settled"
                    && entries_after(&after_a, last) == vec![last + 1],
                json!({"entry": a_final.get("entry"), "entries_after": entries_after(&after_a, last)}),
            );
            let fill_a = judge_advanced_once(
                &mut judge,
                "after the first buy",
                &policies,
                &after_a,
                a_filled,
                sequence_a,
            );
            let winner = fill_a
                .pointer("/fill/pools_matched/0")
                .cloned()
                .unwrap_or(Value::Null);
            let winner_id = s(&winner, "/pool_id");
            let winner_sequence = u(&winner, "/sequence");
            // The same buy again once the first is final: the pool remainder
            // (maxqty - qty_a) is smaller than qty_b.
            let b_again = judge.load("08-rfq-b");
            let b_later = judge.load("10-b-later");
            let after_b = judge.load("11-after-b");
            let b_hold = hold_of(&after_b, last + 2);
            let b_state = s(&b_hold, "/outbox_state");
            let b_status = s(&b_hold, "/reconciliation/status");
            let b_filled = b_status == "consumed";
            // The gateway mirrors the pool remainder in the Maker's reserve
            // and refuses to dispatch a request the standing mandate no
            // longer covers (`engine` abort before any hold); a request that
            // did reach DeFMI could only end released.  Neither may settle.
            let b_aborted_before_hold = b_hold.is_null()
                && (s(&b_again, "/status") == "refused"
                    || (b_again.get("aborted") == Some(&Value::Bool(true))
                        && s(&b_again, "/abort_code") != "queued"));
            judge.check(
                "the second buy, which exceeds the pool remainder, did not settle: it was refused or aborted fail-closed before any hold, or its hold was released, or it is still reserved and unsettled",
                b_aborted_before_hold
                    || (!b_filled
                        && matches!(b_state.as_str(), "released" | "queued" | "dispatching" | "mpc_admitted" | "aborted_before_reserve")
                        && matches!(b_status.as_str(), "released" | "active" | "not_reserved" | "aborted_before_reserve")),
                json!({"rfq": {"status": b_again.get("status"), "refused": b_again.get("refused"), "aborted": b_again.get("aborted"), "abort_code": b_again.get("abort_code"), "abort_reason": b_again.get("abort_reason"), "settlement": b_again.get("settlement")},
                       "later": b_later.get("entry"), "final_hold": b_hold, "entries_after_first": entries_after(&after_b, last + 1)}),
            );
            judge.check(
                "the winning pool kept its sequence through the second buy: nothing was allocated beyond the first fill",
                !winner_id.is_empty()
                    && pool_sequences(&after_b).get(&winner_id).copied() == winner_sequence
                    && pool_movement(&pool_sequences(&after_a), &pool_sequences(&after_b)).0.is_empty()
                    && fill_of(&after_b, last + 2).get("pools_matched").and_then(Value::as_array).is_some_and(Vec::is_empty),
                json!({"pool_id": winner_id, "sequence_after_first": winner_sequence, "pools_after_second": pool_sequences(&after_b),
                       "second_buy_execution": fill_of(&after_b, last + 2)}),
            );
            judge_generation_accounting(
                &mut judge,
                "after the second buy",
                &after_a,
                &after_b,
                0,
                &[],
            );
            // Balances: the gateway's Taker projection around each round
            // (the participant module reports the facility caps, not a
            // running balance).
            let (cash_0, inventory_0) = projected(&a, "before", 0);
            let (cash_a, inventory_a) = projected(&a, "after", 0);
            let (cash_b0, inventory_b0) = projected(&b_again, "before", 0);
            let (cash_b, inventory_b) = projected(&b_again, "after", 0);
            let first_only = match (inventory_0, inventory_a, inventory_b0, inventory_b) {
                (Some(i0), Some(ia), Some(ib0), Some(ib)) => {
                    ia == i0 + qty_a as i64 && ib0 == ia && ib == ia
                }
                _ => false,
            };
            let paid = match (cash_0, cash_a, cash_b0, cash_b) {
                (Some(c0), Some(ca), Some(cb0), Some(cb)) => ca < c0 && cb0 == ca && cb == ca,
                _ => false,
            };
            judge.check(
                "the Taker's projected inventory rose by the first buy only and its cash fell once: of two buys that together exceed the pool, exactly the first amount settled",
                first_only && paid && qty_a <= maxqty,
                json!({"inventory_before": inventory_0, "inventory_after_first": inventory_a, "inventory_after_second": inventory_b,
                       "cash_before": cash_0, "cash_after_first": cash_a, "cash_after_second": cash_b,
                       "facility_before": taker_balances(&policies), "facility_after": taker_balances(&after_b),
                       "settled_quantity": inventory_b.zip(inventory_0).map(|(b, z)| b - z), "pool_maxqty": maxqty, "requested_total": qty_a + qty_b}),
            );
        }
        "pool-race" => {
            let policies = judge.load("02-policies");
            let last = u(&policies, "/next_sequence")
                .unwrap_or(0)
                .saturating_sub(1);
            let on = (0..4)
                .map(|maker| judge.load(&format!("01-policy-maker{maker}-on")))
                .find(|policy| !policy.is_null())
                .unwrap_or(Value::Null);
            let maxqty = u(&on, "/values/maxqty").unwrap_or(0);
            let a = judge.load("03-rfq-a");
            let b = judge.load("05-rfq-b");
            let qty_a = u(&a, "/submit/qty").unwrap_or(0);
            let qty_b = u(&b, "/submit/qty").unwrap_or(0);
            judge.check(
                "one Maker holds one standing pool and the two signed requests each fit it while their sum exceeds it",
                s(&on, "/outcome/status") == "applied" && maxqty > 0
                    && qty_a > 0 && qty_a <= maxqty && qty_b > 0 && qty_b <= maxqty && qty_a + qty_b > maxqty,
                json!({"policy": on.get("values"), "qty_a": qty_a, "qty_b": qty_b, "direction": a.pointer("/submit/direction")}),
            );
            let a_queued = judge.load("04-a-queued");
            let b_queued = judge.load("06-b-queued");
            let both = judge.load("07-both-queued");
            let (sequence_a, request_a, digest_a, _) = identity(&a_queued);
            let (sequence_b, request_b, digest_b, _) = identity(&b_queued);
            judge.check(
                "both requests were signed and durably admitted by the corporate module as distinct requests before either executed",
                s(&a, "/abort_code") == "queued" && s(&b, "/abort_code") == "queued"
                    && sequence_a == last + 1 && sequence_b == last + 2
                    && !request_a.is_empty() && !request_b.is_empty() && request_a != request_b && digest_a != digest_b
                    && entries_after(&both, last) == vec![last + 1, last + 2]
                    && s(&hold_of(&both, last + 1), "/outbox_state") == "queued"
                    && s(&hold_of(&both, last + 2), "/outbox_state") == "queued",
                json!({"a": {"sequence": sequence_a, "request_id": request_a, "request_digest": digest_a, "abort_code": a.get("abort_code")},
                       "b": {"sequence": sequence_b, "request_id": request_b, "request_digest": digest_b, "abort_code": b.get("abort_code")},
                       "entries_after": entries_after(&both, last)}),
            );
            let a_final = judge.load("08-a-finalized");
            let after_a = judge.load("09-after-a");
            let a_filled = s(&a_final, "/entry/reconciliation/status") == "consumed";
            judge.check(
                "the first request replayed from its stored snapshot filled and settled canonically",
                a_final.get("reached") == Some(&Value::Bool(true)) && a_filled
                    && s(&a_final, "/entry/outbox_state") == "settled",
                json!({"entry": a_final.get("entry")}),
            );
            // Which pool each settlement drew from comes from the gateway's
            // DeFMI journal: the replay ticker re-registers pools between the
            // two rounds, so the node bindings in the later snapshots no longer
            // name the first request's pool.
            let journal_a = judge.load("11b-journal-1");
            let journal_b = judge.load("11b-journal-2");
            let pool_a = s(&journal_a, "/params/allocation/poolID");
            let pool_a_before = u(&journal_a, "/params/allocation/expectedPoolSequence");
            let remainder_a = s(&journal_a, "/params/allocation/remainderNote/noteID");
            let pool_a_now = judge.load("11c-pool-a");
            judge.check(
                "the first request's settlement allocated from one pool at the sequence it expected, and that pool's canonical current note is the remainder that allocation created",
                !pool_a.is_empty()
                    && s(&pool_a_now, "/pool_id") == pool_a
                    && s(&pool_a_now, "/pool/currentPoolNoteID") == remainder_a
                    && u(&pool_a_now, "/pool/sequence")
                        .zip(pool_a_before)
                        .is_some_and(|(now, before)| now == before + 1),
                json!({"pool_id": pool_a, "expected_pool_sequence_at_allocation": pool_a_before, "remainder_note_id": remainder_a, "canonical_now": pool_a_now.get("pool")}),
            );
            let gateway_log = judge.text("gateway.log");
            let refused_over_pool = gateway_log.lines().any(|line| {
                line.contains("pre-trade reserves do not cover")
                    || line.contains("does not conserve")
                    || line.contains("remainder")
            });
            let b_final = judge.load("10-b-finalized");
            let after_b = judge.load("11-after-b");
            let b_later = judge.load("12-b-later");
            let after_later = judge.load("13-after-b-later");
            let b_hold = hold_of(&after_later, last + 2);
            let b_state = s(&b_hold, "/outbox_state");
            let b_status = s(&b_hold, "/reconciliation/status");
            let b_filled = b_status == "consumed";
            let pool_b = s(&journal_b, "/params/allocation/poolID");
            let pool_b_before = u(&journal_b, "/params/allocation/expectedPoolSequence");
            // An honest gateway never presents an over-pool allocation: its
            // reserve mirror refuses the match and the next round re-escrows
            // the Maker's pool from its remaining canonical notes under a new
            // pool id; and a request whose signed venue admission sequence
            // another admission consumed is refused by the L1 before any
            // reserve.  So the second request either fails closed, or settles
            // from a different pool; it may never settle from the drained one.
            judge.check(
                "the second request never settled from the pool the first one drained: it failed closed (refused by the L1 or the gateway before any reserve, or released), or it settled from a different, freshly escrowed pool after the gateway refused the over-pool match",
                (!b_filled && journal_b.is_null())
                    || (b_filled
                        && !pool_b.is_empty()
                        && pool_b != pool_a
                        && pool_b_before == Some(0)
                        && refused_over_pool),
                json!({"second_filled": b_filled, "second_state": b_state, "second_status": b_status, "second_pool": pool_b, "second_pool_sequence_at_allocation": pool_b_before,
                       "first_pool": pool_a, "gateway_refused_over_pool_match": refused_over_pool,
                       "final_hold": b_hold, "b_finalized_record": b_final.get("entry"), "b_later": b_later.get("entry")}),
            );
            judge.check(
                "the pool the first request drained kept the sequence its allocation left: nothing was allocated from it for the second request",
                u(&pool_a_now, "/pool/sequence")
                    .zip(pool_a_before)
                    .is_some_and(|(now, before)| now == before + 1)
                    && s(&pool_a_now, "/pool/currentPoolNoteID") == remainder_a,
                json!({"pool_id": pool_a, "canonical_after_second": pool_a_now.get("pool"), "pools_bound_after_first": pool_sequences(&after_a), "pools_bound_after_second": pool_sequences(&after_b), "pools_bound_later": pool_sequences(&after_later)}),
            );
            let guard_line = gateway_log
                .lines()
                .filter(|line| {
                    line.contains("round")
                        || line.contains("pool")
                        || line.contains("remainder")
                        || line.contains("reconciliation")
                        || line.contains("rejected")
                        || line.contains("refused")
                        || line.contains("reserves")
                        || line.contains("sequence")
                })
                .map(str::to_string)
                .collect::<Vec<_>>();
            judge.check(
                "the gateway recorded what happened to the second request (its log; the verdicts above do not depend on it)",
                !guard_line.is_empty(),
                json!({"gateway_log_lines": guard_line}),
            );
            judge.check(
                "the Taker's facility figures show no outstanding reservation once both requests are final",
                u(&after_later, "/taker/portfolio/reserved_cash") == Some(0)
                    && u(&after_later, "/taker/portfolio/reserved_inventory") == Some(0),
                json!({"facility_before": taker_balances(&policies), "facility_after": taker_balances(&after_later),
                       "projection_at_signing": {"a": projected(&a, "before", 0), "b": projected(&b, "before", 0)}}),
            );
        }
        "pool-replay" => {
            let policies = judge.load("02-policies");
            let last = u(&policies, "/next_sequence")
                .unwrap_or(0)
                .saturating_sub(1);
            let rfq = judge.load("03-rfq");
            let waited = judge.load("04-wait");
            let (sequence, ..) = identity(&waited);
            let filled = s(&waited, "/entry/reconciliation/status") == "consumed";
            judge.check(
                "one request settled from the Maker pool through the normal path",
                rfq.get("aborted") == Some(&Value::Bool(false)) && filled && sequence == last + 1,
                json!({"round": rfq.get("round_number"), "entry": waited.get("entry")}),
            );
            let settled = judge.load("05-after-settle");
            let fill = judge_advanced_once(
                &mut judge,
                "after the settlement",
                &policies,
                &settled,
                filled,
                sequence,
            );
            let winner = fill
                .pointer("/fill/pools_matched/0")
                .cloned()
                .unwrap_or(Value::Null);
            let winner_id = s(&winner, "/pool_id");
            let winner_sequence = u(&winner, "/sequence");
            let winner_note = s(&winner, "/remainder_note_id");
            // The allocation the gateway issued for that settlement, taken
            // verbatim from its journal, names the pool, the sequence it
            // expected and the parent note it consumed.
            let journal = judge.load("06-journaled-allocation");
            let journaled_pool = s(&journal, "/params/allocation/poolID");
            let journaled_previous = s(&journal, "/params/allocation/previousPoolNoteID");
            let journaled_remainder = s(&journal, "/params/allocation/remainderNote/noteID");
            let journaled_sequence = u(&journal, "/params/allocation/expectedPoolSequence");
            judge.check(
                "the journaled allocation is the one the settlement applied: same pool, its remainder is the pool's current note, and it expected the sequence the pool had before",
                !winner_id.is_empty()
                    && journaled_pool == winner_id
                    && journaled_remainder == winner_note
                    && journaled_sequence.zip(winner_sequence).is_some_and(|(expected, now)| expected + 1 == now)
                    && !journaled_previous.is_empty() && journaled_previous != journaled_remainder,
                json!({"journaled": {"pool_id": journaled_pool, "expected_pool_sequence": journaled_sequence, "previous_pool_note_id": journaled_previous, "remainder_note_id": journaled_remainder},
                       "winning_pool": winner}),
            );
            let original_height = u(
                &waited,
                "/entry/reconciliation/canonical_receipt/ledger_height",
            );
            let replay = judge.load("07-replay");
            judge.check(
                "re-presenting the identical accepted transition applied nothing: the L1 recognised the transaction it already holds (same id, the original height) and the state root did not move",
                replay.get("deduplicated_by_transaction_id") == Some(&Value::Bool(true))
                    && replay.get("state_root_unchanged") == Some(&Value::Bool(true))
                    && u(&replay, "/final_status/height") == original_height,
                json!({"tx_id": replay.get("tx_id"), "final_status": replay.get("final_status"), "original_height": original_height, "state_root_unchanged": replay.get("state_root_unchanged")}),
            );
            let fresh = judge.load("07b-replay-fresh-root");
            let reason = s(&fresh, "/rejection_reason");
            judge.check(
                "the same allocation under a fresh expected root is a new transaction, and the L1 refused it before any state change: the k-of-n approval binds the expected root, so the re-rooted transition fails the approval check that stands ahead of the pool-note compare-and-swap",
                fresh.get("submitted") == Some(&Value::Bool(true))
                    && fresh.get("rejected") == Some(&Value::Bool(true))
                    && fresh.get("state_root_unchanged") == Some(&Value::Bool(true)),
                json!({"tx_id": fresh.get("tx_id"), "final_status": fresh.get("final_status"), "rejection_reason": reason, "params_summary": fresh.get("params_summary"), "params_digest": fresh.get("params_digest")}),
            );
            let guard = judge.load("07c-pool-guard");
            judge.check(
                "an allocation built against the live state root but taking more than the pool's parent note holds was rejected by the canonical VM's standing-pool guard, before the approval or the root were checked, and moved no state",
                guard.get("expected_before_root_is_live") == Some(&Value::Bool(true))
                    && guard.get("rejected") == Some(&Value::Bool(true))
                    && guard.get("rejected_by_pool_conservation_guard") == Some(&Value::Bool(true))
                    && guard.get("state_root_unchanged") == Some(&Value::Bool(true)),
                json!({"method": guard.get("method"), "expected_before_root": guard.get("expected_before_root"), "pool_id": guard.get("pool_id"),
                       "rejection_reason": guard.get("rejection_reason"), "final_status": guard.get("final_status"),
                       "original_remainder_note_id": guard.get("original_remainder_note_id"), "inflated_remainder_note_id": guard.get("inflated_remainder_note_id")}),
            );
            let cas = judge.load("07d-pool-cas");
            judge.check(
                "a conserving standing-pool allocation, freshly re-approved by a genuine 3-of-7 development-committee quorum over the live state root but naming the pool sequence/current-note the winning fill already advanced past, was rejected by the DeFMI canonical compare-and-swap (\"standing allocation is stale or outside its Maker mandate\"); a one-byte-corrupted approval failed earlier at the k-of-n approval gate and a pre-fill root failed at the root gate, so the compare-and-swap itself is what fails the otherwise valid transaction; and no hold, settlement, pool sequence or state root advanced",
                cas.pointer("/pointer_is_stale_vs_canonical") == Some(&Value::Bool(true))
                    && cas.pointer("/cas/rejected_by_pool_sequence_current_note_cas") == Some(&Value::Bool(true))
                    && cas.pointer("/bad_approval_control/rejected_by_approval_gate") == Some(&Value::Bool(true))
                    && cas.pointer("/stale_root_control/rejected_by_root_gate") == Some(&Value::Bool(true))
                    && cas.pointer("/state_root_unchanged") == Some(&Value::Bool(true))
                    && cas.pointer("/pool_sequence_unchanged") == Some(&Value::Bool(true))
                    && cas.pointer("/pool_current_note_unchanged") == Some(&Value::Bool(true)),
                json!({"pool_id": cas.get("pool_id"), "allocation_expected_pool_sequence": cas.get("allocation_expected_pool_sequence"),
                       "allocation_previous_pool_note_id": cas.get("allocation_previous_pool_note_id"), "pool_state_before": cas.get("pool_state_before"),
                       "signer_subset": cas.get("signer_subset"), "fresh_hold_id": cas.get("fresh_hold_id"), "fresh_reserve_nullifier": cas.get("fresh_reserve_nullifier"),
                       "cas": cas.get("cas"), "bad_approval_control": cas.get("bad_approval_control"), "stale_root_control": cas.get("stale_root_control"),
                       "state_root_unchanged": cas.get("state_root_unchanged"), "pool_state_after": cas.get("pool_state_after")}),
            );
            let after_replay = judge.load("08-after-replay");
            judge.check(
                "after both re-presentations nothing moved: the pool keeps its sequence and current note, no outbox entry or hold appeared, the state root is unchanged and every node kept its generation",
                pool_sequences(&settled) == pool_sequences(&after_replay)
                    && s(&settled, "/defmi/state_root/stateRoot") == s(&after_replay, "/defmi/state_root/stateRoot")
                    && entries_after(&after_replay, sequence).is_empty()
                    && node_generations(&settled) == node_generations(&after_replay)
                    && after_replay.get("pools").and_then(Value::as_array).is_some_and(|pools| pools.iter().any(|pool| s(pool, "/pool_id") == winner_id && s(pool, "/pool/currentPoolNoteID") == winner_note)),
                json!({"pools_before": pool_sequences(&settled), "pools_after": pool_sequences(&after_replay),
                       "state_root_before": s(&settled, "/defmi/state_root/stateRoot"), "state_root_after": s(&after_replay, "/defmi/state_root/stateRoot"),
                       "generations_before": node_generations(&settled), "generations_after": node_generations(&after_replay)}),
            );
        }
        "restart-all" => {
            let restarted = judge.load("01-after-restart");
            judge.check(
                "after restarting DeFMI, the seven nodes and the gateway every node is healthy with its generation intact",
                node_health(&restarted).iter().all(|ok| *ok) && node_health(&restarted).len() == 7
                    && node_generations(&restarted) == node_generations(&before)
                    && pool_sequences(&restarted) == pool_sequences(&before)
                    && s(&restarted, "/gateway/status") == "200",
                json!({"health": node_health(&restarted), "generations": node_generations(&restarted), "pools": pool_sequences(&restarted), "gateway": restarted.get("gateway")}),
            );
            let rfq = judge.load("02-rfq");
            let waited = judge.load("03-wait");
            let (sequence, ..) = identity(&waited);
            judge.check(
                "a fresh RFQ after the restart settled through DeFMI",
                rfq.get("aborted") == Some(&Value::Bool(false))
                    && s(&waited, "/entry/reconciliation/status") == "consumed",
                json!({"round": rfq.get("round_number"), "verified": rfq.get("verified"), "settlement": rfq.get("settlement"), "entry": waited.get("entry")}),
            );
            judge_advanced_once(
                &mut judge,
                "after restart",
                &restarted,
                &after,
                s(&waited, "/entry/reconciliation/status") == "consumed",
                sequence,
            );
        }
        other => return Err(format!("unknown scenario {other}")),
    }
    // Global invariant on the final snapshot: every settled entry has one
    // DeFMI hold, consumed by the transition its receipt names, and no two
    // entries share a hold.
    let mut holds = BTreeSet::new();
    let mut duplicates = Vec::new();
    let mut mismatched = Vec::new();
    for hold in after
        .get("holds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let id = s(&hold, "/reconciliation/hold_id");
        if !id.is_empty() && !holds.insert(id.clone()) {
            duplicates.push(id.clone());
        }
        let state = s(&hold, "/outbox_state");
        let status = s(&hold, "/reconciliation/status");
        let tx = s(&hold, "/reconciliation/canonical_receipt/transaction_id");
        let digest = s(&hold, "/defmi_reservation/settlementDigest");
        let consistent = match state.as_str() {
            "settled" => status == "consumed" && tx == digest,
            "released" => status == "released" && tx == digest,
            "aborted_before_reserve" => status == "aborted_before_reserve",
            _ => true,
        };
        if !consistent {
            mismatched.push(json!({"sequence": hold.get("sequence"), "state": state, "status": status, "tx": tx, "digest": digest}));
        }
    }
    judge.check(
        "final snapshot: one hold per request, and every settled or released entry names the DeFMI transition that closed its hold",
        duplicates.is_empty() && mismatched.is_empty() && !after.is_null(),
        json!({"duplicate_holds": duplicates, "mismatched": mismatched, "entries": after.pointer("/taker/outbox_metrics")}),
    );
    let pass = judge
        .checks
        .iter()
        .all(|check| check.get("pass") == Some(&Value::Bool(true)));
    Ok(json!({
        "recorded_at": unix_now(),
        "scenario": scenario,
        "pass": pass,
        "checks": judge.checks,
    }))
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let result = match args.command.as_str() {
        "snapshot" => cmd_snapshot(&args),
        "rfq" => cmd_rfq(&args),
        "policy" => cmd_policy(&args),
        "wait" => cmd_wait(&args),
        "view" => cmd_view(&args),
        "judge" => cmd_judge(&args),
        "report" => report::cmd_report(&args),
        "defmi-rpc" => cmd_defmi_rpc(&args),
        "pool" => cmd_pool(&args),
        "pool-guard-probe" => cmd_pool_guard_probe(&args),
        "pool-cas-probe" => cmd_pool_cas_probe(&args),
        "force" => cmd_force(&args),
        other => Err(format!("unknown command {other}\n{USAGE}")),
    };
    let (document, code) = match result {
        Ok(value) => (value, ExitCode::SUCCESS),
        Err(error) => (
            json!({"recorded_at": unix_now(), "command": args.command, "error": error}),
            ExitCode::from(1),
        ),
    };
    let text = serde_json::to_string_pretty(&document).unwrap_or_default();
    if let Some(path) = args.options.get("out") {
        if let Err(error) = fs::write(path, format!("{text}\n")) {
            eprintln!("could not write {path}: {error}");
            return ExitCode::from(1);
        }
    }
    println!("{text}");
    // `wait` reports whether the condition was reached in the document; a
    // caller that wants a non-zero exit on timeout checks `reached`.
    if args.command == "wait" && document.get("reached") == Some(&Value::Bool(false)) {
        return ExitCode::from(3);
    }
    if args.command == "report" && document.get("pass") == Some(&Value::Bool(false)) {
        return ExitCode::from(1);
    }
    code
}
