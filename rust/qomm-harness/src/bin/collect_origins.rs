//! Rust port of `scripts/collect_origins.py`.

use qomm_harness::{next_value, parse_value, HarnessResult};
use qomm_transport::ethereum_rpc::{RpcClient, RpcResult};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

const DELEGATION_PREFIX: &str = "0xef0100";

struct Options {
    rpc: String,
    fills: PathBuf,
    out: PathBuf,
    threshold: usize,
    workers: usize,
    hub_limit: usize,
    cluster_only: bool,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let (keep, counts) = candidates(&options.fills, options.threshold)?;
    let kept_flow = keep
        .iter()
        .map(|address| counts.get(address).copied().unwrap_or(0))
        .sum::<usize>();
    let all_flow = counts.values().sum::<usize>();
    eprintln!(
        "{} swappers, {} with at least {} requests ({:.1}% of flow)",
        counts.len(),
        keep.len(),
        options.threshold,
        100.0 * kept_flow as f64 / all_flow.max(1) as f64,
    );

    let mut done = load_done(&options.out)?;
    if !done.is_empty() {
        eprintln!("{} already resolved", done.len());
    }
    if !options.cluster_only {
        let mut rpc = RpcClient::new(&options.rpc);
        let head = rpc
            .call("eth_blockNumber", json!([]))
            .map_err(|error| error.to_string())?
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or("eth_blockNumber returned no block number")?;
        let head = parse_hex_u64(&head)?;
        let todo = keep
            .iter()
            .filter(|address| !done.contains_key(*address))
            .cloned()
            .collect::<Vec<_>>();
        let rows = resolve_origins(&options.rpc, &todo, head, options.workers)?;
        if let Some(parent) = options
            .out
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&options.out)?;
        let started = Instant::now();
        for (index, row) in rows.into_iter().enumerate() {
            serde_json::to_writer(&mut output, &row)?;
            output.write_all(b"\n")?;
            let address = row["address"]
                .as_str()
                .ok_or("resolved origin row has no address")?
                .to_string();
            done.insert(address, row);
            if index % 500 == 0 {
                output.flush()?;
                let rate = (index + 1) as f64 / started.elapsed().as_secs_f64().max(1e-9);
                eprintln!(
                    "  {}/{} ({rate:.1}/s, ~{:.0} min left)",
                    index + 1,
                    todo.len(),
                    (todo.len() - index) as f64 / rate.max(1e-9) / 60.0,
                );
            }
        }
    }

    let rows = keep
        .iter()
        .filter_map(|address| done.get(address).cloned())
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&cluster(&rows, &counts, options.hub_limit)?)?
    );
    Ok(())
}

fn candidates(
    path: &Path,
    threshold: usize,
) -> HarnessResult<(Vec<String>, BTreeMap<String, usize>)> {
    let mut counts = BTreeMap::new();
    let mut order = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(&line)?;
        if row.get("checkpoint").is_some() {
            continue;
        }
        let address = row["swapper"]
            .as_str()
            .ok_or("fill row has no string swapper")?
            .to_string();
        if !counts.contains_key(&address) {
            order.push(address.clone());
        }
        *counts.entry(address).or_insert(0) += 1;
    }
    let keep = order
        .into_iter()
        .filter(|address| counts[address] >= threshold)
        .collect();
    Ok((keep, counts))
}

fn load_done(path: &Path) -> HarnessResult<BTreeMap<String, Value>> {
    let mut done = BTreeMap::new();
    let Ok(file) = File::open(path) else {
        return Ok(done);
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = match serde_json::from_str(&line) {
            Ok(row) => row,
            Err(_) => break,
        };
        let address = row["address"]
            .as_str()
            .ok_or("origin row has no string address")?
            .to_string();
        done.insert(address, row);
    }
    Ok(done)
}

