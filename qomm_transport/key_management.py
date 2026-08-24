"""Encrypted lifecycle management for venue, node, maker and KYB keys.

Private material is never written in plaintext except for an explicitly
materialised, mode-0600 TLS runtime file.  The durable store is protected with
scrypt and AES-256-GCM, updated under a process lock, fsynced, and atomically
replaced.  Public distribution is a signed manifest with generation, validity,
rotation and revocation state.
"""

from __future__ import annotations

import base64
import contextlib
import datetime as dt
import fcntl
import hashlib
import ipaddress
import json
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)
from cryptography.hazmat.primitives.asymmetric.x25519 import (
    X25519PrivateKey,
    X25519PublicKey,
)
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.scrypt import Scrypt
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

MAGIC = b"QOMMKEY1"
AAD = b"QOMM:KEYSTORE:v1"
MANIFEST_DOMAIN = b"QOMM:KEY-MANIFEST:v1"
SALT_BYTES = 16
NONCE_BYTES = 12


def _b64(value: bytes) -> str:
    return base64.b64encode(value).decode("ascii")


def _unb64(value: str) -> bytes:
    return base64.b64decode(value, validate=True)


def _canonical(value) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True).encode("utf-8")


def _derive(passphrase: bytes, salt: bytes) -> bytes:
    return Scrypt(salt=salt, length=32, n=1 << 15, r=8, p=1).derive(passphrase)


def _raw_public(key) -> bytes:
    return key.public_bytes(serialization.Encoding.Raw,
                            serialization.PublicFormat.Raw)


def _raw_private(key) -> bytes:
    return key.private_bytes(serialization.Encoding.Raw,
                             serialization.PrivateFormat.Raw,
                             serialization.NoEncryption())


