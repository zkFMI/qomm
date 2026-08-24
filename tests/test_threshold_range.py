"""A range proof assembled from shares, and what it does and does not establish.

The claim being tested is not that range proofs exist. It is that this one is
built by a quorum of nodes none of which ever holds the value, and that an
ordinary verifier --- no setup, no shares, no knowledge of who assembled it ---
accepts it. The paper records the joint assembly of range proofs as an open
problem; this file is what closes it, so the forgery tests matter more than the
success ones.

The substitution that makes it possible is `b*b = b` in place of the
disjunction, and the first test is that the substitution actually refuses a
non-bit. A square proof that accepted 2 would make the whole thing decorative.
"""

import pytest

from zk.commit import Pedersen, prove_range, verify_range
from zk.groups import make_group
from zk.threshold_range import (BitShares, ValueShares, deal_bits,
                                joint_prove_range, verify_threshold_range)
from zk.threshold_sigma import lagrange_at_zero

PARTIES = [1, 2, 3, 4, 5, 6, 7]
T = 2
WIDTH = 16


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:quote:v1")


def reconstruct(shares, parties, order):
    coefficients = lagrange_at_zero(parties, order)
    return sum(coefficients[p] * shares[p] for p in parties) % order


# --- what it refuses --------------------------------------------------------

def test_a_committed_two_is_not_accepted_as_a_bit(key):
    """The square formulation has to do the work the disjunction did.

    `b*b = b` holds only for 0 and 1 in a field. If the proof could be assembled
    for a commitment holding 2, every range proof here would be vacuous, so this
    is the test the construction stands on.
    """
    group = key.group
    order = group.order
    r = group.random_scalar()
    dishonest = BitShares(
        commitment=key.commit(2, r),
        bit={p: 2 for p in PARTIES},
        blinding={p: r for p in PARTIES},
        cross={p: (r * (1 - 2)) % order for p in PARTIES},
    )
    shares = ValueShares(commitment=key.commit(2, r), value={p: 2 for p in PARTIES},
                         blinding={p: r for p in PARTIES}, bits=(dishonest,),
                         threshold=0)
    proof, _ = joint_prove_range(key, shares, [1, 2, 3], b"ctx")
    assert not verify_threshold_range(key, shares.commitment, proof, b"ctx"), (
        "a commitment holding 2 passed as a bit, so the square proof is not "
        "proving what the disjunction proved")


def test_a_value_that_does_not_fit_the_width_is_refused(key):
    with pytest.raises(ValueError):
        deal_bits(key, 1 << WIDTH, key.group.random_scalar(), WIDTH, PARTIES, T)


def test_a_proof_does_not_verify_against_another_commitment(key):
    group = key.group
    a = deal_bits(key, 1234, group.random_scalar(), WIDTH, PARTIES, T)
    b = deal_bits(key, 1234, group.random_scalar(), WIDTH, PARTIES, T)
    proof, _ = joint_prove_range(key, a, [1, 2, 3], b"ctx")
    assert verify_threshold_range(key, a.commitment, proof, b"ctx")
    assert not verify_threshold_range(key, b.commitment, proof, b"ctx"), (
        "the same value under a different blinding accepted the other's proof")


def test_the_context_is_bound(key):
    shares = deal_bits(key, 999, key.group.random_scalar(), WIDTH, PARTIES, T)
    proof, _ = joint_prove_range(key, shares, [1, 2, 3], b"slot-7")
    assert not verify_threshold_range(key, shares.commitment, proof, b"slot-8")


def test_a_node_that_answers_on_the_wrong_share_breaks_the_proof(key):
    """Not attribution --- that is `audit_partials` --- but the proof must fail."""
    group = key.group
    shares = deal_bits(key, 4321, group.random_scalar(), WIDTH, PARTIES, T)
    tampered = list(shares.bits)
    victim = tampered[0]
    tampered[0] = BitShares(
        commitment=victim.commitment,
        bit={**victim.bit, 2: (victim.bit[2] + 1) % group.order},
        blinding=victim.blinding, cross=victim.cross)
    shares = ValueShares(shares.commitment, shares.value, shares.blinding,
                         tuple(tampered), shares.threshold)
    proof, _ = joint_prove_range(key, shares, [1, 2, 3], b"ctx")
    assert not verify_threshold_range(key, shares.commitment, proof, b"ctx")


# --- what it establishes ----------------------------------------------------

def test_a_quorum_assembles_a_proof_an_ordinary_verifier_accepts(key):
    for value in (0, 1, 2, 255, 1023, (1 << WIDTH) - 1):
        shares = deal_bits(key, value, key.group.random_scalar(), WIDTH, PARTIES, T)
        proof, transcript = joint_prove_range(key, shares, [1, 2, 3], b"ctx")
        assert verify_threshold_range(key, shares.commitment, proof, b"ctx"), value
        assert transcript["width"] == WIDTH


def test_any_quorum_of_t_plus_one_works_and_fewer_does_not(key):
    shares = deal_bits(key, 777, key.group.random_scalar(), WIDTH, PARTIES, T)
    for quorum in ([1, 2, 3], [5, 6, 7], [2, 4, 6], [1, 2, 3, 4, 5, 6, 7]):
        proof, _ = joint_prove_range(key, shares, quorum, b"ctx")
        assert verify_threshold_range(key, shares.commitment, proof, b"ctx"), quorum

    # T shares are not a quorum: the responses interpolate to the wrong scalar,
    # so the proof simply does not verify. That is the threshold doing its job.
    short, _ = joint_prove_range(key, shares, [1, 2], b"ctx")
    assert not verify_threshold_range(key, shares.commitment, short, b"ctx")


def test_no_node_holds_the_value_or_any_bit(key):
    """The property the whole construction exists for, checked rather than asserted."""
    group = key.group
    order = group.order
    value = 0b1011010110110101 & ((1 << WIDTH) - 1)
    shares = deal_bits(key, value, group.random_scalar(), WIDTH, PARTIES, T)

    for party in PARTIES:
        assert shares.value[party] != value, f"party {party} holds the value"
        for j, bit in enumerate(shares.bits):
            assert bit.bit[party] in range(order)
            # a share equal to the bit would be a share that leaks it
            assert bit.bit[party] != (value >> j) & 1 or bit.bit[party] == 0, (
                f"party {party} holds bit {j} in the clear")

    # Any T of them reconstruct the wrong value; T+1 reconstruct the right one.
    assert reconstruct(shares.value, [1, 2], order) != value
    assert reconstruct(shares.value, [1, 2, 3], order) == value
    for j, bit in enumerate(shares.bits):
        assert reconstruct(bit.bit, [4, 5, 6], order) == (value >> j) & 1


# --- what it costs against the proof it replaces ----------------------------

def test_the_square_proof_is_the_same_statement_as_the_ordinary_one(key):
    """Both establish the value is in [0, 2^width). Only the bit step differs."""
    group = key.group
    value, blinding = 40_000 % (1 << WIDTH), group.random_scalar()
    shares = deal_bits(key, value, blinding, WIDTH, PARTIES, T)
    joint, _ = joint_prove_range(key, shares, [1, 2, 3], b"ctx")
    local = prove_range(key, shares.commitment, value, blinding, WIDTH, b"ctx")

    assert verify_threshold_range(key, shares.commitment, joint, b"ctx")
    assert verify_range(key, shares.commitment, local, b"ctx")
    assert joint.bits == local.bits == WIDTH
    assert len(joint.bit_commitments) == len(local.bit_commitments)
