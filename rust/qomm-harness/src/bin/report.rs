use qomm_harness::HarnessResult;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

struct Options {
    artifacts: PathBuf,
    sweep: String,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let mut sweep_path = options.artifacts.join(&options.sweep);
    if !sweep_path.exists() {
        sweep_path = options.artifacts.join("qomm_sweep.jsonl");
    }
    let rows = load_sweep(&sweep_path)?;
    let mut out = String::new();
    writeln!(
        out,
        "# source: {}, {} verified runs\n",
        sweep_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
        rows.len()
    )?;

    writeln!(
        out,
        "## MPC quote latency, 7 parties, malicious Shamir (N=7, T=2)\n"
    )?;
    for mode in ["rfq", "rfm", "rfs"] {
        if !rows.iter().any(|row| row["mode"] == mode) {
            continue;
        }
        writeln!(out, "### {}\n", mode.to_uppercase())?;
        writeln!(out, "{}\n", sweep_table(&rows, mode, "none"))?;
    }
    if rows.iter().any(|row| row["disclose"] == "threshold") {
        writeln!(out, "### RFQ + threshold disclosure (arm B)\n")?;
        writeln!(out, "{}\n", sweep_table(&rows, "rfq", "threshold"))?;
    }
    let cross = options.artifacts.join("host-b/host_b_sweep.jsonl");
    if cross.exists() {
        let other = load_sweep(&cross)?;
        writeln!(
            out,
            "## Reproduced on a second host (circuit cost does not depend on the hardware)\n"
        )?;
        writeln!(out, "| M | one-way delay | rounds (host-a / host-b) | MB sent (host-a / host-b) | median [s] host-a | median [s] host-b |")?;
        writeln!(out, "|---:|---:|---|---|---:|---:|")?;
        for row in other {
            let Some(base) = rows.iter().find(|base| {
                base["mode"] == row["mode"]
                    && base["n_mm"] == row["n_mm"]
                    && base["delay_ms"] == row["delay_ms"]
                    && base["disclose"] == row["disclose"]
            }) else {
                continue;
            };
            let rounds_note = if base["measured_rounds"] == row["measured_rounds"] {
                ""
            } else {
                " <- differs"
            };
            let bytes_note = if base["measured_mb"] == row["measured_mb"] {
                ""
            } else {
                " <- differs"
            };
            writeln!(
                out,
                "| {} | {} ms | {} / {}{} | {} / {}{} | {:.3} | {:.3} |",
                display(&row["n_mm"]),
                general(number(&row["delay_ms"])),
                display(&base["measured_rounds"]),
                display(&row["measured_rounds"]),
                rounds_note,
                display(&base["measured_mb"]),
                display(&row["measured_mb"]),
                bytes_note,
                number(&base["wall_median"]),
                number(&row["wall_median"]),
            )?;
        }
        writeln!(out)?;
    }

    let predictions = check_predictions(&rows);
    writeln!(out, "## Verdict on the preregistered predictions\n")?;
    writeln!(out, "| # | prediction | measured | verdict |")?;
    writeln!(out, "|---|---|---|---|")?;
    for check in &predictions {
        writeln!(
            out,
            "| {} | {} | {} | **{}** |",
            text(&check["id"]),
            text(&check["claim"]),
            text(&check["evidence"]),
            text(&check["verdict"]),
        )?;
    }
    writeln!(out)?;
    for check in &predictions {
        writeln!(
            out,
            "- **{}**: {}",
            text(&check["id"]),
            text(&check["note"])
        )?;
    }
    writeln!(out)?;

    writeln!(out, "## Round-count scaling in M\n")?;
    for mode in ["rfq", "rfm", "rfs"] {
        let Some(fit) = round_scaling(&rows, mode) else {
            continue;
        };
        writeln!(
            out,
            "- **{}**: {}",
            mode.to_uppercase(),
            measured_display(&fit["measured"])
        )?;
        writeln!(
            out,
            "  - log2 fit R^2 = {:.4} (slope {:.1} rounds per doubling)",
            number(&fit["log2_fit"]["r2"]),
            number(&fit["log2_fit"]["slope"]),
        )?;
        writeln!(
            out,
            "  - linear fit R^2 = {:.4}",
            number(&fit["linear_fit"]["r2"])
        )?;
        writeln!(out, "  - verdict: **{}**", text(&fit["verdict"]))?;
    }
    writeln!(out)?;

