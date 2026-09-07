//! Docker/WAN execution adapter for the browser demonstration.
//!
//! The browser coordinator creates the same MP-SPDZ party inputs as the local
//! demo engine, but sends only `Input-P<node>-0` to the corresponding node.
//! Every node independently verifies the pinned circuit digest and runs one
//! stock MP-SPDZ process.  A round succeeds only when all seven services return
//! the same public masked result.

use crate::asset_ids::{cash_asset_id, traded_asset_id};
use crate::defmi_bootstrap::{
    development_receipt_signing_key, DefmiAdmissionReceipt, DefmiMarketEpoch,
    MakerStandingPoolRequest,
};
use crate::model::{evaluate, Outcome, Policy, Request, DEMO_POLICY_VALID_UNTIL};
use crate::mpc::{
    is_corporate_queue_pending, verify_masked_execution, MaskedExecutionOpening, MpcProductHandoff,
    MpcQuoteEngine, MpcRound, MpcSettlementInputs, QueuedMpcRound, CORPORATE_QUEUE_RECONCILING,
    CORPORATE_QUEUE_UNAVAILABLE, CORPORATE_QUEUE_WAITING_SLOT,
};
use crate::participant_client::{
    ClaimedCorporateRequest, CorporateOutboxAction, ParticipantClient, ParticipantSnapshot,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use defmi::claim_redemption::NoteClaimAuthorization;
use defmi::facility::{
    build_threshold_dvp_consumption_from_snapshot, reserve_handle_for, CreditFacilityTransition,
    CreditHoldSnapshot, CreditTransitionKind, ReservationAuthorization, ReservationConsumption,
    ReservationRole, ZERO,
};
use defmi::note_chain::{
    note_claim_recipient_commitment, standing_pool_product_settlement_statement,
    DelegatedClaimOpenings, DelegatedNoteLegProjection, NoteClaimKind, NoteOutput,
    ProductNoteBindings, StandingNotePoolAllocation, VerifiedDelegatedNoteSettlementProjection,
};
use defmi::participant::{EntityApproval, KeyPurpose};
use defmi::product_evidence::{MpcNoFillEvidence, ProductSettlementEvidence};
use defmi::settlement::{build_threshold_package_from_proofs, Sides};
use qomm_mpc::compiler::OfficialCompiler;
use qomm_mpc::inputs::{
    build_inputs, finish_reference, parse_policies, DvpInputs, InputConfig, QuoteProofInputs,
    QUOTE_POLICY_BLINDING_FIELDS,
};
use qomm_mpc::program::{
    build_program, pow2_ceil, sentinel_for, CheckMode, Mode, ProgramConfig, Reference, StopAfter,
    ED25519_ORDER, PRODUCT_DVP_REMAINDER_BITS, PRODUCT_QUOTE_ELIGIBILITY_BITS,
    PRODUCT_QUOTE_SPAN_BITS, PRODUCT_ZKPI_AMOUNT_BITS, PRODUCT_ZKPI_PRICE_BITS,
};
use qomm_proofs::kyb::{verify_presentation, KybPresentation, SignedCohortRegistry};
use qomm_proofs::price_limit::{from_threshold as threshold_price_limit, PriceLimitDirection};
use qomm_proofs::quote_proof::{registered_policy_digest, registry_digest, RegisteredPolicy};
use qomm_transport::application_crypto::{Signature, VerifyingKey};
use qomm_transport::frost_coordinator::{distributed_frost_setup, recall_frost_group};
use qomm_transport::mandate::{
    decode_maker_mandate, decode_taker_mandate, encode_maker_mandate, encode_taker_mandate,
    Direction, MakerPolicyMandate, TakerExecutionMandate,
};
use qomm_transport::mpc_result::fill_mask_commitment;
use qomm_transport::mpc_result::{
    decode_node_public_result_attestation, encode_node_public_result_attestation,
    encode_public_result_attestations, verify_public_result_lane, NodePublicResultAttestation,
};
use qomm_transport::order::{
    encode_admission_attestations, principal_ticket_id, verify_admission_lane,
    NodeAdmissionAttestation, COMMITTEE_NODES,
};
use qomm_transport::pretrade_authority::{
    PretradeAcknowledgement, PretradeReservationBinding, ReservationParty,
};
use qomm_transport::proof_client::ProofPartyRpc;
use qomm_transport::proof_codec::{
    encode_dvp_proofs, encode_quote_verification, encode_threshold_range,
};
use qomm_transport::proof_party::{ProofParty, ProofPartyConfig, ProofRequest, ProofResponse};
use qomm_transport::resident_mpc::{
    combine_partial_commitments, commit_standing_pool_remainder_with_store, rebind_standing_pool,
    scalar_to_decimal, splice_standing_maker_shares, EncryptedMpcStateStore, InputSharing,
    MpcSecretState, StandingPoolBinding, StandingPoolCommitRequest, StandingPoolStateReceipt,
};
use qomm_transport::settlement_handoff::encode_private_record;
use qomm_transport::standing_pool::{
    standing_pool_reservation_metadata, threshold_dvp_package_digest, threshold_dvp_sides,
    threshold_range_proof_digest,
};
use zkfmi_zk::pedersen::Pedersen;
use zkpi::typed::{AuthorizationScope, ExecutionContext, OperationKind, TradeDirection};
use zkpi::{typed_wire, Bounds, Venue};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::fs::symlink;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zkpi_defmi_sdk::application::qomm_manifest_v1;
use zkpi_defmi_sdk::execution::{ApplicationExecutionPlan, ExecutionNodeDigest, ExecutionShape};
use zkpi_defmi_sdk::finality::{accept_canonical_transition, CanonicalTransition};
use zkpi_defmi_sdk::product::{
    authorize_standing_pool_allocation, complete_product_proof, complete_quote_request,
    finalize_product_settlement, prove_complete_quote, prove_product_settlement,
    CompleteQuotePublicInput, ProductSettlementRequest, RegisteredPolicyOpening,
};

const PROTOCOL_VERSION: u8 = 1;
const MAX_HTTP_BYTES: usize = 8 << 20;
const MAX_INPUT_BYTES: usize = 4 << 20;
const DEMO_MAKER_VALID_UNTIL: u64 = DEMO_POLICY_VALID_UNTIL as u64;
type GeneratedRoundInputs = (Vec<String>, Value, u64, BTreeMap<usize, Vec<String>>);

fn market_time_for_execution(wall_now: u64, replayed: Option<i64>) -> Result<i64, String> {
    let current = i64::try_from(wall_now)
        .map_err(|_| "distributed settlement time exceeds the signed market-time range")?;
    let Some(replayed) = replayed else {
        return Ok(current);
    };
    let replayed_unsigned =
        u64::try_from(replayed).map_err(|_| "queued RFQ has a negative market time".to_string())?;
    if replayed_unsigned > wall_now {
        return Err("queued RFQ market time is future-dated".into());
    }
    Ok(replayed)
}

fn execution_lane_for_admission(sequence: u64) -> Result<usize, String> {
    let lane = sequence
        .checked_sub(1)
        .ok_or_else(|| "admission sequence cannot identify an execution lane".to_string())?;
    usize::try_from(lane)
        .ok()
        .filter(|lane| *lane <= 4095)
        .ok_or_else(|| "admission sequence exceeds the execution-lane bound".to_string())
}

#[derive(Clone, Debug)]
struct Endpoint {
    authority: String,
}

impl Endpoint {
    fn parse(value: &str) -> Result<Self, String> {
        let authority = value
            .strip_prefix("http://")
            .ok_or_else(|| {
                "MPC node endpoint must use http:// inside the Docker network".to_string()
            })?
            .trim_end_matches('/');
        if authority.is_empty()
            || authority.contains('/')
            || authority.contains('@')
            || !authority.contains(':')
        {
            return Err("MPC node endpoint must be http://host:port".into());
        }
        Ok(Self {
            authority: authority.to_string(),
        })
    }

    fn post(&self, path: &str, value: &Value, timeout: Duration) -> Result<Value, String> {
        self.request("POST", path, Some(value), timeout)
    }

    fn get(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        self.request("GET", path, None, timeout)
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        value: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let body = match value {
            Some(value) => serde_json::to_vec(value).map_err(|error| error.to_string())?,
            None => Vec::new(),
        };
        let address = self
            .authority
            .to_socket_addrs()
            .map_err(|error| error.to_string())?
            .next()
            .ok_or_else(|| format!("MPC node {} did not resolve", self.authority))?;
        let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|error| {
            format!(
                "MPC node {} did not accept a connection: {error}",
                self.authority
            )
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| stream.set_write_timeout(Some(timeout)))
            .map_err(|error| error.to_string())?;
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.authority,
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .and_then(|_| stream.write_all(&body))
            .and_then(|_| stream.flush())
            .map_err(|error| error.to_string())?;
        let _ = stream.shutdown(Shutdown::Write);
        let mut response = Vec::new();
        Read::by_ref(&mut stream)
            .take((MAX_HTTP_BYTES + 1) as u64)
            .read_to_end(&mut response)
            .map_err(|error| error.to_string())?;
        if response.len() > MAX_HTTP_BYTES {
            return Err("MPC node response exceeded its fixed bound".into());
        }
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| "MPC node returned malformed HTTP".to_string())?;
        let head = std::str::from_utf8(&response[..split])
            .map_err(|_| "MPC node returned non-UTF8 HTTP headers")?;
        let status = head.lines().next().unwrap_or_default();
        let body = &response[split + 4..];
        let value: Value = serde_json::from_slice(body)
            .map_err(|error| format!("MPC node returned malformed JSON: {error}"))?;
        if !status.contains(" 200 ") {
            return Err(value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("MPC node rejected the request")
                .to_string());
        }
        Ok(value)
    }

    fn healthy(&self, expected_source: &str, timeout: Duration) -> bool {
        let Ok(address) = self
            .authority
            .to_socket_addrs()
            .ok()
            .and_then(|mut values| values.next())
            .ok_or(())
        else {
            return false;
        };
        let Ok(mut stream) = TcpStream::connect_timeout(&address, timeout) else {
            return false;
        };
        if stream
            .set_read_timeout(Some(timeout))
            .and_then(|_| stream.set_write_timeout(Some(timeout)))
            .is_err()
        {
            return false;
        }
        let request = format!(
            "GET /health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.authority
        );
        if stream
            .write_all(request.as_bytes())
            .and_then(|_| stream.flush())
            .is_err()
        {
            return false;
        }
        let _ = stream.shutdown(Shutdown::Write);
        let mut response = Vec::new();
        if Read::by_ref(&mut stream)
            .take((MAX_HTTP_BYTES + 1) as u64)
            .read_to_end(&mut response)
            .is_err()
            || response.len() > MAX_HTTP_BYTES
        {
            return false;
        }
        let Some(split) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let Ok(head) = std::str::from_utf8(&response[..split]) else {
            return false;
        };
        if !head.lines().next().unwrap_or_default().contains(" 200 ") {
            return false;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&response[split + 4..]) else {
            return false;
        };
        value.get("ok").and_then(Value::as_bool) == Some(true)
            && value.get("source_sha256").and_then(Value::as_str) == Some(expected_source)
    }
}

struct HttpProofPartyClient {
    endpoint: Endpoint,
    timeout: Duration,
    next_id: u64,
}

impl HttpProofPartyClient {
    fn new(endpoint: Endpoint, timeout: Duration) -> Self {
        Self {
            endpoint,
            timeout,
            next_id: 1,
        }
    }
}