@contextlib.contextmanager
def _lock(path: Path) -> Iterator[None]:
    lock_path = path.with_suffix(path.suffix + ".lock")
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        os.fchmod(fd, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


@dataclass(frozen=True)
class PublicManifest:
    generation: int
    issued_at: int
    records: tuple[dict, ...]
    signer_id: str
    signature: bytes

    def unsigned(self) -> bytes:
        return MANIFEST_DOMAIN + _canonical({
            "generation": self.generation,
            "issued_at": self.issued_at,
            "records": self.records,
            "signer_id": self.signer_id,
        })

    def verify(self, trusted: Ed25519PublicKey) -> bool:
        try:
            trusted.verify(self.signature, self.unsigned())
            return True
        except Exception:
            return False


class EncryptedKeyStore:
    """A small durable store; callers provide the passphrase out of band."""

    def __init__(self, path: Path | str, passphrase: bytes):
        self.path = Path(path)
        if len(passphrase) < 12:
            raise ValueError("key-store passphrase must contain at least 12 bytes")
        self._passphrase = bytes(passphrase)

    def initialize(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with _lock(self.path):
            if self.path.exists():
                raise FileExistsError(self.path)
            self._write_unlocked({"version": 1, "generation": 0, "keys": []})

    def _read_unlocked(self) -> dict:
        mode = self.path.stat().st_mode & 0o777
        if mode & 0o077:
            raise PermissionError(f"refusing key store with mode {mode:o}; expected 600")
        raw = self.path.read_bytes()
        minimum = len(MAGIC) + SALT_BYTES + NONCE_BYTES + 16
        if len(raw) < minimum or raw[:len(MAGIC)] != MAGIC:
            raise ValueError("not a QOMM encrypted key store")
        at = len(MAGIC)
        salt = raw[at:at + SALT_BYTES]
        at += SALT_BYTES
        nonce = raw[at:at + NONCE_BYTES]
        at += NONCE_BYTES
        try:
            clear = AESGCM(_derive(self._passphrase, salt)).decrypt(
                nonce, raw[at:], AAD)
            data = json.loads(clear)
        except Exception as exc:
            raise ValueError("key-store authentication failed") from exc
        if data.get("version") != 1 or not isinstance(data.get("keys"), list):
            raise ValueError("unsupported or malformed key-store payload")
        return data

    def _write_unlocked(self, data: dict) -> None:
        salt, nonce = os.urandom(SALT_BYTES), os.urandom(NONCE_BYTES)
        ciphertext = AESGCM(_derive(self._passphrase, salt)).encrypt(
            nonce, _canonical(data), AAD)
        payload = MAGIC + salt + nonce + ciphertext
        fd, name = tempfile.mkstemp(prefix=f".{self.path.name}.",
                                    dir=self.path.parent)
        try:
            os.fchmod(fd, 0o600)
            with os.fdopen(fd, "wb", closefd=True) as handle:
                handle.write(payload)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(name, self.path)
            directory = os.open(self.path.parent, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        except Exception:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(name)
            raise

    def _mutate(self, operation):
        with _lock(self.path):
            data = self._read_unlocked()
            result = operation(data)
            data["generation"] += 1
            self._write_unlocked(data)
            return result

    def snapshot(self) -> dict:
        with _lock(self.path):
            data = self._read_unlocked()
        # Never hand private material to a status caller.
        return {"version": data["version"], "generation": data["generation"],
                "keys": [{k: v for k, v in row.items() if k != "private"}
                         for row in data["keys"]]}

    def generate(self, purpose: str, kind: str, *, now: int,
                 lifetime: int = 365 * 24 * 3600, metadata: dict | None = None) -> str:
        if not purpose or lifetime <= 0:
            raise ValueError("purpose and positive lifetime are required")
        if kind == "ed25519":
            private = Ed25519PrivateKey.generate()
        elif kind == "x25519":
            private = X25519PrivateKey.generate()
        else:
            raise ValueError("key kind must be ed25519 or x25519")
        public = _raw_public(private.public_key())
        private_raw = _raw_private(private)

        def add(data):
            generation = 1 + max((row["purpose_generation"] for row in data["keys"]
                                  if row["purpose"] == purpose), default=0)
            key_id = hashlib.sha256(
                b"QOMM:KEY-ID:v1" + purpose.encode() + public).hexdigest()
            for row in data["keys"]:
                if row["purpose"] == purpose and row["state"] == "active":
                    row["state"] = "retired"
                    row["retired_at"] = now
            data["keys"].append({
                "key_id": key_id,
                "purpose": purpose,
                "purpose_generation": generation,
                "kind": kind,
                "public": _b64(public),
                "private": _b64(private_raw),
                "created_at": now,
                "not_after": now + lifetime,
                "state": "active",
                "retired_at": None,
                "revoked_at": None,
                "revocation_reason": None,
                "metadata": dict(metadata or {}),
            })
            return key_id

        return self._mutate(add)

    def rotate(self, purpose: str, kind: str, *, now: int,
               lifetime: int = 365 * 24 * 3600, metadata: dict | None = None) -> str:
        return self.generate(purpose, kind, now=now, lifetime=lifetime,
                             metadata=metadata)

    def revoke(self, key_id: str, *, now: int, reason: str) -> None:
        if not reason.strip():
            raise ValueError("a revocation reason is required")

        def revoke_one(data):
            for row in data["keys"]:
                if row["key_id"] == key_id:
                    if row["state"] == "revoked":
                        raise ValueError("the key is already revoked")
                    row["state"] = "revoked"
                    row["revoked_at"] = now
                    row["revocation_reason"] = reason
                    return None
            raise KeyError(key_id)

        self._mutate(revoke_one)

    def _record(self, key_id: str) -> dict:
        with _lock(self.path):
            data = self._read_unlocked()
        for row in data["keys"]:
            if row["key_id"] == key_id:
                return row
        raise KeyError(key_id)

    def private_key(self, key_id: str, *, at: int, allow_retired: bool = False):
        row = self._record(key_id)
        if row["state"] == "revoked" or at > row["not_after"]:
            raise ValueError("the key is revoked or expired")
        if row["state"] != "active" and not allow_retired:
            raise ValueError("the key is no longer active")
        raw = _unb64(row["private"])
        if row["kind"] == "ed25519":
            return Ed25519PrivateKey.from_private_bytes(raw)
        if row["kind"] == "x25519":
            return X25519PrivateKey.from_private_bytes(raw)
        raise ValueError("unsupported key kind")

    def private_keys_for(self, purpose: str, *, at: int,
                         include_retired: bool = False) -> list:
        with _lock(self.path):
            rows = self._read_unlocked()["keys"]
        out = []
        for row in rows:
            if row["purpose"] != purpose or row["state"] == "revoked":
                continue
            if at > row["not_after"] or (row["state"] != "active" and not include_retired):
                continue
            raw = _unb64(row["private"])
            cls = Ed25519PrivateKey if row["kind"] == "ed25519" else X25519PrivateKey
            out.append(cls.from_private_bytes(raw))
        return out

    def public_manifest(self, signer_id: str, *, issued_at: int) -> PublicManifest:
        signing_key = self.private_key(signer_id, at=issued_at)
        if not isinstance(signing_key, Ed25519PrivateKey):
            raise ValueError("public manifests require an Ed25519 signing key")
        snapshot = self.snapshot()
        records = tuple(sorted(snapshot["keys"], key=lambda row: row["key_id"]))
        candidate = PublicManifest(snapshot["generation"], issued_at, records,
                                   signer_id, b"")
        return PublicManifest(candidate.generation, candidate.issued_at,
                              candidate.records, candidate.signer_id,
                              signing_key.sign(candidate.unsigned()))

    def materialize_pkcs8(self, key_id: str, target: Path | str, *, at: int) -> Path:
        """Write an ephemeral TLS-compatible key file with mode 0600."""

        key = self.private_key(key_id, at=at, allow_retired=False)
        pem = key.private_bytes(serialization.Encoding.PEM,
                                serialization.PrivateFormat.PKCS8,
                                serialization.NoEncryption())
        return _secure_write(Path(target), pem, 0o600)


def _secure_write(path: Path, payload: bytes, mode: int) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb", closefd=True) as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(name, path)
        os.chmod(path, mode)
    except Exception:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(name)
        raise
    return path


def create_ca(common_name: str, *, now: dt.datetime,
              lifetime_days: int = 3650) -> tuple[Ed25519PrivateKey, x509.Certificate]:
    key = Ed25519PrivateKey.generate()
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)])
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
            .public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - dt.timedelta(minutes=5))
            .not_valid_after(now + dt.timedelta(days=lifetime_days))
            .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
            .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()),
                           critical=False)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(
                key.public_key()), critical=False)
            .add_extension(x509.KeyUsage(digital_signature=True, content_commitment=False,
                                         key_encipherment=False, data_encipherment=False,
                                         key_agreement=False, key_cert_sign=True,
                                         crl_sign=True, encipher_only=False,
                                         decipher_only=False), critical=True)
            .sign(key, algorithm=None))
    return key, cert