    writeln!(out, "## Threshold disclosure overhead (arm B vs arm A)\n")?;
    writeln!(
        out,
        "| M | A rounds | B rounds | A [s] @1ms | B [s] @1ms | increment |"
    )?;
    writeln!(out, "|---:|---:|---:|---:|---:|---:|")?;
    for n_mm in unique_i64(&rows, "n_mm") {
        let (Some(a), Some(b)) = (
            pick(&rows, "rfq", n_mm, 1.0, "none"),
            pick(&rows, "rfq", n_mm, 1.0, "threshold"),
        ) else {
            continue;
        };
        let ratio = number(&b["wall_median"]) / number(&a["wall_median"]) - 1.0;
        writeln!(
            out,
            "| {n_mm} | {} | {} | {:.3} | {:.3} | {:+.1}% |",
            display(&a["measured_rounds"]),
            display(&b["measured_rounds"]),
            number(&a["wall_median"]),
            number(&b["wall_median"]),
            ratio * 100.0,
        )?;
    }
    writeln!(out)?;

    let sim_path = options.artifacts.join("sim_matrix.json");
    if sim_path.exists() {
        let sim: Value = serde_json::from_slice(&fs::read(sim_path)?)?;
        writeln!(
            out,
            "## Phase-3 comparison ({} seeds, {:.0} minutes of simulated trading each)\n",
            display(&sim["config"]["seeds"]),
            number(&sim["config"]["steps"]) * 50.0 / 1_000.0 / 60.0,
        )?;
        print_sim(&mut out, &sim)?;
    }
    let audit_path = options.artifacts.join("dp_audit.json");
    if audit_path.exists() {
        let audit: Value = serde_json::from_slice(&fs::read(audit_path)?)?;
        writeln!(
            out,
            "## Entity-level DP audit (two-world membership game)\n"
        )?;
        writeln!(
            out,
            "| declared eps/window | audit cells | measured eps lower bound (max) | violations |"
        )?;
        writeln!(out, "|---:|---:|---:|---:|")?;
        for bucket in array(&audit["by_epsilon"]) {
            writeln!(
                out,
                "| {} | {} | {:.3} | {} |",
                display(&bucket["declared_epsilon"]),
                display(&bucket["cells"]),
                number(&bucket["max_empirical_epsilon"]),
                display(&bucket["violations"]),
            )?;
        }
        writeln!(out)?;
    }
    print!("{out}");
    Ok(())
}

fn load_sweep(path: &Path) -> HarnessResult<Vec<Value>> {
    let text = fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?
        .into_iter()
        .filter(|row| row["verified"].as_bool().unwrap_or(false))
        .collect())
}

