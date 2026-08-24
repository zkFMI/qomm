import hashlib
import random

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from qomm_audit.distributed_dp import BudgetState, DpMechanism, U64
from qomm_audit.publication import PublicationStatement, certify


def test_cdf_is_complete_monotone_and_reference_samples_inside_support():
    mechanism = DpMechanism(1_000_000, 3, 64)
    thresholds = mechanism.thresholds()
    assert len(thresholds) == 129 and thresholds[-1] == U64
    assert all(a < b for a, b in zip(thresholds, thresholds[1:]))
    samples = [mechanism.sample_u64(random.Random(7).randrange(U64)) for _ in range(20)]
    assert all(-64 <= sample <= 64 for sample in samples)


def test_emitted_program_keeps_exact_value_and_randomness_secret():
    mechanism = DpMechanism(500_000, 10, 32)
    source = mechanism.mp_spdz_source(
        n_parties=7, budget_total_micros=2_000_000,
        budget_spent_micros=500_000)
    assert "sint.get_input_from(p)" in source
    assert "sint.get_random_bit()" in source
    assert source.count(".reveal()") == 1
    assert "exact.reveal" not in source and "u.reveal" not in source
    assert "published.reveal" in source


def test_budget_exhaustion_blocks_before_circuit_generation():
    mechanism = DpMechanism(750_000, 1, 16)
    with pytest.raises(ValueError, match="budget"):
        mechanism.mp_spdz_source(n_parties=7, budget_total_micros=1_000_000,
                                 budget_spent_micros=500_000)
    state = BudgetState(1_000_000).spend(mechanism)
    with pytest.raises(ValueError, match="exhausted"):
        state.spend(mechanism)


def statement(mechanism, before=0, previous=b"\0" * 32, epoch=1):
    delta_n, delta_d = mechanism.rounding_delta
    return PublicationStatement(
        venue="QOMM", epoch=epoch, slot_start=10, slot_end=19,
        source_digest=hashlib.sha256(b"private source rows").digest(),
        rule_digest=hashlib.sha256(b"entity clipping rule").digest(),
        mechanism_digest=mechanism.digest,
        private_input_commitment=hashlib.sha256(b"secret aggregate commitment").digest(),
        transcript_digest=hashlib.sha256(b"malicious secure MPC transcript").digest(),
        output_name="request_count", output_value=73,
        epsilon_micros=mechanism.epsilon_micros,
        delta_numerator=delta_n, delta_denominator=delta_d,
        budget_total_micros=4_000_000, budget_before_micros=before,
        budget_after_micros=before + mechanism.epsilon_micros,
        previous_certificate=previous)


def test_quorum_certificate_binds_source_rule_budget_output_and_chain():
    mechanism = DpMechanism(500_000, 3, 32)
    keys = {f"node-{i}": Ed25519PrivateKey.generate() for i in range(7)}
    registry = {node: key.public_key() for node, key in keys.items()}
    first = certify(statement(mechanism), dict(list(keys.items())[:3]))
    assert first.verify(registry, 3)
    second_statement = statement(mechanism, before=500_000,
                                 previous=first.digest, epoch=2)
    second = certify(second_statement, dict(list(keys.items())[2:5]))
    assert second.verify(registry, 3, previous=first)

    moved = type(second_statement)(
        **{**second_statement.__dict__, "output_value": 74})
    forged = type(second)(moved, second.signatures)
    assert not forged.verify(registry, 3, previous=first)


def test_two_signers_do_not_meet_a_three_of_seven_trust_rule():
    mechanism = DpMechanism(500_000, 3, 32)
    keys = {f"node-{i}": Ed25519PrivateKey.generate() for i in range(7)}
    registry = {node: key.public_key() for node, key in keys.items()}
    certificate = certify(statement(mechanism), dict(list(keys.items())[:2]))
    assert not certificate.verify(registry, 3)