def issue_mutual_tls_certificate(ca_key: Ed25519PrivateKey, ca_cert: x509.Certificate,
                                 common_name: str, *, dns_names: tuple[str, ...] = (),
                                 ip_addresses: tuple[str, ...] = (),
                                 now: dt.datetime,
                                 lifetime_days: int = 30
                                 ) -> tuple[Ed25519PrivateKey, x509.Certificate]:
    """Issue one short-lived certificate valid for both TLS roles."""

    key = Ed25519PrivateKey.generate()
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, common_name)])
    san = [x509.DNSName(name) for name in dns_names]
    san += [x509.IPAddress(ipaddress.ip_address(address)) for address in ip_addresses]
    builder = (x509.CertificateBuilder().subject_name(subject)
               .issuer_name(ca_cert.subject).public_key(key.public_key())
               .serial_number(x509.random_serial_number())
               .not_valid_before(now - dt.timedelta(minutes=5))
               .not_valid_after(now + dt.timedelta(days=lifetime_days))
               .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
               .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()),
                              critical=False)
               .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(
                   ca_key.public_key()), critical=False)
               .add_extension(x509.KeyUsage(
                   digital_signature=True, content_commitment=False,
                   key_encipherment=False, data_encipherment=False,
                   key_agreement=False, key_cert_sign=False, crl_sign=False,
                   encipher_only=False, decipher_only=False), critical=True)
               .add_extension(x509.ExtendedKeyUsage([
                   ExtendedKeyUsageOID.SERVER_AUTH, ExtendedKeyUsageOID.CLIENT_AUTH]),
                   critical=False))
    if san:
        builder = builder.add_extension(x509.SubjectAlternativeName(san), critical=False)
    return key, builder.sign(ca_key, algorithm=None)


def write_tls_bundle(directory: Path | str, name: str, private_key,
                     certificate: x509.Certificate, ca_cert: x509.Certificate
                     ) -> tuple[Path, Path, Path]:
    """Materialise a runtime bundle; private permissions are checked in tests."""

    directory = Path(directory)
    key_path = _secure_write(directory / f"{name}.key.pem",
                             private_key.private_bytes(
                                 serialization.Encoding.PEM,
                                 serialization.PrivateFormat.PKCS8,
                                 serialization.NoEncryption()), 0o600)
    cert_path = _secure_write(directory / f"{name}.cert.pem",
                              certificate.public_bytes(serialization.Encoding.PEM), 0o644)
    ca_path = _secure_write(directory / "ca.cert.pem",
                            ca_cert.public_bytes(serialization.Encoding.PEM), 0o644)
    return key_path, cert_path, ca_path
