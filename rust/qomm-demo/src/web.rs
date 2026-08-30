//! Small RFC 6455/HTTP server for the embedded browser demo.

use crate::protocol::HONEST;
use crate::room::{DemoConfig, Room, MAKER, NODE, OBSERVER, TAKER};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use openssl::sha::sha1;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const CONT: u8 = 0x0;
pub const TEXT: u8 = 0x1;
pub const BINARY: u8 = 0x2;
pub const CLOSE: u8 = 0x8;
pub const PING: u8 = 0x9;
pub const PONG: u8 = 0xa;
pub const MAX_FRAME: usize = 1 << 20;
const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub fn server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x80 | opcode];
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else if payload.len() < (1 << 16) {
        out.push(126);
        out.extend((payload.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend((payload.len() as u64).to_be_bytes());
    }
    out.extend(payload);
    out
}

pub fn read_client_message(reader: &mut impl Read) -> Result<(u8, Vec<u8>), String> {
    let mut message_opcode = None;
    let mut body = Vec::new();
    loop {
        let mut header = [0_u8; 2];
        reader
            .read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let final_frame = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0f;
        if header[0] & 0x70 != 0 || header[1] & 0x80 == 0 {
            return Err("a client frame must be masked and use no unnegotiated extension".into());
        }
        let mut length = u64::from(header[1] & 0x7f);
        if length == 126 {
            let mut bytes = [0_u8; 2];
            reader
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            length = u64::from(u16::from_be_bytes(bytes));
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            reader
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            length = u64::from_be_bytes(bytes);
        }
        if length > MAX_FRAME as u64 || body.len().saturating_add(length as usize) > MAX_FRAME {
            return Err(format!("frame of {length} bytes refused"));
        }
        if opcode >= CLOSE && (!final_frame || length > 125) {
            return Err("invalid WebSocket control frame".into());
        }
        let mut key = [0_u8; 4];
        reader
            .read_exact(&mut key)
            .map_err(|error| error.to_string())?;
        let mut chunk = vec![0_u8; length as usize];
        reader
            .read_exact(&mut chunk)
            .map_err(|error| error.to_string())?;
        for (index, byte) in chunk.iter_mut().enumerate() {
            *byte ^= key[index & 3];
        }
        if matches!(opcode, PING | PONG | CLOSE) {
            return Ok((opcode, chunk));
        }
        if message_opcode.is_none() {
            if !matches!(opcode, TEXT | BINARY) {
                return Err("continuation without an initial frame".into());
            }
            message_opcode = Some(opcode);
        } else if opcode != CONT {
            return Err("fragmented message used a non-continuation opcode".into());
        }
        body.extend(chunk);
        if final_frame {
            return Ok((message_opcode.expect("message opcode"), body));
        }
    }
}

pub fn websocket_handshake(headers: &BTreeMap<String, String>) -> Option<Vec<u8>> {
    let key = headers.get("sec-websocket-key")?;
    if !headers
        .get("upgrade")
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
    {
        return None;
    }
    let mut bytes = key.as_bytes().to_vec();
    bytes.extend(GUID);
    let accept = BASE64.encode(sha1(&bytes));
    Some(
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
        )
        .into_bytes(),
    )
}

pub fn http_reply(status: &str, body: &[u8], kind: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; connect-src 'self' ws: wss:; script-src 'self'; style-src 'self'\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend(body);
    out
}

