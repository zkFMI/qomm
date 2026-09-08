use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use qomm_harness::{parse_value, timing_summary, write_pretty_json, HarnessResult};
use qomm_transport::roles::{check_field_width, dealt_body, split, ComputingNode, InputParty};
use rand::rngs::OsRng;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;
use zkfmi_zk::pedersen::Pedersen;
use zkpi_committee::application_crypto::SigningKey;

struct Options {
    out: PathBuf,
    n_nodes: usize,
    value_bits: u32,
    field_bits: u32,
    repeats: usize,
    group: String,
    makers: usize,
}

struct CommittedDealing {
    value_commitment: RistrettoPoint,
    share_commitments: Vec<RistrettoPoint>,
    shares: Vec<i128>,
    blindings: Vec<Scalar>,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.group != "ed25519" {
        return Err(
            "the native harness supports the repository's ed25519 measurement group".into(),
        );
    }
    if options.repeats == 0 || options.makers == 0 {
        return Err("--repeats and --makers must be positive".into());
    }
    check_field_width(options.n_nodes, options.value_bits, options.field_bits)?;
    let key = Pedersen::new(b"qomm:maker-update:v1");
    let policy = vec![
        ("asset", 0),
        ("ask_level", 37),
        ("spread", 44),
        ("slope", 2),
        ("invcoef", 1),
        ("inv", -140),
        ("maxqty", 500),
        ("expiry", 900_000),
        ("active", 1),
    ];
    let view = vec![("ask_level", 37)];
    let mut rows = Vec::new();
    for (scope, fields) in [
        ("full policy", policy.as_slice()),
        ("view only", view.as_slice()),
    ] {
        for arm in ["split only", "signed", "signed+committed"] {
            let mut row = measure(arm, fields, &options, &key)?;
            row["scope"] = json!(scope);
            row["sustainable_per_maker"] = row["node_updates_per_second_per_core"]
                .get("median")
                .and_then(Value::as_f64)
                .map(|node| {
                    row["updates_per_second"]["median"]
                        .as_f64()
                        .unwrap_or(f64::INFINITY)
                        .min(node / options.makers as f64)
                })
                .map_or(Value::Null, |value| json!(value));
            println!(
                "{scope:12} {arm:17} {} B  ok={}",
                row["bytes_on_the_wire"]["exact"],
                py_bool(row["verified"] == true && row["node_accepted"] == true),
            );
            rows.push(row);
        }
    }
    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "n_nodes": options.n_nodes,
        "value_bits": options.value_bits,
        "group": options.group,
        "repeats": options.repeats,
        "n_makers": options.makers,
        "note": "node rates are one core. Verification is per-share and independent, so a node with more cores scales; the maker side does not, because one maker is one dealer.",
        "policy_fields": policy.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        "rows": rows,
    });
    let accepted = payload["rows"]
        .as_array()
        .is_some_and(|rows| rows.iter().all(|row| row["verified"] == true));
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    if !accepted {
        return Err("a maker update failed to reconstruct".into());
    }
    Ok(())
}

fn measure(
    arm: &str,
    fields: &[(&str, i128)],
    options: &Options,
    key: &Pedersen,
) -> HarnessResult<Value> {
    let signing = SigningKey::generate(&mut OsRng);
    let maker = InputParty {
        name: "mm-00".into(),
        n_nodes: options.n_nodes,
        value_bits: options.value_bits,
        signing_key: Some(signing),
    };
    let values = fields.iter().map(|(_, value)| *value).collect::<Vec<_>>();
    let mut seconds = Vec::new();
    let mut verified = true;
    for _ in 0..options.repeats {
        let started = Instant::now();
        verified &= match arm {
            "split only" => run_split(&values, options)?,
            "signed" => run_signed(&maker, &values, options)?,
            "signed+committed" => run_committed(&values, options, key)?.1,
            _ => return Err(format!("unknown arm {arm}").into()),
        };
        seconds.push(nonzero(started.elapsed().as_secs_f64()));
    }
    let mut node_seconds = Vec::new();
    let mut node_accepted = true;
    for _ in 0..(options.repeats / 4).max(1) {
        let (seconds, accepted) = node_side(&maker, &values, arm, options, key)?;
        node_seconds.push(nonzero(seconds));
        node_accepted &= accepted;
    }
    let rates = seconds
        .iter()
        .map(|seconds| 1.0 / seconds)
        .collect::<Vec<_>>();
    let node_rates = node_seconds
        .iter()
        .filter(|seconds| **seconds > 0.0)
        .map(|seconds| 1.0 / seconds)
        .collect::<Vec<_>>();
    Ok(json!({
        "arm": arm,
        "fields": fields.len(),
        "seconds": timing_summary(&seconds),
        "updates_per_second": timing_summary(&rates),
        "node_seconds": timing_summary(&node_seconds),
        "node_updates_per_second_per_core": if node_rates.is_empty() { Value::Null } else { timing_summary(&node_rates) },
        "node_accepted": node_accepted,
        "bytes_on_the_wire": {"exact": wire_bytes(&maker.name, options.n_nodes, fields.len(), arm != "split only", arm == "signed+committed")},
        "verified": verified,
    }))
}

fn run_split(values: &[i128], options: &Options) -> HarnessResult<bool> {
    let mut nodes = (0..options.n_nodes)
        .map(ComputingNode::new)
        .collect::<Vec<_>>();
    for value in values {
        for (node, share) in
            nodes
                .iter_mut()
                .zip(split(*value, options.n_nodes, options.value_bits)?)
        {
            node.receive(share, None);
        }
    }
    Ok(reconstructs(&nodes, values))
}

