//! Rust port of `scripts/make_figures.py`.
//!
//! Plot construction is native Rust/SVG. The final SVG is rendered to the two
//! publication formats with `rsvg-convert`; no Python or Matplotlib process is
//! involved.

use qomm_harness::{measurement_value, repo_root, run_checked, unique_temp_dir, HarnessResult};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OBLIVIOUS: &str = "#1f4e79";
const PLAIN: &str = "#c0504d";
const NEUTRAL: &str = "#7f7f7f";
const ACCENT: &str = "#4f7942";

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone)]
struct Series {
    label: String,
    colour: &'static str,
    points: Vec<(f64, f64)>,
    dashed: bool,
}

struct Canvas {
    width: u32,
    height: u32,
    body: String,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            body: String::new(),
        }
    }

    fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, colour: &str, width: f64, dash: bool) {
        self.body.push_str(&format!(
            "<line x1=\"{x1:.2}\" y1=\"{y1:.2}\" x2=\"{x2:.2}\" y2=\"{y2:.2}\" stroke=\"{colour}\" stroke-width=\"{width:.2}\"{} />\n",
            if dash { " stroke-dasharray=\"5 4\"" } else { "" }
        ));
    }

    fn rect(&mut self, rect: Rect, fill: &str, stroke: Option<&str>) {
        self.body.push_str(&format!(
            "<rect x=\"{:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{:.2}\" fill=\"{}\"{} />\n",
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            fill,
            stroke.map_or(String::new(), |s| format!(" stroke=\"{s}\""))
        ));
    }

    fn circle(&mut self, x: f64, y: f64, radius: f64, colour: &str) {
        self.body.push_str(&format!(
            "<circle cx=\"{x:.2}\" cy=\"{y:.2}\" r=\"{radius:.2}\" fill=\"{colour}\" />\n"
        ));
    }

    fn text(&mut self, x: f64, y: f64, text: &str, size: f64, anchor: &str, colour: &str) {
        self.body.push_str(&format!(
            "<text x=\"{x:.2}\" y=\"{y:.2}\" font-family=\"Helvetica,Arial,sans-serif\" font-size=\"{size:.1}\" text-anchor=\"{anchor}\" fill=\"{colour}\">{}</text>\n",
            escape(text)
        ));
    }

    fn finish(self) -> String {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">\n<rect width=\"100%\" height=\"100%\" fill=\"white\"/>\n{}</svg>\n",
            self.width, self.height, self.width, self.height, self.body
        )
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn bounds(series: &[Series], y_range: Option<(f64, f64)>) -> (f64, f64, f64, f64) {
    let points = series.iter().flat_map(|s| s.points.iter().copied());
    let (mut xmin, mut xmax, mut ymin, mut ymax) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in points {
        xmin = xmin.min(x);
        xmax = xmax.max(x);
        ymin = ymin.min(y);
        ymax = ymax.max(y);
    }
    if !xmin.is_finite() {
        return (0.0, 1.0, 0.0, 1.0);
    }
    if xmin == xmax {
        xmin -= 0.5;
        xmax += 0.5;
    }
    if let Some((lo, hi)) = y_range {
        ymin = lo;
        ymax = hi;
    } else if ymin == ymax {
        ymin -= 0.5;
        ymax += 0.5;
    } else {
        let pad = (ymax - ymin) * 0.08;
        ymin -= pad;
        ymax += pad;
    }
    (xmin, xmax, ymin, ymax)
}

