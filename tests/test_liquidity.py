import hashlib

import pytest

from zk.commit import Pedersen
from zk.groups import make_group
from zk.liquidity import (deal_liquidity_shares, joint_prove_liquidity,
                          verify_liquidity)

PARTIES = [1, 2, 3, 4, 5, 6, 7]
QUORUM = [1, 2, 3]
T = 2
QUOTE = hashlib.sha256(b"closed quote statement").digest()


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:liquidity:v1")


def prove(key, eligible, minimum=3, quorum=QUORUM):
    shares = deal_liquidity_shares(key, eligible, minimum=minimum,
                                   parties=PARTIES, threshold=T)
    proof = joint_prove_liquidity(
        key, shares, quorum, minimum=minimum,
        quote_statement_digest=QUOTE)
    commitments = [wire.commitment for wire in shares.eligibility]
    return shares, proof, commitments


def test_threshold_proof_verifies_without_disclosing_exact_count(key):
    shares, proof, commitments = prove(key, [1, 0, 1, 1, 0, 1, 1], minimum=3)
    assert verify_liquidity(key, proof,
                            expected_eligibility_commitments=commitments,
                            quote_statement_digest=QUOTE)
    assert not hasattr(proof, "count")
    for party in PARTIES:
        reconstructed_by_one = sum(wire.value[party] for wire in shares.eligibility)
        assert reconstructed_by_one != 5


def test_false_threshold_has_no_proof(key):
    with pytest.raises(ValueError, match="fewer"):
        deal_liquidity_shares(key, [1, 0, 0, 1], minimum=3,
                              parties=PARTIES, threshold=T)


def test_quote_commitments_and_statement_digest_are_bound(key):
    _, proof, commitments = prove(key, [1, 1, 1, 0])
    moved = list(commitments)
    moved[0] = key.commit(1, key.random_blinding())
    assert not verify_liquidity(key, proof,
                                expected_eligibility_commitments=moved,
                                quote_statement_digest=QUOTE)
    assert not verify_liquidity(
        key, proof, expected_eligibility_commitments=commitments,
        quote_statement_digest=hashlib.sha256(b"another quote").digest())


def test_fewer_than_a_quorum_cannot_assemble(key):
    _, proof, commitments = prove(key, [1, 1, 1, 0], quorum=[1, 2])
    assert not verify_liquidity(key, proof,
                                expected_eligibility_commitments=commitments,
                                quote_statement_digest=QUOTE)


def test_count_commitment_cannot_be_replaced(key):
    _, proof, commitments = prove(key, [1, 1, 1, 0])
    moved = type(proof)(**{**proof.__dict__,
                           "count_commitment": key.commit(4, key.random_blinding())})
    assert not verify_liquidity(key, moved,
                                expected_eligibility_commitments=commitments,
                                quote_statement_digest=QUOTE)
