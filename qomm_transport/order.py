"""Content-independent admission and ordering for one fixed market slot.

The payload cannot decide priority.  A KYB admission authority issues exactly
one opaque ticket to each legal entity before the slot's randomness exists.
After the deadline, an independently signed beacon permutes those tickets.  A
ticket holder receives a signed admission receipt, so omission is challengeable
without disclosing the request carried by the frame.

Every admitted entity is expected to send one frame in every slot, including a
cover frame when it has no request.  Consequently the public manifest has a
fixed number of entries and does not reveal how many requests were real.
"""

from __future__ import annotations

import hashlib
import hmac
import time
from dataclasses import dataclass, field
from typing import Callable, Iterable

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

from .wire import Frame

TICKET_DOMAIN = b"QOMM:ORDER:TICKET:v1"
RECEIPT_DOMAIN = b"QOMM:ORDER:RECEIPT:v1"
BEACON_DOMAIN = b"QOMM:ORDER:BEACON:v1"
MANIFEST_DOMAIN = b"QOMM:ORDER:MANIFEST:v1"
ORDER_DOMAIN = b"QOMM:ORDER:KEY:v1"
ZERO = b"\x00" * 32


def _u64(value: int) -> bytes:
    if not 0 <= value < 1 << 64:
        raise ValueError("integer is outside the unsigned 64-bit range")
    return value.to_bytes(8, "big")


def _digest(*parts: bytes) -> bytes:
    h = hashlib.sha256()
    for part in parts:
        h.update(len(part).to_bytes(4, "big"))
        h.update(part)
    return h.digest()


def _verify(key: Ed25519PublicKey, signature: bytes, body: bytes) -> bool:
    try:
        key.verify(signature, body)
        return True
    except InvalidSignature:
        return False


@dataclass(frozen=True)
class AdmissionTicket:
    slot: int
    ticket_id: bytes
    issued_at: int
    expires_at: int
    signature: bytes

    def unsigned(self) -> bytes:
        if len(self.ticket_id) != 32:
            raise ValueError("ticket_id must be 32 bytes")
        if self.expires_at <= self.issued_at:
            raise ValueError("ticket expiry must follow issuance")
        return (TICKET_DOMAIN + _u64(self.slot) + self.ticket_id
                + _u64(self.issued_at) + _u64(self.expires_at))

    @property
    def digest(self) -> bytes:
        return _digest(self.unsigned(), self.signature)

    def verify(self, authority: Ed25519PublicKey, now: int) -> bool:
        return (self.issued_at <= now <= self.expires_at
                and _verify(authority, self.signature, self.unsigned()))


class AdmissionAuthority:
    """Issues one unlinkable ticket per verified entity and slot.

    ``entity_id`` never appears in the ticket.  Its HMAC nullifier is retained
    only to enforce the one-entity/one-ticket rule.  Production deployments must
    persist ``_issued`` transactionally; :mod:`qomm_transport.key_management`
    provides the encrypted store used by the command-line service.
    """

    def __init__(self, signing_key: Ed25519PrivateKey, entity_key: bytes):
        if len(entity_key) < 32:
            raise ValueError("entity nullifier key must contain at least 32 bytes")
        self.signing_key = signing_key
        self.entity_key = entity_key
        self._issued: dict[tuple[int, bytes], bytes] = {}

    @property
    def verifying_key(self) -> Ed25519PublicKey:
        return self.signing_key.public_key()

    def _entity_nullifier(self, entity_id: bytes) -> bytes:
        return hmac.new(self.entity_key, b"QOMM:ENTITY:v1" + entity_id,
                        hashlib.sha256).digest()

    def issue(self, entity_id: bytes, slot: int, *, issued_at: int | None = None,
              lifetime: int = 300, ticket_id: bytes | None = None) -> AdmissionTicket:
        if not entity_id:
            raise ValueError("an empty legal-entity identifier is not admissible")
        if lifetime <= 0:
            raise ValueError("ticket lifetime must be positive")
        issued_at = int(time.time()) if issued_at is None else issued_at
        nullifier = self._entity_nullifier(entity_id)
        key = (slot, nullifier)
        if key in self._issued:
            raise ValueError("this legal entity already has a ticket for the slot")
        if ticket_id is None:
            import os
            ticket_id = os.urandom(32)
        unsigned = (TICKET_DOMAIN + _u64(slot) + ticket_id
                    + _u64(issued_at) + _u64(issued_at + lifetime))
        ticket = AdmissionTicket(slot, ticket_id, issued_at, issued_at + lifetime,
                                 self.signing_key.sign(unsigned))
        self._issued[key] = ticket.digest
        return ticket


