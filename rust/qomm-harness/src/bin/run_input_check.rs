//! Rust port of `scripts/run_input_check.py`.

use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use qomm_harness::{parse_value, timing_summary, write_pretty_json, HarnessResult};
use qomm_zk::pedersen::Pedersen;
use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha512};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

const CHALLENGE_BITS: usize = 40;
const STATISTICAL_BITS: usize = 40;
const NARROW_CHALLENGE_BITS: usize = 6;
const NARROW_STATISTICAL_BITS: usize = 35;
const NARROW_REPEATS: usize = 7;
const SHARE_SLACK_BITS: usize = 40;
const PEDERSEN_PUBLICLY_VERIFIABLE: bool = true;
const VOLE_PUBLICLY_VERIFIABLE: bool = false;
const BEACON: u64 = 0x9E37_79B9_7F4A_7C15;
const DOMAIN: &[u8] = b"QOMM:ZK:v1";
const VOLE_MODULUS: u128 = (1u128 << 127) - 1;

struct Options {
    out: PathBuf,
    group: String,
    schemes: Vec<String>,
    inputs: Vec<usize>,
    repeats: usize,
    parties: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VoleCommitment {
    value: u128,
    tag: u128,
}

struct VoleScheme {
    delta: u128,
}

impl VoleScheme {
    fn new(rng: &mut OsRng) -> Self {
        Self {
            delta: random_vole(rng),
        }
    }

    fn commit(&self, value: u128, key: u128) -> VoleCommitment {
        let value = value % VOLE_MODULUS;
        VoleCommitment {
            value,
            tag: add_mod(
                key % VOLE_MODULUS,
                mul_mod(self.delta, value, VOLE_MODULUS),
                VOLE_MODULUS,
            ),
        }
    }

    fn add(&self, left: VoleCommitment, right: VoleCommitment) -> VoleCommitment {
        VoleCommitment {
            value: add_mod(left.value, right.value, VOLE_MODULUS),
            tag: add_mod(left.tag, right.tag, VOLE_MODULUS),
        }
    }

    fn scale(&self, commitment: VoleCommitment, scalar: u64) -> VoleCommitment {
        VoleCommitment {
            value: mul_mod(commitment.value, scalar as u128, VOLE_MODULUS),
            tag: mul_mod(commitment.tag, scalar as u128, VOLE_MODULUS),
        }
    }

    fn encode(&self, commitment: VoleCommitment) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(&commitment.value.to_be_bytes());
        out[16..].copy_from_slice(&commitment.tag.to_be_bytes());
        out
    }

    fn opens(&self, commitment: VoleCommitment, value: u128, key: u128) -> bool {
        commitment == self.commit(value, key)
    }
}

#[derive(Clone)]
struct PedersenInputCheck {
    commitments: Vec<RistrettoPoint>,
    mask_commitments: Vec<RistrettoPoint>,
    openings: Vec<Scalar>,
    opening_blindings: Vec<Scalar>,
    challenge_bits: usize,
}

impl PedersenInputCheck {
    fn repeats(&self) -> usize {
        self.openings.len()
    }