pub fn unquote(text: &str) -> String {
    let raw = text.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' && index + 2 < raw.len() {
            if let Some(value) = std::str::from_utf8(&raw[index + 1..index + 3])
                .ok()
                .and_then(|value| u8::from_str_radix(value, 16).ok())
            {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(raw[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(raw: &str) -> BTreeMap<String, String> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            (unquote(name), unquote(value))
        })
        .collect()
}

pub fn static_response(path: &str) -> Vec<u8> {
    let path = unquote(path);
    if path.contains("..") || path.contains('\\') || path.bytes().any(|byte| byte == 0) {
        return http_reply(
            "404 Not Found",
            b"no such page",
            "text/plain; charset=utf-8",
        );
    }
    match path.as_str() {
        "" | "/" | "/index.html" => http_reply(
            "200 OK",
            include_bytes!("../../../qomm_demo/static/index.html"),
            "text/html; charset=utf-8",
        ),
        "/demo.css" => http_reply(
            "200 OK",
            include_bytes!("../../../qomm_demo/static/demo.css"),
            "text/css; charset=utf-8",
        ),
        "/demo.js" => http_reply(
            "200 OK",
            include_bytes!("../../../qomm_demo/static/demo.js"),
            "application/javascript; charset=utf-8",
        ),
        _ => http_reply(
            "404 Not Found",
            b"no such page",
            "text/plain; charset=utf-8",
        ),
    }
}

type Connections = Arc<Mutex<BTreeMap<String, Arc<Mutex<TcpStream>>>>>;

pub struct DemoServer {
    room: Arc<Mutex<Room>>,
    config: Arc<Mutex<DemoConfig>>,
    connections: Connections,
    next_round: Arc<Mutex<Instant>>,
}

impl DemoServer {
    pub fn new(room: Room, config: DemoConfig) -> Self {
        let next = Instant::now() + Duration::from_secs_f64(config.round_seconds);
        Self {
            room: Arc::new(Mutex::new(room)),
            config: Arc::new(Mutex::new(config)),
            connections: Arc::new(Mutex::new(BTreeMap::new())),
            next_round: Arc::new(Mutex::new(next)),
        }
    }

    pub fn serve(self, host: &str, port: u16) -> Result<(), String> {
        let listener = TcpListener::bind((host, port)).map_err(|error| error.to_string())?;
        println!(
            "QOMM Rust demo listening at http://{}/",
            listener.local_addr().map_err(|error| error.to_string())?
        );
        self.start_ticker();
        let server = Arc::new(self);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let server = Arc::clone(&server);
                    thread::spawn(move || {
                        let _ = server.handle(stream);
                    });
                }
                Err(error) => eprintln!("demo accept failed: {error}"),
            }
        }
        Ok(())
    }

    fn start_ticker(&self) {
        let room = Arc::clone(&self.room);
        let config = Arc::clone(&self.config);
        let connections = Arc::clone(&self.connections);
        let next_round = Arc::clone(&self.next_round);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(500));
            // A person in the taker seat decides when to ask; the clock only
            // moves the room while nobody is doing that, and never while a
            // round is still being shown.
            let due = {
                let config = config.lock().expect("demo config lock").clone();
                let room = room.lock().expect("demo room lock");
                config.auto_rounds
                    && !room.busy
                    && room.seats[TAKER].mode() != "manual"
                    && Instant::now() >= *next_round.lock().expect("next round lock")
            };
            if due {
                let _ = play_round(&room, &config, &connections, &next_round);
            } else {
                broadcast(&room, &config, &connections, &next_round);
            }
        });
    }

    fn reset_deadline(&self) {
        let seconds = self.config.lock().expect("demo config lock").round_seconds;
        *self.next_round.lock().expect("next round lock") =
            Instant::now() + Duration::from_secs_f64(seconds);
    }

    fn handle(&self, mut stream: TcpStream) -> Result<(), String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|error| error.to_string())?;
        let request = read_headers(&mut stream)?;
        let text = String::from_utf8_lossy(&request);
        let mut lines = text.split("\r\n");
        let first = lines
            .next()
            .ok_or_else(|| "empty HTTP request".to_string())?;
        let mut fields = first.split_whitespace();
        let method = fields.next().unwrap_or("");
        let target = fields.next().unwrap_or("");
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect::<BTreeMap<_, _>>();
        if method != "GET" {
            stream
                .write_all(&http_reply("405 Method Not Allowed", b"", "text/plain"))
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != "/ws" {
            stream
                .write_all(&static_response(path))
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        let Some(handshake) = websocket_handshake(&headers) else {
            stream
                .write_all(&http_reply(
                    "400 Bad Request",
                    b"not a websocket",
                    "text/plain",
                ))
                .map_err(|error| error.to_string())?;
            return Ok(());
        };
        stream
            .write_all(&handshake)
            .map_err(|error| error.to_string())?;
        stream.flush().map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(None)
            .map_err(|error| error.to_string())?;
        let query = parse_query(query);
        let session = {
            let mut room = self.room.lock().expect("demo room lock");
            let session = query
                .get("session")
                .filter(|value| !value.is_empty())
                .cloned()
                .unwrap_or_else(|| room.new_session());
            if let Some(seat) = query.get("seat") {
                room.claim(
                    &session,
                    seat,
                    query.get("label").map(String::as_str).unwrap_or(""),
                );
            }
            session
        };
        let writer = Arc::new(Mutex::new(
            stream.try_clone().map_err(|error| error.to_string())?,
        ));
        self.connections
            .lock()
            .expect("demo connections lock")
            .insert(session.clone(), Arc::clone(&writer));
        self.broadcast();
        while let Ok((opcode, payload)) = read_client_message(&mut stream) {
            match opcode {
                CLOSE => break,
                PING => {
                    let _ = writer
                        .lock()
                        .expect("demo writer lock")
                        .write_all(&server_frame(PONG, &payload));
                }
                TEXT => {
                    if let Ok(message) = serde_json::from_slice::<Value>(&payload) {
                        self.message(&session, &writer, &message);
                    }
                }
                _ => {}
            }
        }
        let removed = {
            let mut connections = self.connections.lock().expect("demo connections lock");
            if connections
                .get(&session)
                .is_some_and(|current| Arc::ptr_eq(current, &writer))
            {
                connections.remove(&session);
                true
            } else {
                false
            }
        };
        if removed {
            self.room.lock().expect("demo room lock").release(&session);
            self.broadcast();
        }
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    }

    fn broadcast(&self) {
        broadcast(
            &self.room,
            &self.config,
            &self.connections,
            &self.next_round,
        );
    }

    fn message(&self, session: &str, writer: &Arc<Mutex<TcpStream>>, message: &Value) {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        let mut room = self.room.lock().expect("demo room lock");
        let seat = room
            .seat_of(session)
            .map(|seat| (seat.kind.clone(), seat.index));
        let watching = room
            .sessions
            .get(session)
            .is_some_and(|seat| seat == OBSERVER);
        let mut action_error = None;
        let mut wants_round = false;
        match kind {
            "claim" => {
                let (ok, reason) = room.claim(
                    session,
                    message.get("seat").and_then(Value::as_str).unwrap_or(""),
                    message.get("label").and_then(Value::as_str).unwrap_or(""),
                );
                if !ok {
                    let payload = serde_json::to_vec(&json!({
                        "type": "refused",
                        "reason": reason
                    }))
                    .unwrap_or_default();
                    let _ = writer
                        .lock()
                        .expect("demo writer lock")
                        .write_all(&server_frame(TEXT, &payload));
                }
            }
            "release" => room.release(session),
            "policy" if seat.as_ref().is_some_and(|(kind, _)| kind == MAKER) => {
                if let Err(error) = room.set_policy(
                    seat.as_ref().expect("maker seat").1,
                    message.get("values").unwrap_or(&Value::Null),
                ) {
                    action_error = Some(error);
                }
            }
            "behaviour" if seat.as_ref().is_some_and(|(kind, _)| kind == NODE) => {
                if let Err(error) = room.set_behaviour(
                    seat.as_ref().expect("node seat").1,
                    message
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or(HONEST),
                ) {
                    action_error = Some(error);
                }
            }
            "request" if seat.as_ref().is_some_and(|(kind, _)| kind == TAKER) => {
                if let Err(error) = room.set_request(message.get("values").unwrap_or(&Value::Null))
                {
                    action_error = Some(error);
                }
            }
            "submit" if seat.as_ref().is_some_and(|(kind, _)| kind == TAKER) => {
                wants_round = true;
            }
            "submit_any" if watching => wants_round = true,
            "force" => room.set_forced_manual(
                message.get("seat").and_then(Value::as_str).unwrap_or(""),
                message
                    .get("manual")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            "config"
                if watching
                    || seat.is_none()
                    || seat.as_ref().is_some_and(|(kind, _)| kind == TAKER) =>
            {
                let values = message.get("values").unwrap_or(&Value::Null);
                let mut config = self.config.lock().expect("demo config lock");
                if let Some(value) = values.get("round_seconds").and_then(Value::as_f64) {
                    config.round_seconds = value.clamp(1.0, 120.0);
                }
                if let Some(value) = values.get("step_ms").and_then(Value::as_u64) {
                    config.step_ms = value.min(2_000);
                }
                if let Some(value) = values.get("auto_rounds").and_then(Value::as_bool) {
                    config.auto_rounds = value;
                }
                if let Some(value) = values.get("input_check").and_then(Value::as_bool) {
                    if let Err(error) = room.configure_input_check(value) {
                        action_error = Some(error);
                    }
                }
                drop(config);
                self.reset_deadline();
            }
            _ => {}
        }
        drop(room);
        if wants_round {
            // The round holds the room lock only while it computes and while
            // it moves between phases, so every connection keeps being served
            // the phase it is at.
            if let Err(error) = play_round(
                &self.room,
                &self.config,
                &self.connections,
                &self.next_round,
            ) {
                action_error = Some(error);
            }
        }
        if let Some(reason) = action_error {
            let payload = serde_json::to_vec(&json!({
                "type": "refused",
                "reason": reason,
            }))
            .unwrap_or_default();
            let _ = writer
                .lock()
                .expect("demo writer lock")
                .write_all(&server_frame(TEXT, &payload));
        }
        self.broadcast();
    }
}

fn read_headers(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while request.len() < 65_536 {
        stream
            .read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            return Ok(request);
        }
    }
    Err("HTTP headers exceeded 64 KiB".into())
}