@dataclass(frozen=True)
class RandomnessBeacon:
    round: int
    value: bytes
    signature: bytes

    def unsigned(self) -> bytes:
        if len(self.value) != 32:
            raise ValueError("beacon value must be 32 bytes")
        return BEACON_DOMAIN + _u64(self.round) + self.value

    def verify(self, key: Ed25519PublicKey) -> bool:
        return _verify(key, self.signature, self.unsigned())

    @classmethod
    def sign(cls, round_: int, value: bytes,
             key: Ed25519PrivateKey) -> "RandomnessBeacon":
        unsigned = BEACON_DOMAIN + _u64(round_) + value
        return cls(round_, value, key.sign(unsigned))


@dataclass(frozen=True)
class AdmissionReceipt:
    slot: int
    node: int
    ticket_digest: bytes
    frame_digest: bytes
    received_at_ns: int
    signature: bytes

    def unsigned(self) -> bytes:
        if len(self.ticket_digest) != 32 or len(self.frame_digest) != 32:
            raise ValueError("receipt digests must be 32 bytes")
        return (RECEIPT_DOMAIN + _u64(self.slot) + _u64(self.node)
                + self.ticket_digest + self.frame_digest + _u64(self.received_at_ns))

    def verify(self, sealer_key: Ed25519PublicKey) -> bool:
        return _verify(sealer_key, self.signature, self.unsigned())


@dataclass(frozen=True)
class BatchManifest:
    slot: int
    node: int
    beacon_round: int
    beacon_value: bytes
    ordered_ticket_digests: tuple[bytes, ...]
    ordered_frame_digests: tuple[bytes, ...]
    previous_digest: bytes
    signature: bytes

    def unsigned(self) -> bytes:
        if len(self.beacon_value) != 32 or len(self.previous_digest) != 32:
            raise ValueError("manifest digests must be 32 bytes")
        if len(self.ordered_ticket_digests) != len(self.ordered_frame_digests):
            raise ValueError("ticket and frame manifests have different lengths")
        body = (MANIFEST_DOMAIN + _u64(self.slot) + _u64(self.node)
                + _u64(self.beacon_round) + self.beacon_value
                + self.previous_digest + _u64(len(self.ordered_ticket_digests)))
        for ticket, frame in zip(self.ordered_ticket_digests,
                                 self.ordered_frame_digests, strict=True):
            if len(ticket) != 32 or len(frame) != 32:
                raise ValueError("manifest entry digests must be 32 bytes")
            body += ticket + frame
        return body

    @property
    def digest(self) -> bytes:
        return _digest(self.unsigned(), self.signature)

    def verify(self, sealer_key: Ed25519PublicKey) -> bool:
        return _verify(sealer_key, self.signature, self.unsigned())

    def includes(self, receipt: AdmissionReceipt) -> bool:
        return any(t == receipt.ticket_digest and f == receipt.frame_digest
                   for t, f in zip(self.ordered_ticket_digests,
                                   self.ordered_frame_digests, strict=True))