    fn soundness_bits(&self) -> usize {
        self.challenge_bits * self.repeats()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_pedersen_check(
    key: &Pedersen,
    values: &[i64],
    blindings: &[Scalar],
    context: &[u8],
    challenge: u64,
    challenge_bits: usize,
    statistical_bits: usize,
    repeats: usize,
    value_bits: usize,
    masks: Option<&[Scalar]>,
    mask_blindings: Option<&[Scalar]>,
    rng: &mut OsRng,
) -> Result<PedersenInputCheck, String> {
    if values.is_empty() {
        return Err("an input check over no inputs checks nothing".into());
    }
    if values.len() != blindings.len() {
        return Err("every value needs its blinding".into());
    }
    if repeats == 0 {
        return Err("a check with no repetitions checks nothing".into());
    }
    if masks.is_some_and(|values| values.len() != repeats)
        || mask_blindings.is_some_and(|values| values.len() != repeats)
    {
        return Err("one mask and blinding per repetition".into());
    }
    let commitments = values
        .iter()
        .zip(blindings)
        .map(|(value, blinding)| key.commit(&signed_scalar(*value), blinding))
        .collect::<Vec<_>>();
    let generated_masks;
    let masks = match masks {
        Some(masks) => masks,
        None => {
            generated_masks = (0..repeats)
                .map(|_| {
                    random_scalar_bits(
                        mask_bits_with(values.len(), value_bits, challenge_bits, statistical_bits),
                        rng,
                    )
                })
                .collect::<Vec<_>>();
            &generated_masks
        }
    };
    let generated_blindings;
    let mask_blindings = match mask_blindings {
        Some(blindings) => blindings,
        None => {
            generated_blindings = (0..repeats)
                .map(|_| Scalar::random(&mut *rng))
                .collect::<Vec<_>>();
            &generated_blindings
        }
    };
    let mask_commitments = masks
        .iter()
        .zip(mask_blindings)
        .map(|(mask, blinding)| key.commit(mask, blinding))
        .collect::<Vec<_>>();
    let encoded = commitments
        .iter()
        .map(|point| point.compress().to_bytes().to_vec())
        .collect::<Vec<_>>();
    let mut openings = Vec::with_capacity(repeats);
    let mut opening_blindings = Vec::with_capacity(repeats);
    for round in 0..repeats {
        let coefficients = coefficients(
            &encoded,
            &mask_commitments[round].compress().to_bytes(),
            context,
            challenge,
            challenge_bits,
            round as u32,
        );
        openings.push(
            values
                .iter()
                .zip(&coefficients)
                .fold(masks[round], |sum, (value, coefficient)| {
                    sum + signed_scalar(*value) * Scalar::from(*coefficient)
                }),
        );
        opening_blindings.push(
            blindings
                .iter()
                .zip(&coefficients)
                .fold(mask_blindings[round], |sum, (blinding, coefficient)| {
                    sum + blinding * Scalar::from(*coefficient)
                }),
        );
    }
    Ok(PedersenInputCheck {
        commitments,
        mask_commitments,
        openings,
        opening_blindings,
        challenge_bits,
    })
}

fn verify_pedersen_check(
    key: &Pedersen,
    check: &PedersenInputCheck,
    context: &[u8],
    challenge: u64,
) -> (bool, String) {
    if check.commitments.is_empty() {
        return (false, "the check covers no inputs".into());
    }
    if check.mask_commitments.len() != check.repeats()
        || check.opening_blindings.len() != check.repeats()
    {
        return (false, "one mask and opening per repetition".into());
    }
    let encoded = check
        .commitments
        .iter()
        .map(|point| point.compress().to_bytes().to_vec())
        .collect::<Vec<_>>();
    for round in 0..check.repeats() {
        let coefficients = coefficients(
            &encoded,
            &check.mask_commitments[round].compress().to_bytes(),
            context,
            challenge,
            check.challenge_bits,
            round as u32,
        );
        let combined = check.commitments.iter().zip(&coefficients).fold(
            check.mask_commitments[round],
            |sum, (commitment, coefficient)| sum + commitment * Scalar::from(*coefficient),
        );
        if key.commit(&check.openings[round], &check.opening_blindings[round]) != combined {
            return (
                false,
                format!(
                    "combination {round} is not what the committed inputs combine to: an input the circuit used was not the one that was committed"
                ),
            );
        }
    }
    (true, "ok".into())
}

#[derive(Clone)]
struct VoleInputCheck {
    commitments: Vec<VoleCommitment>,
    mask_commitments: Vec<VoleCommitment>,
    openings: Vec<u128>,
    opening_blindings: Vec<u128>,
    challenge_bits: usize,
}

impl VoleInputCheck {
    fn repeats(&self) -> usize {
        self.openings.len()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_vole_check(
    scheme: &VoleScheme,
    values: &[i128],
    blindings: &[u128],
    context: &[u8],
    challenge: u64,
    challenge_bits: usize,
    statistical_bits: usize,
    repeats: usize,
    value_bits: usize,
    masks: Option<&[u128]>,
    mask_blindings: Option<&[u128]>,
    rng: &mut OsRng,
) -> Result<VoleInputCheck, String> {
    if values.is_empty() {
        return Err("an input check over no inputs checks nothing".into());
    }
    if values.len() != blindings.len() {
        return Err("every value needs its blinding".into());
    }
    if repeats == 0 {
        return Err("a check with no repetitions checks nothing".into());
    }
    if masks.is_some_and(|values| values.len() != repeats)
        || mask_blindings.is_some_and(|values| values.len() != repeats)
    {
        return Err("one mask and blinding per repetition".into());
    }
    let commitments = values
        .iter()
        .zip(blindings)
        .map(|(value, blinding)| scheme.commit(normalize_vole(*value), *blinding))
        .collect::<Vec<_>>();
    let generated_masks;
    let masks = match masks {
        Some(masks) => masks,
        None => {
            generated_masks = (0..repeats)
                .map(|_| {
                    random_u128_bits(
                        mask_bits_with(values.len(), value_bits, challenge_bits, statistical_bits),
                        rng,
                    ) % VOLE_MODULUS
                })
                .collect::<Vec<_>>();
            &generated_masks
        }
    };
    let generated_blindings;
    let mask_blindings = match mask_blindings {
        Some(blindings) => blindings,
        None => {
            generated_blindings = (0..repeats).map(|_| random_vole(rng)).collect::<Vec<_>>();
            &generated_blindings
        }
    };
    let mask_commitments = masks
        .iter()
        .zip(mask_blindings)
        .map(|(mask, blinding)| scheme.commit(*mask, *blinding))
        .collect::<Vec<_>>();
    let encoded = commitments
        .iter()
        .map(|commitment| scheme.encode(*commitment).to_vec())
        .collect::<Vec<_>>();
    let mut openings = Vec::with_capacity(repeats);
    let mut opening_blindings = Vec::with_capacity(repeats);
    for round in 0..repeats {
        let coefficients = coefficients(
            &encoded,
            &scheme.encode(mask_commitments[round]),
            context,
            challenge,
            challenge_bits,
            round as u32,
        );
        openings.push(values.iter().zip(&coefficients).fold(
            masks[round],
            |sum, (value, coefficient)| {
                add_mod(
                    sum,
                    mul_mod(normalize_vole(*value), *coefficient as u128, VOLE_MODULUS),
                    VOLE_MODULUS,
                )
            },
        ));
        opening_blindings.push(blindings.iter().zip(&coefficients).fold(
            mask_blindings[round],
            |sum, (blinding, coefficient)| {
                add_mod(
                    sum,
                    mul_mod(*blinding, *coefficient as u128, VOLE_MODULUS),
                    VOLE_MODULUS,
                )
            },
        ));
    }
    Ok(VoleInputCheck {
        commitments,
        mask_commitments,
        openings,
        opening_blindings,
        challenge_bits,
    })
}

fn verify_vole_check(
    scheme: &VoleScheme,
    check: &VoleInputCheck,
    context: &[u8],
    challenge: u64,
) -> (bool, String) {
    if check.commitments.is_empty() {
        return (false, "the check covers no inputs".into());
    }
    if check.mask_commitments.len() != check.repeats()
        || check.opening_blindings.len() != check.repeats()
    {
        return (false, "one mask and opening per repetition".into());
    }
    let encoded = check
        .commitments
        .iter()
        .map(|commitment| scheme.encode(*commitment).to_vec())
        .collect::<Vec<_>>();
    for round in 0..check.repeats() {
        let coefficients = coefficients(
            &encoded,
            &scheme.encode(check.mask_commitments[round]),
            context,
            challenge,
            check.challenge_bits,
            round as u32,
        );
        let combined = check.commitments.iter().zip(&coefficients).fold(
            check.mask_commitments[round],
            |sum, (commitment, coefficient)| {
                scheme.add(sum, scheme.scale(*commitment, *coefficient))
            },
        );
        if scheme.commit(check.openings[round], check.opening_blindings[round]) != combined {
            return (
                false,
                format!(
                    "combination {round} is not what the committed inputs combine to: an input the circuit used was not the one that was committed"
                ),
            );
        }
    }
    (true, "ok".into())
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
        return Err("the Rust port supports the repository's ed25519 measurement group".into());
    }
    if options.repeats == 0 || options.inputs.is_empty() || options.parties < 4 {
        return Err("--repeats/--inputs must be non-zero and --parties must be at least 4".into());
    }
    let mut result = json!({
        "host": qomm_measure::hosts::this_host(),
        "group": options.group,
        "repeats": options.repeats,
        "challenge_bits": CHALLENGE_BITS,
        "width_budget": width_budget(),
        "schemes": {},
    });
    let mut rng = OsRng;
    for name in &options.schemes {
        let block = match name.as_str() {
            "pedersen" => measure_pedersen(&options, &mut rng)?,
            "vole" => measure_vole(&options, &mut rng)?,
            _ => {
                return Err(format!(
                    "unknown commitment scheme {name}; choose from ['pedersen', 'vole']"
                )
                .into())
            }
        };
        result["schemes"][name] = block;
    }

    if let Some(pedersen) = result["schemes"].get("pedersen") {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let repeats = 3usize.max(options.repeats / 4);
        let mut rows = Vec::new();
        for &inputs in &options.inputs {
            let mut row = measure_per_party(&key, inputs, options.parties, repeats, &mut rng)?;
            if let Some(base) = pedersen["rows"]
                .as_array()
                .and_then(|rows| rows.iter().find(|base| base["n_inputs"] == inputs))
            {
                let build = row["build_ms"]["median"].as_f64().unwrap_or(0.0)
                    / base["build_ms"]["median"].as_f64().unwrap_or(1.0);
                let verify = row["verify_ms"]["median"].as_f64().unwrap_or(0.0)
                    / base["verify_ms"]["median"].as_f64().unwrap_or(1.0);
                row["over_aggregate"] = json!({
                    "build": py_round_places(build, 2),
                    "verify": py_round_places(verify, 2),
                });
            }
            rows.push(row);
        }
        result["per_party"] = json!({
            "what": "one opening per party instead of one over all inputs, so a failing check names the node. The commitments it combines are the ones roles.Dealing already publishes, so this is the marginal cost.",
            "n_parties": {"exact": options.parties},
            "field_bits_needed": {"exact": per_party_field_bits(166, 31)},
            "aggregate_field_bits_needed": {"exact": field_bits_needed(166, 31, 7)},
            "rows": rows,
        });
    }

    if result["schemes"].get("pedersen").is_some() && result["schemes"].get("vole").is_some() {
        let ped = &result["schemes"]["pedersen"];
        let vol = &result["schemes"]["vole"];
        let scale = ped["scale_us"]["median"].as_f64().unwrap_or(0.0)
            / vol["scale_us"]["median"].as_f64().unwrap_or(1.0);
        let verify = if options.inputs.len() > 2 {
            Some(py_round_places(
                ped["rows"][2]["verify_ms"]["median"]
                    .as_f64()
                    .unwrap_or(0.0)
                    / vol["rows"][2]["verify_ms"]["median"]
                        .as_f64()
                        .unwrap_or(1.0),
                1,
            ))
        } else {
            None
        };
        result["vole_speedup"] = json!({
            "scale": py_round_places(scale, 1),
            "verify_at_166": verify,
        });
    }

    let accepted = result["schemes"]
        .as_object()
        .into_iter()
        .flat_map(|schemes| schemes.values())
        .flat_map(|block| block["rows"].as_array().into_iter().flatten())
        .all(|row| row["accepted"] == true);
    write_pretty_json(Some(&options.out), &result)?;
    println!("wrote {}", options.out.display());
    if !accepted {
        return Err("an input check failed".into());
    }
    Ok(())
}

fn width_budget() -> Value {
    let mut out = json!({
        "n_inputs": 166,
        "value_bits": 31,
        "challenge_bits": CHALLENGE_BITS,
        "opening_bits": opening_bits(166, 31),
        "mask_bits": mask_bits(166, 31),
        "field_bits_needed": field_bits_needed(166, 31, 7),
        "group_order_bits": 252,
    });
    for (label, prime_bits) in [
        ("mp_spdz_default_128", 127usize),
        ("wide_enough", 192),
        ("group_order", 252),
    ] {
        out[label] = match width_check(166, 31, prime_bits, 252, 7) {
            Ok(_) => json!("fits"),
            Err(error) => json!(format!("refused: {error}")),
        };
    }
    out["finding"] = json!("The check does not run in the field it was proposed to save. It needs about 164 bits against 253 for the group order, and at 253 the same widening also makes threshold_sigma assemble correctly --- so the question is whether 164 is worth it over 253, not whether the check avoids widening at all.");
    out
}

fn measure_pedersen(options: &Options, rng: &mut OsRng) -> HarnessResult<Value> {
    let key = Pedersen::new(b"qomm:pedersen:v1");
    let calibration_commitment = key.commit_u64(12_345, &Scalar::random(&mut *rng));
    let scalar = Scalar::random(&mut *rng);
    let mut scale_us = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        let _ = calibration_commitment * scalar;
        scale_us.push(started.elapsed().as_secs_f64() * 1e6);
    }
    println!("== pedersen ==");
    let mut rows = Vec::new();
    for &count in &options.inputs {
        rows.push(measure_pedersen_row(&key, count, options.repeats, rng)?);
    }
    Ok(json!({
        "publicly_verifiable": PEDERSEN_PUBLICLY_VERIFIABLE,
        "scale_us": timing_summary(&scale_us),
        "rows": rows,
    }))
}

fn measure_pedersen_row(
    key: &Pedersen,
    count: usize,
    repeats: usize,
    rng: &mut OsRng,
) -> HarnessResult<Value> {
    if count == 0 {
        return Err("an input check over no inputs checks nothing".into());
    }
    let values = (0..count)
        .map(|index| {
            let value = (37 * index + 5) as i64;
            if index % 2 == 0 {
                value
            } else {
                -value
            }
        })
        .collect::<Vec<_>>();
    let blindings = (0..count)
        .map(|_| Scalar::random(&mut *rng))
        .collect::<Vec<_>>();
    let commitments = values
        .iter()
        .zip(&blindings)
        .map(|(value, blinding)| key.commit(&signed_scalar(*value), blinding))
        .collect::<Vec<_>>();
    let mut build_ms = Vec::new();
    let mut verify_ms = Vec::new();
    let mut accepted = true;
    for _ in 0..repeats {
        let started = Instant::now();
        let mask = random_scalar_bits(mask_bits(count, 32), rng);
        let mask_blinding = Scalar::random(&mut *rng);
        let mask_commitment = key.commit(&mask, &mask_blinding);
        let coefficients = coefficients(
            &commitments
                .iter()
                .map(|point| point.compress().to_bytes().to_vec())
                .collect::<Vec<_>>(),
            &mask_commitment.compress().to_bytes(),
            b"qomm:input-check:bench",
            BEACON,
            CHALLENGE_BITS,
            0,
        );
        let opening = values
            .iter()
            .zip(&coefficients)
            .fold(mask, |sum, (value, coefficient)| {
                sum + signed_scalar(*value) * Scalar::from(*coefficient)
            });
        let opening_blinding = blindings
            .iter()
            .zip(&coefficients)
            .fold(mask_blinding, |sum, (blinding, coefficient)| {
                sum + blinding * Scalar::from(*coefficient)
            });
        build_ms.push(started.elapsed().as_secs_f64() * 1e3);
        let check = PedersenInputCheck {
            commitments: commitments.clone(),
            mask_commitments: vec![mask_commitment],
            openings: vec![opening],
            opening_blindings: vec![opening_blinding],
            challenge_bits: CHALLENGE_BITS,
        };
        let started = Instant::now();
        accepted &= verify_pedersen_check(key, &check, b"qomm:input-check:bench", BEACON).0;
        verify_ms.push(started.elapsed().as_secs_f64() * 1e3);
    }
    Ok(measurement_row(count, build_ms, verify_ms, accepted))
}

fn measure_vole(options: &Options, rng: &mut OsRng) -> HarnessResult<Value> {
    let scheme = VoleScheme::new(rng);
    let commitment = scheme.commit(12_345, random_vole(rng));
    let scalar = random_vole(rng) as u64;
    let mut scale_us = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        let _ = scheme.scale(commitment, scalar);
        scale_us.push(started.elapsed().as_secs_f64() * 1e6);
    }
    println!("== vole ==");
    let mut rows = Vec::new();
    for &count in &options.inputs {
        rows.push(measure_vole_row(&scheme, count, options.repeats, rng)?);
    }
    Ok(json!({
        "publicly_verifiable": VOLE_PUBLICLY_VERIFIABLE,
        "scale_us": timing_summary(&scale_us),
        "rows": rows,
    }))
}

fn measure_vole_row(
    scheme: &VoleScheme,
    count: usize,
    repeats: usize,
    rng: &mut OsRng,
) -> HarnessResult<Value> {
    if count == 0 {
        return Err("an input check over no inputs checks nothing".into());
    }
    let values = (0..count)
        .map(|index| {
            let value = (37 * index + 5) as i128;
            if index % 2 == 0 {
                value
            } else {
                -value
            }
        })
        .collect::<Vec<_>>();
    let blindings = (0..count).map(|_| random_vole(rng)).collect::<Vec<_>>();
    let commitments = values
        .iter()
        .zip(&blindings)
        .map(|(value, blinding)| scheme.commit(normalize_vole(*value), *blinding))
        .collect::<Vec<_>>();
    let mut build_ms = Vec::new();
    let mut verify_ms = Vec::new();
    let mut accepted = true;
    for _ in 0..repeats {
        let started = Instant::now();
        let mask = random_u128_bits(mask_bits(count, 32), rng) % VOLE_MODULUS;
        let mask_blinding = random_vole(rng);
        let mask_commitment = scheme.commit(mask, mask_blinding);
        let coefficients = coefficients(
            &commitments
                .iter()
                .map(|value| scheme.encode(*value).to_vec())
                .collect::<Vec<_>>(),
            &scheme.encode(mask_commitment),
            b"qomm:input-check:bench",
            BEACON,
            CHALLENGE_BITS,
            0,
        );
        let opening = values
            .iter()
            .zip(&coefficients)
            .fold(mask, |sum, (value, coefficient)| {
                add_mod(
                    sum,
                    mul_mod(normalize_vole(*value), *coefficient as u128, VOLE_MODULUS),
                    VOLE_MODULUS,
                )
            });
        let opening_blinding = blindings.iter().zip(&coefficients).fold(
            mask_blinding,
            |sum, (blinding, coefficient)| {
                add_mod(
                    sum,
                    mul_mod(*blinding, *coefficient as u128, VOLE_MODULUS),
                    VOLE_MODULUS,
                )
            },
        );
        build_ms.push(started.elapsed().as_secs_f64() * 1e3);
        let check = VoleInputCheck {
            commitments: commitments.clone(),
            mask_commitments: vec![mask_commitment],
            openings: vec![opening],
            opening_blindings: vec![opening_blinding],
            challenge_bits: CHALLENGE_BITS,
        };
        let started = Instant::now();
        accepted &= verify_vole_check(scheme, &check, b"qomm:input-check:bench", BEACON).0;
        verify_ms.push(started.elapsed().as_secs_f64() * 1e3);
    }
    Ok(measurement_row(count, build_ms, verify_ms, accepted))
}

fn measurement_row(count: usize, build: Vec<f64>, verify: Vec<f64>, accepted: bool) -> Value {
    json!({
        "n_inputs": count,
        "build_ms": timing_summary(&build),
        "verify_ms": timing_summary(&verify),
        "accepted": accepted,
        "published_bytes_incremental": {"exact": 96},
        "published_bytes_standalone": {"exact": (count + 1) * 32 + 64},
        "opening_bits": {"exact": opening_bits(count, 31)},
        "soundness_bits": {"exact": CHALLENGE_BITS},
    })
}

fn measure_per_party(
    key: &Pedersen,
    n_inputs: usize,
    n_parties: usize,
    repeats: usize,
    rng: &mut OsRng,
) -> HarnessResult<Value> {
    let shares = (0..n_parties)
        .map(|_| {
            (0..n_inputs)
                .map(|_| random_scalar_bits(71, rng))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let blindings = (0..n_parties)
        .map(|_| {
            (0..n_inputs)
                .map(|_| Scalar::random(&mut *rng))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut build_ms = Vec::new();
    let mut verify_ms = Vec::new();
    let mut last = None;
    for _ in 0..repeats {
        let started = Instant::now();
        let check = build_per_party(key, &shares, &blindings, rng);
        build_ms.push(started.elapsed().as_secs_f64() * 1e3);
        let started = Instant::now();
        let culprits = verify_per_party(key, &check);
        verify_ms.push(started.elapsed().as_secs_f64() * 1e3);
        if !culprits.is_empty() {
            return Err("an honest per-party check failed".into());
        }
        last = Some(check);
    }
    let mut check = last.ok_or("no per-party check built")?;
    let coefficients = per_party_coefficients(&check.commitments, &check.masks);
    check.openings[3] += coefficients.iter().fold(Scalar::ZERO, |sum, coefficient| {
        sum + Scalar::from(*coefficient)
    });
    let named = verify_per_party(key, &check);
    Ok(json!({
        "n_inputs": n_inputs,
        "n_parties": n_parties,
        "build_ms": timing_summary(&build_ms),
        "verify_ms": timing_summary(&verify_ms),
        "named_the_substituting_node": named == vec![3],
        "field_bits_needed": per_party_field_bits(n_inputs, 31),
    }))
}

const PER_PARTY_BENCH_CONTEXT: &[u8] = b"qomm:per-party-check:bench";

#[derive(Clone)]
struct PerParty {
    commitments: Vec<Vec<RistrettoPoint>>,
    masks: Vec<RistrettoPoint>,
    openings: Vec<Scalar>,
    opening_blindings: Vec<Scalar>,
    challenge_bits: usize,
}

impl PerParty {
    fn n_parties(&self) -> usize {
        self.openings.len()
    }

    fn n_values(&self) -> usize {
        self.commitments.first().map_or(0, Vec::len)
    }

    fn soundness_bits(&self) -> usize {
        self.challenge_bits
    }
}

fn build_per_party(
    key: &Pedersen,
    shares: &[Vec<Scalar>],
    blindings: &[Vec<Scalar>],
    rng: &mut OsRng,
) -> PerParty {
    build_per_party_with(key, shares, blindings, PER_PARTY_BENCH_CONTEXT, BEACON, rng)
        .expect("measurement generated a rectangular, non-empty dealing")
}

fn build_per_party_with(
    key: &Pedersen,
    shares: &[Vec<Scalar>],
    blindings: &[Vec<Scalar>],
    context: &[u8],
    challenge: u64,
    rng: &mut OsRng,
) -> Result<PerParty, String> {
    let Some(first) = shares.first() else {
        return Err("a per-party check needs at least one party".into());
    };
    if first.is_empty() || shares.iter().any(|row| row.len() != first.len()) {
        return Err("every party needs one share of every value".into());
    }
    if blindings.len() != shares.len() || blindings.iter().any(|row| row.len() != first.len()) {
        return Err("every share needs its blinding".into());
    }
    let commitments = shares
        .iter()
        .zip(blindings)
        .map(|(row, row_blindings)| {
            row.iter()
                .zip(row_blindings)
                .map(|(value, blinding)| key.commit(value, blinding))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let width =
        71 + CHALLENGE_BITS + bit_length(shares[0].len().saturating_sub(1)) + STATISTICAL_BITS;
    let mask_values = (0..shares.len())
        .map(|_| random_scalar_bits(width, rng))
        .collect::<Vec<_>>();
    let mask_blindings = (0..shares.len())
        .map(|_| Scalar::random(&mut *rng))
        .collect::<Vec<_>>();
    let masks = mask_values
        .iter()
        .zip(&mask_blindings)
        .map(|(value, blinding)| key.commit(value, blinding))
        .collect::<Vec<_>>();
    let coefficients = per_party_coefficients_with(&commitments, &masks, context, Some(challenge))?;
    let openings = shares
        .iter()
        .zip(&mask_values)
        .map(|(row, mask)| {
            row.iter()
                .zip(&coefficients)
                .fold(*mask, |sum, (value, coefficient)| {
                    sum + value * Scalar::from(*coefficient)
                })
        })
        .collect::<Vec<_>>();
    let opening_blindings = blindings
        .iter()
        .zip(&mask_blindings)
        .map(|(row, mask)| {
            row.iter()
                .zip(&coefficients)
                .fold(*mask, |sum, (value, coefficient)| {
                    sum + value * Scalar::from(*coefficient)
                })
        })
        .collect::<Vec<_>>();
    Ok(PerParty {
        commitments,
        masks,
        openings,
        opening_blindings,
        challenge_bits: CHALLENGE_BITS,
    })
}

fn verify_per_party(key: &Pedersen, check: &PerParty) -> Vec<usize> {
    verify_per_party_with(key, check, PER_PARTY_BENCH_CONTEXT, BEACON).2
}

fn verify_per_party_with(
    key: &Pedersen,
    check: &PerParty,
    context: &[u8],
    challenge: u64,
) -> (bool, String, Vec<usize>) {
    if check.commitments.len() != check.n_parties()
        || check.masks.len() != check.n_parties()
        || check.opening_blindings.len() != check.n_parties()
        || check
            .commitments
            .iter()
            .any(|row| row.len() != check.n_values())
    {
        return (
            false,
            "the per-party check has a malformed shape".into(),
            vec![],
        );
    }
    let Ok(coefficients) =
        per_party_coefficients_with(&check.commitments, &check.masks, context, Some(challenge))
    else {
        return (
            false,
            "the per-party coefficients could not be derived".into(),
            vec![],
        );
    };
    let culprits = (0..check.openings.len())
        .filter(|party| {
            let combined = check.commitments[*party]
                .iter()
                .zip(&coefficients)
                .fold(check.masks[*party], |sum, (commitment, coefficient)| {
                    sum + commitment * Scalar::from(*coefficient)
                });
            key.commit(&check.openings[*party], &check.opening_blindings[*party]) != combined
        })
        .collect::<Vec<_>>();
    if culprits.is_empty() {
        (true, "ok".into(), culprits)
    } else {
        let names = culprits
            .iter()
            .map(|party| format!("node {party}"))
            .collect::<Vec<_>>()
            .join(", ");
        (false, format!("input check failed for {names}"), culprits)
    }
}

fn per_party_coefficients(
    commitments: &[Vec<RistrettoPoint>],
    masks: &[RistrettoPoint],
) -> Vec<u64> {
    per_party_coefficients_with(commitments, masks, PER_PARTY_BENCH_CONTEXT, Some(BEACON))
        .expect("measurement supplies its post-input challenge")
}

fn per_party_coefficients_with(
    commitments: &[Vec<RistrettoPoint>],
    masks: &[RistrettoPoint],
    context: &[u8],
    challenge: Option<u64>,
) -> Result<Vec<u64>, String> {
    let challenge = challenge.ok_or_else(|| {
        "the coefficients need a challenge drawn AFTER the inputs are fixed".to_string()
    })?;
    let Some(first) = commitments.first() else {
        return Err("a per-party check needs at least one party".into());
    };
    if first.is_empty()
        || masks.len() != commitments.len()
        || commitments.iter().any(|row| row.len() != first.len())
    {
        return Err("every party needs one published commitment per value and one mask".into());
    }
    let mut seed = Sha512::new();
    seed.update(DOMAIN);
    seed.update(b":per-party-check:v1");
    seed.update((context.len() as u32).to_be_bytes());
    seed.update(context);
    seed.update((commitments.len() as u32).to_be_bytes());
    for row in commitments {
        seed.update((row.len() as u32).to_be_bytes());
        for commitment in row {
            let encoded = commitment.compress().to_bytes();
            seed.update((encoded.len() as u32).to_be_bytes());
            seed.update(encoded);
        }
    }
    for commitment in masks {
        let encoded = commitment.compress().to_bytes();
        seed.update((encoded.len() as u32).to_be_bytes());
        seed.update(encoded);
    }
    let mut challenge_bytes = [0u8; 32];
    challenge_bytes[24..].copy_from_slice(&challenge.to_be_bytes());
    seed.update(challenge_bytes);
    Ok(derive_coefficients(
        seed.finalize().as_slice(),
        first.len(),
        CHALLENGE_BITS,
    ))
}

fn coefficients(
    commitments: &[Vec<u8>],
    mask: &[u8],
    context: &[u8],
    challenge: u64,
    challenge_bits: usize,
    round: u32,
) -> Vec<u64> {
    let mut seed = Sha512::new();
    seed.update(DOMAIN);
    seed.update(b":input-check:v1");
    seed.update((context.len() as u32).to_be_bytes());
    seed.update(context);
    seed.update((commitments.len() as u32).to_be_bytes());
    for commitment in commitments {
        seed.update((commitment.len() as u32).to_be_bytes());
        seed.update(commitment);
    }
    seed.update((mask.len() as u32).to_be_bytes());
    seed.update(mask);
    seed.update(round.to_be_bytes());
    let mut challenge_bytes = [0u8; 32];
    challenge_bytes[24..].copy_from_slice(&challenge.to_be_bytes());
    seed.update(challenge_bytes);
    derive_coefficients(
        seed.finalize().as_slice(),
        commitments.len(),
        challenge_bits,
    )
}

fn derive_coefficients(root: &[u8], count: usize, challenge_bits: usize) -> Vec<u64> {
    assert!((1..u64::BITS as usize).contains(&challenge_bits));
    let modulus = (1u64 << challenge_bits) - 1;
    (0..count)
        .map(|index| {
            let mut hash = Sha512::new();
            hash.update(root);
            hash.update((index as u32).to_be_bytes());
            let remainder = hash.finalize().iter().fold(0u64, |value, byte| {
                ((value as u128 * 256 + u128::from(*byte)) % modulus as u128) as u64
            });
            1 + remainder
        })
        .collect()
}

fn opening_bits(inputs: usize, value_bits: usize) -> usize {
    opening_bits_with(inputs, value_bits, CHALLENGE_BITS, STATISTICAL_BITS)
}

fn mask_bits(inputs: usize, value_bits: usize) -> usize {
    mask_bits_with(inputs, value_bits, CHALLENGE_BITS, STATISTICAL_BITS)
}

fn field_bits_needed(inputs: usize, value_bits: usize, nodes: usize) -> usize {
    field_bits_needed_with(
        inputs,
        value_bits,
        nodes,
        CHALLENGE_BITS,
        STATISTICAL_BITS,
        SHARE_SLACK_BITS,
    )
}

fn opening_bits_with(
    inputs: usize,
    value_bits: usize,
    challenge_bits: usize,
    statistical_bits: usize,
) -> usize {
    value_bits + challenge_bits + bit_length(inputs.saturating_sub(1)) + statistical_bits + 1
}

fn mask_bits_with(
    inputs: usize,
    value_bits: usize,
    challenge_bits: usize,
    statistical_bits: usize,
) -> usize {
    value_bits + challenge_bits + bit_length(inputs.saturating_sub(1)) + statistical_bits
}

fn field_bits_needed_with(
    inputs: usize,
    value_bits: usize,
    nodes: usize,
    challenge_bits: usize,
    statistical_bits: usize,
    share_slack: usize,
) -> usize {
    mask_bits_with(inputs, value_bits, challenge_bits, statistical_bits)
        + share_slack
        + bit_length(nodes.saturating_sub(1))
        + 2
}

fn per_party_field_bits(inputs: usize, value_bits: usize) -> usize {
    per_party_field_bits_with_nodes(inputs, value_bits, 7)
}

fn per_party_field_bits_with_nodes(inputs: usize, value_bits: usize, nodes: usize) -> usize {
    let _ = nodes;
    (value_bits + 40) + CHALLENGE_BITS + bit_length(inputs.saturating_sub(1)) + STATISTICAL_BITS + 1
}

fn width_check(
    inputs: usize,
    value_bits: usize,
    mpc_bits: usize,
    group_bits: usize,
    nodes: usize,
) -> Result<usize, String> {
    width_check_with(
        inputs,
        value_bits,
        mpc_bits,
        group_bits,
        nodes,
        CHALLENGE_BITS,
        STATISTICAL_BITS,
        SHARE_SLACK_BITS,
    )
}

#[allow(clippy::too_many_arguments)]
fn width_check_with(
    inputs: usize,
    value_bits: usize,
    mpc_bits: usize,
    group_bits: usize,
    nodes: usize,
    challenge_bits: usize,
    statistical_bits: usize,
    share_slack: usize,
) -> Result<usize, String> {
    let opening = opening_bits_with(inputs, value_bits, challenge_bits, statistical_bits);
    let mask = mask_bits_with(inputs, value_bits, challenge_bits, statistical_bits);
    let needed = field_bits_needed_with(
        inputs,
        value_bits,
        nodes,
        challenge_bits,
        statistical_bits,
        share_slack,
    );
    if needed >= mpc_bits.min(group_bits) {
        return Err(format!(
            "{inputs} inputs of {value_bits} bits with {challenge_bits}-bit coefficients open to {opening} bits, which fits --- but the {mask}-bit mask has to be dealt to {nodes} nodes with {share_slack} bits of slack per share, and that needs {needed} bits against the narrower of the MPC prime ({mpc_bits}) and the group order ({group_bits}). The hiding bits are spent twice, once on the combination and once on each share. Widen the field, or say in the artifact which of the two hidings was given up."
        ));
    }
    Ok(needed)
}

#[derive(Clone, Copy, Debug)]
struct NarrowTradeoff {
    challenge_bits: usize,
    statistical_bits: usize,
    repeats: usize,
    soundness_bits: f64,
    hiding_bits: f64,
}

fn narrow_tradeoff(inputs: usize, value_bits: usize) -> Vec<NarrowTradeoff> {
    let mut rows = Vec::new();
    for challenge_bits in 2usize..=40 {
        let gap = 126isize
            - value_bits as isize
            - bit_length(inputs.saturating_sub(1)) as isize
            - SHARE_SLACK_BITS as isize
            - bit_length(7usize.saturating_sub(1)) as isize
            - 2
            - challenge_bits as isize;
        if gap < 1 {
            break;
        }
        let per_round = (((1u64 << challenge_bits) - 1) as f64).log2();
        let repeats = (40.0 / per_round).ceil() as usize;
        rows.push(NarrowTradeoff {
            challenge_bits,
            statistical_bits: gap as usize,
            repeats,
            soundness_bits: py_round_places(repeats as f64 * per_round, 1),
            hiding_bits: py_round_places(gap as f64 - (repeats as f64).log2(), 1),
        });
    }
    rows
}

fn bit_length(value: usize) -> usize {
    if value == 0 {
        0
    } else {
        (usize::BITS - value.leading_zeros()) as usize
    }
}

fn signed_scalar(value: i64) -> Scalar {
    if value >= 0 {
        Scalar::from(value as u64)
    } else {
        -Scalar::from(value.unsigned_abs())
    }
}

fn random_scalar_bits(bits: usize, rng: &mut OsRng) -> Scalar {
    let mut bytes = [0u8; 32];
    let used = bits.div_ceil(8).min(32);
    rng.fill_bytes(&mut bytes[..used]);
    if bits % 8 != 0 && used > 0 {
        bytes[used - 1] &= (1u8 << (bits % 8)) - 1;
    }
    Scalar::from_bytes_mod_order(bytes)
}

fn random_u128_bits(bits: usize, rng: &mut OsRng) -> u128 {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    let value = u128::from_le_bytes(bytes);
    if bits >= 128 {
        value
    } else {
        value & ((1u128 << bits) - 1)
    }
}

fn random_vole(rng: &mut OsRng) -> u128 {
    loop {
        let value = random_u128_bits(127, rng);
        if value > 0 && value < VOLE_MODULUS {
            return value;
        }
    }
}

fn normalize_vole(value: i128) -> u128 {
    if value >= 0 {
        value as u128 % VOLE_MODULUS
    } else {
        let magnitude = value.unsigned_abs() % VOLE_MODULUS;
        if magnitude == 0 {
            0
        } else {
            VOLE_MODULUS - magnitude
        }
    }
}

fn add_mod(left: u128, right: u128, modulus: u128) -> u128 {
    if left >= modulus - right {
        left - (modulus - right)
    } else {
        left + right
    }
}

fn mul_mod(mut left: u128, mut right: u128, modulus: u128) -> u128 {
    let mut result = 0u128;
    left %= modulus;
    while right > 0 {
        if right & 1 == 1 {
            result = add_mod(result, left, modulus);
        }
        right >>= 1;
        if right > 0 {
            left = add_mod(left, left, modulus);
        }
    }
    result
}

fn py_round_places(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    qomm_sim::market::py_round(value * scale) as f64 / scale
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        out: qomm_harness::repo_root().join("artifacts/input_check.json"),
        group: "ed25519".into(),
        schemes: vec!["pedersen".into(), "vole".into()],
        inputs: vec![16, 64, 166, 512],
        repeats: 20,
        parties: 7,
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
            "--group" => {
                options.group = value(&raw, &mut index, "--group")?
                    .into_string()
                    .map_err(|_| "invalid --group")?
            }
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--parties" => {
                options.parties = parse_value(value(&raw, &mut index, "--parties")?, "--parties")?
            }
            "--schemes" => {
                options.schemes.clear();
                index += 1;
                while index < raw.len() && !raw[index].to_string_lossy().starts_with("--") {
                    options
                        .schemes
                        .push(raw[index].to_string_lossy().into_owned());
                    index += 1;
                }
                continue;
            }
            "--inputs" => {
                options.inputs.clear();
                index += 1;
                while index < raw.len() && !raw[index].to_string_lossy().starts_with("--") {
                    options
                        .inputs
                        .push(parse_value(raw[index].clone(), "--inputs")?);
                    index += 1;
                }
                continue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const TEST_CONTEXT: &[u8] = b"qomm:test:slot:7";

    fn policy(count: usize) -> (Vec<i64>, Vec<Scalar>) {
        let values = (0..count)
            .map(|index| {
                let value = (37 * index + 5) as i64;
                if index % 2 == 0 {
                    value
                } else {
                    -value
                }
            })
            .collect::<Vec<_>>();
        let blindings = (0..count)
            .map(|_| Scalar::random(&mut OsRng))
            .collect::<Vec<_>>();
        (values, blindings)
    }

    fn pedersen_check(
        key: &Pedersen,
        values: &[i64],
        blindings: &[Scalar],
        challenge_bits: usize,
        statistical_bits: usize,
        repeats: usize,
    ) -> PedersenInputCheck {
        build_pedersen_check(
            key,
            values,
            blindings,
            TEST_CONTEXT,
            BEACON,
            challenge_bits,
            statistical_bits,
            repeats,
            32,
            None,
            None,
            &mut OsRng,
        )
        .unwrap()
    }

    fn pedersen_coefficients(check: &PedersenInputCheck, context: &[u8], round: usize) -> Vec<u64> {
        coefficients(
            &check
                .commitments
                .iter()
                .map(|point| point.compress().to_bytes().to_vec())
                .collect::<Vec<_>>(),
            &check.mask_commitments[round].compress().to_bytes(),
            context,
            BEACON,
            check.challenge_bits,
            round as u32,
        )
    }

    fn substitute_pedersen(
        honest: &PedersenInputCheck,
        errors: &[(usize, i64)],
    ) -> PedersenInputCheck {
        let mut forged = honest.clone();
        for round in 0..forged.repeats() {
            let coefficients = pedersen_coefficients(honest, TEST_CONTEXT, round);
            for &(position, error) in errors {
                forged.openings[round] +=
                    signed_scalar(error) * Scalar::from(coefficients[position]);
            }
        }
        forged
    }

    fn scalar_below(value: &Scalar, bits: usize) -> bool {
        let bytes = value.to_bytes();
        let full_bytes = bits / 8;
        let remaining = bits % 8;
        if remaining == 0 {
            bytes[full_bytes..].iter().all(|byte| *byte == 0)
        } else {
            bytes[full_bytes] < (1u8 << remaining)
                && bytes[full_bytes + 1..].iter().all(|byte| *byte == 0)
        }
    }

    #[test]
    fn honest_inputs_verify() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let check = pedersen_check(
            &key,
            &values,
            &blindings,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
        );
        assert_eq!(
            verify_pedersen_check(&key, &check, TEST_CONTEXT, BEACON),
            (true, "ok".into())
        );
    }

    #[test]
    fn one_substituted_input_is_caught() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let honest = pedersen_check(
            &key,
            &values,
            &blindings,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
        );
        assert!(verify_pedersen_check(&key, &honest, TEST_CONTEXT, BEACON).0);
        for position in [0usize, 7, 23] {
            for error in [1i64, -1, 1_000, -4_096] {
                let forged = substitute_pedersen(&honest, &[(position, error)]);
                let (ok, why) = verify_pedersen_check(&key, &forged, TEST_CONTEXT, BEACON);
                assert!(!ok, "position={position}, error={error}");
                assert!(why.contains("was not the one that was committed"), "{why}");
            }
        }
    }

    #[test]
    fn errors_across_two_inputs_do_not_cancel() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let honest = pedersen_check(
            &key,
            &values,
            &blindings,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
        );
        assert!(verify_pedersen_check(&key, &honest, TEST_CONTEXT, BEACON).0);
        let forged = substitute_pedersen(&honest, &[(3, 500), (11, -500)]);
        let (ok, why) = verify_pedersen_check(&key, &forged, TEST_CONTEXT, BEACON);
        assert!(!ok);
        assert!(why.contains("combination 0"), "wrong refusal check: {why}");
    }

    #[test]
    fn the_coefficients_come_from_the_commitments() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let masks = [Scalar::from(123_456u64)];
        let mask_blindings = [Scalar::from(654_321u64)];
        let first = build_pedersen_check(
            &key,
            &values,
            &blindings,
            TEST_CONTEXT,
            BEACON,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
            32,
            Some(&masks),
            Some(&mask_blindings),
            &mut OsRng,
        )
        .unwrap();
        let mut moved = values.clone();
        moved[5] += 1;
        let second = build_pedersen_check(
            &key,
            &moved,
            &blindings,
            TEST_CONTEXT,
            BEACON,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
            32,
            Some(&masks),
            Some(&mask_blindings),
            &mut OsRng,
        )
        .unwrap();
        assert_ne!(
            pedersen_coefficients(&first, TEST_CONTEXT, 0),
            pedersen_coefficients(&second, TEST_CONTEXT, 0)
        );
    }

    #[test]
    fn the_context_separates_slots() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let check = pedersen_check(
            &key,
            &values,
            &blindings,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
        );
        assert!(verify_pedersen_check(&key, &check, TEST_CONTEXT, BEACON).0);
        let (ok, why) = verify_pedersen_check(&key, &check, b"qomm:test:slot:8", BEACON);
        assert!(!ok);
        assert!(why.contains("combination 0"), "wrong refusal check: {why}");
    }

    #[test]
    fn no_coefficient_is_zero() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(64);
        let check = pedersen_check(
            &key,
            &values,
            &blindings,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
        );
        let coefficients = pedersen_coefficients(&check, TEST_CONTEXT, 0);
        assert_eq!(coefficients.len(), 64);
        assert!(coefficients.iter().all(|value| *value > 0));
        assert!(coefficients
            .iter()
            .all(|value| *value < (1u64 << CHALLENGE_BITS)));
    }

    #[test]
    fn the_mask_moves_the_opening() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(24);
        let openings = (0..8)
            .map(|_| {
                pedersen_check(
                    &key,
                    &values,
                    &blindings,
                    CHALLENGE_BITS,
                    STATISTICAL_BITS,
                    1,
                )
                .openings[0]
                    .to_bytes()
            })
            .collect::<HashSet<_>>();
        assert_eq!(openings.len(), 8);
    }

