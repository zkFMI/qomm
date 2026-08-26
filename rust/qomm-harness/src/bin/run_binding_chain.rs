//! Rust port of the Python run_binding_chain measurement.

use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use qomm_harness::local_mpc::LocalMpcRun;
use qomm_harness::{median, parse_value, unique_temp_dir, write_pretty_json, HarnessResult};
use qomm_mpc::inputs::{build_inputs, finish_reference, InputConfig};
use qomm_mpc::program::{
    build_program, ed25519_lagrange_at_zero, pow2_ceil, sentinel_for, CheckMode, Disclosure, Mode,
    ProgramConfig, Reference, ED25519_ORDER, FIELDS,
};
use qomm_proofs::policy_audit::PolicyBounds;
use qomm_transport::binding::{check_all, BindingDealer, BoundInputs};
use qomm_zk::bitrange::{prove_bounded, verify_bounded, BoundedProof};
use qomm_zk::pedersen::Pedersen;
use qomm_zk::shamir;
use rand_core::OsRng;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

struct Options {
    mp_spdz_root: Option<PathBuf>,
    n_mm: usize,
    n_parties: usize,
    threshold: usize,
    qty: i128,
    bit_length: u32,
    seed: i128,
    repeats: usize,
    out: PathBuf,
}

struct DealtMarket {
    dealer: BindingDealer,
    bound: BoundInputs,
    party_values: Vec<Vec<Scalar>>,
    reference: Value,
    deal_ms: f64,
}

