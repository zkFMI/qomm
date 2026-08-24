import os

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey

from qomm_transport.selective_disclosure import (CLEAR_BYTES, open_if_winner,
                                                  seal_for_winner)


def test_only_the_winner_opens_a_fixed_size_envelope():
    taker = Ed25519PrivateKey.generate()
    winner = X25519PrivateKey.generate()
    loser = X25519PrivateKey.generate()
    quote = os.urandom(32)
    envelope = seal_for_winner(maker_id="maker-7", maker_key=winner.public_key(),
                               payload=b"settle instruction", context=b"slot:42",
                               quote_digest=quote, taker_key=taker)
    assert len(envelope.ciphertext) == CLEAR_BYTES + 16
    assert open_if_winner(envelope, maker_id="maker-7", private_keys=[loser],
                          context=b"slot:42", quote_digest=quote,
                          expected_taker=taker.public_key()) is None
    assert open_if_winner(envelope, maker_id="maker-7", private_keys=[winner],
                          context=b"slot:42", quote_digest=quote,
                          expected_taker=taker.public_key()) == b"settle instruction"


def test_the_public_envelope_does_not_name_the_winner_or_key():
    taker = Ed25519PrivateKey.generate()
    winner = X25519PrivateKey.generate()
    quote = b"q" * 32
    envelope = seal_for_winner(maker_id="secret-maker-name",
                               maker_key=winner.public_key(), payload=b"x",
                               context=b"market", quote_digest=quote,
                               taker_key=taker)
    assert b"secret-maker-name" not in envelope.unsigned()
    assert winner.public_key().public_bytes_raw() not in envelope.unsigned()


def test_context_quote_and_taker_tampering_are_refused():
    taker = Ed25519PrivateKey.generate()
    winner = X25519PrivateKey.generate()
    quote = b"q" * 32
    envelope = seal_for_winner(maker_id="m", maker_key=winner.public_key(),
                               payload=b"x", context=b"market",
                               quote_digest=quote, taker_key=taker)
    with pytest.raises(ValueError, match="context"):
        open_if_winner(envelope, maker_id="m", private_keys=[winner],
                       context=b"other", quote_digest=quote)
    with pytest.raises(ValueError, match="signature"):
        open_if_winner(envelope, maker_id="m", private_keys=[winner],
                       context=b"market", quote_digest=quote,
                       expected_taker=Ed25519PrivateKey.generate().public_key())


def test_old_private_key_can_open_during_rotation_overlap():
    taker = Ed25519PrivateKey.generate()
    old = X25519PrivateKey.generate()
    new = X25519PrivateKey.generate()
    quote = b"q" * 32
    envelope = seal_for_winner(maker_id="m", maker_key=old.public_key(),
                               payload=b"rotate", context=b"market",
                               quote_digest=quote, taker_key=taker)
    assert open_if_winner(envelope, maker_id="m", private_keys=[new, old],
                          context=b"market", quote_digest=quote) == b"rotate"


def test_oversized_instruction_is_refused_before_encryption():
    with pytest.raises(ValueError, match="exceeds"):
        seal_for_winner(maker_id="m", maker_key=X25519PrivateKey.generate().public_key(),
                        payload=b"x" * CLEAR_BYTES, context=b"market",
                        quote_digest=b"q" * 32,
                        taker_key=Ed25519PrivateKey.generate())