    #[test]
    fn the_shipped_field_does_not_hold_this_check() {
        let error = width_check(166, 31, 127, 252, 7).unwrap_err();
        assert!(error.contains("spent twice"));
    }

    #[test]
    fn no_coefficient_width_rescues_the_127_bit_field() {
        for challenge_bits in [3usize, 8, 16, 32, 40] {
            assert!(width_check_with(
                166,
                31,
                127,
                252,
                7,
                challenge_bits,
                STATISTICAL_BITS,
                SHARE_SLACK_BITS,
            )
            .is_err());
        }
    }

    #[test]
    fn a_wide_enough_field_is_accepted() {
        assert_eq!(field_bits_needed(166, 31, 7), 164);
        assert_eq!(width_check(166, 31, 192, 252, 7), Ok(164));
        assert_eq!(width_check(166, 31, 252, 252, 7), Ok(164));
    }

    #[test]
    fn the_number_of_inputs_is_almost_free() {
        let small = field_bits_needed(16, 31, 7);
        let shipped = field_bits_needed(166, 31, 7);
        let huge = field_bits_needed(1usize << 30, 31, 7);
        assert_eq!((small, shipped, huge), (160, 164, 186));
        assert_eq!(huge - small, 26);
        assert!(width_check(1usize << 30, 31, 252, 252, 7).is_ok());
    }

