use qomm_harness::{next_value, parse_value, HarnessResult};
use qomm_transport::ethereum_rpc::RpcClient;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const FILL_SIG: &str = "Fill(bytes32,address,address,uint256)";
const TRANSFER_TOPIC: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const NATIVE: &str = "0x0000000000000000000000000000000000000000";
const REACTORS: [&str; 2] = [
    "0x00000011f84b9aa48e5f8aa8b9897600006289be",
    "0x6000da47483062a0d734ba3dc7576ce6a0b645c4",
];
const CHUNK: i128 = 800;

struct Options {
    rpc: String,
    out: PathBuf,
    months: f64,
    from_block: Option<i128>,
    to_block: Option<i128>,
    checkpoint_every: i128,
    amounts: Option<PathBuf>,
    limit: i128,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut rpc = RpcClient::new(&options.rpc);
    if let Some(skeleton_path) = options.amounts.as_deref() {
        decode_amounts(&mut rpc, skeleton_path, &options.out, options.limit)?;
        return Ok(());
    }

    let head = rpc_call(&mut rpc, "eth_blockNumber", json!([]))?;
    let head = parse_hex_i128(
        head.as_str()
            .ok_or("eth_blockNumber returned a non-string result")?,
    )?;
    let end = options.to_block.filter(|value| *value != 0).unwrap_or(head);
    if !options.months.is_finite() {
        return Err("--months must be finite".into());
    }
    let computed_start = end - (options.months * 30.0 * 7200.0).trunc() as i128;
    let start = options
        .from_block
        .filter(|value| *value != 0)
        .unwrap_or(computed_start);
    eprintln!(
        "scanning {start}..{end} ({:.1} months)",
        (end - start) as f64 / 7200.0 / 30.0
    );
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    collect_skeleton(&mut rpc, &options.out, start, end, options.checkpoint_every)
}

fn collect_skeleton(
    rpc: &mut RpcClient,
    out: &Path,
    mut start: i128,
    end: i128,
    checkpoint_every: i128,
) -> HarnessResult<()> {
    let topic = topic_of(rpc, FILL_SIG)?;
    if let Some(resume) = resume_point(out)? {
        eprintln!("resuming at block {resume}");
        start = start.max(resume);
    }
    let mut fills = 0_u64;
    let started = Instant::now();
    let mut output = OpenOptions::new().create(true).append(true).open(out)?;
    let mut chunk_start = start;
    while chunk_start < end {
        let chunk_end = (chunk_start + CHUNK - 1).min(end);
        let logs = rpc_call(
            rpc,
            "eth_getLogs",
            json!([{
                "fromBlock": py_hex(chunk_start),
                "toBlock": py_hex(chunk_end),
                "address": REACTORS,
                "topics": [topic],
            }]),
        )?;
        let logs = logs.as_array().cloned().unwrap_or_default();

        let stamp = if checkpoint_every != 0 && py_remainder(chunk_start, checkpoint_every) < CHUNK
        {
            let header = rpc_call(
                rpc,
                "eth_getBlockByNumber",
                json!([py_hex(chunk_start), false]),
            )?;
            Some(hex_json_number(
                header["timestamp"]
                    .as_str()
                    .ok_or("block header has no string timestamp")?,
            )?)
        } else {
            None
        };

        for log in logs {
            let Some(topics) = log.get("topics").and_then(Value::as_array) else {
                continue;
            };
            if topics.len() < 4 {
                continue;
            }
            let block = hex_json_number(required_string(&log, "blockNumber")?)?;
            let log_index = hex_json_number(required_string(&log, "logIndex")?)?;
            let reactor = required_string(&log, "address")?.to_lowercase();
            let order = topics[1]
                .as_str()
                .ok_or("Fill order topic is not a string")?;
            let filler = topic_address(
                topics[2]
                    .as_str()
                    .ok_or("Fill filler topic is not a string")?,
            )?;
            let swapper = topic_address(
                topics[3]
                    .as_str()
                    .ok_or("Fill swapper topic is not a string")?,
            )?;
            serde_json::to_writer(
                &mut output,
                &json!({
                    "block": block,
                    "tx": required_string(&log, "transactionHash")?,
                    "log_index": log_index,
                    "reactor": reactor,
                    "order": order,
                    "filler": filler,
                    "swapper": swapper,
                }),
            )?;
            output.write_all(b"\n")?;
            fills += 1;
        }
        if let Some(stamp) = stamp {
            serde_json::to_writer(
                &mut output,
                &json!({"block": json_integer(chunk_start)?, "checkpoint": stamp}),
            )?;
            output.write_all(b"\n")?;
        }
        output.flush()?;

        let done = chunk_end - start + 1;
        if chunk_start.div_euclid(CHUNK).rem_euclid(200) == 0 {
            let elapsed = started.elapsed().as_secs_f64().max(1e-9);
            let rate = done as f64 / elapsed;
            let left = (end - chunk_end) as f64 / rate.max(1e-9);
            eprintln!(
                "  block {chunk_end} ({:.1}%), {fills} fills, {rate:.0} blk/s, ~{:.0} min left",
                100.0 * done as f64 / (end - start) as f64,
                left / 60.0,
            );
        }
        chunk_start += CHUNK;
    }
    eprintln!(
        "{fills} fills in {:.0}s, {} rpc calls",
        started.elapsed().as_secs_f64(),
        rpc.calls()
    );
    Ok(())
}

