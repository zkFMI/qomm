use qomm_harness::{median, parse_value, write_pretty_json, HarnessResult};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

const BLOCK_SECONDS: f64 = 12.0;
type PairSeries = Vec<((String, String), Vec<(i64, f64)>)>;

struct Options {
    fills: PathBuf,
    out: PathBuf,
    pairs: usize,
    gaps: Vec<i64>,
}

#[derive(Deserialize)]
struct FillRecord {
    block: Option<i64>,
    #[serde(default)]
    legs: Vec<FillLeg>,
}

#[derive(Deserialize)]
struct FillLeg {
    token: String,
    amount: serde_json::Number,
    #[serde(default)]
    out: bool,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if !options.fills.exists() {
        return Err(format!(
            "{} is not here. This measurement needs the UniswapX fills, which are not shipped with the repository; nothing is substituted for them.",
            options.fills.display()
        )
        .into());
    }
    let by_pair = series(&options.fills)?;
    let mut busiest = by_pair.iter().collect::<Vec<_>>();
    busiest.sort_by_key(|(_, rows)| std::cmp::Reverse(rows.len()));
    busiest.truncate(options.pairs);
    let mut result_rows = Vec::new();
    for (key, rows) in busiest {
        let mut row = measure(rows, &options.gaps);
        row.as_object_mut()
            .expect("measure returns an object")
            .insert("pair".into(), json!([key.0, key.1]));
        let floor = row["within_block"]["median_bp"]
            .as_f64()
            .ok_or("busiest pair has no within-block dispersion")?;
        println!(
            "{}../{}..  n={}",
            prefix(&key.0, 10),
            prefix(&key.1, 10),
            row["observations"]
        );
        println!("   within one block            {floor:6.1} bp");
        for gap in &options.gaps {
            let moved = &row["across_blocks"][gap.to_string()];
            if !moved.is_null() {
                let median_bp = moved["median_bp"].as_f64().unwrap_or(0.0);
                println!(
                    "   {gap:2} blocks (~{:4.0} s)        {median_bp:6.1} bp   = {:.2}x the within-block floor",
                    *gap as f64 * BLOCK_SECONDS,
                    median_bp / floor,
                );
            }
        }
        println!();
        result_rows.push(row);
    }
    let ratios = result_rows
        .iter()
        .filter_map(|row| row["ratio_to_within_block"]["2"].as_f64())
        .filter(|value| *value != 0.0)
        .collect::<Vec<_>>();
    let ratio_at_24 = median(&ratios);
    println!(
        "24 s of drift is {:.2}x the dispersion the market already has inside one block",
        ratio_at_24.ok_or("no pair produced a 24-second ratio")?
    );

