use qomm_harness::{
    median, parse_value, timing_summary, unique_temp_dir, write_pretty_json, HarnessResult,
};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Options {
    mp_spdz_root: PathBuf,
    out: PathBuf,
    near: f64,
    far: f64,
    n_mm: usize,
    n_parties: usize,
    bit_length: usize,
    repeats: usize,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let cases = placements(options.near, options.far, options.n_parties);
    let mut rows = Vec::new();
    for (name, per_party) in cases {
        let mut walls = Vec::new();
        let mut rounds = Vec::new();
        let mut verified = Vec::new();
        for _ in 0..options.repeats {
            let payload = run_qomm(&options, &name, &per_party)?;
            let samples = payload["samples"].as_array().cloned().unwrap_or_default();
            let wall = payload["wall_median"].as_f64().or_else(|| {
                median(
                    &samples
                        .iter()
                        .filter_map(|sample| sample["wall_seconds"].as_f64())
                        .collect::<Vec<_>>(),
                )
            });
            walls.push(wall.ok_or("run_qomm returned no wall timing")?);
            // calls this `measured_rounds`, so the artifact records exact null.
            rounds.push(payload.get("party_rounds").cloned().unwrap_or(Value::Null));
            verified.push(payload["verified"].as_bool().unwrap_or(false));
        }
        let row = json!({
            "placement": name,
            "per_party_ms": per_party,
            "wall_s": timing_summary(&walls),
            "rounds": {"exact": rounds.first().cloned().unwrap_or(Value::Null)},
            "verified": verified.into_iter().all(|value| value),
        });
        println!(
            "  {:10} {}  rounds {}  verified={}",
            row["placement"].as_str().unwrap_or_default(),
            render_seconds(&row["wall_s"]),
            qomm_harness::value_display(&row["rounds"]["exact"]),
            qomm_harness::value_display(&row["verified"]),
        );
        rows.push(row);
    }

    let base = row_named(&rows, "all near")?;
    let worst = row_named(&rows, "all far")?;
    let one = row_named(&rows, "one far")?;
    let spread = row_median(worst)? - row_median(base)?;
    let share =
        (spread != 0.0).then(|| (row_median(one).unwrap() - row_median(base).unwrap()) / spread);
    println!(
        "\none distant node costs {:.0}% of moving them all",
        100.0 * share.unwrap_or(0.0)
    );
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "near_ms": options.near,
        "far_ms": options.far,
        "n_mm": options.n_mm,
        "n_parties": options.n_parties,
        "bit_length": options.bit_length,
        "repeats": options.repeats,
        "rows": rows,
        "one_far_share_of_all_far": share,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn placements(near: f64, far: f64, parties: usize) -> Vec<(String, Vec<f64>)> {
    vec![
        ("all near".into(), vec![near; parties]),
        (
            "one far".into(),
            (0..parties)
                .map(|index| if index + 1 == parties { far } else { near })
                .collect(),
        ),
        (
            "one near".into(),
            (0..parties)
                .map(|index| if index + 1 == parties { near } else { far })
                .collect(),
        ),
        ("all far".into(), vec![far; parties]),
    ]
}

fn run_qomm(options: &Options, name: &str, per_party: &[f64]) -> HarnessResult<Value> {
    let executable = std::env::current_exe()?
        .parent()
        .ok_or("run_placement executable has no parent")?
        .join("run_qomm");
    let temp = unique_temp_dir("qomm-placement")?;
    let output_path = temp.join(format!("placement_{}.json", name.replace(' ', "_")));
    let mut command = Command::new(executable);
    command
        .args(["--mp-spdz-root", &options.mp_spdz_root.to_string_lossy()])
        .args(["--n-mm", &options.n_mm.to_string()])
        .args(["--n-parties", &options.n_parties.to_string()])
        .args(["--bit-length", &options.bit_length.to_string()])
        .args(["--repeats", "1"])
        .arg("--per-party-ms");
    for value in per_party {
        command.arg(value.to_string());
    }
    command.args(["--out", &output_path.to_string_lossy()]);
    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .chars()
            .rev()
            .take(800)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        let _ = fs::remove_dir_all(temp);
        return Err(format!("{name} failed:\n{tail}").into());
    }
    let payload = serde_json::from_slice(&fs::read(&output_path)?)?;
    let _ = fs::remove_dir_all(temp);
    Ok(payload)
}

fn row_named<'a>(rows: &'a [Value], name: &str) -> HarnessResult<&'a Value> {
    rows.iter()
        .find(|row| row["placement"] == name)
        .ok_or_else(|| format!("missing placement {name}").into())
}

fn row_median(row: &Value) -> HarnessResult<f64> {
    row["wall_s"]["median"]
        .as_f64()
        .or_else(|| row["wall_s"]["mean"].as_f64())
        .ok_or_else(|| "placement timing summary is empty".into())
}

fn render_seconds(summary: &Value) -> String {
    let n = summary["n"].as_u64().unwrap_or(0);
    if n == 0 {
        return "—".into();
    }
    let mean = summary["mean"].as_f64().unwrap_or(0.0);
    match summary["sd"].as_f64() {
        Some(sd) => format!("{mean:.3} ± {sd:.3} s (n={n})"),
        None => format!("{mean:.3} s (n=1)"),
    }
}

fn parse_args() -> HarnessResult<Options> {
    let default_root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("work/qomm/MP-SPDZ");
    let mut options = Options {
        mp_spdz_root: default_root,
        out: qomm_harness::repo_root().join("artifacts/placement.json"),
        near: 1.0,
        far: 15.0,
        n_mm: 16,
        n_parties: 7,
        bit_length: 31,
        repeats: 3,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root = PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--near" => options.near = parse_value(value(&raw, &mut index, "--near")?, "--near")?,
            "--far" => options.far = parse_value(value(&raw, &mut index, "--far")?, "--far")?,
            "--n-mm" => options.n_mm = parse_value(value(&raw, &mut index, "--n-mm")?, "--n-mm")?,
            "--n-parties" => {
                options.n_parties =
                    parse_value(value(&raw, &mut index, "--n-parties")?, "--n-parties")?
            }
            "--bit-length" => {
                options.bit_length =
                    parse_value(value(&raw, &mut index, "--bit-length")?, "--bit-length")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
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