impl ProofPartyRpc for HttpProofPartyClient {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        if method.is_empty() || method.len() > 128 {
            return Err("proof-party method is outside its fixed bound".into());
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "proof-party request identifier is exhausted".to_string())?;
        let request = ProofRequest {
            id,
            method: method.to_string(),
            params,
        };
        let response: ProofResponse = serde_json::from_value(self.endpoint.post(
            "/v1/proof",
            &serde_json::to_value(request).map_err(|error| error.to_string())?,
            self.timeout,
        )?)
        .map_err(|_| "MPC node returned a malformed proof response".to_string())?;
        if response.id != id {
            return Err("proof-party response identifier does not match".into());
        }
        if !response.ok {
            return Err(response
                .error
                .unwrap_or_else(|| "proof-party rejected the request".into()));
        }
        response
            .result
            .ok_or_else(|| "proof-party success response has no result".into())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AdmitRequest {
    version: u8,
    node: usize,
    round_id: String,
    source_sha256: String,
    public_market_time: i64,
    input_sha256: String,
    admission: ExecuteAdmission,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExecuteRequest {
    version: u8,
    node: usize,
    round_id: String,
    source_sha256: String,
    public_market_time: i64,
    input_sha256: String,
    input: String,
    /// Monotonic corporate-outbox delivery attempt. Attempt one uses the
    /// canonical round directory. A later attempt runs every party again in
    /// one isolated recovery generation so a node with a cached receipt still
    /// participates for peers that crashed before persisting theirs.
    execution_generation: u32,
    admission: ExecuteAdmission,
    certified_admission: AdmissionAttestationWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExecuteAdmission {
    slot: u64,
    sequence: u64,
    principal: String,
    ticket_id: String,
    claim_digest: String,
    order_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AdmissionAttestationWire {
    node: u16,
    slot: u64,
    sequence: u64,
    principal_digest: String,
    ticket_id: String,
    claim_digest: String,
    batch_digest: String,
    order_digest: String,
    identity_public: String,
    signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExecuteReceipt {
    version: u8,
    node: usize,
    round_id: String,
    source_sha256: String,
    public_market_time: i64,
    input_sha256: String,
    #[serde(default = "initial_execution_generation")]
    execution_generation: u32,
    masked_key: i128,
    masked_fill: i128,
    elapsed_ms: f64,
    stdout_sha256: String,
    stderr_sha256: String,
    persistence_sha256: String,
    result_identity_public: String,
    public_result_attestation: String,
    admission: AdmissionAttestationWire,
    /// Generation of the node's encrypted resident Maker state whose shares
    /// were spliced into this execution.  Zero only for receipts written before
    /// Maker reserves became node-resident.
    #[serde(default)]
    maker_state_generation: u64,
    /// Number of whitespace-separated party input values, which is part of the
    /// canonical proof-job identity.  Zero for legacy receipts.
    #[serde(default)]
    input_count: u64,
}

const fn initial_execution_generation() -> u32 {
    1
}

/// Party inputs of the Docker circuit are summed by `secret_input()`, so each
/// node stores its Shamir evaluation pre-multiplied by its Lagrange weight.
const MAKER_STATE_SHARING: InputSharing = InputSharing::Additive;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateSeedRequest {
    version: u8,
    node: usize,
    source_sha256: String,
    /// This node's additive shares, in circuit order, of every padded Maker's
    /// securities reserve, its blinding, cash reserve, its blinding, and
    /// settlement handle scalar.
    dvp_input_shares: Vec<String>,
    bindings: Vec<StandingPoolBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateCommitRequest {
    version: u8,
    node: usize,
    source_sha256: String,
    round_id: String,
    execution_generation: u32,
    maker: usize,
    direction: u8,
    expected_generation: u64,
    proof_job_id: String,
    /// Canonical DeFMI remainder note created by the accepted allocation.
    remainder_note_id: String,
    pool_id: String,
    pool_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateRebindRequest {
    version: u8,
    node: usize,
    source_sha256: String,
    maker: usize,
    direction: u8,
    expected_generation: u64,
    pool_id: String,
    amount_share: String,
    blinding_share: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateBindingView {
    maker: usize,
    direction: u8,
    pool_id: String,
    pool_sequence: u64,
    /// Pedersen commitment to this node's share pair. Summed over the seven
    /// nodes it equals the commitment of the reconstructed opening; alone it
    /// reveals nothing about the opening.
    partial_commitment: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateView {
    node: usize,
    initialized: bool,
    generation: u64,
    source_sha256: String,
    sharing: InputSharing,
    n_mm: usize,
    bindings: Vec<MakerStateBindingView>,
}

impl MakerStateView {
    fn binding(&self, maker: usize, direction: u8) -> Option<&MakerStateBindingView> {
        self.bindings
            .iter()
            .find(|binding| binding.maker == maker && binding.direction == direction)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MakerStateCommitResponse {
    receipt: StandingPoolStateReceipt,
    state: MakerStateView,
}

/// Public digests of one node-local execution.  The coordinator recomputes the
/// canonical proof-job identifier from all seven and matches it against the
/// DeFMI remainder note when it has to find which execution a pool sequence
/// was allocated from after an outage.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct NodeRoundReceiptView {
    round_id: String,
    execution_generation: u32,
    public_market_time: i64,
    slot: u64,
    sequence: u64,
    order_digest: String,
    input_sha256: String,
    stdout_sha256: String,
    stderr_sha256: String,
    persistence_sha256: String,
    input_count: u64,
    maker_state_generation: u64,
}

impl NodeRoundReceiptView {
    fn from_receipt(receipt: &ExecuteReceipt) -> Self {
        Self {
            round_id: receipt.round_id.clone(),
            execution_generation: receipt.execution_generation,
            public_market_time: receipt.public_market_time,
            slot: receipt.admission.slot,
            sequence: receipt.admission.sequence,
            order_digest: receipt.admission.order_digest.clone(),
            input_sha256: receipt.input_sha256.clone(),
            stdout_sha256: receipt.stdout_sha256.clone(),
            stderr_sha256: receipt.stderr_sha256.clone(),
            persistence_sha256: receipt.persistence_sha256.clone(),
            input_count: receipt.input_count,
            maker_state_generation: receipt.maker_state_generation,
        }
    }
}

/// Circuit rail of a Maker mandate direction: the securities pool (rail 0)
/// pays when the Taker buys, the cash pool (rail 1) when the Taker sells.  The
/// mandate enum itself is numbered 1 and 2 for the DeFMI wire and must not be
/// used as a rail index.
fn standing_rail(direction: Direction) -> u8 {
    match direction {
        Direction::TakerBuys => 0,
        Direction::TakerSells => 1,
    }
}

/// Deal one value into seven uniformly random additive shares over the MPC
/// prime.  Every share alone is independent of the value.
fn deal_additive_shares(value: &Scalar, n_parties: usize) -> Vec<String> {
    let mut shares = Vec::with_capacity(n_parties);
    let mut total = Scalar::ZERO;
    for _ in 1..n_parties {
        let share = Scalar::random(&mut OsRng);
        total += share;
        shares.push(scalar_to_decimal(&share));
    }
    shares.push(scalar_to_decimal(&(value - total)));
    shares
}

#[derive(Clone, Debug)]
struct PretradeSigner {
    client: ParticipantClient,
    snapshot: ParticipantSnapshot,
    venue_id: [u8; 32],
    defmi_id: [u8; 32],
    presentation: KybPresentation,
    registry: SignedCohortRegistry,
    trusted_issuer: qomm_proofs::kyb::KybIssuerKey,
    identity_scope: Vec<u8>,
    identity_context: Vec<u8>,
    required_cohort: String,
}

#[derive(Clone, Debug)]
struct MakerPretradeSigner {
    client: ParticipantClient,
    snapshot: ParticipantSnapshot,
    presentation: KybPresentation,
}

#[derive(Clone, Debug)]
struct CachedMakerAuthority {
    source_policy_digest: [u8; 32],
    registered_policy_digest: [u8; 32],
    policy_blindings: [u64; QUOTE_POLICY_BLINDING_FIELDS],
    inventory_reserve: i64,
    inventory_reserve_blinding: u64,
    cash_reserve: i64,
    cash_reserve_blinding: u64,
    valid_until: u64,
    signed_mandates: Vec<Vec<u8>>,
    standing_pools: Vec<CachedMakerPool>,
}

#[derive(Clone, Debug)]
struct CachedMakerPool {
    mandate: MakerPolicyMandate,
    pool_id: [u8; 32],
    facility_id: [u8; 32],
}

fn allocation_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}

fn allocation_scalar(parts: &[&[u8]]) -> Scalar {
    let mut scalar = Scalar::from_bytes_mod_order(allocation_hash(parts));
    if scalar == Scalar::ZERO {
        scalar = Scalar::ONE;
    }
    scalar
}

fn participant_derived_u64(approval: &EntityApproval, label: &[u8]) -> u64 {
    let digest = Sha256::new()
        .chain_update(b"QOMM:MAKER:PARTICIPANT-DERIVED-VALUE:v1")
        .chain_update(approval.participant_id)
        .chain_update(approval.statement)
        .chain_update(&approval.signature)
        .chain_update(label)
        .finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix")).max(1)
}

/// Create a public covenant note. It deliberately has no wallet recipient:
/// the hold/pool lock controls it and the later DvP produces recipient-bound
/// encrypted claim openings. The deterministic public fields make a lost RPC
/// response safely retryable without exposing any amount opening.
fn allocation_note(
    asset_id: [u8; 32],
    value_commitment: [u8; 32],
    lock_id: [u8; 32],
    job_id: [u8; 32],
    label: &[u8],
) -> Result<NoteOutput, String> {
    if asset_id == ZERO
        || lock_id == ZERO
        || CompressedRistretto(value_commitment).decompress().is_none()
    {
        return Err("standing allocation note has invalid asset, lock, or commitment".into());
    }
    let one_time =
        G * allocation_scalar(&[b"QOMM:DEMO:ALLOCATION-NOTE:ONE-TIME:v1", &job_id, label]);
    let ephemeral =
        G * allocation_scalar(&[b"QOMM:DEMO:ALLOCATION-NOTE:EPHEMERAL:v1", &job_id, label]);
    let mut output = NoteOutput {
        note_id: ZERO,
        asset_id,
        one_time: one_time.compress().to_bytes(),
        value_commitment,
        ephemeral: ephemeral.compress().to_bytes(),
        encrypted_opening: qomm_transport::standing_pool::NoteOpening::Covenant,
        lock_id,
    };
    output.note_id = output.derived_id()?;
    output.validate()?;
    Ok(output)
}

#[derive(Clone, Debug)]
struct PreparedAdmission {
    envelope: ExecuteAdmission,
    mandate_digest: [u8; 32],
    /// Canonical Taker mandate body followed by its Ed25519 signature.  This is
    /// public authorization evidence, not a private key or an opening.
    signed_mandate: Option<Vec<u8>>,
    taker_mandate: Option<TakerExecutionMandate>,
    maximum_amount: u64,
    maximum_blinding: u64,
    response_mask: u64,
    fill_mask: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct QueuedRfqEnvelope {
    version: u8,
    market_time: i64,
    /// Encrypted at rest inside the corporate module. These are the exact
    /// registered policy inputs needed to replay without substituting whatever
    /// happens to be visible in the browser after an outage.
    policies: Vec<Policy>,
    request: Request,
    settlement: MpcSettlementInputs,
    admission: ExecuteAdmission,
    mandate_digest: [u8; 32],
    signed_taker_mandate: Vec<u8>,
    maximum_amount: u64,
    maximum_blinding: u64,
    response_mask: u64,
    fill_mask: u64,
    maker_authority_digest: [u8; 32],
    approval_domain: [u8; 32],
    approval: EntityApproval,
}

impl QueuedRfqEnvelope {
    fn statement(&self) -> Result<[u8; 32], String> {
        let body = serde_json::to_vec(&(
            self.version,
            self.market_time,
            &self.policies,
            &self.request,
            &self.settlement,
            &self.admission,
            self.mandate_digest,
            &self.signed_taker_mandate,
            self.maximum_amount,
            self.maximum_blinding,
            self.response_mask,
            self.fill_mask,
            self.maker_authority_digest,
            self.approval_domain,
        ))
        .map_err(|error| error.to_string())?;
        Ok(Sha256::new()
            .chain_update(b"QOMM:CORPORATE-QUEUED-RFQ:v1")
            .chain_update(body)
            .finalize()
            .into())
    }

    fn verify(
        &self,
        snapshot: &ParticipantSnapshot,
        expected_domain: [u8; 32],
        expected_maker_authority: [u8; 32],
        now: u64,
    ) -> Result<PreparedAdmission, String> {
        let mandate = self.verify_participant_authority(snapshot, expected_domain)?;
        if self.maker_authority_digest != expected_maker_authority || mandate.deadline <= now {
            return Err("queued RFQ mandate, authority, or deadline is inconsistent".into());
        }
        Ok(PreparedAdmission {
            envelope: self.admission.clone(),
            mandate_digest: self.mandate_digest,
            signed_mandate: Some(self.signed_taker_mandate.clone()),
            taker_mandate: Some(mandate),
            maximum_amount: self.maximum_amount,
            maximum_blinding: self.maximum_blinding,
            response_mask: self.response_mask,
            fill_mask: self.fill_mask,
        })
    }

    fn verify_expired(
        &self,
        snapshot: &ParticipantSnapshot,
        expected_domain: [u8; 32],
        now: u64,
    ) -> Result<TakerExecutionMandate, String> {
        let mandate = self.verify_participant_authority(snapshot, expected_domain)?;
        if now <= mandate.deadline {
            return Err("corporate RFQ is not yet eligible for expiry release".into());
        }
        Ok(mandate)
    }

    fn verify_participant_authority(
        &self,
        snapshot: &ParticipantSnapshot,
        expected_domain: [u8; 32],
    ) -> Result<TakerExecutionMandate, String> {
        if self.version != 3
            || self.request.is_real != 1
            || self.policies.is_empty()
            || self.approval_domain != expected_domain
            || self.approval.participant_id != snapshot.participant_id
            || self.approval.key_purpose != KeyPurpose::MpcInput
            || self.approval.key_epoch != 1
            || self.approval.statement != self.statement()?
            || self.signed_taker_mandate.is_empty()
        {
            return Err("queued corporate RFQ differs from its participant authority".into());
        }
        let public = snapshot
            .public_keys
            .get("mpc_input")
            .ok_or_else(|| "Taker participant has no MPC-input key".to_string())?;
        self.approval
            .verify_signature(
                &self.approval_domain,
                &defmi::participant::PurposeKey {
                    public_key: *public,
                    pq_public_key: snapshot
                        .pq_public_keys
                        .get("mpc_input")
                        .ok_or("missing enrolled PQ MPC input key")?
                        .clone(),
                    epoch: 1,
                },
            )
            .map_err(|error| error.to_string())?;
        let mandate = decode_taker_mandate(&self.signed_taker_mandate)?;
        if mandate.digest()? != self.mandate_digest
            || mandate.admission_slot != self.admission.slot
            || mandate.admission_ticket_id
                != decode_hex32(&self.admission.ticket_id, "queued admission ticket")?
            || self.admission.claim_digest != hex::encode(self.mandate_digest)
            || self.maximum_amount == 0
            || self.response_mask == 0
            || self.fill_mask == 0
            || mandate.fill_mask_commitment != fill_mask_commitment(self.fill_mask)
        {
            return Err("queued RFQ mandate or admission is inconsistent".into());
        }
        Ok(mandate)
    }
}

pub struct DistributedMpcEngine {
    endpoints: Vec<Endpoint>,
    n_parties: usize,
    threshold: usize,
    n_makers: usize,
    bit_length: u32,
    references: Vec<i128>,
    input_check: bool,
    config: ProgramConfig,
    source_sha256: String,
    served: u64,
    timeout: Duration,
    note: String,
    maker_handle_scalars: Vec<u64>,
    taker_handle_scalar: u64,
    pretrade_signer: Option<PretradeSigner>,
    maker_pretrade_signers: Vec<MakerPretradeSigner>,
    maker_authorities: Vec<Option<CachedMakerAuthority>>,
    maker_policy_versions: Vec<u64>,
    frost_public: Option<zkpi::frost::keys::PublicKeyPackage>,
    defmi_market: Option<DefmiMarketEpoch>,
    /// Set only by `replay_queued` after the participant module atomically
    /// claims the oldest request. `quote` consumes it exactly once.
    preclaimed_replay: Option<ClaimedCorporateRequest>,
    /// The corporate request of the last round that ended awaiting canonical
    /// reconciliation, whose Taker reserve the room keeps for the replay.
    /// Cleared when a later round completes or when canonical state shows the
    /// request finalized without a replay (see `retained_corporate_finalized`).
    retained_corporate: Option<(String, [u8; 32])>,
    /// When the retained request was last reconciled, so the 500 ms replay
    /// ticker asks the participant module about it at most every few seconds.
    retained_corporate_checked: Option<Instant>,
    /// Queue replay may reconstruct volatile openings and deterministic
    /// signatures, but it must never create a new verifier, facility, or
    /// standing pool after the Taker request was accepted.
    require_existing_maker_authority: bool,
    /// Whitespace-separated value count of one party input.  It is fixed by
    /// the compiled circuit and is part of every canonical proof-job identity.
    input_count_cache: Option<u64>,
}

pub struct TakerPretradeSignerConfig {
    pub endpoint: String,
    pub taker_participant_id: [u8; 32],
    pub venue_id: [u8; 32],
    pub defmi_id: [u8; 32],
    pub presentation: KybPresentation,
    pub registry: SignedCohortRegistry,
    pub trusted_issuer: qomm_proofs::kyb::KybIssuerKey,
    pub identity_scope: Vec<u8>,
    pub identity_context: Vec<u8>,
    pub required_cohort: String,
}

struct RoundInputGeneration<'a> {
    policies: &'a [Policy],
    request: &'a Request,
    settlement: &'a MpcSettlementInputs,
    now: i64,
    round_slot: u64,
    response_mask: u64,
    fill_mask: u64,
}

impl DistributedMpcEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoints: &[String],
        threshold: usize,
        n_makers: usize,
        references: &[i64],
        bit_length: u32,
        input_check: bool,
        timeout: Duration,
    ) -> Result<Self, String> {
        if endpoints.len() < 2 * threshold + 1 || endpoints.len() != 7 || threshold != 2 {
            return Err("the distributed demo requires the approved 3-of-7 MPC shape".into());
        }
        if timeout.is_zero() || timeout > Duration::from_secs(600) {
            return Err("distributed MPC timeout must be between zero and ten minutes".into());
        }
        let n_parties = endpoints.len();
        let endpoints = endpoints
            .iter()
            .map(|value| Endpoint::parse(value))
            .collect::<Result<Vec<_>, _>>()?;
        let padded = pow2_ceil(n_makers).map_err(|error| error.to_string())?;
        let config = ProgramConfig {
            n_mm: padded,
            n_parties,
            n_assets: references.len(),
            ref_table: references.iter().copied().map(i128::from).collect(),
            maker_assets: (0..padded)
                .map(|maker| maker % references.len().max(1))
                .collect(),
            bit_length,
            input_check,
            check_mode: CheckMode::PerParty,
            public_maker_assets: true,
            binding_limit: true,
            stop_after: StopAfter::Tournament,
            persist_wires: true,
            persist_zkpi_wires: true,
            persist_quote_proof_wires: true,
            persist_dvp_wires: true,
            zkpi_amount_bits: PRODUCT_ZKPI_AMOUNT_BITS,
            zkpi_price_bits: PRODUCT_ZKPI_PRICE_BITS,
            dvp_remainder_bits: PRODUCT_DVP_REMAINDER_BITS,
            quote_eligibility_bits: PRODUCT_QUOTE_ELIGIBILITY_BITS,
            quote_span_bits: PRODUCT_QUOTE_SPAN_BITS,
            reference: Reference::Anchored,
            ..ProgramConfig::default()
        };
        let source = build_program(&config).map_err(|error| error.to_string())?;
        let source_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
        Ok(Self {
            endpoints,
            n_parties,
            threshold,
            n_makers,
            bit_length,
            references: references.iter().copied().map(i128::from).collect(),
            input_check,
            config,
            source_sha256,
            served: 0,
            timeout,
            note: "stock MP-SPDZ malicious-Shamir, seven independent Docker services".into(),
            maker_handle_scalars: (0..n_makers)
                .map(|maker| 701_u64.saturating_add(maker as u64))
                .collect(),
            taker_handle_scalar: 799,
            pretrade_signer: None,
            maker_pretrade_signers: Vec::new(),
            maker_authorities: vec![None; n_makers],
            maker_policy_versions: vec![0; n_makers],
            frost_public: None,
            defmi_market: None,
            preclaimed_replay: None,
            retained_corporate: None,
            retained_corporate_checked: None,
            require_existing_maker_authority: false,
            input_count_cache: None,
        })
    }

    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    /// Pin the authoritative DeFMI market epoch and complete the resident
    /// FROST DKG before any Maker policy is published.  The resulting public
    /// package is reused for reserve and final-settlement zkPI signatures.
    pub fn bind_defmi_market(
        &mut self,
        rpc_endpoint: &str,
        chain_id: &str,
        venue_id: [u8; 32],
        defmi_id: [u8; 32],
    ) -> Result<(), String> {
        let frost_session: [u8; 32] = Sha256::new()
            .chain_update(b"QOMM:DEMO:FROST-COMMITTEE:v1")
            .chain_update(self.source_sha256.as_bytes())
            .finalize()
            .into();
        // The committee's durable FROST group is retrieved, not created, at
        // every boot, and the retrieval is a status read: it runs with a short
        // timeout so an unreachable committee is known in seconds rather than
        // after seven full MPC timeouts.  A gateway restarted while the
        // committee is unreachable then recalls the public package it recorded
        // at its last successful retrieval: the package is public, every later
        // use compares it with the verifier DeFMI holds, and signing and
        // execution still need every node, so nothing runs on a stale group.
        // Without a recorded package the boot fails as before; a committee
        // that answers but holds no group yet runs the DKG with the full
        // timeout.
        let quick_timeout = self.timeout.min(Duration::from_secs(10));
        let mut quick_parties = self
            .endpoints
            .iter()
            .cloned()
            .map(|endpoint| HttpProofPartyClient::new(endpoint, quick_timeout))
            .collect::<Vec<_>>();
        let public = match recall_frost_group(&mut quick_parties, frost_session) {
            Ok(Some(public)) => {
                if let Err(error) = record_frost_public(&frost_session, &public) {
                    eprintln!(
                        "qomm-demo: could not record the FROST public package for recall: {error}"
                    );
                }
                public
            }
            Ok(None) => {
                let mut proof_parties = self
                    .endpoints
                    .iter()
                    .cloned()
                    .map(|endpoint| HttpProofPartyClient::new(endpoint, self.timeout))
                    .collect::<Vec<_>>();
                let public = distributed_frost_setup(&mut proof_parties, frost_session)?;
                if let Err(error) = record_frost_public(&frost_session, &public) {
                    eprintln!(
                        "qomm-demo: could not record the FROST public package for recall: {error}"
                    );
                }
                public
            }
            Err(error) => match recall_frost_public(&frost_session)? {
                Some(public) => {
                    eprintln!(
                        "qomm-demo: MPC committee unreachable during FROST group retrieval ({error}); using the public package recorded at the last successful retrieval; no request executes until every node answers"
                    );
                    public
                }
                None => return Err(error),
            },
        };
        self.defmi_market = Some(DefmiMarketEpoch::connect(
            rpc_endpoint,
            chain_id,
            venue_id,
            defmi_id,
        )?);
        self.frost_public = Some(public);
        Ok(())
    }

    /// Bind the MPC Maker handles and the public Taker handle to the legal
    /// entities registered by DeFMI. Participant identifiers themselves never
    /// enter the MPC persistence files.
    pub fn bind_participants(
        &mut self,
        maker_participant_ids: &[[u8; 32]],
        taker_participant_id: [u8; 32],
    ) -> Result<(), String> {
        if maker_participant_ids.len() != self.n_makers
            || taker_participant_id == [0_u8; 32]
            || maker_participant_ids.contains(&[0_u8; 32])
        {
            return Err("DeFMI participant handles differ from the Maker/Taker shape".into());
        }
        self.maker_handle_scalars = maker_participant_ids
            .iter()
            .map(|participant| participant_handle_scalar(b"maker", participant))
            .collect();
        self.taker_handle_scalar = participant_handle_scalar(b"taker", &taker_participant_id);
        Ok(())
    }

    /// Install the entity-owned Taker signer used once, before the RFQ reaches
    /// any MPC service.  The participant module has no post-match settlement
    /// route, so the returned mandate is the complete consent boundary.
    pub fn bind_taker_pretrade_signer(
        &mut self,
        config: TakerPretradeSignerConfig,
    ) -> Result<(), String> {
        let TakerPretradeSignerConfig {
            endpoint,
            taker_participant_id,
            venue_id,
            defmi_id,
            presentation,
            registry,
            trusted_issuer,
            identity_scope,
            identity_context,
            required_cohort,
        } = config;
        if venue_id == [0_u8; 32] || defmi_id == [0_u8; 32] {
            return Err("Taker pre-trade signer needs non-zero venue and DeFMI ids".into());
        }
        let client = ParticipantClient::new(&endpoint, Duration::from_secs(15))?;
        let snapshot = client.snapshot()?;
        if snapshot.role != "taker" || snapshot.participant_id != taker_participant_id {
            return Err("Taker pre-trade signer belongs to another participant".into());
        }
        self.served = self.served.max(snapshot.corporate_outbox_next_sequence);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Taker identity verification time is before Unix epoch")?
            .as_secs();
        verify_presentation(
            &presentation,
            &registry,
            &trusted_issuer,
            &identity_scope,
            &identity_context,
            now,
            &required_cohort,
        )
        .map_err(|error| format!("Taker anonymous KYB proof was rejected: {error:?}"))?;
        self.pretrade_signer = Some(PretradeSigner {
            client,
            snapshot,
            venue_id,
            defmi_id,
            presentation,
            registry,
            trusted_issuer,
            identity_scope,
            identity_context,
            required_cohort,
        });
        Ok(())
    }

    /// Bind every standing Maker policy to an entity-owned quote key and an
    /// anonymous cohort proof.  The actual policy authorization is produced by
    /// `preauthorize_maker_policies`, which the room calls on installation and
    /// on every policy edit, never as a response to an RFQ.
    pub fn bind_maker_pretrade_signers(
        &mut self,
        endpoints: &[String],
        participant_ids: &[[u8; 32]],
        presentations: &[KybPresentation],
    ) -> Result<(), String> {
        if endpoints.len() != self.n_makers
            || participant_ids.len() != self.n_makers
            || presentations.len() != self.n_makers
        {
            return Err("Maker signer population differs from the compiled policy shape".into());
        }
        let identity = self.pretrade_signer.as_ref().ok_or_else(|| {
            "bind the Taker DeFMI identity context before Maker signers".to_string()
        })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Maker identity verification time is before Unix epoch")?
            .as_secs();
        let mut signers = Vec::with_capacity(self.n_makers);
        for (maker, ((endpoint, participant_id), presentation)) in endpoints
            .iter()
            .zip(participant_ids)
            .zip(presentations)
            .enumerate()
        {
            let client = ParticipantClient::new(endpoint, Duration::from_secs(15))?;
            let snapshot = client.snapshot()?;
            if snapshot.role != "maker" || snapshot.participant_id != *participant_id {
                return Err(format!(
                    "Maker {maker} signer belongs to another participant"
                ));
            }
            verify_presentation(
                presentation,
                &identity.registry,
                &identity.trusted_issuer,
                &identity.identity_scope,
                &identity.identity_context,
                now,
                &identity.required_cohort,
            )
            .map_err(|error| format!("Maker {maker} anonymous KYB proof failed: {error:?}"))?;
            signers.push(MakerPretradeSigner {
                client,
                snapshot,
                presentation: presentation.clone(),
            });
        }
        self.maker_pretrade_signers = signers;
        self.maker_authorities.fill(None);
        Ok(())
    }

    fn source_policy_digest(&self, maker: usize, policy: &Policy) -> [u8; 32] {
        let mut digest = Sha256::new()
            .chain_update(b"QOMM:DEMO:REGISTERED-MAKER-POLICY:v1")
            .chain_update(self.source_sha256.as_bytes())
            .chain_update((maker as u64).to_be_bytes());
        for field in policy.fields() {
            digest.update(field.to_be_bytes());
        }
        digest.finalize().into()
    }

    fn registered_policy(
        &self,
        maker: usize,
        policy: &Policy,
        blindings: &[u64; QUOTE_POLICY_BLINDING_FIELDS],
    ) -> Result<RegisteredPolicy, String> {
        let key = Pedersen::new(b"qomm:policy:v1");
        let values = [
            policy.ask_level,
            policy.spread,
            policy.slope,
            policy.invcoef,
            policy.inv,
            policy.maxqty,
            policy.expiry,
            policy.active,
            policy.use_ref,
        ];
        let commit = |field: usize| {
            key.commit(
                &signed_scalar(values[field]),
                &Scalar::from(blindings[field]),
            )
        };
        Ok(RegisteredPolicy {
            maker_asset: u32::try_from(policy.asset)
                .map_err(|_| format!("Maker {maker} asset exceeds u32"))?,
            ask_level: commit(0),
            spread: commit(1),
            slope: commit(2),
            invcoef: commit(3),
            inv: commit(4),
            maxqty: commit(5),
            expiry: commit(6),
            active: commit(7),
            use_ref: commit(8),
        })
    }

    fn policy_blindings(
        &self,
        maker: usize,
    ) -> Result<[u64; QUOTE_POLICY_BLINDING_FIELDS], String> {
        if let Some(cached) = self.maker_authorities.get(maker).and_then(Option::as_ref) {
            return Ok(cached.policy_blindings);
        }
        if self.pretrade_signer.is_some() {
            return Err(format!(
                "Maker {maker} has no registered policy commitment openings"
            ));
        }
        Ok(std::array::from_fn(|field| {
            1_001_u64.saturating_add((maker * QUOTE_POLICY_BLINDING_FIELDS + field) as u64)
        }))
    }

    fn registered_policy_registry_digest(&self, policies: &[Policy]) -> Result<[u8; 32], String> {
        let registered = policies
            .iter()
            .enumerate()
            .map(|(maker, policy)| {
                self.registered_policy(maker, policy, &self.policy_blindings(maker)?)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(registry_digest(&registered))
    }

    fn checked_maker_mandates(
        &self,
        policies: &[Policy],
        settlement: &MpcSettlementInputs,
        now: u64,
    ) -> Result<Vec<Vec<u8>>, String> {
        if self.maker_pretrade_signers.len() != self.n_makers {
            return Err("distributed execution has no complete Maker signer population".into());
        }
        let mut wires = Vec::new();
        for (maker, policy) in policies.iter().enumerate() {
            let cached = self.maker_authorities[maker]
                .as_ref()
                .ok_or_else(|| format!("Maker {maker} policy has no pre-RFQ authority"))?;
            let registered = self.registered_policy(maker, policy, &cached.policy_blindings)?;
            if cached.source_policy_digest != self.source_policy_digest(maker, policy)
                || cached.registered_policy_digest != registered_policy_digest(maker, &registered)
                || cached.inventory_reserve != settlement.maker_securities_reserves[maker]
                || cached.cash_reserve != settlement.maker_cash_reserves[maker]
                || cached.valid_until <= now
            {
                return Err(format!(
                    "Maker {maker} policy or reserve changed without a fresh standing mandate"
                ));
            }
            if policy.active != 0 && cached.signed_mandates.len() != 2 {
                return Err(format!(
                    "active Maker {maker} policy lacks both cash and inventory authority"
                ));
            }
            wires.extend(cached.signed_mandates.clone());
        }
        Ok(wires)
    }

    fn prepare_admission(
        &self,
        request: &Request,
        settlement: &MpcSettlementInputs,
        now: u64,
        canonical_sequence: Option<u64>,
    ) -> Result<PreparedAdmission, String> {
        let slot = self.served;
        let mask_width = self
            .bit_length
            .checked_add(1)
            .filter(|bits| *bits <= 64)
            .ok_or_else(|| "resident response mask width is outside u64".to_string())?;
        let draw_mask = || loop {
            // A 63-bit signed circuit value needs a full 64-bit response
            // mask.  `1_u64 << 64` is undefined, so the full-width case must
            // use the raw draw instead of constructing a 2^64 modulus.
            let raw = OsRng.next_u64();
            let value = if mask_width == 64 {
                raw
            } else {
                raw & ((1_u64 << mask_width) - 1)
            };
            if value != 0 {
                break value;
            }
        };
        let response_mask = draw_mask();
        let fill_mask = draw_mask();
        let slot_u32 = u32::try_from(slot)
            .map_err(|_| "distributed admission slot exceeds the node protocol".to_string())?;
        let sequence = match canonical_sequence {
            Some(sequence) if sequence != 0 => sequence,
            Some(_) => {
                return Err("a canonical admission sequence must be non-zero".into());
            }
            None => slot
                .checked_add(1)
                .ok_or_else(|| "distributed admission sequence overflowed".to_string())?,
        };
        let principal = self
            .pretrade_signer
            .as_ref()
            .map(|signer| hex::encode(signer.snapshot.participant_id))
            .unwrap_or_else(|| "qomm-cover-principal".into());
        let ticket_id = principal_ticket_id(slot_u32, &principal)?;
        let settlement_key = Pedersen::new(b"qomm:defmi:v1");
        let asset_id = traded_asset_id(request.asset);

        let (mandate_digest, signed_mandate, taker_mandate, maximum_amount, maximum_blinding) =
            if request.is_real == 1 {
                let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                    "a real distributed RFQ requires the entity-owned Taker pre-trade signer"
                        .to_string()
                })?;
                let direction = match request.direction {
                    0 => Direction::TakerBuys,
                    1 => Direction::TakerSells,
                    _ => return Err("distributed request direction is outside buy/sell".into()),
                };
                let quantity = u64::try_from(request.qty)
                    .map_err(|_| "Taker quantity is outside its commitment range".to_string())?;
                let limit = u64::try_from(settlement.user_limit)
                    .map_err(|_| "Taker limit is outside its commitment range".to_string())?;
                let (reserve_asset_id, maximum_amount, maximum_blinding) = match direction {
                    Direction::TakerBuys => (
                        cash_asset_id(),
                        u64::try_from(settlement.taker_cash_reserve)
                            .map_err(|_| "Taker cash reserve is outside u64".to_string())?,
                        self.served.saturating_add(251),
                    ),
                    Direction::TakerSells => (
                        asset_id,
                        u64::try_from(settlement.taker_securities_reserve)
                            .map_err(|_| "Taker inventory reserve is outside u64".to_string())?,
                        self.served.saturating_add(201),
                    ),
                };
                if maximum_amount == 0 {
                    return Err("Taker signed mandate cannot carry an empty reserve".into());
                }
                verify_presentation(
                    &signer.presentation,
                    &signer.registry,
                    &signer.trusted_issuer,
                    &signer.identity_scope,
                    &signer.identity_context,
                    now,
                    &signer.required_cohort,
                )
                .map_err(|error| format!("Taker anonymous KYB proof expired: {error:?}"))?;
                let entity_commitment = signer.presentation.entity_commitment();
                let kyb_presentation_digest = signer.presentation.binding_digest();
                let taker_public = signer.snapshot.settlement_application_key.to_bytes();
                let mandate = TakerExecutionMandate {
                    venue_id: signer.venue_id,
                    defmi_id: signer.defmi_id,
                    rfq_nullifier: Sha256::new()
                        .chain_update(b"QOMM:DEMO:RFQ-NULLIFIER:v1")
                        .chain_update(signer.snapshot.participant_id)
                        .chain_update(slot.to_be_bytes())
                        .finalize()
                        .into(),
                    asset_id,
                    reserve_asset_id,
                    direction,
                    quantity_commitment: settlement_key
                        .commit(
                            &Scalar::from(quantity),
                            &Scalar::from(self.served.saturating_add(151)),
                        )
                        .compress()
                        .to_bytes(),
                    limit_price_commitment: settlement_key
                        .commit(
                            &Scalar::from(limit),
                            &Scalar::from(self.served.saturating_add(101)),
                        )
                        .compress()
                        .to_bytes(),
                    maximum_fee_commitment: settlement_key
                        .commit(
                            &Scalar::from(0_u64),
                            &Scalar::from(self.served.saturating_add(181)),
                        )
                        .compress()
                        .to_bytes(),
                    maximum_amount_commitment: settlement_key
                        .commit(
                            &Scalar::from(maximum_amount),
                            &Scalar::from(maximum_blinding),
                        )
                        .compress()
                        .to_bytes(),
                    reserve_id: Sha256::new()
                        .chain_update(b"QOMM:DEMO:TAKER-RESERVE:v1")
                        .chain_update(signer.snapshot.participant_id)
                        .chain_update(slot.to_be_bytes())
                        .finalize()
                        .into(),
                    taker_handle: (settlement_key.g * Scalar::from(self.taker_handle_scalar))
                        .compress()
                        .to_bytes(),
                    entity_commitment,
                    kyb_presentation_digest,
                    admission_ticket_id: ticket_id,
                    admission_slot: slot,
                    fill_mask_commitment: fill_mask_commitment(fill_mask),
                    deadline: now.saturating_add(3_590),
                    allow_partial: false,
                    auto_settle: true,
                    taker_public,
                    signature: Signature::from_bytes(&[0_u8; 64]),
                };
                let mandate = signer
                    .client
                    .sign_taker_mandate(&signer.snapshot, &mandate)?;
                mandate.verify(
                    &signer.presentation,
                    &signer.registry,
                    &signer.trusted_issuer,
                    &signer.identity_scope,
                    &signer.identity_context,
                    &signer.required_cohort,
                    now,
                )?;
                let digest = mandate.digest()?;
                let encoded = encode_taker_mandate(&mandate)?;
                (
                    digest,
                    Some(encoded),
                    Some(mandate),
                    maximum_amount,
                    maximum_blinding,
                )
            } else {
                let digest = Sha256::new()
                    .chain_update(b"QOMM:DEMO:COVER-ADMISSION-CLAIM:v1")
                    .chain_update(slot.to_be_bytes())
                    .chain_update(ticket_id)
                    .finalize()
                    .into();
                (digest, None, None, 0, 0)
            };
        let order_digest: [u8; 32] = Sha256::new()
            .chain_update(b"QOMM:DEMO:FIXED-SLOT-ORDER:v1")
            .chain_update(slot.to_be_bytes())
            .chain_update(sequence.to_be_bytes())
            .chain_update(ticket_id)
            .finalize()
            .into();
        Ok(PreparedAdmission {
            envelope: ExecuteAdmission {
                slot,
                sequence,
                principal,
                ticket_id: hex::encode(ticket_id),
                claim_digest: hex::encode(mandate_digest),
                order_digest: hex::encode(order_digest),
            },
            mandate_digest,
            signed_mandate,
            taker_mandate,
            maximum_amount,
            maximum_blinding,
            response_mask,
            fill_mask,
        })
    }

    fn generate(
        &self,
        generation: RoundInputGeneration<'_>,
    ) -> Result<GeneratedRoundInputs, String> {
        let RoundInputGeneration {
            policies,
            request,
            settlement,
            now,
            round_slot,
            response_mask,
            fill_mask,
        } = generation;
        if policies.len() != self.n_makers {
            return Err("the live policy count differs from the compiled distributed shape".into());
        }
        settlement.validate(self.n_makers)?;
        for (maker, policy) in policies.iter().enumerate() {
            if usize::try_from(policy.asset).ok() != self.config.maker_assets.get(maker).copied() {
                return Err(format!(
                    "Maker {maker} changed the instrument bound to the registered proof circuit"
                ));
            }
        }
        let policy_json = serde_json::to_string(policies).map_err(|error| error.to_string())?;
        let mpc_policies = parse_policies(&policy_json).map_err(|error| error.to_string())?;
        let maker_policy_blindings = (0..self.config.n_mm)
            .map(|maker| {
                let values = if maker < self.n_makers {
                    self.policy_blindings(maker)?
                } else {
                    std::array::from_fn(|field| {
                        9_001_u64
                            .saturating_add((maker * QUOTE_POLICY_BLINDING_FIELDS + field) as u64)
                            .max(1)
                    })
                };
                Ok(values.map(i128::from))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let config = InputConfig {
            n_mm: self.config.n_mm,
            n_real_mm: self.n_makers,
            n_parties: self.n_parties,
            is_real: i128::from(request.is_real),
            n_requests: 1,
            n_assets: self.references.len(),
            ref_table: &self.references,
            user_asset: usize::try_from(request.asset)
                .map_err(|_| "request asset is negative".to_string())?,
            user_qty: i128::from(request.qty),
            user_dir: i128::from(request.direction),
            user_entity: i128::from(request.entity),
            now_t: i128::from(now),
            seed: i128::from(round_slot.saturating_add(7)),
            audit_gates: self.config.audit_gates,
            value_bits: self.bit_length + 1,
            field_bits: 253,
            use_ref: 1,
            reference: self.config.reference,
            input_check: self.input_check,
            check_mode: self.config.check_mode,
            binding_limit: true,
            user_limit: i128::from(settlement.user_limit),
            user_limit_blinding: i128::from(round_slot.saturating_add(101)),
            user_qty_blinding: i128::from(round_slot.saturating_add(151)),
            response_mask: Some(i128::from(response_mask)),
            fill_mask: Some(i128::from(fill_mask)),
            check_coefficients: &self.config.check_coefficients,
            check_repeats: self.config.check_repeats,
            policies: Some(&mpc_policies),
            shamir_inputs: false,
            shamir_threshold: self.threshold,
            // The standing Maker segment is node-resident: every MPC node
            // splices its own encrypted shares of the current pool remainder
            // over these placeholders before it starts MP-SPDZ.  The
            // coordinator therefore never carries a Maker reserve opening, and
            // a restarted coordinator cannot re-inject an initial balance.
            dvp: Some(DvpInputs {
                taker_securities_reserve: i128::from(settlement.taker_securities_reserve),
                taker_securities_blinding: i128::from(round_slot.saturating_add(201)),
                taker_cash_reserve: i128::from(settlement.taker_cash_reserve),
                taker_cash_blinding: i128::from(round_slot.saturating_add(251)),
                maker_securities_reserves: vec![0; self.config.n_mm],
                maker_securities_blindings: vec![0; self.config.n_mm],
                maker_cash_reserves: vec![0; self.config.n_mm],
                maker_cash_blindings: vec![0; self.config.n_mm],
                maker_handle_scalars: vec![0; self.config.n_mm],
            }),
            quote_proof: Some(QuoteProofInputs {
                maker_policy_blindings,
            }),
        };
        let mut generated = build_inputs(&config).map_err(|error| error.to_string())?;
        let max_reference = self.references.iter().copied().max().unwrap_or(0);
        let sentinel = sentinel_for(self.bit_length, self.config.n_mm, 8 * max_reference)
            .map_err(|error| error.to_string())?;
        finish_reference(&mut generated, &config, sentinel, Mode::Rfq)
            .map_err(|error| error.to_string())?;
        let reference: Value =
            serde_json::from_str(&generated.reference_json()).map_err(|error| error.to_string())?;
        let mask = json_u64(
            reference
                .get("mask")
                .ok_or_else(|| "generated reference has no mask".to_string())?,
        )?;
        let party_files = generated.party_files();
        let node_shares = party_files
            .iter()
            .enumerate()
            .map(|(party, contents)| {
                Ok((
                    party,
                    contents
                        .split_whitespace()
                        .take(6)
                        .map(low_72_hex)
                        .collect::<Result<Vec<_>, String>>()?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        Ok((party_files, reference, mask, node_shares))
    }

    fn release_expired_corporate(
        &self,
        signer: &PretradeSigner,
        request_id: &str,
        request_digest: [u8; 32],
        mut signed_request: Vec<u8>,
        wall_now: u64,
    ) -> Result<(), String> {
        if signed_request.is_empty()
            || request_digest != <[u8; 32]>::from(Sha256::digest(&signed_request))
        {
            signed_request.fill(0);
            return Err("expired corporate RFQ differs from its durable digest".into());
        }
        let queued = serde_json::from_slice::<QueuedRfqEnvelope>(&signed_request)
            .map_err(|_| "expired corporate RFQ is not canonical JSON".to_string());
        signed_request.fill(0);
        let queued = queued?;
        let mandate = queued.verify_expired(&signer.snapshot, signer.defmi_id, wall_now)?;
        if request_id != hex::encode(mandate.rfq_nullifier) {
            return Err(
                "expired corporate request id differs from its signed RFQ nullifier".into(),
            );
        }

        let before = signer.client.reconcile_corporate_request(
            &signer.snapshot,
            request_id,
            request_digest,
        )?;
        if before.hold_id != mandate.reserve_id {
            return Err("expired corporate request resolves to another DeFMI hold".into());
        }
        if before.queue_finalized {
            if !matches!(
                before.status.as_str(),
                "released" | "aborted_before_reserve"
            ) {
                return Err(
                    "expired corporate RFQ was finalized by a non-release transition".into(),
                );
            }
            return Ok(());
        }
        // The queue marks an unreserved request expired locally. There is no
        // DeFMI value to recover and claim_cover_slot will not dispatch it
        // again, so no synthetic ledger receipt is manufactured here.
        if before.status == "not_reserved" {
            signer.client.abort_corporate_before_reserve(
                &signer.snapshot,
                request_id,
                request_digest,
            )?;
            return Ok(());
        }
        if before.status != "active" {
            return Err("expired corporate RFQ has no releasable DeFMI reservation".into());
        }

        let market = self
            .defmi_market
            .as_ref()
            .ok_or_else(|| "expired corporate RFQ has no authoritative DeFMI market".to_string())?;
        let released = market.release_expired_taker(
            &signer.snapshot,
            &mandate,
            queued.maximum_amount,
            queued.maximum_blinding,
            wall_now,
        )?;
        if released.hold_id != mandate.reserve_id || released.status != "released" {
            return Err("DeFMI returned another reservation after expiry release".into());
        }
        let after = signer.client.reconcile_corporate_request(
            &signer.snapshot,
            request_id,
            request_digest,
        )?;
        if after.status != "released"
            || !after.queue_finalized
            || after.hold_id != mandate.reserve_id
            || after.state_root != released.state_root
        {
            return Err("corporate outbox did not record the canonical expiry release".into());
        }
        Ok(())
    }
}

/// Coordinator side of the node-resident Maker state.
///
/// The coordinator never holds a current pool opening.  It only (1) deals the
/// initial opening of a pool it registered itself, (2) tells each node which of
/// the node's own executions canonical DeFMI accepted, and (3) audits that the
/// seven partial commitments still add up to the canonical pool note.
impl DistributedMpcEngine {
    fn circuit_input_count(&mut self, policies: &[Policy]) -> Result<u64, String> {
        if let Some(count) = self.input_count_cache {
            return Ok(count);
        }
        let probe = Request {
            asset: 0,
            qty: 1,
            direction: 0,
            entity: 0,
            is_real: 0,
        };
        let settlement = MpcSettlementInputs {
            user_limit: 1,
            taker_securities_reserve: 0,
            taker_cash_reserve: 0,
            maker_securities_reserves: vec![0; self.n_makers],
            maker_cash_reserves: vec![0; self.n_makers],
        };
        let (party_files, _, _, _) = self.generate(RoundInputGeneration {
            policies,
            request: &probe,
            settlement: &settlement,
            now: 1,
            round_slot: 0,
            response_mask: 1,
            fill_mask: 1,
        })?;
        let count = party_files
            .first()
            .map(|input| input.split_whitespace().count())
            .ok_or_else(|| "circuit probe produced no party input".to_string())?;
        let count = u64::try_from(count).map_err(|_| "party input count exceeds u64")?;
        self.input_count_cache = Some(count);
        Ok(count)
    }

    fn maker_state_views(&self) -> Result<Vec<MakerStateView>, String> {
        let views = thread::scope(|scope| {
            self.endpoints
                .iter()
                .enumerate()
                .map(|(node, endpoint)| {
                    let timeout = self.timeout;
                    scope.spawn(move || {
                        let value = endpoint.get("/v1/maker-state", timeout)?;
                        serde_json::from_value::<MakerStateView>(value).map_err(|error| {
                            format!("MPC node {node} returned an invalid Maker-state view: {error}")
                        })
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "Maker-state client panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        for (node, view) in views.iter().enumerate() {
            if view.node != node
                || view.source_sha256 != self.source_sha256
                || view.sharing != MAKER_STATE_SHARING
                || view.n_mm != self.config.n_mm
            {
                return Err(format!(
                    "MPC node {node} reports Maker state for another node, circuit, or sharing"
                ));
            }
        }
        Ok(views)
    }

    fn node_round_receipts(&self) -> Result<Vec<Vec<NodeRoundReceiptView>>, String> {
        thread::scope(|scope| {
            self.endpoints
                .iter()
                .enumerate()
                .map(|(node, endpoint)| {
                    let timeout = self.timeout;
                    scope.spawn(move || {
                        let value = endpoint.get("/v1/rounds", timeout)?;
                        let receipts = value
                            .get("receipts")
                            .cloned()
                            .ok_or_else(|| format!("MPC node {node} returned no receipt list"))?;
                        serde_json::from_value::<Vec<NodeRoundReceiptView>>(receipts).map_err(
                            |error| {
                                format!("MPC node {node} returned an invalid receipt list: {error}")
                            },
                        )
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "round-receipt client panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })
    }

    /// The standing pools the current Maker authorities allocate from, in the
    /// circuit's rail order.
    fn bound_standing_pools(&self) -> Vec<(usize, u8, CachedMakerPool)> {
        let mut pools = Vec::new();
        for (maker, authority) in self.maker_authorities.iter().enumerate() {
            let Some(authority) = authority else {
                continue;
            };
            for pool in &authority.standing_pools {
                pools.push((maker, standing_rail(pool.mandate.direction), pool.clone()));
            }
        }
        pools
    }

    /// Deal the initial opening of every registered pool to the seven nodes.
    /// Only a node set with no resident state at all may be seeded.
    fn seed_maker_state(&self) -> Result<(), String> {
        let mut per_node = vec![Vec::with_capacity(5 * self.config.n_mm); self.n_parties];
        for maker in 0..self.config.n_mm {
            let values: [Scalar; 5] = match self
                .maker_authorities
                .get(maker)
                .and_then(Option::as_ref)
            {
                Some(authority) => [
                    Scalar::from(
                        u64::try_from(authority.inventory_reserve)
                            .map_err(|_| format!("Maker {maker} inventory reserve exceeds u64"))?,
                    ),
                    Scalar::from(authority.inventory_reserve_blinding),
                    Scalar::from(
                        u64::try_from(authority.cash_reserve)
                            .map_err(|_| format!("Maker {maker} cash reserve exceeds u64"))?,
                    ),
                    Scalar::from(authority.cash_reserve_blinding),
                    Scalar::from(self.maker_handle_scalars[maker]),
                ],
                None if maker >= self.n_makers => [Scalar::ZERO; 5],
                None => return Err(format!("Maker {maker} has no preauthorized policy to seed")),
            };
            for value in values {
                for (node, share) in deal_additive_shares(&value, self.n_parties)
                    .into_iter()
                    .enumerate()
                {
                    per_node[node].push(share);
                }
            }
        }
        let bindings = self
            .bound_standing_pools()
            .into_iter()
            .map(|(maker, direction, pool)| StandingPoolBinding {
                maker,
                direction,
                pool_id: pool.pool_id,
                pool_sequence: 0,
            })
            .collect::<Vec<_>>();
        thread::scope(|scope| {
            self.endpoints
                .iter()
                .zip(per_node)
                .enumerate()
                .map(|(node, (endpoint, dvp_input_shares))| {
                    let timeout = self.timeout;
                    let request = MakerStateSeedRequest {
                        version: PROTOCOL_VERSION,
                        node,
                        source_sha256: self.source_sha256.clone(),
                        dvp_input_shares,
                        bindings: bindings.clone(),
                    };
                    scope.spawn(move || {
                        endpoint
                            .post(
                                "/v1/maker-state/seed",
                                &serde_json::to_value(request)
                                    .map_err(|error| error.to_string())?,
                                timeout,
                            )
                            .map(|_| ())
                            .map_err(|error| {
                                format!("MPC node {node} refused its Maker-state seed: {error}")
                            })
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "Maker-state seed client panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        Ok(())
    }

    /// Seed the nodes if none of them holds a state yet.  Called after the
    /// standing pools exist on canonical DeFMI; partial seeding fails closed.
    /// Every node answers `/health` with the gateway's circuit digest.  The
    /// same test gates the corporate queue: a request is claimed for dispatch
    /// only when this holds, and stays `queued` otherwise.
    fn committee_healthy(&self) -> bool {
        let health_timeout = self.timeout.min(Duration::from_secs(2));
        thread::scope(|scope| {
            self.endpoints
                .iter()
                .map(|endpoint| {
                    scope.spawn(|| endpoint.healthy(&self.source_sha256, health_timeout))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .all(|handle| handle.join().unwrap_or(false))
        })
    }

    fn ensure_maker_state_seeded(&self) -> Result<bool, String> {
        let views = self.maker_state_views()?;
        let initialized = views.iter().filter(|view| view.initialized).count();
        if initialized == views.len() {
            return Ok(false);
        }
        if initialized != 0 {
            return Err(
                "only part of the MPC node set holds resident Maker state; repair the missing nodes before dispatching RFQs"
                    .into(),
            );
        }
        self.seed_maker_state()?;
        Ok(true)
    }

    /// Find the execution whose remainder became `canonical`'s current pool
    /// note.  The note id commits to the proof job id, and the job id commits
    /// to all seven nodes' input, output, and persistence digests, so a match
    /// identifies the exact persistence files each node must commit from.
    fn locate_accepted_execution(
        &self,
        receipts: &[Vec<NodeRoundReceiptView>],
        pool: &CachedMakerPool,
        canonical_note_id: [u8; 32],
        remainder_commitment: [u8; 32],
        input_count: u64,
    ) -> Result<Option<(String, u32, [u8; 32])>, String> {
        let source_digest = decode_hex32(&self.source_sha256, "MPC source digest")?;
        let manifest = qomm_manifest_v1();
        let Some(first) = receipts.first() else {
            return Ok(None);
        };
        for candidate in first {
            let mut nodes = Vec::with_capacity(self.n_parties);
            let mut complete = true;
            for node_receipts in receipts {
                let Some(receipt) = node_receipts.iter().find(|receipt| {
                    receipt.round_id == candidate.round_id
                        && receipt.execution_generation == candidate.execution_generation
                }) else {
                    complete = false;
                    break;
                };
                nodes.push(ExecutionNodeDigest {
                    batch_digest: decode_hex32(&receipt.input_sha256, "MPC input digest")?,
                    source_digest,
                    stdout_digest: decode_hex32(&receipt.stdout_sha256, "MPC stdout digest")?,
                    stderr_digest: decode_hex32(&receipt.stderr_sha256, "MPC stderr digest")?,
                    persistence_digest: decode_hex32(
                        &receipt.persistence_sha256,
                        "MPC persistence digest",
                    )?,
                });
            }
            if !complete || candidate.sequence == 0 {
                continue;
            }
            let plan = ApplicationExecutionPlan::new(
                &manifest,
                ExecutionShape {
                    lane: execution_lane_for_admission(candidate.sequence)?,
                    slot: candidate.slot,
                    generation: u64::from(candidate.execution_generation),
                    frame_count: 1,
                    input_count: if candidate.input_count == 0 {
                        input_count
                    } else {
                        candidate.input_count
                    },
                    order_digest: decode_hex32(&candidate.order_digest, "admission order")?,
                    nodes,
                },
            )
            .map_err(|error| error.to_string())?;
            let job_id = plan.job_id();
            let note = allocation_note(
                pool.mandate.asset_id,
                remainder_commitment,
                pool.pool_id,
                job_id,
                b"remainder",
            )?;
            if note.note_id == canonical_note_id {
                return Ok(Some((
                    candidate.round_id.clone(),
                    candidate.execution_generation,
                    job_id,
                )));
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_maker_state_on_nodes(
        &self,
        nodes: &[usize],
        expected_generations: &[u64],
        round_id: &str,
        execution_generation: u32,
        maker: usize,
        direction: u8,
        job_id: [u8; 32],
        remainder_note_id: [u8; 32],
        pool_id: [u8; 32],
        pool_sequence: u64,
    ) -> Vec<Result<MakerStateCommitResponse, String>> {
        thread::scope(|scope| {
            nodes
                .iter()
                .map(|node| {
                    let node = *node;
                    let endpoint = &self.endpoints[node];
                    let timeout = self.timeout;
                    let request = MakerStateCommitRequest {
                        version: PROTOCOL_VERSION,
                        node,
                        source_sha256: self.source_sha256.clone(),
                        round_id: round_id.to_string(),
                        execution_generation,
                        maker,
                        direction,
                        expected_generation: expected_generations[node],
                        proof_job_id: hex::encode(job_id),
                        remainder_note_id: hex::encode(remainder_note_id),
                        pool_id: hex::encode(pool_id),
                        pool_sequence,
                    };
                    scope.spawn(move || {
                        let value = endpoint.post(
                            "/v1/maker-state/commit",
                            &serde_json::to_value(request).map_err(|error| error.to_string())?,
                            timeout,
                        )?;
                        let response: MakerStateCommitResponse = serde_json::from_value(value)
                            .map_err(|error| {
                                format!(
                                    "MPC node {node} returned an invalid commit receipt: {error}"
                                )
                            })?;
                        response.receipt.verify()?;
                        if usize::from(response.receipt.node) != node
                            || response.receipt.maker != maker
                            || response.receipt.direction != direction
                            || response.receipt.proof_job_id != job_id
                            || response.receipt.allocation_statement != remainder_note_id
                        {
                            return Err(format!(
                                "MPC node {node} committed another allocation than requested"
                            ));
                        }
                        Ok(response)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "Maker-state commit client panicked".to_string())?
                })
                .collect()
        })
    }

    fn rebind_maker_state_on_nodes(
        &self,
        maker: usize,
        direction: u8,
        pool_id: [u8; 32],
        amount: u64,
        blinding: u64,
        expected_generations: &[u64],
    ) -> Result<(), String> {
        let amount_shares = deal_additive_shares(&Scalar::from(amount), self.n_parties);
        let blinding_shares = deal_additive_shares(&Scalar::from(blinding), self.n_parties);
        thread::scope(|scope| {
            self.endpoints
                .iter()
                .enumerate()
                .map(|(node, endpoint)| {
                    let timeout = self.timeout;
                    let request = MakerStateRebindRequest {
                        version: PROTOCOL_VERSION,
                        node,
                        source_sha256: self.source_sha256.clone(),
                        maker,
                        direction,
                        expected_generation: expected_generations[node],
                        pool_id: hex::encode(pool_id),
                        amount_share: amount_shares[node].clone(),
                        blinding_share: blinding_shares[node].clone(),
                    };
                    scope.spawn(move || {
                        endpoint
                            .post(
                                "/v1/maker-state/rebind",
                                &serde_json::to_value(request)
                                    .map_err(|error| error.to_string())?,
                                timeout,
                            )
                            .map(|_| ())
                            .map_err(|error| {
                                format!("MPC node {node} refused the pool rebind: {error}")
                            })
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| "Maker-state rebind client panicked".to_string())?
                })
                .collect::<Result<Vec<_>, String>>()
        })?;
        Ok(())
    }

    /// Bring every node's resident Maker state into agreement with canonical
    /// DeFMI before an RFQ executes, then audit it.
    ///
    /// * A pool the node has never seen, or has seen at an older identity, is
    ///   rebound with a fresh dealing of the registration opening while the
    ///   pool is still at sequence zero.
    /// * A pool DeFMI has allocated from is caught up from the node's own
    ///   persistence of the accepted execution, located through the remainder
    ///   note.  This covers a coordinator crash between DeFMI acceptance and
    ///   the node commit, a coordinator restart, and a node that missed a
    ///   commit while its peers advanced.
    /// * Finally the seven partial commitments must equal the canonical pool
    ///   note commitment for every pool; otherwise no RFQ is dispatched.
    fn reconcile_maker_state(&mut self, policies: &[Policy]) -> Result<Value, String> {
        let input_count = self.circuit_input_count(policies)?;
        let market = self.defmi_market.as_ref().ok_or_else(|| {
            "Maker-state reconciliation has no authoritative DeFMI market".to_string()
        })?;
        if self.ensure_maker_state_seeded()? {
            println!(
                "qomm-demo: seeded resident Maker state on all {} MPC nodes",
                self.n_parties
            );
        }
        let mut views = self.maker_state_views()?;
        let mut actions = Vec::new();
        let mut round_receipts: Option<Vec<Vec<NodeRoundReceiptView>>> = None;
        for (maker, direction, pool) in self.bound_standing_pools() {
            let canonical = market.standing_note_pool(pool.pool_id)?;
            let parent_note = market.note_output(canonical.current_pool_note_id)?;
            let pool_hex = hex::encode(pool.pool_id);
            let generations = views.iter().map(|view| view.generation).collect::<Vec<_>>();
            let mut behind = Vec::new();
            for view in &views {
                match view.binding(maker, direction) {
                    Some(binding)
                        if binding.pool_id == pool_hex
                            && binding.pool_sequence == canonical.sequence => {}
                    Some(binding)
                        if binding.pool_id == pool_hex
                            && binding.pool_sequence > canonical.sequence =>
                    {
                        return Err(format!(
                            "MPC node {} is ahead of canonical DeFMI for Maker {maker} pool {}; refusing to execute",
                            view.node, &pool_hex[..16]
                        ));
                    }
                    _ => behind.push(view.node),
                }
            }
            if !behind.is_empty() {
                if canonical.sequence == 0 {
                    // Never allocated from: the registration opening is the
                    // current opening and the coordinator registered it.
                    let authority = self.maker_authorities[maker].as_ref().ok_or_else(|| {
                        format!("Maker {maker} lost its authority during reconciliation")
                    })?;
                    let (amount, blinding) = match direction {
                        0 => (
                            authority.inventory_reserve,
                            authority.inventory_reserve_blinding,
                        ),
                        _ => (authority.cash_reserve, authority.cash_reserve_blinding),
                    };
                    let amount = u64::try_from(amount)
                        .map_err(|_| format!("Maker {maker} reserve is outside u64"))?;
                    self.rebind_maker_state_on_nodes(
                        maker,
                        direction,
                        pool.pool_id,
                        amount,
                        blinding,
                        &generations,
                    )?;
                    actions.push(json!({
                        "action": "rebind",
                        "maker": maker,
                        "direction": direction,
                        "pool_id": pool_hex,
                        "nodes": (0..self.n_parties).collect::<Vec<_>>(),
                    }));
                } else {
                    if round_receipts.is_none() {
                        round_receipts = Some(self.node_round_receipts()?);
                    }
                    let receipts = round_receipts
                        .as_ref()
                        .expect("round receipts were just fetched");
                    let (round_id, execution_generation, job_id) = self
                        .locate_accepted_execution(
                            receipts,
                            &pool,
                            canonical.current_pool_note_id,
                            parent_note.value_commitment,
                            input_count,
                        )?
                        .ok_or_else(|| {
                            format!(
                                "Maker {maker} pool {} is at DeFMI sequence {} but no MPC node set still holds the accepted execution; the pool needs operator re-registration",
                                &pool_hex[..16], canonical.sequence
                            )
                        })?;
                    let results = self.commit_maker_state_on_nodes(
                        &behind,
                        &generations,
                        &round_id,
                        execution_generation,
                        maker,
                        direction,
                        job_id,
                        canonical.current_pool_note_id,
                        pool.pool_id,
                        canonical.sequence,
                    );
                    for (node, result) in behind.iter().zip(&results) {
                        if let Err(error) = result {
                            return Err(format!(
                                "MPC node {node} could not catch up Maker {maker} pool {}: {error}",
                                &pool_hex[..16]
                            ));
                        }
                    }
                    actions.push(json!({
                        "action": "catch_up",
                        "maker": maker,
                        "direction": direction,
                        "pool_id": pool_hex,
                        "pool_sequence": canonical.sequence,
                        "round_id": round_id,
                        "execution_generation": execution_generation,
                        "proof_job_id": hex::encode(job_id),
                        "nodes": behind,
                    }));
                }
                views = self.maker_state_views()?;
            }
            // Audit: the seven shares must still open the canonical pool note.
            let partials = views
                .iter()
                .map(|view| {
                    view.binding(maker, direction)
                        .filter(|binding| {
                            binding.pool_id == pool_hex && binding.pool_sequence == canonical.sequence
                        })
                        .ok_or_else(|| {
                            format!(
                                "MPC node {} still lacks Maker {maker} pool {} after reconciliation",
                                view.node, &pool_hex[..16]
                            )
                        })
                        .and_then(|binding| {
                            decode_hex32(&binding.partial_commitment, "partial commitment")
                        })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let combined = combine_partial_commitments(
                &partials,
                MAKER_STATE_SHARING,
                u16::try_from(self.n_parties).map_err(|_| "party count exceeds u16")?,
            )?;
            if combined != parent_note.value_commitment {
                return Err(format!(
                    "resident MPC Maker state for Maker {maker} pool {} does not open the canonical DeFMI pool note at sequence {}; refusing to execute",
                    &pool_hex[..16], canonical.sequence
                ));
            }
        }
        Ok(json!({
            "sharing": MAKER_STATE_SHARING,
            "generations": views.iter().map(|view| view.generation).collect::<Vec<_>>(),
            "pools": views
                .first()
                .map(|view| view.bindings.len())
                .unwrap_or(0),
            "actions": actions,
        }))
    }
}

impl MpcQuoteEngine for DistributedMpcEngine {
    fn name(&self) -> &'static str {
        "mpc"
    }

    fn note(&self) -> String {
        self.note.clone()
    }

    fn robust(&self) -> bool {
        false
    }

    fn robust_reason(&self) -> &str {
        "3-of-7 malicious-Shamir detects invalid execution; n=7 is below the n>=4T+1 correction shape"
    }

    fn input_check(&self) -> bool {
        self.input_check
    }

    fn preauthorize_maker_policies(
        &mut self,
        policies: &[Policy],
        settlement: &MpcSettlementInputs,
        _now: i64,
    ) -> Result<(), String> {
        if self.pretrade_signer.is_none() {
            // A distributed MPC-only laboratory can still be installed, but a
            // real RFQ will fail closed in `prepare_admission`.  No placeholder
            // Maker authority is created.
            return Ok(());
        }
        if policies.len() != self.n_makers || self.maker_pretrade_signers.len() != self.n_makers {
            return Err("Maker policy, signer, and compiled circuit populations differ".into());
        }
        settlement.validate(self.n_makers)?;
        let identity = self
            .pretrade_signer
            .as_ref()
            .expect("checked DeFMI identity context")
            .clone();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "Maker policy authorization time is before Unix epoch")?
            .as_secs();
        let valid_until = DEMO_MAKER_VALID_UNTIL;
        let settlement_key = Pedersen::new(b"qomm:defmi:v1");
        for (maker, policy) in policies.iter().enumerate() {
            if usize::try_from(policy.asset).ok() != self.config.maker_assets.get(maker).copied() {
                return Err(format!(
                    "Maker {maker} changed the instrument bound to the registered proof circuit"
                ));
            }
            let source_policy_digest = self.source_policy_digest(maker, policy);
            let inventory_reserve = settlement.maker_securities_reserves[maker];
            let cash_reserve = settlement.maker_cash_reserves[maker];
            if !self.require_existing_maker_authority
                && self.maker_authorities[maker]
                    .as_ref()
                    .is_some_and(|cached| {
                        cached.source_policy_digest == source_policy_digest
                            && cached.inventory_reserve == inventory_reserve
                            && cached.cash_reserve == cash_reserve
                            && cached.valid_until > now.saturating_add(30)
                    })
            {
                continue;
            }
            let blinding_domain: [u8; 32] = Sha256::new()
                .chain_update(b"QOMM:MAKER:POLICY-BLINDING-DOMAIN:v1")
                .chain_update(identity.venue_id)
                .chain_update(identity.defmi_id)
                .finalize()
                .into();
            let policy_seed = self.maker_pretrade_signers[maker].client.entity_approval(
                &self.maker_pretrade_signers[maker].snapshot,
                blinding_domain,
                KeyPurpose::Quote,
                source_policy_digest,
            )?;
            let policy_version = participant_derived_u64(&policy_seed, b"policy-version");
            self.maker_policy_versions[maker] = policy_version;
            let mut policy_blindings = [0_u64; QUOTE_POLICY_BLINDING_FIELDS];
            for (field, blinding) in policy_blindings.iter_mut().enumerate() {
                *blinding = participant_derived_u64(
                    &policy_seed,
                    format!("policy-field-{field}").as_bytes(),
                );
            }
            let registered = self.registered_policy(maker, policy, &policy_blindings)?;
            let policy_digest = registered_policy_digest(maker, &registered);
            let inventory_reserve_blinding =
                participant_derived_u64(&policy_seed, b"inventory-reserve");
            let cash_reserve_blinding = participant_derived_u64(&policy_seed, b"cash-reserve");
            if policy.active == 0 {
                self.maker_authorities[maker] = Some(CachedMakerAuthority {
                    source_policy_digest,
                    registered_policy_digest: policy_digest,
                    policy_blindings,
                    inventory_reserve,
                    inventory_reserve_blinding,
                    cash_reserve,
                    cash_reserve_blinding,
                    valid_until,
                    signed_mandates: Vec::new(),
                    standing_pools: Vec::new(),
                });
                continue;
            }
            if inventory_reserve <= 0 || cash_reserve <= 0 {
                return Err(format!(
                    "active Maker {maker} policy must reserve both inventory and cash before publication"
                ));
            }
            let signer = self.maker_pretrade_signers[maker].clone();
            verify_presentation(
                &signer.presentation,
                &identity.registry,
                &identity.trusted_issuer,
                &identity.identity_scope,
                &identity.identity_context,
                now,
                &identity.required_cohort,
            )
            .map_err(|error| format!("Maker {maker} anonymous KYB proof failed: {error:?}"))?;
            let maker_public = signer.snapshot.quote_application_key.to_bytes();
            let traded_asset_id = traded_asset_id(policy.asset);
            let cash_asset_id = cash_asset_id();
            let mut signed_mandates = Vec::with_capacity(2);
            for (direction, asset_id, maximum_amount, blinding) in [
                (
                    Direction::TakerBuys,
                    traded_asset_id,
                    inventory_reserve,
                    inventory_reserve_blinding,
                ),
                (
                    Direction::TakerSells,
                    cash_asset_id,
                    cash_reserve,
                    cash_reserve_blinding,
                ),
            ] {
                let maximum_amount = u64::try_from(maximum_amount)
                    .map_err(|_| format!("Maker {maker} reserve is outside u64"))?;
                let blinding = Scalar::from(blinding);
                let reserve_id: [u8; 32] = Sha256::new()
                    .chain_update(b"QOMM:DEMO:MAKER-RESERVE:v1")
                    .chain_update(signer.snapshot.participant_id)
                    .chain_update(policy_digest)
                    .chain_update([direction as u8])
                    .finalize()
                    .into();
                let mandate = MakerPolicyMandate {
                    venue_id: identity.venue_id,
                    defmi_id: identity.defmi_id,
                    policy_digest,
                    policy_version,
                    asset_id,
                    direction,
                    reserve_id,
                    maximum_amount_commitment: settlement_key
                        .commit(&Scalar::from(maximum_amount), &blinding)
                        .compress()
                        .to_bytes(),
                    maker_handle: (settlement_key.g
                        * Scalar::from(self.maker_handle_scalars[maker]))
                    .compress()
                    .to_bytes(),
                    entity_commitment: signer.presentation.entity_commitment(),
                    kyb_presentation_digest: signer.presentation.binding_digest(),
                    valid_from: 1,
                    valid_until,
                    auto_execute: true,
                    maker_public,
                    signature: Signature::from_bytes(&[0_u8; 64]),
                };
                let signed = signer
                    .client
                    .sign_maker_mandate(&signer.snapshot, &mandate)?;
                signed.verify(
                    &signer.presentation,
                    &identity.registry,
                    &identity.trusted_issuer,
                    &identity.identity_scope,
                    &identity.identity_context,
                    &identity.required_cohort,
                    now,
                )?;
                signed_mandates.push(encode_maker_mandate(&signed)?);
            }
            self.maker_authorities[maker] = Some(CachedMakerAuthority {
                source_policy_digest,
                registered_policy_digest: policy_digest,
                policy_blindings,
                inventory_reserve,
                inventory_reserve_blinding,
                cash_reserve,
                cash_reserve_blinding,
                valid_until,
                signed_mandates,
                standing_pools: Vec::new(),
            });
        }
        let registry_digest = self.registered_policy_registry_digest(policies)?;
        if let Some(market) = self.defmi_market.as_mut() {
            let public = self
                .frost_public
                .as_ref()
                .ok_or_else(|| "DeFMI market has no resident FROST public package".to_string())?;
            let mut proof_parties = self
                .endpoints
                .iter()
                .cloned()
                .map(|endpoint| HttpProofPartyClient::new(endpoint, self.timeout))
                .collect::<Vec<_>>();
            let pq_committee =
                qomm_transport::frost_coordinator::read_pq_committee(&mut proof_parties, public)?;
            market.register_verifier(
                registry_digest,
                public,
                &pq_committee,
                now,
                self.require_existing_maker_authority,
            )?;
        }
        let mut pool_updates = Vec::new();
        if let Some(market) = self.defmi_market.as_ref() {
            for maker in 0..self.n_makers {
                let signer = self
                    .maker_pretrade_signers
                    .get(maker)
                    .ok_or_else(|| format!("Maker {maker} has no participant service"))?
                    .clone();
                let cached = self.maker_authorities[maker]
                    .as_ref()
                    .ok_or_else(|| format!("Maker {maker} has no preauthorized policy"))?;
                let mut pools = Vec::new();
                for encoded in &cached.signed_mandates {
                    let mandate = decode_maker_mandate(encoded)?;
                    let (maximum_amount, maximum_blinding) = match mandate.direction {
                        Direction::TakerBuys => (
                            u64::try_from(cached.inventory_reserve).map_err(|_| {
                                format!("Maker {maker} inventory reserve exceeds u64")
                            })?,
                            cached.inventory_reserve_blinding,
                        ),
                        Direction::TakerSells => (
                            u64::try_from(cached.cash_reserve)
                                .map_err(|_| format!("Maker {maker} cash reserve exceeds u64"))?,
                            cached.cash_reserve_blinding,
                        ),
                    };
                    let receipt = market.ensure_maker_standing_pool(MakerStandingPoolRequest {
                        participant: &signer.client,
                        snapshot: &signer.snapshot,
                        mandate: &mandate,
                        maximum_amount,
                        maximum_blinding,
                        now,
                        existing_only: self.require_existing_maker_authority,
                    })?;
                    pools.push(CachedMakerPool {
                        mandate,
                        pool_id: receipt.pool.pool_id,
                        facility_id: receipt.facility_id,
                    });
                }
                pool_updates.push((maker, pools));
            }
        }
        for (maker, pools) in pool_updates {
            self.maker_authorities[maker]
                .as_mut()
                .ok_or_else(|| format!("Maker {maker} policy disappeared during pool setup"))?
                .standing_pools = pools;
        }
        // The standing pools now exist on canonical DeFMI.  A node set that
        // has never been seeded receives the registration openings once; any
        // later pool change or allocation is reconciled per RFQ from DeFMI and
        // from each node's own accepted executions, never re-seeded.
        //
        // Seeding needs every node.  When the committee is unreachable the
        // registered policies are still valid authority for a signed request
        // to enter the corporate queue, so seeding is deferred: the same check
        // runs again in `reconcile_maker_state` before any input is dealt, and
        // a partially seeded node set still fails closed there.
        if self.defmi_market.is_some() {
            if self.committee_healthy() {
                if self.ensure_maker_state_seeded()? {
                    println!(
                        "qomm-demo: seeded resident Maker state on all {} MPC nodes",
                        self.n_parties
                    );
                }
            } else {
                eprintln!(
                    "qomm-demo: MPC committee unreachable during Maker preauthorization; resident Maker state seeding deferred to the pre-execution reconciliation"
                );
            }
        }
        Ok(())
    }

    fn quote(
        &mut self,
        policies: &[Policy],
        request: &Request,
        settlement: &MpcSettlementInputs,
        _room_time: i64,
        corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        let mut corporate_reserve_active = false;
        let mut corporate_identity: Option<(String, [u8; 32])> = None;
        let execution = (|| -> Result<MpcRound, String> {
            let mut preclaimed_replay = self.preclaimed_replay.take();
            if !corrupt.is_empty() {
                return Err("the Docker MPC path refuses synthetic in-process corruption; stop a real node container to test failure".into());
            }
            let wall_now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "distributed settlement time is before Unix epoch")?
                .as_secs();
            let fresh_market_time = market_time_for_execution(wall_now, None)?;
            if request.is_real == 1 && preclaimed_replay.is_none() {
                let next_sequence = {
                    let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                        "a real distributed RFQ has no corporate participant module".to_string()
                    })?;
                    let current = signer.client.snapshot()?;
                    if current.participant_id != signer.snapshot.participant_id
                        || current.role != "taker"
                    {
                        return Err(
                            "Taker participant identity changed before RFQ allocation".into()
                        );
                    }
                    current.corporate_outbox_next_sequence
                };
                self.served = self.served.max(next_sequence);
            }
            let signed_maker_mandates = if self.pretrade_signer.is_some() {
                self.checked_maker_mandates(policies, settlement, wall_now)?
            } else {
                Vec::new()
            };
            // Before any admission or reserve, every MPC node's resident Maker
            // state must open the canonical DeFMI pool notes this RFQ can
            // allocate from.  Nothing there reveals an opening to the gateway.
            // That reconciliation needs every node, so it runs only after the
            // signed RFQ is durable in the corporate outbox (below): a request
            // signed while the committee is unavailable is accepted into the
            // queue and executed by the replay ticker once the committee is
            // back, instead of being refused before it was ever recorded.
            let mut maker_state_reconciliation: Option<Value> = None;
            let maker_authority_digest: [u8; 32] = {
                let mut digest = Sha256::new().chain_update(b"QOMM:MAKER-AUTHORITY-SET:v1");
                for mandate in &signed_maker_mandates {
                    digest = digest
                        .chain_update((mandate.len() as u64).to_be_bytes())
                        .chain_update(mandate);
                }
                digest.finalize().into()
            };
            // This participant signature and admission claim are complete before
            // the first node receives an MP-SPDZ input.  No post-match consent
            // route exists in the participant service.
            let replay_envelope = preclaimed_replay
                .as_ref()
                .map(|claimed| {
                    serde_json::from_slice::<QueuedRfqEnvelope>(&claimed.signed_request)
                        .map_err(|_| "claimed corporate RFQ is not canonical JSON".to_string())
                })
                .transpose()?;
            let fresh_admission_sequence = if replay_envelope.is_none() {
                match self.defmi_market.as_ref() {
                    // Cover traffic is admitted through the same public DeFMI
                    // path as a real RFQ so an observer cannot distinguish the
                    // two from transaction shape.  It therefore needs the
                    // consensus-owned venue cursor too: a process-local cover
                    // slot diverges as soon as the gateway restarts.
                    Some(market) => Some(market.next_admission_sequence()?),
                    None if request.is_real == 1 => {
                        return Err(
                            "a real distributed RFQ has no authoritative DeFMI admission cursor"
                                .to_string(),
                        );
                    }
                    None => None,
                }
            } else {
                None
            };
            let mut admission = match replay_envelope.as_ref() {
                Some(queued) => queued.verify(
                    &self
                        .pretrade_signer
                        .as_ref()
                        .ok_or_else(|| {
                            "queued RFQ has no corporate participant module".to_string()
                        })?
                        .snapshot,
                    self.pretrade_signer
                        .as_ref()
                        .expect("checked queued participant")
                        .defmi_id,
                    maker_authority_digest,
                    wall_now,
                )?,
                None => {
                    self.prepare_admission(request, settlement, wall_now, fresh_admission_sequence)?
                }
            };
            let mut effective_market_time = fresh_market_time;
            let mut corporate_dispatch: Option<(String, [u8; 32], u64, u32)> = None;
            if request.is_real == 1 {
                // Cloned rather than borrowed: the Maker-state reconciliation
                // below needs `&mut self` after the request is durable.
                let signer = self.pretrade_signer.clone().ok_or_else(|| {
                    "a real distributed RFQ has no corporate participant module".to_string()
                })?;
                let mandate = admission
                    .taker_mandate
                    .as_ref()
                    .ok_or_else(|| "real RFQ has no signed Taker mandate".to_string())?;
                let request_id = hex::encode(mandate.rfq_nullifier);
                let existing = signer
                    .client
                    .corporate_request_status(&signer.snapshot, &request_id)?;
                if existing.is_none() && preclaimed_replay.is_none() {
                    let mut queued = QueuedRfqEnvelope {
                        version: 3,
                        market_time: fresh_market_time,
                        policies: policies.to_vec(),
                        request: request.clone(),
                        settlement: settlement.clone(),
                        admission: admission.envelope.clone(),
                        mandate_digest: admission.mandate_digest,
                        signed_taker_mandate: admission
                            .signed_mandate
                            .clone()
                            .ok_or_else(|| "real RFQ has no canonical mandate wire".to_string())?,
                        maximum_amount: admission.maximum_amount,
                        maximum_blinding: admission.maximum_blinding,
                        response_mask: admission.response_mask,
                        fill_mask: admission.fill_mask,
                        maker_authority_digest,
                        approval_domain: signer.defmi_id,
                        approval: EntityApproval {
                            participant_id: signer.snapshot.participant_id,
                            key_purpose: KeyPurpose::MpcInput,
                            key_epoch: 1,
                            statement: ZERO,
                            signature: Vec::new(),
                        },
                    };
                    let statement = queued.statement()?;
                    queued.approval = signer.client.entity_approval(
                        &signer.snapshot,
                        signer.defmi_id,
                        KeyPurpose::MpcInput,
                        statement,
                    )?;
                    let queued_bytes =
                        serde_json::to_vec(&queued).map_err(|error| error.to_string())?;
                    signer.client.enqueue_corporate_request(
                        &signer.snapshot,
                        &request_id,
                        &queued_bytes,
                        mandate.deadline,
                    )?;
                } else if matches!(
                    existing.as_ref().map(|item| item.state.as_str()),
                    Some("mpc_admitted" | "settled" | "released")
                ) {
                    return Err(
                    "this corporate RFQ already reached MPC or DeFMI; reconcile its canonical result instead of executing it again"
                        .into(),
                );
                }
                let claimed = match preclaimed_replay.take() {
                    Some(claimed) => {
                        // The replay ticker already found every node healthy
                        // and claimed this attempt; the nodes must still open
                        // the canonical pool notes before any input is dealt.
                        if self.defmi_market.is_some() {
                            maker_state_reconciliation =
                                Some(self.reconcile_maker_state(policies)?);
                        }
                        claimed
                    }
                    None => {
                        let quorum_healthy = self.committee_healthy();
                        // Reconcile before the claim so a node that is
                        // reachable but behind or ahead of DeFMI does not
                        // consume one of the bounded automatic attempts; the
                        // signed request stays `queued` and the ticker retries.
                        if quorum_healthy && self.defmi_market.is_some() {
                            maker_state_reconciliation =
                                Some(self.reconcile_maker_state(policies)?);
                        }
                        match signer.client.claim_corporate_cover_slot(
                            &signer.snapshot,
                            quorum_healthy,
                            2,
                            1,
                        )? {
                            CorporateOutboxAction::Real(claimed) => claimed,
                            CorporateOutboxAction::Expire {
                                request_id,
                                request_digest,
                                signed_request,
                                ..
                            } => {
                                self.release_expired_corporate(
                                    &signer,
                                    &request_id,
                                    request_digest,
                                    signed_request,
                                    wall_now,
                                )?;
                                return Err(format!(
                                "corporate RFQ {request_id} expired in the durable queue; its DeFMI reserve was released"
                            ));
                            }
                            CorporateOutboxAction::Dummy { .. } if !quorum_healthy => {
                                return Err(CORPORATE_QUEUE_UNAVAILABLE.into())
                            }
                            CorporateOutboxAction::Dummy { .. } | CorporateOutboxAction::NotDue => {
                                return Err(CORPORATE_QUEUE_WAITING_SLOT.into())
                            }
                        }
                    }
                };
                if claimed.request_id != request_id {
                    return Err(format!(
                        "an older corporate RFQ ({}) must dispatch before this request",
                        claimed.request_id
                    ));
                }
                let queued = match replay_envelope.as_ref() {
                    Some(queued) => queued.clone(),
                    None => serde_json::from_slice::<QueuedRfqEnvelope>(&claimed.signed_request)
                        .map_err(|_| "claimed corporate RFQ is not canonical JSON".to_string())?,
                };
                if queued.policies != policies
                    || queued.request != *request
                    || queued.settlement != *settlement
                {
                    return Err(
                    "the claimed corporate RFQ differs from the supplied policy or request snapshot"
                        .into(),
                );
                }
                admission = queued.verify(
                    &signer.snapshot,
                    signer.defmi_id,
                    maker_authority_digest,
                    wall_now,
                )?;
                effective_market_time =
                    market_time_for_execution(wall_now, Some(queued.market_time))?;
                corporate_dispatch = Some((
                    claimed.request_id,
                    claimed.request_digest,
                    claimed.sequence,
                    claimed.attempt,
                ));
            }
            let now = effective_market_time;
            // A durable replay must reproduce the exact MPC inputs, commitments,
            // and round identifier of the signed RFQ. `self.served` points at the
            // next fresh corporate request after a restart, so it must never be
            // used as the replay's per-round entropy/blinding cursor.
            let round_slot = admission.envelope.slot;
            let (party_files, reference, mask, node_shares) =
                self.generate(RoundInputGeneration {
                    policies,
                    request,
                    settlement,
                    now,
                    round_slot,
                    response_mask: admission.response_mask,
                    fill_mask: admission.fill_mask,
                })?;
            let fill_mask = json_u64(
                reference
                    .get("fill_mask")
                    .ok_or_else(|| "generated reference has no fill mask".to_string())?,
            )?;
            if mask != admission.response_mask || fill_mask != admission.fill_mask {
                return Err("generated MPC response masks differ from the pre-signed RFQ".into());
            }
            let input_counts = party_files
                .iter()
                .map(|input| input.split_whitespace().count())
                .collect::<BTreeSet<_>>();
            if input_counts.len() != 1 {
                return Err("distributed MPC party inputs disagree on their fixed shape".into());
            }
            let input_count = u64::try_from(*input_counts.iter().next().expect("one input shape"))
                .map_err(|_| "distributed MPC input count exceeds u64")?;
            let mut round_digest = Sha256::new()
                .chain_update(b"QOMM:DEMO:DISTRIBUTED-MPC-ROUND:v1")
                .chain_update(self.source_sha256.as_bytes())
                .chain_update(round_slot.to_be_bytes())
                .chain_update(now.to_be_bytes())
                .chain_update(admission.mandate_digest);
            for mandate in &signed_maker_mandates {
                round_digest.update(Sha256::digest(mandate));
            }
            for input in &party_files {
                round_digest.update(Sha256::digest(input.as_bytes()));
            }
            let round_id = hex::encode(round_digest.finalize());
            let started = Instant::now();
            let prepared_requests = party_files
                .into_iter()
                .enumerate()
                .map(|(node, input)| {
                    let input_sha256 = hex::encode(execution_input_digest(now, input.as_bytes()));
                    (
                        input,
                        AdmitRequest {
                            version: PROTOCOL_VERSION,
                            node,
                            round_id: round_id.clone(),
                            source_sha256: self.source_sha256.clone(),
                            public_market_time: now,
                            input_sha256,
                            admission: admission.envelope.clone(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            // Phase 1: every node commits to its node-local input digest and the
            // same ordered legal-entity claim. No plaintext MPC input is sent yet.
            let admission_receipts = thread::scope(|scope| {
                let mut handles = Vec::with_capacity(self.n_parties);
                for (node, (endpoint, (_, request))) in self
                    .endpoints
                    .iter()
                    .zip(prepared_requests.iter())
                    .enumerate()
                {
                    let timeout = self.timeout;
                    let request = request.clone();
                    handles.push(scope.spawn(move || {
                        let value = endpoint.post(
                            "/v1/admit",
                            &serde_json::to_value(request).map_err(|error| error.to_string())?,
                            timeout,
                        )?;
                        serde_json::from_value::<AdmissionAttestationWire>(value).map_err(|error| {
                            format!("MPC node {node} returned an invalid admission: {error}")
                        })
                    }));
                }
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| "distributed MPC admission client panicked".to_string())?
                    })
                    .collect::<Result<Vec<_>, String>>()
            })?;
            if admission_receipts.len() != self.n_parties {
                return Err("distributed MPC returned an incomplete admission set".into());
            }
            let (admission_attestations, admission_node_keys) = admission_receipts
                .iter()
                .map(decode_admission_receipt)
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .unzip::<_, _, Vec<_>, Vec<_>>();
            let certified_admission =
                verify_admission_lane(&admission_attestations, &admission_node_keys)?;
            if certified_admission.slot != admission.envelope.slot
                || certified_admission.sequence != admission.envelope.sequence
                || certified_admission.ticket_id
                    != decode_hex32(&admission.envelope.ticket_id, "admission ticket")?
                || certified_admission.claim_digest != admission.mandate_digest
                || certified_admission.order_digest
                    != decode_hex32(&admission.envelope.order_digest, "admission order")?
            {
                return Err("MPC nodes certified another admission claim or order".into());
            }
            let admission_wire = encode_admission_attestations(&admission_attestations)?;
            // Phase 2 is the DeFMI reservation boundary. The admission population
            // above is already fixed; authoritative reservation receipts are
            // attached here before phase 3 is allowed to reveal any node input.
            // Establish the one durable 3-of-7 key before reserving.  The same
            // public package must authorize each reserve zkPI and the later final
            // settlement; a coordinator cannot switch committees after matching.
            let frost_session: [u8; 32] = Sha256::new()
                .chain_update(b"QOMM:DEMO:FROST-COMMITTEE:v1")
                .chain_update(self.source_sha256.as_bytes())
                .finalize()
                .into();
            let mut proof_parties = self
                .endpoints
                .iter()
                .cloned()
                .map(|endpoint| HttpProofPartyClient::new(endpoint, self.timeout))
                .collect::<Vec<_>>();
            let frost_public = match self.frost_public.clone() {
                Some(public) => public,
                None => distributed_frost_setup(&mut proof_parties, frost_session)?,
            };
            let frost_public_sha256 = hex::encode(Sha256::digest(
                frost_public
                    .serialize()
                    .map_err(|_| "FROST public package serialization failed")?,
            ));
            // The admission batch is part of the durable RFQ identity.  A queued
            // request must therefore keep the deadline that the Taker signed;
            // deriving a fresh wall-clock deadline on every replay would change
            // the batch statement and turn an otherwise exact retry into a scope
            // reuse attempt.  Cover traffic has no signed mandate and is not
            // durably replayed, so it retains the bounded live-session deadline.
            let admission_expires_at = admission
                .taker_mandate
                .as_ref()
                .map(|mandate| mandate.deadline)
                .unwrap_or_else(|| wall_now.saturating_add(3_590));
            let defmi_admission: Option<DefmiAdmissionReceipt> = match self.defmi_market.as_mut() {
                Some(market) => match market.register_admission(
                    &admission_attestations,
                    &admission_node_keys,
                    admission_expires_at,
                ) {
                    Ok(receipt) => Some(receipt),
                    Err(error) => {
                        // No Taker reserve has been submitted at this point. A
                        // stale admission cursor or other terminal DeFMI rejection
                        // must not leave the entity-owned outbox dispatching until
                        // its one-hour deadline. The participant module performs
                        // its own canonical non-existence read before freeing the
                        // corporate cap; if that read is uncertain, the queue stays
                        // durable for reconciliation instead.
                        if let Some((request_id, request_digest, _, _)) =
                            corporate_dispatch.as_ref()
                        {
                            let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                                "failed corporate admission lost its participant module".to_string()
                            })?;
                            match signer.client.abort_corporate_before_reserve(
                            &signer.snapshot,
                            request_id,
                            *request_digest,
                        ) {
                            Ok(state_root) => {
                                return Err(format!(
                                    "{error}; corporate RFQ was finalized before reserve at DeFMI state {}",
                                    hex::encode(state_root)
                                ))
                            }
                            Err(abort_error) => {
                                return Err(format!(
                                    "{error}; pre-reserve reconciliation did not finalize the corporate RFQ: {abort_error}"
                                ))
                            }
                        }
                        }
                        return Err(error);
                    }
                },
                None => None,
            };

            // A real request cannot reveal any MP-SPDZ input until the Taker's
            // exact maximum has become an active anonymous covenant on DeFMI. The
            // same fixed admission lane and signed mandate bind both operations.
            let taker_note_reservation_receipt = if request.is_real == 1 {
                let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                    "real MPC execution has no entity-owned Taker participant".to_string()
                })?;
                let market = self.defmi_market.as_ref().ok_or_else(|| {
                    "real MPC execution has no authoritative DeFMI market".to_string()
                })?;
                let mandate = admission
                    .taker_mandate
                    .as_ref()
                    .ok_or_else(|| "real admission omitted its signed Taker mandate".to_string())?;
                let receipt = defmi_admission.as_ref().ok_or_else(|| {
                    "real admission has no canonical DeFMI batch receipt".to_string()
                })?;
                let (corporate_request_id, corporate_request_digest) = corporate_dispatch
                    .as_ref()
                    .map(|(request_id, request_digest, _, _)| {
                        (request_id.as_str(), *request_digest)
                    })
                    .ok_or_else(|| {
                        "real MPC execution has no participant-owned corporate RFQ".to_string()
                    })?;
                let reserved = match market.reserve_taker_note(
                    &signer.client,
                    &signer.snapshot,
                    corporate_request_id,
                    corporate_request_digest,
                    mandate,
                    admission.maximum_amount,
                    admission.maximum_blinding,
                    &certified_admission,
                    receipt,
                    &signer.presentation,
                    &signer.registry,
                    &signer.trusted_issuer,
                    &signer.identity_scope,
                    &signer.identity_context,
                    &signer.required_cohort,
                    &mut proof_parties,
                    &frost_public,
                    wall_now,
                ) {
                    Ok(reserved) => reserved,
                    Err(error) => {
                        // Admission is public traffic, but no MPC input has been
                        // revealed and no usable Taker reserve receipt exists.
                        // Release the entity-owned queue cap only after the
                        // participant module confirms the hold is absent.  If a
                        // concurrent/uncertain reserve exists, its reconciliation
                        // fails closed and the durable queue remains intact.
                        if let Some((request_id, request_digest, _, _)) =
                            corporate_dispatch.as_ref()
                        {
                            match signer.client.abort_corporate_before_reserve(
                                &signer.snapshot,
                                request_id,
                                *request_digest,
                            ) {
                                Ok(state_root) => {
                                    return Err(format!(
                                    "{error}; corporate RFQ was finalized before reserve at DeFMI state {}",
                                    hex::encode(state_root)
                                ));
                                }
                                Err(abort_error) => {
                                    return Err(format!(
                                    "{error}; pre-reserve reconciliation did not finalize the corporate RFQ: {abort_error}"
                                ));
                                }
                            }
                        }
                        return Err(error);
                    }
                };
                corporate_reserve_active = corporate_dispatch.is_some();
                corporate_identity =
                    corporate_dispatch
                        .as_ref()
                        .map(|(request_id, request_digest, _, _)| {
                            (request_id.clone(), *request_digest)
                        });
                Some(reserved)
            } else {
                None
            };
            let taker_note_reservation = taker_note_reservation_receipt.as_ref().map(|reserved| {
                json!({
                    "facility_id": hex::encode(reserved.facility_id),
                    "reserve_id": hex::encode(reserved.reservation.hold_id),
                    "escrow_note_id": hex::encode(reserved.reservation.escrow_note_id),
                    "asset_id": hex::encode(reserved.reservation.asset_id),
                    "status": reserved.reservation.status,
                    "reserve_receipt_digest": hex::encode(reserved.reserve_receipt_digest),
                    "state_root": hex::encode(reserved.reservation.state_root),
                })
            });

            // Phase 3: execute only against the exact durable admission receipt
            // returned by that node. A coordinator cannot swap another RFQ or
            // another node-local share after a reservation succeeds.
            let execution_generation = corporate_dispatch
                .as_ref()
                .map(|(_, _, _, attempt)| *attempt)
                .unwrap_or(1);
            let receipts = thread::scope(|scope| {
                let mut handles = Vec::with_capacity(self.n_parties);
                for (node, ((endpoint, (input, admitted)), certified_admission)) in self
                    .endpoints
                    .iter()
                    .zip(prepared_requests)
                    .zip(admission_receipts)
                    .enumerate()
                {
                    let timeout = self.timeout;
                    handles.push(scope.spawn(move || {
                        let request = ExecuteRequest {
                            version: admitted.version,
                            node: admitted.node,
                            round_id: admitted.round_id,
                            source_sha256: admitted.source_sha256,
                            public_market_time: admitted.public_market_time,
                            input_sha256: admitted.input_sha256,
                            input,
                            execution_generation,
                            admission: admitted.admission,
                            certified_admission,
                        };
                        let value = endpoint.post(
                            "/v1/execute",
                            &serde_json::to_value(request).map_err(|error| error.to_string())?,
                            timeout,
                        )?;
                        serde_json::from_value::<ExecuteReceipt>(value).map_err(|error| {
                            format!("MPC node {node} returned an invalid receipt: {error}")
                        })
                    }));
                }
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| "distributed MPC node client panicked".to_string())?
                    })
                    .collect::<Result<Vec<_>, String>>()
            })?;
            if receipts.len() != self.n_parties {
                return Err("distributed MPC returned an incomplete receipt set".into());
            }
            for (node, receipt) in receipts.iter().enumerate() {
                if receipt.version != PROTOCOL_VERSION
                    || receipt.node != node
                    || receipt.round_id != round_id
                    || receipt.source_sha256 != self.source_sha256
                    || receipt.public_market_time != now
                    || receipt.input_sha256 != receipt.admission.batch_digest
                    || decode_admission_receipt(&receipt.admission)?.0
                        != admission_attestations[node]
                {
                    return Err(format!(
                        "MPC node {node} returned a receipt for another execution or admission"
                    ));
                }
            }
            let public_results = receipts
                .iter()
                .enumerate()
                .map(|(node, receipt)| {
                    let identity = decode_hex32(
                        &receipt.result_identity_public,
                        "MPC public-result identity",
                    )?;
                    if identity != admission_node_keys[node].to_bytes() {
                        return Err(format!(
                            "MPC node {node} changed identity between admission and result"
                        ));
                    }
                    let raw = hex::decode(&receipt.public_result_attestation).map_err(|_| {
                        "MPC public-result attestation is not hexadecimal".to_string()
                    })?;
                    let value = decode_node_public_result_attestation(&raw)?;
                    if usize::from(value.node) != node
                        || value.masked_key != receipt.masked_key
                        || value.masked_fill != receipt.masked_fill
                        || value.batch_digest
                            != decode_hex32(&receipt.input_sha256, "MPC input digest")?
                        || !value.verify(&admission_node_keys[node])
                    {
                        return Err(format!(
                            "MPC node {node} signed another public result or input batch"
                        ));
                    }
                    Ok(value)
                })
                .collect::<Result<Vec<_>, String>>()?;
            let certified_result = verify_public_result_lane(
                &public_results,
                &admission_node_keys,
                decode_hex32(&admission.envelope.order_digest, "admission order")?,
            )?;
            if certified_result.slot != certified_admission.slot
                || certified_result.sequence != certified_admission.sequence
                || certified_result.cluster_digest != certified_admission.cluster_digest
                || certified_result.round_digest
                    != decode_hex32(&round_id, "distributed MPC round")?
            {
                return Err("signed public MPC result differs from its admitted RFQ lane".into());
            }
            let public_result_attestations = encode_public_result_attestations(&public_results)?;
            let masked = receipts
                .iter()
                .map(|receipt| receipt.masked_key)
                .collect::<BTreeSet<_>>();
            if masked.len() != 1 {
                return Err(format!(
                    "the distributed MPC nodes disagreed on the public masked result: {masked:?}"
                ));
            }
            let masked_key = *masked.iter().next().expect("one masked result");
            let masked_fills = receipts
                .iter()
                .map(|receipt| receipt.masked_fill)
                .collect::<BTreeSet<_>>();
            if masked_fills.len() != 1 {
                return Err(format!(
                "the distributed MPC nodes disagreed on the public masked fill: {masked_fills:?}"
            ));
            }
            let masked_fill = *masked_fills.iter().next().expect("one masked fill");
            let persistence_sha256 = receipts
                .iter()
                .map(|receipt| (receipt.node, receipt.persistence_sha256.clone()))
                .collect::<BTreeMap<_, _>>();
            let references = self
                .references
                .iter()
                .map(|value| *value as i64)
                .collect::<Vec<_>>();
            let outcome: Outcome = evaluate(policies, request, &references, now);
            let opened_key = masked_key
                .checked_sub(i128::from(mask))
                .ok_or_else(|| "masked result underflowed its Taker mask".to_string())?;
            let (opened_cost, opened_winner) = unpack_key(opened_key, self.config.n_mm);
            let (verified, detail, filled) = verify_masked_execution(
                &outcome,
                request,
                settlement.user_limit,
                MaskedExecutionOpening {
                    padded_makers: self.config.n_mm,
                    masked_key,
                    key_mask: mask,
                    masked_fill,
                    fill_mask,
                },
            )?;
            let mut proof_job_id = None;
            let mut quote_proof_digest = None;
            let mut settlement_record = None;
            let mut execution_attestations = None;
            let mut execution_node_keys = None;
            let mut standing_pool_allocation = None;
            let mut defmi_product_settlement = None;
            let mut maker_state_commits = None;
            let mut taker_no_fill_release = None;
            if request.is_real == 1 && filled {
                let source_digest = decode_hex32(&self.source_sha256, "MPC source digest")?;
                let execution_nodes = receipts
                    .iter()
                    .map(|receipt| {
                        Ok(ExecutionNodeDigest {
                            batch_digest: decode_hex32(&receipt.input_sha256, "MPC input digest")?,
                            source_digest,
                            stdout_digest: decode_hex32(
                                &receipt.stdout_sha256,
                                "MPC stdout digest",
                            )?,
                            stderr_digest: decode_hex32(
                                &receipt.stderr_sha256,
                                "MPC stderr digest",
                            )?,
                            persistence_digest: decode_hex32(
                                &receipt.persistence_sha256,
                                "MPC persistence digest",
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let application_manifest = qomm_manifest_v1();
                let execution_plan = ApplicationExecutionPlan::new(
                    &application_manifest,
                    ExecutionShape {
                        lane: execution_lane_for_admission(admission.envelope.sequence)?,
                        slot: admission.envelope.slot,
                        generation: u64::from(execution_generation),
                        frame_count: 1,
                        input_count,
                        order_digest: decode_hex32(
                            &admission.envelope.order_digest,
                            "admission order",
                        )?,
                        nodes: execution_nodes,
                    },
                )
                .map_err(|error| error.to_string())?;
                // The job identifier is the canonical digest that DeFMI will
                // independently reconstruct from all seven signed receipts.
                // It must exist before quote/ZK/zkPI transcripts are loaded.
                let job_id = execution_plan.job_id();
                let application_manifest_digest = execution_plan.manifest_digest();
                let application_binding = execution_plan.binding_digest();
                let execution = execution_plan.into_product_request();
                for (node, party) in proof_parties.iter_mut().enumerate() {
                    let persistence =
                        execution_persistence_path(&round_id, execution_generation, node)?;
                    let loaded = party.call(
                        "load",
                        json!({
                            "job_id": hex::encode(job_id),
                            "persistence": persistence,
                            "quote_digest": hex::encode(job_id),
                        }),
                    )?;
                    if loaded.get("party").and_then(Value::as_u64) != Some(node as u64 + 1) {
                        return Err(format!(
                            "proof node {node} loaded another node's MPC persistence"
                        ));
                    }
                }
                let request_context = admission.mandate_digest;
                let asset = usize::try_from(request.asset)
                    .ok()
                    .filter(|asset| *asset < references.len())
                    .ok_or_else(|| "real request names an unknown asset".to_string())?;
                let biased_cost = opened_cost
                    .checked_add(
                        sentinel_for(
                            self.bit_length,
                            self.config.n_mm,
                            8 * self.references.iter().copied().max().unwrap_or(0),
                        )
                        .map_err(|error| error.to_string())?,
                    )
                    .ok_or_else(|| "quote proof cost bias overflowed".to_string())?;
                let winner_value = biased_cost
                    .checked_mul(self.config.n_mm as i128)
                    .and_then(|value| value.checked_add(opened_winner as i128))
                    .and_then(|value| u64::try_from(value).ok())
                    .ok_or_else(|| "quote proof winner value exceeds u64".to_string())?;
                let policy_openings = policies
                    .iter()
                    .enumerate()
                    .map(|(maker, policy)| {
                        Ok(RegisteredPolicyOpening {
                            maker_asset: u32::try_from(policy.asset)
                                .map_err(|_| "Maker asset exceeds u32".to_string())?,
                            values: [
                                policy.ask_level,
                                policy.spread,
                                policy.slope,
                                policy.invcoef,
                                policy.inv,
                                policy.maxqty,
                                policy.expiry,
                                policy.active,
                                policy.use_ref,
                            ],
                            blindings: self.policy_blindings(maker)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let market_digest: [u8; 32] = Sha256::new()
                    .chain_update(b"QOMM:DEMO:REFERENCE-MARKET:v1")
                    .chain_update(self.references[asset].to_be_bytes())
                    .chain_update(now.to_be_bytes())
                    .finalize()
                    .into();
                let quote_request = complete_quote_request(CompleteQuotePublicInput {
                    job_id,
                    request_context,
                    quantity: request.qty,
                    quantity_blinding: round_slot.saturating_add(151),
                    now,
                    sentinel: i64::try_from(
                        sentinel_for(
                            self.bit_length,
                            self.config.n_mm,
                            8 * self.references.iter().copied().max().unwrap_or(0),
                        )
                        .map_err(|error| error.to_string())?,
                    )
                    .map_err(|_| "quote sentinel exceeds i64")?,
                    direction: u8::try_from(request.direction)
                        .map_err(|_| "request direction exceeds u8".to_string())?,
                    asset: u32::try_from(request.asset)
                        .map_err(|_| "request asset exceeds u32".to_string())?,
                    reference_price: references[asset],
                    policies: policy_openings,
                    market_digest,
                    // DeFMI orders and consumes admissions by their one-based
                    // sequence, not by the zero-based local slot counter.
                    slot: admission.envelope.sequence,
                    winner_index: opened_winner,
                    winner_value,
                    eligibility_bits: self.config.quote_eligibility_bits,
                    span_bits: self.config.quote_span_bits,
                })?;
                let quote = prove_complete_quote(&mut proof_parties, &quote_request)?;
                let quote_digest = quote.verify()?;
                let settlement_key = Pedersen::new(b"qomm:defmi:v1");
                let limit_blinding = Scalar::from(round_slot.saturating_add(101));
                let limit_value = if settlement.user_limit < 0 {
                    -Scalar::from(settlement.user_limit.unsigned_abs())
                } else {
                    Scalar::from(settlement.user_limit as u64)
                };
                let limit_commitment = settlement_key.commit(&limit_value, &limit_blinding);
                let taker_handle = settlement_key.g * Scalar::from(self.taker_handle_scalar);
                let asset_id = traded_asset_id(request.asset);
                let admission_ticket_id =
                    decode_hex32(&admission.envelope.ticket_id, "admission ticket")?;
                let now_u64 = wall_now;
                let limit_direction = match request.direction {
                    0 => PriceLimitDirection::MaximumBuyPrice,
                    1 => PriceLimitDirection::MinimumSellPrice,
                    _ => return Err("distributed request direction is outside buy/sell".into()),
                };
                let mut proof = prove_product_settlement(
                    &mut proof_parties,
                    &frost_public,
                    ProductSettlementRequest {
                        job_id,
                        admission_sequence: admission.envelope.sequence,
                        admission_ticket_id,
                        quote_verification: quote,
                        limit_direction,
                        limit_commitment,
                        limit_context: request_context,
                        taker_handle,
                        asset_id,
                        deadline: now_u64.saturating_add(3_590),
                        now: now_u64,
                        execution,
                    },
                )?;
                if !verified || outcome.winner != Some(opened_winner) {
                    return Err(
                        "MPC outcome and complete quote proof selected different Makers".into(),
                    );
                }
                let required_direction = match request.direction {
                    0 => Direction::TakerBuys,
                    1 => Direction::TakerSells,
                    _ => {
                        return Err(
                            "standing allocation request direction is outside buy/sell".into()
                        )
                    }
                };
                let maker_pool = self
                    .maker_authorities
                    .get(opened_winner)
                    .and_then(Option::as_ref)
                    .and_then(|authority| {
                        authority
                            .standing_pools
                            .iter()
                            .find(|pool| pool.mandate.direction == required_direction)
                    })
                    .cloned()
                    .ok_or_else(|| {
                        "winning Maker has no canonical standing pool for this direction"
                            .to_string()
                    })?;
                let market = self.defmi_market.as_ref().ok_or_else(|| {
                    "real MPC execution has no authoritative DeFMI market".to_string()
                })?;
                let snapshot_root = market.state_root()?;
                let canonical_pool = market.standing_note_pool(maker_pool.pool_id)?;
                let parent_note = market.note_output(canonical_pool.current_pool_note_id)?;
                let canonical_facility = market.credit_facility(maker_pool.facility_id)?;
                if market.state_root()? != snapshot_root {
                    return Err(
                        "DeFMI standing-pool state changed while the allocation was prepared"
                            .into(),
                    );
                }
                let mandate_digest = maker_pool.mandate.digest()?;
                if canonical_pool.pool_id != maker_pool.pool_id
                    || canonical_pool.entity_commitment != maker_pool.mandate.entity_commitment
                    || canonical_pool.policy_digest != maker_pool.mandate.policy_digest
                    || canonical_pool.mandate_digest != mandate_digest
                    || canonical_pool.asset_id != maker_pool.mandate.asset_id
                    || canonical_pool.direction != maker_pool.mandate.direction as u8
                    || parent_note.note_id != canonical_pool.current_pool_note_id
                    || parent_note.asset_id != canonical_pool.asset_id
                    || parent_note.lock_id != canonical_pool.pool_id
                    || canonical_facility.facility.facility_id != maker_pool.facility_id
                    || canonical_facility.facility.beneficiary_commitment
                        != maker_pool.mandate.entity_commitment
                    || canonical_facility.facility.rail_asset_id != maker_pool.mandate.asset_id
                {
                    return Err(
                        "winning Maker pool, parent note, and aggregate facility diverged".into(),
                    );
                }
                let child = match required_direction {
                    Direction::TakerBuys => proof.handoff.securities_reserve,
                    Direction::TakerSells => proof.handoff.cash_reserve,
                };
                let parent = CompressedRistretto(parent_note.value_commitment)
                    .decompress()
                    .ok_or_else(|| "Maker parent commitment is not canonical".to_string())?;
                let before_available =
                    CompressedRistretto(canonical_facility.facility.available_commitment)
                        .decompress()
                        .ok_or_else(|| "Maker available commitment is not canonical".to_string())?;
                let before_held = CompressedRistretto(canonical_facility.facility.held_commitment)
                    .decompress()
                    .ok_or_else(|| "Maker held commitment is not canonical".to_string())?;
                let before_outstanding =
                    CompressedRistretto(canonical_facility.facility.outstanding_commitment)
                        .decompress()
                        .ok_or_else(|| {
                            "Maker outstanding commitment is not canonical".to_string()
                        })?;
                if parent - child != proof.handoff.maker_pool_remainder {
                    return Err(
                        "MPC pool remainder does not conserve the canonical parent note".into(),
                    );
                }
                let hold_id = allocation_hash(&[
                    b"QOMM:DEMO:STANDING-POOL-HOLD:v1",
                    &maker_pool.pool_id,
                    &canonical_pool.sequence.to_be_bytes(),
                    &job_id,
                ]);
                let transition = CreditFacilityTransition {
                    operation_id: allocation_hash(&[
                        b"QOMM:DEMO:STANDING-POOL-ALLOCATION-OP:v1",
                        &hold_id,
                    ]),
                    facility_id: maker_pool.facility_id,
                    hold_id,
                    kind: CreditTransitionKind::Hold,
                    query_commitment: maker_pool.mandate.policy_digest,
                    amount_commitment: child.compress().to_bytes(),
                    consumed_commitment: ZERO,
                    refund_commitment: ZERO,
                    // The standing pool is a policy-specific sub-limit, while the
                    // facility is the legal entity's larger aggregate limit. Both
                    // atomically lose the same child commitment; equating their
                    // balances would collapse the aggregate cap into one policy.
                    before_available_commitment: before_available.compress().to_bytes(),
                    after_available_commitment: (before_available - child).compress().to_bytes(),
                    before_held_commitment: before_held.compress().to_bytes(),
                    after_held_commitment: (before_held + child).compress().to_bytes(),
                    before_outstanding_commitment: before_outstanding.compress().to_bytes(),
                    after_outstanding_commitment: before_outstanding.compress().to_bytes(),
                    before_sequence: canonical_facility.facility.sequence,
                    expires_at: proof
                        .handoff
                        .instruction
                        .deadline
                        .min(maker_pool.mandate.valid_until),
                    settlement_digest: ZERO,
                    relation_proof_digest: allocation_hash(&[
                        b"QOMM:DEMO:STANDING-POOL-CREDIT-RELATION:v1",
                        &job_id,
                        &quote_digest,
                        &child.compress().to_bytes(),
                        &proof.handoff.maker_pool_remainder.compress().to_bytes(),
                    ]),
                };
                transition.body()?;
                let dvp_proof_digest = threshold_dvp_package_digest(
                    &proof.handoff.instruction,
                    &threshold_dvp_sides(&proof.handoff.instruction),
                    &proof.handoff.cash_commitment,
                    &proof.handoff.securities_remainder,
                    &proof.handoff.cash_remainder,
                    &proof.handoff.dvp_proofs,
                );
                let remainder_range_proof_digest =
                    threshold_range_proof_digest(&proof.handoff.maker_pool_remainder_proof);
                let escrow_note = allocation_note(
                    maker_pool.mandate.asset_id,
                    child.compress().to_bytes(),
                    hold_id,
                    job_id,
                    b"escrow",
                )?;
                let remainder_note = allocation_note(
                    maker_pool.mandate.asset_id,
                    proof.handoff.maker_pool_remainder.compress().to_bytes(),
                    maker_pool.pool_id,
                    job_id,
                    b"remainder",
                )?;
                let mut authorization = ReservationAuthorization {
                    role: ReservationRole::Maker,
                    entity_commitment: maker_pool.mandate.entity_commitment,
                    asset_id: maker_pool.mandate.asset_id,
                    direction: maker_pool.mandate.direction as u8,
                    authorization_digest: maker_pool.mandate.policy_digest,
                    mandate_digest,
                    typed_reserve_digest: [1; 32],
                    reserve_nullifier: [1; 32],
                    asset_link_proof_digest: [1; 32],
                    limit_price_commitment: ZERO,
                    escrow_digest: ZERO,
                    rfq_nullifier: ZERO,
                    policy_version: maker_pool.mandate.policy_version,
                    admission_ticket_id: ZERO,
                    admission_slot: 0,
                    admission_receipt_digest: ZERO,
                    admission_epoch: 0,
                    admission_sequence: 0,
                    admission_batch_id: ZERO,
                };
                let mut allocation = StandingNotePoolAllocation {
                    pool_id: maker_pool.pool_id,
                    delegation_digest: canonical_pool.delegation_digest,
                    committee_epoch: canonical_pool.committee_epoch,
                    expected_pool_sequence: canonical_pool.sequence,
                    previous_pool_note_id: canonical_pool.current_pool_note_id,
                    previous_amount_commitment: parent_note.value_commitment,
                    escrow_note,
                    remainder_note,
                    proof_job_id: job_id,
                    quote_proof_digest: quote_digest,
                    dvp_proof_digest,
                    remainder_range_proof_digest,
                    committee_signature: Vec::new(),
                    pq_authorization: None,
                };
                let metadata = standing_pool_reservation_metadata(
                    allocation.pool_id,
                    allocation.delegation_digest,
                    allocation.expected_pool_sequence,
                    allocation.previous_pool_note_id,
                    allocation.proof_job_id,
                    allocation.quote_proof_digest,
                    allocation.dvp_proof_digest,
                    transition.statement()?,
                    authorization.entity_commitment,
                    authorization.asset_id,
                    authorization.direction,
                    authorization.authorization_digest,
                    authorization.mandate_digest,
                    authorization.policy_version,
                )?;
                authorization.typed_reserve_digest = metadata.typed_reserve_digest;
                authorization.reserve_nullifier = metadata.reserve_nullifier;
                authorization.asset_link_proof_digest = metadata.asset_link_proof_digest;
                let binding = allocation.signing_binding(&transition, &authorization)?;
                let signature = authorize_standing_pool_allocation(
                    &mut proof_parties,
                    &proof.handoff,
                    &maker_pool.mandate,
                    &binding,
                )?;
                allocation.pq_authorization = Some(signature.pq);
                allocation.committee_signature = signature
                    .classical
                    .serialize()
                    .map_err(|_| "standing-pool FROST signature cannot be serialized")?;
                authorization.escrow_digest = allocation.statement(&transition, &authorization)?;

                // Preview the exact Maker split without changing canonical state.
                // The proof job must remain live until typed zkPI, DvP, and both
                // remainder proofs have been finalized below.
                let allocation_preview = market.preview_standing_note_pool_allocation(
                    &transition,
                    &authorization,
                    &allocation,
                )?;
                let maker_preview = &allocation_preview.allocation;

                let taker_reservation_receipt =
                    taker_note_reservation_receipt.as_ref().ok_or_else(|| {
                        "filled MPC execution has no canonical Taker reservation".to_string()
                    })?;
                let taker_mandate = admission.taker_mandate.as_ref().ok_or_else(|| {
                    "filled MPC execution omitted its signed Taker mandate".to_string()
                })?;
                let admission_receipt = defmi_admission.as_ref().ok_or_else(|| {
                    "filled MPC execution has no canonical admission batch".to_string()
                })?;
                let maker_reserve_receipt_digest = authorization.statement(&transition)?;
                let settlement_root = maker_preview.after_state_root;
                let taker_reservation =
                    market.note_reservation(taker_reservation_receipt.reservation.hold_id)?;
                let taker_hold = market.credit_hold(taker_reservation.hold_id)?;
                let taker_facility =
                    market.credit_facility(taker_reservation_receipt.facility_id)?;
                if [
                    taker_reservation.state_root,
                    taker_hold.state_root,
                    taker_facility.state_root,
                ]
                .iter()
                .any(|root| *root != maker_preview.before_state_root)
                    || market.state_root()? != maker_preview.before_state_root
                    || maker_preview.reservation_status != "active"
                    || taker_reservation.status != "active"
                    || maker_preview.hold_status != "active"
                    || taker_hold.status != "active"
                    || maker_preview.reserve_receipt_digest != maker_reserve_receipt_digest
                    || taker_reservation.reserve_receipt_digest
                        != taker_reservation_receipt.reserve_receipt_digest
                    || maker_preview.reservation_amount_commitment != child.compress().to_bytes()
                    || taker_reservation.amount_commitment
                        != taker_reservation_receipt.reservation.amount_commitment
                    || maker_preview.hold_amount_commitment
                        != maker_preview.reservation_amount_commitment
                    || taker_hold.amount_commitment != taker_reservation.amount_commitment
                    || maker_preview.reservation_hold_id != hold_id
                    || maker_preview.hold_facility_id != maker_pool.facility_id
                {
                    return Err(
                    "Maker preview and canonical Taker reservation do not share one DeFMI pre-state"
                        .into(),
                );
                }

                let maker_handle = CompressedRistretto(maker_pool.mandate.maker_handle)
                    .decompress()
                    .ok_or_else(|| "winning Maker handle is not canonical".to_string())?;
                let taker_handle = CompressedRistretto(taker_mandate.taker_handle)
                    .decompress()
                    .ok_or_else(|| "Taker handle is not canonical".to_string())?;
                let taker_mandate_digest = taker_mandate.digest()?;
                let authority_digest = allocation_hash(&[
                    b"QOMM:DEMO:PRETRADE-AUTHORITY:v1",
                    &job_id,
                    &mandate_digest,
                    &taker_mandate_digest,
                    &admission_receipt.admission_digest,
                    &settlement_root,
                ]);
                let receipt_key = development_receipt_signing_key();
                let acknowledgement = PretradeAcknowledgement {
                    authority_digest,
                    defmi_id: market.defmi_id(),
                    after_state_root: settlement_root,
                    bindings: vec![
                        PretradeReservationBinding {
                            party: ReservationParty::Maker,
                            owner_index: u16::try_from(opened_winner)
                                .map_err(|_| "winning Maker index exceeds u16")?,
                            direction: required_direction,
                            owner_handle: maker_pool.mandate.maker_handle,
                            facility_id: maker_pool.facility_id,
                            reserve_id: hold_id,
                            mandate_digest,
                            policy_digest: maker_pool.mandate.policy_digest,
                            amount_commitment: maker_preview.reservation_amount_commitment,
                            reserve_receipt_digest: maker_reserve_receipt_digest,
                        },
                        PretradeReservationBinding {
                            party: ReservationParty::Taker,
                            owner_index: 0,
                            direction: required_direction,
                            owner_handle: taker_mandate.taker_handle,
                            facility_id: taker_reservation_receipt.facility_id,
                            reserve_id: taker_reservation.hold_id,
                            mandate_digest: taker_mandate_digest,
                            policy_digest: ZERO,
                            amount_commitment: taker_reservation.amount_commitment,
                            reserve_receipt_digest: taker_reservation_receipt
                                .reserve_receipt_digest,
                        },
                    ],
                    signer_public: receipt_key.verifying_key().to_bytes(),
                    signature: Signature::from_bytes(&[0_u8; 64]),
                }
                .sign(&receipt_key)?;

                let joint_reserve_id = allocation_hash(&[
                    b"QOMM:DEMO:JOINT-SETTLEMENT-RESERVE:v1",
                    &maker_pool.facility_id,
                    &taker_reservation_receipt.facility_id,
                    &job_id,
                ]);
                let direction = match required_direction {
                    Direction::TakerBuys => TradeDirection::TakerBuys,
                    Direction::TakerSells => TradeDirection::TakerSells,
                };
                let context = ExecutionContext {
                    operation: OperationKind::Settle,
                    scope: AuthorizationScope::Joint,
                    direction,
                    venue_id: market.venue_id(),
                    defmi_id: market.defmi_id(),
                    maker_handle,
                    taker_handle,
                    reserve_handle: reserve_handle_for(&joint_reserve_id),
                    maker_reservation_id: hold_id,
                    maker_reservation_sequence: maker_preview.facility.sequence,
                    taker_reservation_id: taker_reservation.hold_id,
                    taker_reservation_sequence: taker_facility.facility.sequence,
                    rfq_nullifier: taker_mandate.rfq_nullifier,
                    taker_mandate_digest,
                    maker_policy_digest: maker_pool.mandate.policy_digest,
                    maker_mandate_digest: mandate_digest,
                    maker_reserve_receipt_digest,
                    taker_reserve_receipt_digest: taker_reservation_receipt.reserve_receipt_digest,
                    quote_proof_digest: quote_digest,
                    market_statement_digest: proof.handoff.quote_verification.public.market_digest,
                    before_state_root: settlement_root,
                };
                // All one-use proof outputs now exist. Consume every signer and
                // observer transcript first so each node durably records the
                // completed MPC evidence used by `authorize_typed`.  The typed
                // settlement authorization is deliberately unavailable while the
                // proof job is merely active.
                complete_product_proof(&mut proof_parties, job_id)?;
                finalize_product_settlement(
                    &mut proof_parties,
                    &mut proof.handoff,
                    context,
                    &acknowledgement,
                    &receipt_key.verifying_key(),
                )?;

                let typed = proof.handoff.typed_instruction()?;
                let bounds = Bounds {
                    amount_bits: self.config.zkpi_amount_bits,
                    price_bits: self.config.zkpi_price_bits,
                    max_horizon: 3_600,
                };
                let settlement_venue =
                    Venue::new(settlement_key.clone(), &bounds, frost_public.clone())
                        .require_threshold_ranges()
                        .require_pq_committee(market.registered_pq_committee(&frost_public)?)
                        .map_err(|error| error.to_string())?;
                let dvp = build_threshold_package_from_proofs(
                    &settlement_key,
                    typed.payment.clone(),
                    Sides::of(&typed.payment),
                    proof.handoff.securities_reserve,
                    proof.handoff.cash_reserve,
                    proof.handoff.cash_commitment,
                    proof.handoff.dvp_proofs.clone(),
                    self.config.dvp_remainder_bits,
                )?;
                if dvp.securities_remainder != proof.handoff.securities_remainder
                    || dvp.cash_remainder != proof.handoff.cash_remainder
                {
                    return Err("final DvP package differs from the proof-party handoff".into());
                }
                let maker_leg = DelegatedNoteLegProjection {
                    asset_id: maker_preview.reservation_asset_id,
                    hold_id,
                    escrow_note_id: maker_preview.escrow_note_id,
                    delegation_digest: maker_preview.reservation_delegation_digest,
                    reserve_commitment: maker_preview.reservation_amount_commitment,
                };
                let taker_leg = DelegatedNoteLegProjection {
                    asset_id: taker_reservation.asset_id,
                    hold_id: taker_reservation.hold_id,
                    escrow_note_id: taker_reservation.escrow_note_id,
                    delegation_digest: taker_reservation.delegation_digest,
                    reserve_commitment: taker_reservation.amount_commitment,
                };
                let (securities_leg, cash_leg) = match required_direction {
                    Direction::TakerBuys => (maker_leg, taker_leg),
                    Direction::TakerSells => (taker_leg, maker_leg),
                };
                let claim_authorization =
                    |opening: &qomm_proofs::opening_envelope::OpeningEnvelope,
                     asset: [u8; 32],
                     hold: [u8; 32],
                     kind: NoteClaimKind| {
                        NoteClaimAuthorization::generate(
                            note_claim_recipient_commitment(
                                opening.recipient_view.compress().to_bytes(),
                                typed.context.rfq_nullifier,
                                asset,
                                hold,
                                kind,
                            )?,
                            wall_now,
                            u64::MAX,
                        )?
                        .commitment()
                    };
                let openings = DelegatedClaimOpenings {
                    proof_job_id: job_id,
                    securities_delivery: proof.handoff.securities_delivery_opening.clone(),
                    securities_refund: proof.handoff.securities_refund_opening.clone(),
                    cash_delivery: proof.handoff.cash_delivery_opening.clone(),
                    cash_refund: proof.handoff.cash_refund_opening.clone(),
                    authorizations: [
                        claim_authorization(
                            &proof.handoff.securities_delivery_opening,
                            securities_leg.asset_id,
                            securities_leg.hold_id,
                            NoteClaimKind::Delivery,
                        )?,
                        claim_authorization(
                            &proof.handoff.securities_refund_opening,
                            securities_leg.asset_id,
                            securities_leg.hold_id,
                            NoteClaimKind::Refund,
                        )?,
                        claim_authorization(
                            &proof.handoff.cash_delivery_opening,
                            cash_leg.asset_id,
                            cash_leg.hold_id,
                            NoteClaimKind::Delivery,
                        )?,
                        claim_authorization(
                            &proof.handoff.cash_refund_opening,
                            cash_leg.asset_id,
                            cash_leg.hold_id,
                            NoteClaimKind::Refund,
                        )?,
                    ],
                };
                let operation_id = allocation_hash(&[
                    b"QOMM:DEMO:PRODUCT-SETTLEMENT-OP:v1",
                    &job_id,
                    &quote_digest,
                ]);
                let projection = VerifiedDelegatedNoteSettlementProjection::verify_and_project(
                    &settlement_venue,
                    &typed,
                    &dvp,
                    operation_id,
                    securities_leg,
                    cash_leg,
                    openings,
                    proof.handoff.quote_verification.public.market_digest,
                    wall_now,
                )?;
                let base_settlement_digest = projection.settlement.statement()?;
                let maker_hold_snapshot = CreditHoldSnapshot {
                    hold_id: maker_preview.reservation_hold_id,
                    facility_id: maker_preview.hold_facility_id,
                    query_commitment: maker_preview.hold_query_commitment,
                    amount_commitment: maker_preview.hold_amount_commitment,
                    expires_at: maker_preview.hold_expires_at,
                };
                let taker_hold_snapshot = CreditHoldSnapshot {
                    hold_id: taker_hold.hold_id,
                    facility_id: taker_hold.facility_id,
                    query_commitment: taker_hold.query_commitment,
                    amount_commitment: taker_hold.amount_commitment,
                    expires_at: taker_hold.expires_at,
                };
                let (maker_consumed, maker_refund, taker_consumed, taker_refund) =
                    match required_direction {
                        Direction::TakerBuys => (
                            typed.payment.amount_commitment,
                            dvp.securities_remainder,
                            dvp.cash_commitment,
                            dvp.cash_remainder,
                        ),
                        Direction::TakerSells => (
                            dvp.cash_commitment,
                            dvp.cash_remainder,
                            typed.payment.amount_commitment,
                            dvp.securities_remainder,
                        ),
                    };
                let maker_consumption = build_threshold_dvp_consumption_from_snapshot(
                    allocation_hash(&[b"QOMM:DEMO:MAKER-CONSUME-OP:v1", &job_id]),
                    &maker_hold_snapshot,
                    &maker_preview.facility,
                    ReservationRole::Maker,
                    maker_consumed,
                    maker_refund,
                    base_settlement_digest,
                    dvp.digest(),
                )?;
                let taker_consumption = build_threshold_dvp_consumption_from_snapshot(
                    allocation_hash(&[b"QOMM:DEMO:TAKER-CONSUME-OP:v1", &job_id]),
                    &taker_hold_snapshot,
                    &taker_facility.facility,
                    ReservationRole::Taker,
                    taker_consumed,
                    taker_refund,
                    base_settlement_digest,
                    dvp.digest(),
                )?;
                let price_limit = threshold_price_limit(
                    &settlement_key,
                    &typed.payment.price_commitment,
                    &proof.handoff.limit_commitment,
                    proof.handoff.limit_direction,
                    self.config.zkpi_price_bits,
                    &proof.handoff.limit_context,
                    proof.handoff.price_limit_proof.clone(),
                )?;
                let price_limit_proof_digest = price_limit.digest(
                    &typed.payment.price_commitment,
                    &proof.handoff.limit_commitment,
                    &proof.handoff.limit_context,
                );
                let asset_link = defmi::asset_link::prove(
                    &settlement_key,
                    proof.handoff.asset_id,
                    &typed.payment.asset_commitment,
                    &proof.handoff.asset_blinding,
                    &mut OsRng,
                )?;
                let asset_link_proof_digest =
                    asset_link.digest(&proof.handoff.asset_id, &typed.payment.asset_commitment);
                let order = projection.into_product(ProductNoteBindings {
                    maker_entity_commitment: maker_pool.mandate.entity_commitment,
                    taker_entity_commitment: taker_mandate.entity_commitment,
                    traded_asset_id: proof.handoff.asset_id,
                    price_limit_proof_digest,
                    asset_link_proof_digest,
                    admission_receipt_digest: admission_receipt.admission_digest,
                    admission_epoch: admission_receipt.epoch,
                    admission_sequence: admission.envelope.sequence,
                    reservations: vec![
                        ReservationConsumption {
                            role: ReservationRole::Maker,
                            reserve_receipt_digest: maker_reserve_receipt_digest,
                            transition: maker_consumption,
                        },
                        ReservationConsumption {
                            role: ReservationRole::Taker,
                            reserve_receipt_digest: taker_reservation_receipt
                                .reserve_receipt_digest,
                            transition: taker_consumption,
                        },
                    ],
                })?;
                let evidence = ProductSettlementEvidence {
                    typed_instruction: typed_wire::encode(&typed),
                    quote_verification: encode_quote_verification(
                        &proof.handoff.quote_verification,
                    )?,
                    price_limit_proof: encode_threshold_range(&proof.handoff.price_limit_proof)?,
                    dvp_proofs: encode_dvp_proofs(&proof.handoff.dvp_proofs)?,
                    mpc_execution_attestations: proof.execution_attestations.clone(),
                    asset_link,
                };
                let expected_settlement_statement = standing_pool_product_settlement_statement(
                    &transition,
                    &authorization,
                    &allocation,
                    &order,
                    evidence.digest()?,
                )?;
                let settled = market.settle_product_with_standing_pool(
                    &transition,
                    &authorization,
                    &allocation,
                    &allocation_preview,
                    &order,
                    &evidence,
                )?;
                let sdk_finality = accept_canonical_transition(
                    application_binding,
                    &CanonicalTransition {
                        transaction_id: settled.transaction_id.clone(),
                        block_id: settled.block_id.clone(),
                        height: settled.height,
                        statement: settled.statement,
                        before_state_root: settled.before_state_root,
                        after_state_root: settled.after_state_root,
                    },
                    expected_settlement_statement,
                    allocation_preview.allocation.before_state_root,
                    &settled.canonical_readbacks,
                )
                .map_err(|error| error.to_string())?;
                // DeFMI has accepted the allocation.  Each node now commits its
                // own remainder shares from this execution's persistence under a
                // compare-and-swap on the generation it executed with.  A node
                // that misses this (crash, selective abort) is caught up by the
                // next reconciliation from the same accepted execution.
                let maker_state_generations = receipts
                    .iter()
                    .map(|receipt| receipt.maker_state_generation)
                    .collect::<Vec<_>>();
                let commit_results = self.commit_maker_state_on_nodes(
                    &(0..self.n_parties).collect::<Vec<_>>(),
                    &maker_state_generations,
                    &round_id,
                    execution_generation,
                    opened_winner,
                    standing_rail(required_direction),
                    job_id,
                    allocation.remainder_note.note_id,
                    maker_pool.pool_id,
                    allocation.expected_pool_sequence.saturating_add(1),
                );
                let mut committed_nodes = Vec::new();
                let mut failed_commits = Vec::new();
                for (node, result) in commit_results.into_iter().enumerate() {
                    match result {
                        Ok(response) => committed_nodes.push(json!({
                            "node": node,
                            "generation": response.receipt.after_generation,
                            "receipt_digest": hex::encode(response.receipt.receipt_digest),
                        })),
                        Err(error) => {
                            eprintln!(
                                "qomm-demo: MPC node {node} did not commit the accepted Maker remainder (it will be caught up before the next RFQ): {error}"
                            );
                            failed_commits.push(json!({"node": node, "error": error}));
                        }
                    }
                }
                maker_state_commits = Some(json!({
                    "pool_id": hex::encode(maker_pool.pool_id),
                    "pool_sequence": allocation.expected_pool_sequence.saturating_add(1),
                    "committed": committed_nodes,
                    "failed": failed_commits,
                }));
                defmi_product_settlement = Some(json!({
                    "application_id": application_manifest.application_id.as_str(),
                    "application_manifest_digest": hex::encode(application_manifest_digest),
                    "application_execution_binding": hex::encode(application_binding),
                    "sdk_finality_receipt_digest": hex::encode(sdk_finality.receipt_digest),
                    "canonical_readback_digest": hex::encode(sdk_finality.canonical_readback_digest),
                    "canonical_readback_count": settled.canonical_readbacks.len(),
                    "transaction_id": settled.transaction_id,
                    "block_id": settled.block_id,
                    "height": settled.height,
                    "statement": hex::encode(settled.statement),
                    "before_state_root": hex::encode(settled.before_state_root),
                    "after_state_root": hex::encode(settled.after_state_root),
                    "maker_hold_id": hex::encode(settled.maker_hold_id),
                    "taker_hold_id": hex::encode(settled.taker_hold_id),
                    "claim_ids": settled.claim_ids.iter().map(hex::encode).collect::<Vec<_>>(),
                    "post_match_maker_signature": false,
                    "post_match_taker_signature": false,
                }));
                standing_pool_allocation = Some(json!({
                    "maker": opened_winner,
                    "pool_id": hex::encode(maker_preview.pool_id),
                    "facility_id": hex::encode(maker_pool.facility_id),
                    "hold_id": hex::encode(hold_id),
                    "sequence": maker_preview.pool_sequence,
                    "parent_note_id": hex::encode(allocation.previous_pool_note_id),
                    "escrow_note_id": hex::encode(allocation.escrow_note.note_id),
                    "remainder_note_id": hex::encode(maker_preview.current_pool_note_id),
                    "authorization_statement": hex::encode(authorization.statement(&transition)?),
                }));
                proof_job_id = Some(hex::encode(job_id));
                quote_proof_digest = Some(hex::encode(quote_digest));
                settlement_record = Some(encode_private_record(&proof.handoff)?);
                execution_attestations = Some(proof.execution_attestations);
                execution_node_keys = Some(proof.execution_node_keys);
            }
            if request.is_real == 1 && !filled {
                let market = self.defmi_market.as_ref().ok_or_else(|| {
                    "real no-fill execution has no authoritative DeFMI market".to_string()
                })?;
                let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                    "real no-fill execution lost its Taker participant".to_string()
                })?;
                let mandate = admission.taker_mandate.as_ref().ok_or_else(|| {
                    "real no-fill execution omitted its signed Taker mandate".to_string()
                })?;
                let signed_taker_mandate = admission.signed_mandate.clone().ok_or_else(|| {
                    "real no-fill execution omitted its canonical Taker mandate bytes".to_string()
                })?;
                let reservation = taker_note_reservation_receipt.as_ref().ok_or_else(|| {
                    "real no-fill execution has no canonical Taker reservation".to_string()
                })?;
                let admission_receipt = defmi_admission.as_ref().ok_or_else(|| {
                    "real no-fill execution has no canonical admission batch".to_string()
                })?;
                let evidence = MpcNoFillEvidence {
                    signed_taker_mandate,
                    public_result_attestations: public_result_attestations.clone(),
                    fill_mask,
                };
                let release_now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| "system clock is before Unix epoch".to_string())?
                    .as_secs();
                let released = market.release_taker_no_fill(
                    &signer.snapshot,
                    mandate,
                    reservation,
                    &certified_admission,
                    admission_receipt,
                    admission.maximum_amount,
                    admission.maximum_blinding,
                    &evidence,
                    release_now,
                )?;
                taker_no_fill_release = Some(json!({
                    "reserve_id": hex::encode(released.hold_id),
                    "escrow_note_id": hex::encode(released.escrow_note_id),
                    "asset_id": hex::encode(released.asset_id),
                    "status": released.status,
                    "settlement_digest": hex::encode(released.settlement_digest),
                    "state_root": hex::encode(released.state_root),
                    "certified_nodes": public_results.len(),
                    "post_match_taker_signature": false,
                }));
            }
            let mut corporate_outbox_state = None;
            if let Some((request_id, request_digest, _, _)) = corporate_dispatch.as_ref() {
                let signer = self.pretrade_signer.as_ref().ok_or_else(|| {
                    "corporate dispatch lost its participant module before receipt recording"
                        .to_string()
                })?;
                signer.client.record_corporate_mpc_admission(
                    &signer.snapshot,
                    request_id,
                    *request_digest,
                    &hex::encode(certified_admission.cluster_digest),
                    &round_id,
                )?;
                let reconciliation = signer.client.reconcile_corporate_request(
                    &signer.snapshot,
                    request_id,
                    *request_digest,
                )?;
                let expected_hold = admission
                    .taker_mandate
                    .as_ref()
                    .map(|mandate| mandate.reserve_id)
                    .ok_or_else(|| {
                        "queue reconciliation lost its signed Taker mandate".to_string()
                    })?;
                let expected_status = if filled { "consumed" } else { "released" };
                if reconciliation.status != expected_status
                    || !reconciliation.queue_finalized
                    || reconciliation.hold_id != expected_hold
                {
                    return Err(format!(
                    "corporate queue did not finalize the canonical {expected_status} transition"
                ));
                }
                let state = expected_status;
                corporate_outbox_state = Some(state);
            }
            self.served = match corporate_dispatch.as_ref() {
                Some((_, _, sequence, _)) => self.served.max(sequence.saturating_add(1)),
                None => self.served.saturating_add(1),
            };
            Ok(MpcRound {
                outcome,
                filled,
                masked_key,
                mask,
                node_shares,
                named: BTreeMap::new(),
                verified,
                detail,
                stats: json!({
                    "distributed": true,
                    "wall_ms": started.elapsed().as_secs_f64() * 1_000.0,
                    "parties": self.n_parties,
                    "defmi_admission": defmi_admission.as_ref().map(|receipt| json!({
                        "epoch": receipt.epoch,
                        "batch_id": hex::encode(receipt.batch_id),
                        "admission_digest": hex::encode(receipt.admission_digest),
                        "after_state_root": hex::encode(receipt.after_state_root),
                    })),
                    "taker_note_reservation": taker_note_reservation,
                    "taker_no_fill_release": taker_no_fill_release,
                    "standing_pool_allocation": standing_pool_allocation,
                    "defmi_product_settlement": defmi_product_settlement,
                    "maker_state": maker_state_reconciliation,
                    "maker_state_commits": maker_state_commits,
                    "corporate_outbox": corporate_dispatch.as_ref().map(|(request_id, request_digest, sequence, attempt)| json!({
                        "request_id": request_id,
                        "request_digest": hex::encode(request_digest),
                        "sequence": sequence,
                        "attempt": attempt,
                        "state": corporate_outbox_state.unwrap_or("mpc_admitted"),
                        "local_execution_fallback": false,
                    })),
                    "containers": receipts.iter().map(|receipt| json!({
                        "node": receipt.node,
                        "elapsed_ms": receipt.elapsed_ms,
                        "stdout_sha256": receipt.stdout_sha256,
                        "stderr_sha256": receipt.stderr_sha256,
                    })).collect::<Vec<_>>(),
                }),
                product_handoff: Some(MpcProductHandoff {
                    round_id,
                    source_sha256: self.source_sha256.clone(),
                    persistence_sha256,
                    frost_public_sha256,
                    proof_job_id,
                    quote_proof_digest,
                    settlement_record,
                    execution_attestations,
                    execution_node_keys,
                    admission_attestations: Some(admission_wire),
                    admission_node_keys: Some(
                        admission_node_keys
                            .iter()
                            .map(VerifyingKey::to_bytes)
                            .collect(),
                    ),
                    signed_taker_mandate: admission.signed_mandate,
                    signed_maker_mandates,
                }),
            })
        })();
        match execution {
            Err(error) if corporate_reserve_active && !is_corporate_queue_pending(&error) => {
                self.retained_corporate = corporate_identity;
                self.retained_corporate_checked = None;
                Err(format!("{CORPORATE_QUEUE_RECONCILING}: {error}"))
            }
            Err(error) if corporate_reserve_active => {
                self.retained_corporate = corporate_identity;
                self.retained_corporate_checked = None;
                Err(error)
            }
            result => {
                self.retained_corporate = None;
                result
            }
        }
    }

    /// A request whose
    /// no-fill refund lost the race to the participant module's own release
    /// ends `released` on canonical state while the room still holds the
    /// reserve for a replay; the ticker never claims a finalized entry, so
    /// without this the room would refuse every later Taker request with
    /// "the previous Taker reservation is still active".
    fn retained_corporate_finalized(&mut self) -> Result<Option<String>, String> {
        let Some((request_id, request_digest)) = self.retained_corporate.clone() else {
            return Ok(None);
        };
        if self.preclaimed_replay.is_some() {
            return Ok(None);
        }
        if self
            .retained_corporate_checked
            .is_some_and(|checked| checked.elapsed() < Duration::from_secs(5))
        {
            return Ok(None);
        }
        self.retained_corporate_checked = Some(Instant::now());
        let Some(signer) = self.pretrade_signer.as_ref() else {
            return Ok(None);
        };
        let reconciliation = signer.client.reconcile_corporate_request(
            &signer.snapshot,
            &request_id,
            request_digest,
        )?;
        if !reconciliation.queue_finalized {
            return Ok(None);
        }
        self.retained_corporate = None;
        Ok(Some(reconciliation.status))
    }

    fn replay_queued(&mut self) -> Result<Option<QueuedMpcRound>, String> {
        if self.preclaimed_replay.is_some() {
            return Err("a corporate replay is already in progress".into());
        }
        let signer = match self.pretrade_signer.as_ref() {
            Some(signer) => signer.clone(),
            None => return Ok(None),
        };
        let quorum_healthy = self.committee_healthy();
        let action =
            signer
                .client
                .claim_corporate_cover_slot(&signer.snapshot, quorum_healthy, 2, 1)?;
        let claimed = match action {
            CorporateOutboxAction::Real(claimed) => claimed,
            CorporateOutboxAction::Expire {
                request_id,
                request_digest,
                signed_request,
                ..
            } => {
                let wall_now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| "distributed settlement time is before Unix epoch")?
                    .as_secs();
                self.release_expired_corporate(
                    &signer,
                    &request_id,
                    request_digest,
                    signed_request,
                    wall_now,
                )?;
                return Ok(None);
            }
            CorporateOutboxAction::Dummy { .. } | CorporateOutboxAction::NotDue => return Ok(None),
        };
        let queued: QueuedRfqEnvelope = serde_json::from_slice(&claimed.signed_request)
            .map_err(|_| "claimed corporate RFQ is not canonical JSON".to_string())?;
        if queued.version != 3
            || queued.request.is_real != 1
            || queued.policies.len() != self.n_makers
        {
            return Err("queued RFQ has an unsupported replay schema or market shape".into());
        }
        let policies = queued.policies.clone();
        let request = queued.request.clone();
        let settlement = queued.settlement.clone();
        let market_time = queued.market_time;

        // A gateway restart reconstructs its in-memory Maker authority cache
        // from the exact policy and reserve snapshot stored with the RFQ.  The
        // resulting Ed25519 mandate bytes are deterministic and the standing
        // pools must already exist on canonical DeFMI from before the RFQ was
        // accepted; `QueuedRfqEnvelope::verify` below rejects any different
        // authority set.  Never substitute the room's current policies here.
        let reconciliation = signer.client.reconcile_corporate_request(
            &signer.snapshot,
            &claimed.request_id,
            claimed.request_digest,
        )?;
        if reconciliation.queue_finalized {
            return Ok(None);
        }
        if !matches!(reconciliation.status.as_str(), "not_reserved" | "active") {
            return Err("queued RFQ has no executable corporate or DeFMI reserve".into());
        }
        self.require_existing_maker_authority = true;
        let authority = self.preauthorize_maker_policies(&policies, &settlement, market_time);
        self.require_existing_maker_authority = false;
        authority?;
        self.preclaimed_replay = Some(claimed);
        let round = match self.quote(&policies, &request, &settlement, market_time, &[]) {
            Ok(round) => round,
            Err(error) if is_corporate_queue_pending(&error) => MpcRound {
                outcome: Outcome::default(),
                filled: false,
                masked_key: 0,
                mask: 0,
                node_shares: BTreeMap::new(),
                named: BTreeMap::new(),
                verified: false,
                detail: error.clone(),
                stats: json!({
                    "distributed": true,
                    "corporate_reconciliation": true,
                    "error": error,
                }),
                product_handoff: None,
            },
            Err(error) => return Err(error),
        };
        Ok(Some(QueuedMpcRound {
            policies,
            request,
            settlement,
            market_time,
            round,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct MpcNodeConfig {
    pub node: usize,
    pub n_parties: usize,
    pub threshold: usize,
    pub n_makers: usize,
    pub references: Vec<i64>,
    pub bit_length: u32,
    pub input_check: bool,
    pub listen_host: String,
    pub api_port: u16,
    pub mp_spdz_root: PathBuf,
    pub state_root: PathBuf,
    pub party_hosts: Vec<String>,
    pub timeout: Duration,
}

struct PreparedNode {
    config: MpcNodeConfig,
    source_sha256: String,
    program: String,
    party_binary: PathBuf,
    host_file: PathBuf,
    proof_party: Mutex<ProofParty>,
    /// Encrypted, node-local shares of every Maker's standing reserve
    /// openings.  It is seeded once per circuit, advanced only by
    /// compare-and-swap commits proved from this node's own MP-SPDZ
    /// persistence, and never read back by the coordinator.
    maker_state: EncryptedMpcStateStore,
    padded_makers: usize,
}

fn read_certified_admission(
    path: &Path,
    expected: &AdmissionAttestationWire,
) -> Result<AdmissionAttestationWire, String> {
    let stored: AdmissionAttestationWire = serde_json::from_slice(
        &fs::read(path)
            .map_err(|_| "MPC input was sent before this node durably admitted the slot")?,
    )
    .map_err(|_| "stored MPC admission receipt is malformed".to_string())?;
    if &stored != expected {
        return Err("MPC execution carries another admission receipt".into());
    }
    Ok(stored)
}

impl PreparedNode {
    fn prepare(config: MpcNodeConfig) -> Result<Self, String> {
        if config.n_parties != 7
            || config.threshold != 2
            || config.node >= config.n_parties
            || config.party_hosts.len() != config.n_parties
            || config.references.is_empty()
            || config.api_port == 0
            || config.timeout.is_zero()
        {
            return Err("MPC node configuration is outside the approved demo shape".into());
        }
        for host in &config.party_hosts {
            if host.trim().is_empty() || !host.contains(':') || host.contains('/') {
                return Err("each MP-SPDZ party host must be host:port".into());
            }
        }
        fs::create_dir_all(&config.state_root).map_err(|error| error.to_string())?;
        fs::set_permissions(&config.state_root, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let padded = pow2_ceil(config.n_makers).map_err(|error| error.to_string())?;
        let program_config = ProgramConfig {
            n_mm: padded,
            n_parties: config.n_parties,
            n_assets: config.references.len(),
            ref_table: config.references.iter().copied().map(i128::from).collect(),
            maker_assets: (0..padded)
                .map(|maker| maker % config.references.len())
                .collect(),
            bit_length: config.bit_length,
            input_check: config.input_check,
            check_mode: CheckMode::PerParty,
            public_maker_assets: true,
            binding_limit: true,
            stop_after: StopAfter::Tournament,
            persist_wires: true,
            persist_zkpi_wires: true,
            persist_quote_proof_wires: true,
            persist_dvp_wires: true,
            zkpi_amount_bits: PRODUCT_ZKPI_AMOUNT_BITS,
            zkpi_price_bits: PRODUCT_ZKPI_PRICE_BITS,
            dvp_remainder_bits: PRODUCT_DVP_REMAINDER_BITS,
            quote_eligibility_bits: PRODUCT_QUOTE_ELIGIBILITY_BITS,
            quote_span_bits: PRODUCT_QUOTE_SPAN_BITS,
            reference: Reference::Anchored,
            ..ProgramConfig::default()
        };
        let source = build_program(&program_config).map_err(|error| error.to_string())?;
        let source_sha256 = hex::encode(Sha256::digest(source.as_bytes()));
        let program = format!("qomm_demo_network_{}", &source_sha256[..16]);
        let source_path = config
            .mp_spdz_root
            .join("Programs/Source")
            .join(format!("{program}.mpc"));
        if let Some(parent) = source_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(&source_path, source).map_err(|error| error.to_string())?;
        let schedule = config
            .mp_spdz_root
            .join("Programs/Schedules")
            .join(format!("{program}.sch"));
        if !schedule.is_file() {
            let compiler = OfficialCompiler::from_checkout(&config.mp_spdz_root)
                .map_err(|error| error.to_string())?;
            let output = compiler
                .compile_field(253, &program)
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "official MP-SPDZ compiler rejected the demo circuit: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        let party_binary = config.mp_spdz_root.join("malicious-shamir-party.x");
        if !party_binary.is_file() {
            return Err(format!(
                "stock MP-SPDZ party binary is absent: {}",
                party_binary.display()
            ));
        }
        let host_file = config.state_root.join("mp-spdz-hosts");
        let contents = config
            .party_hosts
            .iter()
            .map(|host| format!("{host}\n"))
            .collect::<String>();
        atomic_private_write(&host_file, contents.as_bytes())?;
        let proof_passphrase_path = config.state_root.join("proof-party.passphrase");
        if !proof_passphrase_path.exists() {
            let mut entropy = [0_u8; 32];
            OsRng.fill_bytes(&mut entropy);
            atomic_private_write(&proof_passphrase_path, hex::encode(entropy).as_bytes())?;
            entropy.fill(0);
        }
        let mut proof_passphrase = fs::read(&proof_passphrase_path)
            .map_err(|error| format!("proof-party passphrase is unavailable: {error}"))?;
        let proof_party = ProofParty::new(ProofPartyConfig {
            recipient_opening_keys: Vec::new(),
            node: u16::try_from(config.node)
                .map_err(|_| "MPC node index exceeds proof-party bounds")?,
            allowed_root: config.state_root.clone(),
            // Proof security parameters are part of the durable state
            // identity. Keep the previous transcript available for audit when
            // a circuit-width migration starts a fresh market epoch.
            state_file: config.state_root.join(format!(
                "proof-party-state-a{}-r{}.qps",
                PRODUCT_ZKPI_AMOUNT_BITS, PRODUCT_DVP_REMAINDER_BITS
            )),
            state_passphrase: proof_passphrase.clone(),
            n_mm: padded,
            n_parties: config.n_parties,
            threshold: config.threshold,
            amount_bits: PRODUCT_ZKPI_AMOUNT_BITS,
            price_bits: PRODUCT_ZKPI_PRICE_BITS,
            remainder_bits: PRODUCT_DVP_REMAINDER_BITS,
            complete_quote_proof: true,
            quote_eligibility_bits: PRODUCT_QUOTE_ELIGIBILITY_BITS,
            quote_span_bits: PRODUCT_QUOTE_SPAN_BITS,
            // The public Docker network uses one explicitly non-production
            // receipt authority. Production nodes load the governance-pinned
            // key from their own deployment configuration/HSM policy.
            trusted_defmi_receipt_public: Some(crate::defmi_bootstrap::development_receipt_public()),
            allow_health_signing: false,
        })?;
        proof_passphrase.fill(0);
        let maker_passphrase_path = config.state_root.join("maker-state.passphrase");
        if !maker_passphrase_path.exists() {
            let mut entropy = [0_u8; 32];
            OsRng.fill_bytes(&mut entropy);
            atomic_private_write(&maker_passphrase_path, hex::encode(entropy).as_bytes())?;
            entropy.fill(0);
        }
        let mut maker_passphrase = fs::read(&maker_passphrase_path)
            .map_err(|error| format!("Maker-state passphrase is unavailable: {error}"))?;
        // The state file is keyed by circuit: a circuit-width migration starts
        // a fresh Maker epoch and keeps the previous encrypted state for audit.
        let maker_state = EncryptedMpcStateStore::new(
            config
                .state_root
                .join(format!("maker-state-{}.qms", &source_sha256[..16])),
            &maker_passphrase,
        )?;
        maker_passphrase.fill(0);
        Ok(Self {
            config,
            source_sha256,
            program,
            party_binary,
            host_file,
            proof_party: Mutex::new(proof_party),
            maker_state,
            padded_makers: padded,
        })
    }

    fn node_u16(&self) -> Result<u16, String> {
        u16::try_from(self.config.node).map_err(|_| "MPC node index exceeds u16".to_string())
    }

    fn loaded_maker_state(&self) -> Result<MpcSecretState, String> {
        if !self.maker_state.exists() {
            return Err(
                "MPC node has no resident Maker state; the coordinator must seed the standing pools before the first execution"
                    .into(),
            );
        }
        let state = self.maker_state.load()?;
        state.verify(self.node_u16()?, &self.source_sha256, self.padded_makers)?;
        Ok(state)
    }

    fn maker_state_view(&self) -> Result<MakerStateView, String> {
        if !self.maker_state.exists() {
            return Ok(MakerStateView {
                node: self.config.node,
                initialized: false,
                generation: 0,
                source_sha256: self.source_sha256.clone(),
                sharing: MAKER_STATE_SHARING,
                n_mm: self.padded_makers,
                bindings: Vec::new(),
            });
        }
        let state = self.loaded_maker_state()?;
        let bindings = state
            .standing_pool_bindings
            .iter()
            .map(|binding| {
                Ok(MakerStateBindingView {
                    maker: binding.maker,
                    direction: binding.direction,
                    pool_id: hex::encode(binding.pool_id),
                    pool_sequence: binding.pool_sequence,
                    partial_commitment: hex::encode(
                        state.standing_pool_partial_commitment(binding.maker, binding.direction)?,
                    ),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(MakerStateView {
            node: self.config.node,
            initialized: true,
            generation: state.generation,
            source_sha256: self.source_sha256.clone(),
            sharing: MAKER_STATE_SHARING,
            n_mm: self.padded_makers,
            bindings,
        })
    }

    fn validate_maker_state_scope(
        &self,
        version: u8,
        node: usize,
        source_sha256: &str,
    ) -> Result<(), String> {
        if version != PROTOCOL_VERSION
            || node != self.config.node
            || source_sha256 != self.source_sha256
        {
            return Err("Maker-state request names another node, circuit, or protocol".into());
        }
        Ok(())
    }

    /// Install the initial additive shares of every standing pool opening.
    /// Refused once a state exists: a restarted coordinator must reconcile
    /// against the node's state, not overwrite it with an initial balance.
    fn seed_maker_state(&self, request: MakerStateSeedRequest) -> Result<MakerStateView, String> {
        self.validate_maker_state_scope(request.version, request.node, &request.source_sha256)?;
        if self.maker_state.exists() {
            return Err(
                "MPC node already holds resident Maker state; reconcile it instead of re-seeding"
                    .into(),
            );
        }
        if request
            .bindings
            .iter()
            .any(|binding| binding.pool_sequence != 0)
        {
            return Err(
                "a seed may only describe pools that have never been allocated from".into(),
            );
        }
        let state = MpcSecretState {
            version: 1,
            node: self.node_u16()?,
            generation: 1,
            source_sha256: self.source_sha256.clone(),
            dvp_input_shares: request.dvp_input_shares,
            policy_input_shares: Vec::new(),
            quote_policy_blinding_input_shares: Vec::new(),
            standing_pool_bindings: request.bindings,
        };
        state.verify(self.node_u16()?, &self.source_sha256, self.padded_makers)?;
        self.maker_state.initialize(&state)?;
        self.maker_state_view()
    }

    fn maker_state_receipt_path(&self, round_id: &str, generation: u32) -> Result<PathBuf, String> {
        Ok(
            execution_directory(&self.round_directory(round_id), generation)?
                .join(format!("maker-state-receipt-P{}.json", self.config.node)),
        )
    }

    /// Advance one Maker rail to the remainder this node itself computed in an
    /// execution that canonical DeFMI accepted.  Idempotent for the same
    /// accepted allocation so a coordinator retry after a lost response does
    /// not double-apply.
    fn commit_maker_state(
        &self,
        request: MakerStateCommitRequest,
    ) -> Result<MakerStateCommitResponse, String> {
        self.validate_maker_state_scope(request.version, request.node, &request.source_sha256)?;
        if request.round_id.len() != 64
            || !request
                .round_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Maker-state commit names an invalid round".into());
        }
        let proof_job_id = decode_hex32(&request.proof_job_id, "proof job")?;
        let remainder_note_id = decode_hex32(&request.remainder_note_id, "remainder note")?;
        let pool_id = decode_hex32(&request.pool_id, "standing pool")?;
        if request.pool_sequence == 0 {
            return Err("an allocated pool cannot be at sequence zero".into());
        }
        let round = self.round_directory(&request.round_id);
        let receipt_path = execution_receipt_path(&round, request.execution_generation)?;
        let execution_receipt: ExecuteReceipt = serde_json::from_slice(
            &fs::read(&receipt_path)
                .map_err(|_| "this node has no receipt for the named execution".to_string())?,
        )
        .map_err(|_| "stored MPC execution receipt is malformed".to_string())?;
        let persistence = execution_directory(&round, request.execution_generation)?
            .join("Persistence")
            .join(format!("Transactions-P{}.data", self.config.node));
        let persistence_digest =
            hex::encode(Sha256::digest(fs::read(&persistence).map_err(|_| {
                "this node no longer holds the persistence of the named execution".to_string()
            })?));
        if persistence_digest != execution_receipt.persistence_sha256 {
            return Err("node-local persistence differs from its signed execution receipt".into());
        }
        let binding = StandingPoolBinding {
            maker: request.maker,
            direction: request.direction,
            pool_id,
            pool_sequence: request.pool_sequence,
        };
        let stored_receipt_path =
            self.maker_state_receipt_path(&request.round_id, request.execution_generation)?;
        let current = self.loaded_maker_state()?;
        if stored_receipt_path.is_file() {
            let stored: StandingPoolStateReceipt = serde_json::from_slice(
                &fs::read(&stored_receipt_path).map_err(|error| error.to_string())?,
            )
            .map_err(|_| "stored Maker-state receipt is malformed".to_string())?;
            stored.verify()?;
            if stored.proof_job_id == proof_job_id
                && stored.allocation_statement == remainder_note_id
                && stored.maker == request.maker
                && stored.direction == request.direction
                && current.standing_pool_binding(request.maker, request.direction) == Some(&binding)
            {
                return Ok(MakerStateCommitResponse {
                    receipt: stored,
                    state: self.maker_state_view()?,
                });
            }
        }
        let receipt = commit_standing_pool_remainder_with_store(StandingPoolCommitRequest {
            store: &self.maker_state,
            node: self.node_u16()?,
            n_parties: u16::try_from(self.config.n_parties)
                .map_err(|_| "MPC party count exceeds u16".to_string())?,
            n_mm: self.padded_makers,
            source_sha256: &self.source_sha256,
            sharing: MAKER_STATE_SHARING,
            persistence_path: &persistence,
            maker: request.maker,
            direction: request.direction,
            expected_generation: request.expected_generation,
            proof_job_id,
            allocation_statement: remainder_note_id,
            binding: Some(binding),
            amount_bits: PRODUCT_ZKPI_AMOUNT_BITS,
            price_bits: PRODUCT_ZKPI_PRICE_BITS,
            remainder_bits: PRODUCT_DVP_REMAINDER_BITS,
            eligibility_bits: PRODUCT_QUOTE_ELIGIBILITY_BITS,
            span_bits: PRODUCT_QUOTE_SPAN_BITS,
        })?;
        atomic_private_write(
            &stored_receipt_path,
            &serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?,
        )?;
        Ok(MakerStateCommitResponse {
            receipt,
            state: self.maker_state_view()?,
        })
    }

    /// Point one Maker rail at a freshly registered pool.  Only pools that have
    /// never been allocated from may be installed this way.
    fn rebind_maker_state(
        &self,
        request: MakerStateRebindRequest,
    ) -> Result<MakerStateView, String> {
        self.validate_maker_state_scope(request.version, request.node, &request.source_sha256)?;
        let pool_id = decode_hex32(&request.pool_id, "standing pool")?;
        rebind_standing_pool(
            &self.maker_state,
            self.node_u16()?,
            self.padded_makers,
            &self.source_sha256,
            request.expected_generation,
            StandingPoolBinding {
                maker: request.maker,
                direction: request.direction,
                pool_id,
                pool_sequence: 0,
            },
            &request.amount_share,
            &request.blinding_share,
        )?;
        self.maker_state_view()
    }

    /// Public digests of every execution this node still holds, oldest first.
    fn round_receipts(&self) -> Result<Vec<NodeRoundReceiptView>, String> {
        let rounds = self.config.state_root.join("rounds");
        let mut views = Vec::new();
        let Ok(entries) = fs::read_dir(&rounds) else {
            return Ok(views);
        };
        let read_receipt = |path: PathBuf| -> Option<NodeRoundReceiptView> {
            let raw = fs::read(&path).ok()?;
            let receipt = serde_json::from_slice::<ExecuteReceipt>(&raw).ok()?;
            Some(NodeRoundReceiptView::from_receipt(&receipt))
        };
        for entry in entries.flatten() {
            let round = entry.path();
            if !round.is_dir() {
                continue;
            }
            views.extend(read_receipt(round.join("receipt.json")));
            if let Ok(attempts) = fs::read_dir(round.join("recovery")) {
                for attempt in attempts.flatten() {
                    views.extend(read_receipt(attempt.path().join("receipt.json")));
                }
            }
            if views.len() > 4_096 {
                return Err("MPC node holds more execution receipts than the listing bound".into());
            }
        }
        views.sort_by_key(|view| {
            (
                view.public_market_time,
                view.round_id.clone(),
                view.execution_generation,
            )
        });
        Ok(views)
    }

    fn admission_attestation(
        &self,
        request: &AdmitRequest,
    ) -> Result<AdmissionAttestationWire, String> {
        let ticket_id = decode_hex32(&request.admission.ticket_id, "admission ticket")?;
        let claim_digest = decode_hex32(&request.admission.claim_digest, "admission claim")?;
        let batch_digest = decode_hex32(&request.input_sha256, "MPC input digest")?;
        let order_digest = decode_hex32(&request.admission.order_digest, "admission order")?;
        let (attestation, identity_public) = self
            .proof_party
            .lock()
            .map_err(|_| "proof-party state lock is poisoned".to_string())?
            .sign_admission_attestation(
                request.admission.slot,
                request.admission.sequence,
                &request.admission.principal,
                ticket_id,
                claim_digest,
                batch_digest,
                order_digest,
            )?;
        Ok(AdmissionAttestationWire {
            node: attestation.node,
            slot: attestation.slot,
            sequence: attestation.sequence,
            principal_digest: hex::encode(attestation.principal_digest),
            ticket_id: hex::encode(attestation.ticket_id),
            claim_digest: hex::encode(attestation.claim_digest),
            batch_digest: hex::encode(attestation.batch_digest),
            order_digest: hex::encode(attestation.order_digest),
            identity_public: hex::encode(identity_public),
            signature: hex::encode(attestation.signature.to_bytes()),
        })
    }

    fn validate_admit_request(&self, request: &AdmitRequest) -> Result<(), String> {
        if request.version != PROTOCOL_VERSION
            || request.node != self.config.node
            || request.source_sha256 != self.source_sha256
            || request.round_id.len() != 64
            || !request
                .round_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || request.public_market_time < 0
            || decode_hex32(&request.input_sha256, "MPC input digest").is_err()
            || request.admission.principal.is_empty()
            || request.admission.principal.len() > 256
        {
            return Err(
                "MPC admission request failed its circuit, node, digest, or identity bounds".into(),
            );
        }
        Ok(())
    }

    fn round_directory(&self, round_id: &str) -> PathBuf {
        self.config.state_root.join("rounds").join(round_id)
    }

    fn admit(&self, request: AdmitRequest) -> Result<AdmissionAttestationWire, String> {
        self.validate_admit_request(&request)?;
        let round = self.round_directory(&request.round_id);
        fs::create_dir_all(&round).map_err(|error| error.to_string())?;
        fs::set_permissions(&round, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let path = round.join("admission.json");
        if path.is_file() {
            let stored: AdmissionAttestationWire =
                serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                    .map_err(|_| "stored MPC admission receipt is malformed".to_string())?;
            // ML-DSA signing is randomized. Verify the durable receipt against
            // this node's enrolled key and compare its statement, without re-signing.
            let trusted = self
                .proof_party
                .lock()
                .map_err(|_| "proof-party state lock is poisoned".to_string())?
                .application_verifying_key();
            let (attestation, identity) = decode_admission_receipt(&stored)?;
            let slot = u32::try_from(request.admission.slot)
                .map_err(|_| "admission slot is outside the resident-node range".to_string())?;
            let expected = NodeAdmissionAttestation {
                node: self.node_u16()?,
                slot: request.admission.slot,
                sequence: request.admission.sequence,
                principal_digest: qomm_transport::order::admission_principal_digest(
                    &request.admission.principal,
                )?,
                ticket_id: principal_ticket_id(slot, &request.admission.principal)?,
                claim_digest: decode_hex32(&request.admission.claim_digest, "admission claim")?,
                batch_digest: decode_hex32(&request.input_sha256, "MPC input digest")?,
                order_digest: decode_hex32(&request.admission.order_digest, "admission order")?,
                signature: Signature::from_bytes(&[]),
            };
            if identity != trusted
                || !attestation.verify(&trusted)
                || decode_hex32(&request.admission.ticket_id, "admission ticket")?
                    != expected.ticket_id
                || attestation.unsigned()? != expected.unsigned()?
            {
                return Err("round id was already admitted with another claim, input digest, or node identity".into());
            }
            return Ok(stored);
        }
        let attestation = self.admission_attestation(&request)?;
        atomic_private_write(
            &path,
            &serde_json::to_vec_pretty(&attestation).map_err(|error| error.to_string())?,
        )?;
        Ok(attestation)
    }

    fn load_admission(&self, request: &ExecuteRequest) -> Result<AdmissionAttestationWire, String> {
        let admitted = AdmitRequest {
            version: request.version,
            node: request.node,
            round_id: request.round_id.clone(),
            source_sha256: request.source_sha256.clone(),
            public_market_time: request.public_market_time,
            input_sha256: request.input_sha256.clone(),
            admission: request.admission.clone(),
        };
        self.validate_admit_request(&admitted)?;
        let path = self
            .round_directory(&request.round_id)
            .join("admission.json");
        read_certified_admission(&path, &request.certified_admission)
    }

    fn execute(&self, mut request: ExecuteRequest) -> Result<ExecuteReceipt, String> {
        if request.version != PROTOCOL_VERSION
            || request.node != self.config.node
            || request.source_sha256 != self.source_sha256
            || request.round_id.len() != 64
            || !request
                .round_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || request.input.len() > MAX_INPUT_BYTES
            || request.input.is_empty()
            || !(1..=64).contains(&request.execution_generation)
            || request.public_market_time < 0
            || request.input_sha256
                != hex::encode(execution_input_digest(
                    request.public_market_time,
                    request.input.as_bytes(),
                ))
            || !request.input.bytes().all(|byte| {
                byte.is_ascii_digit() || matches!(byte, b'-' | b' ' | b'\n' | b'\r' | b'\t')
            })
        {
            return Err(
                "MPC execution request failed its circuit, node, digest, or input bounds".into(),
            );
        }
        let admission_attestation = self.load_admission(&request)?;
        let round = self.round_directory(&request.round_id);
        fs::create_dir_all(&round).map_err(|error| error.to_string())?;
        fs::set_permissions(&round, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let execution = execution_directory(&round, request.execution_generation)?;
        let receipt_path = execution_receipt_path(&round, request.execution_generation)?;
        let canonical_receipt_path = round.join("canonical-receipt.json");
        let legacy_receipt_path = round.join("receipt.json");
        let read_receipt = |path: &Path| -> Result<ExecuteReceipt, String> {
            serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("stored MPC execution receipt is malformed: {error}"))
        };
        let matches_request = |receipt: &ExecuteReceipt, generation: Option<u32>| {
            receipt.input_sha256 == request.input_sha256
                && receipt.source_sha256 == request.source_sha256
                && receipt.public_market_time == request.public_market_time
                && receipt.admission == admission_attestation
                && generation.is_none_or(|expected| receipt.execution_generation == expected)
        };
        let canonical_receipt = if canonical_receipt_path.is_file() {
            Some(read_receipt(&canonical_receipt_path)?)
        } else if legacy_receipt_path.is_file() {
            Some(read_receipt(&legacy_receipt_path)?)
        } else {
            None
        };
        if canonical_receipt
            .as_ref()
            .is_some_and(|receipt| !matches_request(receipt, None))
        {
            return Err("round id was already used with a different party input".into());
        }
        if receipt_path.is_file() {
            let receipt = read_receipt(&receipt_path)?;
            if matches_request(&receipt, Some(request.execution_generation)) {
                return Ok(receipt);
            }
            return Err("execution generation was already used with another party input".into());
        }
        fs::create_dir_all(execution.join("Persistence")).map_err(|error| error.to_string())?;
        fs::set_permissions(&execution, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        ensure_round_programs(
            &execution,
            &self.config.mp_spdz_root,
            &self.program,
            request.public_market_time,
        )?;
        ensure_round_directory_link(
            &execution.join("Player-Data"),
            &self.config.mp_spdz_root.join("Player-Data"),
        )?;
        let prefix = execution.join("Input");
        let input_path = execution.join(format!("Input-P{}-0", self.config.node));
        if input_path.exists() {
            return Err("a plaintext party input survived an interrupted execution".into());
        }
        // The coordinator's party input carries placeholders where the standing
        // Maker reserves belong.  Only this node's encrypted resident shares
        // enter MP-SPDZ there; the admission digest still binds the
        // coordinator-visible part of the input.
        let maker_state = self.loaded_maker_state()?;
        let mut tokens = request
            .input
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        request.input.clear();
        let input_count =
            u64::try_from(tokens.len()).map_err(|_| "party input count exceeds u64".to_string())?;
        splice_standing_maker_shares(&mut tokens, &maker_state, self.padded_makers)?;
        let mut input = tokens.join(" ").into_bytes();
        for token in tokens.iter_mut() {
            token.clear();
        }
        input.push(b'\n');
        atomic_private_write(&input_path, &input)?;
        input.fill(0);
        let started = Instant::now();
        let mut child = Command::new(&self.party_binary)
            .current_dir(&execution)
            .arg(self.config.node.to_string())
            .arg(&self.program)
            .args(["-N", &self.config.n_parties.to_string()])
            .args(["-T", &self.config.threshold.to_string()])
            .arg("-ip")
            .arg(&self.host_file)
            .arg("-IF")
            .arg(&prefix)
            .arg("-P")
            .arg(ED25519_ORDER)
            .args(["-OF", "."])
            .env(
                if cfg!(target_os = "macos") {
                    "DYLD_LIBRARY_PATH"
                } else {
                    "LD_LIBRARY_PATH"
                },
                &self.config.mp_spdz_root,
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("stock MP-SPDZ party did not start: {error}"))?;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                break status;
            }
            if started.elapsed() >= self.config.timeout {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&input_path);
                return Err("stock MP-SPDZ party exceeded its execution timeout".into());
            }
            thread::sleep(Duration::from_millis(10));
        };
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&input_path);
        if !status.success() || !output.status.success() {
            // Keep the engine's diagnostic local to the MPC operator.  The
            // coordinator receives only digests, while the node retains a
            // private, bounded stderr file that makes production incidents
            // diagnosable without logging the participant input or sending
            // the engine output over HTTP.
            let diagnostic_path = execution.join(format!("engine-error-P{}.log", self.config.node));
            let diagnostic = if output.stderr.len() > 16 * 1_024 {
                &output.stderr[..16 * 1_024]
            } else {
                &output.stderr
            };
            atomic_private_write(&diagnostic_path, diagnostic)?;
            return Err(format!(
                "stock MP-SPDZ party failed (exit={:?}, stdout={}, stderr={})",
                output.status.code(),
                hex::encode(Sha256::digest(&output.stdout)),
                hex::encode(Sha256::digest(&output.stderr))
            ));
        }
        let _ = fs::remove_file(execution.join(format!("engine-error-P{}.log", self.config.node)));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let masked_key = opened_key(&stdout)
            .ok_or_else(|| "stock MP-SPDZ party emitted no QOMM_MASKED_KEY".to_string())?;
        let masked_fill = opened_fill(&stdout)
            .ok_or_else(|| "stock MP-SPDZ party emitted no QOMM_MASKED_FILL".to_string())?;
        let persistence = execution
            .join("Persistence")
            .join(format!("Transactions-P{}.data", self.config.node));
        let metadata = persistence.metadata().map_err(|error| {
            format!("stock MP-SPDZ did not persist its product proof handoff: {error}")
        })?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("stock MP-SPDZ wrote an empty product proof handoff".into());
        }
        fs::set_permissions(&persistence, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
        let stdout_sha256: [u8; 32] = Sha256::digest(&output.stdout).into();
        let stderr_sha256: [u8; 32] = Sha256::digest(&output.stderr).into();
        let persistence_sha256: [u8; 32] =
            Sha256::digest(fs::read(&persistence).map_err(|error| error.to_string())?).into();
        if let Some(receipt) = canonical_receipt.as_ref() {
            if receipt.masked_key != masked_key || receipt.masked_fill != masked_fill {
                return Err(
                    "partial-round recovery disagreed with the cached public MPC result".into(),
                );
            }
        }
        let unsigned_result = NodePublicResultAttestation {
            node: u16::try_from(self.config.node)
                .map_err(|_| "MPC node index exceeds public-result bounds")?,
            slot: request.admission.slot,
            sequence: request.admission.sequence,
            batch_digest: decode_hex32(&request.input_sha256, "MPC input digest")?,
            source_digest: decode_hex32(&self.source_sha256, "MPC source digest")?,
            round_digest: decode_hex32(&request.round_id, "MPC round digest")?,
            masked_key,
            masked_fill,
            stdout_digest: stdout_sha256,
            persistence_digest: persistence_sha256,
            signature: Signature::from_bytes(&[0; 64]),
        };
        let (public_result, result_identity_public) = self
            .proof_party
            .lock()
            .map_err(|_| "proof-party state lock is poisoned".to_string())?
            .sign_public_result_attestation(unsigned_result)?;
        let receipt = ExecuteReceipt {
            version: PROTOCOL_VERSION,
            node: self.config.node,
            round_id: request.round_id,
            source_sha256: self.source_sha256.clone(),
            public_market_time: request.public_market_time,
            input_sha256: request.input_sha256,
            execution_generation: request.execution_generation,
            masked_key,
            masked_fill,
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            stdout_sha256: hex::encode(stdout_sha256),
            stderr_sha256: hex::encode(stderr_sha256),
            persistence_sha256: hex::encode(persistence_sha256),
            result_identity_public: hex::encode(result_identity_public),
            public_result_attestation: hex::encode(encode_node_public_result_attestation(
                &public_result,
            )?),
            admission: admission_attestation,
            maker_state_generation: maker_state.generation,
            input_count,
        };
        let encoded_receipt =
            serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?;
        atomic_private_write(&receipt_path, &encoded_receipt)?;
        if !canonical_receipt_path.is_file() {
            let canonical = canonical_receipt.as_ref().unwrap_or(&receipt);
            atomic_private_write(
                &canonical_receipt_path,
                &serde_json::to_vec_pretty(canonical).map_err(|error| error.to_string())?,
            )?;
        }
        Ok(receipt)
    }
}

/// Start one independently addressable MPC service.  Only the API port is an
/// HTTP listener; the stock MP-SPDZ process opens its own party port for each
/// round using the pinned host file.
pub fn serve_mpc_node(config: MpcNodeConfig) -> Result<(), String> {
    let prepared = Arc::new(PreparedNode::prepare(config)?);
    let execution = Arc::new(Mutex::new(()));
    let listener = TcpListener::bind((
        prepared.config.listen_host.as_str(),
        prepared.config.api_port,
    ))
    .map_err(|error| error.to_string())?;
    println!(
        "QOMM MPC node {} listening at {} with circuit {}",
        prepared.config.node,
        listener.local_addr().map_err(|error| error.to_string())?,
        prepared.source_sha256
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let prepared = Arc::clone(&prepared);
                let execution = Arc::clone(&execution);
                thread::spawn(move || {
                    let _ = handle_node(stream, &prepared, &execution);
                });
            }
            Err(error) => eprintln!("MPC node accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_node(
    mut stream: TcpStream,
    prepared: &PreparedNode,
    execution: &Mutex<()>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(30))))
        .map_err(|error| error.to_string())?;
    let (method, path, mut body) = read_http_request(&mut stream)?;
    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/health") => http_json(
            "200 OK",
            &json!({
                "ok": true,
                "service": "qomm-mpc-node",
                "node": prepared.config.node,
                "n_parties": prepared.config.n_parties,
                "threshold": prepared.config.threshold,
                "source_sha256": prepared.source_sha256,
                "proof_instance_id": hex::encode(
                    prepared
                        .proof_party
                        .lock()
                        .map_err(|_| "proof-party state lock is poisoned".to_string())?
                        .instance_id()
                ),
            }),
        ),
        ("POST", "/v1/admit") => {
            let parsed = serde_json::from_slice::<AdmitRequest>(&body)
                .map_err(|_| "MPC admission body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let _guard = execution
                        .lock()
                        .map_err(|_| "MPC execution lock poisoned")?;
                    match prepared.admit(request) {
                        Ok(receipt) => http_json(
                            "200 OK",
                            &serde_json::to_value(receipt).map_err(|error| error.to_string())?,
                        ),
                        Err(error) => http_json("409 Conflict", &json!({"error": error})),
                    }
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        ("POST", "/v1/execute") => {
            let parsed = serde_json::from_slice::<ExecuteRequest>(&body)
                .map_err(|_| "MPC node request body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let _guard = execution
                        .lock()
                        .map_err(|_| "MPC execution lock poisoned")?;
                    match prepared.execute(request) {
                        Ok(receipt) => http_json(
                            "200 OK",
                            &serde_json::to_value(receipt).map_err(|error| error.to_string())?,
                        ),
                        Err(error) => http_json("409 Conflict", &json!({"error": error})),
                    }
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        ("GET", "/v1/maker-state") => match prepared.maker_state_view() {
            Ok(view) => http_json(
                "200 OK",
                &serde_json::to_value(view).map_err(|error| error.to_string())?,
            ),
            Err(error) => http_json("409 Conflict", &json!({"error": error})),
        },
        ("GET", "/v1/rounds") => match prepared.round_receipts() {
            Ok(receipts) => http_json(
                "200 OK",
                &json!({"node": prepared.config.node, "receipts": receipts}),
            ),
            Err(error) => http_json("409 Conflict", &json!({"error": error})),
        },
        ("POST", "/v1/maker-state/seed") => {
            let parsed = serde_json::from_slice::<MakerStateSeedRequest>(&body)
                .map_err(|_| "Maker-state seed body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let _guard = execution
                        .lock()
                        .map_err(|_| "MPC execution lock poisoned")?;
                    match prepared.seed_maker_state(request) {
                        Ok(view) => http_json(
                            "200 OK",
                            &serde_json::to_value(view).map_err(|error| error.to_string())?,
                        ),
                        Err(error) => http_json("409 Conflict", &json!({"error": error})),
                    }
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        ("POST", "/v1/maker-state/commit") => {
            let parsed = serde_json::from_slice::<MakerStateCommitRequest>(&body)
                .map_err(|_| "Maker-state commit body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let _guard = execution
                        .lock()
                        .map_err(|_| "MPC execution lock poisoned")?;
                    match prepared.commit_maker_state(request) {
                        Ok(response) => http_json(
                            "200 OK",
                            &serde_json::to_value(response).map_err(|error| error.to_string())?,
                        ),
                        Err(error) => http_json("409 Conflict", &json!({"error": error})),
                    }
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        ("POST", "/v1/maker-state/rebind") => {
            let parsed = serde_json::from_slice::<MakerStateRebindRequest>(&body)
                .map_err(|_| "Maker-state rebind body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let _guard = execution
                        .lock()
                        .map_err(|_| "MPC execution lock poisoned")?;
                    match prepared.rebind_maker_state(request) {
                        Ok(view) => http_json(
                            "200 OK",
                            &serde_json::to_value(view).map_err(|error| error.to_string())?,
                        ),
                        Err(error) => http_json("409 Conflict", &json!({"error": error})),
                    }
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        ("POST", "/v1/proof") => {
            let parsed = serde_json::from_slice::<ProofRequest>(&body)
                .map_err(|_| "proof request body is not canonical JSON".to_string());
            body.fill(0);
            match parsed {
                Ok(request) => {
                    let response = prepared
                        .proof_party
                        .lock()
                        .map_err(|_| "proof-party state lock is poisoned".to_string())?
                        .handle(request);
                    http_json(
                        "200 OK",
                        &serde_json::to_value(response).map_err(|error| error.to_string())?,
                    )
                }
                Err(error) => http_json("400 Bad Request", &json!({"error": error})),
            }
        }
        _ => http_json(
            "404 Not Found",
            &json!({"error":"no such MPC node operation"}),
        ),
    };
    stream
        .write_all(&response)
        .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn read_http_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    let mut raw = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("HTTP request ended before its headers".into());
        }
        raw.extend_from_slice(&chunk[..count]);
        if raw.len() > MAX_HTTP_BYTES {
            return Err("HTTP request exceeded its fixed bound".into());
        }
        if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head = std::str::from_utf8(&raw[..header_end]).map_err(|_| "HTTP headers are not UTF-8")?;
    let mut lines = head.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(|| "HTTP request line is absent".to_string())?;
    let mut fields = first.split_whitespace();
    let method = fields.next().unwrap_or_default().to_string();
    let path = fields.next().unwrap_or_default().to_string();
    if fields.next().is_none() || path.contains('?') {
        return Err("HTTP request line is malformed".into());
    }
    let mut length = 0_usize;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "HTTP header is malformed".to_string())?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("chunked HTTP requests are not accepted".into());
        }
        if name.eq_ignore_ascii_case("content-length") {
            length = value
                .trim()
                .parse()
                .map_err(|_| "Content-Length is invalid".to_string())?;
        }
    }
    if length > MAX_HTTP_BYTES || raw.len().saturating_sub(header_end) > length {
        return Err("HTTP body length is outside its fixed bound".into());
    }
    while raw.len().saturating_sub(header_end) < length {
        let remaining = length - raw.len().saturating_sub(header_end);
        let mut chunk = vec![0_u8; remaining.min(4096)];
        stream
            .read_exact(&mut chunk)
            .map_err(|error| error.to_string())?;
        raw.extend_from_slice(&chunk);
    }
    Ok((method, path, raw[header_end..].to_vec()))
}

fn http_json(status: &str, value: &Value) -> Vec<u8> {
    let body =
        serde_json::to_vec(value).unwrap_or_else(|_| b"{\"error\":\"serialization\"}".to_vec());
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    response
}

fn atomic_private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("qomm"),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn opened_key(log: &str) -> Option<i128> {
    log.lines()
        .find_map(|line| line.trim().strip_prefix("QOMM_MASKED_KEY="))?
        .trim()
        .parse()
        .ok()
}

fn opened_fill(log: &str) -> Option<i128> {
    log.lines()
        .find_map(|line| line.trim().strip_prefix("QOMM_MASKED_FILL="))?
        .trim()
        .parse()
        .ok()
}

fn signed_scalar(value: i64) -> Scalar {
    if value < 0 {
        -Scalar::from(value.unsigned_abs())
    } else {
        Scalar::from(value as u64)
    }
}

fn participant_handle_scalar(role: &[u8], participant_id: &[u8; 32]) -> u64 {
    let digest = Sha256::new()
        .chain_update(b"QOMM:DEMO:PARTICIPANT-HANDLE:v1")
        .chain_update(role)
        .chain_update(participant_id)
        .finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix")).max(1)
}

/// Where the gateway keeps the committee's public FROST package between
/// boots: beside its round persistence, one file per DKG session.
fn frost_public_recall_path(session: &[u8; 32]) -> PathBuf {
    PathBuf::from(format!("frost-public-{}.b64", &hex::encode(session)[..16]))
}

fn record_frost_public(
    session: &[u8; 32],
    public: &zkpi::frost::keys::PublicKeyPackage,
) -> Result<(), String> {
    let bytes = public
        .serialize()
        .map_err(|_| "FROST public package serialization failed".to_string())?;
    let path = frost_public_recall_path(session);
    let temporary = path.with_extension("b64.tmp");
    fs::write(&temporary, BASE64.encode(bytes)).map_err(|error| error.to_string())?;
    fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

fn recall_frost_public(
    session: &[u8; 32],
) -> Result<Option<zkpi::frost::keys::PublicKeyPackage>, String> {
    let path = frost_public_recall_path(session);
    let encoded = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let bytes = BASE64
        .decode(encoded.trim())
        .map_err(|_| "recorded FROST public package is not base64".to_string())?;
    zkpi::frost::keys::PublicKeyPackage::deserialize(&bytes)
        .map(Some)
        .map_err(|_| "recorded FROST public package cannot be decoded".into())
}

fn decode_hex32(value: &str, name: &str) -> Result<[u8; 32], String> {
    hex::decode(value)
        .map_err(|_| format!("{name} is not hexadecimal"))?
        .try_into()
        .map_err(|_| format!("{name} is not 32 bytes"))
}

/// The remainder note one accepted execution produced for a standing pool,
/// recomputed from the seven nodes' public receipt digests exactly as
/// `locate_accepted_execution` does.  `receipts` holds one `/v1/rounds`
/// receipt per node for the same round and execution generation; the
/// result is the note id canonical DeFMI shows as the pool's current note
/// if that execution's allocation was accepted.  The live-acceptance judge
/// uses it to tie a fill to the pool it allocated from through the accepted
/// execution and the canonical note, instead of trusting pool ids to stay
/// bound on the nodes between two snapshots.
pub fn remainder_note_id_of_execution(
    source_sha256: &str,
    receipts: &[Value],
    asset_id: &str,
    pool_id: &str,
    remainder_commitment: &str,
) -> Result<String, String> {
    let source_digest = decode_hex32(source_sha256, "MPC source digest")?;
    let views = receipts
        .iter()
        .map(|receipt| {
            serde_json::from_value::<NodeRoundReceiptView>(receipt.clone())
                .map_err(|error| format!("node receipt is malformed: {error}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Some(first) = views.first() else {
        return Err("no node receipts".into());
    };
    if first.sequence == 0 {
        return Err("admission sequence zero has no execution lane".into());
    }
    let mut nodes = Vec::with_capacity(views.len());
    for receipt in &views {
        if receipt.round_id != first.round_id
            || receipt.execution_generation != first.execution_generation
        {
            return Err("node receipts name different executions".into());
        }
        nodes.push(ExecutionNodeDigest {
            batch_digest: decode_hex32(&receipt.input_sha256, "MPC input digest")?,
            source_digest,
            stdout_digest: decode_hex32(&receipt.stdout_sha256, "MPC stdout digest")?,
            stderr_digest: decode_hex32(&receipt.stderr_sha256, "MPC stderr digest")?,
            persistence_digest: decode_hex32(
                &receipt.persistence_sha256,
                "MPC persistence digest",
            )?,
        });
    }
    let plan = ApplicationExecutionPlan::new(
        &qomm_manifest_v1(),
        ExecutionShape {
            lane: execution_lane_for_admission(first.sequence)?,
            slot: first.slot,
            generation: u64::from(first.execution_generation),
            frame_count: 1,
            input_count: first.input_count,
            order_digest: decode_hex32(&first.order_digest, "admission order")?,
            nodes,
        },
    )
    .map_err(|error| error.to_string())?;
    let note = allocation_note(
        decode_hex32(asset_id, "pool asset")?,
        decode_hex32(remainder_commitment, "remainder commitment")?,
        decode_hex32(pool_id, "pool id")?,
        plan.job_id(),
        b"remainder",
    )?;
    Ok(hex::encode(note.note_id))
}

fn decode_admission_receipt(
    value: &AdmissionAttestationWire,
) -> Result<(NodeAdmissionAttestation, VerifyingKey), String> {
    let signature = hex::decode(&value.signature)
        .map_err(|_| "admission signature is not hexadecimal".to_string())?;
    Signature::try_from(signature.as_slice()).map_err(|error| error.to_string())?;
    let identity =
        VerifyingKey::from_bytes(&decode_hex32(&value.identity_public, "admission identity")?)
            .map_err(|_| "admission identity is not a valid hybrid key fingerprint".to_string())?;
    let attestation = NodeAdmissionAttestation {
        node: value.node,
        slot: value.slot,
        sequence: value.sequence,
        principal_digest: decode_hex32(&value.principal_digest, "admission principal")?,
        ticket_id: decode_hex32(&value.ticket_id, "admission ticket")?,
        claim_digest: decode_hex32(&value.claim_digest, "admission claim")?,
        batch_digest: decode_hex32(&value.batch_digest, "admission batch")?,
        order_digest: decode_hex32(&value.order_digest, "admission order")?,
        signature: Signature::from_bytes(&signature),
    };
    if !attestation.verify(&identity) {
        return Err("MPC node admission signature is invalid".into());
    }
    Ok((attestation, identity))
}

fn unpack_key(key: i128, padded: usize) -> (i128, usize) {
    let width = padded as i128;
    let maker = key.rem_euclid(width) as usize;
    ((key - maker as i128) / width, maker)
}

fn json_u64(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.to_string().parse().ok())
        .ok_or_else(|| "generated mask is outside the unsigned 64-bit range".to_string())
}

fn low_72_hex(decimal: &str) -> Result<String, String> {
    const MASK: u128 = (1_u128 << 72) - 1;
    let (negative, digits) = decimal
        .strip_prefix('-')
        .map_or((false, decimal), |digits| (true, digits));
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err("MP-SPDZ input is not a decimal integer".into());
    }
    let mut value = 0_u128;
    for digit in digits.bytes() {
        value = (value * 10 + u128::from(digit - b'0')) & MASK;
    }
    if negative {
        value = value.wrapping_neg() & MASK;
    }
    Ok(format!("{value:018x}"))
}

fn execution_input_digest(public_market_time: i64, private_input: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"QOMM:MPC:EXECUTION-INPUT:v1")
        .chain_update(public_market_time.to_be_bytes())
        .chain_update((private_input.len() as u64).to_be_bytes())
        .chain_update(private_input)
        .finalize()
        .into()
}

fn execution_directory(round: &Path, generation: u32) -> Result<PathBuf, String> {
    match generation {
        1 => Ok(round.to_path_buf()),
        2..=64 => Ok(round
            .join("recovery")
            .join(format!("attempt-{generation:08}"))),
        _ => Err("MPC execution generation is outside its bounded retry range".into()),
    }
}

fn execution_receipt_path(round: &Path, generation: u32) -> Result<PathBuf, String> {
    Ok(execution_directory(round, generation)?.join("receipt.json"))
}

fn execution_persistence_path(
    round_id: &str,
    generation: u32,
    node: usize,
) -> Result<String, String> {
    if round_id.len() != 64
        || !round_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || node >= COMMITTEE_NODES
    {
        return Err("MPC persistence path is outside its round or node bound".into());
    }
    execution_directory(&Path::new("rounds").join(round_id), generation)?
        .join("Persistence")
        .join(format!("Transactions-P{node}.data"))
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "MPC persistence path is not UTF-8".to_string())
}

fn ensure_round_programs(
    round: &Path,
    mp_spdz_root: &Path,
    program: &str,
    public_market_time: i64,
) -> Result<(), String> {
    if public_market_time < 0
        || program.is_empty()
        || !program
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("MPC round public input or program name is invalid".into());
    }
    let programs = round.join("Programs");
    let pinned_programs = mp_spdz_root.join("Programs");
    match fs::symlink_metadata(&programs) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if fs::read_link(&programs).map_err(|error| error.to_string())? != pinned_programs {
                return Err("MPC round Programs link points outside the pinned tree".into());
            }
            fs::remove_file(&programs).map_err(|error| error.to_string())?;
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err("MPC round Programs path is not a directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    fs::create_dir_all(&programs).map_err(|error| error.to_string())?;
    fs::set_permissions(&programs, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    for name in ["Bytecode", "Schedules"] {
        ensure_round_directory_link(&programs.join(name), &pinned_programs.join(name))?;
    }
    let public_inputs = programs.join("Public-Input");
    match fs::symlink_metadata(&public_inputs) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("MPC round Public-Input path is not a private directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&public_inputs).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    fs::set_permissions(&public_inputs, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let public_input = public_inputs.join(program);
    let expected = format!("{public_market_time}\n");
    match fs::read(&public_input) {
        Ok(existing) if existing == expected.as_bytes() => Ok(()),
        Ok(_) => Err("MPC round public market time changed after admission".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            atomic_private_write(&public_input, expected.as_bytes())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_round_directory_link(link: &Path, target: &Path) -> Result<(), String> {
    if !target.is_dir() {
        return Err(format!(
            "required MP-SPDZ directory is unavailable: {}",
            target.display()
        ));
    }
    match fs::symlink_metadata(link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let actual = fs::read_link(link).map_err(|error| error.to_string())?;
            if actual == target {
                Ok(())
            } else {
                Err(format!(
                    "MPC round link {} points outside the pinned MP-SPDZ tree",
                    link.display()
                ))
            }
        }
        Ok(metadata) if metadata.is_dir() => {
            let mut entries = fs::read_dir(link).map_err(|error| error.to_string())?;
            if entries.next().is_some() {
                return Err(format!(
                    "MPC round directory {} contains unexpected retained state",
                    link.display()
                ));
            }
            fs::remove_dir(link).map_err(|error| error.to_string())?;
            symlink(target, link).map_err(|error| error.to_string())
        }
        Ok(_) => Err(format!(
            "MPC round path {} is not a directory link",
            link.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(target, link).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    use tempfile::tempdir;

    fn admission_wire(node: u16, claim: &str) -> AdmissionAttestationWire {
        AdmissionAttestationWire {
            node,
            slot: 7,
            sequence: 8,
            principal_digest: "11".repeat(32),
            ticket_id: "22".repeat(32),
            claim_digest: claim.repeat(32),
            batch_digest: "44".repeat(32),
            order_digest: "55".repeat(32),
            identity_public: "66".repeat(32),
            signature: "77".repeat(64),
        }
    }

    #[test]
    fn prepared_node_admission_retry_restores_exact_hybrid_receipt() {
        let directory = tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        // Exercise the real admission path and encrypted ProofParty state without
        // launching an MPC program; admission precedes execution by construction.
        let open = || {
            let root = directory.path().to_path_buf();
            let proof_party = ProofParty::new(ProofPartyConfig {
                recipient_opening_keys: Vec::new(),
                node: 0,
                allowed_root: root.clone(),
                state_file: root.join("proof-state.qps"),
                state_passphrase: vec![31; 32],
                n_mm: 2,
                n_parties: 7,
                threshold: 4,
                amount_bits: 16,
                price_bits: 16,
                remainder_bits: 16,
                complete_quote_proof: true,
                quote_eligibility_bits: 16,
                quote_span_bits: 16,
                trusted_defmi_receipt_public: None,
                allow_health_signing: false,
            })
            .unwrap();
            PreparedNode {
                config: MpcNodeConfig {
                    node: 0,
                    n_parties: 7,
                    threshold: 4,
                    n_makers: 2,
                    references: vec![100],
                    bit_length: 16,
                    input_check: true,
                    listen_host: "127.0.0.1".into(),
                    api_port: 0,
                    mp_spdz_root: root.join("unused-mpc"),
                    state_root: root.clone(),
                    party_hosts: Vec::new(),
                    timeout: Duration::from_secs(5),
                },
                source_sha256: "11".repeat(32),
                program: String::new(),
                party_binary: root.join("unused-party"),
                host_file: root.join("unused-hosts"),
                proof_party: Mutex::new(proof_party),
                maker_state: EncryptedMpcStateStore::new(root.join("maker.qms"), &[32; 32])
                    .unwrap(),
                padded_makers: 2,
            }
        };
        let node = open();
        let request = AdmitRequest {
            version: PROTOCOL_VERSION,
            node: 0,
            round_id: "22".repeat(32),
            source_sha256: node.source_sha256.clone(),
            public_market_time: 100,
            input_sha256: "33".repeat(32),
            admission: ExecuteAdmission {
                slot: 7,
                sequence: 1,
                principal: "test-participant".into(),
                ticket_id: hex::encode(principal_ticket_id(7, "test-participant").unwrap()),
                claim_digest: "44".repeat(32),
                order_digest: "55".repeat(32),
            },
        };
        let first = node.admit(request.clone()).unwrap();
        let path = node
            .round_directory(&request.round_id)
            .join("admission.json");
        let persisted = fs::read(&path).unwrap();
        assert_eq!(node.admit(request.clone()).unwrap(), first);
        drop(node);
        let node = open();
        assert_eq!(node.admit(request.clone()).unwrap(), first);
        assert_eq!(fs::read(&path).unwrap(), persisted);
        let mut changed = request.clone();
        changed.admission.claim_digest = "66".repeat(32);
        assert!(node.admit(changed).is_err());
        assert_eq!(fs::read(&path).unwrap(), persisted);
        // A valid receipt under another node key is not an enrolled receipt.
        let (mut attestation, _) = decode_admission_receipt(&first).unwrap();
        let other = qomm_transport::application_crypto::SigningKey::generate(&mut OsRng);
        attestation = attestation.sign(&other).unwrap();
        let mut forged = first.clone();
        forged.identity_public = hex::encode(other.verifying_key().to_bytes());
        forged.signature = hex::encode(attestation.signature.to_bytes());
        atomic_private_write(&path, &serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(node.admit(request.clone()).is_err());
        for offset in [14 + 1984, 14 + 1984 + 64] {
            let mut corrupted = first.clone();
            let mut bytes = hex::decode(&corrupted.signature).unwrap();
            bytes[offset] ^= 1;
            corrupted.signature = hex::encode(bytes);
            let encoded = serde_json::to_vec(&corrupted).unwrap();
            atomic_private_write(&path, &encoded).unwrap();
            assert!(node.admit(request.clone()).is_err());
            assert_eq!(fs::read(&path).unwrap(), encoded);
        }
    }

    #[test]
    fn endpoint_and_execution_identity_are_bounded() {
        assert_eq!(
            Endpoint::parse("http://mpc-0:9100/").unwrap().authority,
            "mpc-0:9100"
        );
        assert!(Endpoint::parse("https://mpc-0:9100").is_err());
        assert!(Endpoint::parse("http://user@mpc-0:9100").is_err());
        assert!(Endpoint::parse("http://mpc-0:9100/path").is_err());
        assert_eq!(unpack_key(-17, 8), (-3, 7));
        assert_eq!(low_72_hex("-1").unwrap(), "ffffffffffffffffff");
    }

    #[test]
    fn fresh_market_time_uses_wall_clock_and_replay_keeps_the_signed_time() {
        assert_eq!(market_time_for_execution(1_000, None).unwrap(), 1_000);
        assert_eq!(market_time_for_execution(1_000, Some(997)).unwrap(), 997);
        assert!(market_time_for_execution(1_000, Some(-1)).is_err());
        assert!(market_time_for_execution(1_000, Some(1_001)).is_err());
    }

    #[test]
    fn execution_lane_is_the_zero_based_admission_sequence() {
        assert_eq!(execution_lane_for_admission(1).unwrap(), 0);
        assert_eq!(execution_lane_for_admission(8).unwrap(), 7);
        assert_eq!(execution_lane_for_admission(4096).unwrap(), 4095);
        assert!(execution_lane_for_admission(0).is_err());
        assert!(execution_lane_for_admission(4097).is_err());
    }

    #[test]
    fn partial_round_recovery_uses_one_isolated_directory_per_attempt() {
        let round = Path::new("/state/rounds/abc");
        assert_eq!(execution_directory(round, 1).unwrap(), round);
        assert_eq!(
            execution_directory(round, 12).unwrap(),
            round.join("recovery/attempt-00000012")
        );
        assert_eq!(
            execution_receipt_path(round, 1).unwrap(),
            round.join("receipt.json")
        );
        assert_eq!(
            execution_receipt_path(round, 12).unwrap(),
            round.join("recovery/attempt-00000012/receipt.json")
        );
        let round_id = "ab".repeat(32);
        assert_eq!(
            execution_persistence_path(&round_id, 1, 0).unwrap(),
            format!("rounds/{round_id}/Persistence/Transactions-P0.data")
        );
        assert_eq!(
            execution_persistence_path(&round_id, 12, 6).unwrap(),
            format!("rounds/{round_id}/recovery/attempt-00000012/Persistence/Transactions-P6.data")
        );
        assert!(execution_directory(round, 0).is_err());
        assert!(execution_directory(round, 65).is_err());
        assert!(execution_persistence_path(&round_id, 12, 7).is_err());
    }

    #[test]
    fn standing_rails_follow_the_circuit_order_not_the_mandate_wire_numbers() {
        assert_eq!(standing_rail(Direction::TakerBuys), 0);
        assert_eq!(standing_rail(Direction::TakerSells), 1);
        assert_eq!(
            MpcSecretState::standing_share_offset(2, standing_rail(Direction::TakerSells)).unwrap(),
            12
        );
        let shares = deal_additive_shares(&Scalar::from(4_900_u64), 7);
        assert_eq!(shares.len(), 7);
        let total = shares
            .iter()
            .map(|share| qomm_transport::resident_mpc::decimal_to_scalar(share).unwrap())
            .sum::<Scalar>();
        assert_eq!(total, Scalar::from(4_900_u64));
    }

    #[test]
    fn legacy_execution_receipt_defaults_to_the_initial_generation() {
        let receipt = ExecuteReceipt {
            version: PROTOCOL_VERSION,
            node: 0,
            round_id: "11".repeat(32),
            source_sha256: "22".repeat(32),
            public_market_time: 1,
            input_sha256: "33".repeat(32),
            execution_generation: 7,
            masked_key: 4,
            masked_fill: 5,
            elapsed_ms: 6.0,
            stdout_sha256: "44".repeat(32),
            stderr_sha256: "55".repeat(32),
            persistence_sha256: "66".repeat(32),
            result_identity_public: "77".repeat(32),
            public_result_attestation: "88".repeat(32),
            admission: admission_wire(0, "9"),
            maker_state_generation: 3,
            input_count: 42,
        };
        let mut value = serde_json::to_value(receipt).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("execution_generation");
        let decoded: ExecuteReceipt = serde_json::from_value(value).unwrap();
        assert_eq!(decoded.execution_generation, 1);
    }

    #[test]
    fn maker_blindings_are_restart_stable_and_domain_separated() {
        let approval = EntityApproval {
            participant_id: [1; 32],
            key_purpose: KeyPurpose::Quote,
            key_epoch: 1,
            statement: [2; 32],
            signature: vec![3; 64],
        };
        let first = participant_derived_u64(&approval, b"inventory-reserve");
        assert_eq!(
            first,
            participant_derived_u64(&approval, b"inventory-reserve")
        );
        assert_ne!(first, participant_derived_u64(&approval, b"cash-reserve"));
    }

    #[test]
    fn distributed_shape_matches_the_node_circuit_digest() {
        let endpoints = (0..7)
            .map(|node| format!("http://mpc-{node}:9100"))
            .collect::<Vec<_>>();
        let engine = DistributedMpcEngine::new(
            &endpoints,
            2,
            8,
            &[15_750, 10_850, 6_420_000],
            63,
            true,
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(engine.source_sha256.len(), 64);
    }

    #[test]
    fn sixty_three_bit_circuit_accepts_full_width_response_masks() {
        let endpoints = (0..7)
            .map(|node| format!("http://mpc-{node}:9100"))
            .collect::<Vec<_>>();
        let engine = DistributedMpcEngine::new(
            &endpoints,
            2,
            4,
            &[15_750, 10_850, 6_420_000],
            63,
            true,
            Duration::from_secs(30),
        )
        .unwrap();
        let request = Request {
            is_real: 0,
            ..Request::default()
        };
        let settlement = MpcSettlementInputs {
            user_limit: 15_907,
            taker_securities_reserve: 0,
            taker_cash_reserve: 0,
            maker_securities_reserves: vec![500; 4],
            maker_cash_reserves: vec![10_000_000; 4],
        };

        let admission = engine
            .prepare_admission(&request, &settlement, 1, None)
            .unwrap();
        assert_ne!(admission.response_mask, 0);
        assert_ne!(admission.fill_mask, 0);
    }

    #[test]
    fn cover_admission_uses_the_consensus_cursor_after_a_gateway_restart() {
        let endpoints = (0..7)
            .map(|node| format!("http://mpc-{node}:9100"))
            .collect::<Vec<_>>();
        let mut engine = DistributedMpcEngine::new(
            &endpoints,
            2,
            4,
            &[15_750, 10_850, 6_420_000],
            63,
            true,
            Duration::from_secs(30),
        )
        .unwrap();
        // A restarted gateway may recover a larger process-local outbox
        // cursor than the DeFMI venue cursor.  Cover traffic must not use it.
        engine.served = 41;
        let request = Request {
            is_real: 0,
            ..Request::default()
        };
        let settlement = MpcSettlementInputs {
            user_limit: 15_907,
            taker_securities_reserve: 0,
            taker_cash_reserve: 0,
            maker_securities_reserves: vec![500; 4],
            maker_cash_reserves: vec![10_000_000; 4],
        };

        let admission = engine
            .prepare_admission(&request, &settlement, 1, Some(7))
            .unwrap();
        assert_eq!(admission.envelope.slot, 41);
        assert_eq!(admission.envelope.sequence, 7);
    }

    #[test]
    fn execution_requires_the_exact_durable_pre_admission() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("admission.json");
        let admitted = admission_wire(0, "33");

        let before = read_certified_admission(&path, &admitted).unwrap_err();
        assert!(before.contains("before this node durably admitted"));

        atomic_private_write(&path, &serde_json::to_vec(&admitted).unwrap()).unwrap();
        assert_eq!(
            read_certified_admission(&path, &admitted).unwrap(),
            admitted
        );

        let substituted = admission_wire(0, "88");
        let mismatch = read_certified_admission(&path, &substituted).unwrap_err();
        assert!(mismatch.contains("another admission receipt"));
    }

    #[test]
    fn round_directory_link_recovers_only_an_empty_legacy_directory() {
        let directory = tempdir().unwrap();
        let target = directory.path().join("official-player-data");
        fs::create_dir(&target).unwrap();
        let link = directory.path().join("Player-Data");

        fs::create_dir(&link).unwrap();
        ensure_round_directory_link(&link, &target).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), target);

        fs::remove_file(&link).unwrap();
        fs::create_dir(&link).unwrap();
        fs::write(link.join("retained-secret"), b"do-not-delete").unwrap();
        let error = ensure_round_directory_link(&link, &target).unwrap_err();
        assert!(error.contains("unexpected retained state"));
        assert_eq!(
            fs::read(link.join("retained-secret")).unwrap(),
            b"do-not-delete"
        );
    }

    #[test]
    fn round_programs_bind_a_runtime_public_market_time() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("mp-spdz");
        fs::create_dir_all(root.join("Programs/Bytecode")).unwrap();
        fs::create_dir_all(root.join("Programs/Schedules")).unwrap();
        let round = directory.path().join("round");
        fs::create_dir(&round).unwrap();

        ensure_round_programs(&round, &root, "qomm_test", 37).unwrap();
        assert_eq!(
            fs::read(round.join("Programs/Public-Input/qomm_test")).unwrap(),
            b"37\n"
        );
        assert_eq!(
            fs::read_link(round.join("Programs/Bytecode")).unwrap(),
            root.join("Programs/Bytecode")
        );
        ensure_round_programs(&round, &root, "qomm_test", 37).unwrap();
        assert!(ensure_round_programs(&round, &root, "qomm_test", 38).is_err());

        assert_ne!(
            execution_input_digest(37, b"same private share"),
            execution_input_digest(38, b"same private share")
        );
    }

    #[test]
    fn maker_mandates_and_quote_proof_share_one_policy_registry() {
        let endpoints = (0..7)
            .map(|node| format!("http://mpc-{node}:9100"))
            .collect::<Vec<_>>();
        let engine = DistributedMpcEngine::new(
            &endpoints,
            2,
            4,
            &[15_750, 10_850, 6_420_000],
            63,
            true,
            Duration::from_secs(30),
        )
        .unwrap();
        let policies = (0..4)
            .map(|maker| Policy {
                asset: (maker % 3) as i64,
                ask_level: maker as i64 * 5,
                inv: maker as i64 * -3,
                ..Policy::default()
            })
            .collect::<Vec<_>>();
        let openings = policies
            .iter()
            .enumerate()
            .map(|(maker, policy)| RegisteredPolicyOpening {
                maker_asset: policy.asset as u32,
                values: [
                    policy.ask_level,
                    policy.spread,
                    policy.slope,
                    policy.invcoef,
                    policy.inv,
                    policy.maxqty,
                    policy.expiry,
                    policy.active,
                    policy.use_ref,
                ],
                blindings: engine.policy_blindings(maker).unwrap(),
            })
            .collect::<Vec<_>>();
        let quote = complete_quote_request(CompleteQuotePublicInput {
            job_id: [1; 32],
            request_context: [2; 32],
            quantity: 10,
            quantity_blinding: 3,
            now: 100,
            sentinel: 1_000_000,
            direction: 0,
            asset: 0,
            reference_price: 15_750,
            policies: openings,
            market_digest: [4; 32],
            slot: 1,
            winner_index: 0,
            winner_value: 7,
            eligibility_bits: 48,
            span_bits: 48,
        })
        .unwrap();
        assert_eq!(
            engine.registered_policy_registry_digest(&policies).unwrap(),
            quote.public.registry_digest
        );
        for (maker, policy) in policies.iter().enumerate() {
            let registered = engine
                .registered_policy(maker, policy, &engine.policy_blindings(maker).unwrap())
                .unwrap();
            assert_eq!(
                registered_policy_digest(maker, &registered),
                registered_policy_digest(maker, &quote.public.registry[maker])
            );
        }
    }

    #[test]
    fn generated_reference_matches_the_clear_model_at_runtime_market_time() {
        let endpoints = (0..7)
            .map(|node| format!("http://mpc-{node}:9100"))
            .collect::<Vec<_>>();
        let mut engine = DistributedMpcEngine::new(
            &endpoints,
            2,
            4,
            &[15_750, 10_850, 6_420_000],
            63,
            true,
            Duration::from_secs(30),
        )
        .unwrap();
        let mut rng = StdRng::seed_from_u64(1);
        let policies = (0..4)
            .map(|index| Policy {
                asset: (index % 3) as i64,
                ask_level: rng.gen_range(-15..=15),
                spread: rng.gen_range(10..=80),
                slope: rng.gen_range(0..=3),
                invcoef: 1,
                inv: rng.gen_range(-50..=50),
                maxqty: [50, 100, 200, 500][rng.gen_range(0..4)],
                expiry: 1_000_000_000,
                active: 1,
                use_ref: 1,
            })
            .collect::<Vec<_>>();
        let request = Request::default();
        let settlement = MpcSettlementInputs {
            user_limit: 15_907,
            taker_securities_reserve: 0,
            taker_cash_reserve: 1_590_700,
            maker_securities_reserves: vec![500; 4],
            maker_cash_reserves: vec![10_000_000; 4],
        };
        let (party_files, reference, mask, node_shares) = engine
            .generate(RoundInputGeneration {
                policies: &policies,
                request: &request,
                settlement: &settlement,
                now: 1,
                round_slot: 2,
                response_mask: 17,
                fill_mask: 29,
            })
            .unwrap();
        // A gateway restart advances the fresh-request cursor, but an already
        // signed RFQ must still reproduce byte-identical MPC inputs from its
        // durable slot.
        engine.served = 99;
        let (replayed_files, replayed_reference, replayed_mask, replayed_shares) = engine
            .generate(RoundInputGeneration {
                policies: &policies,
                request: &request,
                settlement: &settlement,
                now: 1,
                round_slot: 2,
                response_mask: 17,
                fill_mask: 29,
            })
            .unwrap();
        assert_eq!(replayed_files, party_files);
        assert_eq!(replayed_reference, reference);
        assert_eq!(replayed_mask, mask);
        assert_eq!(replayed_shares, node_shares);
        let expected = evaluate(&policies, &request, &[15_750, 10_850, 6_420_000], 1);
        let best_key = reference["best_key"].as_i64().unwrap() as i128;
        assert_eq!(
            best_key,
            i128::from(expected.cost.unwrap()) * 4 + expected.winner.unwrap() as i128
        );
        assert_eq!(json_u64(&reference["mask"]).unwrap(), mask);
        assert!(json_u64(&reference["fill_mask"]).unwrap() > 0);
        assert!(expected.price.unwrap() > settlement.user_limit);
    }
}
