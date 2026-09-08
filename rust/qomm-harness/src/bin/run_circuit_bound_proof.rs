//!
//! The witness shares are read from MP-SPDZ persistence files. Run the circuit
//! with both `--persist-wires` and `--shamir-inputs`; otherwise there are no
//! circuit-owned wires in the ristretto255 scalar field to prove statements
//! about.

use curve25519_dalek::scalar::Scalar;
use merlin::Transcript;
use qomm_harness::{parse_value, repo_root, write_pretty_json, HarnessResult};
use qomm_mpc::persistence::{read_wires, FieldElement, Wires, WIRE_NAMES};
use rand::rngs::OsRng;
use serde_json::{json, Map};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use zkfmi_zk::pedersen::Pedersen;
use zkfmi_zk::shamir;
use zkfmi_zk::sigma::verify_product;
use zkpi_proofs::threshold_gadgets::{
    coefficient_commitments_from_evaluations, commitment_from_shares,
    joint_prove_product_from_contributions, verify_square_bit, ProductNodeContribution, Shared,
};
use zkpi_proofs::threshold_quote::RISTRETTO_SCALAR_ORDER_LE;
use zkpi_proofs::threshold_sigma::{PartyId, ScalarShares};

struct Options {
    persistence: PathBuf,
    out: PathBuf,
    parties: usize,
    threshold: usize,
    makers: usize,
    qty: i64,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    if options.parties <= options.threshold {
        return Err("--parties must be greater than --threshold".into());
    }
    if options.makers == 0 {
        return Err("--makers must be positive".into());
    }
    if options.qty < 0 {
        return Err("--qty must be non-negative".into());
    }

    let wires = read_wires(&options.persistence, options.parties, options.makers, -1)?;
    let prime_le: [u8; 32] = wires
        .prime
        .to_bytes_le(32)?
        .try_into()
        .expect("a requested 32-byte encoding");
    if prime_le != RISTRETTO_SCALAR_ORDER_LE {
        return Err(format!(
            "the circuit wrote in a {}-bit field and the commitments use a 253-bit one; run it with --persist-wires --shamir-inputs",
            wires.prime.bit_length()
        )
        .into());
    }

    let key = Pedersen::new(b"qomm:policy:v1");
    let parties = (1..=options.parties).collect::<Vec<_>>();
    let quorum = parties[..=options.threshold].to_vec();
    let mut rng = OsRng;
    let scalar_wires = scalar_wires(&wires)?;
    let winner_key = reconstruct(&scalar_wires.winner_key, &quorum)?;
    let mut rows = Vec::new();

