"""Signed per-slot receipts and the evidence that convicts a misbehaving node.

The design follows the three kinds of audit the Issue separates.

    computation      every slot carries a result digest that a proof commits to
    equivocation     two signatures by one node for one slot, over different
                     digests, are self-contained evidence anyone can check
    unresponsiveness  a slot deadline passes with fewer receipts than required

Nothing here reveals whether a slot carried a real request. A receipt is emitted
for every slot on the fixed schedule, and its contents are digests, so a dummy
slot and a real slot are indistinguishable from the receipt alone. That is the
property the Issue calls query-oblivious auditability, and it is why the
liveness rule can be "a receipt from every node, every slot": without it,
"the node did not answer" is not something a third party can check.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from enum import Enum
from typing import Iterable, Mapping, Sequence

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey, Ed25519PublicKey,
)

DOMAIN = b"QOMM:AUDIT:v1"

# Bumped when `signed_body` changes shape, so a receipt signed under the old
# shape cannot be replayed as one under the new.
VERSION = b"2"
GENESIS = b"\x00" * 32


def digest(*parts) -> bytes:
    h = hashlib.sha256(DOMAIN)
    for part in parts:
        encoded = part if isinstance(part, (bytes, bytearray)) else json.dumps(
            part, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
        h.update(len(encoded).to_bytes(4, "big"))
        h.update(encoded)
    return h.digest()


@dataclass(frozen=True)
class SlotSpec:
    """What the venue fixes publicly *before* the slot runs.

    Fixing the eligible-maker set to one digest in advance is what makes
    omission detectable without hashing the set inside the circuit: a node that
    drops a maker has to sign a different digest, and that mismatch is visible.
    """

    slot: int
    mm_set_digest: bytes
    market_digest: bytes
    deadline: int
    required_receipts: int

    def binding(self) -> bytes:
        return digest(b"slot", self.slot.to_bytes(8, "big"),
                      self.mm_set_digest, self.market_digest)


@dataclass(frozen=True)
class NodeReceipt:
    node: int
    slot: int
    mm_set_digest: bytes
    market_digest: bytes
    deadline: int
    prev_state_digest: bytes
    new_state_digest: bytes
    result_digest: bytes
    emitted_at: int
    signature: bytes

    def signed_body(self) -> bytes:
        """Everything a reader of this receipt is asked to believe.

        `emitted_at` used to be outside this. It is the field the deadline test
        reads, so a receipt could be signed late and the timestamp rewritten
        afterwards with the signature still checking out --- and being late past
        the deadline is one of the faults this file exists to attribute. The
        market digest and the deadline were outside it too, so a node could sign
        for one slot's configuration and have it counted for another's.
        """
        return digest(b"receipt:v2", VERSION,
                      self.node.to_bytes(4, "big"),
                      self.slot.to_bytes(8, "big"),
                      self.mm_set_digest, self.market_digest,
                      self.deadline.to_bytes(8, "big"),
                      self.emitted_at.to_bytes(8, "big"),
                      self.prev_state_digest, self.new_state_digest,
                      self.result_digest)

    def content_digest(self) -> bytes:
        """What the node is committing to. Two receipts differing here equivocate."""
        return digest(b"content", self.mm_set_digest, self.prev_state_digest,
                      self.new_state_digest, self.result_digest)


def sign_receipt(key: Ed25519PrivateKey, node: int, spec: SlotSpec, *,
                 prev_state_digest: bytes, new_state_digest: bytes,
                 result_digest: bytes, emitted_at: int,
                 mm_set_digest: bytes | None = None) -> NodeReceipt:
    receipt = NodeReceipt(
        node=node, slot=spec.slot,
        mm_set_digest=mm_set_digest if mm_set_digest is not None else spec.mm_set_digest,
        market_digest=spec.market_digest, deadline=spec.deadline,
        prev_state_digest=prev_state_digest, new_state_digest=new_state_digest,
        result_digest=result_digest, emitted_at=emitted_at, signature=b"")
    return NodeReceipt(**{**receipt.__dict__, "signature": key.sign(receipt.signed_body())})


class Fault(str, Enum):
    EQUIVOCATION = "equivocation"          # two results signed for one slot
    OMITTED_MAKERS = "omitted_makers"      # signed a maker set other than the fixed one
    STALE_STATE = "stale_state"            # continued from a state that was superseded
    MISSING_RECEIPT = "missing_receipt"    # silent past the deadline
    BAD_SIGNATURE = "bad_signature"
    FORKED_STATE = "forked_state"          # nodes disagree on the new state


@dataclass(frozen=True)
class Evidence:
    fault: Fault
    node: int
    slot: int
    detail: str
    exhibits: tuple = field(default=())

    def as_dict(self) -> dict:
        return {"fault": self.fault.value, "node": self.node, "slot": self.slot,
                "detail": self.detail}


class AuditLedger:
    """Collects receipts, settles each slot at its deadline, and emits evidence."""

    def __init__(self, node_keys: Mapping[int, Ed25519PublicKey]):
        self.node_keys = dict(node_keys)
        self._by_slot: dict[int, dict[int, list[NodeReceipt]]] = {}
        self._specs: dict[int, SlotSpec] = {}
        # when this ledger saw each node's first receipt for a slot; the field
        # the node signs says when the node says it sent one
        self._arrived: dict[tuple[int, int], int] = {}
        self._settled_state: dict[int, bytes] = {}
        self.evidence: list[Evidence] = []

    def open_slot(self, spec: SlotSpec) -> None:
        self._specs[spec.slot] = spec
        self._by_slot.setdefault(spec.slot, {})

    def record(self, receipt: NodeReceipt, arrived_at: int | None = None
               ) -> list[Evidence]:
        """Accept a receipt. Equivocation is detectable the moment it arrives.

        `arrived_at` is when *this ledger* saw it, and it is what lateness is
        judged by. `emitted_at` is a field the node chooses and signs, so
        settling against that let a node miss a deadline, sign
        `emitted_at = deadline - 1` afterwards, and be counted as on time --- a
        deadline nobody but the node observes is not a deadline. Callers that
        pass nothing keep the old behaviour, which is only safe where the
        emitter and the ledger are the same process.
        """
        found: list[Evidence] = []
        spec = self._specs.get(receipt.slot)
        if spec is None:
            return [Evidence(Fault.MISSING_RECEIPT, receipt.node, receipt.slot,
                             "receipt for a slot that was never opened")]
        key = self.node_keys.get(receipt.node)
        if key is None:
            return [Evidence(Fault.BAD_SIGNATURE, receipt.node, receipt.slot, "unknown node")]
        try:
            key.verify(receipt.signature, receipt.signed_body())
        except (InvalidSignature, ValueError):
            found.append(Evidence(Fault.BAD_SIGNATURE, receipt.node, receipt.slot,
                                  "signature does not verify"))
            self.evidence.extend(found)
            return found

        # The slot fixes the market and the deadline, and both are signed --- and
        # neither was compared, so a receipt for another market with an invented
        # deadline still counted toward this slot's quorum.
        if receipt.market_digest != spec.market_digest:
            found.append(Evidence(
                Fault.STALE_STATE, receipt.node, receipt.slot,
                "signed a market other than the one fixed for this slot", (receipt,)))
        if receipt.deadline != spec.deadline:
            found.append(Evidence(
                Fault.STALE_STATE, receipt.node, receipt.slot,
                "signed a deadline other than the one fixed for this slot", (receipt,)))

        if arrived_at is not None:
            key_at = (receipt.slot, receipt.node)
            previous = self._arrived.get(key_at)
            # the first arrival is what counts: a node that answered on time and
            # then answered again has not become late
            self._arrived[key_at] = (arrived_at if previous is None
                                     else min(previous, arrived_at))

        existing = self._by_slot[receipt.slot].setdefault(receipt.node, [])
        for other in existing:
            if other.content_digest() != receipt.content_digest():
                found.append(Evidence(
                    Fault.EQUIVOCATION, receipt.node, receipt.slot,
                    "two signed results for one slot", (other, receipt)))
        existing.append(receipt)

        if receipt.mm_set_digest != spec.mm_set_digest:
            found.append(Evidence(
                Fault.OMITTED_MAKERS, receipt.node, receipt.slot,
                "signed a maker set other than the one fixed for this slot", (receipt,)))

        expected_prev = self._settled_state.get(receipt.slot - 1, GENESIS)
        if receipt.slot - 1 in self._settled_state and receipt.prev_state_digest != expected_prev:
            found.append(Evidence(
                Fault.STALE_STATE, receipt.node, receipt.slot,
                "continued from a state other than the settled predecessor", (receipt,)))

        self.evidence.extend(found)
        return found

    def settle(self, slot: int, now: int) -> tuple[bytes | None, list[Evidence]]:
        """Close a slot at its deadline and report who failed to answer."""
        spec = self._specs[slot]
        found: list[Evidence] = []
        receipts = self._by_slot.get(slot, {})

        for node in sorted(self.node_keys):
            seen_at = self._arrived.get((slot, node))
            fresh = [r for r in receipts.get(node, [])
                     if (seen_at if seen_at is not None else r.emitted_at)
                     <= spec.deadline]
            if not fresh:
                found.append(Evidence(
                    Fault.MISSING_RECEIPT, node, slot,
                    f"no receipt by the deadline ({spec.deadline}); observed at {now}"))

        # the majority state wins; nodes that signed something else have forked
        tally: dict[bytes, list[int]] = {}
        for node, node_receipts in receipts.items():
            for receipt in node_receipts:
                seen_at = self._arrived.get((slot, node))
                when = seen_at if seen_at is not None else receipt.emitted_at
                if (when <= spec.deadline
                        and receipt.mm_set_digest == spec.mm_set_digest
                        and receipt.market_digest == spec.market_digest
                        and receipt.deadline == spec.deadline):
                    tally.setdefault(receipt.new_state_digest, []).append(node)
        settled = None
        if tally:
            plurality = max(tally, key=lambda k: len(set(tally[k])))
            agreeing = len(set(tally[plurality]))
            for state, nodes in tally.items():
                if state != plurality:
                    for node in sorted(set(nodes)):
                        found.append(Evidence(
                            Fault.FORKED_STATE, node, slot,
                            "signed a new state that the quorum did not agree with"))
            if agreeing < spec.required_receipts:
                # A plurality is not a quorum. This used to record the shortfall
                # and then store the plurality state anyway, so a state no
                # quorum ever agreed to became the predecessor the next slot
                # continued from --- and every honest receipt after it was
                # scored stale against it. The slot stays unresolved instead.
                found.append(Evidence(
                    Fault.MISSING_RECEIPT, -1, slot,
                    f"only {agreeing} agreeing receipts, "
                    f"{spec.required_receipts} required; the slot is unresolved "
                    "and the state does not advance"))
            else:
                settled = plurality
                self._settled_state[slot] = settled

        self.evidence.extend(found)
        return settled, found

    def settled_state(self, slot: int) -> bytes:
        return self._settled_state.get(slot, GENESIS)


@dataclass
class BondLedger:
    """Bonds and the penalty schedule. Evidence in, slashing out."""

    bonds: dict[int, int]
    penalties: Mapping[Fault, int] = field(default_factory=lambda: {
        Fault.EQUIVOCATION: 1_000_000,     # provable double-signing: the worst case
        Fault.FORKED_STATE: 500_000,
        Fault.OMITTED_MAKERS: 500_000,
        Fault.STALE_STATE: 250_000,
        Fault.BAD_SIGNATURE: 250_000,
        Fault.MISSING_RECEIPT: 50_000,     # may be an honest outage, so it is cheap
    })
    slashed: list[dict] = field(default_factory=list)

    def apply(self, evidence: Iterable[Evidence]) -> list[dict]:
        applied = []
        for item in evidence:
            if item.node < 0 or item.node not in self.bonds:
                continue
            amount = min(self.bonds[item.node], self.penalties.get(item.fault, 0))
            if amount <= 0:
                continue
            self.bonds[item.node] -= amount
            record = {"node": item.node, "slot": item.slot,
                      "fault": item.fault.value, "amount": amount,
                      "remaining_bond": self.bonds[item.node]}
            self.slashed.append(record)
            applied.append(record)
        return applied
