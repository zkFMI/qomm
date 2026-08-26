//! Rust port of `scripts/run_multiplication_cost.py`.

use qomm_harness::local_mpc::{maybe_run_party, LocalMpcRun};
use qomm_harness::{
    parse_value, timing_summary, unique_temp_dir, write_pretty_json, HarnessResult,
};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

const PROGRAM: &str = r#"
# {n_mults} multiplications in ONE SIMD row, so the round count stays at three
# and only traffic moves with the size. The first version of this ran a
# sequential `for_range` and measured 22,002 rounds for 22,000 multiplications
# --- a multiplication on the critical path, which is a different and much more
# expensive thing than a multiplication. The quote circuit batches; so does this.
a = sint.get_input_from(0)
b = sint.get_input_from(1)
x = a.expand_to_vector({n_mults})
y = b.expand_to_vector({n_mults})
arr = sint.Array({n_mults})
arr.assign(x * y)
print_ln('%s', arr.sum().reveal())
"#;

const CONTROL: &str = r#"
a = sint.get_input_from(0)
b = sint.get_input_from(1)
x = a.expand_to_vector({n_mults})
arr = sint.Array({n_mults})
arr.assign(x + b.expand_to_vector({n_mults}))
print_ln('%s', arr.sum().reveal())
"#;

struct Options {
    root: PathBuf,
    parties: usize,
    threshold: usize,
    sizes: [usize; 2],
    field_bits: usize,
    repeats: usize,
    protocols: Option<Vec<(String, String)>>,
    prime: Option<String>,
    runtime_options: Option<String>,
    file_prep: bool,
    control: bool,
    out: PathBuf,
}

