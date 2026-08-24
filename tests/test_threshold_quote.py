"""The whole quote proof, assembled by a quorum that holds no witness.

The paper's contribution said "a publicly verifiable proof that the opened quote
is correct, assembled by a quorum of computing nodes from shares". What was
assembled that way was one Pedersen opening; the prover took every maker's
policy in one call, in one process, and that process saw every pricing rule in
the market. This file is the test of the claim as written.

What matters here is not that a proof verifies --- a prover holding everything
already produced one. It is that *nothing in the assembly ever holds a wire*,
and that the proof it produces still refuses the things the original refused.
"""

import pytest

from zk.commit import Pedersen
from zk.groups import make_group
from zk.quote_proof import FIELDS, MakerWitness, QuoteProver, QuoteVerifier
from zk.threshold_quote import deal_quote_shares, joint_prove_quote
from zk.threshold_sigma import lagrange_at_zero

PARTIES = [1, 2, 3, 4, 5, 6, 7]
T = 2
QUORUM = [1, 2, 3]
NOW, SENTINEL, SLOTS, QTY = 1_000, 1 << 20, 8, 100


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:policy:v1")


def witness(key, *, half=20, slope=1, maxqty=500, expiry=2_000, active=1):
    return MakerWitness(mid=10_000, half=half, slope=slope, invcoef=1, inv=3,
                        maxqty=maxqty, expiry=expiry, active=active,
                        blindings={f: key.random_blinding() for f in FIELDS})


def assemble(key, makers, quorum=QUORUM, **over):
    settings = dict(qty=QTY, direction=0, now=NOW, sentinel=SENTINEL,
                    n_slots=SLOTS)
    settings.update(over)
    shares = deal_quote_shares(key, makers, parties=PARTIES, threshold=T,
                               **settings)
    proof, public = joint_prove_quote(
        key, shares, makers, quorum, now=settings["now"],
        sentinel=settings["sentinel"], n_slots=settings["n_slots"],
        direction=settings["direction"])
    return shares, proof, public


# --- the claim --------------------------------------------------------------

def test_a_quorum_assembles_the_whole_proof_and_it_verifies(key):
    makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
    _, proof, public = assemble(key, makers)
    ok, why = QuoteVerifier(key.group, key, assembled=True).verify(proof, public)
    assert ok, why
    assert public["assembled_by"] == QUORUM


def test_no_node_holds_any_wire(key):
    """The property the whole construction exists for.

    Checked over every wire the assembly touches, not only the interesting ones:
    a single wire held in the clear by one node is a maker's policy leaked to
    whoever runs it.
    """
    order = key.group.order
    makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
    shares, _, _ = assemble(key, makers)

    def reconstruct(mapping, subset):
        coefficients = lagrange_at_zero(subset, order)
        return sum(coefficients[p] * mapping[p] for p in subset) % order

    checked = 0
    for index, maker in enumerate(shares.makers):
        wires = [maker.fields[name] for name in FIELDS]
        wires += [maker.depth, maker.skew, maker.both, maker.ok, maker.gated,
                  maker.cost, maker.shifted_cost, maker.packed,
                  maker.fits.holds, maker.fits.product, maker.fits.witness,
                  maker.fresh.holds, maker.fresh.product, maker.fresh.witness]
        for wire in wires:
            secret = reconstruct(wire.value, QUORUM)
            for party in PARTIES:
                # A share equal to the secret is a share that leaks it. Zero and
                # one are exempt only because a bit's share can coincide by
                # chance without meaning anything; every other wire must differ.
                if secret > 1:
                    assert wire.value[party] != secret, (
                        f"maker {index}: party {party} holds a wire in the clear")
            # T of them cannot reconstruct it; T+1 can. That is the threshold.
            assert reconstruct(wire.value, QUORUM[:T]) != secret or secret == 0
            checked += 1
    assert checked >= 3 * 22, f"only {checked} wires checked"


def test_fewer_than_a_quorum_cannot_assemble(key):
    makers = [witness(key, half=20 + i) for i in range(2)]
    _, proof, public = assemble(key, makers, quorum=[1, 2])
    ok, _ = QuoteVerifier(key.group, key, assembled=True).verify(proof, public)
    assert not ok, "two of seven assembled a proof at a threshold of two"


def test_any_quorum_of_t_plus_one_assembles(key):
    makers = [witness(key, half=20 + i) for i in range(2)]
    for quorum in ([1, 2, 3], [5, 6, 7], [2, 4, 6], [1, 3, 5, 7]):
        _, proof, public = assemble(key, makers, quorum=quorum)
        ok, why = QuoteVerifier(key.group, key, assembled=True).verify(proof, public)
        assert ok, f"{quorum}: {why}"


# --- what it still refuses --------------------------------------------------

def test_the_published_winner_cannot_be_moved(key):
    makers = [witness(key, half=20 + i, slope=1 + i) for i in range(3)]
    _, proof, public = assemble(key, makers)
    moved = type(proof)(proof.winner_index, proof.winner_value + 1,
                        proof.maker_proofs, proof.winner_opening,
                        proof.minimality, proof.key_commitments, proof.range_bits)
    ok, why = QuoteVerifier(key.group, key, assembled=True).verify(moved, public)
    assert not ok and "opens to" in why, why


def test_a_proof_about_another_register_is_refused(key):
    makers = [witness(key, half=20 + i) for i in range(2)]
    _, proof, public = assemble(key, makers)
    others = [witness(key, half=99) for _ in range(2)]
    swapped = dict(public, registry=[m.registered(key) for m in others])
    ok, why = QuoteVerifier(key.group, key, assembled=True).verify(proof, swapped)
    assert not ok, why


def test_an_ineligible_maker_is_representable(key):
    """The original prover proves ineligibility rather than omitting it, and so
    must this one --- otherwise the only way to quote is to leave a maker out."""
    makers = [witness(key, half=20), witness(key, maxqty=QTY - 1),
              witness(key, expiry=NOW - 1), witness(key, active=0)]
    _, proof, public = assemble(key, makers)
    ok, why = QuoteVerifier(key.group, key, assembled=True).verify(proof, public)
    assert ok, why
    assert proof.winner_index == 0, "an ineligible maker won"


# --- the substitution is real and is not hidden -----------------------------

def test_an_assembled_proof_does_not_pass_the_ordinary_verifier(key):
    """Not a defect --- a fact worth pinning.

    A disjunction picks its simulated branch from the bit, so an assembled proof
    shows `b*b = b` instead. Same statement, different object, and the verifier
    has to be told which. A silent acceptance either way would mean one of the
    two checks was not checking.
    """
    makers = [witness(key, half=20 + i) for i in range(2)]
    _, proof, public = assemble(key, makers)
    ok, _ = QuoteVerifier(key.group, key, assembled=False).verify(proof, public)
    assert not ok, "the ordinary verifier accepted a square proof as a disjunction"


def test_the_local_prover_still_fails_the_assembled_verifier(key):
    makers = [witness(key, half=20 + i) for i in range(2)]
    prover = QuoteProver(key.group, key)
    proof, public = prover.prove(makers, qty=QTY, direction=0, now=NOW,
                                 sentinel=SENTINEL, n_slots=SLOTS)
    ok, _ = QuoteVerifier(key.group, key, assembled=False).verify(proof, public)
    assert ok, "the local path stopped working"
    ok, _ = QuoteVerifier(key.group, key, assembled=True).verify(proof, public)
    assert not ok, "the assembled verifier accepted a disjunction as a square"