    for (index, maker) in scalar_wires.makers.iter().enumerate() {
        let mut shared = BTreeMap::new();
        let mut blindings = BTreeMap::new();
        for name in WIRE_NAMES {
            let (wire, blinding) = wire_from_shares(
                &key,
                maker
                    .get(name)
                    .ok_or_else(|| format!("maker {index}: missing wire {name}"))?,
                &parties,
                &quorum,
                options.threshold,
                &mut rng,
            )?;
            shared.insert(name, wire);
            blindings.insert(name, blinding);
        }

        let commitments_from_shares_agree = WIRE_NAMES.iter().all(|name| {
            let value = reconstruct(&maker[name], &quorum).expect("validated share map");
            let blinding = reconstruct(&blindings[name], &quorum).expect("validated blinding map");
            shared[name].commitment.compress() == key.commit(&value, &blinding).compress()
        });

        let slope_blinding = reconstruct(&blindings["slope"], &quorum)?;
        let depth_blinding = reconstruct(&blindings["depth"], &quorum)?;
        let qty_value = scalar_from_i64(options.qty);
        let qty_shares = share_scalar(&qty_value, &parties, options.threshold, &mut rng);
        let qty_blinding_secret = Scalar::random(&mut rng);
        let qty_blindings =
            share_scalar(&qty_blinding_secret, &parties, options.threshold, &mut rng);
        let qty_shared =
            shared_from_maps(&key, qty_shares, qty_blindings, &quorum, options.threshold)?;
        let depth_cross_secret = depth_blinding - slope_blinding * qty_value;
        let depth_cross = share_scalar(&depth_cross_secret, &parties, options.threshold, &mut rng);
        let depth_contributions = node_product_contributions(&qty_shared, &depth_cross, &quorum)?;
        let mut transcript = tagged(b"circuit:depth");
        let (depth_proof, _) = joint_prove_product_from_contributions(
            &key,
            &shared["slope"].commitment,
            &shared["depth"].commitment,
            &depth_contributions,
            &quorum,
            options.threshold,
            &mut transcript,
            &mut rng,
        )?;
        let mut transcript = tagged(b"circuit:depth");
        let depth_is_slope_times_qty = verify_product(
            &key,
            &mut transcript,
            &shared["slope"].commitment,
            &qty_shared.commitment,
            &shared["depth"].commitment,
            &depth_proof,
        );

        let mut bits_proved = Map::new();
        for name in ["fits", "ok", "active"] {
            let blinding = reconstruct(&blindings[name], &quorum)?;
            let bit = reconstruct(&maker[name], &quorum)?;
            let cross_secret = blinding * (Scalar::ONE - bit);
            let cross = share_scalar(&cross_secret, &parties, options.threshold, &mut rng);
            let contributions = node_product_contributions(&shared[name], &cross, &quorum)?;
            let context = format!("circuit:{name}");
            let mut transcript = tagged(context.as_bytes());
            let (proof, _) = joint_prove_product_from_contributions(
                &key,
                &shared[name].commitment,
                &shared[name].commitment,
                &contributions,
                &quorum,
                options.threshold,
                &mut transcript,
                &mut rng,
            )?;
            let mut transcript = tagged(context.as_bytes());
            bits_proved.insert(
                name.to_string(),
                json!(verify_square_bit(
                    &key,
                    &shared[name].commitment,
                    &proof,
                    &mut transcript,
                )),
            );
        }

        let slope_value = reconstruct(&maker["slope"], &quorum)?;
        let wrong_qty_value = scalar_from_i64(options.qty + 1);
        let wrong_value_shares =
            share_scalar(&wrong_qty_value, &parties, options.threshold, &mut rng);
        let wrong_blinding_secret = Scalar::random(&mut rng);
        let wrong_blinding_shares = share_scalar(
            &wrong_blinding_secret,
            &parties,
            options.threshold,
            &mut rng,
        );
        let wrong = shared_from_maps(
            &key,
            wrong_value_shares,
            wrong_blinding_shares,
            &quorum,
            options.threshold,
        )?;
        let wrong_cross_secret = depth_blinding - slope_blinding * wrong_qty_value;
        let wrong_cross = share_scalar(&wrong_cross_secret, &parties, options.threshold, &mut rng);
        let wrong_contributions = node_product_contributions(&wrong, &wrong_cross, &quorum)?;
        let mut transcript = tagged(b"circuit:depth");
        let refused = match joint_prove_product_from_contributions(
            &key,
            &shared["slope"].commitment,
            &shared["depth"].commitment,
            &wrong_contributions,
            &quorum,
            options.threshold,
            &mut transcript,
            &mut rng,
        ) {
            Err(_) => true,
            Ok((proof, _)) => {
                let mut transcript = tagged(b"circuit:depth");
                !verify_product(
                    &key,
                    &mut transcript,
                    &shared["slope"].commitment,
                    &wrong.commitment,
                    &shared["depth"].commitment,
                    &proof,
                )
            }
        };

        let other = (index + 1) % options.makers;
        let other_blinding = Scalar::random(&mut rng);
        let other_blindings = share_scalar(&other_blinding, &parties, options.threshold, &mut rng);
        let other_depth = commitment_from_shares(
            &key,
            &scalar_wires.makers[other]["depth"],
            &other_blindings,
            &quorum,
        )?;
        let mut transcript = tagged(b"circuit:depth");
        let another_makers_depth_is_refused = !verify_product(
            &key,
            &mut transcript,
            &shared["slope"].commitment,
            &qty_shared.commitment,
            &other_depth,
            &depth_proof,
        );

        let no_node_holds_a_wire = WIRE_NAMES.iter().all(|name| {
            let opened = reconstruct(&maker[name], &quorum).expect("validated share map");
            opened == Scalar::ZERO
                || opened == Scalar::ONE
                || maker[name].values().all(|share| *share != opened)
        });
        let control_applies = slope_value != Scalar::ZERO;
        let slope = signed_i64(slope_value)?;
        println!(
            "maker {index}: commitments {commitments_from_shares_agree}, depth {depth_is_slope_times_qty}, bits {bits_proved:?}, wrong qty refused {refused} (control applies: {control_applies}), other depth refused {another_makers_depth_is_refused}"
        );
        rows.push(json!({
            "maker": index,
            "slope": slope,
            "wrong_quantity_control_applies": control_applies,
            "a_wrong_quantity_is_refused": refused,
            "another_makers_depth_is_refused": another_makers_depth_is_refused,
            "commitments_from_shares_agree": commitments_from_shares_agree,
            "depth_is_slope_times_qty": depth_is_slope_times_qty,
            "bits_proved": bits_proved,
            "no_node_holds_a_wire": no_node_holds_a_wire,
        }));
    }