fn main() {
    if maybe_run_party() {
        return;
    }
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.repeats == 0 || options.sizes[0] == options.sizes[1] {
        return Err("--repeats must be positive and --sizes must differ".into());
    }
    let mut sizes = options.sizes;
    sizes.sort_unstable();
    let delta = sizes[1] - sizes[0];
    let custom_protocols = options.protocols.is_some();
    let protocols = options.protocols.clone().unwrap_or_else(|| {
        let mut values = vec![("malicious".into(), "malicious-shamir-party.x".into())];
        if options.control {
            values.push(("semi_honest".into(), "shamir-party.x".into()));
        }
        values
    });
    let mut arms = Map::new();
    for (name, binary) in protocols {
        let mut rows = Vec::new();
        for size in sizes {
            rows.push(one_size(
                &options,
                size,
                &binary,
                PROGRAM,
                &name[..name.len().min(3)],
            )?);
        }
        let per_multiplication = slope(&rows, options.parties, options.field_bits, delta)?;
        let mut arm = json!({
            "binary": binary,
            "rows": rows,
            "per_multiplication": per_multiplication,
        });
        if options.control && !custom_protocols {
            let mut control_rows = Vec::new();
            for size in sizes {
                control_rows.push(one_size(
                    &options,
                    size,
                    &binary,
                    CONTROL,
                    &format!("{}c", &name[..name.len().min(3)]),
                )?);
            }
            let overhead = slope(&control_rows, options.parties, options.field_bits, delta)?;
            let net = py_round_places(
                arm["per_multiplication"]["per_party_elements"]
                    .as_f64()
                    .unwrap_or(0.0)
                    - overhead["per_party_elements"].as_f64().unwrap_or(0.0),
                3,
            );
            arm["control_no_multiplications"] = json!({"rows": control_rows, "slope": overhead});
            arm["per_multiplication_net"] = json!({"per_party_elements": net});
        }
        arms.insert(name, arm);
    }
    let first = arms.values().next().ok_or("no protocol arms")?;
    let here = first
        .get("per_multiplication_net")
        .unwrap_or(&first["per_multiplication"])["per_party_elements"]
        .as_f64()
        .unwrap_or(0.0);
    let ratio = (here != 0.0).then(|| py_round_places(5.5 / here, 2));
    let payload = json!({
        "host": qomm_measure::hosts::this_host(),
        "n_parties": {"exact": options.parties},
        "threshold": {"exact": options.threshold},
        "field_bits": {"exact": options.field_bits},
        "security_of_the_baseline": "with abort. MP-SPDZ's README: 'malicious means that not following the protocol will at least be detected'. No party is named and the protocol stops.",
        "arms": arms,
        "file_prep": {"exact": options.file_prep},
        "comparison": {
            "gsz2020_god_best_case_elements_per_party": 5.5,
            "gsz2020_god_after_identification": 7.5,
            "gsz2020_best_semi_honest": 5.5,
            "gsz2020_threshold": "t < n/2 assuming broadcast; at t < n/3 the broadcast channel can be simulated over point-to-point links",
            "this_deployment": "n=7, T=2, which is t < n/3 (2 < 2.33), so no broadcast channel has to be assumed",
            "measured_here": here,
            "god_over_this_baseline": ratio,
        },
        "reading": "A ratio near or below 1 means guaranteed output delivery costs about what this engine already spends, so 'comes free' holds against the baseline actually deployed and the gap is an implementation. Well above 1 means 'free' was said about a protocol we are not running.",
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

fn one_size(
    options: &Options,
    n_mults: usize,
    binary: &str,
    template: &str,
    tag: &str,
) -> HarnessResult<Value> {
    let work = unique_temp_dir("qomm-mult-cost")?;
    let source = work.join("prog.mpc");
    fs::write(&source, template.replace("{n_mults}", &n_mults.to_string()))?;
    let party_files = (0..options.parties)
        .map(|_| "7\n".to_string())
        .collect::<Vec<_>>();
    let program = format!("multcost{tag}{n_mults}_{}", std::process::id());
    let mut run = LocalMpcRun::new(
        options.root.canonicalize()?,
        program,
        options.parties,
        options.threshold,
        protocol_name(binary),
        None,
    )?;
    run.install(&source, &party_files)?;
    let compile_rounds = run.compile(options.field_bits)?;
    let mut extra = Vec::new();
    if let Some(runtime_options) = &options.runtime_options {
        extra.extend(["--options".into(), runtime_options.clone()]);
    }
    if options.file_prep {
        extra.push("-F".into());
        if let Some(prime) = &options.prime {
            extra.extend(["-P".into(), prime.clone()]);
        }
    }
    let mut global = Vec::new();
    let mut rounds = None;
    for _ in 0..options.repeats {
        let sample = run.execute_stock(binary, &extra)?;
        global.push(
            sample
                .global_mb
                .ok_or("MP-SPDZ did not report global data")?,
        );
        rounds.get_or_insert(
            sample
                .party0_rounds
                .ok_or("MP-SPDZ did not report rounds")?,
        );
    }
    let _ = fs::remove_dir_all(work);
    Ok(json!({
        "n_mults": {"exact": n_mults},
        "field_bits": {"exact": options.field_bits},
        "compile_rounds": compile_rounds,
        "global_mb": timing_summary(&global),
        "party0_rounds": {"exact": rounds.ok_or("no runtime samples")?},
    }))
}

fn slope(rows: &[Value], parties: usize, field_bits: usize, delta: usize) -> HarnessResult<Value> {
    let delta_mb = rows[1]["global_mb"]["median"]
        .as_f64()
        .ok_or("large median missing")?
        - rows[0]["global_mb"]["median"]
            .as_f64()
            .ok_or("small median missing")?;
    let global_bytes = delta_mb * 1e6 / delta as f64;
    let per_party = global_bytes / parties as f64;
    Ok(json!({
        "global_bytes": py_round_places(global_bytes, 2),
        "per_party_bytes": py_round_places(per_party, 2),
        "per_party_elements": py_round_places(per_party / (field_bits as f64 / 8.0), 3),
    }))
}

fn protocol_name(binary: &str) -> &'static str {
    if binary == "shamir-party.x" {
        "semi-honest-shamir"
    } else {
        "malicious-shamir"
    }
}

fn py_round_places(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    qomm_sim::market::py_round(value * scale) as f64 / scale
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        root: PathBuf::new(),
        parties: 7,
        threshold: 2,
        sizes: [2_000, 22_000],
        field_bits: 128,
        repeats: 3,
        protocols: None,
        prime: None,
        runtime_options: None,
        file_prep: false,
        control: false,
        out: qomm_harness::repo_root().join("artifacts/multiplication_cost.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--root" => options.root = PathBuf::from(value(&raw, &mut index, "--root")?),
            "--parties" => {
                options.parties = parse_value(value(&raw, &mut index, "--parties")?, "--parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--field-bits" => {
                options.field_bits =
                    parse_value(value(&raw, &mut index, "--field-bits")?, "--field-bits")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--prime" => {
                options.prime = Some(
                    value(&raw, &mut index, "--prime")?
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            "--options" => {
                options.runtime_options = Some(
                    value(&raw, &mut index, "--options")?
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            "--file-prep" => options.file_prep = true,
            "--control" => options.control = true,
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--sizes" => {
                let first = parse_value(value(&raw, &mut index, "--sizes")?, "--sizes")?;
                let second = parse_value(value(&raw, &mut index, "--sizes")?, "--sizes")?;
                options.sizes = [first, second];
            }
            "--protocols" => {
                let mut protocols = Vec::new();
                index += 1;
                while index < raw.len() && !raw[index].to_string_lossy().starts_with("--") {
                    let pair = raw[index].to_string_lossy();
                    let (name, binary) = pair
                        .split_once('=')
                        .ok_or("--protocols entries must be name=binary")?;
                    protocols.push((name.into(), binary.into()));
                    index += 1;
                }
                options.protocols = Some(protocols);
                continue;
            }
            unknown => return Err(format!("unknown argument {unknown}").into()),
        }
        index += 1;
    }
    if options.root.as_os_str().is_empty() {
        return Err("--root is required".into());
    }
    Ok(options)
}

fn value(raw: &[OsString], index: &mut usize, name: &str) -> HarnessResult<OsString> {
    *index += 1;
    raw.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} expects a value").into())
}