    let mut sensitivity = Map::new();
    for count in [2usize, 4, 8, 16, 32] {
        let mut subset = by_pair.iter().collect::<Vec<_>>();
        subset.sort_by_key(|(_, rows)| std::cmp::Reverse(rows.len()));
        subset.truncate(count);
        let ratios = subset
            .into_iter()
            .filter_map(|(_, rows)| measure(rows, &[2])["ratio_to_within_block"]["2"].as_f64())
            .filter(|value| *value != 0.0)
            .collect::<Vec<_>>();
        sensitivity.insert(
            count.to_string(),
            json!({"pairs": ratios.len(), "median_ratio": median(&ratios)}),
        );
    }
    println!("\nhow much the choice of four pairs mattered:");
    for count in [2usize, 4, 8, 16, 32] {
        let value = &sensitivity[&count.to_string()];
        match value["median_ratio"].as_f64() {
            Some(ratio) if ratio != 0.0 => {
                println!("  busiest {count:2} pairs: median ratio {ratio:.2}")
            }
            _ => println!("  busiest {count:2} pairs: none"),
        }
    }
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "block_seconds": BLOCK_SECONDS,
        "gaps_blocks": options.gaps,
        "gaps_seconds": options.gaps.iter().map(|gap| *gap as f64 * BLOCK_SECONDS).collect::<Vec<_>>(),
        "pairs_seen": by_pair.len(),
        "rows": result_rows,
        "median_ratio_at_24s": ratio_at_24,
        "conditioned_on": "orders that settled; an order that went stale and did not settle leaves no Fill log, so the drift estimate is a lower bound",
        "pair_count_sensitivity": sensitivity,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn series(path: &std::path::Path) -> HarnessResult<PairSeries> {
    let text = fs::read_to_string(path)?;
    let mut by_pair: PairSeries = Vec::new();
    for line in text.lines() {
        let record: FillRecord = serde_json::from_str(line)?;
        let outs = record.legs.iter().filter(|leg| leg.out).collect::<Vec<_>>();
        let ins = record
            .legs
            .iter()
            .filter(|leg| !leg.out)
            .collect::<Vec<_>>();
        if outs.len() != 1 || ins.len() != 1 {
            continue;
        }
        let amount_out = &outs[0].amount;
        let amount_in = &ins[0].amount;
        if amount_out.as_f64() == Some(0.0) || amount_in.as_f64() == Some(0.0) {
            continue;
        }
        let token_out = outs[0].token.clone();
        let token_in = ins[0].token.clone();
        let flip = token_in > token_out;
        let key = if flip {
            (token_out, token_in)
        } else {
            (token_in, token_out)
        };
        let rate = correctly_rounded_integer_ratio(amount_out, amount_in)?;
        let log_rate = if flip { (1.0 / rate).ln() } else { rate.ln() };
        let block = record.block.ok_or("fill record has no integer block")?;
        if let Some((_, values)) = by_pair.iter_mut().find(|(existing, _)| *existing == key) {
            values.push((block, log_rate));
        } else {
            by_pair.push((key, vec![(block, log_rate)]));
        }
    }
    Ok(by_pair)
}

/// Correctly rounded division for the positive integer token amounts in the
/// fill feed. Converting each JSON integer to `f64` first
/// loses low limbs before the division; double-double parsing and correction
/// retain them, including values wider than `u128`.
fn correctly_rounded_integer_ratio(
    numerator: &serde_json::Number,
    denominator: &serde_json::Number,
) -> HarnessResult<f64> {
    let (numerator_hi, numerator_lo) = decimal_double_double(numerator)?;
    let (denominator_hi, denominator_lo) = decimal_double_double(denominator)?;
    let estimate = numerator_hi / denominator_hi;
    let residual = (-estimate).mul_add(denominator_hi, numerator_hi) + numerator_lo
        - estimate * denominator_lo;
    Ok(estimate + residual / (denominator_hi + denominator_lo))
}

fn decimal_double_double(value: &serde_json::Number) -> HarnessResult<(f64, f64)> {
    let mut high = 0.0f64;
    let mut low = 0.0f64;
    for byte in value.to_string().bytes() {
        if !byte.is_ascii_digit() {
            return Err(format!("fill amount is not a non-negative integer: {value}").into());
        }
        let product = high * 10.0;
        let product_error = high.mul_add(10.0, -product) + low * 10.0;
        (high, low) = quick_two_sum(product, product_error);
        let (sum, sum_error) = quick_two_sum(high, f64::from(byte - b'0'));
        (high, low) = quick_two_sum(sum, low + sum_error);
    }
    Ok((high, low))
}

fn quick_two_sum(left: f64, right: f64) -> (f64, f64) {
    let sum = left + right;
    (sum, right - (sum - left))
}

fn basis_points(values: &[f64]) -> Value {
    median(values).map_or(
        Value::Null,
        |value| json!({"median_bp": 1e4 * value, "n": values.len()}),
    )
}

fn measure(rows: &[(i64, f64)], gaps: &[i64]) -> Value {
    let mut per_block: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for &(block, log_rate) in rows {
        per_block.entry(block).or_default().push(log_rate);
    }
    let mut within = Vec::new();
    for values in per_block.values() {
        within.extend(values.windows(2).map(|pair| (pair[1] - pair[0]).abs()));
    }
    let mut across = Map::new();
    for &gap in gaps {
        let moves = per_block
            .iter()
            .filter_map(|(&block, values)| {
                let later = per_block.get(&(block + gap))?;
                Some((median(later)? - median(values)?).abs())
            })
            .collect::<Vec<_>>();
        across.insert(gap.to_string(), basis_points(&moves));
    }
    let floor = basis_points(&within);
    let mut result = Map::new();
    result.insert("observations".into(), json!(rows.len()));
    result.insert("blocks".into(), json!(per_block.len()));
    result.insert("within_block".into(), floor.clone());
    result.insert("across_blocks".into(), Value::Object(across.clone()));
    let mut ratios = Map::new();
    if let Some(floor_bp) = floor["median_bp"].as_f64().filter(|value| *value > 0.0) {
        for (gap, moved) in across {
            if let Some(moved_bp) = moved["median_bp"].as_f64() {
                ratios.insert(gap, json!(moved_bp / floor_bp));
            }
        }
    }
    if !ratios.is_empty() {
        result.insert("ratio_to_within_block".into(), Value::Object(ratios));
    }
    Value::Object(result)
}

fn prefix(value: &str, chars: usize) -> String {
    value.chars().take(chars).collect()
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        fills: PathBuf::new(),
        out: qomm_harness::repo_root().join("artifacts/staleness.json"),
        pairs: 4,
        gaps: vec![2, 8, 25],
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--fills" => options.fills = PathBuf::from(value(&raw, &mut index, "--fills")?),
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--pairs" => {
                options.pairs = parse_value(value(&raw, &mut index, "--pairs")?, "--pairs")?
            }
            "--gaps" => {
                options.gaps = parse_many(&raw, &mut index, "--gaps")?;
                continue;
            }
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    if options.fills.as_os_str().is_empty() {
        return Err("--fills is required".into());
    }
    Ok(options)
}

fn parse_many<T>(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let mut values = Vec::new();
    *index += 1;
    while *index < raw.len() && !raw[*index].to_string_lossy().starts_with("--") {
        values.push(parse_value(raw[*index].clone(), name)?);
        *index += 1;
    }
    Ok(values)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