/// One round, then a walk through what it did, slowly enough to watch.
///
/// The round is computed first and the phases are a replay of it.  Pausing
/// between real steps instead would make the pauses look like protocol time,
/// and they are not: the arithmetic takes milliseconds and the view reports
/// that separately from the pacing.  Each phase is broadcast to every
/// connection before the pause, and settlement --- the only step that moves
/// balances --- happens exactly when the `settle` phase is shown.
fn play_round(
    room: &Arc<Mutex<Room>>,
    config: &Arc<Mutex<DemoConfig>>,
    connections: &Connections,
    next_round: &Arc<Mutex<Instant>>,
) -> Result<(), String> {
    let phases = room.lock().expect("demo room lock").begin_round()?;
    let pause = || {
        let step_ms = config.lock().expect("demo config lock").step_ms;
        if step_ms > 0 {
            thread::sleep(Duration::from_millis(step_ms));
        }
    };
    for (index, phase) in phases.iter().enumerate() {
        if index > 0 {
            room.lock().expect("demo room lock").set_phase(phase);
        }
        broadcast(room, config, connections, next_round);
        pause();
    }
    let settled = room.lock().expect("demo room lock").finish_round();
    if settled.is_ok() {
        broadcast(room, config, connections, next_round);
        pause();
        room.lock().expect("demo room lock").end_round();
    }
    let seconds = config.lock().expect("demo config lock").round_seconds;
    *next_round.lock().expect("next round lock") =
        Instant::now() + Duration::from_secs_f64(seconds);
    broadcast(room, config, connections, next_round);
    settled
}

fn broadcast(
    room: &Arc<Mutex<Room>>,
    config: &Arc<Mutex<DemoConfig>>,
    connections: &Connections,
    next_round: &Arc<Mutex<Instant>>,
) {
    let connections = connections
        .lock()
        .expect("demo connections lock")
        .iter()
        .map(|(session, stream)| (session.clone(), Arc::clone(stream)))
        .collect::<Vec<_>>();
    let config = config.lock().expect("demo config lock").clone();
    let countdown = next_round
        .lock()
        .expect("next round lock")
        .saturating_duration_since(Instant::now())
        .as_secs_f64();
    let room = room.lock().expect("demo room lock");
    for (session, stream) in connections {
        if let Ok(payload) = serde_json::to_vec(&room.view(&session, &config, countdown)) {
            let _ = stream
                .lock()
                .expect("demo writer lock")
                .write_all(&server_frame(TEXT, &payload));
        }
    }
}
