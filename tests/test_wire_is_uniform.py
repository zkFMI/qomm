"""A slot on the wire is the same whether or not anyone asked.

The cover traffic is what makes a request's *existence* secret, and existence is
the first thing a plain request-for-quote venue gives away. The property is
mechanical --- same length, same shape, same number of frames --- so it is
tested rather than argued, and tested on the encoded bytes rather than on the
structure that produces them.

The values matter as much as the length. Each share is uniform in the field by
construction, so one node's payload is uniform whether the request behind it was
real or a vector of zeros. A test that only compared lengths would pass on an
implementation that padded a real request with zeros and a cover one with
random bytes, which would be distinguishable at a glance.
"""

from __future__ import annotations

import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_transport.client import N_REQUEST_VALUES                    # noqa: E402
from qomm_transport.wire import (                                     # noqa: E402
    PAYLOAD_BYTES, Frame, frame_mac, reconstruct, share_request,
)

NODES = 7
KEY = b"k" * 32
REAL = [3, 1_000, 1, 42][:N_REQUEST_VALUES] + [0] * max(0, N_REQUEST_VALUES - 4)
COVER = [0] * N_REQUEST_VALUES


def frames(values: list[int], slot: int = 5) -> list[bytes]:
    return [Frame(slot=slot, node=node, payload=payload,
                  mac=frame_mac(KEY, slot, node, payload)).encode()
            for node, payload in enumerate(share_request(values, NODES))]


def test_a_real_slot_and_a_cover_slot_are_the_same_size() -> None:
    real, cover = frames(REAL), frames(COVER)
    assert len(real) == len(cover) == NODES
    assert {len(f) for f in real} == {len(f) for f in cover}
    assert len({len(f) for f in real}) == 1, "the frames differ from each other"


def test_no_share_of_a_real_request_is_the_request() -> None:
    """Any single share is uniform, so a relay carrying one learns nothing."""
    payloads = share_request(REAL, NODES)
    for payload in payloads:
        assert len(payload) == PAYLOAD_BYTES
        # a payload that leaked the request would carry its small integers in
        # the clear, which shows up as leading zero bytes in the 32-byte slots
        for index in range(N_REQUEST_VALUES):
            slot = payload[index * 32:(index + 1) * 32]
            assert slot[:8] != b"\0" * 8, \
                "a share has eight leading zero bytes: it is not field-uniform"


def test_the_shares_still_reconstruct() -> None:
    """The uniformity above must not have been bought by breaking the sharing."""
    assert reconstruct(share_request(REAL, NODES), N_REQUEST_VALUES) == REAL
    assert reconstruct(share_request(COVER, NODES), N_REQUEST_VALUES) == COVER


def test_the_padding_is_not_a_tell() -> None:
    """The bytes past the values must look like the bytes before them.

    Padding a real request with zeros and a cover one with randomness --- or the
    reverse --- would make the two arms distinguishable without touching a
    single share.
    """
    for values in (REAL, COVER):
        for payload in share_request(values, NODES):
            tail = payload[N_REQUEST_VALUES * 32:]
            if not tail:
                continue
            counts = Counter(tail)
            assert counts.most_common(1)[0][1] < len(tail) * 0.2, \
                "the padding is not random"


def test_a_relay_given_a_key_refuses_a_frame_nobody_signed() -> None:
    """The MAC was computed on every frame and checked on none.

    Thirty-two bytes of every frame carried it, every traffic measurement
    counted it, and it bought nothing: anyone who could reach a relay's port
    could write a well-formed frame into a slot's batch and corrupt that slot's
    reconstruction for every node downstream. The relay verifies it now when it
    is given a key, and counts what it refused.
    """
    from qomm_transport.wire import frame_is_authentic

    payload = share_request(REAL, NODES)[0]
    honest = Frame(slot=5, node=0, payload=payload,
                   mac=frame_mac(KEY, 5, 0, payload))
    assert frame_is_authentic(KEY, honest)

    # the same payload with a MAC somebody made up
    forged = Frame(slot=5, node=0, payload=payload, mac=b"\x00" * 32)
    assert not frame_is_authentic(KEY, forged)

    # and the honest frame replayed into a different slot, which the MAC covers
    moved = Frame(slot=6, node=0, payload=payload, mac=honest.mac)
    assert not frame_is_authentic(KEY, moved), \
        "the MAC does not cover the slot, so a frame can be replayed into another"

    # and into another node's stream
    other = Frame(slot=5, node=1, payload=payload, mac=honest.mac)
    assert not frame_is_authentic(KEY, other), \
        "the MAC does not cover the node, so a frame can be moved between them"