fn line_chart(
    canvas: &mut Canvas,
    panel: Rect,
    title: &str,
    xlabel: &str,
    ylabel: &str,
    series: &[Series],
    y_range: Option<(f64, f64)>,
    horizontal: Option<f64>,
) {
    let plot = Rect {
        x: panel.x + 72.0,
        y: panel.y + 52.0,
        w: panel.w - 100.0,
        h: panel.h - 118.0,
    };
    let (xmin, xmax, ymin, ymax) = bounds(series, y_range);
    let sx = |x: f64| plot.x + (x - xmin) / (xmax - xmin) * plot.w;
    let sy = |y: f64| plot.y + plot.h - (y - ymin) / (ymax - ymin) * plot.h;
    for tick in 0..=4 {
        let y = plot.y + plot.h * tick as f64 / 4.0;
        canvas.line(plot.x, y, plot.x + plot.w, y, "#dddddd", 1.0, false);
        let value = ymax - (ymax - ymin) * tick as f64 / 4.0;
        canvas.text(
            plot.x - 8.0,
            y + 4.0,
            &format!("{value:.2}"),
            11.0,
            "end",
            "#555555",
        );
    }
    canvas.line(
        plot.x,
        plot.y,
        plot.x,
        plot.y + plot.h,
        "#333333",
        1.2,
        false,
    );
    canvas.line(
        plot.x,
        plot.y + plot.h,
        plot.x + plot.w,
        plot.y + plot.h,
        "#333333",
        1.2,
        false,
    );
    canvas.text(
        panel.x + 8.0,
        panel.y + 18.0,
        title,
        17.0,
        "start",
        "#222222",
    );
    canvas.text(
        plot.x + plot.w / 2.0,
        panel.y + panel.h - 12.0,
        xlabel,
        13.0,
        "middle",
        "#333333",
    );
    canvas.text(
        panel.x + 10.0,
        plot.y - 12.0,
        ylabel,
        12.0,
        "start",
        "#555555",
    );
    canvas.text(
        plot.x,
        plot.y + plot.h + 18.0,
        &format!("{xmin:.2}"),
        11.0,
        "middle",
        "#555555",
    );
    canvas.text(
        plot.x + plot.w,
        plot.y + plot.h + 18.0,
        &format!("{xmax:.2}"),
        11.0,
        "middle",
        "#555555",
    );
    if let Some(y) = horizontal.filter(|v| *v >= ymin && *v <= ymax) {
        canvas.line(plot.x, sy(y), plot.x + plot.w, sy(y), NEUTRAL, 1.2, true);
    }
    for (index, item) in series.iter().enumerate() {
        let mut previous = None;
        for &(x, y) in &item.points {
            let point = (sx(x), sy(y));
            if let Some((px, py)) = previous {
                canvas.line(px, py, point.0, point.1, item.colour, 2.4, item.dashed);
            }
            canvas.circle(point.0, point.1, 4.2, item.colour);
            previous = Some(point);
        }
        let lx = plot.x + plot.w - 160.0;
        let ly = plot.y + 18.0 + index as f64 * 20.0;
        canvas.line(
            lx,
            ly - 4.0,
            lx + 22.0,
            ly - 4.0,
            item.colour,
            2.4,
            item.dashed,
        );
        canvas.text(lx + 28.0, ly, &item.label, 11.0, "start", "#333333");
    }
}

