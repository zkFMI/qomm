"""Anonymous winner-only delivery of settlement instructions.

The taker learns the opened best price and winner.  It then broadcasts one
fixed-size envelope.  Only the winning maker can decrypt it; the envelope has no
maker identifier or key identifier in its public header, so losing makers and
network observers do not learn who won.  The taker signs the complete envelope
and the encrypted body binds the quote proof and market context.
"""

from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from typing import Iterable

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)
from cryptography.hazmat.primitives.asymmetric.x25519 import (
    X25519PrivateKey,
    X25519PublicKey,
)
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

DOMAIN = b"QOMM:WINNER:ENVELOPE:v1"
INNER_DOMAIN = b"QOMM:WINNER:PAYLOAD:v1"
CLEAR_BYTES = 1024
VERSION = 1


def _raw_public(key) -> bytes:
    return key.public_bytes(serialization.Encoding.Raw,
                            serialization.PublicFormat.Raw)


def _derive(shared: bytes, ephemeral: bytes, context_digest: bytes,
            quote_digest: bytes) -> bytes:
    return HKDF(algorithm=hashes.SHA256(), length=32,
                salt=context_digest,
                info=DOMAIN + ephemeral + quote_digest).derive(shared)


@dataclass(frozen=True)
class WinnerEnvelope:
    version: int
    ephemeral_public: bytes
    nonce: bytes
    context_digest: bytes
    quote_digest: bytes
    ciphertext: bytes
    taker_public: bytes
    signature: bytes

    def unsigned(self) -> bytes:
        if self.version != VERSION:
            raise ValueError("unsupported winner-envelope version")
        if len(self.ephemeral_public) != 32 or len(self.nonce) != 12:
            raise ValueError("invalid public-key or nonce length")
        if len(self.context_digest) != 32 or len(self.quote_digest) != 32:
            raise ValueError("context and quote digests must be 32 bytes")
        if len(self.taker_public) != 32:
            raise ValueError("taker public key must be 32 bytes")
        if len(self.ciphertext) != CLEAR_BYTES + 16:
            raise ValueError("winner ciphertext is not the fixed wire size")
        return (DOMAIN + bytes([self.version]) + self.ephemeral_public + self.nonce
                + self.context_digest + self.quote_digest + self.taker_public
                + self.ciphertext)

    @property
    def commitment(self) -> bytes:
        return hashlib.sha256(self.unsigned() + self.signature).digest()

    def verify_taker(self, expected: Ed25519PublicKey | None = None) -> bool:
        if expected is not None and _raw_public(expected) != self.taker_public:
            return False
        try:
            Ed25519PublicKey.from_public_bytes(self.taker_public).verify(
                self.signature, self.unsigned())
            return True
        except (InvalidSignature, ValueError):
            return False


def seal_for_winner(*, maker_id: str, maker_key: X25519PublicKey,
                    payload: bytes, context: bytes, quote_digest: bytes,
                    taker_key: Ed25519PrivateKey) -> WinnerEnvelope:
    """Encrypt a fixed-size, context-bound instruction to one maker."""

    maker = maker_id.encode("utf-8")
    if not maker or len(maker) > 255:
        raise ValueError("maker identifier must contain 1..255 UTF-8 bytes")
    if len(quote_digest) != 32:
        raise ValueError("quote proof digest must be 32 bytes")
    context_digest = hashlib.sha256(context).digest()
    fixed = (INNER_DOMAIN + bytes([len(maker)]) + maker
             + len(payload).to_bytes(4, "big") + payload
             + context_digest + quote_digest)
    if len(fixed) > CLEAR_BYTES:
        raise ValueError(f"settlement payload exceeds {CLEAR_BYTES} encrypted bytes")
    clear = fixed + os.urandom(CLEAR_BYTES - len(fixed))
    ephemeral_private = X25519PrivateKey.generate()
    ephemeral_public = _raw_public(ephemeral_private.public_key())
    shared = ephemeral_private.exchange(maker_key)
    key = _derive(shared, ephemeral_public, context_digest, quote_digest)
    nonce = os.urandom(12)
    associated = DOMAIN + ephemeral_public + context_digest + quote_digest
    ciphertext = ChaCha20Poly1305(key).encrypt(nonce, clear, associated)
    taker_public = _raw_public(taker_key.public_key())
    unsigned = (DOMAIN + bytes([VERSION]) + ephemeral_public + nonce
                + context_digest + quote_digest + taker_public + ciphertext)
    return WinnerEnvelope(VERSION, ephemeral_public, nonce, context_digest,
                          quote_digest, ciphertext, taker_public,
                          taker_key.sign(unsigned))


def open_if_winner(envelope: WinnerEnvelope, *, maker_id: str,
                   private_keys: Iterable[X25519PrivateKey], context: bytes,
                   quote_digest: bytes,
                   expected_taker: Ed25519PublicKey | None = None) -> bytes | None:
    """Return the instruction for the winner and ``None`` for every loser.

    Signature or context failures are protocol errors and raise.  An AEAD
    failure is the normal losing-maker result and therefore returns ``None``.
    """

    if not envelope.verify_taker(expected_taker):
        raise ValueError("the taker signature on the winner envelope is invalid")
    context_digest = hashlib.sha256(context).digest()
    if envelope.context_digest != context_digest or envelope.quote_digest != quote_digest:
        raise ValueError("the envelope is bound to another quote or market context")
    ephemeral = X25519PublicKey.from_public_bytes(envelope.ephemeral_public)
    associated = DOMAIN + envelope.ephemeral_public + context_digest + quote_digest
    clear = None
    for private in private_keys:
        shared = private.exchange(ephemeral)
        key = _derive(shared, envelope.ephemeral_public, context_digest, quote_digest)
        try:
            clear = ChaCha20Poly1305(key).decrypt(
                envelope.nonce, envelope.ciphertext, associated)
            break
        except InvalidSignature:
            # cryptography uses InvalidTag, imported lazily below to keep the
            # signature failure path visibly separate.
            raise
        except Exception as exc:  # only InvalidTag is a normal key mismatch
            from cryptography.exceptions import InvalidTag
            if isinstance(exc, InvalidTag):
                continue
            raise
    if clear is None:
        return None
    if not clear.startswith(INNER_DOMAIN):
        raise ValueError("decrypted winner payload has the wrong domain")
    at = len(INNER_DOMAIN)
    maker_len = clear[at]
    at += 1
    recipient = clear[at:at + maker_len].decode("utf-8")
    at += maker_len
    payload_len = int.from_bytes(clear[at:at + 4], "big")
    at += 4
    end = at + payload_len
    if end + 64 > len(clear):
        raise ValueError("decrypted winner payload has an invalid length")
    payload = clear[at:end]
    if recipient != maker_id:
        raise ValueError("a key opened an envelope addressed to another maker")
    if clear[end:end + 32] != context_digest or clear[end + 32:end + 64] != quote_digest:
        raise ValueError("the encrypted payload and public header disagree")
    return payload