fn sweep_table(rows: &[Value], mode: &str, disclose: &str) -> String {
    let keep = rows
        .iter()
        .filter(|row| row["mode"] == mode && row["disclose"] == disclose)
        .collect::<Vec<_>>();
    let mut delays = keep
        .iter()
        .filter_map(|row| row["delay_ms"].as_f64())
        .collect::<Vec<_>>();
    delays.sort_by(f64::total_cmp);
    delays.dedup_by(|left, right| *left == *right);
    let mut mms = keep
        .iter()
        .filter_map(|row| row["n_mm"].as_i64())
        .collect::<Vec<_>>();
    mms.sort_unstable();
    mms.dedup();
    let header = format!(
        "| M | rounds | sent (MB/party) | {} |",
        delays
            .iter()
            .map(|delay| format!("{}ms", general(*delay)))
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let separator = format!("{}|", "|---:".repeat(3 + delays.len()));
    let mut lines = vec![header, separator];
    for n_mm in mms {
        let mut cells = Vec::new();
        let mut rounds = Value::Null;
        let mut megabytes = Value::Null;
        for delay in &delays {
            let Some(row) = keep
                .iter()
                .find(|row| row["n_mm"] == n_mm && row["delay_ms"].as_f64() == Some(*delay))
            else {
                cells.push("-".into());
                continue;
            };
            if rounds.is_null() || rounds.as_f64() == Some(0.0) {
                rounds = row.get("measured_rounds").cloned().unwrap_or(Value::Null);
            }
            if megabytes.is_null() || megabytes.as_f64() == Some(0.0) {
                megabytes = row.get("measured_mb").cloned().unwrap_or(Value::Null);
            }
            cells.push(format!("{:.3}", number(&row["wall_median"])));
        }
        lines.push(format!(
            "| {n_mm} | {} | {} | {} |",
            display(&rounds),
            display(&megabytes),
            cells.join(" | ")
        ));
    }
    lines.join("\n")
}

fn round_scaling(rows: &[Value], mode: &str) -> Option<Value> {
    let mut keep = BTreeMap::<i64, f64>::new();
    for row in rows {
        if row["mode"] == mode && row["disclose"] == "none" && row["delay_ms"].as_f64() == Some(0.0)
        {
            if let (Some(m), Some(rounds)) = (row["n_mm"].as_i64(), row["measured_rounds"].as_f64())
            {
                keep.insert(m, rounds);
            }
        }
    }
    if keep.len() < 3 {
        return None;
    }
    let log_x = keep
        .keys()
        .map(|value| (*value as f64).log2())
        .collect::<Vec<_>>();
    let linear_x = keep.keys().map(|value| *value as f64).collect::<Vec<_>>();
    let ys = keep.values().copied().collect::<Vec<_>>();
    let log = fit(&log_x, &ys);
    let linear = fit(&linear_x, &ys);
    let measured = keep
        .into_iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect::<serde_json::Map<_, _>>();
    Some(json!({
        "measured": measured,
        "log2_fit": {"intercept": log.0, "slope": log.1, "r2": log.2},
        "linear_fit": {"intercept": linear.0, "slope": linear.1, "r2": linear.2},
        "verdict": if log.2 > linear.2 {"logarithmic"} else {"linear"},
    }))
}

fn fit(xs: &[f64], ys: &[f64]) -> (f64, f64, f64) {
    // The locked metric contract uses a Neumaier compensation term. A naive fold lands one
    // unit in the last place away and the difference reaches the reported R^2.
    use qomm_sim::fsum::nsum;
    let n = xs.len() as f64;
    let mx = nsum(xs.iter().copied()) / n;
    let my = nsum(ys.iter().copied()) / n;
    let sxx = nsum(xs.iter().map(|x| (x - mx).powi(2)));
    let sxy = nsum(xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)));
    let slope = if sxx == 0.0 { 0.0 } else { sxy / sxx };
    let intercept = my - slope * mx;
    let residual = nsum(
        xs.iter()
            .zip(ys)
            .map(|(x, y)| (y - (intercept + slope * x)).powi(2)),
    );
    let total = nsum(ys.iter().map(|y| (y - my).powi(2)));
    (
        intercept,
        slope,
        if total == 0.0 {
            0.0
        } else {
            1.0 - residual / total
        },
    )
}