fn decode_amounts(
    rpc: &mut RpcClient,
    skeleton_path: &Path,
    out: &Path,
    limit: i128,
) -> HarnessResult<()> {
    let mut rows = Vec::new();
    for line in BufReader::new(File::open(skeleton_path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(&line)?;
        if row.get("checkpoint").is_none() {
            rows.push(row);
        }
    }
    rows = signed_tail_slice(rows, limit)?;
    if let Some(done) = resume_point(out)? {
        let mut remaining = Vec::with_capacity(rows.len());
        for row in rows {
            if json_i128(row.get("block").ok_or("skeleton row has no block field")?)? >= done {
                remaining.push(row);
            }
        }
        rows = remaining;
        eprintln!("resuming at block {done}, {} left", rows.len());
    }

    let started = Instant::now();
    let mut output = OpenOptions::new().create(true).append(true).open(out)?;
    let mut seen_tx: Option<(String, Vec<Value>)> = None;
    for (index, row) in rows.iter().enumerate() {
        let transaction = required_string(row, "tx")?.to_string();
        if seen_tx
            .as_ref()
            .is_none_or(|(known, _)| known != &transaction)
        {
            let receipt = rpc_call(rpc, "eth_getTransactionReceipt", json!([transaction]))?;
            let logs = receipt
                .get("logs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // recently encountered transaction in this cache.
            seen_tx = Some((transaction.clone(), logs));
        }
        let swapper = required_string(row, "swapper")?.to_lowercase();
        let mut legs = Vec::new();
        for log in &seen_tx.as_ref().expect("receipt cache set above").1 {
            let Some(topics) = log.get("topics").and_then(Value::as_array) else {
                continue;
            };
            if topics.is_empty() || topics[0].as_str() != Some(TRANSFER_TOPIC) || topics.len() < 3 {
                continue;
            }
            let sender = topic_address(
                topics[1]
                    .as_str()
                    .ok_or("Transfer sender topic is not a string")?,
            )?;
            let receiver = topic_address(
                topics[2]
                    .as_str()
                    .ok_or("Transfer receiver topic is not a string")?,
            )?;
            if swapper != sender && swapper != receiver {
                continue;
            }
            let data = required_string(log, "data")?;
            let amount = if data.len() >= 66 {
                hex_json_number(&data[..66])?
            } else {
                json_integer(0)?
            };
            legs.push(json!({
                "token": required_string(log, "address")?.to_lowercase(),
                "amount": amount,
                "out": sender == swapper,
            }));
        }
        if !legs
            .iter()
            .any(|leg| leg.get("out").and_then(Value::as_bool) == Some(false))
        {
            let traces = rpc_call(rpc, "trace_transaction", json!([transaction]))?;
            for trace in traces.as_array().cloned().unwrap_or_default() {
                let Some(action) = trace.get("action").and_then(Value::as_object) else {
                    continue;
                };
                let recipient = action
                    .get("to")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let value = action
                    .get("value")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("0x0");
                if recipient == swapper && hex_is_positive(value)? {
                    legs.push(json!({
                        "token": NATIVE,
                        "amount": hex_json_number(value)?,
                        "out": false,
                    }));
                }
            }
        }

        let mut merged = row
            .as_object()
            .cloned()
            .ok_or("skeleton row is not a JSON object")?;
        merged.insert("legs".to_string(), Value::Array(legs));
        serde_json::to_writer(&mut output, &Value::Object(merged))?;
        output.write_all(b"\n")?;
        if index % 500 == 0 {
            output.flush()?;
            let rate = (index + 1) as f64 / started.elapsed().as_secs_f64().max(1e-9);
            eprintln!(
                "  {}/{} ({rate:.0}/s, ~{:.0} min left)",
                index + 1,
                rows.len(),
                (rows.len() - index) as f64 / rate.max(1e-9) / 60.0,
            );
        }
    }
    eprintln!(
        "decoded {} fills in {:.0}s",
        rows.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn topic_of(rpc: &mut RpcClient, signature: &str) -> HarnessResult<String> {
    let encoded = format!("0x{}", hex::encode(signature.as_bytes()));
    rpc_call(rpc, "web3_sha3", json!([encoded]))?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "web3_sha3 returned a non-string result".into())
}

fn rpc_call(rpc: &mut RpcClient, method: &str, params: Value) -> HarnessResult<Value> {
    rpc.call_strict(method, params)
        .map_err(|error| error.to_string().into())
}

fn resume_point(path: &Path) -> HarnessResult<Option<i128>> {
    let Ok(file) = File::open(path) else {
        return Ok(None);
    };
    let mut last = None;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(row) => last = Some(row),
            Err(_) => break,
        }
    }
    let Some(last) = last else {
        return Ok(None);
    };
    Ok(Some(
        json_i128(last.get("block").ok_or("last output row has no block")?)? + 1,
    ))
}

fn signed_tail_slice(mut rows: Vec<Value>, limit: i128) -> HarnessResult<Vec<Value>> {
    if limit == 0 {
        return Ok(rows);
    }
    if limit > 0 {
        let keep = usize::try_from(limit).unwrap_or(usize::MAX);
        let start = rows.len().saturating_sub(keep);
        return Ok(rows.split_off(start));
    }
    let drop = limit
        .checked_abs()
        .ok_or("--limit is too negative")
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX))?;
    if drop >= rows.len() {
        return Ok(Vec::new());
    }
    Ok(rows.split_off(drop))
}