    let payload = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "field_bits": wires.prime.bit_length(),
        "field_matches_commitment_scalar_field": true,
        "parties": options.parties,
        "threshold": options.threshold,
        "wires_per_maker": WIRE_NAMES.len(),
        "wire_order": WIRE_NAMES,
        "runs_in_file": wires.runs_in_file,
        "winner_key": scalar_u64(winner_key)?,
        "supplied_outside_the_circuit": [
            "blindings, which the computation does not know about",
            "cross terms, which are one multiplication in a deployment",
        ],
        "rows": rows,
    });
    write_pretty_json(Some(&options.out), &payload)?;
    println!("wrote {}", options.out.display());
    Ok(())
}

struct ScalarWires {
    winner_key: ScalarShares,
    makers: Vec<BTreeMap<&'static str, ScalarShares>>,
}

fn scalar_wires(wires: &Wires) -> HarnessResult<ScalarWires> {
    Ok(ScalarWires {
        winner_key: scalar_map(&wires.winner_key)?,
        makers: wires
            .makers
            .iter()
            .map(|maker| {
                maker
                    .iter()
                    .map(|(name, shares)| Ok((*name, scalar_map(shares)?)))
                    .collect::<HarnessResult<BTreeMap<_, _>>>()
            })
            .collect::<HarnessResult<Vec<_>>>()?,
    })
}

fn scalar_map(values: &BTreeMap<usize, FieldElement>) -> HarnessResult<ScalarShares> {
    values
        .iter()
        .map(|(party, value)| Ok((*party, field_scalar(value)?)))
        .collect()
}

fn field_scalar(value: &FieldElement) -> HarnessResult<Scalar> {
    let bytes: [u8; 32] = value
        .to_bytes_le(32)?
        .try_into()
        .expect("a requested 32-byte encoding");
    Option::<Scalar>::from(Scalar::from_canonical_bytes(bytes))
        .ok_or_else(|| "a persisted share is outside the scalar field".into())
}

fn share_scalar<R: rand::RngCore + rand::CryptoRng>(
    secret: &Scalar,
    parties: &[PartyId],
    threshold: usize,
    rng: &mut R,
) -> ScalarShares {
    let points = shamir::points(parties.len());
    parties
        .iter()
        .copied()
        .zip(shamir::share(secret, threshold, &points, rng))
        .collect()
}

fn node_product_contributions(
    factor: &Shared,
    cross: &ScalarShares,
    quorum: &[PartyId],
) -> HarnessResult<Vec<ProductNodeContribution>> {
    quorum
        .iter()
        .map(|party| {
            let factor = factor
                .node_share(*party)
                .ok_or_else(|| format!("missing factor share for party {party}"))?;
            let cross_share = cross
                .get(party)
                .copied()
                .ok_or_else(|| format!("missing cross-term share for party {party}"))?;
            Ok(ProductNodeContribution::new(factor, cross_share))
        })
        .collect()
}