fn resolve_origins(
    url: &str,
    addresses: &[String],
    head: u64,
    workers: usize,
) -> HarnessResult<Vec<Value>> {
    if addresses.is_empty() {
        return Ok(Vec::new());
    }
    let queue = Arc::new(Mutex::new(
        addresses
            .iter()
            .cloned()
            .enumerate()
            .collect::<VecDeque<_>>(),
    ));
    let (sender, receiver) = mpsc::channel();
    let mut handles = Vec::new();
    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let url = url.to_string();
        handles.push(thread::spawn(move || loop {
            let job = queue.lock().expect("origin queue lock").pop_front();
            let Some((index, address)) = job else {
                break;
            };
            let mut rpc = RpcClient::new(&url);
            let result = origin(&mut rpc, &address, head).map_err(|error| error.to_string());
            if sender.send((index, result)).is_err() {
                break;
            }
        }));
    }
    drop(sender);
    let mut ordered = vec![None; addresses.len()];
    let mut first_error = None;
    for (index, result) in receiver {
        match result {
            Ok(row) => ordered[index] = Some(row),
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    for handle in handles {
        handle.join().map_err(|_| "origin worker panicked")?;
    }
    if let Some(error) = first_error {
        return Err(error.into());
    }
    ordered
        .into_iter()
        .map(|row| row.ok_or_else(|| "origin worker returned no row".into()))
        .collect()
}

fn origin(rpc: &mut RpcClient, address: &str, head: u64) -> RpcResult<Value> {
    let code =
        rpc_string(rpc, "eth_getCode", json!([address, "latest"]))?.unwrap_or_else(|| "0x".into());
    let deployed = code != "0x" && !code.starts_with(DELEGATION_PREFIX);
    if deployed {
        let block = bisect(rpc, 0, head, |rpc, block| {
            Ok(
                rpc_string(rpc, "eth_getCode", json!([address, hex_block(block)]))?
                    .unwrap_or_else(|| "0x".into())
                    != "0x",
            )
        })?;
        if let Some(block) = block {
            for trace in rpc_array(rpc, "trace_block", json!([hex_block(block)]))? {
                let found = trace["type"] == "create"
                    && trace
                        .pointer("/result/address")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_lowercase()
                        == address;
                if found {
                    return Ok(json!({
                        "address": address,
                        "kind": "contract",
                        "block": block,
                        "origin": trace.pointer("/action/from").and_then(Value::as_str).unwrap_or_default().to_lowercase(),
                        "evidence": "deployer",
                    }));
                }
            }
        }
        return Ok(json!({
            "address": address,
            "kind": "contract",
            "block": block,
            "origin": null,
            "evidence": "deployer not found in the block's traces",
        }));
    }

    let kind = if code.starts_with(DELEGATION_PREFIX) {
        "delegated key"
    } else {
        "key"
    };
    let spent = bisect(rpc, 0, head, |rpc, block| {
        let value = rpc_string(
            rpc,
            "eth_getTransactionCount",
            json!([address, hex_block(block)]),
        )?
        .unwrap_or_else(|| "0x0".into());
        hex_is_positive(&value)
    })?;
    let balance_hi = spent.unwrap_or(head);
    let block = bisect(rpc, 0, balance_hi, |rpc, block| {
        let value = rpc_string(rpc, "eth_getBalance", json!([address, hex_block(block)]))?
            .unwrap_or_else(|| "0x0".into());
        hex_is_positive(&value)
    })?;
    let Some(block) = block else {
        return Ok(json!({
            "address": address,
            "kind": kind,
            "block": null,
            "origin": null,
            "evidence": "never held a balance",
        }));
    };
    for trace in rpc_array(rpc, "trace_block", json!([hex_block(block)]))? {
        let action = trace.get("action").unwrap_or(&Value::Null);
        let receiver = action
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let value = action.get("value").and_then(Value::as_str).unwrap_or("0x0");
        if receiver == address && hex_is_positive(value)? {
            return Ok(json!({
                "address": address,
                "kind": kind,
                "block": block,
                "origin": action.get("from").and_then(Value::as_str).unwrap_or_default().to_lowercase(),
                "evidence": "first funding",
            }));
        }
    }
    Ok(json!({
        "address": address,
        "kind": kind,
        "block": block,
        "origin": null,
        "evidence": "funding trace not found",
    }))
}

fn bisect<F>(
    rpc: &mut RpcClient,
    mut low: u64,
    mut high: u64,
    mut predicate: F,
) -> RpcResult<Option<u64>>
where
    F: FnMut(&mut RpcClient, u64) -> RpcResult<bool>,
{
    if !predicate(rpc, high)? {
        return Ok(None);
    }
    while low < high {
        let middle = low + (high - low) / 2;
        if predicate(rpc, middle)? {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    Ok(Some(low))
}

fn rpc_string(rpc: &mut RpcClient, method: &str, params: Value) -> RpcResult<Option<String>> {
    Ok(rpc
        .call(method, params)?
        .and_then(|value| value.as_str().map(ToOwned::to_owned)))
}

fn rpc_array(rpc: &mut RpcClient, method: &str, params: Value) -> RpcResult<Vec<Value>> {
    Ok(rpc
        .call(method, params)?
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default())
}

fn hex_block(block: u64) -> String {
    format!("0x{block:x}")
}

fn parse_hex_u64(value: &str) -> HarnessResult<u64> {
    Ok(u64::from_str_radix(
        value.strip_prefix("0x").unwrap_or(value),
        16,
    )?)
}

fn hex_is_positive(value: &str) -> RpcResult<bool> {
    let digits = value.strip_prefix("0x").unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("invalid hexadecimal quantity {value:?}").into());
    }
    Ok(digits.bytes().any(|byte| byte != b'0'))
}

fn cluster(
    rows: &[Value],
    counts: &BTreeMap<String, usize>,
    hub_limit: usize,
) -> HarnessResult<Value> {
    let mut by_origin: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        if let Some(origin) = row
            .get("origin")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let address = row["address"]
                .as_str()
                .ok_or("origin row has no string address")?;
            by_origin
                .entry(origin.to_string())
                .or_default()
                .push(address.to_string());
        }
    }
    let hubs = by_origin
        .iter()
        .filter(|(_, addresses)| addresses.len() > hub_limit)
        .map(|(origin, _)| origin.clone())
        .collect::<BTreeSet<_>>();
    let mut parent = BTreeMap::new();
    for (origin, addresses) in &by_origin {
        if hubs.contains(origin) {
            continue;
        }
        for other in addresses.iter().skip(1) {
            let left = find(&mut parent, &addresses[0]);
            let right = find(&mut parent, other);
            if left != right {
                parent.insert(left, right);
            }
        }
    }

    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut resolved = 0usize;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    for row in rows {
        let kind = row["kind"]
            .as_str()
            .ok_or("origin row has no string kind")?
            .to_string();
        *kinds.entry(kind).or_default() += 1;
        if row
            .get("origin")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            resolved += 1;
            let address = row["address"]
                .as_str()
                .ok_or("origin row has no string address")?;
            groups
                .entry(find(&mut parent, address))
                .or_default()
                .push(address.to_string());
        } else {
            let evidence = row["evidence"]
                .as_str()
                .ok_or("unresolved origin row has no evidence")?
                .to_string();
            *unresolved.entry(evidence).or_default() += 1;
        }
    }
    let multi = groups
        .values()
        .filter(|addresses| addresses.len() > 1)
        .collect::<Vec<_>>();
    let mut sizes = multi
        .iter()
        .map(|addresses| addresses.len())
        .collect::<Vec<_>>();
    sizes.sort_by(|left, right| right.cmp(left));
    let linked = sizes.iter().sum::<usize>();
    let requests_in_multi = multi
        .iter()
        .flat_map(|addresses| addresses.iter())
        .map(|address| counts.get(address).copied().unwrap_or(0))
        .sum::<usize>();
    let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0);
    Ok(json!({
        "addresses": rows.len(),
        "origin_resolved": resolved,
        "hub_origins_dropped": hubs.len(),
        "entities": groups.len(),
        "entities_with_more_than_one_wallet": multi.len(),
        "wallets_in_multi_wallet_entities": linked,
        "linkage_rho_estimate": linked as f64 / resolved.max(1) as f64,
        "requests_in_multi_wallet_entities": requests_in_multi,
        "median_wallets_per_multi_entity": median,
        "largest_entities": sizes.into_iter().take(15).collect::<Vec<_>>(),
        "kinds": kinds,
        "unresolved_reasons": unresolved,
    }))
}

