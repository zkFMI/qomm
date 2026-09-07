# Audit authorization and application custody

Audit receipts, public statistics certificates, and privacy-budget allocations use the common Ed25519 + ML-DSA-65 verifier with `AuditCheckpoint` purpose. Both signature components must verify. The certificate and receipt domains are version 2; historical 64-byte signatures are not accepted as live authorization. Existing publication ledgers are preserved and require an explicit archival checkpoint and migration before reuse.

Publication and governance registries contain independently enrolled 1984-byte public keys. The three-of-seven publication ledger rejects repeated Ed25519 or ML-DSA components within a registry. Its quorum does not derive authority from a key supplied with a signature. The distributed publication harness pins each locally supervised child's publication key during startup, before asking it to sign; deployment enrollment must authenticate that startup channel. Since 2026-09-08 the FROST identity self-signature (both its Ed25519 and ML-DSA-65 components, domain `QOMM:FROST:PEER-IDENTITY:v3`) covers this additional publication key, and the harness verifies both self-signatures before pinning it.

These signatures authenticate the existing audit statements. They do not re-prove private predicates, the MPC computation, or the differential-privacy mechanism. The separate Miden public batch proof has its own explicitly limited statement and provenance.

Application signing keys use the QOMM `QOMSIG02` envelope and independent 64-byte seeds stored in authenticated encrypted custody. Participant policy and execution mandates use dedicated `quote_application` and `settlement_application` keys; entity governance approvals retain their separately enrolled purpose keys. Old participant records missing these dedicated keys require migration and are never silently re-enrolled.

The `qomm_key_tool` binary manages application keys without printing private material:

```text
qomm_key_tool keys.qks passphrase-file init
qomm_key_tool keys.qks passphrase-file generate order-signing 1788652800 86400
qomm_key_tool keys.qks passphrase-file public
qomm_key_tool keys.qks passphrase-file rotate order-signing 1788652900 86400
qomm_key_tool keys.qks passphrase-file revoke KEY_ID 1788653000 compromised
```

The passphrase file must be owned by the current user, have no group or other access, and contain the exact passphrase bytes. It is opened without following symlinks. Initialization refuses an existing store. Rotation retires the previous key for live use; explicitly authorized historical access remains possible until revocation. Revoked keys are rejected even through the historical accessor. A hybrid purpose cannot be downgraded to Ed25519 through rotation.

Public key-store manifests also require a hybrid application key and bind the registry generation, issue time, signer ID, and canonical public records. Operators must obtain the verifying fingerprint through enrollment rather than from the manifest being verified.
