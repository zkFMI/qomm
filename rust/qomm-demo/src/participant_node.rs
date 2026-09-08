//! Legal-entity participant service used by the Docker demonstration.
//!
//! The service owns five signature keys, one anonymous KYB key, and separate
//! note viewing/spending keys in the same encrypted Rust key store used by
//! deployed QOMM nodes. It signs only pre-trade policy or execution mandates.
//! There is deliberately no post-match approval route: an accepted MPC result
//! must settle under the standing mandate already registered with DeFMI.

use crate::asset_ids::cash_asset_id;
use crate::defmi_bootstrap::docker_rpc_client;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use defmi::avalanche::{AvalancheClient, CanonicalNoteClaim};
use defmi::facility::{
    CreditFacilityRelationProof, CreditFacilityTransition, CreditTransitionKind, ZERO,
};
use defmi::note_chain::{
    materialize_claim, ClaimOwnershipProof, NoteClaimKind, NoteClaimMaterialization, NoteOutput,
    NoteSpend,
};
use defmi::notes::{encode_spend_proof, Address, NoteLedger, Wallet};
use ed25519_dalek::Signer;
use qomm_mpc::program::PRODUCT_DVP_REMAINDER_BITS;
use qomm_transport::kyb_wire::{KybPresentationWire, KybRegistryWire};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zkfmi_zk::pedersen::Pedersen;
use zkpi_committee::key_management::{EncryptedKeyStore, KeyKind};
use zkpi_committee::mandate::{decode_taker_mandate, Direction, TakerExecutionMandate};
use zkpi_defmi_sdk::corporate::{
    CanonicalReceipt, CorporateOutbox, CoverAction, EnqueueOutcome, MpcAdmissionReceipt,
    OutboxState,
};
use zkpi_proofs::kyb::{
    present, verify_presentation, verify_registry, BusinessAttributes, KybCredential,
};

const MAX_HTTP_BYTES: usize = 1 << 20;
const MAX_SIGNED_BODY: usize = 1 << 16;
const MAX_OUTBOX_REQUEST_BYTES: usize = 1 << 18;
const MAX_OUTBOX_ENTRIES: usize = 10_000;
const MAKER_DOMAIN: &[u8] = b"QOMM:MAKER:POLICY-MANDATE:v2";
const TAKER_DOMAIN: &[u8] = b"QOMM:TAKER:EXECUTION-MANDATE:v2";
const KYB_KEY_PURPOSE: &str = "kyb_entity";
const NOTE_VIEW_KEY_PURPOSE: &str = "note_view";
const NOTE_SPEND_KEY_PURPOSE: &str = "note_spend";
const NOTE_OPENING_KEY_PURPOSE: &str = "note_opening";
const KYB_MAX_TIER: u32 = 4;