fn check_predictions(rows: &[Value]) -> Vec<Value> {
    let mut output = Vec::new();
    if let (Some(small), Some(large)) = (
        pick(rows, "rfq", 4, 0.0, "none"),
        pick(rows, "rfq", 64, 0.0, "none"),
    ) {
        let ratio = number(&large["measured_rounds"]) / number(&small["measured_rounds"]);
        output.push(json!({
            "id": "A",
            "claim": "the round count grows only logarithmically in M",
            "evidence": format!("M 4->64 (16x) takes rounds {}->{} = {ratio:.2}x", display(&small["measured_rounds"]), display(&large["measured_rounds"])),
            "verdict": if ratio < 4.0 {"PASS"} else {"FAIL"},
            "note": "linear would be 16x; how well the log model actually fits is judged separately by R^2",
        }));
    }
    if let Some(wide) = pick(rows, "rfq", 16, 15.0, "none") {
        output.push(json!({
            "id": "C",
            "claim": "over a wide area the communication floor dominates and it stays in seconds",
            "evidence": format!("15 ms one way (30 ms RTT), M=16, RFQ: median {:.3} s ({} rounds)", number(&wide["wall_median"]), display(&wide["measured_rounds"])),
            "verdict": if number(&wide["wall_median"]) > 0.5 {"PASS"} else {"FAIL"},
            "note": "does not reach an immediate answer (<200 ms); an order of magnitude fewer rounds would still not",
        }));
    }
    if let (Some(rfq), Some(rfm)) = (
        pick(rows, "rfq", 16, 1.0, "none"),
        pick(rows, "rfm", 16, 1.0, "none"),
    ) {
        let difference = (number(&rfq["measured_rounds"]) - number(&rfm["measured_rounds"])).abs();
        output.push(json!({
            "id": "D",
            "claim": "hiding the side costs essentially nothing (the RFQ circuit is the RFM circuit)",
            "evidence": format!("rounds RFQ {} / RFM {}", display(&rfq["measured_rounds"]), display(&rfm["measured_rounds"])),
            "verdict": if difference <= 2.0 {"PASS"} else {"FAIL"},
            "note": "an unencrypted RFM hides the side too; that must not be counted as an effect of the MPC",
        }));
        let mut delays = rows
            .iter()
            .filter_map(|row| row["delay_ms"].as_f64())
            .collect::<Vec<_>>();
        delays.sort_by(f64::total_cmp);
        delays.dedup_by(|left, right| *left == *right);
        let ratios = delays
            .into_iter()
            .filter_map(|delay| {
                let a = pick(rows, "rfq", 64, delay, "none")?;
                let b = pick(rows, "rfm", 64, delay, "none")?;
                Some(number(&b["wall_median"]) / number(&a["wall_median"]))
            })
            .collect::<Vec<_>>();
        if !ratios.is_empty() {
            output.push(json!({
                "id": "E",
                "claim": "RFM answers within 1.0 to 1.3x the time RFQ takes",
                "evidence": format!("ratio at M=64 {}", ratios.iter().map(|ratio| format!("{ratio:.2}")).collect::<Vec<_>>().join(", ")),
                "verdict": if ratios.iter().copied().max_by(f64::total_cmp).unwrap() <= 1.3 {"PASS"} else {"FAIL"},
                "note": "two trees instead of one, but the depth of a layer is unchanged",
            }));
        }
    }
    if let (Some(rfs), Some(rfq)) = (
        pick(rows, "rfs", 16, 15.0, "none"),
        pick(rows, "rfq", 16, 15.0, "none"),
    ) {
        let latency = number(&rfs["wall_median"]) / number(&rfq["wall_median"]);
        let rounds = number(&rfs["measured_rounds"]) / number(&rfq["measured_rounds"]);
        output.push(json!({
            "id": "F",
            "claim": "RFS (k=5) takes about five times as long in total as RFQ",
            "evidence": format!("round ratio {rounds:.2}, wall-clock ratio {latency:.2}"),
            "verdict": if (4.0..=6.0).contains(&latency) {"PASS"} else {"REFUTED"},
            "note": "rounds are about k times as predicted, but the wall clock is less than that: repeating the same circuit amortises preprocessing and start-up",
        }));
    }
    let mut increments = Vec::new();
    let mut overheads = Vec::new();
    for n_mm in unique_i64(rows, "n_mm") {
        if let (Some(a), Some(b)) = (
            pick(rows, "rfq", n_mm, 1.0, "none"),
            pick(rows, "rfq", n_mm, 1.0, "threshold"),
        ) {
            increments.push(number(&b["measured_rounds"]) - number(&a["measured_rounds"]));
            overheads.push(number(&b["wall_median"]) / number(&a["wall_median"]) - 1.0);
        }
    }
    if !increments.is_empty() {
        output.push(json!({
            "id": "G",
            "claim": "adding threshold disclosure costs a constant number of rounds and under +10% in time",
            "evidence": format!(
                "round increments {}, time increments {}",
                number_list(&increments),
                overheads.iter().map(|value| format!("{:+.1}%", value * 100.0)).collect::<Vec<_>>().join(", "),
            ),
            "verdict": if overheads.iter().copied().max_by(f64::total_cmp).unwrap() > 0.10 {"PARTIAL"} else {"PASS"},
            "note": "the round increment barely depends on M; the time increment exceeds 10% at large M because of the extra traffic",
        }));
    }
    output
}

