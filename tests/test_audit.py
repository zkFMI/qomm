"""Negative controls for query-oblivious auditability.

Each audit mechanism has to convict the node that actually misbehaved and leave
the honest ones alone. The awkward case is that the audit must do this while
being unable to tell a real slot from a cover slot, so every test below feeds the
ledger digests only.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_audit.receipts import (                                    # noqa: E402
    GENESIS, AuditLedger, BondLedger, Fault, NodeReceipt, SlotSpec, digest, sign_receipt,
)

N_NODES = 7
QUORUM = 5
MAKERS = digest(b"makers", *[f"MM-{i}".encode() for i in range(16)])


@pytest.fixture()
def venue():
    keys = {node: Ed25519PrivateKey.generate() for node in range(N_NODES)}
    ledger = AuditLedger({node: key.public_key() for node, key in keys.items()})
    return keys, ledger


def _spec(slot: int) -> SlotSpec:
    return SlotSpec(slot=slot, mm_set_digest=MAKERS,
                    market_digest=digest(b"market", slot.to_bytes(4, "big")),
                    deadline=100 * slot + 50, required_receipts=QUORUM)


def _honest_round(keys, ledger, slot: int, prev: bytes, skip=(), makers=None,
                  parent_override=None):
    spec = _spec(slot)
    ledger.open_slot(spec)
    result = digest(b"result", slot.to_bytes(4, "big"))
    new_state = digest(b"state", prev, result)
    for node in range(N_NODES):
        if node in skip:
            continue
        ledger.record(sign_receipt(
            keys[node], node, spec,
            prev_state_digest=parent_override.get(node, prev) if parent_override else prev,
            new_state_digest=new_state, result_digest=result,
            emitted_at=100 * slot + 10,
            mm_set_digest=(makers or {}).get(node, MAKERS)))
    return spec, result, new_state


def test_an_honest_run_produces_no_evidence(venue):
    keys, ledger = venue
    prev = GENESIS
    for slot in range(4):
        _honest_round(keys, ledger, slot, prev)
        prev, found = ledger.settle(slot, now=100 * slot + 60)
        assert found == []
    assert ledger.evidence == []


def test_equivocation_is_self_contained_evidence(venue):
    keys, ledger = venue
    spec, result, new_state = _honest_round(keys, ledger, 0, GENESIS)
    other = digest(b"result-other")
    found = ledger.record(sign_receipt(
        keys[3], 3, spec, prev_state_digest=GENESIS,
        new_state_digest=digest(b"state", GENESIS, other), result_digest=other,
        emitted_at=11))
    faults = {e.fault for e in found}
    assert Fault.EQUIVOCATION in faults
    evidence = next(e for e in found if e.fault is Fault.EQUIVOCATION)
    assert evidence.node == 3 and len(evidence.exhibits) == 2
    # the two exhibits are signed by the same node for the same slot
    first, second = evidence.exhibits
    assert first.node == second.node == 3 and first.slot == second.slot
    assert first.content_digest() != second.content_digest()


def test_dropping_an_eligible_maker_is_caught(venue):
    keys, ledger = venue
    short_set = digest(b"makers", *[f"MM-{i}".encode() for i in range(15)])
    _honest_round(keys, ledger, 0, GENESIS, makers={2: short_set})
    assert any(e.fault is Fault.OMITTED_MAKERS and e.node == 2 for e in ledger.evidence)
    assert not any(e.node == 3 for e in ledger.evidence)


def test_reusing_a_superseded_state_is_caught(venue):
    keys, ledger = venue
    _honest_round(keys, ledger, 0, GENESIS)
    settled, _ = ledger.settle(0, now=60)
    ledger.evidence.clear()
    _honest_round(keys, ledger, 1, settled, parent_override={4: GENESIS})
    assert any(e.fault is Fault.STALE_STATE and e.node == 4 for e in ledger.evidence)


def test_a_silent_node_is_named_at_the_deadline(venue):
    keys, ledger = venue
    _honest_round(keys, ledger, 0, GENESIS, skip=(6,))
    _, found = ledger.settle(0, now=60)
    assert any(e.fault is Fault.MISSING_RECEIPT and e.node == 6 for e in found)
    assert not any(e.fault is Fault.MISSING_RECEIPT and e.node == 0 for e in found)


def test_a_late_receipt_does_not_count(venue):
    """Answering after the deadline is the same as not answering."""
    keys, ledger = venue
    spec, result, new_state = _honest_round(keys, ledger, 0, GENESIS, skip=(5,))
    ledger.record(sign_receipt(keys[5], 5, spec, prev_state_digest=GENESIS,
                               new_state_digest=new_state, result_digest=result,
                               emitted_at=spec.deadline + 1))
    _, found = ledger.settle(0, now=spec.deadline + 5)
    assert any(e.fault is Fault.MISSING_RECEIPT and e.node == 5 for e in found)


def test_a_slot_short_of_quorum_does_not_settle(venue):
    """A plurality is not a quorum, and this used to store it as one.

    The shortfall was recorded and the plurality state saved anyway, so a state
    no quorum ever agreed to became the predecessor the next slot continued
    from --- and honest receipts after it were scored stale against it.
    """
    keys, ledger = venue
    _honest_round(keys, ledger, 0, GENESIS, skip=(1, 2, 3))
    settled, found = ledger.settle(0, now=60)
    assert settled is None
    assert ledger.settled_state(0) == GENESIS, "the state advanced without a quorum"
    assert any(e.fault is Fault.MISSING_RECEIPT and e.node == -1 for e in found)


def test_a_slot_with_a_quorum_still_settles(venue):
    keys, ledger = venue
    _honest_round(keys, ledger, 0, GENESIS, skip=(6,))
    settled, found = ledger.settle(0, now=60)
    assert settled is not None
    assert ledger.settled_state(0) == settled
    assert not any(e.fault is Fault.MISSING_RECEIPT and e.node == -1 for e in found)


def test_the_timestamp_cannot_be_moved_after_signing(venue):
    """`emitted_at` decides lateness, so it has to be inside the signature."""
    keys, ledger = venue
    spec = _spec(0)
    ledger.open_slot(spec)
    late = sign_receipt(keys[0], 0, spec, prev_state_digest=GENESIS,
                        new_state_digest=digest(b"s"), result_digest=digest(b"r"),
                        emitted_at=spec.deadline + 100)
    backdated = NodeReceipt(**{**late.__dict__, "emitted_at": 1})
    found = ledger.record(backdated)
    assert any(e.fault is Fault.BAD_SIGNATURE for e in found), (
        "a receipt signed late was backdated and still verified")


def test_the_slot_configuration_cannot_be_moved_after_signing(venue):
    """A receipt signed for one market must not count for another."""
    keys, ledger = venue
    spec = _spec(0)
    ledger.open_slot(spec)
    genuine = sign_receipt(keys[0], 0, spec, prev_state_digest=GENESIS,
                           new_state_digest=digest(b"s"), result_digest=digest(b"r"),
                           emitted_at=10)
    for field, value in (("market_digest", digest(b"another market")),
                         ("deadline", spec.deadline + 10_000)):
        moved = NodeReceipt(**{**genuine.__dict__, field: value})
        found = ledger.record(moved)
        assert any(e.fault is Fault.BAD_SIGNATURE for e in found), (
            f"{field} was changed after signing and the receipt still verified")


def test_a_forged_signature_is_rejected(venue):
    keys, ledger = venue
    spec = _spec(0)
    ledger.open_slot(spec)
    genuine = sign_receipt(keys[0], 0, spec, prev_state_digest=GENESIS,
                           new_state_digest=digest(b"s"), result_digest=digest(b"r"),
                           emitted_at=10)
    forged = NodeReceipt(**{**genuine.__dict__, "node": 1})
    found = ledger.record(forged)
    assert any(e.fault is Fault.BAD_SIGNATURE and e.node == 1 for e in found)


def test_a_minority_state_forks_and_is_named(venue):
    keys, ledger = venue
    spec = _spec(0)
    ledger.open_slot(spec)
    result = digest(b"result")
    majority = digest(b"state", GENESIS, result)
    minority = digest(b"state-other")
    for node in range(N_NODES):
        state = minority if node == 2 else majority
        ledger.record(sign_receipt(keys[node], node, spec, prev_state_digest=GENESIS,
                                   new_state_digest=state, result_digest=result,
                                   emitted_at=10))
    settled, found = ledger.settle(0, now=60)
    assert settled == majority
    assert any(e.fault is Fault.FORKED_STATE and e.node == 2 for e in found)


def test_receipts_do_not_reveal_whether_a_slot_was_real(venue):
    """A cover slot and a real slot differ only in secret inputs, not in receipts."""
    keys, ledger = venue
    spec = _spec(0)
    ledger.open_slot(spec)
    real = sign_receipt(keys[0], 0, spec, prev_state_digest=GENESIS,
                        new_state_digest=digest(b"state-real"),
                        result_digest=digest(b"result-real"), emitted_at=10)
    cover = sign_receipt(keys[1], 1, spec, prev_state_digest=GENESIS,
                         new_state_digest=digest(b"state-cover"),
                         result_digest=digest(b"result-cover"), emitted_at=10)
    # same shape, same field sizes, no flag distinguishing them
    assert set(real.__dict__) == set(cover.__dict__)
    assert len(real.result_digest) == len(cover.result_digest) == 32
    assert len(real.signature) == len(cover.signature)


def test_slashing_follows_the_published_schedule():
    bonds = BondLedger({0: 900_000})
    from qomm_audit.receipts import Evidence

    applied = bonds.apply([Evidence(Fault.EQUIVOCATION, 0, 1, "double signed")])
    assert applied[0]["amount"] == 900_000        # capped by the bond
    assert bonds.bonds[0] == 0
    assert bonds.apply([Evidence(Fault.STALE_STATE, 0, 2, "stale")]) == []


def test_slashing_ignores_venue_level_findings():
    bonds = BondLedger({0: 100_000})
    from qomm_audit.receipts import Evidence

    assert bonds.apply([Evidence(Fault.MISSING_RECEIPT, -1, 0, "quorum short")]) == []
    assert bonds.bonds[0] == 100_000


# --- what a node signs about itself is not evidence --------------------------

def test_a_late_node_cannot_backdate_its_own_receipt():
    """`emitted_at` is a field the node chooses and signs.

    Settling against it let a node miss a deadline, sign
    `emitted_at = deadline - 1` afterwards, and be counted as on time. A
    deadline nobody but the node observes is not a deadline, so the ledger
    judges by when it saw the receipt.
    """
    keys = {node: Ed25519PrivateKey.generate() for node in range(3)}
    ledger = AuditLedger({n: k.public_key() for n, k in keys.items()})
    spec = SlotSpec(slot=1, market_digest=b"m" * 32, deadline=100,
                    mm_set_digest=b"s" * 32, required_receipts=3)
    ledger.open_slot(spec)

    for node in (0, 1):
        ledger.record(sign_receipt(keys[node], node, spec,
                                   prev_state_digest=GENESIS,
                                   new_state_digest=b"n" * 32,
                                   result_digest=b"r" * 32, emitted_at=50),
                      arrived_at=50)
    # node 2 answers late and says it did not
    ledger.record(sign_receipt(keys[2], 2, spec, prev_state_digest=GENESIS,
                               new_state_digest=b"n" * 32,
                               result_digest=b"r" * 32, emitted_at=99),
                  arrived_at=150)

    _, evidence = ledger.settle(1, now=150)
    named = {e.node for e in evidence if e.fault is Fault.MISSING_RECEIPT}
    assert 2 in named, "the backdated receipt was accepted as on time"
    assert 0 not in named and 1 not in named, "an on-time node was named"
    # and the slot fails its quorum, which is the consequence of the above
    assert -1 in named, "two of three receipts should not settle a slot of three"


def test_a_receipt_for_another_market_does_not_count():
    """The slot fixes the market and the deadline, both are signed, and
    neither was compared --- so a receipt for another market with an invented
    deadline still counted toward this slot's quorum."""
    keys = {node: Ed25519PrivateKey.generate() for node in range(3)}
    ledger = AuditLedger({n: k.public_key() for n, k in keys.items()})
    spec = SlotSpec(slot=1, market_digest=b"m" * 32, deadline=100,
                    mm_set_digest=b"s" * 32, required_receipts=3)
    ledger.open_slot(spec)
    elsewhere = SlotSpec(slot=1, market_digest=b"OTHER MARKET".ljust(32, b"."),
                         deadline=100, mm_set_digest=b"s" * 32,
                         required_receipts=3)

    evidence = ledger.record(
        sign_receipt(keys[0], 0, elsewhere, prev_state_digest=GENESIS,
                     new_state_digest=b"n" * 32, result_digest=b"r" * 32,
                     emitted_at=50), arrived_at=50)
    assert any("market other than" in e.detail for e in evidence), \
        "a receipt for another market was accepted without comment"
