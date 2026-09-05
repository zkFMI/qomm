use ed25519_dalek::{Signer, SigningKey};
use qomm_transport::selective_disclosure::{
    open_if_winner, seal_for_winner, WinnerPrivateKey, CLEAR_BYTES,
};
use rand_core::OsRng;

#[test]
fn only_the_winner_opens_a_fixed_size_envelope() {
    let taker = SigningKey::generate(&mut OsRng);
    let winner = WinnerPrivateKey::generate().unwrap();
    let loser = WinnerPrivateKey::generate().unwrap();
    let quote = [4_u8; 32];
    let envelope = seal_for_winner(
        "maker-7",
        &winner.public_key().unwrap(),
        b"settle instruction",
        b"slot:42",
        quote,
        &taker,
    )
    .unwrap();
    assert_eq!(envelope.ciphertext.len(), CLEAR_BYTES + 16);
    assert_eq!(
        open_if_winner(
            &envelope,
            "maker-7",
            &[loser],
            b"slot:42",
            quote,
            Some(&taker.verifying_key())
        )
        .unwrap(),
        None
    );
    assert_eq!(
        open_if_winner(
            &envelope,
            "maker-7",
            &[winner],
            b"slot:42",
            quote,
            Some(&taker.verifying_key())
        )
        .unwrap(),
        Some(b"settle instruction".to_vec())
    );
}

#[test]
fn public_envelope_does_not_name_the_winner_or_key() {
    let taker = SigningKey::generate(&mut OsRng);
    let winner = WinnerPrivateKey::generate().unwrap();
    let public = winner.public_key().unwrap().raw_public_key().unwrap();
    let envelope = seal_for_winner(
        "secret-maker-name",
        &winner.public_key().unwrap(),
        b"x",
        b"market",
        [7; 32],
        &taker,
    )
    .unwrap();
    let unsigned = envelope.unsigned().unwrap();
    assert!(!unsigned
        .windows(b"secret-maker-name".len())
        .any(|part| part == b"secret-maker-name"));
    assert!(!unsigned.windows(public.len()).any(|part| part == public));
}

#[test]
fn context_quote_taker_and_oversize_tampering_are_refused() {
    let taker = SigningKey::generate(&mut OsRng);
    let winner = WinnerPrivateKey::generate().unwrap();
    let envelope = seal_for_winner(
        "m",
        &winner.public_key().unwrap(),
        b"x",
        b"market",
        [8; 32],
        &taker,
    )
    .unwrap();
    assert!(open_if_winner(
        &envelope,
        "m",
        std::slice::from_ref(&winner),
        b"other",
        [8; 32],
        None,
    )
    .unwrap_err()
    .contains("context"));
    let stranger = SigningKey::generate(&mut OsRng);
    assert!(open_if_winner(
        &envelope,
        "m",
        &[winner],
        b"market",
        [8; 32],
        Some(&stranger.verifying_key())
    )
    .unwrap_err()
    .contains("signature"));
    assert!(seal_for_winner(
        "m",
        &WinnerPrivateKey::generate().unwrap().public_key().unwrap(),
        &vec![0; CLEAR_BYTES],
        b"market",
        [8; 32],
        &taker
    )
    .unwrap_err()
    .contains("exceeds"));
}

#[test]
fn old_private_key_opens_during_rotation_overlap() {
    let taker = SigningKey::generate(&mut OsRng);
    let old = WinnerPrivateKey::generate().unwrap();
    let new = WinnerPrivateKey::generate().unwrap();
    let envelope = seal_for_winner(
        "m",
        &old.public_key().unwrap(),
        b"rotate",
        b"market",
        [9; 32],
        &taker,
    )
    .unwrap();
    assert_eq!(
        open_if_winner(&envelope, "m", &[new, old], b"market", [9; 32], None).unwrap(),
        Some(b"rotate".to_vec())
    );
}

#[test]
fn hybrid_delivery_refuses_downgrade_and_requires_both_kem_components() {
    use qomm_transport::selective_disclosure::WinnerPublicKey;
    use zkfmi_crypto::suite::{Suite, SuiteId};
    let taker = SigningKey::generate(&mut OsRng);
    let winner = WinnerPrivateKey::from_seed(&[31; 96]);
    let envelope = seal_for_winner(
        "m",
        &winner.public_key().unwrap(),
        b"private",
        b"market",
        [8; 32],
        &taker,
    )
    .unwrap();
    let open = |candidate: &qomm_transport::selective_disclosure::WinnerEnvelope| {
        open_if_winner(
            candidate,
            "m",
            std::slice::from_ref(&winner),
            b"market",
            [8; 32],
            Some(&taker.verifying_key()),
        )
    };
    assert_eq!(envelope.kem_ciphertext.len(), 1120);
    let encoded = envelope.encode().unwrap();
    let decoded = qomm_transport::selective_disclosure::WinnerEnvelope::decode(&encoded).unwrap();
    assert_eq!(decoded.encode().unwrap(), encoded);
    assert_eq!(open(&decoded).unwrap(), Some(b"private".to_vec()));
    assert!(
        qomm_transport::selective_disclosure::WinnerEnvelope::decode(&encoded[..encoded.len() - 1])
            .is_err()
    );
    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(qomm_transport::selective_disclosure::WinnerEnvelope::decode(&trailing).is_err());
    assert_eq!(
        winner.public_key().unwrap().raw_public_key().unwrap().len(),
        1216
    );
    assert!(WinnerPublicKey::from_raw(&[7; 32]).is_err());
    for at in [0, 32] {
        let mut altered = envelope.clone();
        altered.kem_ciphertext[at] ^= 1;
        // Keep the outer signature valid to reach the actual KEM/AEAD checks.
        altered.signature = taker.sign(&altered.unsigned().unwrap());
        assert_eq!(open(&altered).unwrap(), None);
    }
    let mut altered = envelope.clone();
    altered.version = 1;
    assert!(open(&altered).is_err());
    altered = envelope.clone();
    altered.suite = Suite::new(SuiteId::MlKem768);
    assert!(open(&altered).is_err());
    altered = envelope.clone();
    altered.kem_ciphertext.truncate(32);
    assert!(open(&altered).is_err());
    let mut wrong_pq_seed = [31; 96];
    wrong_pq_seed[32] ^= 1;
    assert_eq!(
        open_if_winner(
            &envelope,
            "m",
            &[WinnerPrivateKey::from_seed(&wrong_pq_seed)],
            b"market",
            [8; 32],
            None,
        )
        .unwrap(),
        None
    );
}