@dataclass
class FixedSlotSealer:
    """Collects one frame per pre-issued ticket and closes exactly once."""

    slot: int
    node: int
    deadline_ns: int
    tickets: tuple[AdmissionTicket, ...]
    authority_key: Ed25519PublicKey
    beacon_key: Ed25519PublicKey
    signing_key: Ed25519PrivateKey
    previous_digest: bytes = ZERO
    _frames: dict[bytes, Frame] = field(default_factory=dict, init=False)
    _receipts: dict[bytes, AdmissionReceipt] = field(default_factory=dict, init=False)
    _closed: bool = field(default=False, init=False)

    def __post_init__(self) -> None:
        if len(self.previous_digest) != 32:
            raise ValueError("previous manifest digest must be 32 bytes")
        if len({ticket.ticket_id for ticket in self.tickets}) != len(self.tickets):
            raise ValueError("the expected ticket list contains a duplicate")
        for ticket in self.tickets:
            if ticket.slot != self.slot:
                raise ValueError("a ticket belongs to another slot")

    @property
    def verifying_key(self) -> Ed25519PublicKey:
        return self.signing_key.public_key()

    def admit(self, ticket: AdmissionTicket, frame: Frame, *, now_ns: int) -> AdmissionReceipt:
        if self._closed:
            raise RuntimeError("the slot is already closed")
        if now_ns > self.deadline_ns:
            raise TimeoutError("the frame arrived after the sealed deadline")
        now = now_ns // 1_000_000_000
        if not ticket.verify(self.authority_key, now):
            raise ValueError("the admission ticket is invalid or expired")
        expected = {item.digest for item in self.tickets}
        if ticket.digest not in expected:
            raise ValueError("the ticket was not in the slot's precommitted population")
        if frame.slot != self.slot or frame.node != self.node:
            raise ValueError("the frame belongs to another slot or node")
        raw = frame.encode()
        frame_digest = hashlib.sha256(raw).digest()
        prior = self._frames.get(ticket.digest)
        if prior is not None:
            if prior.encode() != raw:
                raise ValueError("one ticket attempted to replace its admitted frame")
            return self._receipts[ticket.digest]
        unsigned = (RECEIPT_DOMAIN + _u64(self.slot) + _u64(self.node)
                    + ticket.digest + frame_digest + _u64(now_ns))
        receipt = AdmissionReceipt(self.slot, self.node, ticket.digest,
                                   frame_digest, now_ns,
                                   self.signing_key.sign(unsigned))
        self._frames[ticket.digest] = frame
        self._receipts[ticket.digest] = receipt
        return receipt

    def close(self, beacon: RandomnessBeacon, *, now_ns: int
              ) -> tuple[list[Frame], BatchManifest]:
        if self._closed:
            raise RuntimeError("the slot is already closed")
        if now_ns <= self.deadline_ns:
            raise TimeoutError("the batch cannot close before its deadline")
        if not beacon.verify(self.beacon_key):
            raise ValueError("the ordering beacon signature is invalid")
        if beacon.round <= self.slot:
            raise ValueError("ordering randomness must be generated after the slot")
        missing = [ticket.digest for ticket in self.tickets
                   if ticket.digest not in self._frames]
        if missing:
            raise RuntimeError(
                f"fixed population incomplete: {len(missing)} cover or request frame(s) missing")
        ordered = sorted(
            self.tickets,
            key=lambda ticket: hashlib.sha256(
                ORDER_DOMAIN + _u64(self.slot) + _u64(beacon.round)
                + beacon.value + ticket.ticket_id).digest(),
        )
        frames = [self._frames[ticket.digest] for ticket in ordered]
        ticket_digests = tuple(ticket.digest for ticket in ordered)
        frame_digests = tuple(hashlib.sha256(frame.encode()).digest() for frame in frames)
        unsigned = (MANIFEST_DOMAIN + _u64(self.slot) + _u64(self.node)
                    + _u64(beacon.round) + beacon.value + self.previous_digest
                    + _u64(len(ticket_digests))
                    + b"".join(t + f for t, f in zip(ticket_digests, frame_digests,
                                                       strict=True)))
        manifest = BatchManifest(
            self.slot, self.node, beacon.round, beacon.value,
            ticket_digests, frame_digests, self.previous_digest,
            self.signing_key.sign(unsigned),
        )
        self._closed = True
        return frames, manifest


def prove_omission(receipt: AdmissionReceipt, manifest: BatchManifest,
                   sealer_key: Ed25519PublicKey) -> bool:
    """A public, payload-free omission challenge.

    ``True`` means the sealer signed both an admission receipt and a closed
    manifest that omitted that exact ticket/frame pair.
    """

    return (receipt.slot == manifest.slot and receipt.node == manifest.node
            and receipt.verify(sealer_key) and manifest.verify(sealer_key)
            and not manifest.includes(receipt))