fn main() {
    if let Err(error) = run_main() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_main() -> HarnessResult<()> {
    let options = parse_args()?;
    let ref_table = [100_000i128];
    let honest = deal_market(&options, &ref_table)?;
    let party_file_is_dealt = honest.bound.party_file(1)? == honest.party_values[0];
    let started = Instant::now();
    let failures = check_all(&honest.dealer.key, &honest.bound);
    let check_ms = started.elapsed().as_secs_f64() * 1e3;
    let n_checks = honest.bound.values.len() * options.n_parties;

    let spread_position = 5 + 1 + FIELDS.iter().position(|field| *field == "spread").unwrap();
    let half_value = scalar_to_i64(reconstruct(&honest.bound, spread_position)?)?;
    let blinding = reconstruct_blinding(&honest.bound, spread_position)?;
    let band = PolicyBounds::default().spread;
    let started = Instant::now();
    let (range_proof, _) = prove_dealt_range(
        &honest.dealer,
        &honest.bound,
        spread_position,
        half_value,
        &blinding,
        band.0,
        band.1,
        b"band",
        &mut OsRng,
    )?;
    let range_ms = started.elapsed().as_secs_f64() * 1e3;
    let range_verifies = check_dealt_range(
        &honest.dealer.key,
        &honest.bound,
        spread_position,
        &range_proof,
        band.0,
        band.1,
        b"band",
    );
    let rebuilt_request = (0..5)
        .map(|position| reconstruct(&honest.bound, position).and_then(scalar_to_i64))
        .collect::<Result<Vec<_>, _>>()?;
    let lagrange = ed25519_lagrange_at_zero(options.n_parties)?;

    let mut output = json!({
        "host": qomm_measure::hosts::this_host(),
        "n_mm": options.n_mm,
        "n_parties": options.n_parties,
        "threshold": options.threshold,
        "prime": ED25519_ORDER,
        "prime_bits": 253,
        "group": "ed25519",
        "honest": {
            "values": honest.bound.values.len(),
            "deal_ms": round_places(honest.deal_ms, 1),
            "share_checks": n_checks,
            "check_ms": round_places(check_ms, 1),
            "check_ms_each": round_places(check_ms / n_checks as f64, 3),
            "failures": failures.len(),
            "party_file_is_the_dealt_share": party_file_is_dealt,
            "rebuilt_request": rebuilt_request,
            "range_proof_ms": round_places(range_ms, 1),
            "band": [band.0, band.1],
            "half_in_band": half_value,
            "range_verifies": range_verifies,
            "lagrange": lagrange,
        },
    });

    let mut liar = deal_market(&options, &ref_table)?;
    let party_id = 4usize;
    *liar.bound.values[spread_position]
        .shares
        .value_shares
        .get_mut(&party_id)
        .ok_or("tampered party is missing")? += Scalar::ONE;
    let caught = check_all(&liar.dealer.key, &liar.bound)
        .into_iter()
        .map(|(party, position)| json!([party - 1, position]))
        .collect::<Vec<_>>();
    output["dealer_that_deals_what_it_did_not_commit"] = json!({
        "moved_position": spread_position,
        "moved_party": 3,
        "caught": caught,
        "caught_before_computing": !caught.is_empty(),
    });
    output["node_that_feeds_something_else"] = json!({
        "caught_by_this_chain": false,
        "why": "an input the node substitutes is still a valid share of a different number, and every commitment it opens still opens. That is rust/qomm-harness/src/bin/run_input_check.rs, which catches the node where this catches the dealer. Neither implies the other.",
    });

    let Some(root) = options
        .mp_spdz_root
        .as_deref()
        .filter(|root| root.join("malicious-shamir-party.x").exists())
    else {
        output["circuit"] =
            json!({"ran": false, "why": "no malicious-shamir-party.x; pass --mp-spdz-root"});
        write_pretty_json(Some(&options.out), &output)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    };

    let padded = pow2_ceil(options.n_mm)?;
    let config = ProgramConfig {
        n_mm: padded,
        n_parties: options.n_parties,
        mode: Mode::Rfq,
        rfs_steps: 1,
        disclose: Disclosure::None,
        now_t: 1,
        ref_mid: ref_table[0],
        band_bps: 0,
        threshold_k: 0,
        threshold_v: 0,
        public_check: true,
        n_requests: 1,
        n_assets: 1,
        ref_table: ref_table.to_vec(),
        maker_assets: vec![0; padded],
        bit_length: options.bit_length,
        lagrange: Some(ed25519_lagrange_at_zero(options.n_parties)?),
        ..ProgramConfig::default()
    };
    let work = unique_temp_dir("qomm-binding")?;
    let source = work.join("program.mpc");
    fs::write(&source, build_program(&config)?)?;
    let party_files = (1..=options.n_parties)
        .map(|party| {
            honest.bound.party_file(party).map(|values| {
                let mut text = values
                    .iter()
                    .map(scalar_decimal)
                    .collect::<Vec<_>>()
                    .join(" ");
                text.push('\n');
                text
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let program = format!("qomm_binding_{}", std::process::id());
    let mut run = LocalMpcRun::new(
        root.canonicalize()?,
        program,
        options.n_parties,
        options.threshold,
        "malicious-shamir",
        Some(ED25519_ORDER.into()),
    )?;
    run.install(&source, &party_files)?;
    let _ = run.compile(253)?;
    let extra = vec!["-P".into(), ED25519_ORDER.into()];
    let mut samples = Vec::new();
    let mut verdicts = Vec::new();
    for _ in 0..options.repeats {
        let observed = run.execute_stock_observed("malicious-shamir-party.x", &extra, &[])?;
        if !observed.ok {
            output["circuit"] = json!({"ran": false, "why": "a party failed"});
            break;
        }
        verdicts.push(verify_rfq(&observed.combined, &honest.reference)?);
        samples.push((
            observed.party0_rounds,
            observed.global_mb,
            observed.wall_seconds,
        ));
    }
    if samples.len() == options.repeats {
        output["circuit"] = json!({
            "ran": true,
            "verified": verdicts.iter().all(|value| value.0),
            "detail": verdicts.first().map(|value| value.1.clone()).unwrap_or_default(),
            "rounds": samples[0].0,
            "global_mb": samples[0].1,
            "wall_s_median": median(&samples.iter().map(|value| value.2).collect::<Vec<_>>()),
            "repeats": options.repeats,
        });
    }
    let _ = fs::remove_dir_all(work);
    write_pretty_json(Some(&options.out), &output)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn deal_market(options: &Options, ref_table: &[i128]) -> HarnessResult<DealtMarket> {
    let started = Instant::now();
    let input_config = InputConfig {
        n_mm: options.n_mm,
        n_real_mm: options.n_mm,
        n_parties: options.n_parties,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        ref_table,
        user_asset: 0,
        user_qty: options.qty,
        user_dir: 0,
        user_entity: 0,
        now_t: 1,
        seed: options.seed,
        audit_gates: false,
        value_bits: 32,
        field_bits: 253,
        use_ref: 1,
        reference: Reference::Anchored,
        input_check: false,
        check_mode: CheckMode::Aggregate,
        binding_limit: false,
        user_limit: 100_000,
        check_coefficients: &[],
        check_repeats: 7,
        policies: None,
        shamir_inputs: false,
        shamir_threshold: options.threshold,
    };
    let mut generated = build_inputs(&input_config)?;
    let additive = generated
        .party_files()
        .iter()
        .map(|text| {
            text.split_whitespace()
                .map(str::parse::<i128>)
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let positions = additive
        .first()
        .map(Vec::len)
        .ok_or("no generated party inputs")?;
    let values = (0..positions)
        .map(|position| {
            additive.iter().try_fold(0i128, |sum, party| {
                sum.checked_add(party[position])
                    .ok_or("input reconstruction overflow")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let key = Pedersen::new(b"qomm:binding:v1");
    let labels = (0..positions)
        .map(|position| format!("input:{position}"))
        .collect();
    let mut dealer = BindingDealer::new(key, options.n_parties, options.threshold, labels)?;
    for (position, value) in values.into_iter().enumerate() {
        dealer.deal(i64::try_from(value)?, position, &mut OsRng)?;
    }
    let bound = dealer.bound();
    let party_values = (1..=options.n_parties)
        .map(|party| bound.party_file(party))
        .collect::<Result<Vec<_>, _>>()?;
    let padded = pow2_ceil(options.n_mm)?;
    let sentinel = sentinel_for(options.bit_length, padded, 8 * ref_table[0])?;
    finish_reference(&mut generated, &input_config, sentinel, Mode::Rfq)?;
    let reference = serde_json::from_str(&generated.reference_json())?;
    Ok(DealtMarket {
        dealer,
        bound,
        party_values,
        reference,
        deal_ms: started.elapsed().as_secs_f64() * 1e3,
    })
}

fn reconstruct_blinding(bound: &BoundInputs, position: usize) -> HarnessResult<Scalar> {
    let dealt = bound.values.get(position).ok_or("missing dealt value")?;
    let points = qomm_zk::shamir::points(bound.n_parties);
    let shares = (1..=bound.n_parties)
        .map(|party| {
            dealt
                .shares
                .blinding_shares
                .get(&party)
                .copied()
                .ok_or("missing blinding share")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(shamir::reconstruct(&points, &shares))
}

fn prove_dealt_range<R: rand_core::RngCore + rand_core::CryptoRng>(
    dealer: &BindingDealer,
    bound: &BoundInputs,
    position: usize,
    value: i64,
    blinding: &Scalar,
    low: i64,
    high: i64,
    context: &[u8],
    rng: &mut R,
) -> Result<(BoundedProof, usize), String> {
    let dealt = bound
        .values
        .get(position)
        .ok_or_else(|| format!("no dealt value at position {position}"))?;
    let (commitment, proof, bits) =
        prove_bounded(&dealer.key, value, blinding, low, high, context, rng)
            .map_err(str::to_string)?;
    if commitment != dealt.shares.commitment {
        return Err("range proof is about a different commitment".into());
    }
    Ok((proof, bits))
}

fn check_dealt_range(
    key: &Pedersen,
    bound: &BoundInputs,
    position: usize,
    proof: &BoundedProof,
    low: i64,
    high: i64,
    context: &[u8],
) -> bool {
    bound.values.get(position).is_some_and(|dealt| {
        verify_bounded(key, &dealt.shares.commitment, proof, low, high, context)
    })
}

fn position_of(maker: usize, field: &str, binding_limit: bool) -> Result<usize, String> {
    let field = FIELDS
        .iter()
        .position(|candidate| *candidate == field)
        .ok_or_else(|| format!("{field} is not a policy field"))?;
    Ok(5 + 1 + usize::from(binding_limit) * 2 + maker * FIELDS.len() + field)
}

fn commitment_at(
    bound: &BoundInputs,
    maker: usize,
    field: &str,
    binding_limit: bool,
) -> Result<RistrettoPoint, String> {
    let position = position_of(maker, field, binding_limit)?;
    bound
        .values
        .get(position)
        .map(|dealt| dealt.shares.commitment)
        .ok_or_else(|| format!("no dealt value at position {position}"))
}

fn reconstruct(bound: &BoundInputs, position: usize) -> Result<Scalar, String> {
    let dealt = bound
        .values
        .get(position)
        .ok_or_else(|| "missing dealt value".to_string())?;
    let points = qomm_zk::shamir::points(bound.n_parties);
    let shares = (1..=bound.n_parties)
        .map(|party| {
            dealt
                .shares
                .value_shares
                .get(&party)
                .copied()
                .ok_or_else(|| "missing value share".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(shamir::reconstruct(&points, &shares))
}

fn scalar_to_i64(value: Scalar) -> Result<i64, String> {
    let bytes = value.to_bytes();
    if bytes[8..].iter().all(|byte| *byte == 0) {
        return i64::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
            .map_err(|error| error.to_string());
    }
    let negative = (-value).to_bytes();
    if negative[8..].iter().all(|byte| *byte == 0) {
        return i64::try_from(u64::from_le_bytes(negative[..8].try_into().unwrap()))
            .map(|value| -value)
            .map_err(|error| error.to_string());
    }
    Err("reconstructed input does not fit i64".into())
}

fn scalar_decimal(value: &Scalar) -> String {
    let mut digits = vec![0u8];
    for byte in value.to_bytes().iter().rev() {
        let mut carry = *byte as u16;
        for digit in &mut digits {
            let next = *digit as u16 * 256 + carry;
            *digit = (next % 10) as u8;
            carry = next / 10;
        }
        while carry != 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    digits
        .iter()
        .rev()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn verify_rfq(log: &str, reference: &Value) -> HarnessResult<(bool, String)> {
    let padded = json_i128(&reference["padded_mm"])?;
    let mask = reference
        .get("mask")
        .map(json_i128)
        .transpose()?
        .unwrap_or(0);
    let Some(masked) = named_integer(log, "QOMM_MASKED_KEY=") else {
        return Ok((false, "no masked quote in log".into()));
    };
    let got = unpack_key(masked - mask, padded);
    let want = (
        json_i128(&reference["best_cost"])?,
        json_i128(&reference["best_mm"])?,
    );
    Ok((
        got == want,
        format!("got=({}, {}) want=({}, {})", got.0, got.1, want.0, want.1),
    ))
}

fn named_integer(text: &str, marker: &str) -> Option<i128> {
    text.lines().find_map(|line| {
        let at = line.find(marker)?;
        let rest = &line[at + marker.len()..];
        let end = rest
            .char_indices()
            .take_while(|(index, ch)| ch.is_ascii_digit() || (*index == 0 && *ch == '-'))
            .last()
            .map_or(0, |(index, ch)| index + ch.len_utf8());
        rest[..end].parse().ok()
    })
}

fn unpack_key(key: i128, padded: i128) -> (i128, i128) {
    let index = key.rem_euclid(padded);
    ((key - index) / padded, index)
}

fn json_i128(value: &Value) -> HarnessResult<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .ok_or_else(|| format!("expected an integer, got {value}").into())
}

fn round_places(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    qomm_sim::market::py_round(value * scale) as f64 / scale
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        mp_spdz_root: None,
        n_mm: 8,
        n_parties: 7,
        threshold: 2,
        qty: 20,
        bit_length: 31,
        seed: 5,
        repeats: 3,
        out: qomm_harness::repo_root().join("artifacts/binding_chain.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--mp-spdz-root" => {
                options.mp_spdz_root =
                    Some(PathBuf::from(value(&raw, &mut index, "--mp-spdz-root")?))
            }
            "--n-mm" => options.n_mm = parse_value(value(&raw, &mut index, "--n-mm")?, "--n-mm")?,
            "--n-parties" => {
                options.n_parties =
                    parse_value(value(&raw, &mut index, "--n-parties")?, "--n-parties")?
            }
            "--threshold" => {
                options.threshold =
                    parse_value(value(&raw, &mut index, "--threshold")?, "--threshold")?
            }
            "--qty" => options.qty = parse_value(value(&raw, &mut index, "--qty")?, "--qty")?,
            "--bit-length" => {
                options.bit_length =
                    parse_value(value(&raw, &mut index, "--bit-length")?, "--bit-length")?
            }
            "--seed" => options.seed = parse_value(value(&raw, &mut index, "--seed")?, "--seed")?,
            "--repeats" => {
                options.repeats = parse_value(value(&raw, &mut index, "--repeats")?, "--repeats")?
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
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
    use qomm_proofs::threshold_quote::RISTRETTO_SCALAR_ORDER_LE;
    use qomm_transport::binding::check_share;

    fn options(n_mm: usize) -> Options {
        Options {
            mp_spdz_root: None,
            n_mm,
            n_parties: 7,
            threshold: 2,
            qty: 20,
            bit_length: 31,
            seed: 5,
            repeats: 1,
            out: PathBuf::new(),
        }
    }

    fn fixture() -> DealtMarket {
        deal_market(&options(8), &[100_000]).unwrap()
    }

    fn scalar_from_decimal(value: &str) -> Scalar {
        value.bytes().fold(Scalar::ZERO, |acc, digit| {
            acc * Scalar::from(10_u64) + Scalar::from(u64::from(digit - b'0'))
        })
    }

    fn decimal_from_le_bytes(bytes: &[u8]) -> String {
        let mut digits = vec![0_u8];
        for byte in bytes.iter().rev() {
            let mut carry = u16::from(*byte);
            for digit in &mut digits {
                let next = u16::from(*digit) * 256 + carry;
                *digit = (next % 10) as u8;
                carry = next / 10;
            }
            while carry != 0 {
                digits.push((carry % 10) as u8);
                carry /= 10;
            }
        }
        digits
            .iter()
            .rev()
            .map(|digit| char::from(b'0' + *digit))
            .collect()
    }

    #[test]
    fn the_party_file_is_the_dealt_share_and_not_a_second_dealing() {
        let market = fixture();
        for party in 1..=market.bound.n_parties {
            assert_eq!(
                market.bound.party_file(party).unwrap(),
                market.party_values[party - 1]
            );
        }
    }

    #[test]
    fn the_two_copies_of_the_prime_agree() {
        assert_eq!(
            decimal_from_le_bytes(&RISTRETTO_SCALAR_ORDER_LE),
            ED25519_ORDER
        );
    }

    #[test]
    fn every_value_the_circuit_reads_was_committed_to() {
        let market = fixture();
        assert_eq!(
            market.bound.values.len(),
            market.bound.party_file(1).unwrap().len()
        );
        assert!(check_all(&market.dealer.key, &market.bound).is_empty());
    }

    #[test]
    fn the_public_coefficients_rebuild_a_degree_t_polynomial() {
        let coefficients = ed25519_lagrange_at_zero(7)
            .unwrap()
            .iter()
            .map(|value| scalar_from_decimal(value))
            .collect::<Vec<_>>();
        let points = shamir::points(7);
        let mut rng = OsRng;
        for value in [
            Scalar::ZERO,
            Scalar::ONE,
            Scalar::from(4_242_u64),
            -Scalar::from(17_u64),
            -Scalar::ONE,
        ] {
            let shares = shamir::share(&value, 2, &points, &mut rng);
            let rebuilt = coefficients
                .iter()
                .zip(&shares)
                .fold(Scalar::ZERO, |sum, (coefficient, share)| {
                    sum + coefficient * share
                });
            assert_eq!(rebuilt, value);
        }
    }

    #[test]
    fn the_reconstruction_is_the_committed_value() {
        let market = fixture();
        let request = (0..5)
            .map(|position| scalar_to_i64(reconstruct(&market.bound, position).unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(request, vec![0, 20, 0, 0, 1]);

        let field = |name: &str| {
            let position = position_of(0, name, false).unwrap();
            scalar_to_i64(reconstruct(&market.bound, position).unwrap()).unwrap()
        };
        let anchor = field("use_ref") * 100_000 + field("ask_level");
        let depth = field("slope") * 20;
        let skew = field("invcoef") * field("inv");
        let quote = &market.reference["quotes"][0];
        assert_eq!(
            json_i128(&quote["ask"]).unwrap(),
            i128::from(anchor + depth + skew)
        );
        assert_eq!(
            json_i128(&quote["bid"]).unwrap(),
            i128::from(anchor - field("spread") - depth + skew)
        );
    }

    #[test]
    fn any_threshold_plus_one_nodes_reconstruct() {
        let market = fixture();
        let dealt = &market.bound.values[1];
        for subset in [[1_usize, 2, 3], [5, 6, 7], [1, 4, 7]] {
            let points = subset.map(|party| Scalar::from(party as u64));
            let shares = subset.map(|party| dealt.shares.value_shares[&party]);
            assert_eq!(shamir::reconstruct(&points, &shares), Scalar::from(20_u64));
        }
    }

    #[test]
    fn a_share_that_does_not_open_its_commitment_is_caught() {
        let market = fixture();
        assert!(check_all(&market.dealer.key, &market.bound).is_empty());
        let mut tampered = market.bound.clone();
        *tampered.values[8].shares.value_shares.get_mut(&4).unwrap() += Scalar::ONE;
        assert_eq!(check_all(&market.dealer.key, &tampered), vec![(4, 8)]);
    }

    #[test]
    fn a_dealer_that_swaps_the_whole_value_is_caught_at_every_node() {
        let market = fixture();
        assert!(check_all(&market.dealer.key, &market.bound).is_empty());
        let mut other =
            BindingDealer::new(market.dealer.key.clone(), 7, 2, vec!["other".into()]).unwrap();
        other.deal(999, 0, &mut OsRng).unwrap();
        let mut tampered = market.bound.clone();
        let committed = tampered.values[8].shares.commitment;
        tampered.values[8].shares = other.bound().values[0].shares.clone();
        tampered.values[8].shares.commitment = committed;
        assert_eq!(
            check_all(&market.dealer.key, &tampered),
            (1..=7).map(|party| (party, 8)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn one_node_checks_only_its_own_share() {
        let market = fixture();
        for party in 1..=market.bound.n_parties {
            assert!(check_share(
                &market.bound.values[0],
                party,
                &market.dealer.key
            ));
        }
    }

    #[test]
    fn the_range_proof_is_about_the_commitment_that_was_dealt() {
        let market = fixture();
        let position = position_of(0, "spread", false).unwrap();
        let value = scalar_to_i64(reconstruct(&market.bound, position).unwrap()).unwrap();
        let blinding = reconstruct_blinding(&market.bound, position).unwrap();
        let band = PolicyBounds::default().spread;
        let (proof, _) = prove_dealt_range(
            &market.dealer,
            &market.bound,
            position,
            value,
            &blinding,
            band.0,
            band.1,
            b"band",
            &mut OsRng,
        )
        .unwrap();
        assert!(check_dealt_range(
            &market.dealer.key,
            &market.bound,
            position,
            &proof,
            band.0,
            band.1,
            b"band"
        ));
    }

    #[test]
    fn a_range_proof_against_a_different_blinding_is_refused() {
        let market = fixture();
        let position = position_of(0, "spread", false).unwrap();
        let value = scalar_to_i64(reconstruct(&market.bound, position).unwrap()).unwrap();
        let blinding = reconstruct_blinding(&market.bound, position).unwrap();
        let band = PolicyBounds::default().spread;
        assert!(prove_dealt_range(
            &market.dealer,
            &market.bound,
            position,
            value,
            &blinding,
            band.0,
            band.1,
            b"band",
            &mut OsRng,
        )
        .is_ok());
        let error = prove_dealt_range(
            &market.dealer,
            &market.bound,
            position,
            value,
            &(blinding + Scalar::ONE),
            band.0,
            band.1,
            b"band",
            &mut OsRng,
        )
        .unwrap_err();
        assert!(error.contains("different commitment"));
    }

    #[test]
    fn a_value_outside_the_band_cannot_be_proved_inside_it() {
        let key = Pedersen::new(b"qomm:binding:v1");
        let mut honest = BindingDealer::new(key.clone(), 7, 2, vec!["spread".into()]).unwrap();
        honest.deal(100, 0, &mut OsRng).unwrap();
        let honest_bound = honest.bound();
        let honest_blinding = reconstruct_blinding(&honest_bound, 0).unwrap();
        let band = PolicyBounds::default().spread;
        assert!(prove_dealt_range(
            &honest,
            &honest_bound,
            0,
            100,
            &honest_blinding,
            band.0,
            band.1,
            b"band",
            &mut OsRng,
        )
        .is_ok());

        let mut outside = BindingDealer::new(key, 7, 2, vec!["spread".into()]).unwrap();
        outside.deal(9_999, 0, &mut OsRng).unwrap();
        let outside_bound = outside.bound();
        let outside_blinding = reconstruct_blinding(&outside_bound, 0).unwrap();
        let error = prove_dealt_range(
            &outside,
            &outside_bound,
            0,
            9_999,
            &outside_blinding,
            band.0,
            band.1,
            b"band",
            &mut OsRng,
        )
        .unwrap_err();
        assert_eq!(error, "value outside the bounded interval");
    }

    #[test]
    fn a_node_substituting_its_input_is_not_caught_here() {
        let market = fixture();
        let mut fed = market.bound.party_file(4).unwrap();
        fed[8] += Scalar::ONE;
        assert!(check_all(&market.dealer.key, &market.bound).is_empty());
        assert_ne!(fed, market.bound.party_file(4).unwrap());
    }

    #[test]
    fn the_dealer_refuses_a_threshold_its_party_count_cannot_carry() {
        let key = Pedersen::new(b"qomm:binding:v1");
        assert!(BindingDealer::new(key.clone(), 5, 2, vec![]).is_ok());
        let error = BindingDealer::new(key, 4, 2, vec![]).err().unwrap();
        assert!(error.contains("cannot carry"));
    }

    #[test]
    fn the_position_of_a_field_is_where_the_circuit_reads_it() {
        let market = fixture();
        for maker in [0, 1, 7] {
            for field in ["asset", "inv", "use_ref"] {
                let position = position_of(maker, field, false).unwrap();
                let base = 5
                    + 1
                    + maker * FIELDS.len()
                    + FIELDS
                        .iter()
                        .position(|candidate| *candidate == field)
                        .unwrap();
                assert_eq!(position, base);
                assert_eq!(market.bound.values[position].position, position);
                assert_eq!(
                    reconstruct(&market.bound, position).unwrap(),
                    reconstruct(&market.bound, base).unwrap()
                );
            }
        }
        assert_eq!(
            commitment_at(&market.bound, 3, "inv", false).unwrap(),
            market.bound.values[position_of(3, "inv", false).unwrap()]
                .shares
                .commitment
        );
    }

    #[test]
    fn a_field_that_is_not_a_policy_field_is_refused() {
        assert_eq!(position_of(0, "asset", false).unwrap(), 6);
        let error = position_of(0, "not_a_field", false).unwrap_err();
        assert!(error.contains("not a policy field"));
    }

    #[test]
    fn the_binding_limit_moves_every_policy_along() {
        assert_eq!(
            position_of(0, "asset", true).unwrap() - position_of(0, "asset", false).unwrap(),
            2
        );
    }
}
