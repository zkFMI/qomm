use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use qomm_harness::local_mpc::{maybe_run_party, LocalMpcRun};
use qomm_harness::{parse_value, unique_temp_dir, write_pretty_json, HarnessResult};
use qomm_mpc::inputs::{build_inputs, InputConfig};
use qomm_mpc::program::{
    build_program, pow2_ceil, CheckMode, Mode, ProgramConfig, Reference, ED25519_ORDER, FIELDS,
};
use zkfmi_zk::pedersen::Pedersen;
use rand::rngs::OsRng;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

struct Options {
    root: PathBuf,
    n_mm: usize,
    parties: usize,
    threshold: usize,
    field_bits: i128,
    tamper_index: usize,
    tamper_party: Option<usize>,
    out: PathBuf,
}

struct RawRun {
    openings: BTreeMap<usize, String>,
    challenge: String,
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
    if options.parties == 0 {
        return Err("--parties must be positive".into());
    }
    let tamper_party = options
        .tamper_party
        .unwrap_or_else(|| 4.min(options.parties - 1));
    let n_checked = 4 + 2 + options.n_mm * FIELDS.len();
    if tamper_party >= options.parties {
        return Err("--tamper-party is outside --parties".into());
    }
    if options.tamper_index >= n_checked {
        return Err("--tamper-index is outside the checked input vector".into());
    }
    let padded = pow2_ceil(options.n_mm)?;
    let config = ProgramConfig {
        n_mm: padded,
        n_parties: options.parties,
        mode: Mode::Rfq,
        input_check: true,
        check_mode: CheckMode::PerParty,
        check_repeats: 1,
        bit_length: 63,
        ..ProgramConfig::default()
    };
    let program_text = build_program(&config)?;
    let ref_table = [100_000i128];
    let input_config = InputConfig {
        n_mm: padded,
        n_real_mm: options.n_mm,
        n_parties: options.parties,
        is_real: 1,
        n_requests: 1,
        n_assets: 1,
        ref_table: &ref_table,
        user_asset: 0,
        user_qty: 100,
        user_dir: 0,
        user_entity: 42,
        now_t: config.now_t,
        seed: 7,
        audit_gates: false,
        value_bits: 64,
        field_bits: options.field_bits,
        use_ref: 1,
        reference: Reference::Anchored,
        input_check: true,
        check_mode: CheckMode::PerParty,
        binding_limit: false,
        user_limit: 100_000,
        user_limit_blinding: 1,
        user_qty_blinding: 1,
        response_mask: None,
        fill_mask: None,
        check_coefficients: &config.check_coefficients,
        check_repeats: 1,
        policies: None,
        shamir_inputs: false,
        shamir_threshold: (options.parties - 1) / 2,
        dvp: None,
        quote_proof: None,
    };
    let generated = build_inputs(&input_config)?;
    let party_files = generated.party_files();
    let per_party = party_files
        .iter()
        .map(|file| {
            file.split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if per_party.iter().any(|row| row.len() <= n_checked) {
        return Err("generated party inputs do not contain the per-party mask".into());
    }

    let key = Pedersen::new(b"qomm:pedersen:v1");
    let mut rng = OsRng;
    let shares = per_party
        .iter()
        .map(|row| row[..n_checked].to_vec())
        .collect::<Vec<_>>();
    let masks = per_party
        .iter()
        .map(|row| row[n_checked].clone())
        .collect::<Vec<_>>();
    let blindings = (0..options.parties)
        .map(|_| {
            (0..n_checked)
                .map(|_| Scalar::random(&mut rng))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mask_blindings = (0..options.parties)
        .map(|_| Scalar::random(&mut rng))
        .collect::<Vec<_>>();
    let commitments = shares
        .iter()
        .zip(&blindings)
        .map(|(row, row_blindings)| {
            row.iter()
                .zip(row_blindings)
                .map(|(value, blinding)| key.commit(&scalar_from_decimal(value), blinding))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mask_commitments = masks
        .iter()
        .zip(&mask_blindings)
        .map(|(value, blinding)| key.commit(&scalar_from_decimal(value), blinding))
        .collect::<Vec<_>>();

    let work = unique_temp_dir("qomm-identity")?;
    let source = work.join("identity.mpc");
    fs::write(&source, program_text)?;
    let program = format!("identity_{}", std::process::id());
    let mut mpc = LocalMpcRun::new(
        options.root.canonicalize()?,
        program,
        options.parties,
        options.threshold,
        "malicious-shamir",
        Some(ED25519_ORDER.into()),
    )?;
    mpc.install(&source, &party_files)?;
    let _ = mpc.compile(decimal_bit_length(ED25519_ORDER))?;
    let honest = parse_run(&mpc.execute()?, options.parties)?;
    let honest_verdict = verify(
        &key,
        &commitments,
        &mask_commitments,
        &blindings,
        &mask_blindings,
        &honest,
        n_checked,
    )?;

    let mut tampered_files = party_files.clone();
    let mut tampered_values = per_party[tamper_party].clone();
    tampered_values[options.tamper_index] = decimal_add_one(&tampered_values[options.tamper_index]);
    tampered_files[tamper_party] = format!("{}\n", tampered_values.join(" "));
    mpc.replace_inputs(&tampered_files)?;
    let tampered = parse_run(&mpc.execute()?, options.parties)?;
    let tampered_verdict = verify(
        &key,
        &commitments,
        &mask_commitments,
        &blindings,
        &mask_blindings,
        &tampered,
        n_checked,
    )?;
    let _ = fs::remove_dir_all(&work);

    let honest_openings = openings_json(&honest.openings)?;
    let right_node = tampered_verdict.2 == vec![tamper_party];
    let result = json!({
        "host": zkfmi_measure::hosts::this_host(),
        "question": "Is the request the circuit priced the request the taker sent, and is the policy the one the maker published?",
        "setting": {
            "n_makers": options.n_mm,
            "n_parties": options.parties,
            "threshold": options.threshold,
            "field_bits": options.field_bits,
            "checked_values": n_checked,
            "repeats": 1,
        },
        "the_seam_that_was_open": "An earlier generator revision emitted a FIXTURE coefficient list with a comment saying so. With fixture coefficients the check proves nothing --- the argument is entirely that the coefficients arrive after the commitments, so a node knowing them in advance picks its error to cancel. This run derives them from the published commitments and hands the same list to the circuit, which closes and verifies that seam.",
        "honest": {
            "verified": honest_verdict.0,
            "culprits": honest_verdict.2,
            "openings": honest_openings,
        },
        "tampered": {
            "what": format!(
                "node {} fed a different value at input {}, which is the taker's quantity --- the case a taker cares about rather than a maker",
                tamper_party, options.tamper_index
            ),
            "verified": tampered_verdict.0,
            "named": tampered_verdict.2,
            "named_the_right_node": right_node,
            "message": tampered_verdict.1,
        },
        "what_this_does_not_establish": [
            "That the maker's committed policy is one it would honour off-venue. policy_audit shows the fields sit in bands the venue published; it cannot show intent.",
            "That the request was real. The maker never sees it --- that is the design --- so it cannot tell a genuine request from probing, which is what is_real cover traffic and the disclosure budget are for.",
            "That every eligible maker was included. An omission has no commitment in the statement to fail against; rust/qomm-audit/src/receipts.rs covers that per slot, on a different axis."
        ],
    });
    println!(
        "honest run:   verified={} culprits={}",
        display_bool(honest_verdict.0),
        qomm_harness::value_display(&json!(honest_verdict.2)),
    );
    println!("node {} substitutes the taker's quantity:", tamper_party);
    println!(
        "  verified={}  named={}",
        display_bool(tampered_verdict.0),
        qomm_harness::value_display(&json!(tampered_verdict.2)),
    );
    println!("  {}", tampered_verdict.1);
    write_pretty_json(Some(&options.out), &result)?;
    println!("wrote {}", options.out.display());
    if !honest_verdict.0 || !right_node {
        return Err("identity acceptance verdict failed".into());
    }
    Ok(())
}

fn parse_run(log: &str, parties: usize) -> HarnessResult<RawRun> {
    let mut openings = BTreeMap::new();
    let mut challenge = None;
    for line in log.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("QOMM_CHALLENGE=") {
            challenge.get_or_insert_with(|| value.trim().to_string());
        }
        if let Some(rest) = line.strip_prefix("QOMM_PER_PARTY_CHECK_") {
            let Some((repeat, rest)) = rest.split_once('_') else {
                continue;
            };
            let Some((party, value)) = rest.split_once('=') else {
                continue;
            };
            if repeat == "0" {
                openings.insert(party.parse()?, value.trim().to_string());
            }
        }
    }
    if openings.len() != parties {
        return Err(format!("the circuit opened {} of {parties} parties", openings.len()).into());
    }
    Ok(RawRun {
        openings,
        challenge: challenge.ok_or("the circuit did not open a challenge; without one the check has no soundness to verify")?,
    })
}

type Verdict = (bool, String, Vec<usize>);

fn verify(
    key: &Pedersen,
    commitments: &[Vec<RistrettoPoint>],
    mask_commitments: &[RistrettoPoint],
    blindings: &[Vec<Scalar>],
    mask_blindings: &[Scalar],
    run: &RawRun,
    count: usize,
) -> HarnessResult<Verdict> {
    let challenge = scalar_from_decimal(&run.challenge);
    let mut coefficients = Vec::with_capacity(count);
    let mut coefficient = Scalar::ONE;
    for _ in 0..count {
        coefficient *= challenge;
        coefficients.push(coefficient);
    }
    let mut culprits = Vec::new();
    for party in 0..commitments.len() {
        let combined = commitments[party]
            .iter()
            .zip(&coefficients)
            .fold(mask_commitments[party], |sum, (commitment, coefficient)| {
                sum + commitment * coefficient
            });
        let blinding = blindings[party]
            .iter()
            .zip(&coefficients)
            .fold(mask_blindings[party], |sum, (value, coefficient)| {
                sum + value * coefficient
            });
        let opening = run
            .openings
            .get(&party)
            .ok_or_else(|| format!("missing opening for party {party}"))?;
        if key.commit(&scalar_from_decimal(opening), &blinding) != combined {
            culprits.push(party);
        }
    }
    if culprits.is_empty() {
        Ok((true, "ok".into(), culprits))
    } else {
        let named = culprits
            .iter()
            .map(|party| format!("node {party}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok((
            false,
            format!("the inputs {named} fed the circuit were not the ones committed to them"),
            culprits,
        ))
    }
}

fn scalar_from_decimal(text: &str) -> Scalar {
    let trimmed = text.trim();
    let (negative, digits) = trimmed
        .strip_prefix('-')
        .map_or((false, trimmed), |digits| (true, digits));
    let value = digits.bytes().fold(Scalar::ZERO, |value, digit| {
        value * Scalar::from(10u64) + Scalar::from(u64::from(digit - b'0'))
    });
    if negative {
        -value
    } else {
        value
    }
}

fn decimal_add_one(text: &str) -> String {
    if let Some(digits) = text.strip_prefix('-') {
        if digits == "1" {
            return "0".into();
        }
        return format!("-{}", decimal_sub_one(digits));
    }
    let mut bytes = text.as_bytes().to_vec();
    let mut carry = 1u8;
    for digit in bytes.iter_mut().rev() {
        let value = (*digit - b'0') + carry;
        *digit = b'0' + value % 10;
        carry = value / 10;
        if carry == 0 {
            break;
        }
    }
    if carry > 0 {
        bytes.insert(0, b'1');
    }
    String::from_utf8(bytes).expect("decimal digits are UTF-8")
}

fn decimal_sub_one(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for digit in bytes.iter_mut().rev() {
        if *digit > b'0' {
            *digit -= 1;
            break;
        }
        *digit = b'9';
    }
    let first = bytes
        .iter()
        .position(|digit| *digit != b'0')
        .unwrap_or(bytes.len() - 1);
    String::from_utf8(bytes[first..].to_vec()).expect("decimal digits are UTF-8")
}

fn openings_json(openings: &BTreeMap<usize, String>) -> HarnessResult<Value> {
    let mut out = Map::new();
    for (party, value) in openings {
        out.insert(party.to_string(), serde_json::from_str(value)?);
    }
    Ok(Value::Object(out))
}

fn decimal_bit_length(value: &str) -> usize {
    let mut digits = value.bytes().map(|byte| byte - b'0').collect::<Vec<_>>();
    let mut bits = 0;
    while !digits.is_empty() {
        let mut carry = 0u16;
        for digit in &mut digits {
            let value = carry * 10 + u16::from(*digit);
            *digit = (value / 2) as u8;
            carry = value % 2;
        }
        while digits.first() == Some(&0) {
            digits.remove(0);
        }
        bits += 1;
    }
    bits
}

fn parse_args() -> HarnessResult<Options> {
    let mut options = Options {
        root: PathBuf::new(),
        n_mm: 4,
        parties: 7,
        threshold: 2,
        field_bits: 253,
        tamper_index: 1,
        tamper_party: None,
        out: qomm_harness::repo_root().join("artifacts/identity.json"),
    };
    let raw = std::env::args_os().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < raw.len() {
        match raw[index].to_string_lossy().as_ref() {
            "--root" => options.root = PathBuf::from(value(&raw, &mut index, "--root")?),
            "--n-mm" => options.n_mm = parse_value(value(&raw, &mut index, "--n-mm")?, "--n-mm")?,
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
            "--tamper-index" => {
                options.tamper_index =
                    parse_value(value(&raw, &mut index, "--tamper-index")?, "--tamper-index")?
            }
            "--tamper-party" => {
                options.tamper_party = Some(parse_value(
                    value(&raw, &mut index, "--tamper-party")?,
                    "--tamper-party",
                )?)
            }
            "--out" => options.out = PathBuf::from(value(&raw, &mut index, "--out")?),
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

fn display_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}