fn required_string<'a>(value: &'a Value, key: &str) -> HarnessResult<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field {key} is not a string").into())
}

fn topic_address(topic: &str) -> HarnessResult<String> {
    if topic.len() < 40 {
        return Err("address topic is shorter than 40 characters".into());
    }
    Ok(format!("0x{}", &topic[topic.len() - 40..]))
}

fn parse_hex_i128(text: &str) -> HarnessResult<i128> {
    let (negative, digits) = if let Some(rest) = text.strip_prefix("-0x") {
        (true, rest)
    } else if let Some(rest) = text.strip_prefix("0x") {
        (false, rest)
    } else {
        (false, text)
    };
    let magnitude = i128::from_str_radix(if digits.is_empty() { "0" } else { digits }, 16)?;
    Ok(if negative { -magnitude } else { magnitude })
}

fn py_hex(value: i128) -> String {
    if value < 0 {
        format!("-0x{:x}", value.unsigned_abs())
    } else {
        format!("0x{value:x}")
    }
}

fn py_remainder(left: i128, right: i128) -> i128 {
    let remainder = left % right;
    if remainder != 0 && (remainder.is_negative() != right.is_negative()) {
        remainder + right
    } else {
        remainder
    }
}

fn json_i128(value: &Value) -> HarnessResult<i128> {
    value
        .as_number()
        .ok_or_else(|| format!("expected JSON integer, got {value}"))?
        .to_string()
        .parse::<i128>()
        .map_err(|error| format!("invalid JSON integer {value}: {error}").into())
}

