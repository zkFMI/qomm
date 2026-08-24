"""Quorum certificate for a privacy-preserving public market statistic."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)

DOMAIN = b"QOMM:PUBLICATION-CERTIFICATE:v1"
ZERO = b"\x00" * 32


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def _hex32(value: bytes, name: str) -> str:
    if len(value) != 32:
        raise ValueError(f"{name} must be 32 bytes")
    return value.hex()


@dataclass(frozen=True)
class PublicationStatement:
    venue: str
    epoch: int
    slot_start: int
    slot_end: int
    source_digest: bytes
    rule_digest: bytes
    mechanism_digest: bytes
    private_input_commitment: bytes
    transcript_digest: bytes
    output_name: str
    output_value: int
    epsilon_micros: int
    delta_numerator: int
    delta_denominator: int
    budget_total_micros: int
    budget_before_micros: int
    budget_after_micros: int
    previous_certificate: bytes = ZERO

    def validate(self) -> None:
        if not self.venue or not self.output_name:
            raise ValueError("venue and output name are required")
        if self.slot_end < self.slot_start or self.epoch < 0:
            raise ValueError("invalid epoch or slot range")
        for name in ("source_digest", "rule_digest", "mechanism_digest",
                     "private_input_commitment", "transcript_digest",
                     "previous_certificate"):
            _hex32(getattr(self, name), name)
        if self.epsilon_micros <= 0:
            raise ValueError("epsilon must be positive")
        if not 0 <= self.delta_numerator < self.delta_denominator:
            raise ValueError("delta must be a proper non-negative fraction")
        if not (0 <= self.budget_before_micros <= self.budget_after_micros
                <= self.budget_total_micros):
            raise ValueError("invalid privacy budget transition")
        if self.budget_after_micros - self.budget_before_micros != self.epsilon_micros:
            raise ValueError("privacy budget transition does not equal epsilon spent")

    def body(self) -> bytes:
        self.validate()
        return DOMAIN + _canonical({
            "venue": self.venue,
            "epoch": self.epoch,
            "slot_start": self.slot_start,
            "slot_end": self.slot_end,
            "source_digest": self.source_digest.hex(),
            "rule_digest": self.rule_digest.hex(),
            "mechanism_digest": self.mechanism_digest.hex(),
            "private_input_commitment": self.private_input_commitment.hex(),
            "transcript_digest": self.transcript_digest.hex(),
            "output_name": self.output_name,
            "output_value": self.output_value,
            "epsilon_micros": self.epsilon_micros,
            "delta_numerator": self.delta_numerator,
            "delta_denominator": self.delta_denominator,
            "budget_total_micros": self.budget_total_micros,
            "budget_before_micros": self.budget_before_micros,
            "budget_after_micros": self.budget_after_micros,
            "previous_certificate": self.previous_certificate.hex(),
        })

    @property
    def digest(self) -> bytes:
        return hashlib.sha256(self.body()).digest()


@dataclass(frozen=True)
class NodeSignature:
    node_id: str
    signature: bytes


@dataclass(frozen=True)
class PublicationCertificate:
    statement: PublicationStatement
    signatures: tuple[NodeSignature, ...]

    @property
    def digest(self) -> bytes:
        h = hashlib.sha256(self.statement.body())
        for signed in sorted(self.signatures, key=lambda item: item.node_id):
            h.update(signed.node_id.encode())
            h.update(signed.signature)
        return h.digest()

    def verify(self, registry: dict[str, Ed25519PublicKey], threshold: int,
               previous: "PublicationCertificate | None" = None) -> bool:
        try:
            self.statement.validate()
        except ValueError:
            return False
        if not 1 <= threshold <= len(registry):
            return False
        if previous is None:
            if self.statement.previous_certificate != ZERO:
                return False
        else:
            if self.statement.previous_certificate != previous.digest:
                return False
            if self.statement.epoch <= previous.statement.epoch:
                return False
            if self.statement.budget_before_micros != previous.statement.budget_after_micros:
                return False
        body = self.statement.body()
        seen = set()
        valid = 0
        for signed in self.signatures:
            if signed.node_id in seen or signed.node_id not in registry:
                continue
            seen.add(signed.node_id)
            try:
                registry[signed.node_id].verify(signed.signature, body)
                valid += 1
            except InvalidSignature:
                pass
        return valid >= threshold


def certify(statement: PublicationStatement,
            signers: dict[str, Ed25519PrivateKey]) -> PublicationCertificate:
    body = statement.body()
    return PublicationCertificate(
        statement,
        tuple(NodeSignature(node, key.sign(body))
              for node, key in sorted(signers.items())),
    )