fn run_signed(maker: &InputParty, values: &[i128], options: &Options) -> HarnessResult<bool> {
    let mut nodes = (0..options.n_nodes)
        .map(ComputingNode::new)
        .collect::<Vec<_>>();
    maker.deal(values, &mut nodes)?;
    Ok(reconstructs(&nodes, values))
}

fn run_committed(
    values: &[i128],
    options: &Options,
    key: &Pedersen,
) -> HarnessResult<(Vec<CommittedDealing>, bool)> {
    let mut dealings = Vec::new();
    let mut reconstructed = true;
    for value in values {
        let dealing = committed_dealing(*value, options, key)?;
        reconstructed &= dealing.shares.iter().sum::<i128>() == *value;
        dealings.push(dealing);
    }
    Ok((dealings, reconstructed))
}

fn committed_dealing(
    value: i128,
    options: &Options,
    key: &Pedersen,
) -> HarnessResult<CommittedDealing> {
    let shares = split(value, options.n_nodes, options.value_bits)?;
    let mut rng = OsRng;
    let blindings = shares
        .iter()
        .map(|_| Scalar::random(&mut rng))
        .collect::<Vec<_>>();
    let share_commitments = shares
        .iter()
        .zip(&blindings)
        .map(|(share, blinding)| key.commit(&scalar(*share), blinding))
        .collect::<Vec<_>>();
    let aggregate_blinding: Scalar = blindings.iter().sum();
    Ok(CommittedDealing {
        value_commitment: key.commit(&scalar(value), &aggregate_blinding),
        share_commitments,
        shares,
        blindings,
    })
}

fn node_side(
    maker: &InputParty,
    values: &[i128],
    arm: &str,
    options: &Options,
    key: &Pedersen,
) -> HarnessResult<(f64, bool)> {
    match arm {
        "split only" => {
            // Prepare the same shares the node would receive; acceptance is
            // intentionally unauthenticated in this arm.
            for value in values {
                let _ = split(*value, options.n_nodes, options.value_bits)?;
            }
            let started = Instant::now();
            Ok((started.elapsed().as_secs_f64(), true))
        }
        "signed" => {
            let key = maker.signing_key.as_ref().expect("maker is signed");
            let verifying = key.verifying_key();
            let mut prepared = Vec::new();
            for (position, value) in values.iter().enumerate() {
                let shares = split(*value, options.n_nodes, options.value_bits)?;
                let body = dealt_body(&maker.name, 0, position, shares[0]);
                prepared.push((body.clone(), key.try_sign(&body)?));
            }
            let started = Instant::now();
            let accepted = prepared
                .iter()
                .all(|(body, signature)| verifying.verify(body, signature).is_ok());
            Ok((started.elapsed().as_secs_f64(), accepted))
        }
        "signed+committed" => {
            let (prepared, _) = run_committed(values, options, key)?;
            let started = Instant::now();
            let accepted = prepared.iter().all(|dealing| {
                let total = dealing
                    .share_commitments
                    .iter()
                    .fold(RistrettoPoint::default(), |sum, commitment| {
                        sum + commitment
                    });
                let adds_up = total == dealing.value_commitment;
                let opens = key.commit(&scalar(dealing.shares[0]), &dealing.blindings[0])
                    == dealing.share_commitments[0];
                adds_up && opens
            });
            Ok((started.elapsed().as_secs_f64(), accepted))
        }
        _ => Err(format!("unknown arm {arm}").into()),
    }
}

fn reconstructs(nodes: &[ComputingNode], values: &[i128]) -> bool {
    nodes.iter().all(|node| node.inputs.len() == values.len())
        && values.iter().enumerate().all(|(position, value)| {
            nodes.iter().map(|node| node.inputs[position]).sum::<i128>() == *value
        })
}

fn wire_bytes(name: &str, nodes: usize, fields: usize, signed: bool, committed: bool) -> usize {
    let body = dealt_body(name, 0, 0, 12_345).len();
    let signature = if signed { 64 } else { 0 };
    let mut total = (body + signature) * nodes * fields;
    if committed {
        total += fields * ((nodes + 1) * 32 + nodes * 32);
    }
    total
}

fn scalar(value: i128) -> Scalar {
    if value >= 0 {
        Scalar::from(value as u64)
    } else {
        -Scalar::from(value.unsigned_abs() as u64)
    }
}

fn nonzero(value: f64) -> f64 {
    value.max(f64::EPSILON)
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: qomm_harness::repo_root().join("artifacts/maker_updates.json"),
        n_nodes: 7,
        value_bits: 32,
        field_bits: 128,
        repeats: 200,
        group: "ed25519".into(),
        makers: 16,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--n-nodes" => {
                options.n_nodes = parse_value(value(&raw, &mut index, "--n-nodes")?, "--n-nodes")?
            }
            "--value-bits" => {
                options.value_bits =
                    parse_value(value(&raw, &mut index, "--value-bits")?, "--value-bits")?
            }
            "--field-bits" => {
                options.field_bits =
                    parse_value(value(&raw, &mut index, "--field-bits")?, "--field-bits")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--group" => {
                options.group = value(&raw, &mut index, "--group")?
                    .into_string()
                    .map_err(|_| "invalid --group")?
            }
            "--makers" => {
                options.makers = parse_value(value(&raw, &mut index, "--makers")?, "--makers")?
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

fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}
