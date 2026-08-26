use ed25519_dalek::SigningKey;
use qomm_audit::distributed_dp::{BudgetState, DpMechanism, U64_SPACE};
use qomm_audit::publication::{certify, PublicationCertificate, PublicationStatement, ZERO};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn statement(
    mechanism: &DpMechanism,
    before: u64,
    previous: [u8; 32],
    epoch: u64,
) -> PublicationStatement {
    let (delta_numerator, delta_denominator) = mechanism.rounding_delta();
    PublicationStatement {
        venue: "QOMM".into(),
        epoch,
        slot_start: 10,
        slot_end: 19,
        source_digest: Sha256::digest(b"private source rows").into(),
        rule_digest: Sha256::digest(b"entity clipping rule").into(),
        mechanism_digest: mechanism.digest(),
        private_input_commitment: Sha256::digest(b"secret aggregate commitment").into(),
        transcript_digest: Sha256::digest(b"malicious secure MPC transcript").into(),
        output_name: "request_count".into(),
        output_value: 73,
        epsilon_micros: mechanism.epsilon_micros,
        delta_numerator,
        delta_denominator,
        budget_total_micros: 4_000_000,
        budget_before_micros: before,
        budget_after_micros: before + mechanism.epsilon_micros,
        previous_certificate: previous,
    }
}

#[test]
fn cdf_is_complete_monotone_and_samples_stay_in_support() {
    let mechanism = DpMechanism::new(1_000_000, 3, 64).unwrap();
    let thresholds = mechanism.thresholds().unwrap();
    assert_eq!(thresholds.len(), 129);
    assert_eq!(thresholds.last(), Some(&U64_SPACE));
    assert!(thresholds.windows(2).all(|pair| pair[0] < pair[1]));
    for uniform in [0, 1, u64::MAX / 2, u64::MAX - 1, u64::MAX] {
        assert!((-64..=64).contains(&mechanism.sample_u64(uniform).unwrap()));
    }
}

#[test]
fn emitted_program_keeps_exact_value_and_randomness_secret() {
    let mechanism = DpMechanism::new(500_000, 10, 32).unwrap();
    let source = mechanism
        .mp_spdz_source(7, 2_000_000, 500_000, "published")
        .unwrap();
    assert!(source.contains("sint.get_input_from(p)"));
    assert!(source.contains("sint.get_random_bit()"));
    assert_eq!(source.matches(".reveal()").count(), 1);
    assert!(!source.contains("exact.reveal") && !source.contains("u.reveal"));
    assert!(source.contains("published.reveal"));
}

#[test]
fn privacy_budget_exhaustion_blocks_before_generation() {
    let mechanism = DpMechanism::new(750_000, 1, 16).unwrap();
    assert!(mechanism
        .mp_spdz_source(7, 1_000_000, 500_000, "published")
        .unwrap_err()
        .contains("budget"));
    let state = BudgetState {
        total_micros: 1_000_000,
        spent_micros: 0,
    }
    .spend(&mechanism)
    .unwrap();
    assert!(state.spend(&mechanism).unwrap_err().contains("exhausted"));
}

#[test]
fn quorum_certificate_binds_output_budget_source_rule_and_chain() {
    let mechanism = DpMechanism::new(500_000, 3, 32).unwrap();
    let keys = (0..7)
        .map(|index| (format!("node-{index}"), SigningKey::generate(&mut OsRng)))
        .collect::<BTreeMap<_, _>>();
    let registry = keys
        .iter()
        .map(|(node, key)| (node.clone(), key.verifying_key()))
        .collect::<BTreeMap<_, _>>();
    let first_signers = keys
        .iter()
        .take(3)
        .map(|(node, key)| (node.clone(), key.clone()))
        .collect();
    let first = certify(statement(&mechanism, 0, ZERO, 1), &first_signers).unwrap();
    assert!(first.verify(&registry, 3, None));
    let second_signers = keys
        .iter()
        .skip(2)
        .take(3)
        .map(|(node, key)| (node.clone(), key.clone()))
        .collect();
    let second = certify(
        statement(&mechanism, 500_000, first.digest().unwrap(), 2),
        &second_signers,
    )
    .unwrap();
    assert!(second.verify(&registry, 3, Some(&first)));

    let mut moved = second.statement.clone();
    moved.output_value = 74;
    let forged = PublicationCertificate {
        statement: moved,
        signatures: second.signatures,
    };
    assert!(!forged.verify(&registry, 3, Some(&first)));
}

#[test]
fn two_signers_do_not_meet_three_of_seven() {
    let mechanism = DpMechanism::new(500_000, 3, 32).unwrap();
    let keys = (0..7)
        .map(|index| (format!("node-{index}"), SigningKey::generate(&mut OsRng)))
        .collect::<BTreeMap<_, _>>();
    let registry = keys
        .iter()
        .map(|(node, key)| (node.clone(), key.verifying_key()))
        .collect::<BTreeMap<_, _>>();
    let signers = keys
        .iter()
        .take(2)
        .map(|(node, key)| (node.clone(), key.clone()))
        .collect();
    let certificate = certify(statement(&mechanism, 0, ZERO, 1), &signers).unwrap();
    assert!(!certificate.verify(&registry, 3, None));
}
