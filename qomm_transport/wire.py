"""Constant-size frames and the secret sharing that fills them.

Every frame is the same length whether it carries a real request or cover
traffic, so the byte count on the wire cannot separate the two. The request is
additively shared across the nodes before it ever leaves the client, so a relay
that sees one share sees a uniformly random string.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import secrets
import struct
from dataclasses import dataclass

MAGIC = b"QOMMWIRE"
VERSION = 1
PAYLOAD_BYTES = 256          # every share is padded to this, real or cover
MAC_BYTES = 32
HEADER = struct.Struct("!8sBIH")     # magic, version, slot, node
FRAME_BYTES = HEADER.size + PAYLOAD_BYTES + MAC_BYTES

# The share field. Requests are small integers, so a 256-bit modulus is ample and
# keeps a single share uniform over its whole range.
FIELD = (1 << 255) - 19


@dataclass(frozen=True)
class Frame:
    slot: int
    node: int
    payload: bytes
    mac: bytes

    def encode(self) -> bytes:
        if len(self.payload) != PAYLOAD_BYTES:
            raise ValueError("payload must be exactly PAYLOAD_BYTES")
        return HEADER.pack(MAGIC, VERSION, self.slot, self.node) + self.payload + self.mac

    @classmethod
    def decode(cls, raw: bytes) -> "Frame":
        if len(raw) != FRAME_BYTES:
            raise ValueError(f"frame must be {FRAME_BYTES} bytes, got {len(raw)}")
        magic, version, slot, node = HEADER.unpack(raw[:HEADER.size])
        if magic != MAGIC or version != VERSION:
            raise ValueError("bad frame header")
        body = raw[HEADER.size:HEADER.size + PAYLOAD_BYTES]
        return cls(slot=slot, node=node, payload=body, mac=raw[HEADER.size + PAYLOAD_BYTES:])


def share_request(values: list[int], n_nodes: int, rng=None) -> list[bytes]:
    """Additively share a request; each share is padded to the fixed payload size.

    Cover traffic shares a vector of zeros. Any single share of either is uniform
    in the field, so the relay that carries it learns nothing from its contents.
    """
    rng = rng or secrets.SystemRandom()
    per_value = 32
    if len(values) * per_value > PAYLOAD_BYTES:
        raise ValueError("request does not fit the fixed payload")
    columns: list[list[int]] = []
    for value in values:
        shares = [rng.randrange(FIELD) for _ in range(n_nodes - 1)]
        shares.append((value - sum(shares)) % FIELD)
        columns.append(shares)
    payloads = []
    for node in range(n_nodes):
        blob = b"".join(columns[v][node].to_bytes(per_value, "big") for v in range(len(values)))
        payloads.append(blob + os.urandom(PAYLOAD_BYTES - len(blob)))
    return payloads


def reconstruct(payloads: list[bytes], n_values: int) -> list[int]:
    per_value = 32
    out = []
    for index in range(n_values):
        total = 0
        for payload in payloads:
            chunk = payload[index * per_value:(index + 1) * per_value]
            total = (total + int.from_bytes(chunk, "big")) % FIELD
        out.append(total)
    return out


def frame_mac(key: bytes, slot: int, node: int, payload: bytes) -> bytes:
    return hmac.new(key, HEADER.pack(MAGIC, VERSION, slot, node) + payload,
                    hashlib.sha256).digest()


def frame_is_authentic(key: bytes, frame: "Frame") -> bool:
    """Whether this frame was written by a holder of the key.

    The client computed a MAC over every frame from the first version of this
    module and nothing ever checked one. Thirty-two bytes of every frame were
    carried, counted in every traffic measurement, and bought nothing: anyone
    who could reach a relay's port could write a well-formed frame into a slot's
    batch and corrupt that slot's reconstruction for every node downstream.

    Compared in constant time, because a relay compares a great many of these
    and the comparison is against a secret.
    """
    return hmac.compare_digest(
        frame.mac, frame_mac(key, frame.slot, frame.node, frame.payload))