fn find(parent: &mut BTreeMap<String, String>, value: &str) -> String {
    parent
        .entry(value.to_string())
        .or_insert_with(|| value.to_string());
    let mut root = value.to_string();
    while parent[&root] != root {
        root = parent[&root].clone();
    }
    let mut current = value.to_string();
    while parent[&current] != current {
        let next = parent[&current].clone();
        parent.insert(current, root.clone());
        current = next;
    }
    root
}

fn parse_args() -> HarnessResult<Options> {
    let mut rpc = "http://127.0.0.1:8545".to_string();
    let mut fills = None;
    let mut out = None;
    let mut threshold: usize = 3;
    let mut workers: usize = 4;
    let mut hub_limit: usize = 50;
    let mut cluster_only = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--rpc") => {
                rpc = next_value(&mut args, "--rpc")?
                    .into_string()
                    .map_err(|_| "--rpc is not valid UTF-8")?
            }
            Some("--fills") => fills = Some(PathBuf::from(next_value(&mut args, "--fills")?)),
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("--threshold") => {
                threshold = parse_value(next_value(&mut args, "--threshold")?, "--threshold")?
            }
            Some("--workers") => {
                workers = parse_value(next_value(&mut args, "--workers")?, "--workers")?
            }
            Some("--hub-limit") => {
                hub_limit = parse_value(next_value(&mut args, "--hub-limit")?, "--hub-limit")?
            }
            Some("--cluster-only") => cluster_only = true,
            Some("-h" | "--help") => {
                println!("usage: collect_origins --fills PATH --out PATH [--rpc URL] [--threshold N] [--workers N] [--hub-limit N] [--cluster-only]");
                std::process::exit(0);
            }
            _ => {
                return Err(
                    format!("unknown argument {}", OsString::from(arg).to_string_lossy()).into(),
                )
            }
        }
    }
    if workers == 0 {
        return Err("--workers must be positive".into());
    }
    Ok(Options {
        rpc,
        fills: fills.ok_or("--fills is required")?,
        out: out.ok_or("--out is required")?,
        threshold,
        workers,
        hub_limit,
        cluster_only,
    })
}
