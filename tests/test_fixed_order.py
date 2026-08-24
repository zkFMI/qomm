import hashlib

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from qomm_transport.order import (AdmissionAuthority, BatchManifest,
                                  FixedSlotSealer, RandomnessBeacon,
                                  prove_omission)
from qomm_transport.wire import Frame, PAYLOAD_BYTES, frame_mac


def frame(slot, node, marker, key=b"k" * 32):
    payload = bytes([marker]) + bytes(PAYLOAD_BYTES - 1)
    return Frame(slot, node, payload, frame_mac(key, slot, node, payload))


def setup(population=4):
    authority_key = Ed25519PrivateKey.generate()
    authority = AdmissionAuthority(authority_key, b"e" * 32)
    tickets = tuple(authority.issue(f"lei-{i}".encode(), 100, issued_at=10,
                                    lifetime=1000,
                                    ticket_id=bytes([i + 1]) * 32)
                    for i in range(population))
    beacon_key = Ed25519PrivateKey.generate()
    sealer_key = Ed25519PrivateKey.generate()
    sealer = FixedSlotSealer(100, 2, 20_000_000_000, tickets,
                             authority.verifying_key, beacon_key.public_key(),
                             sealer_key)
    return authority, tickets, beacon_key, sealer


def test_priority_depends_on_preissued_ticket_and_future_beacon_not_payload():
    _, tickets, beacon_key, sealer_a = setup()
    _, tickets_b, _, _ = setup()
    # Rebuild a second sealer over the exact same signed tickets.
    sealer_b = FixedSlotSealer(100, 2, 20_000_000_000, tickets,
                               sealer_a.authority_key, beacon_key.public_key(),
                               Ed25519PrivateKey.generate())
    for i, ticket in enumerate(tickets):
        sealer_a.admit(ticket, frame(100, 2, i), now_ns=15_000_000_000)
        sealer_b.admit(ticket, frame(100, 2, 200 - i), now_ns=15_000_000_000)
    beacon = RandomnessBeacon.sign(101, b"b" * 32, beacon_key)
    _, a = sealer_a.close(beacon, now_ns=21_000_000_000)
    _, b = sealer_b.close(beacon, now_ns=21_000_000_000)
    assert a.ordered_ticket_digests == b.ordered_ticket_digests
    assert a.ordered_frame_digests != b.ordered_frame_digests


def test_one_legal_entity_cannot_obtain_two_tickets_for_a_slot():
    key = Ed25519PrivateKey.generate()
    authority = AdmissionAuthority(key, b"e" * 32)
    authority.issue(b"LEI", 7, issued_at=1)
    with pytest.raises(ValueError, match="already"):
        authority.issue(b"LEI", 7, issued_at=1)


def test_close_refuses_a_missing_cover_frame_and_late_replacement():
    _, tickets, beacon_key, sealer = setup(2)
    receipt = sealer.admit(tickets[0], frame(100, 2, 1), now_ns=15_000_000_000)
    with pytest.raises(ValueError, match="replace"):
        sealer.admit(tickets[0], frame(100, 2, 2), now_ns=15_000_000_001)
    with pytest.raises(RuntimeError, match="incomplete"):
        sealer.close(RandomnessBeacon.sign(101, b"b" * 32, beacon_key),
                     now_ns=21_000_000_000)
    assert receipt.verify(sealer.verifying_key)


def test_receipt_proves_omission_without_revealing_the_payload():
    _, tickets, beacon_key, sealer = setup(2)
    receipt = sealer.admit(tickets[0], frame(100, 2, 1), now_ns=15_000_000_000)
    sealer.admit(tickets[1], frame(100, 2, 2), now_ns=15_000_000_000)
    _, manifest = sealer.close(RandomnessBeacon.sign(101, b"b" * 32, beacon_key),
                               now_ns=21_000_000_000)
    assert manifest.verify(sealer.verifying_key)
    assert not prove_omission(receipt, manifest, sealer.verifying_key)

    # A maliciously signed manifest omitting the admitted pair is challengeable.
    kept_tickets = manifest.ordered_ticket_digests[1:]
    kept_frames = manifest.ordered_frame_digests[1:]
    candidate = BatchManifest(manifest.slot, manifest.node, manifest.beacon_round,
                              manifest.beacon_value, kept_tickets, kept_frames,
                              manifest.previous_digest, b"")
    forged = BatchManifest(candidate.slot, candidate.node, candidate.beacon_round,
                           candidate.beacon_value, candidate.ordered_ticket_digests,
                           candidate.ordered_frame_digests, candidate.previous_digest,
                           sealer.signing_key.sign(candidate.unsigned()))
    omitted_receipt = receipt if receipt.ticket_digest not in kept_tickets else sealer._receipts[
        manifest.ordered_ticket_digests[0]]
    assert prove_omission(omitted_receipt, forged, sealer.verifying_key)


def test_ticket_tampering_and_old_beacon_are_rejected():
    _, tickets, beacon_key, sealer = setup(1)
    bad = type(tickets[0])(tickets[0].slot, hashlib.sha256(b"bad").digest(),
                           tickets[0].issued_at, tickets[0].expires_at,
                           tickets[0].signature)
    with pytest.raises(ValueError, match="invalid"):
        sealer.admit(bad, frame(100, 2, 1), now_ns=15_000_000_000)
    sealer.admit(tickets[0], frame(100, 2, 1), now_ns=15_000_000_000)
    with pytest.raises(ValueError, match="after"):
        sealer.close(RandomnessBeacon.sign(100, b"b" * 32, beacon_key),
                     now_ns=21_000_000_000)
