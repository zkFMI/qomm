"""The chain from the registered policy to the published key, checked.

The quote proof establishes that the opened winner is the minimum of keys, and
each key is the maker's policy applied to the request. The verifier checked the
second half in pieces --- depth is slope times quantity, eligibility is the
conjunction of three tests, the gated cost is the cost gated by eligibility ---
and never checked that the *cost* was those pieces, or that the *key* was that
cost. So the pieces were proved and then a different number was minimised.

Two consequences, both demonstrated below and both fixed. A cost commitment
could be replaced outright. And a proof built for one direction verified when
published as the other, naming a maker that does not win in that direction ---
which is the statement "a wrong winner is not provable" failing exactly.

The shape tests are the same failure seen from further away: a proof with no
makers, with the minimality proofs deleted, or with a winner index Python is
happy to read backwards, each says nothing and was accepted as saying something.
"""

import pytest

from zk.commit import Pedersen, prove_opening
from zk.groups import make_group
from zk.quote_proof import (FIELDS, MakerWitness, QuoteProof, QuoteProver,
                            QuoteVerifier, registry_digest, shift_commitment)

SETTINGS = dict(qty=100, now=1_000, sentinel=1 << 20, n_slots=8)


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:policy:v1")


def witness(key, **over):
    fields = dict(mid=10_000, half=20, slope=1, invcoef=1, inv=3, maxqty=500,
                  expiry=2_000, active=1)
    fields.update(over)
    return MakerWitness(**fields,
                        blindings={f: key.random_blinding() for f in FIELDS})


def proved(key, makers, direction=0):
    prover = QuoteProver(key.group, key)
    return prover.prove(makers, direction=direction, **SETTINGS)


def rebuilt(proof, **over):
    fields = dict(winner_index=proof.winner_index, winner_value=proof.winner_value,
                  maker_proofs=proof.maker_proofs,
                  winner_opening=proof.winner_opening,
                  minimality=proof.minimality,
                  key_commitments=proof.key_commitments,
                  range_bits=proof.range_bits)
    fields.update(over)
    return QuoteProof(**fields)


# --- the chain --------------------------------------------------------------

def test_a_proof_for_one_direction_does_not_verify_as_the_other(key):
    """The one that matters: it names a winner that is not the winner.

    Two makers whose order reverses between buying and selling. The buy proof,
    republished as a sell, used to verify --- and it says maker 1 won, when
    selling maker 0 wins.
    """
    makers = [witness(key, half=5, mid=10_000), witness(key, half=60, mid=9_900)]
    buy, public = proved(key, makers, direction=0)
    sell, _ = proved(key, makers, direction=1)
    assert buy.winner_index != sell.winner_index, "the fixture does not separate them"

    ok, why = QuoteVerifier(key.group, key).verify(buy, dict(public, direction=1))
    assert not ok, (f"a buy proof verified as a sell, naming maker "
                    f"{buy.winner_index} where {sell.winner_index} wins")
    assert "direction" in why, why


def test_the_cost_commitment_cannot_be_replaced(key):
    makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
    proof, public = proved(key, makers)
    swapped = list(proof.maker_proofs)
    commitments = dict(swapped[0].commitments)
    commitments["cost"] = key.commit(1, key.random_blinding())
    swapped[0] = type(swapped[0])(**{**swapped[0].__dict__,
                                     "commitments": commitments})
    ok, why = QuoteVerifier(key.group, key).verify(
        rebuilt(proof, maker_proofs=tuple(swapped)), public)
    assert not ok and "cost" in why, why


def test_the_key_has_to_be_this_makers_gated_cost(key):
    makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
    proof, public = proved(key, makers)
    keys = list(proof.key_commitments)
    keys[1] = key.commit(7, key.random_blinding())
    ok, why = QuoteVerifier(key.group, key).verify(
        rebuilt(proof, key_commitments=tuple(keys)), public)
    assert not ok and "key" in why, why


def test_the_baseline_still_verifies(key):
    for direction in (0, 1):
        makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
        proof, public = proved(key, makers, direction=direction)
        ok, why = QuoteVerifier(key.group, key).verify(proof, public)
        assert ok, why


# --- the shapes -------------------------------------------------------------

def test_a_proof_about_no_makers_is_not_a_proof(key):
    blinding = key.random_blinding()
    commitment = key.commit(0, blinding)
    empty = QuoteProof(0, 0, (), prove_opening(
        key, shift_commitment(key, commitment, 0), 0, blinding, b":winner"),
        (), (commitment,), 24)
    public = {"qty_commitment": commitment, "now": 0, "sentinel": 1,
              "n_slots": 1, "direction": 0, "registry": [],
              "registry_digest": registry_digest(key.group, [])}
    ok, why = QuoteVerifier(key.group, key).verify(empty, public)
    assert not ok, why


def test_minimality_cannot_be_deleted(key):
    makers = [witness(key, half=20 + i) for i in range(3)]
    proof, public = proved(key, makers)
    ok, why = QuoteVerifier(key.group, key).verify(rebuilt(proof, minimality=()),
                                                   public)
    assert not ok and "minimality" in why, why


def test_a_winner_index_is_a_maker_and_not_a_python_index(key):
    makers = [witness(key, half=20 + i) for i in range(3)]
    proof, public = proved(key, makers)
    verifier = QuoteVerifier(key.group, key)
    for index in (-3, -1, 3, 99):
        ok, why = verifier.verify(rebuilt(proof, winner_index=index), public)
        assert not ok, f"index {index} was accepted"
        assert "not a maker" in why, why
