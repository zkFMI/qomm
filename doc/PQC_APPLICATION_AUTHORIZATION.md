# Hybrid application authorization

QOMM application protocol version 2 requires Ed25519 and ML-DSA-65 together.
The transport's `application_crypto` adapter uses the common `zkfmi-crypto`
hybrid implementation. It does not implement a signature algorithm itself.

## Enrolled identity and wire format

An application verification identity is a 32-byte SHA-256 fingerprint of the
complete 1,984-byte public key, the explicit hybrid suite, and the attestation
purpose under `QOMM:APPLICATION-KEY-FINGERPRINT:v2`. It is not an Ed25519 public
key. Changing either component requires a new enrolled fingerprint. A key in a
received message must match the independently enrolled identity; it cannot
authorize itself.

Each signature is a closed 5,371-byte envelope:

| Field | Bytes |
| --- | ---: |
| `QOMSIG02` | 8 |
| Encoded `Ed25519MlDsa65` suite | 4 |
| Attestation purpose | 2 |
| Ed25519 and ML-DSA-65 public keys | 1,984 |
| Both signature components | 3,373 |

Verification requires the exact envelope length, magic, suite, purpose, enrolled
fingerprint, statement domain, and both signatures. Classical-only signatures,
raw Ed25519 identities, altered PQ keys, stripped components, and trailing bytes
are rejected. Signing is fallible; randomness/backend errors are returned before
new ticket or slot completion state is accepted.

The signed statements continue to bind their existing venue, chain, asset,
amount commitments, validity intervals, slot, lane, request, and state digests.
Maker and Taker mandate domains and wires, node admission/execution domains and
wires, ticket/beacon/receipt/manifest domains, and public MPC result wires use
the new version. Pretrade authority bundles use version 6, acknowledgements
version 2, and settlement handoffs version 9.

## Custody and restoration

Application signing keys contain two independently generated 32-byte seeds.
Production keys are generated with an operating-system CSPRNG and restored from
the exact encrypted 64-byte record. They are never derived from participant
names, public identifiers, an Ed25519 public key, or a public seed.

The encrypted key store distinguishes `HybridSignature` from legacy Ed25519
keys. Node admission, beacon, and receipt roles use their enrolled hybrid keys.
Participant mandate roles use dedicated application keys; entity approvals keep
their own separately enrolled purpose keys and protocol.

Proof-party encrypted state version 6 adds an independent application identity
for admission, execution, and public-result attestations. It preserves FROST
identity and proof state as distinct fields. A missing application record or an
older state version fails startup explicitly. Restart restores the enrolled
application identity; it never silently generates a replacement for an existing
state file.

## Separate foundation protocols

DeFMI application reserve mandates retain the complete 1,984-byte public key and
3,373-byte hybrid signature with the `SettlementInstruction` purpose. Publication
approvals use complete keys and raw hybrid signatures with `AuditCheckpoint`.
These formats must not be confused with the QOMM attestation envelope or its
fingerprint. CSD issuance and entity approvals also retain their explicit
independently enrolled two-component contracts.

## Scope of the claim

This change protects application authorizations and public attestations. It does
not make the existing curve-based FROST, Pedersen commitments, private range or
price proofs, anonymous credentials, or nullifier relations post-quantum. The
public batch STARK proves its stated public transition relation only. Local and
remote integration tests do not establish independent seven-operator WAN
operation, physical HSM deployment, or production key rollover.

Node-control requests use protocol version 2 and fixed 16 KiB records: one 5,371-byte hybrid signature occupies 10,742 hexadecimal characters before receipt metadata. NodeService persists the complete signed JSON response and returns those exact bytes on the same authenticated request-ID retry. Distributed MPC admission likewise verifies the persisted signature against the node's restored application identity and compares the unsigned statement before returning the exact stored receipt, including after restart. It never compares a saved signature with a newly randomized ML-DSA signature.