fn json_integer(value: i128) -> HarnessResult<Value> {
    serde_json::from_str(&value.to_string()).map_err(Into::into)
}

fn hex_json_number(text: &str) -> HarnessResult<Value> {
    let decimal = hex_to_decimal(text)?;
    serde_json::from_str(&decimal).map_err(Into::into)
}

fn hex_is_positive(text: &str) -> HarnessResult<bool> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    if digits.is_empty() {
        return Ok(false);
    }
    for byte in digits.bytes() {
        match byte {
            b'0' => {}
            b'1'..=b'9' | b'a'..=b'f' | b'A'..=b'F' => return Ok(true),
            _ => return Err(format!("invalid hexadecimal value {text}").into()),
        }
    }
    Ok(false)
}

/// Convert an unsigned hexadecimal JSON-RPC quantity to an arbitrary-precision
/// decimal string without passing through a signed JSON accessor.
fn hex_to_decimal(text: &str) -> HarnessResult<String> {
    let digits = text.strip_prefix("0x").unwrap_or(text);
    let mut limbs = vec![0_u32]; // little-endian base 1e9
    for byte in digits.bytes() {
        let nibble = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            _ => return Err(format!("invalid hexadecimal value {text}").into()),
        };
        let mut carry = nibble;
        for limb in &mut limbs {
            let value = u64::from(*limb) * 16 + carry;
            *limb = (value % 1_000_000_000) as u32;
            carry = value / 1_000_000_000;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut out = limbs.pop().unwrap_or(0).to_string();
    for limb in limbs.iter().rev() {
        out.push_str(&format!("{limb:09}"));
    }
    Ok(out)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        rpc: "http://127.0.0.1:8545".to_string(),
        out: PathBuf::new(),
        months: 28.0,
        from_block: None,
        to_block: None,
        checkpoint_every: 50_000,
        amounts: None,
        limit: 150_000,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--rpc") => {
                options.rpc = next_value(&mut args, "--rpc")
                    .and_then(|value| value.into_string().map_err(|_| "invalid --rpc".into()))?;
            }
            Some("--out") => options.out = PathBuf::from(next_value(&mut args, "--out")?),
            Some("--months") => {
                options.months = parse_value(next_value(&mut args, "--months")?, "--months")?
            }
            Some("--from-block") => {
                options.from_block = Some(parse_value(
                    next_value(&mut args, "--from-block")?,
                    "--from-block",
                )?)
            }
            Some("--to-block") => {
                options.to_block = Some(parse_value(
                    next_value(&mut args, "--to-block")?,
                    "--to-block",
                )?)
            }
            Some("--checkpoint-every") => {
                options.checkpoint_every = parse_value(
                    next_value(&mut args, "--checkpoint-every")?,
                    "--checkpoint-every",
                )?
            }
            Some("--amounts") => {
                options.amounts = Some(PathBuf::from(next_value(&mut args, "--amounts")?))
            }
            Some("--limit") => {
                options.limit = parse_value(next_value(&mut args, "--limit")?, "--limit")?
            }
            Some("-h" | "--help") => {
                println!(
                    "usage: collect_uniswapx --out PATH [--rpc URL] [--months N] \
                     [--from-block N] [--to-block N] [--checkpoint-every N] \
                     [--amounts PATH] [--limit N]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", argument.to_string_lossy()).into()),
        }
    }
    if options.out.as_os_str().is_empty() {
        return Err("--out is required".into());
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_full_uint256_without_signed_narrowing() {
        assert_eq!(
            hex_to_decimal("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
                .unwrap(),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    #[test]
    fn signed_negative_limit_and_remainder_match() {
        let rows = (0..5)
            .map(json_integer)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(signed_tail_slice(rows, -2).unwrap().len(), 3);
        assert_eq!(py_remainder(-1, 50_000), 49_999);
        assert_eq!(py_remainder(1, -50_000), -49_999);
    }
}