    #[test]
    fn the_opening_stays_inside_the_narrower_field() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(166);
        let mut bytes = [0u8; 32];
        bytes[12] = 1 << 4; // 2^100, safely above the signed combination.
        let masks = [Scalar::from_bytes_mod_order(bytes)];
        let mask_blindings = [Scalar::from(7u64)];
        let check = build_pedersen_check(
            &key,
            &values,
            &blindings,
            TEST_CONTEXT,
            BEACON,
            CHALLENGE_BITS,
            STATISTICAL_BITS,
            1,
            31,
            Some(&masks),
            Some(&mask_blindings),
            &mut OsRng,
        )
        .unwrap();
        assert!(scalar_below(&check.openings[0], opening_bits(166, 31)));
        assert!(scalar_below(&check.openings[0], 127));
    }

    #[test]
    fn the_narrow_configuration_fits_the_default_field() {
        assert_eq!(
            width_check_with(
                166,
                32,
                127,
                252,
                7,
                NARROW_CHALLENGE_BITS,
                NARROW_STATISTICAL_BITS,
                SHARE_SLACK_BITS,
            ),
            Ok(126)
        );
    }

    #[test]
    fn repetition_buys_the_soundness_back() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(166);
        let check = pedersen_check(
            &key,
            &values,
            &blindings,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
        );
        assert_eq!(check.repeats(), 7);
        assert_eq!(check.soundness_bits(), 42);
        assert_eq!(
            verify_pedersen_check(&key, &check, TEST_CONTEXT, BEACON),
            (true, "ok".into())
        );
    }

    #[test]
    fn the_narrow_check_still_catches_a_substitution() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(166);
        let honest = pedersen_check(
            &key,
            &values,
            &blindings,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
        );
        assert!(verify_pedersen_check(&key, &honest, TEST_CONTEXT, BEACON).0);
        for position in [0usize, 83, 165] {
            let forged = substitute_pedersen(&honest, &[(position, 17)]);
            let (ok, why) = verify_pedersen_check(&key, &forged, TEST_CONTEXT, BEACON);
            assert!(!ok, "position={position}");
            assert!(why.contains("combination"), "wrong refusal check: {why}");
        }
    }

    #[test]
    fn every_repetition_uses_different_coefficients() {
        let key = Pedersen::new(b"qomm:input-check:test");
        let (values, blindings) = policy(40);
        let check = pedersen_check(
            &key,
            &values,
            &blindings,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            4,
        );
        let rounds = (0..4)
            .map(|round| pedersen_coefficients(&check, TEST_CONTEXT, round))
            .collect::<HashSet<_>>();
        assert_eq!(rounds.len(), 4);
    }

    #[test]
    fn forty_bit_hiding_is_not_reachable_in_the_narrow_field() {
        let curve = narrow_tradeoff(166, 32);
        assert!(!curve.is_empty());
        let peak = curve
            .iter()
            .max_by(|left, right| left.hiding_bits.total_cmp(&right.hiding_bits))
            .unwrap();
        assert!(33.0 < peak.hiding_bits && peak.hiding_bits < 35.0);
        assert!(peak.hiding_bits < 40.0);
        assert!(peak.challenge_bits <= 4 && peak.repeats >= 11);
        assert!(peak.statistical_bits > 0 && peak.soundness_bits >= 40.0);
    }

    #[test]
    fn the_check_runs_on_either_scheme() {
        let (values, blindings) = policy(64);
        let key = Pedersen::new(b"qomm:input-check:test");
        let pedersen = pedersen_check(
            &key,
            &values,
            &blindings,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
        );
        assert_eq!(
            verify_pedersen_check(&key, &pedersen, TEST_CONTEXT, BEACON),
            (true, "ok".into())
        );

        let scheme = VoleScheme::new(&mut OsRng);
        let vole_values = values
            .iter()
            .map(|value| *value as i128)
            .collect::<Vec<_>>();
        let vole_blindings = (0..values.len())
            .map(|_| random_vole(&mut OsRng))
            .collect::<Vec<_>>();
        let vole = build_vole_check(
            &scheme,
            &vole_values,
            &vole_blindings,
            TEST_CONTEXT,
            BEACON,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
            32,
            None,
            None,
            &mut OsRng,
        )
        .unwrap();
        assert_eq!(
            verify_vole_check(&scheme, &vole, TEST_CONTEXT, BEACON),
            (true, "ok".into())
        );
    }

    #[test]
    fn a_substitution_is_caught_on_either_scheme() {
        let (values, blindings) = policy(64);
        let key = Pedersen::new(b"qomm:input-check:test");
        let honest = pedersen_check(
            &key,
            &values,
            &blindings,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
        );
        assert!(verify_pedersen_check(&key, &honest, TEST_CONTEXT, BEACON).0);
        let forged = substitute_pedersen(&honest, &[(11, 23)]);
        let (ok, why) = verify_pedersen_check(&key, &forged, TEST_CONTEXT, BEACON);
        assert!(!ok);
        assert!(why.contains("combination"), "wrong refusal check: {why}");

        let scheme = VoleScheme::new(&mut OsRng);
        let vole_values = values
            .iter()
            .map(|value| *value as i128)
            .collect::<Vec<_>>();
        let vole_blindings = (0..values.len())
            .map(|_| random_vole(&mut OsRng))
            .collect::<Vec<_>>();
        let honest = build_vole_check(
            &scheme,
            &vole_values,
            &vole_blindings,
            TEST_CONTEXT,
            BEACON,
            NARROW_CHALLENGE_BITS,
            NARROW_STATISTICAL_BITS,
            NARROW_REPEATS,
            32,
            None,
            None,
            &mut OsRng,
        )
        .unwrap();
        assert!(verify_vole_check(&scheme, &honest, TEST_CONTEXT, BEACON).0);
        let mut forged = honest.clone();
        for round in 0..forged.repeats() {
            let coefficients = coefficients(
                &honest
                    .commitments
                    .iter()
                    .map(|commitment| scheme.encode(*commitment).to_vec())
                    .collect::<Vec<_>>(),
                &scheme.encode(honest.mask_commitments[round]),
                TEST_CONTEXT,
                BEACON,
                honest.challenge_bits,
                round as u32,
            );
            forged.openings[round] = add_mod(
                forged.openings[round],
                mul_mod(coefficients[11] as u128, 23, VOLE_MODULUS),
                VOLE_MODULUS,
            );
        }
        let (ok, why) = verify_vole_check(&scheme, &forged, TEST_CONTEXT, BEACON);
        assert!(!ok);
        assert!(why.contains("combination"), "wrong refusal check: {why}");
    }

    #[test]
    fn the_two_schemes_do_not_promise_the_same_thing() {
        assert!(PEDERSEN_PUBLICLY_VERIFIABLE);
        assert!(!VOLE_PUBLICLY_VERIFIABLE);
    }

    #[test]
    fn the_vole_scheme_binds_and_hides() {
        let scheme = VoleScheme::new(&mut OsRng);
        let key = random_vole(&mut OsRng);
        let commitment = scheme.commit(12_345, key);
        assert!(scheme.opens(commitment, 12_345, key));
        assert!(!scheme.opens(commitment, 12_346, key));
        let second_key = random_vole(&mut OsRng);
        let second = scheme.commit(678, second_key);
        let combined = scheme.add(scheme.scale(commitment, 7), second);
        assert!(scheme.opens(
            combined,
            7 * 12_345 + 678,
            add_mod(mul_mod(7, key, VOLE_MODULUS), second_key, VOLE_MODULUS),
        ));
    }

    fn per_party_fixture(
        n_values: usize,
        n_parties: usize,
    ) -> (Vec<Vec<Scalar>>, Vec<Vec<Scalar>>) {
        let shares = (0..n_parties)
            .map(|_| {
                (0..n_values)
                    .map(|_| random_scalar_bits(71, &mut OsRng))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let blindings = (0..n_parties)
            .map(|_| {
                (0..n_values)
                    .map(|_| Scalar::random(&mut OsRng))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        (shares, blindings)
    }

    fn per_party_check(
        key: &Pedersen,
        shares: &[Vec<Scalar>],
        blindings: &[Vec<Scalar>],
    ) -> PerParty {
        build_per_party_with(key, shares, blindings, b"ctx", BEACON, &mut OsRng).unwrap()
    }

    fn substitute_per_party(key: &Pedersen, honest: &PerParty, who: &[usize]) -> PerParty {
        let coefficients =
            per_party_coefficients_with(&honest.commitments, &honest.masks, b"ctx", Some(BEACON))
                .unwrap();
        let drift = coefficients.iter().fold(Scalar::ZERO, |sum, coefficient| {
            sum + Scalar::from(*coefficient)
        });
        let mut forged = honest.clone();
        for &party in who {
            forged.openings[party] += drift;
        }
        let control = verify_per_party_with(key, honest, b"ctx", BEACON);
        assert_eq!(control, (true, "ok".into(), vec![]));
        forged
    }

    #[test]
    fn the_per_party_check_needs_a_narrower_field() {
        assert_eq!(per_party_field_bits(166, 31), 160);
        assert_eq!(field_bits_needed(166, 31, 7), 164);
        assert!(per_party_field_bits(166, 31) < field_bits_needed(166, 31, 7));
    }

    #[test]
    fn neither_fits_the_default_prime_and_both_fit_the_group_order() {
        assert!(per_party_field_bits(166, 31) > 127);
        assert!(per_party_field_bits(166, 31) < 252);
    }

    #[test]
    fn width_grows_with_the_input_count_and_nothing_else() {
        for inputs in [1usize, 16, 166, 1_024] {
            assert_eq!(
                per_party_field_bits_with_nodes(inputs, 31, 3),
                per_party_field_bits_with_nodes(inputs, 31, 99)
            );
        }
    }

    #[test]
    fn an_honest_dealing_names_nobody() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(12, 7);
        let check = per_party_check(&key, &shares, &blindings);
        assert_eq!(
            verify_per_party_with(&key, &check, b"ctx", BEACON),
            (true, "ok".into(), vec![])
        );
    }

    #[test]
    fn a_single_substituting_node_is_named() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(12, 7);
        let honest = per_party_check(&key, &shares, &blindings);
        for party in [0usize, 3, 6] {
            let forged = substitute_per_party(&key, &honest, &[party]);
            let (ok, why, culprits) = verify_per_party_with(&key, &forged, b"ctx", BEACON);
            assert!(!ok);
            assert_eq!(culprits, vec![party]);
            assert!(why.contains(&format!("node {party}")), "{why}");
        }
    }

    #[test]
    fn every_node_position_is_named_correctly() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(6, 7);
        let honest = per_party_check(&key, &shares, &blindings);
        for party in 0..7 {
            let forged = substitute_per_party(&key, &honest, &[party]);
            assert_eq!(
                verify_per_party_with(&key, &forged, b"ctx", BEACON).2,
                vec![party]
            );
        }
    }

    #[test]
    fn the_innocent_are_not_named() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(12, 7);
        let honest = per_party_check(&key, &shares, &blindings);
        let forged = substitute_per_party(&key, &honest, &[2]);
        let (_, _, culprits) = verify_per_party_with(&key, &forged, b"ctx", BEACON);
        assert_eq!(culprits, vec![2]);
    }

    #[test]
    fn there_is_no_threshold_on_how_many_can_be_named() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(8, 7);
        let honest = per_party_check(&key, &shares, &blindings);
        for count in [1usize, 2, 3, 4, 6, 7] {
            let parties = (0..count).collect::<Vec<_>>();
            let forged = substitute_per_party(&key, &honest, &parties);
            let (ok, _, culprits) = verify_per_party_with(&key, &forged, b"ctx", BEACON);
            assert!(!ok);
            assert_eq!(culprits, parties);
        }
    }

    #[test]
    fn all_seven_lying_is_still_resolvable() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(5, 7);
        let honest = per_party_check(&key, &shares, &blindings);
        let forged = substitute_per_party(&key, &honest, &(0..7).collect::<Vec<_>>());
        assert_eq!(
            verify_per_party_with(&key, &forged, b"ctx", BEACON).2,
            (0..7).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_aggregate_check_is_implied() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(9, 7);
        let check = per_party_check(&key, &shares, &blindings);
        let coefficients =
            per_party_coefficients_with(&check.commitments, &check.masks, b"ctx", Some(BEACON))
                .unwrap();
        let values = (0..9)
            .map(|position| {
                shares
                    .iter()
                    .fold(Scalar::ZERO, |sum, row| sum + row[position])
            })
            .collect::<Vec<_>>();
        let left = check
            .openings
            .iter()
            .fold(Scalar::ZERO, |sum, value| sum + value)
            - coefficients
                .iter()
                .zip(&values)
                .fold(Scalar::ZERO, |sum, (coefficient, value)| {
                    sum + Scalar::from(*coefficient) * value
                });
        let right =
            shares
                .iter()
                .zip(&check.openings)
                .fold(Scalar::ZERO, |total, (row, opening)| {
                    total + opening
                        - coefficients
                            .iter()
                            .zip(row)
                            .fold(Scalar::ZERO, |sum, (coefficient, share)| {
                                sum + Scalar::from(*coefficient) * share
                            })
                });
        assert_eq!(left, right);
    }

    #[test]
    fn a_different_context_does_not_verify() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(12, 7);
        let check = per_party_check(&key, &shares, &blindings);
        assert_eq!(
            verify_per_party_with(&key, &check, b"ctx", BEACON),
            (true, "ok".into(), vec![])
        );
        let (ok, why, culprits) = verify_per_party_with(&key, &check, b"another auction", BEACON);
        assert!(!ok);
        assert!(!culprits.is_empty());
        assert!(why.contains("node"), "wrong refusal check: {why}");
    }

    #[test]
    fn coefficients_depend_on_every_published_commitment() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(4, 7);
        let first = per_party_check(&key, &shares, &blindings);
        let mut second = first.clone();
        second.commitments[5][2] = key.commit(&(shares[5][2] + Scalar::ONE), &blindings[5][2]);
        assert_ne!(
            per_party_coefficients_with(&first.commitments, &first.masks, b"ctx", Some(BEACON),)
                .unwrap(),
            per_party_coefficients_with(&second.commitments, &second.masks, b"ctx", Some(BEACON),)
                .unwrap()
        );
    }

    #[test]
    fn a_node_that_sees_the_coefficients_cannot_cancel_its_error() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(12, 7);
        let check = per_party_check(&key, &shares, &blindings);
        let seen =
            per_party_coefficients_with(&check.commitments, &check.masks, b"ctx", Some(BEACON))
                .unwrap();
        let error_one = Scalar::from(seen[2]) * Scalar::from(1_000u64);
        let error_two = -(Scalar::from(seen[1]) * Scalar::from(1_000u64));
        assert_eq!(
            Scalar::from(seen[1]) * error_one + Scalar::from(seen[2]) * error_two,
            Scalar::ZERO,
            "the pre-challenge cancellation was not constructed"
        );
        let later = per_party_coefficients_with(
            &check.commitments,
            &check.masks,
            b"ctx",
            Some(BEACON ^ 0xa5a5_a5a5),
        )
        .unwrap();
        let drift = Scalar::from(later[1]) * error_one + Scalar::from(later[2]) * error_two;
        assert_ne!(drift, Scalar::ZERO);
    }

    #[test]
    fn the_coefficients_move_with_the_challenge() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(6, 7);
        let check = per_party_check(&key, &shares, &blindings);
        let a = per_party_coefficients_with(&check.commitments, &check.masks, b"ctx", Some(BEACON))
            .unwrap();
        let b =
            per_party_coefficients_with(&check.commitments, &check.masks, b"ctx", Some(BEACON + 1))
                .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn deriving_coefficients_without_a_challenge_is_refused() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(4, 7);
        let check = per_party_check(&key, &shares, &blindings);
        let error = per_party_coefficients_with(&check.commitments, &check.masks, b"ctx", None)
            .unwrap_err();
        assert!(error.contains("AFTER the inputs"));
    }

    #[test]
    fn a_ragged_dealing_is_refused() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (mut shares, blindings) = per_party_fixture(4, 7);
        shares[3].truncate(2);
        let error = build_per_party_with(&key, &shares, &blindings, b"ctx", BEACON, &mut OsRng)
            .err()
            .unwrap();
        assert!(error.contains("one share of every value"));
    }

    #[test]
    fn missing_blindings_are_refused() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, mut blindings) = per_party_fixture(4, 7);
        blindings.pop();
        let error = build_per_party_with(&key, &shares, &blindings, b"ctx", BEACON, &mut OsRng)
            .err()
            .unwrap();
        assert!(error.contains("blinding"));
    }

    #[test]
    fn no_parties_is_refused() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let error = build_per_party_with(&key, &[], &[], b"ctx", BEACON, &mut OsRng)
            .err()
            .unwrap();
        assert!(error.contains("at least one party"));
    }

    #[test]
    fn the_shape_is_reported() {
        let key = Pedersen::new(b"qomm:pedersen:v1");
        let (shares, blindings) = per_party_fixture(11, 7);
        let check = per_party_check(&key, &shares, &blindings);
        assert_eq!(check.n_parties(), 7);
        assert_eq!(check.n_values(), 11);
        assert_eq!(check.soundness_bits(), CHALLENGE_BITS);
    }
}