fn shared_from_maps(
    key: &Pedersen,
    value: ScalarShares,
    blinding: ScalarShares,
    quorum: &[PartyId],
    threshold: usize,
) -> HarnessResult<Shared> {
    let evaluations = value
        .iter()
        .map(|(party, share)| (*party, key.commit(share, &blinding[party])))
        .collect::<BTreeMap<_, _>>();
    Ok(Shared {
        commitment: commitment_from_shares(key, &value, &blinding, quorum)?,
        coefficient_commitments: coefficient_commitments_from_evaluations(&evaluations, threshold)?,
        value,
        blinding,
    })
}

fn wire_from_shares<R: rand::RngCore + rand::CryptoRng>(
    key: &Pedersen,
    value: &ScalarShares,
    parties: &[PartyId],
    quorum: &[PartyId],
    threshold: usize,
    rng: &mut R,
) -> HarnessResult<(Shared, ScalarShares)> {
    let blinding = share_scalar(&Scalar::random(rng), parties, threshold, rng);
    Ok((
        shared_from_maps(key, value.clone(), blinding.clone(), quorum, threshold)?,
        blinding,
    ))
}

fn reconstruct(shares: &ScalarShares, quorum: &[PartyId]) -> HarnessResult<Scalar> {
    let values = quorum
        .iter()
        .map(|party| {
            shares
                .get(party)
                .copied()
                .ok_or_else(|| format!("missing share for party {party}").into())
        })
        .collect::<HarnessResult<Vec<_>>>()?;
    let points = quorum
        .iter()
        .map(|party| Scalar::from(*party as u64))
        .collect::<Vec<_>>();
    Ok(shamir::reconstruct(&points, &values))
}

fn scalar_from_i64(value: i64) -> Scalar {
    if value < 0 {
        -Scalar::from(value.unsigned_abs())
    } else {
        Scalar::from(value as u64)
    }
}

fn scalar_u64(value: Scalar) -> HarnessResult<u64> {
    let bytes = value.to_bytes();
    if bytes[8..].iter().any(|byte| *byte != 0) {
        return Err("a circuit result does not fit u64".into());
    }
    Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
}

fn signed_i64(value: Scalar) -> HarnessResult<i64> {
    if let Ok(positive) = scalar_u64(value) {
        return i64::try_from(positive)
            .map_err(|_| "a positive circuit wire does not fit i64".into());
    }
    let magnitude = scalar_u64(-value)?;
    i64::try_from(magnitude)
        .map(|value| -value)
        .map_err(|_| "a negative circuit wire does not fit i64".into())
}

fn tagged(context: &[u8]) -> Transcript {
    let mut transcript = Transcript::new(b"qomm:circuit-bound:v1");
    transcript.append_message(b"context", context);
    transcript
}

fn parse_args() -> HarnessResult<Options> {
    let mut persistence = None;
    let mut options = Options {
        persistence: PathBuf::new(),
        out: repo_root().join("artifacts/circuit_bound_proof.json"),
        parties: 7,
        threshold: 2,
        makers: 4,
        qty: 100,
    };
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        match flag.to_str() {
            Some("--persistence") => {
                persistence = Some(PathBuf::from(next(&mut args, "--persistence")?))
            }
            Some("--out") => options.out = PathBuf::from(next(&mut args, "--out")?),
            Some("--parties") => {
                options.parties = parse_value(next(&mut args, "--parties")?, "--parties")?
            }
            Some("--threshold") => {
                options.threshold = parse_value(next(&mut args, "--threshold")?, "--threshold")?
            }
            Some("--makers") => {
                options.makers = parse_value(next(&mut args, "--makers")?, "--makers")?
            }
            Some("--qty") => options.qty = parse_value(next(&mut args, "--qty")?, "--qty")?,
            Some("-h" | "--help") => {
                println!("usage: run_circuit_bound_proof --persistence PATH [--out PATH] [--parties N] [--threshold N] [--makers N] [--qty N]");
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {}", flag.to_string_lossy()).into()),
        }
    }
    options.persistence = persistence.ok_or("--persistence is required")?;
    Ok(options)
}

fn next(args: &mut impl Iterator<Item = OsString>, flag: &str) -> HarnessResult<OsString> {
    args.next()
        .ok_or_else(|| format!("argument {flag} expects one value").into())
}