fn bar_chart(
    canvas: &mut Canvas,
    panel: Rect,
    title: &str,
    ylabel: &str,
    labels: &[String],
    groups: &[(&str, &'static str, Vec<f64>)],
    horizontal: Option<f64>,
) {
    let plot = Rect {
        x: panel.x + 72.0,
        y: panel.y + 52.0,
        w: panel.w - 100.0,
        h: panel.h - 118.0,
    };
    let mut ymin = 0.0f64;
    let mut ymax = 0.0f64;
    for (_, _, values) in groups {
        for value in values {
            ymin = ymin.min(*value);
            ymax = ymax.max(*value);
        }
    }
    if let Some(line) = horizontal {
        ymax = ymax.max(line);
    }
    if ymin == ymax {
        ymax += 1.0;
    }
    let pad = (ymax - ymin) * 0.12;
    ymax += pad;
    if ymin < 0.0 {
        ymin -= pad;
    }
    let sy = |y: f64| plot.y + plot.h - (y - ymin) / (ymax - ymin) * plot.h;
    for tick in 0..=4 {
        let y = plot.y + plot.h * tick as f64 / 4.0;
        canvas.line(plot.x, y, plot.x + plot.w, y, "#dddddd", 1.0, false);
    }
    let baseline = sy(0.0);
    canvas.line(
        plot.x,
        baseline,
        plot.x + plot.w,
        baseline,
        "#333333",
        1.2,
        false,
    );
    canvas.line(
        plot.x,
        plot.y,
        plot.x,
        plot.y + plot.h,
        "#333333",
        1.2,
        false,
    );
    canvas.text(
        panel.x + 8.0,
        panel.y + 18.0,
        title,
        17.0,
        "start",
        "#222222",
    );
    canvas.text(
        panel.x + 10.0,
        plot.y - 12.0,
        ylabel,
        12.0,
        "start",
        "#555555",
    );
    if let Some(line) = horizontal {
        canvas.line(
            plot.x,
            sy(line),
            plot.x + plot.w,
            sy(line),
            NEUTRAL,
            1.2,
            true,
        );
    }
    let slots = labels.len().max(1) as f64;
    let group_width = plot.w / slots * 0.72;
    let bar_width = group_width / groups.len().max(1) as f64;
    for (group_index, (name, colour, values)) in groups.iter().enumerate() {
        for (index, value) in values.iter().enumerate() {
            let center = plot.x + plot.w * (index as f64 + 0.5) / slots;
            let x = center - group_width / 2.0 + group_index as f64 * bar_width;
            let top = sy(*value);
            canvas.rect(
                Rect {
                    x,
                    y: top.min(baseline),
                    w: bar_width * 0.88,
                    h: (baseline - top).abs(),
                },
                colour,
                None,
            );
        }
        let lx = plot.x + plot.w - 150.0;
        let ly = plot.y + 17.0 + group_index as f64 * 20.0;
        canvas.rect(
            Rect {
                x: lx,
                y: ly - 11.0,
                w: 18.0,
                h: 10.0,
            },
            colour,
            None,
        );
        canvas.text(lx + 24.0, ly, name, 11.0, "start", "#333333");
    }
    for (index, label) in labels.iter().enumerate() {
        let x = plot.x + plot.w * (index as f64 + 0.5) / slots;
        canvas.text(x, plot.y + plot.h + 19.0, label, 10.0, "middle", "#444444");
    }
}

fn load(artifacts: &Path, name: &str) -> HarnessResult<Option<Value>> {
    let path = artifacts.join(name);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    if name.ends_with(".jsonl") {
        let values = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<Vec<Value>, _>>()?;
        Ok(Some(Value::Array(values)))
    } else {
        Ok(Some(serde_json::from_str(&text)?))
    }
}

fn center(value: &Value) -> HarnessResult<f64> {
    measurement_value(value)
}

fn save(canvas: Canvas, stem: &str, output: &Path, temp: &Path) -> HarnessResult<String> {
    fs::create_dir_all(output)?;
    let svg = temp.join(format!("{stem}.svg"));
    fs::write(&svg, canvas.finish())?;
    for suffix in ["pdf", "png"] {
        let target = output.join(format!("{stem}.{suffix}"));
        run_checked(
            Command::new("rsvg-convert")
                .arg("--format")
                .arg(suffix)
                .arg("--output")
                .arg(&target)
                .arg(&svg),
            &format!("rendering {stem}.{suffix}"),
        )?;
    }
    fs::remove_file(svg)?;
    Ok(stem.to_string())
}

type Figure = Result<Result<Canvas, String>, Box<dyn std::error::Error>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> HarnessResult<()> {
    let root = repo_root();
    let art = root.join("artifacts");
    let mut output = art.join("figures");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                index += 1;
                output = PathBuf::from(args.get(index).ok_or("--out expects one path")?);
            }
            "-h" | "--help" => {
                println!("usage: make_figures [--out PATH]");
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
        index += 1;
    }
    let temp = unique_temp_dir("qomm-rust-figures")?;
    let figures: [(&str, fn(&Path) -> Figure); 13] = [
        ("fig_rho", fig_rho),
        ("fig_settlement_cost", fig_settlement_cost),
        ("fig_parallel", fig_parallel),
        ("fig_residency", fig_residency),
        ("fig_real_market", fig_real_market),
        ("fig_relay", fig_relay),
        ("fig_wasm", fig_wasm),
        ("fig_notes", fig_notes),
        ("fig_rings", fig_rings),
        ("fig_evm", fig_evm),
        ("fig_placement", fig_placement),
        ("fig_state_audit", fig_state_audit),
        ("fig_dp_effect", fig_dp_effect),
    ];
    let mut drawn = Vec::new();
    let mut skipped = Vec::new();
    for (function_name, figure) in figures {
        match figure(&art)? {
            Ok(canvas) => {
                let stem = function_name.trim_start_matches("fig_");
                let stem = match stem {
                    "rho" => "rho_sweep",
                    "parallel" => "parallel_scaling",
                    "residency" => "mpc_residency",
                    "relay" => "relay_hops",
                    "wasm" => "wasm_vs_native",
                    "notes" => "anonymity_set",
                    "rings" => "ring_anonymity",
                    "evm" => "evm_blocks",
                    "placement" => "node_placement",
                    other => other,
                };
                drawn.push(save(canvas, stem, &output, &temp)?);
                println!("  drew {stem}");
            }
            Err(missing) => skipped.push((function_name, missing)),
        }
    }
    fs::remove_dir(&temp)?;
    if !skipped.is_empty() {
        println!("\nskipped, because their measurements are not in this checkout:");
        for (name, missing) in skipped {
            println!("  {name}: needs {missing}");
        }
    }
    println!("\n{} figure(s) in {}", drawn.len(), output.display());
    Ok(())
}

fn fig_rho(art: &Path) -> Figure {
    let Some(data) = load(art, "rho_sweep.json")? else {
        return Ok(Err("rho_sweep.json (make rho-sweep)".into()));
    };
    let arms: Vec<&str> = ["generated", "tape"]
        .into_iter()
        .filter(|arm| data["arms"].get(*arm).is_some())
        .collect();
    let mut canvas = Canvas::new(if arms.len() > 1 { 1440 } else { 780 }, 560);
    for (index, arm) in arms.iter().enumerate() {
        let mut series = Vec::new();
        for (protocol, colour, name) in [
            ("qomm_rfq", OBLIVIOUS, "query-oblivious"),
            ("plain_rfq", PLAIN, "plain RFQ"),
        ] {
            let mut points: Vec<(f64, f64)> = data["arms"][*arm]["rows"]
                .as_array()
                .ok_or("rho rows is not an array")?
                .iter()
                .filter(|row| row["protocol"] == protocol)
                .map(|row| Ok((center(&row["linkage_rho"])?, center(&row["auc_mean"])?)))
                .collect::<HarnessResult<_>>()?;
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            if !points.is_empty() {
                series.push(Series {
                    label: name.into(),
                    colour,
                    points,
                    dashed: false,
                });
            }
        }
        let source = data["arms"][*arm]["meta"]["source"].as_str().unwrap_or(arm);
        line_chart(
            &mut canvas,
            Rect {
                x: index as f64 * 710.0 + 10.0,
                y: 10.0,
                w: 700.0,
                h: 530.0,
            },
            source,
            "fraction of wallets the adversary can already attribute",
            "detection AUC",
            &series,
            Some((0.45, 1.03)),
            Some(0.5),
        );
    }
    Ok(Ok(canvas))
}

fn fig_settlement_cost(art: &Path) -> Figure {
    let Some(python) = load(art, "defmi.json")? else {
        return Ok(Err("defmi.json (make defmi)".into()));
    };
    let rust = load(art, "rust_bench.json")?;
    let scaling = python["scaling"]
        .as_array()
        .ok_or("defmi scaling is not an array")?;
    let mut left = vec![Series {
        label: "Python, bit decomposition".into(),
        colour: PLAIN,
        points: scaling
            .iter()
            .map(|r| Ok((center(&r["bits"])?, center(&r["settle"])?)))
            .collect::<HarnessResult<_>>()?,
        dashed: false,
    }];
    let mut right = vec![Series {
        label: "Python".into(),
        colour: PLAIN,
        points: scaling
            .iter()
            .map(|r| Ok((center(&r["bits"])?, center(&r["package_bytes"])? / 1024.0)))
            .collect::<HarnessResult<_>>()?,
        dashed: false,
    }];
    if let Some(rust) = rust {
        let rows = rust["scaling"]
            .as_array()
            .ok_or("rust scaling is not an array")?;
        left.push(Series {
            label: "Rust, aggregated range proofs".into(),
            colour: OBLIVIOUS,
            points: rows
                .iter()
                .map(|r| Ok((center(&r["bits"])?, center(&r["settle_ms"])?)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        });
        right.push(Series {
            label: "Rust".into(),
            colour: OBLIVIOUS,
            points: rows
                .iter()
                .map(|r| Ok((center(&r["bits"])?, center(&r["package_bytes"])? / 1024.0)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        });
    }
    let mut canvas = Canvas::new(1440, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "settlement verification",
        "ledger balance width (bits)",
        "ms",
        &left,
        None,
        None,
    );
    line_chart(
        &mut canvas,
        Rect {
            x: 720.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "wire size",
        "ledger balance width (bits)",
        "KiB",
        &right,
        None,
        None,
    );
    Ok(Ok(canvas))
}

fn fig_parallel(art: &Path) -> Figure {
    let Some(local) = load(art, "defmi.json")? else {
        return Ok(Err("defmi.json with a parallel section".into()));
    };
    let Some(local_rows) = local
        .get("parallel")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    else {
        return Ok(Err("defmi.json with a parallel section".into()));
    };
    let big = load(art, "defmi_host_a.json")?;
    let mut series = Vec::new();
    for (rows, colour, name) in [
        (Some(local_rows), PLAIN, "host-c"),
        (
            big.as_ref()
                .and_then(|v| v.get("parallel"))
                .and_then(Value::as_array),
            OBLIVIOUS,
            "host-a",
        ),
    ] {
        let Some(rows) = rows.filter(|v| !v.is_empty()) else {
            continue;
        };
        let points: Vec<(f64, f64)> = rows
            .iter()
            .map(|r| {
                Ok((
                    center(&r["workers"])?.log2(),
                    center(&r["per_second"])?.log10(),
                ))
            })
            .collect::<HarnessResult<_>>()?;
        let first = center(&rows[0]["per_second"])?;
        let ideal = rows
            .iter()
            .map(|r| {
                Ok((
                    center(&r["workers"])?.log2(),
                    (first * center(&r["workers"])?).log10(),
                ))
            })
            .collect::<HarnessResult<_>>()?;
        series.push(Series {
            label: name.into(),
            colour,
            points,
            dashed: false,
        });
        series.push(Series {
            label: format!("{name}, perfect"),
            colour,
            points: ideal,
            dashed: true,
        });
    }
    let mut canvas = Canvas::new(800, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "settlement verification scales with cores (dotted: perfect scaling)",
        "worker processes (log2)",
        "settlements per second (log10)",
        &series,
        None,
        None,
    );
    Ok(Ok(canvas))
}

fn fig_residency(art: &Path) -> Figure {
    let Some(data) = load(art, "mpc_resident.json")? else {
        return Ok(Err("mpc_resident.json (make mpc-resident)".into()));
    };
    let cold = data["cold"].as_array().ok_or("cold is not an array")?;
    let warm = data["resident"]
        .as_array()
        .ok_or("resident is not an array")?;
    let log_points = |rows: &Vec<Value>, key: &str| -> HarnessResult<Vec<(f64, f64)>> {
        rows.iter()
            .map(|r| Ok((center(&r["batch"])?.log2(), center(&r[key])?.log10())))
            .collect()
    };
    let first = vec![
        Series {
            label: "compiler runs every time".into(),
            colour: PLAIN,
            points: log_points(cold, "ms_per_quote")?,
            dashed: false,
        },
        Series {
            label: "compiled circuit kept".into(),
            colour: OBLIVIOUS,
            points: log_points(warm, "ms_per_quote")?,
            dashed: false,
        },
    ];
    let second = vec![
        Series {
            label: "protocol".into(),
            colour: OBLIVIOUS,
            points: warm
                .iter()
                .map(|r| {
                    Ok((
                        center(&r["batch"])?.log2(),
                        center(&r["protocol_ms_per_quote"])?,
                    ))
                })
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
        Series {
            label: "fixed cost".into(),
            colour: NEUTRAL,
            points: warm
                .iter()
                .map(|r| {
                    Ok((
                        center(&r["batch"])?.log2(),
                        center(&r["overhead_ms_per_quote"])?,
                    ))
                })
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
    ];
    let mut canvas = Canvas::new(1440, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "cost of one quote",
        "requests per job (log2)",
        "ms per quote (log10)",
        &first,
        None,
        None,
    );
    line_chart(
        &mut canvas,
        Rect {
            x: 720.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "where the time goes once the circuit is kept",
        "requests per job (log2)",
        "ms per quote",
        &second,
        None,
        None,
    );
    Ok(Ok(canvas))
}

fn symbol(source: &str) -> &str {
    source.split_once("bybit:").map_or(source, |(_, tail)| tail)
}

fn fig_real_market(art: &Path) -> Figure {
    let Some(data) = load(art, "sim_matrix_bybit.json")? else {
        return Ok(Err("sim_matrix_bybit.json (make sim-real)".into()));
    };
    let aggregate = data["aggregate"]
        .as_array()
        .ok_or("aggregate is not an array")?;
    let raw = data["rows"].as_array().ok_or("rows is not an array")?;
    let mut rates = BTreeMap::new();
    for row in raw {
        if let Some(source) = row["tape"]["source"].as_str() {
            rates.insert(
                symbol(source).to_string(),
                center(&row["tape"]["arrival_per_s"])?,
            );
        }
    }
    let mut left = Vec::new();
    for (protocol, colour, name) in [
        ("qomm_rfq", OBLIVIOUS, "query-oblivious"),
        ("plain_rfq", PLAIN, "plain RFQ"),
    ] {
        let mut points = Vec::new();
        for row in aggregate.iter().filter(|r| {
            r["protocol"] == protocol && r["disclosure"] == "A_none" && r["layer"] == "replay"
        }) {
            let Some(source) = row["source"].as_str() else {
                continue;
            };
            if !row["A1_passive_observer.auc"].is_null() {
                if let Some(rate) = rates.get(symbol(source)) {
                    points.push((rate.log10(), center(&row["A1_passive_observer.auc"])?));
                }
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        left.push(Series {
            label: name.into(),
            colour,
            points,
            dashed: false,
        });
    }
    let mut right = Vec::new();
    for (disclosure, colour, name) in [
        ("C_dp", OBLIVIOUS, "differentially private"),
        ("B_threshold", PLAIN, "threshold"),
    ] {
        let mut points = Vec::new();
        for row in aggregate.iter().filter(|r| {
            r["protocol"] == "plain_rfq"
                && r["disclosure"] == disclosure
                && r["layer"] == "reactive"
        }) {
            let Some(source) = row["source"].as_str() else {
                continue;
            };
            if !row["suppression_rate"].is_null() {
                if let Some(rate) = rates.get(symbol(source)) {
                    points.push((rate.log10(), center(&row["suppression_rate"])?));
                }
            }
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        right.push(Series {
            label: name.into(),
            colour,
            points,
            dashed: false,
        });
    }
    let mut canvas = Canvas::new(1440, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "detection across eight real symbols",
        "trades per second (log10)",
        "detection AUC",
        &left,
        Some((0.45, 0.85)),
        Some(0.5),
    );
    line_chart(
        &mut canvas,
        Rect {
            x: 720.0,
            y: 10.0,
            w: 700.0,
            h: 530.0,
        },
        "how often disclosure is withheld",
        "trades per second (log10)",
        "fraction of windows suppressed",
        &right,
        Some((-0.05, 1.05)),
        None,
    );
    Ok(Ok(canvas))
}

fn fig_relay(art: &Path) -> Figure {
    let Some(across) = load(art, "transport.json")? else {
        return Ok(Err("transport.json (make transport)".into()));
    };
    let within = load(art, "transport_colocated.json")?;
    let mut series = Vec::new();
    for (data, colour, name) in [
        (within.as_ref(), OBLIVIOUS, "within one site (0.43 ms)"),
        (Some(&across), PLAIN, "between sites (8.7 ms)"),
    ] {
        let Some(data) = data else {
            continue;
        };
        let mut points = data["by_hops"]
            .as_array()
            .ok_or("by_hops is not an array")?
            .iter()
            .map(|r| {
                Ok((
                    center(&r["hops"])?,
                    r["slot_wall_median_ms"].as_f64().unwrap_or(0.0),
                ))
            })
            .collect::<HarnessResult<Vec<_>>>()?;
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        series.push(Series {
            label: name.into(),
            colour,
            points,
            dashed: false,
        });
    }
    let mut canvas = Canvas::new(800, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "each relay keeps its own clock",
        "relay hops",
        "ms per slot",
        &series,
        None,
        None,
    );
    Ok(Ok(canvas))
}

fn fig_wasm(art: &Path) -> Figure {
    let Some(native) = load(art, "wasm_native.json")? else {
        return Ok(Err(
            "wasm_native.json and wasm_wasm32.json (make wasm-bench)".into(),
        ));
    };
    let Some(wasm) = load(art, "wasm_wasm32.json")? else {
        return Ok(Err(
            "wasm_native.json and wasm_wasm32.json (make wasm-bench)".into(),
        ));
    };
    let native_rows = native["scaling"]
        .as_array()
        .ok_or("native scaling is not array")?;
    let wasm_rows = wasm["scaling"]
        .as_array()
        .ok_or("wasm scaling is not array")?;
    let labels = native_rows
        .iter()
        .map(|r| format!("{} bit", r["bits"].as_i64().unwrap_or(0)))
        .collect::<Vec<_>>();
    let native_values = native_rows
        .iter()
        .map(|r| center(&r["settle_ms"]))
        .collect::<HarnessResult<Vec<_>>>()?;
    let wasm_values = wasm_rows
        .iter()
        .map(|r| center(&r["settle_ms"]))
        .collect::<HarnessResult<Vec<_>>>()?;
    let mut canvas = Canvas::new(800, 560);
    bar_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "settlement verification, same source",
        "ms",
        &labels,
        &[
            ("native", OBLIVIOUS, native_values),
            ("WebAssembly", PLAIN, wasm_values),
        ],
        None,
    );
    Ok(Ok(canvas))
}

fn fig_notes(art: &Path) -> Figure {
    let Some(data) = load(art, "defmi.json")? else {
        return Ok(Err("defmi.json with a notes section".into()));
    };
    let Some(rings) = data
        .get("notes")
        .and_then(|n| n.get("rings"))
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
    else {
        return Ok(Err("defmi.json with a notes section".into()));
    };
    let series = vec![
        Series {
            label: "payer proves".into(),
            colour: PLAIN,
            points: rings
                .iter()
                .map(|r| Ok((center(&r["ring"])?.log2(), center(&r["build"])?)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
        Series {
            label: "node verifies".into(),
            colour: OBLIVIOUS,
            points: rings
                .iter()
                .map(|r| Ok((center(&r["ring"])?.log2(), center(&r["check"])?)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
    ];
    let mut canvas = Canvas::new(800, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "the anonymity set is bounded by the payer, not the node",
        "candidates the spend hides among (log2)",
        "ms",
        &series,
        None,
        None,
    );
    Ok(Ok(canvas))
}

fn fig_rings(art: &Path) -> Figure {
    let Some(data) = load(art, "rings.json")? else {
        return Ok(Err("rings.json (make rings-bench)".into()));
    };
    let rows = data["rows"].as_array().ok_or("ring rows is not array")?;
    let sizes: Vec<i64> = rows
        .iter()
        .filter_map(|r| r["ring"].as_i64())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut series = Vec::new();
    for (decoys, colour) in [("uniform", PLAIN), ("recent", OBLIVIOUS)] {
        for size in &sizes {
            let mut points = rows
                .iter()
                .filter(|r| r["decoys"] == decoys && r["ring"].as_i64() == Some(*size))
                .map(|r| Ok((center(&r["traffic"])?, center(&r["observer_success"])?)))
                .collect::<HarnessResult<Vec<_>>>()?;
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            if !points.is_empty() {
                series.push(Series {
                    label: format!("{decoys}, ring {size}"),
                    colour,
                    points,
                    dashed: decoys == "uniform",
                });
            }
        }
    }
    let mut canvas = Canvas::new(800, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "what a chain reader names, and what the proof promises",
        "other settlements between being paid and paying",
        "the observer is right this often",
        &series,
        Some((0.0, 1.05)),
        None,
    );
    Ok(Ok(canvas))
}

fn fig_evm(art: &Path) -> Figure {
    let Some(data) = load(art, "evm_settlement.json")? else {
        return Ok(Err("evm_settlement.json (make evm-gas)".into()));
    };
    let rows = data["scaling"]
        .as_array()
        .ok_or("evm scaling is not array")?;
    let labels = rows
        .iter()
        .map(|r| r["bits"].as_i64().unwrap_or(0).to_string())
        .collect::<Vec<_>>();
    let values = rows
        .iter()
        .map(|r| center(&r["blocks"]))
        .collect::<HarnessResult<Vec<_>>>()?;
    let gas = data["unit"]["gas"].as_i64().unwrap_or(0);
    let mut canvas = Canvas::new(800, 560);
    bar_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        &format!("one settlement verified on an EVM, at {gas} gas a scalar multiplication"),
        "blocks of gas",
        &labels,
        &[("settlement", PLAIN, values)],
        Some(1.0),
    );
    Ok(Ok(canvas))
}

fn fig_placement(art: &Path) -> Figure {
    let Some(data) = load(art, "placement.json")? else {
        return Ok(Err(
            "placement.json (make placement, needs seven parties)".into()
        ));
    };
    let rows = data["rows"]
        .as_array()
        .ok_or("placement rows is not array")?;
    let labels = rows
        .iter()
        .map(|r| r["placement"].as_str().unwrap_or("?").to_string())
        .collect::<Vec<_>>();
    let values = rows
        .iter()
        .map(|r| center(&r["wall_median_s"]))
        .collect::<HarnessResult<Vec<_>>>()?;
    let mut canvas = Canvas::new(850, 560);
    bar_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 820.0,
            h: 530.0,
        },
        &format!(
            "node placement ({:.0} ms near, {:.0} ms far)",
            center(&data["near_ms"])?,
            center(&data["far_ms"])?
        ),
        "seconds per quote",
        &labels,
        &[("wall clock", ACCENT, values)],
        None,
    );
    Ok(Ok(canvas))
}

fn fig_state_audit(art: &Path) -> Figure {
    let Some(data) = load(art, "state_audit.json")? else {
        return Ok(Err("state_audit.json (make state-audit)".into()));
    };
    let rows = data["chains"]
        .as_array()
        .ok_or("state chains is not array")?;
    let series = vec![
        Series {
            label: "maker proves".into(),
            colour: PLAIN,
            points: rows
                .iter()
                .map(|r| Ok((center(&r["steps"])?, center(&r["prove_per_step"])?)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
        Series {
            label: "venue verifies".into(),
            colour: OBLIVIOUS,
            points: rows
                .iter()
                .map(|r| Ok((center(&r["steps"])?, center(&r["verify_ms_per_step"])?)))
                .collect::<HarnessResult<_>>()?,
            dashed: false,
        },
    ];
    let ymax = rows
        .iter()
        .map(|r| center(&r["verify_ms_per_step"]))
        .collect::<HarnessResult<Vec<_>>>()?
        .into_iter()
        .fold(0.0, f64::max)
        * 1.4;
    let mut canvas = Canvas::new(800, 560);
    line_chart(
        &mut canvas,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 770.0,
            h: 530.0,
        },
        "auditing one inventory update",
        "fills already in the chain",
        "ms per fill",
        &series,
        Some((0.0, ymax)),
        None,
    );
    Ok(Ok(canvas))
}

fn fig_dp_effect(art: &Path) -> Figure {
    let Some(data) = load(art, "dp_effect.json")? else {
        return Ok(Err("dp_effect.json (make dp-effect)".into()));
    };
    let arms: Vec<&str> = ["generated", "tape"]
        .into_iter()
        .filter(|a| data["arms"].get(*a).is_some())
        .collect();
    let mut canvas = Canvas::new(if arms.len() > 1 { 1440 } else { 780 }, 560);
    for (index, arm) in arms.iter().enumerate() {
        let paired = &data["arms"][*arm]["paired_against_no_disclosure"];
        let mut labels = Vec::new();
        let mut plain = Vec::new();
        let mut corrected = Vec::new();
        for (kind, label) in [
            ("dp_uncorrected", "as published"),
            ("dp_corrected", "corrected"),
        ] {
            let stat = &paired[kind]["fill_rate"];
            if stat["mean"].is_null() {
                continue;
            }
            labels.push(label.to_string());
            if kind == "dp_uncorrected" {
                plain.push(center(&stat["mean"])?);
                corrected.push(0.0);
            } else {
                plain.push(0.0);
                corrected.push(center(&stat["mean"])?);
            }
        }
        let source = data["arms"][*arm]["meta"]["source"].as_str().unwrap_or(arm);
        bar_chart(
            &mut canvas,
            Rect {
                x: index as f64 * 710.0 + 10.0,
                y: 10.0,
                w: 700.0,
                h: 530.0,
            },
            &format!("fill rate against publishing nothing — {source}"),
            "difference",
            &labels,
            &[
                ("as published", PLAIN, plain),
                ("corrected", OBLIVIOUS, corrected),
            ],
            Some(0.0),
        );
    }
    Ok(Ok(canvas))
}