fn print_sim(out: &mut String, sim: &Value) -> HarnessResult<()> {
    let aggregate = array(&sim["aggregate"]);
    writeln!(out, "### Privacy (layer 1, replay experiment, eps=1.0)\n")?;
    writeln!(out, "| arm | disclosure | AUC on unsettled requests | side guessed | size band guessed | per-maker inventory correlation | total inventory correlation | informed-or-not AUC |")?;
    writeln!(out, "|---|---|---:|---:|---:|---:|---:|---:|")?;
    for row in aggregate {
        if row["layer"] != "replay"
            || (row["disclosure"] == "C_dp" && row["epsilon_per_window"].as_f64() != Some(1.0))
        {
            continue;
        }
        writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            text(&row["protocol"]),
            text(&row["disclosure"]),
            fmt_cell(row.get("A1_passive_observer.auc"), 3),
            fmt_cell(row.get("A1b_pretrade_attributes.direction_accuracy"), 3),
            fmt_cell(row.get("A1b_pretrade_attributes.size_bucket_accuracy"), 3),
            fmt_cell(
                row.get("A3_probing_entity.own_inventory_corr_from_per_mm_quotes"),
                3
            ),
            fmt_cell(
                row.get("A3_probing_entity.net_inventory_corr_from_best_quote"),
                3
            ),
            fmt_cell(row.get("A5_external_info.auc"), 3),
        )?;
    }
    writeln!(out)?;
    for layer in ["replay", "reactive"] {
        let description = if layer == "replay" {
            "1, replay"
        } else {
            "2, reactive"
        };
        writeln!(
            out,
            "### Economics (layer {description} experiment, qomm_rfq)\n"
        )?;
        writeln!(out, "| disclosure | eps/window | fill rate | user cost [tick] | maker P&L per fill | 1 s markout | disclosure halt rate | eps spent | count error MAE |")?;
        writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|")?;
        for row in aggregate {
            if row["layer"] != layer || row["protocol"] != "qomm_rfq" {
                continue;
            }
            writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                text(&row["disclosure"]),
                display(&row["epsilon_per_window"]),
                fmt_cell(row.get("fill_rate"), 3),
                fmt_cell(row.get("user_cost_mean_ticks"), 2),
                fmt_cell(row.get("mm_pnl_per_fill"), 1),
                fmt_cell(row.get("mm_markout_1s_mean"), 2),
                fmt_cell(row.get("suppression_rate"), 2),
                fmt_cell(row.get("epsilon_spent_max"), 1),
                fmt_cell(row.get("release_requests_mae"), 1),
            )?;
        }
        writeln!(out)?;
    }
    writeln!(
        out,
        "### Power: the smallest difference this many trials can detect\n"
    )?;
    writeln!(out, "From the observed variance, the smallest difference a two-group comparison needs at 5% significance and 80% power.")?;
    writeln!(out, "A smaller difference than this is not 'no difference' but 'undecidable at this number of trials'.\n")?;
    writeln!(out, "| metric | mean (arm A) | standard deviation | n | minimum detectable difference | as a fraction of the mean |")?;
    writeln!(out, "|---|---:|---:|---:|---:|---:|")?;
    if let Some(base) = aggregate.iter().find(|row| {
        row["layer"] == "reactive" && row["protocol"] == "qomm_rfq" && row["disclosure"] == "A_none"
    }) {
        for (field, label, digits) in [
            ("fill_rate", "fill rate", 4usize),
            ("user_cost_mean_ticks", "user cost [tick]", 2),
            ("mm_pnl_per_fill", "maker P&L per fill", 1),
            ("mm_markout_1s_mean", "1 s markout", 2),
        ] {
            let Some(cell) = base.get(field).filter(|cell| !cell.is_null()) else {
                continue;
            };
            let n = cell["n"].as_u64().unwrap_or(0);
            if n < 2 {
                continue;
            }
            let mean = number(&cell["mean"]);
            let sd = number(&cell["ci95"]) * (n as f64).sqrt() / 1.96;
            let mde = 2.80 * sd * (2.0 / n as f64).sqrt();
            let share = if mean == 0.0 {
                f64::NAN
            } else {
                mde / mean.abs()
            };
            writeln!(
                out,
                "| {label} | {:.*} | {:.*} | {n} | {:.*} | {}% |",
                digits,
                mean,
                digits,
                sd,
                digits,
                mde,
                percent0(share * 100.0),
            )?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "### The cost of probing (how many probes are needed)\n"
    )?;
    writeln!(
        out,
        "| arm | probes to reach correlation 0.8 (total inventory) | same (per-maker inventory) |"
    )?;
    writeln!(out, "|---|---:|---:|")?;
    let mut seen = BTreeSet::new();
    for row in aggregate {
        let protocol = text(&row["protocol"]);
        if row["layer"] != "replay"
            || row["disclosure"] != "A_none"
            || !seen.insert(protocol.clone())
        {
            continue;
        }
        writeln!(
            out,
            "| {protocol} | {} | {} |",
            fmt_cell(
                row.get("A4_colluding_wallets.probes_needed_net_corr_0.8"),
                1
            ),
            fmt_cell(
                row.get("A4_colluding_wallets.probes_needed_per_mm_corr_0.8"),
                1
            ),
        )?;
    }
    writeln!(out)?;
    Ok(())
}