/// A note reservation and its credit-facility hold use related, but distinct,
/// finality statements.  On consumption the reservation records the product
/// order statement while the credit hold records the nested settlement
/// statement.  On release only the reservation records the release statement;
/// the credit hold deliberately keeps a zero settlement digest.  Treating the
/// two digests as identical rejects valid canonical history after a restart.
fn reservation_and_facility_finality_agree(
    reservation_status: &str,
    reservation_settlement_digest: [u8; 32],
    hold_status: &str,
    hold_settlement_digest: [u8; 32],
) -> bool {
    if reservation_status != hold_status {
        return false;
    }
    match reservation_status {
        "active" => reservation_settlement_digest == ZERO && hold_settlement_digest == ZERO,
        "released" => reservation_settlement_digest != ZERO && hold_settlement_digest == ZERO,
        "consumed" => reservation_settlement_digest != ZERO && hold_settlement_digest != ZERO,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantRole {
    Maker,
    Taker,
    #[serde(alias = "mpcoperator")]
    MpcOperator,
}

impl ParticipantRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Maker => "maker",
            Self::Taker => "taker",
            Self::MpcOperator => "mpc_operator",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParticipantNodeConfig {
    pub role: ParticipantRole,
    pub label: String,
    pub participant_id: [u8; 32],
    pub listen_host: String,
    pub port: u16,
    pub state_root: PathBuf,
    pub initial_cash: u64,
    pub initial_inventory: u64,
    pub defmi_endpoint: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ParticipantState {
    version: u8,
    role: ParticipantRole,
    label: String,
    participant_id: String,
    cash: u64,
    inventory: u64,
    reserved_cash: u64,
    reserved_inventory: u64,
    sequence: u64,
    defmi_endpoint: String,
    key_ids: BTreeMap<String, String>,
}

struct ParticipantService {
    config: ParticipantNodeConfig,
    store: EncryptedKeyStore,
    outbox: CorporateOutbox,
    state_path: PathBuf,
    state: Mutex<ParticipantState>,
}

#[derive(Debug, Deserialize)]
struct SignRequest {
    body: String,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KybPresentRequest {
    registry: KybRegistryWire,
    trusted_issuer: String,
    scope: String,
    context: String,
    required_cohort: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingPoolSpendRequest {
    asset_id: String,
    source_note_id: String,
    pool_id: String,
    maximum_amount_commitment: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteReservationSpendRequest {
    asset_id: String,
    reserve_id: String,
    amount: u64,
    amount_blinding: u64,
    amount_commitment: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteConsolidationRequest {
    asset_id: String,
    consolidation_id: String,
    minimum_amount: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxEnqueueRequest {
    request_id: String,
    signed_request: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxClaimRequest {
    quorum_healthy: bool,
    retry_after_seconds: u64,
    interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxAdmissionRequest {
    request_id: String,
    request_digest: String,
    committee_id: String,
    job_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxReconcileRequest {
    request_id: String,
    request_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxPreReserveAbortRequest {
    request_id: String,
    request_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FacilityHoldEvidenceRequest {
    request_id: String,
    request_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimMaterializationRequest {
    request_id: String,
    request_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalPortfolioRequest {
    asset_ids: Vec<String>,
}

/// Minimum projection needed to recover the signed Taker reserve identifier
/// from the exact encrypted outbox bytes. Unknown outer fields are ignored so
/// the corporate module need not duplicate the MPC coordinator's full schema.
#[derive(Debug, Deserialize)]
struct QueuedReservationEnvelope {
    maximum_amount: u64,
    maximum_blinding: u64,
    signed_taker_mandate: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueuedReserveTotals {
    cash: u64,
    inventory: u64,
}

impl ParticipantService {
    fn initialize(config: ParticipantNodeConfig) -> Result<Self, String> {
        if config.label.trim().is_empty()
            || config.participant_id == [0; 32]
            || config.port == 0
            || !config.defmi_endpoint.starts_with("http://")
        {
            return Err("participant identity, endpoint, or port is invalid".into());
        }
        fs::create_dir_all(&config.state_root).map_err(|error| error.to_string())?;
        fs::set_permissions(&config.state_root, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let passphrase_path = config.state_root.join("participant-store.passphrase");
        if !passphrase_path.exists() {
            let mut entropy = [0_u8; 32];
            OsRng.fill_bytes(&mut entropy);
            atomic_private_write(&passphrase_path, hex::encode(entropy).as_bytes())?;
            entropy.fill(0);
        }
        let mut passphrase = fs::read(&passphrase_path).map_err(|error| error.to_string())?;
        if passphrase.len() < 12 {
            return Err("participant key-store passphrase is invalid".into());
        }
        let store_path = config.state_root.join("participant-keys.qks");
        let store = EncryptedKeyStore::new(&store_path, &passphrase)?;
        let outbox = CorporateOutbox::new(
            config.state_root.join("corporate-outbox.qob"),
            &passphrase,
            MAX_OUTBOX_ENTRIES,
            MAX_OUTBOX_REQUEST_BYTES,
        )?;
        outbox.initialize_if_missing()?;
        passphrase.fill(0);
        let state_path = config.state_root.join("participant-state.json");
        let mut state = if state_path.exists() {
            let state: ParticipantState =
                serde_json::from_slice(&fs::read(&state_path).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            if state.version != 3
                || !state.key_ids.contains_key("quote_application")
                || !state.key_ids.contains_key("settlement_application")
                || !state.key_ids.contains_key(NOTE_OPENING_KEY_PURPOSE)
                || state.role != config.role
                || state.participant_id != hex::encode(config.participant_id)
            {
                return Err("participant state belongs to another legal entity or role".into());
            }
            state
        } else {
            store.initialize()?;
            let now = unix_seconds()?;
            let lifetime = 3650_u64 * 24 * 60 * 60;
            let metadata = || {
                BTreeMap::from([
                    (
                        "participant_id".into(),
                        Value::String(hex::encode(config.participant_id)),
                    ),
                    ("role".into(), Value::String(config.role.as_str().into())),
                ])
            };
            let mut key_ids = BTreeMap::new();
            for purpose in ["admin", "settlement", "quote", "mpc_input", "emergency"] {
                key_ids.insert(
                    purpose.into(),
                    store.generate(purpose, KeyKind::Ed25519, now, lifetime, metadata())?,
                );
                let pq_purpose = format!("{purpose}_pq");
                key_ids.insert(
                    pq_purpose.clone(),
                    store.generate(&pq_purpose, KeyKind::MlDsa65, now, lifetime, metadata())?,
                );
            }
            for purpose in ["quote_application", "settlement_application"] {
                key_ids.insert(
                    purpose.into(),
                    store.generate(purpose, KeyKind::HybridSignature, now, lifetime, metadata())?,
                );
            }
            key_ids.insert(
                KYB_KEY_PURPOSE.into(),
                store.generate(
                    KYB_KEY_PURPOSE,
                    KeyKind::Ristretto,
                    now,
                    lifetime,
                    metadata(),
                )?,
            );
            for purpose in [NOTE_VIEW_KEY_PURPOSE, NOTE_SPEND_KEY_PURPOSE] {
                key_ids.insert(
                    purpose.into(),
                    store.generate(purpose, KeyKind::Ristretto, now, lifetime, metadata())?,
                );
            }
            key_ids.insert(
                NOTE_OPENING_KEY_PURPOSE.into(),
                store.generate(
                    NOTE_OPENING_KEY_PURPOSE,
                    KeyKind::HybridKem,
                    now,
                    lifetime,
                    metadata(),
                )?,
            );
            let state = ParticipantState {
                version: 3,
                role: config.role,
                label: config.label.clone(),
                participant_id: hex::encode(config.participant_id),
                cash: config.initial_cash,
                inventory: config.initial_inventory,
                reserved_cash: 0,
                reserved_inventory: 0,
                sequence: 0,
                defmi_endpoint: config.defmi_endpoint.clone(),
                key_ids,
            };
            atomic_private_write(
                &state_path,
                &serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
            )?;
            state
        };
        // Migrate participant volumes created before anonymous KYB credentials
        // were introduced.  If key generation completed but the process died
        // before the state file rename, reuse the active key instead of
        // rotating the legal entity's venue-scoped nullifier.
        let missing_ristretto = [
            KYB_KEY_PURPOSE,
            NOTE_VIEW_KEY_PURPOSE,
            NOTE_SPEND_KEY_PURPOSE,
        ]
        .into_iter()
        .filter(|purpose| !state.key_ids.contains_key(*purpose))
        .collect::<Vec<_>>();
        if !missing_ristretto.is_empty() {
            let active = store
                .snapshot()?
                .keys
                .into_iter()
                .filter(|record| record.state == "active")
                .map(|record| (record.purpose, record.key_id))
                .collect::<BTreeMap<_, _>>();
            let now = unix_seconds()?;
            for purpose in missing_ristretto {
                let key_id = match active.get(purpose) {
                    Some(key_id) => key_id.clone(),
                    None => store.generate(
                        purpose,
                        KeyKind::Ristretto,
                        now,
                        3650_u64 * 24 * 60 * 60,
                        BTreeMap::from([
                            (
                                "participant_id".into(),
                                Value::String(hex::encode(config.participant_id)),
                            ),
                            ("role".into(), Value::String(config.role.as_str().into())),
                        ]),
                    )?,
                };
                state.key_ids.insert(purpose.into(), key_id);
            }
            atomic_private_write(
                &state_path,
                &serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
            )?;
        }
        for purpose in ["admin", "settlement", "quote", "mpc_input", "emergency"] {
            if !state.key_ids.contains_key(&format!("{purpose}_pq")) {
                return Err("participant requires explicit independently enrolled PQ keys before live recovery".into());
            }
        }
        Ok(Self {
            config,
            store,
            outbox,
            state_path,
            state: Mutex::new(state),
        })
    }

    fn active_application_fingerprint(&self, purpose: &str) -> Result<[u8; 32], String> {
        let record = self
            .store
            .snapshot()?
            .keys
            .into_iter()
            .find(|record| record.purpose == purpose && record.state == "active")
            .ok_or_else(|| format!("participant has no active {purpose} key"))?;
        if record.kind != KeyKind::HybridSignature {
            return Err(format!(
                "participant {purpose} key requires hybrid re-enrollment"
            ));
        }
        let fingerprint: [u8; 32] = BASE64
            .decode(record.public)
            .map_err(|_| format!("participant {purpose} key is not base64"))?
            .try_into()
            .map_err(|_| format!("participant {purpose} fingerprint is not 32 bytes"))?;
        zkpi_committee::application_crypto::VerifyingKey::from_bytes(&fingerprint)
            .map_err(|_| format!("participant {purpose} fingerprint is invalid"))?;
        Ok(fingerprint)
    }

    fn queued_reservation(
        signed_request: &[u8],
        request_id: &str,
        expires_at: u64,
        taker_public: [u8; 32],
    ) -> Result<(Direction, u64, [u8; 32]), String> {
        let queued: QueuedReservationEnvelope = serde_json::from_slice(signed_request)
            .map_err(|_| "queued RFQ has no canonical reserve envelope".to_string())?;
        if queued.maximum_amount == 0 {
            return Err("queued RFQ cannot reserve an empty amount".into());
        }
        let mandate = decode_taker_mandate(&queued.signed_taker_mandate)?;
        mandate.verify_signature()?;
        if mandate.taker_public != taker_public
            || request_id != hex::encode(mandate.rfq_nullifier)
            || expires_at != mandate.deadline
            || Pedersen::new(b"qomm:defmi:v1")
                .commit_u64(
                    queued.maximum_amount,
                    &Scalar::from(queued.maximum_blinding),
                )
                .compress()
                .to_bytes()
                != mandate.maximum_amount_commitment
        {
            return Err("queued RFQ reserve differs from its signed Taker mandate".into());
        }
        Ok((mandate.direction, queued.maximum_amount, mandate.reserve_id))
    }

    fn queued_reserve_totals(&self, taker_public: [u8; 32]) -> Result<QueuedReserveTotals, String> {
        let mut totals = QueuedReserveTotals::default();
        for entry in self.outbox.summaries()? {
            if matches!(
                entry.state,
                OutboxState::Settled { .. }
                    | OutboxState::Released { .. }
                    | OutboxState::AbortedBeforeReserve { .. }
            ) {
                continue;
            }
            let mut signed = self
                .outbox
                .signed_request(&entry.request_id, entry.request_digest)?;
            let reserve = Self::queued_reservation(
                &signed,
                &entry.request_id,
                entry.expires_at,
                taker_public,
            );
            signed.fill(0);
            let (direction, amount, reserve_id) = reserve?;
            if matches!(entry.state, OutboxState::Expired { .. }) {
                let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
                match rpc.note_reservation_snapshot(reserve_id) {
                    Ok(reservation) if reservation.status == "active" => {}
                    Ok(_) => continue,
                    Err(error) if error.to_ascii_lowercase().contains("not found") => continue,
                    Err(error) => return Err(error),
                }
            }
            match direction {
                Direction::TakerBuys => {
                    totals.cash = totals
                        .cash
                        .checked_add(amount)
                        .ok_or_else(|| "queued cash reserve overflowed".to_string())?;
                }
                Direction::TakerSells => {
                    totals.inventory = totals
                        .inventory
                        .checked_add(amount)
                        .ok_or_else(|| "queued inventory reserve overflowed".to_string())?;
                }
            }
        }
        Ok(totals)
    }

    fn queued_envelope_and_mandate(
        &self,
        request_id: &str,
        request_digest: [u8; 32],
    ) -> Result<(QueuedReservationEnvelope, TakerExecutionMandate), String> {
        let mut signed = self.outbox.signed_request(request_id, request_digest)?;
        let envelope = serde_json::from_slice::<QueuedReservationEnvelope>(&signed)
            .map_err(|_| "queued RFQ envelope is not canonical JSON".to_string());
        signed.fill(0);
        let mut envelope = envelope?;
        let mandate = decode_taker_mandate(&envelope.signed_taker_mandate);
        envelope.signed_taker_mandate.fill(0);
        let mandate = mandate?;
        mandate.verify_signature()?;
        let amount_blinding = Scalar::from(envelope.maximum_blinding);
        if request_id != hex::encode(mandate.rfq_nullifier)
            || envelope.maximum_amount == 0
            || Pedersen::new(b"qomm:defmi:v1")
                .commit_u64(envelope.maximum_amount, &amount_blinding)
                .compress()
                .to_bytes()
                != mandate.maximum_amount_commitment
        {
            return Err("queued RFQ differs from its signed Taker mandate".into());
        }
        Ok((envelope, mandate))
    }

    /// Rebuild the Taker's economic holdings from final DeFMI claims and the
    /// exact corporate RFQs that created them.  The static participant values
    /// are genesis holdings only; returning those values after a gateway
    /// restart would silently erase prior DvP from the browser projection.
    fn canonical_taker_portfolio(
        &self,
        request: CanonicalPortfolioRequest,
    ) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker participant can reconstruct this portfolio".into());
        }
        if request.asset_ids.is_empty() || request.asset_ids.len() > 64 {
            return Err("canonical portfolio asset count is outside product bounds".into());
        }
        let mut requested_assets = BTreeSet::new();
        for encoded in request.asset_ids {
            let asset_id = fixed_hex(&encoded, "canonical portfolio asset")?;
            if asset_id == ZERO || !requested_assets.insert(asset_id) {
                return Err("canonical portfolio assets are empty or repeated".into());
            }
        }
        let (initial_cash, initial_inventory) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (state.cash, state.inventory)
        };
        let mut cash = initial_cash;
        let mut inventory = requested_assets
            .iter()
            .map(|asset_id| (*asset_id, initial_inventory))
            .collect::<BTreeMap<_, _>>();
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let before = rpc.state_root()?;
        let recipient_secret = Scalar::from(participant_handle_scalar(
            b"taker",
            &self.config.participant_id,
        ));
        let recipient_view = (G * recipient_secret).compress();
        let mut claims_by_settlement = BTreeMap::<[u8; 32], Vec<CanonicalNoteClaim>>::new();
        let mut after = None;
        loop {
            let page = rpc.note_claim_recipient_page(recipient_view.to_bytes(), after, 256)?;
            if page.state_root != before {
                return Err("DeFMI changed while reading the Taker portfolio".into());
            }
            for claim in page.claims {
                claim.claim()?;
                if claim.opening_envelope.recipient_view.compress() != recipient_view
                    || claim.settlement_digest == ZERO
                {
                    return Err("canonical Taker claim has another recipient or settlement".into());
                }
                claims_by_settlement
                    .entry(claim.settlement_digest)
                    .or_default()
                    .push(claim);
            }
            let Some(next) = page.next else { break };
            if after == Some(next) {
                return Err("recipient-claim pagination cursor did not advance".into());
            }
            after = Some(next);
        }

        let key = Pedersen::new(b"qomm:defmi:v1");
        let cash_asset = cash_asset_id();
        let mut consumed_settlements = 0_u64;
        for entry in self.outbox.summaries()? {
            let (envelope, mandate) =
                self.queued_envelope_and_mandate(&entry.request_id, entry.request_digest)?;
            if !requested_assets.contains(&mandate.asset_id) {
                return Err(
                    "a settled RFQ names an asset omitted from the portfolio request".into(),
                );
            }
            let reservation = match rpc.note_reservation_snapshot(mandate.reserve_id) {
                Ok(reservation) => reservation,
                Err(error) if error.to_ascii_lowercase().contains("not found") => continue,
                Err(error) => return Err(error),
            };
            if reservation.state_root != before
                || reservation.hold_id != mandate.reserve_id
                || reservation.asset_id != mandate.reserve_asset_id
                || reservation.amount_commitment != mandate.maximum_amount_commitment
            {
                return Err("canonical Taker reservation differs from its signed RFQ".into());
            }
            if reservation.status != "consumed" {
                if !matches!(reservation.status.as_str(), "active" | "released") {
                    return Err("canonical Taker reservation has an unknown status".into());
                }
                continue;
            }
            let claims = claims_by_settlement
                .remove(&reservation.settlement_digest)
                .ok_or_else(|| "settled Taker RFQ has no recipient claims".to_string())?;
            let mut refund = None;
            let mut delivery = None;
            for claim in claims {
                if claim.source_hold_id == mandate.reserve_id && claim.kind == NoteClaimKind::Refund
                {
                    if refund.replace(claim).is_some() {
                        return Err("settled Taker RFQ has duplicate refund claims".into());
                    }
                } else if claim.source_hold_id != mandate.reserve_id
                    && claim.kind == NoteClaimKind::Delivery
                {
                    if delivery.replace(claim).is_some() {
                        return Err("settled Taker RFQ has duplicate delivery claims".into());
                    }
                } else {
                    return Err("settled Taker RFQ has an unexpected recipient claim".into());
                }
            }
            let refund =
                refund.ok_or_else(|| "settled Taker RFQ has no refund claim".to_string())?;
            let delivery =
                delivery.ok_or_else(|| "settled Taker RFQ has no delivery claim".to_string())?;
            let refund_amount = canonical_claim_amount(
                &refund,
                &recipient_secret,
                self.note_opening_key()?.as_ref(),
                &key,
            )?;
            let delivery_amount = canonical_claim_amount(
                &delivery,
                &recipient_secret,
                self.note_opening_key()?.as_ref(),
                &key,
            )?;
            let consumed = envelope
                .maximum_amount
                .checked_sub(refund_amount)
                .ok_or_else(|| "Taker refund exceeds its signed maximum".to_string())?;
            match mandate.direction {
                Direction::TakerBuys => {
                    if mandate.reserve_asset_id != cash_asset
                        || delivery.asset_id != mandate.asset_id
                        || refund.asset_id != cash_asset
                    {
                        return Err("Taker buy claims cross the cash and securities rails".into());
                    }
                    cash = cash
                        .checked_sub(consumed)
                        .ok_or_else(|| "canonical Taker cash balance underflowed".to_string())?;
                    let value = inventory.get_mut(&mandate.asset_id).ok_or_else(|| {
                        "Taker buy asset is absent from its portfolio".to_string()
                    })?;
                    *value = value
                        .checked_add(delivery_amount)
                        .ok_or_else(|| "canonical Taker inventory overflowed".to_string())?;
                }
                Direction::TakerSells => {
                    if mandate.reserve_asset_id != mandate.asset_id
                        || refund.asset_id != mandate.asset_id
                        || delivery.asset_id != cash_asset
                    {
                        return Err("Taker sell claims cross the cash and securities rails".into());
                    }
                    let value = inventory.get_mut(&mandate.asset_id).ok_or_else(|| {
                        "Taker sell asset is absent from its portfolio".to_string()
                    })?;
                    *value = value
                        .checked_sub(consumed)
                        .ok_or_else(|| "canonical Taker inventory underflowed".to_string())?;
                    cash = cash
                        .checked_add(delivery_amount)
                        .ok_or_else(|| "canonical Taker cash balance overflowed".to_string())?;
                }
            }
            consumed_settlements = consumed_settlements
                .checked_add(1)
                .ok_or_else(|| "canonical settlement count overflowed".to_string())?;
        }
        if !claims_by_settlement.is_empty() || rpc.state_root()? != before {
            return Err("Taker portfolio has an unbound claim or stale DeFMI root".into());
        }
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "state_root": hex::encode(before),
            "cash": cash,
            "inventory": inventory.into_iter().map(|(asset_id, amount)| json!({
                "asset_id": hex::encode(asset_id),
                "amount": amount,
            })).collect::<Vec<_>>(),
            "consumed_settlements": consumed_settlements,
            "source": "defmi_final_claims_and_corporate_outbox",
            "private_openings_disclosed": false,
        }))
    }

    /// Build a hold transition from the participant's exact encrypted RFQ
    /// history and current DeFMI state.  The coordinator receives a range proof
    /// and public commitments, never the aggregate available, held, or
    /// outstanding openings.
    fn facility_hold_evidence(
        &self,
        request: FacilityHoldEvidenceRequest,
    ) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker participant can prove an aggregate facility hold".into());
        }
        let request_digest = fixed_hex(&request.request_digest, "outbox request digest")?;
        let summary = self
            .outbox
            .summaries()?
            .into_iter()
            .find(|entry| {
                entry.request_id == request.request_id && entry.request_digest == request_digest
            })
            .ok_or_else(|| "corporate outbox request was not found".to_string())?;
        if matches!(
            summary.state,
            OutboxState::Settled { .. }
                | OutboxState::Released { .. }
                | OutboxState::AbortedBeforeReserve { .. }
        ) {
            return Err("a finalized corporate RFQ cannot create a new facility hold".into());
        }
        let (envelope, mandate) =
            self.queued_envelope_and_mandate(&request.request_id, request_digest)?;
        let mandate_digest = mandate.digest()?;
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        match rpc.note_reservation_snapshot(mandate.reserve_id) {
            Ok(_) => return Err("canonical DeFMI already contains this Taker reserve".into()),
            Err(error) if error.to_ascii_lowercase().contains("not found") => {}
            Err(error) => return Err(error),
        }
        let facility_id = hash_parts(&[
            b"QOMM:DEMO:TAKER-FACILITY:v1",
            &mandate.defmi_id,
            &mandate.entity_commitment,
            &mandate.reserve_asset_id,
        ]);
        let cap_amount = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            match mandate.direction {
                Direction::TakerBuys => state.cash,
                Direction::TakerSells => state.inventory,
            }
        };
        if cap_amount == 0 {
            return Err("Taker aggregate facility cannot have an empty cap".into());
        }
        let cap_blinding =
            deterministic_scalar(&[b"QOMM:DEMO:TAKER-FACILITY-CAP-BLINDING:v1", &facility_id]);
        let key = Pedersen::new(b"qomm:defmi:v1");
        let expected_cap = key
            .commit_u64(cap_amount, &cap_blinding)
            .compress()
            .to_bytes();
        let initial_facility = rpc.credit_facility_snapshot(facility_id)?;
        if initial_facility.facility.facility_id != facility_id
            || initial_facility.facility.beneficiary_commitment != mandate.entity_commitment
            || initial_facility.facility.rail_asset_id != mandate.reserve_asset_id
            || initial_facility.facility.cap_commitment != expected_cap
        {
            return Err("canonical Taker facility differs from the queued legal entity".into());
        }

        let recipient_secret = Scalar::from(participant_handle_scalar(
            b"taker",
            &self.config.participant_id,
        ));
        let recipient_view = G * recipient_secret;
        let mut held_amount = 0_u64;
        let mut held_blinding = Scalar::ZERO;
        let mut outstanding_amount = 0_u64;
        let mut outstanding_blinding = Scalar::ZERO;
        for entry in self.outbox.summaries()? {
            let (prior_envelope, prior_mandate) =
                self.queued_envelope_and_mandate(&entry.request_id, entry.request_digest)?;
            if prior_mandate.defmi_id != mandate.defmi_id
                || prior_mandate.entity_commitment != mandate.entity_commitment
                || prior_mandate.reserve_asset_id != mandate.reserve_asset_id
            {
                continue;
            }
            let reservation = match rpc.note_reservation_snapshot(prior_mandate.reserve_id) {
                Ok(reservation) => reservation,
                Err(error) if error.to_ascii_lowercase().contains("not found") => continue,
                Err(error) => return Err(error),
            };
            let hold = rpc.credit_hold_snapshot(prior_mandate.reserve_id)?;
            if reservation.hold_id != prior_mandate.reserve_id
                || reservation.asset_id != prior_mandate.reserve_asset_id
                || reservation.amount_commitment != prior_mandate.maximum_amount_commitment
                || reservation.state_root != initial_facility.state_root
                || hold.hold_id != reservation.hold_id
                || hold.facility_id != facility_id
                || hold.query_commitment != prior_mandate.digest()?
                || hold.amount_commitment != reservation.amount_commitment
                || hold.expires_at != prior_mandate.deadline
                || hold.state_root != initial_facility.state_root
                || !reservation_and_facility_finality_agree(
                    &reservation.status,
                    reservation.settlement_digest,
                    &hold.status,
                    hold.settlement_digest,
                )
            {
                return Err("canonical Taker reservation and facility hold disagree".into());
            }
            let maximum_blinding = Scalar::from(prior_envelope.maximum_blinding);
            match reservation.status.as_str() {
                "active" => {
                    held_amount = held_amount
                        .checked_add(prior_envelope.maximum_amount)
                        .ok_or_else(|| "aggregate held amount overflowed".to_string())?;
                    held_blinding += maximum_blinding;
                }
                "released" => {}
                "consumed" => {
                    let mut after = None;
                    let mut refund = None;
                    let mut claim_root = None;
                    loop {
                        let page = rpc.note_claim_page(prior_mandate.reserve_id, after, 64)?;
                        if page.state_root != initial_facility.state_root {
                            return Err(
                                "DeFMI note claims and facility use different state roots".into()
                            );
                        }
                        match claim_root {
                            None => claim_root = Some(page.state_root),
                            Some(root) if root != page.state_root => {
                                return Err(
                                    "DeFMI note claims changed while rebuilding the facility"
                                        .into(),
                                )
                            }
                            Some(_) => {}
                        }
                        for claim in page.claims {
                            if claim.source_hold_id != prior_mandate.reserve_id
                                || claim.settlement_digest != reservation.settlement_digest
                            {
                                return Err(
                                    "canonical note claim differs from its consumed hold".into()
                                );
                            }
                            if claim.kind == NoteClaimKind::Refund
                                && refund.replace(claim).is_some()
                            {
                                return Err(
                                    "consumed Taker hold has more than one refund claim".into()
                                );
                            }
                        }
                        let Some(next) = page.next else { break };
                        if after == Some(next) {
                            return Err("note-claim pagination cursor did not advance".into());
                        }
                        after = Some(next);
                    }
                    let refund = refund
                        .ok_or_else(|| "consumed Taker hold has no refund claim".to_string())?;
                    refund.claim()?;
                    if refund.opening_envelope.recipient_view.compress()
                        != recipient_view.compress()
                    {
                        return Err("Taker refund opening belongs to another recipient".into());
                    }
                    let quorum = refund
                        .opening_envelope
                        .shares
                        .iter()
                        .take(refund.opening_envelope.threshold)
                        .map(|share| share.party)
                        .collect::<Vec<_>>();
                    let (refund_scalar, refund_blinding) = refund.opening_envelope.decrypt(
                        &recipient_secret,
                        self.note_opening_key()?.as_ref(),
                        &quorum,
                    )?;
                    let refund_amount = scalar_to_u64(refund_scalar)?;
                    if refund_amount > prior_envelope.maximum_amount
                        || key
                            .commit_u64(refund_amount, &refund_blinding)
                            .compress()
                            .to_bytes()
                            != refund.value_commitment
                    {
                        return Err("Taker refund opening differs from its final claim".into());
                    }
                    let consumed = prior_envelope.maximum_amount - refund_amount;
                    outstanding_amount = outstanding_amount
                        .checked_add(consumed)
                        .ok_or_else(|| "aggregate outstanding amount overflowed".to_string())?;
                    outstanding_blinding += maximum_blinding - refund_blinding;
                }
                _ => return Err("canonical Taker reservation has an unknown status".into()),
            }
        }
        let encumbered = held_amount
            .checked_add(outstanding_amount)
            .ok_or_else(|| "aggregate facility usage overflowed".to_string())?;
        let available_amount = cap_amount
            .checked_sub(encumbered)
            .ok_or_else(|| "canonical Taker facility exceeds its aggregate cap".to_string())?;
        let available_blinding = cap_blinding - held_blinding - outstanding_blinding;
        let canonical = rpc.credit_facility_snapshot(facility_id)?;
        if canonical != initial_facility
            || rpc.state_root()? != canonical.state_root
            || canonical.facility.available_commitment
                != key
                    .commit_u64(available_amount, &available_blinding)
                    .compress()
                    .to_bytes()
            || canonical.facility.held_commitment
                != key
                    .commit_u64(held_amount, &held_blinding)
                    .compress()
                    .to_bytes()
            || canonical.facility.outstanding_commitment
                != key
                    .commit_u64(outstanding_amount, &outstanding_blinding)
                    .compress()
                    .to_bytes()
        {
            return Err("participant reconstruction differs from canonical facility state".into());
        }
        if envelope.maximum_amount > available_amount {
            return Err("Taker RFQ exceeds the remaining legal-entity facility".into());
        }
        let amount_blinding = Scalar::from(envelope.maximum_blinding);
        let after_available = available_amount - envelope.maximum_amount;
        let after_held = held_amount
            .checked_add(envelope.maximum_amount)
            .ok_or_else(|| "post-hold aggregate amount overflowed".to_string())?;
        let after_available_blinding = available_blinding - amount_blinding;
        let after_held_blinding = held_blinding + amount_blinding;
        let mut transition = CreditFacilityTransition {
            operation_id: hash_parts(&[b"QOMM:DEMO:TAKER-HOLD-OP:v1", &mandate.reserve_id]),
            facility_id,
            hold_id: mandate.reserve_id,
            kind: CreditTransitionKind::Hold,
            query_commitment: mandate_digest,
            amount_commitment: mandate.maximum_amount_commitment,
            consumed_commitment: ZERO,
            refund_commitment: ZERO,
            before_available_commitment: canonical.facility.available_commitment,
            after_available_commitment: key
                .commit_u64(after_available, &after_available_blinding)
                .compress()
                .to_bytes(),
            before_held_commitment: canonical.facility.held_commitment,
            after_held_commitment: key
                .commit_u64(after_held, &after_held_blinding)
                .compress()
                .to_bytes(),
            before_outstanding_commitment: canonical.facility.outstanding_commitment,
            after_outstanding_commitment: canonical.facility.outstanding_commitment,
            before_sequence: canonical.facility.sequence,
            expires_at: mandate.deadline,
            settlement_digest: ZERO,
            relation_proof_digest: ZERO,
        };
        let relation_proof = CreditFacilityRelationProof::prove(
            &mut transition,
            [
                after_available,
                after_held,
                outstanding_amount,
                envelope.maximum_amount,
            ],
            [
                after_available_blinding,
                after_held_blinding,
                outstanding_blinding,
                amount_blinding,
            ],
            [0, 0],
            [Scalar::ZERO, Scalar::ZERO],
            &mut OsRng,
        )?;
        relation_proof.verify(&transition)?;
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "request_id": request.request_id,
            "request_digest": hex::encode(request_digest),
            "state_root": hex::encode(canonical.state_root),
            "transition": transition.body()?,
            "relation_proof": BASE64.encode(relation_proof.to_bytes()?),
            "private_openings_disclosed": false,
        }))
    }

    /// Convert every final entitlement addressed to this legal entity into a
    /// spendable confidential note.  This is a recipient withdrawal proof, not
    /// a second trade signature: settlement is already final and the operation
    /// cannot alter its amount or beneficiary.
    fn claim_materializations(
        &self,
        request: ClaimMaterializationRequest,
    ) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker participant can recover Taker settlement claims".into());
        }
        let request_digest = fixed_hex(&request.request_digest, "outbox request digest")?;
        self.queued_envelope_and_mandate(&request.request_id, request_digest)?;
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let mut settlement_requests = BTreeMap::new();
        for entry in self.outbox.summaries()? {
            let (_, mandate) =
                self.queued_envelope_and_mandate(&entry.request_id, entry.request_digest)?;
            let reservation = match rpc.note_reservation_snapshot(mandate.reserve_id) {
                Ok(reservation) => reservation,
                Err(error) if error.to_ascii_lowercase().contains("not found") => continue,
                Err(error) => return Err(error),
            };
            if reservation.status == "consumed" {
                if reservation.settlement_digest == ZERO {
                    return Err("consumed Taker reservation has no settlement digest".into());
                }
                match settlement_requests
                    .insert(reservation.settlement_digest, mandate.rfq_nullifier)
                {
                    Some(existing) if existing != mandate.rfq_nullifier => {
                        return Err("one settlement digest is bound to multiple Taker RFQs".into())
                    }
                    _ => {}
                }
            }
        }
        let (view_key_id, spend_key_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (
                state
                    .key_ids
                    .get(NOTE_VIEW_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note view key".to_string())?,
                state
                    .key_ids
                    .get(NOTE_SPEND_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note spend key".to_string())?,
            )
        };
        let now = unix_seconds()?;
        let view = self
            .store
            .private_key(&view_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note view key is not Ristretto".to_string())?;
        let spend = self
            .store
            .private_key(&spend_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note spend key is not Ristretto".to_string())?;
        let destination = Wallet::from_parts(view, spend, self.note_opening_key()?).address;
        let recipient_secret = Scalar::from(participant_handle_scalar(
            b"taker",
            &self.config.participant_id,
        ));
        let recipient_view = G * recipient_secret;
        let recipient_encoded = recipient_view.compress().to_bytes();
        let mut root = None;
        let mut after = None;
        let mut active = Vec::new();
        loop {
            let page = rpc.note_claim_recipient_page(recipient_encoded, after, 256)?;
            match root {
                None => root = Some(page.state_root),
                Some(expected) if expected != page.state_root => {
                    return Err("DeFMI recipient claims changed during materialization".into())
                }
                Some(_) => {}
            }
            active.extend(
                page.claims
                    .into_iter()
                    .filter(|claim| claim.status == "active"),
            );
            if active.len() > 4_096 {
                return Err("active recipient claims exceed the participant bound".into());
            }
            let Some(next) = page.next else { break };
            if after == Some(next) {
                return Err("recipient-claim pagination cursor did not advance".into());
            }
            after = Some(next);
        }
        let state_root = root.unwrap_or(rpc.state_root()?);
        if rpc.state_root()? != state_root {
            return Err("DeFMI changed before recipient proofs were sealed".into());
        }
        let key = Pedersen::new(b"qomm:defmi:v1");
        let mut values = Vec::with_capacity(active.len());
        for canonical in active {
            let rfq_nullifier = *settlement_requests
                .get(&canonical.settlement_digest)
                .ok_or_else(|| {
                    "recipient claim is not bound to this Taker's settled RFQ".to_string()
                })?;
            let claim = canonical.claim()?;
            let quorum = claim
                .opening_envelope
                .shares
                .iter()
                .take(claim.opening_envelope.threshold)
                .map(|share| share.party)
                .collect::<Vec<_>>();
            let operation_id = hash_parts(&[
                b"QOMM:DEMO:TAKER-CLAIM-MATERIALIZATION:v1",
                &self.config.participant_id,
                &claim.claim_id,
            ]);
            let (materialization, proof) = materialize_claim(
                &claim,
                &key,
                PRODUCT_DVP_REMAINDER_BITS,
                rfq_nullifier,
                &recipient_secret,
                self.note_opening_key()?.as_ref(),
                &destination,
                &quorum,
                operation_id,
                &mut OsRng,
            )?;
            values.push(claim_materialization_json(
                &materialization,
                &proof,
                rfq_nullifier,
                recipient_encoded,
                &destination,
            )?);
        }
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "request_id": request.request_id,
            "request_digest": hex::encode(request_digest),
            "state_root": hex::encode(state_root),
            "materializations": values,
            "post_match_signature": false,
            "private_openings_disclosed": false,
        }))
    }

    fn note_opening_key(
        &self,
    ) -> Result<std::sync::Arc<zkfmi_crypto::hybrid::kem::HybridKemKey>, String> {
        let key_id = self
            .state
            .lock()
            .map_err(|_| "participant state lock poisoned")?
            .key_ids
            .get(NOTE_OPENING_KEY_PURPOSE)
            .cloned()
            .ok_or("participant has no hybrid note opening key")?;
        self.store
            .private_key(&key_id, unix_seconds()?, false)?
            .hybrid_kem()
            .map(|key| key.shared_key())
            .ok_or("note opening key is not hybrid KEM".into())
    }

    fn snapshot(&self) -> Result<Value, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "participant state lock poisoned")?;
        let keys = self.store.snapshot()?;
        let public_for = |purpose: &str| -> Result<String, String> {
            keys.keys
                .iter()
                .find(|record| record.purpose == purpose && record.state == "active")
                .map(|record| record.public.clone())
                .ok_or_else(|| format!("participant has no active {purpose} key"))
        };
        let note_view = public_for(NOTE_VIEW_KEY_PURPOSE)?;
        let note_spend = public_for(NOTE_SPEND_KEY_PURPOSE)?;
        let note_opening = public_for(NOTE_OPENING_KEY_PURPOSE)?;
        let now = unix_seconds()?;
        let outbox_entries = self.outbox.summaries()?;
        let outbox_metrics = self.outbox.metrics(now)?;
        let queued_reserves = if self.config.role == ParticipantRole::Taker {
            let settlement_public =
                self.active_application_fingerprint("settlement_application")?;
            self.queued_reserve_totals(settlement_public)?
        } else {
            QueuedReserveTotals {
                cash: state.reserved_cash,
                inventory: state.reserved_inventory,
            }
        };
        Ok(json!({
            "ok": true,
            "service": "qomm-participant-node",
            "role": state.role,
            "label": state.label,
            "participant_id": state.participant_id,
            "portfolio": {
                "cash": state.cash,
                "inventory": state.inventory,
                "reserved_cash": queued_reserves.cash,
                "reserved_inventory": queued_reserves.inventory,
            },
            "sequence": state.sequence,
            "defmi_endpoint": state.defmi_endpoint,
            "keys": {
                "version": keys.version,
                "generation": keys.generation,
                "keys": keys.keys,
            },
            "note_address": {
                "view": note_view,
                "spend": note_spend,
                "opening": note_opening,
            },
            "mpc_outbox": {
                "durable": true,
                "local_execution_fallback": false,
                "entries": outbox_entries,
                "metrics": outbox_metrics,
            },
            "post_match_signature": false,
        }))
    }

    fn enqueue_outbox(&self, request: OutboxEnqueueRequest) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker corporate module can enqueue an RFQ".into());
        }
        let OutboxEnqueueRequest {
            request_id,
            signed_request: encoded_request,
            expires_at,
        } = request;
        let mut signed_request = BASE64
            .decode(encoded_request)
            .map_err(|_| "outbox request is not base64".to_string())?;
        // Keep every fallible branch inside one scope, then wipe the decoded
        // request exactly once.  This covers storage, capacity and lock errors
        // as well as validation failures; none can leave the plaintext queue
        // envelope in this process after the call returns.
        let result = (|| {
            if signed_request.is_empty() || signed_request.len() > MAX_OUTBOX_REQUEST_BYTES {
                return Err("outbox request is empty or outside its fixed bound".into());
            }
            let accepted_at = unix_seconds()?;
            if expires_at <= accepted_at {
                return Err("an expired request cannot enter the MPC outbox".into());
            }
            let settlement_public =
                self.active_application_fingerprint("settlement_application")?;
            let (direction, amount, _) = Self::queued_reservation(
                &signed_request,
                &request_id,
                expires_at,
                settlement_public,
            )?;
            // Serialize the capacity check with the append. The totals are
            // derived from encrypted durable entries, so a process restart
            // cannot forget a reservation and two simultaneous RFQs cannot
            // both spend the same corporate limit.
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            let existing = self
                .outbox
                .summaries()?
                .into_iter()
                .any(|entry| entry.request_id == request_id);
            let mut totals = self.queued_reserve_totals(settlement_public)?;
            if !existing {
                match direction {
                    Direction::TakerBuys => {
                        totals.cash = totals
                            .cash
                            .checked_add(amount)
                            .ok_or_else(|| "queued cash reserve overflowed".to_string())?;
                    }
                    Direction::TakerSells => {
                        totals.inventory = totals
                            .inventory
                            .checked_add(amount)
                            .ok_or_else(|| "queued inventory reserve overflowed".to_string())?;
                    }
                }
            }
            if totals.cash > state.cash || totals.inventory > state.inventory {
                return Err("queued RFQs exceed the corporate cash or inventory limit".into());
            }
            let digest = Sha256::digest(&signed_request);
            let outcome = self.outbox.enqueue_first_seen(
                &request_id,
                &signed_request,
                accepted_at,
                expires_at,
            )?;
            drop(state);
            let (status, sequence) = match outcome {
                EnqueueOutcome::Enqueued { sequence } => ("enqueued", sequence),
                EnqueueOutcome::AlreadyPresent { sequence } => ("already_present", sequence),
            };
            Ok(json!({
                "version": 1,
                "participant_id": hex::encode(self.config.participant_id),
                "request_id": request_id,
                "request_digest": hex::encode(digest),
                "sequence": sequence,
                "status": status,
                "local_execution_fallback": false,
            }))
        })();
        signed_request.fill(0);
        result
    }

    fn claim_outbox(&self, request: OutboxClaimRequest) -> Result<Value, String> {
        if self.config.role == ParticipantRole::MpcOperator {
            return Err("an MPC operator has no corporate request outbox".into());
        }
        if request.retry_after_seconds > 3_600
            || request.interval_seconds == 0
            || request.interval_seconds > 3_600
        {
            return Err("outbox retry or cover interval is outside its fixed bound".into());
        }
        let now = unix_seconds()?;
        let slot = self.outbox.claim_cover_slot(
            now,
            request.quorum_healthy,
            request.retry_after_seconds,
            request.interval_seconds,
        )?;
        let Some(slot) = slot else {
            return Ok(json!({
                "version": 1,
                "participant_id": hex::encode(self.config.participant_id),
                "action": "not_due",
                "local_execution_fallback": false,
            }));
        };
        let mut value = match slot.action {
            CoverAction::Real(mut claimed) => {
                let encoded = BASE64.encode(&claimed.signed_request);
                claimed.signed_request.fill(0);
                json!({
                    "action": "real",
                    "request_id": claimed.request_id,
                    "sequence": claimed.sequence,
                    "accepted_at": claimed.accepted_at,
                    "expires_at": claimed.expires_at,
                    "request_digest": hex::encode(claimed.request_digest),
                    "signed_request": encoded,
                    "attempt": claimed.attempt,
                })
            }
            CoverAction::Dummy => json!({"action": "dummy"}),
            CoverAction::Expire {
                request_id,
                request_digest,
            } => {
                let mut signed_request = self.outbox.signed_request(&request_id, request_digest)?;
                let encoded = BASE64.encode(&signed_request);
                signed_request.fill(0);
                json!({
                    "action": "expire",
                    "request_id": request_id,
                    "request_digest": hex::encode(request_digest),
                    "signed_request": encoded,
                })
            }
        };
        let object = value
            .as_object_mut()
            .ok_or_else(|| "outbox cover response is not an object".to_string())?;
        object.insert("version".into(), Value::from(1));
        object.insert(
            "participant_id".into(),
            Value::String(hex::encode(self.config.participant_id)),
        );
        object.insert("slot".into(), Value::from(slot.slot));
        object.insert("due_at".into(), Value::from(slot.due_at));
        object.insert("local_execution_fallback".into(), Value::Bool(false));
        Ok(value)
    }

    fn record_outbox_admission(&self, request: OutboxAdmissionRequest) -> Result<Value, String> {
        if self.config.role == ParticipantRole::MpcOperator {
            return Err("an MPC operator has no corporate request outbox".into());
        }
        let request_digest = fixed_hex(&request.request_digest, "outbox request digest")?;
        let admitted_at = unix_seconds()?;
        self.outbox.record_mpc_admission(
            &request.request_id,
            request_digest,
            admitted_at,
            MpcAdmissionReceipt {
                committee_id: request.committee_id,
                job_id: request.job_id,
                admitted_request_digest: request_digest,
            },
        )?;
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "request_id": request.request_id,
            "request_digest": hex::encode(request_digest),
            "status": "mpc_admitted",
            "admitted_at": admitted_at,
            "local_execution_fallback": false,
        }))
    }

    /// Close a failed dispatch only after this participant module has proved
    /// from canonical DeFMI state that the signed reserve was never created.
    /// This prevents a coordinator error from pinning the corporate limit,
    /// without manufacturing a ledger receipt or releasing a real hold.
    fn abort_outbox_before_reserve(
        &self,
        request: OutboxPreReserveAbortRequest,
    ) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker participant can abort an unreserved RFQ".into());
        }
        let request_digest = fixed_hex(&request.request_digest, "outbox request digest")?;
        let summary = self
            .outbox
            .summaries()?
            .into_iter()
            .find(|entry| {
                entry.request_id == request.request_id && entry.request_digest == request_digest
            })
            .ok_or_else(|| "corporate outbox request was not found".to_string())?;
        let settlement_public = self.active_application_fingerprint("settlement_application")?;
        let mut signed = self
            .outbox
            .signed_request(&request.request_id, request_digest)?;
        let reservation = Self::queued_reservation(
            &signed,
            &request.request_id,
            summary.expires_at,
            settlement_public,
        );
        signed.fill(0);
        let (_, _, reserve_id) = reservation?;
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        match rpc.note_reservation_snapshot(reserve_id) {
            Ok(_) => return Err(
                "canonical DeFMI already contains this reserve; reconcile it instead of aborting"
                    .into(),
            ),
            Err(error) if error.to_ascii_lowercase().contains("not found") => {}
            Err(error) => return Err(error),
        }
        let finalized_at = unix_seconds()?;
        self.outbox
            .record_pre_reserve_abort(&request.request_id, request_digest, finalized_at)?;
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "request_id": request.request_id,
            "request_digest": hex::encode(request_digest),
            "reserve_id": hex::encode(reserve_id),
            "state_root": hex::encode(rpc.state_root()?),
            "status": "aborted_before_reserve",
            "finalized_at": finalized_at,
            "local_execution_fallback": false,
        }))
    }

    /// Periodically close expired queue entries that never reached DeFMI.
    /// The participant reconstructs each reserve identifier from its encrypted
    /// request and checks canonical DeFMI itself; the coordinator cannot forge
    /// this terminal transition or manufacture a ledger receipt.
    fn sweep_expired_unreserved(&self) -> Result<usize, String> {
        if self.config.role != ParticipantRole::Taker {
            return Ok(0);
        }
        let candidates = self
            .outbox
            .summaries()?
            .into_iter()
            .filter(|entry| matches!(entry.state, OutboxState::Expired { .. }))
            .map(|entry| OutboxPreReserveAbortRequest {
                request_id: entry.request_id,
                request_digest: hex::encode(entry.request_digest),
            })
            .collect::<Vec<_>>();
        let mut finalized = 0_usize;
        for request in candidates {
            let request_id = request.request_id.clone();
            let request_digest = request.request_digest.clone();
            match self.abort_outbox_before_reserve(request) {
                Ok(_) => finalized += 1,
                Err(error) if error.contains("canonical DeFMI already contains this reserve") => {
                    // A real canonical reserve must be released or settled,
                    // never relabelled as a pre-reserve abort. Reconciliation
                    // records an already terminal ledger transition and leaves
                    // an active reserve for the coordinator's release path.
                    self.reconcile_outbox(OutboxReconcileRequest {
                        request_id,
                        request_digest,
                    })?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(finalized)
    }

    /// Reconcile an uncertain coordinator response against the canonical DeFMI
    /// reservation. The caller cannot choose the hold: it is recovered from the
    /// exact participant-owned queued bytes and authenticated Taker mandate.
    fn reconcile_outbox(&self, request: OutboxReconcileRequest) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Taker {
            return Err("only a Taker participant can reconcile an RFQ outbox".into());
        }
        let request_digest = fixed_hex(&request.request_digest, "outbox request digest")?;
        let summary = self
            .outbox
            .summaries()?
            .into_iter()
            .find(|entry| {
                entry.request_id == request.request_id && entry.request_digest == request_digest
            })
            .ok_or_else(|| "corporate outbox request was not found".to_string())?;
        let aborted_before_reserve =
            matches!(&summary.state, OutboxState::AbortedBeforeReserve { .. });
        let prior_terminal = match &summary.state {
            OutboxState::Settled { receipt } => Some(("consumed", receipt.clone())),
            OutboxState::Released { receipt } => Some(("released", receipt.clone())),
            _ => None,
        };
        let mut queued = self
            .outbox
            .signed_request(&request.request_id, request_digest)?;
        let envelope = serde_json::from_slice::<QueuedReservationEnvelope>(&queued)
            .map_err(|_| "queued RFQ envelope is not canonical JSON".to_string());
        queued.fill(0);
        let mut signed_mandate = envelope?.signed_taker_mandate;
        let mandate = decode_taker_mandate(&signed_mandate);
        signed_mandate.fill(0);
        let mandate = mandate?;
        if request.request_id != hex::encode(mandate.rfq_nullifier) {
            return Err("outbox request id differs from its signed RFQ nullifier".into());
        }

        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let reservation = match rpc.note_reservation_snapshot(mandate.reserve_id) {
            Ok(reservation) => reservation,
            Err(error) if error.contains("not found") => {
                let status = if aborted_before_reserve {
                    "aborted_before_reserve"
                } else {
                    "not_reserved"
                };
                return Ok(json!({
                    "version": 1,
                    "participant_id": hex::encode(self.config.participant_id),
                    "request_id": request.request_id,
                    "request_digest": hex::encode(request_digest),
                    "status": status,
                    "hold_id": hex::encode(mandate.reserve_id),
                    "state_root": hex::encode(rpc.state_root()?),
                    "ledger_height": 0,
                    "queue_finalized": aborted_before_reserve,
                    "local_execution_fallback": false,
                }));
            }
            Err(error) => return Err(error),
        };
        if reservation.hold_id != mandate.reserve_id
            || reservation.asset_id != mandate.reserve_asset_id
            || reservation.amount_commitment != mandate.maximum_amount_commitment
            || reservation.reserve_receipt_digest == ZERO
        {
            return Err("canonical DeFMI reservation differs from the queued mandate".into());
        }
        if reservation.status == "active" {
            return Ok(json!({
                "version": 1,
                "participant_id": hex::encode(self.config.participant_id),
                "request_id": request.request_id,
                "request_digest": hex::encode(request_digest),
                "status": "active",
                "hold_id": hex::encode(mandate.reserve_id),
                "state_root": hex::encode(reservation.state_root),
                "ledger_height": reservation.accepted_height,
                "queue_finalized": false,
                "local_execution_fallback": false,
            }));
        }
        if reservation.settlement_digest == ZERO {
            return Err("terminal DeFMI reservation has no canonical transition digest".into());
        }
        // A snapshot reports the ledger's current height, not necessarily the
        // height at which this reservation became terminal.  Preserve the first
        // participant-owned canonical receipt on retries and compare only its
        // immutable transition identity.  This makes reconciliation genuinely
        // idempotent after later unrelated DeFMI blocks are accepted.
        if let Some((expected_status, receipt)) = prior_terminal {
            if reservation.status != expected_status
                || receipt.defmi_network_id != hex::encode(mandate.defmi_id)
                || receipt.transaction_id != hex::encode(reservation.settlement_digest)
                || receipt.request_digest != request_digest
            {
                return Err("stored corporate receipt differs from canonical DeFMI".into());
            }
            return Ok(json!({
                "version": 1,
                "participant_id": hex::encode(self.config.participant_id),
                "request_id": request.request_id,
                "request_digest": hex::encode(request_digest),
                "status": reservation.status,
                "hold_id": hex::encode(mandate.reserve_id),
                "state_root": hex::encode(reservation.state_root),
                "ledger_height": receipt.ledger_height,
                "canonical_receipt": receipt,
                "queue_finalized": true,
                "local_execution_fallback": false,
            }));
        }
        let finalized_at = unix_seconds()?;
        let receipt = CanonicalReceipt {
            defmi_network_id: hex::encode(mandate.defmi_id),
            transaction_id: hex::encode(reservation.settlement_digest),
            ledger_height: reservation.accepted_height,
            request_digest,
            finalized_at,
        };
        match reservation.status.as_str() {
            "consumed" => self.outbox.record_settlement(
                &request.request_id,
                request_digest,
                receipt.clone(),
            )?,
            "released" => {
                self.outbox
                    .record_release(&request.request_id, request_digest, receipt.clone())?
            }
            _ => return Err("canonical DeFMI returned an unknown reservation state".into()),
        }
        Ok(json!({
            "version": 1,
            "participant_id": hex::encode(self.config.participant_id),
            "request_id": request.request_id,
            "request_digest": hex::encode(request_digest),
            "status": reservation.status,
            "hold_id": hex::encode(mandate.reserve_id),
            "state_root": hex::encode(reservation.state_root),
            "ledger_height": reservation.accepted_height,
            "canonical_receipt": receipt,
            "queue_finalized": true,
            "local_execution_fallback": false,
        }))
    }

    fn sign(&self, operation: &str, request: SignRequest) -> Result<Value, String> {
        let mut body = BASE64
            .decode(request.body)
            .map_err(|_| "mandate body is not base64".to_string())?;
        if body.is_empty() || body.len() > MAX_SIGNED_BODY {
            body.fill(0);
            return Err("mandate body is empty or outside its bound".into());
        }
        let (purpose, domain) = match (self.config.role, operation) {
            (ParticipantRole::Maker, "policy-mandate") => ("quote_application", MAKER_DOMAIN),
            (ParticipantRole::Taker, "execution-mandate") => {
                ("settlement_application", TAKER_DOMAIN)
            }
            (_, "entity-approval") => {
                let purpose = request
                    .purpose
                    .as_deref()
                    .filter(|purpose| {
                        matches!(
                            *purpose,
                            "admin" | "settlement" | "quote" | "mpc_input" | "emergency"
                        )
                    })
                    .ok_or_else(|| {
                        "entity approval requires a supported purpose-specific key".to_string()
                    })?;
                (purpose, b"QOMM:DEFMI:ENTITY-SIGNATURE:v1".as_slice())
            }
            (_, "settlement") => {
                body.fill(0);
                return Err(
                    "post-match signatures are forbidden; DeFMI must execute the standing mandate"
                        .into(),
                );
            }
            _ => {
                body.fill(0);
                return Err("this participant role cannot sign that mandate".into());
            }
        };
        if !body.starts_with(domain) {
            body.fill(0);
            return Err("mandate body has the wrong canonical domain".into());
        }
        let now = unix_seconds()?;
        let key_id = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            state
                .key_ids
                .get(purpose)
                .cloned()
                .ok_or_else(|| format!("participant has no {purpose} key"))?
        };
        let key = self.store.private_key(&key_id, now, false)?;
        let (public_key, mut signature) = if operation == "entity-approval" {
            let signing = key
                .ed25519()
                .ok_or("entity approval classical key is not Ed25519")?;
            (
                signing.verifying_key().to_bytes(),
                signing.sign(&body).to_bytes().to_vec(),
            )
        } else {
            let signing = key
                .hybrid_signature()
                .ok_or("participant application key requires explicit hybrid enrollment")?;
            // A Maker mandate's digest includes its complete hybrid signature.
            // ML-DSA is randomized, so re-signing an identical policy after a
            // gateway/participant restart would name a different standing pool.
            // Persist the first signature, not the private key or the body, and
            // verify BOTH components before returning an idempotent retry.
            let signed = if operation == "policy-mandate" {
                let _guard = self
                    .state
                    .lock()
                    .map_err(|_| "participant state lock poisoned")?;
                let cache_id = Sha256::new()
                    .chain_update(b"QOMM:PARTICIPANT:POLICY-SIGNATURE:v1")
                    .chain_update((key_id.len() as u64).to_be_bytes())
                    .chain_update(key_id.as_bytes())
                    .chain_update(&body)
                    .finalize();
                let path = self
                    .config
                    .state_root
                    .join(format!("policy-signature-{}.bin", hex::encode(cache_id)));
                match fs::read(&path) {
                    Ok(bytes) => {
                        let signature = zkpi_committee::application_crypto::Signature::try_from(
                            bytes.as_slice(),
                        )
                        .map_err(|_| "stored Maker policy signature is malformed")?;
                        signing
                            .verifying_key()
                            .verify(&body, &signature)
                            .map_err(|_| "stored Maker policy signature does not verify")?;
                        signature
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let signature = signing.try_sign(&body)?;
                        atomic_private_write(&path, &signature.to_bytes())?;
                        fs::File::open(&self.config.state_root)
                            .and_then(|directory| directory.sync_all())
                            .map_err(|error| error.to_string())?;
                        signature
                    }
                    Err(error) => return Err(error.to_string()),
                }
            } else {
                signing.try_sign(&body)?
            };
            (signing.verifying_key().to_bytes(), signed.to_bytes())
        };
        if operation == "entity-approval" {
            use zkfmi_crypto::traits::Signer as _;
            let pq_id = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?
                .key_ids
                .get(&format!("{purpose}_pq"))
                .cloned()
                .ok_or("participant has no independently enrolled PQ purpose key")?;
            let pq = self.store.private_key(&pq_id, now, false)?;
            signature.extend(
                pq.ml_dsa65()
                    .ok_or("participant PQ purpose key has wrong suite")?
                    .sign(zkfmi_crypto::key::KeyPurpose::Attestation, &body)
                    .map_err(|error| error.to_string())?,
            );
        }
        let digest = hex::encode(Sha256::digest(&body));
        body.fill(0);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "participant state lock poisoned")?;
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| "participant sequence overflow".to_string())?;
        atomic_private_write(
            &self.state_path,
            &serde_json::to_vec_pretty(&*state).map_err(|error| error.to_string())?,
        )?;
        Ok(json!({
            "version": 1,
            "participant_id": state.participant_id,
            "role": state.role,
            "operation": operation,
            "key_id": key_id,
            "public_key": hex::encode(public_key),
            "body_sha256": digest,
            "signature": hex::encode(signature),
            "sequence": state.sequence,
            "post_match_signature": false,
        }))
    }

    fn present_kyb(&self, request: KybPresentRequest) -> Result<Value, String> {
        let trusted = zkpi_proofs::kyb::KybIssuerKey::from_bytes(
            &hex::decode(&request.trusted_issuer)
                .map_err(|_| "malformed hybrid issuer key".to_string())?,
        )
        .map_err(|_| "trusted KYB issuer is not a hybrid key".to_string())?;
        let registry = request.registry.into_registry()?;
        let now = unix_seconds()?;
        verify_registry(&registry, &trusted, now)
            .map_err(|error| format!("KYB registry was rejected: {error:?}"))?;
        if registry.cohort != request.required_cohort {
            return Err("KYB registry does not satisfy the requested cohort".into());
        }
        let scope = bounded_hex(&request.scope, "KYB scope", 256)?;
        let context = bounded_hex(&request.context, "KYB context", 512)?;
        let (key_id, role, participant_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (
                state
                    .key_ids
                    .get(KYB_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no anonymous KYB credential".to_string())?,
                state.role,
                state.participant_id.clone(),
            )
        };
        let attributes = match role {
            ParticipantRole::Maker | ParticipantRole::Taker => BusinessAttributes {
                jurisdiction: "JP".into(),
                entity_type: "regulated-dealer".into(),
                collateral_tier: 3,
            },
            ParticipantRole::MpcOperator => BusinessAttributes {
                jurisdiction: "JP".into(),
                entity_type: "regulated-infrastructure".into(),
                collateral_tier: 3,
            },
        };
        let stored = self.store.private_key(&key_id, now, false)?;
        let secret = stored
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant KYB credential is not a Ristretto scalar".to_string())?;
        let credential =
            KybCredential::from_secret_scalar(&participant_id, attributes, KYB_MAX_TIER, secret)
                .map_err(str::to_string)?;
        if !registry.points.contains(&credential.public_point) {
            return Err("participant KYB credential is absent from the signed cohort".into());
        }
        let presentation = present(&credential, &registry, &scope, &context, &mut OsRng)
            .map_err(str::to_string)?;
        verify_presentation(
            &presentation,
            &registry,
            &trusted,
            &scope,
            &context,
            now,
            &request.required_cohort,
        )
        .map_err(|error| format!("generated KYB presentation was rejected: {error:?}"))?;
        Ok(json!({
            "version": 1,
            "participant_id": participant_id,
            "presentation": KybPresentationWire::from_presentation(&presentation),
            "entity_commitment": hex::encode(presentation.entity_commitment()),
            "presentation_digest": hex::encode(presentation.digest()),
            "raw_legal_entity_id_disclosed": false,
        }))
    }

    /// Spend one canonical Maker-owned source note into a policy-locked parent
    /// pool without exporting the participant's view key, spend key, amount or
    /// blinding. The response contains the complete public proof so DeFMI's
    /// governance path can independently verify it before signing.
    fn standing_pool_spend(&self, request: StandingPoolSpendRequest) -> Result<Value, String> {
        if self.config.role != ParticipantRole::Maker {
            return Err("only a Maker participant can create a standing pool".into());
        }
        let asset_id = fixed_hex(&request.asset_id, "standing-pool asset")?;
        let source_note_id = fixed_hex(&request.source_note_id, "standing-pool source note")?;
        let pool_id = fixed_hex(&request.pool_id, "standing-pool id")?;
        let maximum_amount_commitment = fixed_hex(
            &request.maximum_amount_commitment,
            "standing-pool maximum amount commitment",
        )?;
        if [asset_id, source_note_id, pool_id, maximum_amount_commitment].contains(&ZERO) {
            return Err("standing-pool request contains an empty identifier".into());
        }
        let (view_key_id, spend_key_id, participant_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (
                state
                    .key_ids
                    .get(NOTE_VIEW_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note view key".to_string())?,
                state
                    .key_ids
                    .get(NOTE_SPEND_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note spend key".to_string())?,
                state.participant_id.clone(),
            )
        };
        let now = unix_seconds()?;
        let view = self
            .store
            .private_key(&view_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note view key is not Ristretto".to_string())?;
        let spend = self
            .store
            .private_key(&spend_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note spend key is not Ristretto".to_string())?;
        let wallet = Wallet::from_parts(view, spend, self.note_opening_key()?);
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let key = Pedersen::new(b"qomm:defmi:v1");
        let mut ledger = NoteLedger::new(key.clone(), 64);
        let mut canonical = Vec::new();
        let mut root = None;
        let mut after = None;
        loop {
            let page = rpc.note_page(asset_id, after, 256)?;
            match root {
                None => root = Some(page.state_root),
                Some(expected) if expected != page.state_root => {
                    return Err(
                        "DeFMI note pool changed while the Maker was proving ownership".into(),
                    )
                }
                Some(_) => {}
            }
            for output in page.notes {
                ledger.add(output.to_note()?);
                canonical.push(output);
            }
            match page.next {
                Some(next) if after != Some(next) => after = Some(next),
                Some(_) => return Err("DeFMI note pagination cursor did not advance".into()),
                None => break,
            }
            if canonical.len() > 16_384 {
                return Err("DeFMI note pool exceeds the participant proof bound".into());
            }
        }
        let state_root = root.ok_or_else(|| "DeFMI returned no note state root".to_string())?;
        if rpc.state_root()? != state_root {
            return Err("DeFMI note pool changed before the Maker proof was sealed".into());
        }
        let source_index = canonical
            .iter()
            .position(|output| output.note_id == source_note_id)
            .ok_or_else(|| "Maker source note is absent from DeFMI".to_string())?;
        if canonical[source_index].lock_id != ZERO
            || canonical[source_index].value_commitment != maximum_amount_commitment
        {
            return Err("Maker source note is locked or differs from the signed maximum".into());
        }
        let opening = ledger
            .scan(&wallet, &key)
            .into_iter()
            .find_map(|(index, opening)| (index == source_index).then_some(opening))
            .ok_or_else(|| "Maker cannot open the canonical source note".to_string())?;
        let decoy_index = canonical
            .iter()
            .enumerate()
            .find_map(|(index, output)| {
                (index != source_index && output.lock_id == ZERO).then_some(index)
            })
            .ok_or_else(|| "standing-pool proof has no unlocked decoy note".to_string())?;
        let mut ring = vec![source_index, decoy_index];
        ring.sort_by_key(|index| canonical[*index].note_id);
        let scalar = |label: &[u8]| {
            let mut value = Scalar::from_bytes_mod_order(
                Sha256::new()
                    .chain_update(b"QOMM:DEMO:STANDING-POOL-ADDRESS:v1")
                    .chain_update(label)
                    .chain_update(pool_id)
                    .finalize()
                    .into(),
            );
            if value == Scalar::ZERO {
                value = Scalar::ONE;
            }
            value
        };
        let covenant = Address {
            view: G * scalar(b"view"),
            spend: G * scalar(b"spend"),
            opening_public: wallet.address.opening_public,
        };
        let change_blinding = Scalar::random(&mut OsRng);
        let context = [b"QOMM:DEMO:STANDING-POOL-SPEND:v1".as_slice(), &pool_id].concat();
        let generated = ledger
            .build_spend_constrained_with_blindings(
                &ring,
                source_index,
                &opening,
                &key.g,
                &Scalar::ZERO,
                &[(covenant, opening.value), (wallet.address, 0)],
                &[opening.blinding, change_blinding],
                &[true, true],
                &context,
                &mut OsRng,
            )
            .map_err(str::to_string)?;
        let output_locks = [pool_id, ZERO];
        let projected = NoteSpend::from_verified(
            &ledger,
            &ring,
            &generated.proof,
            &generated.notes,
            asset_id,
            &vec![ZERO; ring.len()],
            ZERO,
            &output_locks,
            &context,
            &mut OsRng,
        )?;
        if projected.outputs[0].value_commitment != maximum_amount_commitment
            || projected.outputs[0].lock_id != pool_id
        {
            return Err("Maker proof did not create the signed parent commitment".into());
        }
        Ok(json!({
            "version": 1,
            "participant_id": participant_id,
            "state_root": hex::encode(state_root),
            "asset_id": hex::encode(asset_id),
            "ring": projected.ring.iter().map(hex::encode).collect::<Vec<_>>(),
            "proof": BASE64.encode(encode_spend_proof(&generated.proof)?),
            "proof_digest": hex::encode(generated.proof.digest()),
            "outputs": projected.outputs.iter().map(NoteOutput::body).collect::<Result<Vec<_>, _>>()?,
            "input_lock_id": hex::encode(ZERO),
            "context": hex::encode(context),
            "secrets_disclosed": false,
        }))
    }

    /// Merge several unlocked notes owned by this participant into one note
    /// without revealing which ring member was spent or any value opening.
    /// Every source has its own verified spend proof; DeFMI inserts only the
    /// public commitment-sum output in the same atomic transaction.
    fn note_consolidation(&self, request: NoteConsolidationRequest) -> Result<Value, String> {
        if !matches!(
            self.config.role,
            ParticipantRole::Maker | ParticipantRole::Taker
        ) {
            return Err("only a corporate participant can consolidate notes".into());
        }
        let asset_id = fixed_hex(&request.asset_id, "consolidation asset")?;
        let consolidation_id = fixed_hex(&request.consolidation_id, "consolidation id")?;
        if asset_id == ZERO || consolidation_id == ZERO || request.minimum_amount == 0 {
            return Err("note consolidation contains an empty asset, id, or amount".into());
        }
        let (view_key_id, spend_key_id, participant_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (
                state
                    .key_ids
                    .get(NOTE_VIEW_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note view key".to_string())?,
                state
                    .key_ids
                    .get(NOTE_SPEND_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note spend key".to_string())?,
                state.participant_id.clone(),
            )
        };
        let now = unix_seconds()?;
        let view = self
            .store
            .private_key(&view_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note view key is not Ristretto".to_string())?;
        let spend = self
            .store
            .private_key(&spend_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note spend key is not Ristretto".to_string())?;
        let wallet = Wallet::from_parts(view, spend, self.note_opening_key()?);
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let key = Pedersen::new(b"qomm:defmi:v1");
        let mut ledger = NoteLedger::new(key.clone(), 64);
        let mut canonical = Vec::new();
        let mut root = None;
        let mut after = None;
        loop {
            let page = rpc.note_page(asset_id, after, 256)?;
            match root {
                None => root = Some(page.state_root),
                Some(expected) if expected != page.state_root => {
                    return Err("DeFMI note pool changed during consolidation selection".into())
                }
                Some(_) => {}
            }
            for output in page.notes {
                ledger.add(output.to_note()?);
                canonical.push(output);
            }
            match page.next {
                Some(next) if after != Some(next) => after = Some(next),
                Some(_) => return Err("DeFMI note pagination cursor did not advance".into()),
                None => break,
            }
            if canonical.len() > 16_384 {
                return Err("DeFMI note pool exceeds the participant proof bound".into());
            }
        }
        let state_root = root.ok_or_else(|| "DeFMI returned no note state root".to_string())?;
        let mut owned = Vec::new();
        for (index, opening) in ledger.scan(&wallet, &key) {
            if !canonical
                .get(index)
                .is_some_and(|output| output.lock_id == ZERO)
            {
                continue;
            }
            let serial = defmi::notes::note_nullifier(&opening.serial)
                .compress()
                .to_bytes();
            let status = rpc.note_serial_snapshot(serial)?;
            if status.state_root != state_root {
                return Err("DeFMI changed while checking consolidation inputs".into());
            }
            if !status.spent {
                owned.push((index, opening));
            }
        }
        if owned
            .iter()
            .any(|(_, opening)| opening.value >= request.minimum_amount)
        {
            return Ok(json!({
                "version": 1,
                "participant_id": participant_id,
                "state_root": hex::encode(state_root),
                "asset_id": hex::encode(asset_id),
                "consolidation_id": hex::encode(consolidation_id),
                "needed": false,
                "secrets_disclosed": false,
            }));
        }
        owned.sort_by_key(|(index, opening)| {
            (std::cmp::Reverse(opening.value), canonical[*index].note_id)
        });
        let mut selected = Vec::new();
        let mut total = 0_u64;
        for item in owned {
            total = total
                .checked_add(item.1.value)
                .ok_or_else(|| "participant note total overflowed".to_string())?;
            selected.push(item);
            if total >= request.minimum_amount {
                break;
            }
            if selected.len() == 8 {
                break;
            }
        }
        if selected.len() < 2 || total < request.minimum_amount {
            return Err("participant total unspent notes are below the requested reserve".into());
        }
        let mut spend_values = Vec::with_capacity(selected.len());
        let mut merged_commitment = RistrettoPoint::identity();
        let mut merged_blinding = Scalar::ZERO;
        for (lane, (source_index, opening)) in selected.into_iter().enumerate() {
            let decoy_index = canonical
                .iter()
                .enumerate()
                .find_map(|(index, output)| {
                    (index != source_index && output.lock_id == ZERO).then_some(index)
                })
                .ok_or_else(|| "note consolidation has no unlocked anonymity decoy".to_string())?;
            let mut ring = vec![source_index, decoy_index];
            ring.sort_by_key(|index| canonical[*index].note_id);
            let context = [
                b"QOMM:DEMO:NOTE-CONSOLIDATION-SPEND:v1".as_slice(),
                &consolidation_id,
                &(lane as u64).to_be_bytes(),
            ]
            .concat();
            let eligibility = vec![true; ring.len()];
            let generated = ledger
                .build_spend_constrained_with_blindings(
                    &ring,
                    source_index,
                    &opening,
                    &key.g,
                    &Scalar::ZERO,
                    &[(wallet.address, opening.value)],
                    &[opening.blinding],
                    &eligibility,
                    &context,
                    &mut OsRng,
                )
                .map_err(str::to_string)?;
            let projected = NoteSpend::from_verified(
                &ledger,
                &ring,
                &generated.proof,
                &generated.notes,
                asset_id,
                &vec![ZERO; ring.len()],
                ZERO,
                &[ZERO],
                &context,
                &mut OsRng,
            )?;
            merged_commitment += projected.outputs[0].to_note()?.value_commitment;
            merged_blinding += opening.blinding;
            spend_values.push(json!({
                "ring": projected.ring.iter().map(hex::encode).collect::<Vec<_>>(),
                "proof": BASE64.encode(encode_spend_proof(&generated.proof)?),
                "proof_digest": hex::encode(generated.proof.digest()),
                "outputs": projected.outputs.iter().map(NoteOutput::body).collect::<Result<Vec<_>, _>>()?,
                "context": hex::encode(context),
            }));
        }
        let merged_note = ledger.build_note(
            &wallet.address,
            total,
            merged_commitment,
            &merged_blinding,
            &mut OsRng,
        )?;
        let consolidated_output = NoteOutput::from_note(&merged_note, asset_id, ZERO)?;
        if rpc.state_root()? != state_root {
            return Err("DeFMI changed before consolidation proofs were sealed".into());
        }
        Ok(json!({
            "version": 1,
            "participant_id": participant_id,
            "state_root": hex::encode(state_root),
            "asset_id": hex::encode(asset_id),
            "consolidation_id": hex::encode(consolidation_id),
            "needed": true,
            "spends": spend_values,
            "consolidated_output": consolidated_output.body()?,
            "secrets_disclosed": false,
        }))
    }

    /// Spend an unspent corporate note into one exact RFQ covenant. The
    /// participant, not the coordinator, selects the source note and checks its
    /// serial against canonical DeFMI state. Private note keys and the source
    /// opening never leave this service.
    fn note_reservation_spend(
        &self,
        request: NoteReservationSpendRequest,
    ) -> Result<Value, String> {
        if !matches!(
            self.config.role,
            ParticipantRole::Maker | ParticipantRole::Taker
        ) {
            return Err("only a corporate participant can reserve a note".into());
        }
        let asset_id = fixed_hex(&request.asset_id, "reservation asset")?;
        let reserve_id = fixed_hex(&request.reserve_id, "reservation id")?;
        let amount_commitment = fixed_hex(&request.amount_commitment, "reservation amount")?;
        if asset_id == ZERO || reserve_id == ZERO || request.amount == 0 {
            return Err("note reservation contains an empty asset, id, or amount".into());
        }
        let key = Pedersen::new(b"qomm:defmi:v1");
        let amount_blinding = Scalar::from(request.amount_blinding);
        if key
            .commit_u64(request.amount, &amount_blinding)
            .compress()
            .to_bytes()
            != amount_commitment
        {
            return Err("note reservation opening differs from its signed commitment".into());
        }
        let (view_key_id, spend_key_id, participant_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "participant state lock poisoned")?;
            (
                state
                    .key_ids
                    .get(NOTE_VIEW_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note view key".to_string())?,
                state
                    .key_ids
                    .get(NOTE_SPEND_KEY_PURPOSE)
                    .cloned()
                    .ok_or_else(|| "participant has no note spend key".to_string())?,
                state.participant_id.clone(),
            )
        };
        let now = unix_seconds()?;
        let view = self
            .store
            .private_key(&view_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note view key is not Ristretto".to_string())?;
        let spend = self
            .store
            .private_key(&spend_key_id, now, false)?
            .ristretto_scalar()
            .copied()
            .ok_or_else(|| "participant note spend key is not Ristretto".to_string())?;
        let wallet = Wallet::from_parts(view, spend, self.note_opening_key()?);
        let rpc = docker_rpc_client(&self.config.defmi_endpoint, Duration::from_secs(30))?;
        let mut ledger = NoteLedger::new(key.clone(), 64);
        let mut canonical = Vec::new();
        let mut root = None;
        let mut after = None;
        loop {
            let page = rpc.note_page(asset_id, after, 256)?;
            match root {
                None => root = Some(page.state_root),
                Some(expected) if expected != page.state_root => {
                    return Err("DeFMI note pool changed while selecting a reserve".into())
                }
                Some(_) => {}
            }
            for output in page.notes {
                ledger.add(output.to_note()?);
                canonical.push(output);
            }
            match page.next {
                Some(next) if after != Some(next) => after = Some(next),
                Some(_) => return Err("DeFMI note pagination cursor did not advance".into()),
                None => break,
            }
            if canonical.len() > 16_384 {
                return Err("DeFMI note pool exceeds the participant proof bound".into());
            }
        }
        let state_root = root.ok_or_else(|| "DeFMI returned no note state root".to_string())?;
        let mut owned = ledger
            .scan(&wallet, &key)
            .into_iter()
            .filter(|(index, opening)| {
                canonical
                    .get(*index)
                    .is_some_and(|output| output.lock_id == ZERO)
                    && opening.value >= request.amount
            })
            .collect::<Vec<_>>();
        owned.sort_by_key(|(index, opening)| (opening.value, canonical[*index].note_id));
        let mut source = None;
        for (index, opening) in owned {
            let serial_point = defmi::notes::note_nullifier(&opening.serial)
                .compress()
                .to_bytes();
            let status = rpc.note_serial_snapshot(serial_point)?;
            if status.state_root != state_root {
                return Err("DeFMI changed while checking the source-note serial".into());
            }
            if !status.spent {
                source = Some((index, opening));
                break;
            }
        }
        let (source_index, source_opening) = source.ok_or_else(|| {
            "participant has no unspent note large enough for the reserve".to_string()
        })?;
        let decoy_index = canonical
            .iter()
            .enumerate()
            .find_map(|(index, output)| {
                (index != source_index && output.lock_id == ZERO).then_some(index)
            })
            .ok_or_else(|| "note reservation has no unlocked anonymity decoy".to_string())?;
        let mut ring = vec![source_index, decoy_index];
        ring.sort_by_key(|index| canonical[*index].note_id);
        let covenant_scalar = |label: &[u8]| {
            let mut value = Scalar::from_bytes_mod_order(
                Sha256::new()
                    .chain_update(b"QOMM:DEMO:NOTE-RESERVATION-ADDRESS:v1")
                    .chain_update(label)
                    .chain_update(reserve_id)
                    .finalize()
                    .into(),
            );
            if value == Scalar::ZERO {
                value = Scalar::ONE;
            }
            value
        };
        let covenant = Address {
            view: G * covenant_scalar(b"view"),
            spend: G * covenant_scalar(b"spend"),
            opening_public: wallet.address.opening_public,
        };
        let change = source_opening
            .value
            .checked_sub(request.amount)
            .ok_or_else(|| "source note is smaller than the requested reserve".to_string())?;
        let change_blinding = Scalar::random(&mut OsRng);
        let context = [
            b"QOMM:DEMO:NOTE-RESERVATION-SPEND:v1".as_slice(),
            &reserve_id,
        ]
        .concat();
        let generated = ledger
            .build_spend_constrained_with_blindings(
                &ring,
                source_index,
                &source_opening,
                &key.g,
                &Scalar::ZERO,
                &[(covenant, request.amount), (wallet.address, change)],
                &[amount_blinding, change_blinding],
                &[true, true],
                &context,
                &mut OsRng,
            )
            .map_err(str::to_string)?;
        let projected = NoteSpend::from_verified(
            &ledger,
            &ring,
            &generated.proof,
            &generated.notes,
            asset_id,
            &vec![ZERO; ring.len()],
            ZERO,
            &[reserve_id, ZERO],
            &context,
            &mut OsRng,
        )?;
        if projected.outputs[0].value_commitment != amount_commitment
            || projected.outputs[0].lock_id != reserve_id
        {
            return Err("participant proof did not create the signed reserve covenant".into());
        }
        if rpc.state_root()? != state_root {
            return Err("DeFMI changed before the note-reservation proof was sealed".into());
        }
        Ok(json!({
            "version": 1,
            "participant_id": participant_id,
            "state_root": hex::encode(state_root),
            "asset_id": hex::encode(asset_id),
            "reserve_id": hex::encode(reserve_id),
            "ring": projected.ring.iter().map(hex::encode).collect::<Vec<_>>(),
            "proof": BASE64.encode(encode_spend_proof(&generated.proof)?),
            "proof_digest": hex::encode(generated.proof.digest()),
            "outputs": projected.outputs.iter().map(NoteOutput::body).collect::<Result<Vec<_>, _>>()?,
            "input_lock_id": hex::encode(ZERO),
            "context": hex::encode(context),
            "secrets_disclosed": false,
        }))
    }
}

pub fn serve_participant_node(config: ParticipantNodeConfig) -> Result<(), String> {
    let service = Arc::new(ParticipantService::initialize(config)?);
    if service.config.role == ParticipantRole::Taker {
        let reconciliation_service = Arc::clone(&service);
        thread::spawn(move || loop {
            match reconciliation_service.sweep_expired_unreserved() {
                Ok(finalized) if finalized > 0 => eprintln!(
                    "participant finalized {finalized} expired RFQ(s) that never reached DeFMI"
                ),
                Ok(_) => {}
                Err(error) => eprintln!("participant expiry reconciliation failed: {error}"),
            }
            thread::sleep(Duration::from_secs(30));
        });
    }
    let listener = TcpListener::bind((service.config.listen_host.as_str(), service.config.port))
        .map_err(|error| error.to_string())?;
    println!(
        "QOMM {} participant {} listening at {}",
        service.config.role.as_str(),
        service.config.label,
        listener.local_addr().map_err(|error| error.to_string())?
    );
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let service = Arc::clone(&service);
                thread::spawn(move || {
                    if let Err(error) = handle(stream, &service) {
                        eprintln!("participant request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("participant accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, service: &ParticipantService) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(15))))
        .map_err(|error| error.to_string())?;
    let (method, path, mut body) = read_request(&mut stream)?;
    let response = match (method.as_str(), path.as_str()) {
        ("GET", "/health") | ("GET", "/v1/snapshot") => {
            json_response("200 OK", &service.snapshot()?)
        }
        ("POST", path) if path.starts_with("/v1/sign/") => {
            let operation = path.trim_start_matches("/v1/sign/");
            let request = serde_json::from_slice::<SignRequest>(&body)
                .map_err(|_| "participant sign request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.sign(operation, request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/kyb/present") => {
            let request = serde_json::from_slice::<KybPresentRequest>(&body)
                .map_err(|_| "participant KYB request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.present_kyb(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/notes/standing-pool-spend") => {
            let request = serde_json::from_slice::<StandingPoolSpendRequest>(&body)
                .map_err(|_| "standing-pool spend request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.standing_pool_spend(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/notes/consolidation") => {
            let request = serde_json::from_slice::<NoteConsolidationRequest>(&body)
                .map_err(|_| "note-consolidation request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.note_consolidation(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/notes/reservation-spend") => {
            let request = serde_json::from_slice::<NoteReservationSpendRequest>(&body)
                .map_err(|_| "note-reservation spend request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.note_reservation_spend(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/facility/hold-evidence") => {
            let request = serde_json::from_slice::<FacilityHoldEvidenceRequest>(&body)
                .map_err(|_| "facility-hold request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.facility_hold_evidence(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/claims/materializations") => {
            let request = serde_json::from_slice::<ClaimMaterializationRequest>(&body)
                .map_err(|_| "claim-materialization request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.claim_materializations(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/portfolio/canonical") => {
            let request = serde_json::from_slice::<CanonicalPortfolioRequest>(&body)
                .map_err(|_| "canonical-portfolio request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.canonical_taker_portfolio(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/outbox/requests") => {
            let request = serde_json::from_slice::<OutboxEnqueueRequest>(&body)
                .map_err(|_| "outbox request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.enqueue_outbox(request)) {
                Ok(value) => json_response("202 Accepted", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/outbox/claim") => {
            let request = serde_json::from_slice::<OutboxClaimRequest>(&body)
                .map_err(|_| "outbox claim request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.claim_outbox(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/outbox/mpc-admission") => {
            let request = serde_json::from_slice::<OutboxAdmissionRequest>(&body)
                .map_err(|_| "outbox admission request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.record_outbox_admission(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/outbox/abort-before-reserve") => {
            let request = serde_json::from_slice::<OutboxPreReserveAbortRequest>(&body)
                .map_err(|_| "outbox abort request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.abort_outbox_before_reserve(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        ("POST", "/v1/outbox/reconcile") => {
            let request = serde_json::from_slice::<OutboxReconcileRequest>(&body)
                .map_err(|_| "outbox reconciliation request is not canonical JSON".to_string());
            body.fill(0);
            match request.and_then(|request| service.reconcile_outbox(request)) {
                Ok(value) => json_response("200 OK", &value),
                Err(error) => json_response("409 Conflict", &json!({"error": error})),
            }
        }
        _ => json_response(
            "404 Not Found",
            &json!({"error":"no such participant operation"}),
        ),
    };
    stream
        .write_all(&response)
        .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
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
    let request = lines
        .next()
        .ok_or_else(|| "HTTP request line is absent".to_string())?;
    let mut fields = request.split_whitespace();
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

fn json_response(status: &str, value: &Value) -> Vec<u8> {
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
            .unwrap_or("participant"),
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

fn unix_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "system clock is before Unix epoch".to_string())
}

fn hash_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}

fn deterministic_scalar(parts: &[&[u8]]) -> Scalar {
    let mut value = Scalar::from_bytes_mod_order(hash_parts(parts));
    if value == Scalar::ZERO {
        value = Scalar::ONE;
    }
    value
}

fn participant_handle_scalar(role: &[u8], participant_id: &[u8; 32]) -> u64 {
    let digest = Sha256::new()
        .chain_update(b"QOMM:DEMO:PARTICIPANT-HANDLE:v1")
        .chain_update(role)
        .chain_update(participant_id)
        .finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix")).max(1)
}

fn scalar_to_u64(value: Scalar) -> Result<u64, String> {
    let bytes = value.to_bytes();
    if bytes[8..].iter().any(|byte| *byte != 0) {
        return Err("decrypted claim opening is outside the u64 amount range".into());
    }
    Ok(u64::from_le_bytes(
        bytes[..8]
            .try_into()
            .expect("checked eight-byte scalar prefix"),
    ))
}

fn canonical_claim_amount(
    claim: &CanonicalNoteClaim,
    recipient_secret: &Scalar,
    recipient_key: &zkfmi_crypto::hybrid::kem::HybridKemKey,
    key: &Pedersen,
) -> Result<u64, String> {
    claim.claim()?;
    if claim.opening_envelope.recipient_view != G * recipient_secret {
        return Err("canonical claim opening belongs to another recipient".into());
    }
    let quorum = claim
        .opening_envelope
        .shares
        .iter()
        .take(claim.opening_envelope.threshold)
        .map(|share| share.party)
        .collect::<Vec<_>>();
    let (amount, blinding) = claim.opening_envelope.decrypt_u64(
        recipient_secret,
        recipient_key,
        &quorum,
        PRODUCT_DVP_REMAINDER_BITS,
    )?;
    if key.commit_u64(amount, &blinding).compress().to_bytes() != claim.value_commitment {
        return Err("canonical claim opening differs from its commitment".into());
    }
    Ok(amount)
}

fn claim_materialization_json(
    materialization: &NoteClaimMaterialization,
    proof: &ClaimOwnershipProof,
    rfq_nullifier: [u8; 32],
    recipient_handle: [u8; 32],
    destination: &Address,
) -> Result<Value, String> {
    Ok(json!({
        "rfq_nullifier": hex::encode(rfq_nullifier),
        "recipient_handle": hex::encode(recipient_handle),
        "destination": {
            "view": hex::encode(destination.view.compress().to_bytes()),
            "spend": hex::encode(destination.spend.compress().to_bytes()),
            "opening": hex::encode(destination.opening_public),
        },
        "materialization": materialization.body()?,
        "ownership_proof": {
            "nonce": hex::encode(proof.nonce.compress().to_bytes()),
            "response": hex::encode(proof.response.to_bytes()),
        },
    }))
}

fn fixed_hex<const N: usize>(value: &str, name: &str) -> Result<[u8; N], String> {
    hex::decode(value)
        .map_err(|_| format!("{name} must be {N}-byte hexadecimal"))?
        .try_into()
        .map_err(|_| format!("{name} must be {N}-byte hexadecimal"))
}

fn bounded_hex(value: &str, name: &str, maximum: usize) -> Result<Vec<u8>, String> {
    let decoded = hex::decode(value).map_err(|_| format!("{name} is not hexadecimal"))?;
    if decoded.is_empty() || decoded.len() > maximum {
        return Err(format!("{name} is empty or exceeds {maximum} bytes"));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use zkpi_committee::mandate::{encode_taker_mandate, TakerExecutionMandate};

    fn config(role: ParticipantRole, root: &Path) -> ParticipantNodeConfig {
        ParticipantNodeConfig {
            role,
            label: role.as_str().into(),
            participant_id: Sha256::digest(role.as_str().as_bytes()).into(),
            listen_host: "127.0.0.1".into(),
            port: 9000,
            state_root: root.to_path_buf(),
            initial_cash: 1_000_000,
            initial_inventory: 1_000,
            defmi_endpoint: "http://defmi-network:9650/rpc".into(),
        }
    }

    fn queued_request(
        service: &ParticipantService,
        amount: u64,
        nonce: u8,
        expires_at: u64,
    ) -> (String, String) {
        let now = unix_seconds().unwrap();
        let key_id = service.state.lock().unwrap().key_ids["settlement_application"].clone();
        let stored = service.store.private_key(&key_id, now, false).unwrap();
        let signing = stored.hybrid_signature().unwrap();
        let maximum_blinding = u64::from(nonce) + 70;
        let nonzero = |label: u8| {
            Sha256::new()
                .chain_update(b"QOMM:TEST:QUEUED-RFQ:v1")
                .chain_update([nonce, label])
                .finalize()
                .into()
        };
        let rfq_nullifier = nonzero(1);
        let mandate = TakerExecutionMandate {
            venue_id: nonzero(2),
            defmi_id: nonzero(3),
            rfq_nullifier,
            asset_id: nonzero(4),
            reserve_asset_id: nonzero(5),
            direction: Direction::TakerBuys,
            quantity_commitment: nonzero(6),
            limit_price_commitment: nonzero(7),
            maximum_fee_commitment: nonzero(8),
            maximum_amount_commitment: Pedersen::new(b"qomm:defmi:v1")
                .commit_u64(amount, &Scalar::from(maximum_blinding))
                .compress()
                .to_bytes(),
            reserve_id: nonzero(9),
            taker_handle: (G * Scalar::from(77_u64)).compress().to_bytes(),
            entity_commitment: nonzero(10),
            kyb_presentation_digest: nonzero(11),
            admission_ticket_id: nonzero(12),
            admission_slot: u64::from(nonce),
            fill_mask_commitment: nonzero(13),
            deadline: expires_at,
            allow_partial: false,
            auto_settle: true,
            taker_public: signing.verifying_key().to_bytes(),
            signature: zkpi_committee::application_crypto::Signature::from_bytes(&[]),
        }
        .sign(signing)
        .unwrap();
        let envelope = serde_json::to_vec(&json!({
            "maximum_amount": amount,
            "maximum_blinding": maximum_blinding,
            "signed_taker_mandate": encode_taker_mandate(&mandate).unwrap(),
        }))
        .unwrap();
        (hex::encode(rfq_nullifier), BASE64.encode(envelope))
    }

    #[test]
    fn mpc_operator_role_uses_the_network_contract_and_reads_legacy_state() {
        assert_eq!(
            serde_json::to_string(&ParticipantRole::MpcOperator).unwrap(),
            "\"mpc_operator\""
        );
        assert_eq!(
            serde_json::from_str::<ParticipantRole>("\"mpcoperator\"").unwrap(),
            ParticipantRole::MpcOperator
        );
    }

    #[test]
    fn reservation_and_facility_finality_uses_status_specific_statements() {
        let order_statement = [31_u8; 32];
        let settlement_statement = [32_u8; 32];

        assert!(reservation_and_facility_finality_agree(
            "active", ZERO, "active", ZERO,
        ));
        assert!(reservation_and_facility_finality_agree(
            "released",
            order_statement,
            "released",
            ZERO,
        ));
        assert!(reservation_and_facility_finality_agree(
            "consumed",
            order_statement,
            "consumed",
            settlement_statement,
        ));

        assert!(!reservation_and_facility_finality_agree(
            "released",
            order_statement,
            "released",
            settlement_statement,
        ));
        assert!(!reservation_and_facility_finality_agree(
            "consumed",
            order_statement,
            "released",
            ZERO,
        ));
        assert!(!reservation_and_facility_finality_agree(
            "active",
            order_statement,
            "active",
            ZERO,
        ));
    }

    #[test]
    fn maker_policy_signature_is_identical_after_retry_and_participant_restart() {
        let root = tempdir().unwrap();
        let service =
            ParticipantService::initialize(config(ParticipantRole::Maker, root.path())).unwrap();
        let body = [MAKER_DOMAIN, b":idempotent-policy-test"].concat();
        let request = || SignRequest {
            body: BASE64.encode(&body),
            purpose: None,
        };
        let first = service.sign("policy-mandate", request()).unwrap();
        let second = service.sign("policy-mandate", request()).unwrap();
        assert_eq!(first["signature"], second["signature"]);
        drop(service);
        let reopened =
            ParticipantService::initialize(config(ParticipantRole::Maker, root.path())).unwrap();
        let after_restart = reopened.sign("policy-mandate", request()).unwrap();
        assert_eq!(first["signature"], after_restart["signature"]);
        let cache = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("policy-signature-")
            })
            .unwrap();
        let mut corrupted = fs::read(&cache).unwrap();
        *corrupted.last_mut().unwrap() ^= 1;
        fs::write(&cache, corrupted).unwrap();
        assert!(reopened
            .sign("policy-mandate", request())
            .unwrap_err()
            .contains("does not verify"));
        let changed = reopened
            .sign(
                "policy-mandate",
                SignRequest {
                    body: BASE64.encode([body.as_slice(), b":changed"].concat()),
                    purpose: None,
                },
            )
            .unwrap();
        assert_ne!(first["signature"], changed["signature"]);
    }

    #[test]
    fn role_keys_persist_and_post_match_signing_is_absent() {
        let root = tempdir().unwrap();
        let service =
            ParticipantService::initialize(config(ParticipantRole::Maker, root.path())).unwrap();
        let snapshot = service.snapshot().unwrap();
        let keys = service.store.snapshot().unwrap().keys;
        let actual = keys
            .iter()
            .map(|record| (record.purpose.as_str(), record.kind))
            .collect::<BTreeMap<_, _>>();
        let required = BTreeMap::from([
            ("admin", KeyKind::Ed25519),
            ("admin_pq", KeyKind::MlDsa65),
            ("settlement", KeyKind::Ed25519),
            ("settlement_pq", KeyKind::MlDsa65),
            ("quote", KeyKind::Ed25519),
            ("quote_pq", KeyKind::MlDsa65),
            ("mpc_input", KeyKind::Ed25519),
            ("mpc_input_pq", KeyKind::MlDsa65),
            ("emergency", KeyKind::Ed25519),
            ("emergency_pq", KeyKind::MlDsa65),
            ("quote_application", KeyKind::HybridSignature),
            ("settlement_application", KeyKind::HybridSignature),
            (KYB_KEY_PURPOSE, KeyKind::Ristretto),
            (NOTE_VIEW_KEY_PURPOSE, KeyKind::Ristretto),
            (NOTE_SPEND_KEY_PURPOSE, KeyKind::Ristretto),
            (NOTE_OPENING_KEY_PURPOSE, KeyKind::HybridKem),
        ]);
        assert_eq!(actual, required);
        let application_key = |purpose: &str| {
            keys.iter()
                .find(|record| record.purpose == purpose)
                .unwrap()
        };
        assert_ne!(
            application_key("quote_application").key_id.as_str(),
            application_key("settlement_application").key_id.as_str()
        );
        assert_ne!(
            application_key("quote_application").public.as_str(),
            application_key("settlement_application").public.as_str()
        );
        assert!(snapshot["note_address"]["view"].as_str().is_some());
        assert!(snapshot["note_address"]["spend"].as_str().is_some());
        assert!(service
            .sign(
                "settlement",
                SignRequest {
                    body: BASE64.encode(b"anything"),
                    purpose: None,
                },
            )
            .unwrap_err()
            .contains("post-match"));
        let persisted_keys = snapshot["keys"]["keys"].clone();
        drop(service);
        let reopened =
            ParticipantService::initialize(config(ParticipantRole::Maker, root.path())).unwrap();
        assert_eq!(reopened.snapshot().unwrap()["keys"]["keys"], persisted_keys);
    }

    #[test]
    fn corporate_outbox_survives_participant_restart_and_rejects_operator_origin() {
        let root = tempdir().unwrap();
        let service =
            ParticipantService::initialize(config(ParticipantRole::Taker, root.path())).unwrap();
        let now = unix_seconds().unwrap();
        let expires_at = now + 300;
        let (request_id, signed_request) = queued_request(&service, 600_000, 1, expires_at);
        assert!(service
            .enqueue_outbox(OutboxEnqueueRequest {
                request_id: "00".repeat(32),
                signed_request: signed_request.clone(),
                expires_at,
            })
            .unwrap_err()
            .contains("differs from its signed Taker mandate"));
        let response = service
            .enqueue_outbox(OutboxEnqueueRequest {
                request_id: request_id.clone(),
                signed_request: signed_request.clone(),
                expires_at,
            })
            .unwrap();
        assert_eq!(response["status"], "enqueued");
        assert_eq!(response["local_execution_fallback"], false);
        drop(service);

        let reopened =
            ParticipantService::initialize(config(ParticipantRole::Taker, root.path())).unwrap();
        let snapshot = reopened.snapshot().unwrap();
        assert_eq!(snapshot["mpc_outbox"]["metrics"]["queued"], 1);
        assert_eq!(snapshot["mpc_outbox"]["durable"], true);
        assert_eq!(snapshot["portfolio"]["reserved_cash"], 600_000);
        let repeated = reopened
            .enqueue_outbox(OutboxEnqueueRequest {
                request_id: request_id.clone(),
                signed_request: signed_request.clone(),
                expires_at,
            })
            .unwrap();
        assert_eq!(repeated["status"], "already_present");
        let (excess_id, excess_request) = queued_request(&reopened, 500_000, 2, expires_at);
        assert!(reopened
            .enqueue_outbox(OutboxEnqueueRequest {
                request_id: excess_id,
                signed_request: excess_request,
                expires_at,
            })
            .unwrap_err()
            .contains("exceed"));
        let claimed = reopened
            .claim_outbox(OutboxClaimRequest {
                quorum_healthy: true,
                retry_after_seconds: 2,
                interval_seconds: 1,
            })
            .unwrap();
        assert_eq!(claimed["action"], "real");
        assert_eq!(claimed["signed_request"], signed_request);
        reopened
            .record_outbox_admission(OutboxAdmissionRequest {
                request_id,
                request_digest: claimed["request_digest"].as_str().unwrap().into(),
                committee_id: "committee-1".into(),
                job_id: "job-1".into(),
            })
            .unwrap();
        assert_eq!(
            reopened.snapshot().unwrap()["mpc_outbox"]["metrics"]["mpc_admitted"],
            1
        );

        let operator_root = tempdir().unwrap();
        let operator = ParticipantService::initialize(config(
            ParticipantRole::MpcOperator,
            operator_root.path(),
        ))
        .unwrap();
        assert!(operator
            .enqueue_outbox(OutboxEnqueueRequest {
                request_id: "not-a-corporate-request".into(),
                signed_request: BASE64.encode(b"signed-rfq-envelope"),
                expires_at: now + 300,
            })
            .is_err());
    }
}
