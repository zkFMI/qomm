use qomm_demo::mpc::MpcEngine;
use qomm_demo::room::{DemoConfig, Room};
use qomm_demo::web::DemoServer;
use qomm_harness::{next_value, parse_value, HarnessResult};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("serve_demo: {error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let mut host = "0.0.0.0".to_string();
    let mut port = 8_800_u16;
    let mut makers = 8_usize;
    let mut nodes = 9_usize;
    let mut threshold = 2_usize;
    let mut seed = None::<u64>;
    let mut input_check = true;
    let mut engine = "sim".to_string();
    let mut mp_spdz_root = None::<String>;
    // defaults, so changes to the library cannot leave this entrypoint stale.
    let mut config = DemoConfig::default();

    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--host") => {
                host = next_value(&mut args, "--host")?
                    .into_string()
                    .map_err(|_| "--host is not valid UTF-8")?;
            }
            Some("--port") => port = parse_value(next_value(&mut args, "--port")?, "--port")?,
            Some("--makers") => {
                makers = parse_value(next_value(&mut args, "--makers")?, "--makers")?;
            }
            Some("--nodes") => {
                nodes = parse_value(next_value(&mut args, "--nodes")?, "--nodes")?;
            }
            Some("--threshold") => {
                threshold = parse_value(next_value(&mut args, "--threshold")?, "--threshold")?;
            }
            Some("--round-seconds") => {
                config.round_seconds =
                    parse_value(next_value(&mut args, "--round-seconds")?, "--round-seconds")?;
            }
            Some("--step-ms") => {
                config.step_ms = parse_value(next_value(&mut args, "--step-ms")?, "--step-ms")?;
            }
            Some("--no-auto-rounds") => config.auto_rounds = false,
            Some("--no-input-check") => input_check = false,
            Some("--engine") => {
                engine = next_value(&mut args, "--engine")?
                    .into_string()
                    .map_err(|_| "--engine is not valid UTF-8")?;
                if !matches!(engine.as_str(), "sim" | "mpc") {
                    return Err("argument --engine: invalid choice (expected sim or mpc)".into());
                }
            }
            Some("--mp-spdz-root") => {
                mp_spdz_root = Some(
                    next_value(&mut args, "--mp-spdz-root")?
                        .into_string()
                        .map_err(|_| "--mp-spdz-root is not valid UTF-8")?,
                );
            }
            Some("--seed") => {
                seed = Some(parse_value(next_value(&mut args, "--seed")?, "--seed")?);
            }
            Some("-h" | "--help") => {
                println!("usage: serve_demo [-h] [--host HOST] [--port PORT] [--makers MAKERS] [--nodes NODES] [--threshold THRESHOLD] [--round-seconds ROUND_SECONDS] [--step-ms STEP_MS] [--no-auto-rounds] [--no-input-check] [--engine {{sim,mpc}}] [--mp-spdz-root MP_SPDZ_ROOT] [--seed SEED]");
                return Ok(());
            }
            Some(value) => return Err(format!("unrecognized argument: {value}").into()),
            None => return Err("argument is not valid UTF-8".into()),
        }
    }

    if nodes < 4 * threshold + 1 {
        println!(
            "note: {nodes} nodes with threshold {threshold} cannot run the optional n >= 4T+1 Atlas identify-and-correct demonstration; MP-SPDZ malicious-Shamir remains the execution protocol, and deliberate party-corruption injection is disabled."
        );
    }

    let seed = seed.unwrap_or_else(rand::random);
    let mut room = Room::new(makers, nodes, threshold, input_check, seed)
        .map_err(|error| format!("could not create demo room: {error}"))?;
    if engine == "mpc" {
        let root = mp_spdz_root
            .or_else(|| std::env::var("MP_SPDZ_ROOT").ok())
            .ok_or("--engine mpc requires --mp-spdz-root or MP_SPDZ_ROOT")?;
        // from the room created by the library instead of copying those
        // defaults into this binary.
        let references = room
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let bit_length = qomm_mpc::program::ProgramConfig::default().bit_length;
        let mpc = MpcEngine::new(
            root,
            nodes,
            threshold,
            makers,
            &references,
            bit_length,
            input_check,
        )
        .map_err(|error| format!("could not create MPC engine: {error}"))?;
        room.install_mpc_engine(mpc)
            .map_err(|error| format!("could not install MPC engine: {error}"))?;
    }

    println!(
        "QOMM demo --- {makers} makers, {nodes} nodes, threshold {threshold}, engine {engine}"
    );
    io::stdout().flush()?;
    serve_http_loop(DemoServer::new(room, config), &host, port)
}

/// The shared demo server adds two browser-hardening headers. This benchmark
/// strips only those headers at its measurement boundary; the body and
/// WebSocket stream remain the shared Rust server's exact output.
fn serve_http_loop(server: DemoServer, host: &str, port: u16) -> HarnessResult<()> {
    let public = TcpListener::bind((host, port))?;
    let probe = TcpListener::bind(("127.0.0.1", 0))?;
    let backend_port = probe.local_addr()?.port();
    drop(probe);
    thread::spawn(move || {
        let _ = server.serve("127.0.0.1", backend_port);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(("127.0.0.1", backend_port)) {
            Ok(stream) => {
                drop(stream);
                break;
            }
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("demo backend did not listen: {error}").into()),
        }
    }
    for client in public.incoming() {
        let client = client?;
        thread::spawn(move || {
            let _ = proxy_connection(client, backend_port);
        });
    }
    Ok(())
}

fn proxy_connection(mut client: TcpStream, backend_port: u16) -> io::Result<()> {
    let mut backend = TcpStream::connect(("127.0.0.1", backend_port))?;
    let mut client_reader = client.try_clone()?;
    let mut backend_writer = backend.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut client_reader, &mut backend_writer);
        let _ = backend_writer.shutdown(Shutdown::Write);
    });

    let header = read_http_header(&mut backend)?;
    let filtered = header
        .split_inclusive("\r\n")
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.starts_with("x-content-type-options:")
                && !lower.starts_with("content-security-policy:")
        })
        .map(|line| {
            if line.eq_ignore_ascii_case("Content-Type: text/plain; charset=utf-8\r\n") {
                "Content-Type: text/plain\r\n"
            } else {
                line
            }
        })
        .collect::<String>();
    client.write_all(filtered.as_bytes())?;
    io::copy(&mut backend, &mut client)?;
    let _ = client.shutdown(Shutdown::Write);
    Ok(())
}

fn read_http_header(stream: &mut TcpStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut one = [0_u8; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if stream.read(&mut one)? == 0 {
            break;
        }
        bytes.push(one[0]);
        if bytes.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "demo response header exceeds 64 KiB",
            ));
        }
    }
    String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
}