fn pick<'a>(
    rows: &'a [Value],
    mode: &str,
    n_mm: i64,
    delay: f64,
    disclose: &str,
) -> Option<&'a Value> {
    rows.iter().find(|row| {
        row["mode"] == mode
            && row["n_mm"] == n_mm
            && row["delay_ms"].as_f64() == Some(delay)
            && row["disclose"] == disclose
    })
}

fn unique_i64(rows: &[Value], field: &str) -> Vec<i64> {
    rows.iter()
        .filter_map(|row| row[field].as_i64())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn display(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn general(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn fmt_cell(cell: Option<&Value>, digits: usize) -> String {
    let Some(cell) = cell.filter(|cell| cell.is_object()) else {
        return "-".into();
    };
    format!(
        "{:.*} ± {:.*}",
        digits,
        number(&cell["mean"]),
        digits,
        number(&cell["ci95"]),
    )
}

fn measured_display(value: &Value) -> String {
    let Some(object) = value.as_object() else {
        return "{}".into();
    };
    let mut entries = object.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(key, _)| key.parse::<i64>().unwrap_or(i64::MAX));
    format!(
        "{{{}}}",
        entries
            .into_iter()
            .map(|(key, value)| {
                let rendered = value.as_f64().map_or_else(|| display(value), general);
                format!("{key}: {rendered}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn number_list(values: &[f64]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| if value.fract() == 0.0 {
                format!("{value:.0}")
            } else {
                value.to_string()
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn percent0(value: f64) -> String {
    if value.is_nan() {
        "nan".into()
    } else {
        format!("{value:.0}")
    }
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        artifacts: PathBuf::from("artifacts"),
        sweep: "qomm_sweep_clean.jsonl".into(),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--artifacts" => {
                options.artifacts = PathBuf::from(value(&raw, &mut index, "--artifacts")?)
            }
            "--sweep" => {
                options.sweep = value(&raw, &mut index, "--sweep")?
                    .to_string_lossy()
                    .into_owned()
            }
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
