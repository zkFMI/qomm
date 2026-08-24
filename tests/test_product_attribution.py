"""Naming the node whose partial does not match its share.

A proof that fails tells you somebody cheated. It does not tell you who, and the
design's answer to a misbehaving node is to name it and slash its bond, so "the
proof did not verify" is not an answer. `threshold_sigma.audit_partials` does
this for the opening assembly; the product assembly --- which is every bit,
every gate and every conjunction in the quote proof --- had no equivalent, so
the assembly that does most of the work was the one that could not attribute.

The tests that matter are the two boundaries: a bad node is named, and an honest
one never is. A check that names everybody is as useless as one that names
nobody.
"""

import pytest

from zk.commit import Pedersen, verify_product
from zk.groups import make_group
from zk.threshold_gadgets import (Shared, audit_product_partials,
                                  commitment_from_shares, joint_prove_bit)
from zk.threshold_quote import _share

PARTIES = [1, 2, 3, 4, 5, 6, 7]
T = 2
QUORUM = [1, 2, 3]


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:quote:v1")


def shared_bit(key, bit):
    group = key.group
    blinding = group.random_scalar()
    value = _share(group, bit, PARTIES, T)
    blinds = _share(group, blinding, PARTIES, T)
    cross = _share(group, (blinding * (1 - bit)) % group.order, PARTIES, T)
    return Shared(key.commit(bit, blinding), value, blinds), cross


def per_node(key, shared, cross):
    """What a verifier derives about each node without holding a share.

    Modelled here as a commitment to the node's own share; a deployment reads it
    off the published coefficient ladder, which is the same point.
    """
    group = key.group
    shares = {p: key.commit(shared.value[p], shared.blinding[p]) for p in QUORUM}
    crosses = {p: group.mul(group.point_pow(shared.commitment, shared.value[p]),
                            group.point_pow(key.h, cross[p])) for p in QUORUM}
    return shares, crosses


def test_an_honest_quorum_names_nobody(key):
    for bit in (0, 1):
        shared, cross = shared_bit(key, bit)
        record = []
        proof = joint_prove_bit(key, shared, cross, QUORUM, T, b"ctx", record)
        assert verify_product(key, shared.commitment, shared.commitment,
                              shared.commitment, proof, b"ctx")
        shares, crosses = per_node(key, shared, cross)
        assert audit_product_partials(key, record[0], shares, crosses) == [], bit


def test_the_node_that_answered_on_a_different_share_is_named(key):
    shared, cross = shared_bit(key, 1)
    record = []
    joint_prove_bit(key, shared, cross, QUORUM, T, b"ctx", record)
    entry = dict(record[0])
    # party 2 answers on something other than the share it published
    answers = dict(entry["answers"])
    z_b, z_rb, z_s = answers[2]
    answers[2] = ((z_b + 1) % key.group.order, z_rb, z_s)
    entry["answers"] = answers
    shares, crosses = per_node(key, shared, cross)
    assert audit_product_partials(key, entry, shares, crosses) == [2]


def test_a_bad_partial_breaks_the_proof_as_well_as_being_named(key):
    """Attribution does not replace the proof failing; both have to happen.

    A node that answers on the wrong share should produce a proof that does not
    verify *and* be nameable. If only the first held, the venue would know it had
    been cheated and not by whom; if only the second, naming would be a claim
    about a proof that was fine.
    """
    from zk.commit import ProductProof
    from zk.threshold_gadgets import combine_commitments
    from zk.threshold_sigma import lagrange_at_zero

    order = key.group.order
    shared, cross = shared_bit(key, 1)
    record = []
    joint_prove_bit(key, shared, cross, QUORUM, T, b"ctx", record)
    entry = record[0]

    # party 3 answers on something other than the share it published, and the
    # record says so, because the record is what the assembly actually saw
    z_b, z_rb, z_s = entry["answers"][3]
    entry["answers"][3] = ((z_b + 5) % order, z_rb, z_s)

    coefficients = lagrange_at_zero(sorted(entry["answers"]), order)
    z = [sum(coefficients[p] * entry["answers"][p][k] for p in entry["answers"])
         % order for k in range(3)]
    assembled = ProductProof(combine_commitments(key, entry["factor_parts"]),
                             combine_commitments(key, entry["product_parts"]), *z)

    assert not verify_product(key, shared.commitment, shared.commitment,
                              shared.commitment, assembled, b"ctx"), \
        "a proof built on a bad partial verified"
    shares, crosses = per_node(key, shared, cross)
    assert audit_product_partials(key, entry, shares, crosses) == [3]
