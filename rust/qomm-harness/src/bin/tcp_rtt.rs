//! Median TCP connect time to a host:port, in milliseconds.

use qomm_harness::{next_value, parse_value, HarnessResult};
use std::ffi::OsString;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

fn resolve_ipv4(host: &str, port: u16) -> HarnessResult<SocketAddr> {
    (host, port)
        .to_socket_addrs()?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| format!("{host}:{port} did not resolve to an IPv4 address").into())
}

fn median(samples: &mut [f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f64::total_cmp);
    let middle = samples.len() / 2;
    Some(if samples.len().is_multiple_of(2) {
        (samples[middle - 1] + samples[middle]) / 2.0
    } else {
        samples[middle]
    })
}

fn main() -> HarnessResult<()> {
    let mut args = std::env::args_os().skip(1);
    let host = next_value(&mut args, "host")?
        .into_string()
        .map_err(|_| "host is not valid UTF-8")?;
    let port: u16 = parse_value(next_value(&mut args, "port")?, "port")?;
    let attempts: usize = args
        .next()
        .map(|value: OsString| parse_value(value, "attempts"))
        .transpose()?
        .unwrap_or(9);
    if args.next().is_some() {
        return Err("usage: tcp_rtt HOST PORT [ATTEMPTS]".into());
    }

    // socket.socket() defaults to AF_INET in Python, so do not silently switch
    // to an IPv6 address when a hostname resolves to both families.
    let address = resolve_ipv4(&host, port)?;
    let timeout = Duration::from_secs(5);
    let mut samples = Vec::with_capacity(attempts);
    for _ in 0..attempts {
        let start = Instant::now();
        if TcpStream::connect_timeout(&address, timeout).is_ok() {
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        }
    }

    match median(&mut samples) {
        Some(value) => println!("{value:.2}"),
        None => println!("nan"),
    }
    Ok(())
}
