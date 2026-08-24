import datetime as dt
import os

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey

from qomm_transport.key_management import (EncryptedKeyStore, create_ca,
                                           issue_mutual_tls_certificate,
                                           write_tls_bundle)


def store(tmp_path, passphrase=b"correct horse battery staple"):
    vault = EncryptedKeyStore(tmp_path / "keys.qks", passphrase)
    vault.initialize()
    return vault


def test_private_material_is_encrypted_atomic_and_mode_0600(tmp_path):
    vault = store(tmp_path)
    key_id = vault.generate("maker:m1:encryption", "x25519", now=100)
    raw = (tmp_path / "keys.qks").read_bytes()
    private = vault.private_key(key_id, at=101)
    secret = private.private_bytes_raw()
    assert secret not in raw
    assert (tmp_path / "keys.qks").stat().st_mode & 0o777 == 0o600
    assert (tmp_path / "keys.qks.lock").stat().st_mode & 0o777 == 0o600
    with pytest.raises(ValueError, match="authentication"):
        EncryptedKeyStore(tmp_path / "keys.qks", b"wrong passphrase").snapshot()


def test_rotation_keeps_overlap_only_when_explicit_and_revocation_is_final(tmp_path):
    vault = store(tmp_path)
    old = vault.generate("maker:m1:encryption", "x25519", now=100)
    new = vault.rotate("maker:m1:encryption", "x25519", now=200)
    assert len(vault.private_keys_for("maker:m1:encryption", at=201)) == 1
    assert len(vault.private_keys_for("maker:m1:encryption", at=201,
                                      include_retired=True)) == 2
    with pytest.raises(ValueError, match="active"):
        vault.private_key(old, at=201)
    vault.revoke(new, now=202, reason="operator credential compromise")
    with pytest.raises(ValueError, match="revoked"):
        vault.private_key(new, at=203)


def test_public_registry_is_signed_and_contains_no_private_field(tmp_path):
    vault = store(tmp_path)
    signer = vault.generate("registry-signing", "ed25519", now=100)
    signing_key = vault.private_key(signer, at=101)
    vault.generate("maker:m1:encryption", "x25519", now=100)
    manifest = vault.public_manifest(signer, issued_at=102)
    assert manifest.verify(signing_key.public_key())
    assert all("private" not in row for row in manifest.records)
    moved = type(manifest)(manifest.generation + 1, manifest.issued_at,
                           manifest.records, manifest.signer_id,
                           manifest.signature)
    assert not moved.verify(signing_key.public_key())


def test_tls_certificates_require_the_ca_and_private_files_are_not_world_readable(tmp_path):
    now = dt.datetime.now(dt.UTC)
    ca_key, ca_cert = create_ca("QOMM test CA", now=now)
    node_key, node_cert = issue_mutual_tls_certificate(
        ca_key, ca_cert, "node-0", dns_names=("node-0",),
        ip_addresses=("127.0.0.1",), now=now)
    key_path, cert_path, ca_path = write_tls_bundle(
        tmp_path / "pki", "node-0", node_key, node_cert, ca_cert)
    assert key_path.stat().st_mode & 0o777 == 0o600
    assert cert_path.stat().st_mode & 0o777 == 0o644
    assert ca_path.stat().st_mode & 0o777 == 0o644
    assert node_cert.issuer == ca_cert.subject


def test_weak_passphrase_and_relaxed_file_permissions_fail_closed(tmp_path):
    with pytest.raises(ValueError, match="12"):
        EncryptedKeyStore(tmp_path / "keys", b"short")
    vault = store(tmp_path)
    os.chmod(tmp_path / "keys.qks", 0o644)
    with pytest.raises(PermissionError, match="600"):
        vault.snapshot()
