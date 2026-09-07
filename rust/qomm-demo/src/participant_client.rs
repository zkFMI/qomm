//! Authenticated client for one legal-entity participant module.
//!
//! The coordinator learns only public participant metadata and asks the
//! entity-owned service to sign a canonical pre-trade or registry body.  The
//! participant private key never enters the gateway container.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use defmi::facility::{
    CreditFacilityRelationProof, CreditFacilityTransition, CreditTransitionKind,
};
use defmi::note_chain::{ClaimOwnershipProof, NoteClaimMaterialization, NoteOutput};
use defmi::notes::{decode_spend_proof, Address, SpendProof};
use defmi::participant::{EntityApproval, KeyPurpose};
use qomm_proofs::kyb::{verify_presentation, KybPresentation, SignedCohortRegistry};
use qomm_transport::kyb_wire::{KybPresentationWire, KybRegistryWire};
use qomm_transport::mandate::{MakerPolicyMandate, TakerExecutionMandate};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

const MAX_HTTP_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParticipantSnapshot {
    pub participant_id: [u8; 32],
    pub role: String,
    pub label: String,
    pub sequence: u64,
    pub public_keys: BTreeMap<String, [u8; 32]>,
    pub pq_public_keys: BTreeMap<String, Vec<u8>>,
    pub quote_application_key: qomm_transport::application_crypto::VerifyingKey,
    pub settlement_application_key: qomm_transport::application_crypto::VerifyingKey,
    pub kyb_public_point: [u8; 32],
    pub note_view_public: [u8; 32],
    pub note_opening_public: [u8; zkfmi_crypto::sealed::RECIPIENT_PUBLIC_BYTES],
    pub note_spend_public: [u8; 32],
    pub cash: u64,
    pub inventory: u64,
    /// Next durable corporate-request sequence. This survives gateway restarts
    /// and is the canonical source for one-time RFQ/admission identifiers.
    pub corporate_outbox_next_sequence: u64,
}

#[derive(Clone, Debug)]
pub struct ParticipantClient {
    authority: String,
    timeout: Duration,
}

pub struct KybPresentationRequest<'a> {
    pub snapshot: &'a ParticipantSnapshot,
    pub registry: &'a SignedCohortRegistry,
    pub trusted_issuer: &'a qomm_proofs::kyb::KybIssuerKey,
    pub scope: &'a [u8],
    pub context: &'a [u8],
    pub required_cohort: &'a str,
    pub now: u64,
}

pub struct StandingPoolSpendEvidence {
    pub participant_id: [u8; 32],
    pub state_root: [u8; 32],
    pub asset_id: [u8; 32],
    pub ring: Vec<[u8; 32]>,
    pub proof: SpendProof,
    pub outputs: Vec<NoteOutput>,
    pub context: Vec<u8>,
}

pub struct NoteReservationSpendEvidence {
    pub participant_id: [u8; 32],
    pub state_root: [u8; 32],
    pub asset_id: [u8; 32],
    pub reserve_id: [u8; 32],
    pub ring: Vec<[u8; 32]>,
    pub proof: SpendProof,
    pub outputs: Vec<NoteOutput>,
    pub context: Vec<u8>,
}

pub struct NoteConsolidationSpendEvidence {
    pub ring: Vec<[u8; 32]>,
    pub proof: SpendProof,
    pub outputs: Vec<NoteOutput>,
    pub context: Vec<u8>,
}

pub struct NoteConsolidationEvidence {
    pub participant_id: [u8; 32],
    pub state_root: [u8; 32],
    pub asset_id: [u8; 32],
    pub consolidation_id: [u8; 32],
    pub spends: Vec<NoteConsolidationSpendEvidence>,
    pub consolidated_output: NoteOutput,
}

pub struct FacilityHoldEvidence {
    pub participant_id: [u8; 32],
    pub state_root: [u8; 32],
    pub transition: CreditFacilityTransition,
    pub relation_proof: CreditFacilityRelationProof,
}

pub struct ClaimMaterializationEvidence {
    pub rfq_nullifier: [u8; 32],
    pub recipient_handle: RistrettoPoint,
    pub destination: Address,
    pub materialization: NoteClaimMaterialization,
    pub ownership_proof: ClaimOwnershipProof,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalParticipantPortfolio {
    pub participant_id: [u8; 32],
    pub state_root: [u8; 32],
    pub cash: u64,
    pub inventory: BTreeMap<[u8; 32], u64>,
    pub consumed_settlements: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboxEnqueueReceipt {
    pub request_id: String,
    pub request_digest: [u8; 32],
    pub sequence: u64,
    pub already_present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedCorporateRequest {
    pub slot: u64,
    pub due_at: u64,
    pub request_id: String,
    pub sequence: u64,
    pub accepted_at: u64,
    pub expires_at: u64,
    pub request_digest: [u8; 32],
    pub signed_request: Vec<u8>,
    pub attempt: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CorporateOutboxAction {
    Real(ClaimedCorporateRequest),
    Dummy {
        slot: u64,
        due_at: u64,
    },
    NotDue,
    Expire {
        slot: u64,
        due_at: u64,
        request_id: String,
        request_digest: [u8; 32],
        signed_request: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorporateRequestStatus {
    pub request_id: String,
    pub request_digest: [u8; 32],
    pub expires_at: u64,
    pub state: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorporateReconciliation {
    pub request_id: String,
    pub request_digest: [u8; 32],
    pub status: String,
    pub hold_id: [u8; 32],
    pub state_root: [u8; 32],
    pub ledger_height: u64,
    pub queue_finalized: bool,
}

impl ParticipantClient {
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self, String> {
        let authority = endpoint
            .strip_prefix("http://")
            .ok_or_else(|| {
                "participant endpoint must use http:// inside the Docker network".to_string()
            })?
            .trim_end_matches('/');
        if authority.is_empty()
            || authority.contains('/')
            || authority.contains('@')
            || !authority.contains(':')
            || timeout.is_zero()
            || timeout > Duration::from_secs(60)
        {
            return Err("participant endpoint or timeout is invalid".into());
        }
        Ok(Self {
            authority: authority.to_string(),
            timeout,
        })
    }

    pub fn snapshot(&self) -> Result<ParticipantSnapshot, String> {
        let value = self.request("GET", "/v1/snapshot", None)?;
        let participant_id = fixed_hex(
            value
                .get("participant_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "participant snapshot has no participant id".to_string())?,
            "participant id",
        )?;
        let role = required_string(&value, "role")?;
        let label = required_string(&value, "label")?;
        let sequence = value
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "participant snapshot has no sequence".to_string())?;
        let cash = value
            .pointer("/portfolio/cash")
            .and_then(Value::as_u64)
            .ok_or_else(|| "participant snapshot has no cash balance".to_string())?;
        let inventory = value
            .pointer("/portfolio/inventory")
            .and_then(Value::as_u64)
            .ok_or_else(|| "participant snapshot has no inventory balance".to_string())?;
        let outbox_entries = value
            .pointer("/mpc_outbox/entries")
            .and_then(Value::as_array)
            .ok_or_else(|| "participant snapshot has no corporate outbox".to_string())?;
        let mut outbox_sequences = BTreeSet::new();
        for entry in outbox_entries {
            let outbox_sequence = entry
                .get("sequence")
                .and_then(Value::as_u64)
                .ok_or_else(|| "corporate outbox entry has no sequence".to_string())?;
            if !outbox_sequences.insert(outbox_sequence) {
                return Err("corporate outbox repeats a sequence".into());
            }
        }
        let corporate_outbox_next_sequence = outbox_sequences
            .last()
            .copied()
            .map(|sequence| {
                sequence
                    .checked_add(1)
                    .ok_or_else(|| "corporate outbox sequence is exhausted".to_string())
            })
            .transpose()?
            .unwrap_or(0);
        let records = value
            .pointer("/keys/keys")
            .and_then(Value::as_array)
            .ok_or_else(|| "participant snapshot has no public keys".to_string())?;
        let mut public_keys = BTreeMap::new();
        let mut pq_public_keys = BTreeMap::new();
        let mut application_keys = BTreeMap::new();
        let mut kyb_public_point = None;
        let mut note_view_public = None;
        let mut note_spend_public = None;
        let mut note_opening_public = None;
        for record in records {
            if record.get("state").and_then(Value::as_str) != Some("active") {
                continue;
            }
            let purpose = required_string(record, "purpose")?;
            let kind = required_string(record, "kind")?;
            let encoded = required_string(record, "public")?;
            let decoded = BASE64
                .decode(encoded)
                .map_err(|_| "participant public key is not base64".to_string())?;
            if let Some(base) = purpose.strip_suffix("_pq") {
                if !matches!(
                    base,
                    "admin" | "settlement" | "quote" | "mpc_input" | "emergency"
                ) || kind != "mldsa65"
                    || decoded.len() != zkfmi_crypto::suite::ML_DSA_65_PK_BYTES
                    || pq_public_keys.insert(base.to_string(), decoded).is_some()
                {
                    return Err("participant PQ key enrollment is invalid or repeated".into());
                }
                continue;
            }
            if purpose == "note_opening" {
                if kind != "x25519_mlkem768" || note_opening_public.is_some() {
                    return Err("participant hybrid note key is malformed or repeated".into());
                }
                let public: [u8; zkfmi_crypto::sealed::RECIPIENT_PUBLIC_BYTES] = decoded
                    .try_into()
                    .map_err(|_| "participant hybrid note key has wrong length")?;
                qomm_transport::selective_disclosure::WinnerPublicKey::from_raw(&public)?;
                note_opening_public = Some(public);
                continue;
            }
            let public: [u8; 32] = decoded
                .try_into()
                .map_err(|_| "participant public key is not 32 bytes".to_string())?;
            if matches!(
                purpose.as_str(),
                "quote_application" | "settlement_application"
            ) {
                if kind != "ed25519_mldsa65"
                    || application_keys
                        .insert(
                            purpose.clone(),
                            qomm_transport::application_crypto::VerifyingKey::from_bytes(&public)
                                .map_err(|error| error.to_string())?,
                        )
                        .is_some()
                {
                    return Err("invalid or repeated participant application enrollment".into());
                }
                continue;
            }
            if purpose == "kyb_entity" {
                if kind != "ristretto"
                    || CompressedRistretto(public).decompress().is_none()
                    || kyb_public_point.replace(public).is_some()
                {
                    return Err("participant anonymous KYB key is malformed or repeated".into());
                }
                continue;
            }
            if matches!(purpose.as_str(), "note_view" | "note_spend") {
                if kind != "ristretto" || CompressedRistretto(public).decompress().is_none() {
                    return Err(format!(
                        "participant {purpose} key is not canonical Ristretto"
                    ));
                }
                let target = if purpose == "note_view" {
                    &mut note_view_public
                } else {
                    &mut note_spend_public
                };
                if target.replace(public).is_some() {
                    return Err(format!("participant repeats its active {purpose} key"));
                }
                continue;
            }
            if kind != "ed25519" {
                return Err(format!(
                    "participant {purpose} key has unsupported kind {kind}"
                ));
            }
            VerifyingKey::from_bytes(&public)
                .map_err(|_| "participant public key is not valid Ed25519".to_string())?;
            if public_keys.insert(purpose, public).is_some() {
                return Err("participant snapshot repeats an active key purpose".into());
            }
        }
        for purpose in ["admin", "settlement", "quote", "mpc_input", "emergency"] {
            if !public_keys.contains_key(purpose) || !pq_public_keys.contains_key(purpose) {
                return Err(format!("participant snapshot has no active {purpose} key"));
            }
        }
        let kyb_public_point = kyb_public_point
            .ok_or_else(|| "participant snapshot has no anonymous KYB key".to_string())?;
        let note_opening_public =
            note_opening_public.ok_or("participant has no hybrid note opening key")?;
        let note_view_public = note_view_public
            .ok_or_else(|| "participant snapshot has no note viewing key".to_string())?;
        let note_spend_public = note_spend_public
            .ok_or_else(|| "participant snapshot has no note spending key".to_string())?;
        if value.pointer("/note_address/view").and_then(Value::as_str)
            != Some(BASE64.encode(note_view_public).as_str())
            || value
                .pointer("/note_address/opening")
                .and_then(Value::as_str)
                != Some(BASE64.encode(note_opening_public).as_str())
            || value.pointer("/note_address/spend").and_then(Value::as_str)
                != Some(BASE64.encode(note_spend_public).as_str())
        {
            return Err("participant note address differs from its active key records".into());
        }
        Ok(ParticipantSnapshot {
            participant_id,
            role,
            label,
            sequence,
            public_keys,
            pq_public_keys,
            quote_application_key: *application_keys
                .get("quote_application")
                .ok_or("missing enrolled quote application key")?,
            settlement_application_key: *application_keys
                .get("settlement_application")
                .ok_or("missing enrolled settlement application key")?,
            kyb_public_point,
            note_view_public,
            note_opening_public,
            note_spend_public,
            cash,
            inventory,
            corporate_outbox_next_sequence,
        })
    }

    /// Ask the entity-owned module to open only its aggregate economic
    /// holdings at one canonical DeFMI root. Individual claim openings and
    /// note keys never leave that module.
    pub fn canonical_portfolio(
        &self,
        snapshot: &ParticipantSnapshot,
        asset_ids: &[[u8; 32]],
    ) -> Result<CanonicalParticipantPortfolio, String> {
        if snapshot.role != "taker" || asset_ids.is_empty() || asset_ids.len() > 64 {
            return Err("canonical portfolio needs one Taker and a bounded asset set".into());
        }
        let expected = asset_ids.iter().copied().collect::<BTreeSet<_>>();
        if expected.len() != asset_ids.len() || expected.contains(&[0; 32]) {
            return Err("canonical portfolio assets are empty or repeated".into());
        }
        let value = self.request(
            "POST",
            "/v1/portfolio/canonical",
            Some(&json!({
                "asset_ids": asset_ids.iter().map(hex::encode).collect::<Vec<_>>(),
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "canonical portfolio participant",
        )?;
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "canonical portfolio state root",
        )?;
        if participant_id != snapshot.participant_id
            || state_root == [0; 32]
            || value.get("source").and_then(Value::as_str)
                != Some("defmi_final_claims_and_corporate_outbox")
            || value
                .get("private_openings_disclosed")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("canonical portfolio metadata is not participant-bound".into());
        }
        let cash = value
            .get("cash")
            .and_then(Value::as_u64)
            .ok_or_else(|| "canonical portfolio has no cash balance".to_string())?;
        let consumed_settlements = value
            .get("consumed_settlements")
            .and_then(Value::as_u64)
            .ok_or_else(|| "canonical portfolio has no settlement count".to_string())?;
        let rows = value
            .get("inventory")
            .and_then(Value::as_array)
            .ok_or_else(|| "canonical portfolio has no inventory".to_string())?;
        let mut inventory = BTreeMap::new();
        for row in rows {
            let asset_id = fixed_hex(
                &required_string(row, "asset_id")?,
                "canonical portfolio inventory asset",
            )?;
            let amount = row
                .get("amount")
                .and_then(Value::as_u64)
                .ok_or_else(|| "canonical inventory row has no amount".to_string())?;
            if inventory.insert(asset_id, amount).is_some() {
                return Err("canonical portfolio repeats an inventory asset".into());
            }
        }
        if inventory.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err("canonical portfolio returned another asset set".into());
        }
        Ok(CanonicalParticipantPortfolio {
            participant_id,
            state_root,
            cash,
            inventory,
            consumed_settlements,
        })
    }

    /// Ask the legal-entity service to prove membership in the signed cohort.
    /// The response is verified locally; the participant identifier is used
    /// only to authenticate the service response and is not embedded in the
    /// proof or the later DeFMI reservation.
    pub fn kyb_presentation(
        &self,
        request: KybPresentationRequest<'_>,
    ) -> Result<KybPresentation, String> {
        let KybPresentationRequest {
            snapshot,
            registry,
            trusted_issuer,
            scope,
            context,
            required_cohort,
            now,
        } = request;
        let value = self.request(
            "POST",
            "/v1/kyb/present",
            Some(&json!({
                "registry": KybRegistryWire::from_registry(registry),
                "trusted_issuer": hex::encode(trusted_issuer.to_bytes()),
                "scope": hex::encode(scope),
                "context": hex::encode(context),
                "required_cohort": required_cohort,
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "KYB participant id",
        )?;
        if participant_id != snapshot.participant_id {
            return Err("KYB presentation came from another participant service".into());
        }
        let wire: KybPresentationWire = serde_json::from_value(
            value
                .get("presentation")
                .cloned()
                .ok_or_else(|| "KYB response has no presentation".to_string())?,
        )
        .map_err(|_| "KYB response presentation is not canonical".to_string())?;
        let presentation = wire.into_presentation()?;
        verify_presentation(
            &presentation,
            registry,
            trusted_issuer,
            scope,
            context,
            now,
            required_cohort,
        )
        .map_err(|error| format!("participant KYB presentation was rejected: {error:?}"))?;
        if required_string(&value, "entity_commitment")?
            != hex::encode(presentation.entity_commitment())
            || required_string(&value, "presentation_digest")? != hex::encode(presentation.digest())
            || value
                .get("raw_legal_entity_id_disclosed")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("participant KYB response metadata does not match its proof".into());
        }
        Ok(presentation)
    }

    pub fn entity_approval(
        &self,
        snapshot: &ParticipantSnapshot,
        domain_id: [u8; 32],
        purpose: KeyPurpose,
        statement: [u8; 32],
    ) -> Result<EntityApproval, String> {
        let purpose_name = purpose_name(purpose);
        let body = EntityApproval::signing_body(&domain_id, purpose, 1, &statement);
        let value = self.request(
            "POST",
            "/v1/sign/entity-approval",
            Some(&json!({
                "body": BASE64.encode(&body), "purpose": purpose_name,
            })),
        )?;
        let signature = hex::decode(required_string(&value, "signature")?)
            .map_err(|_| "participant approval is not hex")?;
        if signature.len() != 64 + zkfmi_crypto::suite::ML_DSA_65_SIG_BYTES {
            return Err("participant approval requires both signature components".into());
        }
        let signed = SignedBody {
            participant_id: fixed_hex(
                &required_string(&value, "participant_id")?,
                "participant id",
            )?,
            public_key: fixed_hex(&required_string(&value, "public_key")?, "public key")?,
            signature: signature[..64].to_vec(),
        };
        self.verify_signature(snapshot, purpose_name, &body, &signed)?;
        zkfmi_crypto::traits::Verifier::verify(
            &zkfmi_crypto::backend::MlDsa65Verifier,
            zkfmi_crypto::key::KeyPurpose::Attestation,
            snapshot
                .pq_public_keys
                .get(purpose_name)
                .ok_or("missing enrolled PQ purpose key")?,
            &body,
            &signature[64..],
        )
        .map_err(|error| error.to_string())?;
        Ok(EntityApproval {
            participant_id: snapshot.participant_id,
            key_purpose: purpose,
            key_epoch: 1,
            statement,
            signature,
        })
    }

    pub fn sign_maker_mandate(
        &self,
        snapshot: &ParticipantSnapshot,
        mandate: &MakerPolicyMandate,
    ) -> Result<MakerPolicyMandate, String> {
        if snapshot.role != "maker" || snapshot.participant_id == [0; 32] {
            return Err("maker mandate was routed to a non-Maker participant".into());
        }
        let body = mandate.unsigned()?;
        let signed = self.sign("policy-mandate", &body, None)?;
        self.verify_application_signature(
            snapshot,
            &snapshot.quote_application_key,
            &body,
            &signed,
        )?;
        MakerPolicyMandate::from_signed_bytes(&body, signed.signature)
    }

    pub fn sign_taker_mandate(
        &self,
        snapshot: &ParticipantSnapshot,
        mandate: &TakerExecutionMandate,
    ) -> Result<TakerExecutionMandate, String> {
        if snapshot.role != "taker" || snapshot.participant_id == [0; 32] {
            return Err("taker mandate was routed to a non-Taker participant".into());
        }
        let body = mandate.unsigned()?;
        let signed = self.sign("execution-mandate", &body, None)?;
        self.verify_application_signature(
            snapshot,
            &snapshot.settlement_application_key,
            &body,
            &signed,
        )?;
        TakerExecutionMandate::from_signed_bytes(&body, signed.signature)
    }

    pub fn create_standing_pool_spend(
        &self,
        snapshot: &ParticipantSnapshot,
        asset_id: [u8; 32],
        source_note_id: [u8; 32],
        pool_id: [u8; 32],
        maximum_amount_commitment: [u8; 32],
    ) -> Result<StandingPoolSpendEvidence, String> {
        if snapshot.role != "maker" {
            return Err("standing-pool spend was routed to a non-Maker participant".into());
        }
        let value = self.request(
            "POST",
            "/v1/notes/standing-pool-spend",
            Some(&json!({
                "asset_id": hex::encode(asset_id),
                "source_note_id": hex::encode(source_note_id),
                "pool_id": hex::encode(pool_id),
                "maximum_amount_commitment": hex::encode(maximum_amount_commitment),
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "standing-pool participant id",
        )?;
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "standing-pool state root",
        )?;
        let returned_asset =
            fixed_hex(&required_string(&value, "asset_id")?, "standing-pool asset")?;
        if participant_id != snapshot.participant_id
            || returned_asset != asset_id
            || value.get("secrets_disclosed").and_then(Value::as_bool) != Some(false)
        {
            return Err("standing-pool proof came from another participant or asset".into());
        }
        let ring = value
            .get("ring")
            .and_then(Value::as_array)
            .ok_or_else(|| "standing-pool proof has no ring".to_string())?
            .iter()
            .map(|entry| {
                fixed_hex(
                    entry
                        .as_str()
                        .ok_or_else(|| "standing-pool ring member is not hex".to_string())?,
                    "standing-pool ring member",
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        if ring.len() < 2
            || !ring.len().is_power_of_two()
            || !ring.contains(&source_note_id)
            || ring.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("standing-pool proof ring is incomplete or non-canonical".into());
        }
        let proof_wire = BASE64
            .decode(required_string(&value, "proof")?)
            .map_err(|_| "standing-pool proof is not base64".to_string())?;
        let proof = decode_spend_proof(&proof_wire)?;
        if required_string(&value, "proof_digest")? != hex::encode(proof.digest()) {
            return Err("standing-pool proof digest differs from its complete wire".into());
        }
        let outputs = value
            .get("outputs")
            .and_then(Value::as_array)
            .ok_or_else(|| "standing-pool proof has no outputs".to_string())?
            .iter()
            .map(NoteOutput::from_body)
            .collect::<Result<Vec<_>, _>>()?;
        if outputs.len() != 2
            || outputs[0].asset_id != asset_id
            || outputs[0].lock_id != pool_id
            || outputs[0].value_commitment != maximum_amount_commitment
            || outputs[1].asset_id != asset_id
            || outputs[1].lock_id != [0; 32]
        {
            return Err("standing-pool proof outputs differ from the signed parent".into());
        }
        let context = hex::decode(required_string(&value, "context")?)
            .map_err(|_| "standing-pool proof context is not hex".to_string())?;
        let expected_context = [b"QOMM:DEMO:STANDING-POOL-SPEND:v1".as_slice(), &pool_id].concat();
        if context != expected_context {
            return Err("standing-pool proof uses another transcript context".into());
        }
        Ok(StandingPoolSpendEvidence {
            participant_id,
            state_root,
            asset_id,
            ring,
            proof,
            outputs,
            context,
        })
    }

    /// Ask the entity-owned wallet to merge several anonymous unlocked notes.
    /// The coordinator receives complete spend proofs and commitments, but no
    /// value opening and not even the merged wallet balance.
    pub fn create_note_consolidation(
        &self,
        snapshot: &ParticipantSnapshot,
        asset_id: [u8; 32],
        consolidation_id: [u8; 32],
        minimum_amount: u64,
    ) -> Result<Option<NoteConsolidationEvidence>, String> {
        if !matches!(snapshot.role.as_str(), "maker" | "taker")
            || asset_id == [0; 32]
            || consolidation_id == [0; 32]
            || minimum_amount == 0
        {
            return Err("note consolidation has no eligible participant or scope".into());
        }
        let value = self.request(
            "POST",
            "/v1/notes/consolidation",
            Some(&json!({
                "asset_id": hex::encode(asset_id),
                "consolidation_id": hex::encode(consolidation_id),
                "minimum_amount": minimum_amount,
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "note-consolidation participant id",
        )?;
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "note-consolidation state root",
        )?;
        let returned_asset = fixed_hex(
            &required_string(&value, "asset_id")?,
            "note-consolidation asset",
        )?;
        let returned_id = fixed_hex(
            &required_string(&value, "consolidation_id")?,
            "note-consolidation id",
        )?;
        let needed = value
            .get("needed")
            .and_then(Value::as_bool)
            .ok_or_else(|| "note-consolidation response has no decision".to_string())?;
        if participant_id != snapshot.participant_id
            || returned_asset != asset_id
            || returned_id != consolidation_id
            || value.get("secrets_disclosed").and_then(Value::as_bool) != Some(false)
            || value.get("total").is_some()
        {
            return Err("note-consolidation proof came from another participant or scope".into());
        }
        if !needed {
            if value.get("spends").is_some() || value.get("consolidated_output").is_some() {
                return Err("unneeded note consolidation returned spend material".into());
            }
            return Ok(None);
        }
        let spend_values = value
            .get("spends")
            .and_then(Value::as_array)
            .ok_or_else(|| "note-consolidation response has no spends".to_string())?;
        if !(2..=8).contains(&spend_values.len()) {
            return Err("note consolidation is outside the atomic input bound".into());
        }
        let mut serials = BTreeSet::new();
        let mut output_ids = BTreeSet::new();
        let mut commitment_sum = RistrettoPoint::identity();
        let mut spends = Vec::with_capacity(spend_values.len());
        for (lane, entry) in spend_values.iter().enumerate() {
            let ring = entry
                .get("ring")
                .and_then(Value::as_array)
                .ok_or_else(|| "note-consolidation spend has no ring".to_string())?
                .iter()
                .map(|member| {
                    fixed_hex(
                        member.as_str().ok_or_else(|| {
                            "note-consolidation ring member is not hex".to_string()
                        })?,
                        "note-consolidation ring member",
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if ring.len() < 2
                || !ring.len().is_power_of_two()
                || ring.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err("note-consolidation ring is incomplete or non-canonical".into());
            }
            let proof_wire = BASE64
                .decode(required_string(entry, "proof")?)
                .map_err(|_| "note-consolidation proof is not base64".to_string())?;
            let proof = decode_spend_proof(&proof_wire)?;
            if required_string(entry, "proof_digest")? != hex::encode(proof.digest())
                || !serials.insert(proof.serial_point.compress().to_bytes())
            {
                return Err("note-consolidation proof digest or serial is invalid".into());
            }
            let outputs = entry
                .get("outputs")
                .and_then(Value::as_array)
                .ok_or_else(|| "note-consolidation spend has no output".to_string())?
                .iter()
                .map(NoteOutput::from_body)
                .collect::<Result<Vec<_>, _>>()?;
            if outputs.len() != 1
                || outputs[0].asset_id != asset_id
                || outputs[0].lock_id != [0; 32]
                || !output_ids.insert(outputs[0].note_id)
            {
                return Err("note-consolidation spend output is incompatible".into());
            }
            commitment_sum += CompressedRistretto(outputs[0].value_commitment)
                .decompress()
                .ok_or_else(|| {
                    "note-consolidation output commitment is not canonical".to_string()
                })?;
            let context = hex::decode(required_string(entry, "context")?)
                .map_err(|_| "note-consolidation context is not hex".to_string())?;
            let expected_context = [
                b"QOMM:DEMO:NOTE-CONSOLIDATION-SPEND:v1".as_slice(),
                &consolidation_id,
                &(lane as u64).to_be_bytes(),
            ]
            .concat();
            if context != expected_context {
                return Err("note-consolidation proof uses another transcript context".into());
            }
            spends.push(NoteConsolidationSpendEvidence {
                ring,
                proof,
                outputs,
                context,
            });
        }
        let consolidated_output = NoteOutput::from_body(
            value
                .get("consolidated_output")
                .ok_or_else(|| "note-consolidation response has no final output".to_string())?,
        )?;
        if consolidated_output.asset_id != asset_id
            || consolidated_output.lock_id != [0; 32]
            || output_ids.contains(&consolidated_output.note_id)
            || consolidated_output.value_commitment != commitment_sum.compress().to_bytes()
        {
            return Err("note-consolidation final output changes the committed value".into());
        }
        Ok(Some(NoteConsolidationEvidence {
            participant_id,
            state_root,
            asset_id,
            consolidation_id,
            spends,
            consolidated_output,
        }))
    }

    pub fn create_note_reservation_spend(
        &self,
        snapshot: &ParticipantSnapshot,
        asset_id: [u8; 32],
        reserve_id: [u8; 32],
        amount: u64,
        amount_blinding: u64,
        amount_commitment: [u8; 32],
    ) -> Result<NoteReservationSpendEvidence, String> {
        if !matches!(snapshot.role.as_str(), "maker" | "taker") {
            return Err("note reservation was routed to a non-corporate participant".into());
        }
        let value = self.request(
            "POST",
            "/v1/notes/reservation-spend",
            Some(&json!({
                "asset_id": hex::encode(asset_id),
                "reserve_id": hex::encode(reserve_id),
                "amount": amount,
                "amount_blinding": amount_blinding,
                "amount_commitment": hex::encode(amount_commitment),
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "note-reservation participant id",
        )?;
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "note-reservation state root",
        )?;
        let returned_asset = fixed_hex(
            &required_string(&value, "asset_id")?,
            "note-reservation asset",
        )?;
        let returned_reserve = fixed_hex(
            &required_string(&value, "reserve_id")?,
            "note-reservation id",
        )?;
        if participant_id != snapshot.participant_id
            || returned_asset != asset_id
            || returned_reserve != reserve_id
            || value.get("secrets_disclosed").and_then(Value::as_bool) != Some(false)
        {
            return Err("note-reservation proof came from another participant or scope".into());
        }
        let ring = value
            .get("ring")
            .and_then(Value::as_array)
            .ok_or_else(|| "note-reservation proof has no ring".to_string())?
            .iter()
            .map(|entry| {
                fixed_hex(
                    entry
                        .as_str()
                        .ok_or_else(|| "note-reservation ring member is not hex".to_string())?,
                    "note-reservation ring member",
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        if ring.len() < 2
            || !ring.len().is_power_of_two()
            || ring.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("note-reservation proof ring is incomplete or non-canonical".into());
        }
        let proof_wire = BASE64
            .decode(required_string(&value, "proof")?)
            .map_err(|_| "note-reservation proof is not base64".to_string())?;
        let proof = decode_spend_proof(&proof_wire)?;
        if required_string(&value, "proof_digest")? != hex::encode(proof.digest()) {
            return Err("note-reservation proof digest differs from its complete wire".into());
        }
        let outputs = value
            .get("outputs")
            .and_then(Value::as_array)
            .ok_or_else(|| "note-reservation proof has no outputs".to_string())?
            .iter()
            .map(NoteOutput::from_body)
            .collect::<Result<Vec<_>, _>>()?;
        if outputs.len() != 2
            || outputs[0].asset_id != asset_id
            || outputs[0].lock_id != reserve_id
            || outputs[0].value_commitment != amount_commitment
            || outputs[1].asset_id != asset_id
            || outputs[1].lock_id != [0; 32]
        {
            return Err("note-reservation proof outputs differ from the signed reserve".into());
        }
        let context = hex::decode(required_string(&value, "context")?)
            .map_err(|_| "note-reservation proof context is not hex".to_string())?;
        let expected_context = [
            b"QOMM:DEMO:NOTE-RESERVATION-SPEND:v1".as_slice(),
            &reserve_id,
        ]
        .concat();
        if context != expected_context {
            return Err("note-reservation proof uses another transcript context".into());
        }
        Ok(NoteReservationSpendEvidence {
            participant_id,
            state_root,
            asset_id,
            reserve_id,
            ring,
            proof,
            outputs,
            context,
        })
    }

    pub fn enqueue_corporate_request(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        signed_request: &[u8],
        expires_at: u64,
    ) -> Result<OutboxEnqueueReceipt, String> {
        if !matches!(snapshot.role.as_str(), "maker" | "taker")
            || request_id.is_empty()
            || signed_request.is_empty()
        {
            return Err("corporate outbox enqueue has no eligible origin or payload".into());
        }
        let value = self.request(
            "POST",
            "/v1/outbox/requests",
            Some(&json!({
                "request_id": request_id,
                "signed_request": BASE64.encode(signed_request),
                "expires_at": expires_at,
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "outbox participant id",
        )?;
        let returned_id = required_string(&value, "request_id")?;
        let request_digest = fixed_hex(
            &required_string(&value, "request_digest")?,
            "outbox request digest",
        )?;
        let sequence = value
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "outbox response has no sequence".to_string())?;
        let status = required_string(&value, "status")?;
        if participant_id != snapshot.participant_id
            || returned_id != request_id
            || request_digest != <[u8; 32]>::from(Sha256::digest(signed_request))
            || !matches!(status.as_str(), "enqueued" | "already_present")
            || value
                .get("local_execution_fallback")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("corporate outbox returned another request or unsafe policy".into());
        }
        Ok(OutboxEnqueueReceipt {
            request_id: returned_id,
            request_digest,
            sequence,
            already_present: status == "already_present",
        })
    }

    pub fn create_facility_hold_evidence(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        request_digest: [u8; 32],
    ) -> Result<FacilityHoldEvidence, String> {
        if snapshot.role != "taker" || request_id.is_empty() {
            return Err("facility hold evidence needs a Taker corporate request".into());
        }
        let value = self.request(
            "POST",
            "/v1/facility/hold-evidence",
            Some(&json!({
                "request_id": request_id,
                "request_digest": hex::encode(request_digest),
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "facility participant id",
        )?;
        let returned_id = required_string(&value, "request_id")?;
        let returned_digest = fixed_hex(
            &required_string(&value, "request_digest")?,
            "facility request digest",
        )?;
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "facility state root",
        )?;
        if participant_id != snapshot.participant_id
            || returned_id != request_id
            || returned_digest != request_digest
            || value
                .get("private_openings_disclosed")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("facility hold evidence came from another participant or request".into());
        }
        let transition_value = value
            .get("transition")
            .ok_or_else(|| "facility hold evidence has no transition".to_string())?;
        let transition = credit_transition_from_body(transition_value)?;
        let proof_wire = BASE64
            .decode(required_string(&value, "relation_proof")?)
            .map_err(|_| "facility relation proof is not base64".to_string())?;
        let relation_proof = CreditFacilityRelationProof::from_bytes(&proof_wire)?;
        relation_proof.verify(&transition)?;
        Ok(FacilityHoldEvidence {
            participant_id,
            state_root,
            transition,
            relation_proof,
        })
    }

    pub fn create_claim_materializations(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        request_digest: [u8; 32],
    ) -> Result<([u8; 32], Vec<ClaimMaterializationEvidence>), String> {
        let value = self.request(
            "POST",
            "/v1/claims/materializations",
            Some(&json!({
                "request_id": request_id,
                "request_digest": hex::encode(request_digest),
            })),
        )?;
        if fixed_hex(
            &required_string(&value, "participant_id")?,
            "claim participant id",
        )? != snapshot.participant_id
            || required_string(&value, "request_id")? != request_id
            || fixed_hex(
                &required_string(&value, "request_digest")?,
                "claim request digest",
            )? != request_digest
            || value.get("post_match_signature").and_then(Value::as_bool) != Some(false)
            || value
                .get("private_openings_disclosed")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("claim materializations came from another participant or request".into());
        }
        let state_root = fixed_hex(
            &required_string(&value, "state_root")?,
            "claim materialization state root",
        )?;
        let values = value
            .get("materializations")
            .and_then(Value::as_array)
            .ok_or_else(|| "claim response has no materialization array".to_string())?;
        if values.len() > 4_096 {
            return Err("claim response exceeds the participant materialization bound".into());
        }
        let expected_recipient = G * Scalar::from(participant_handle_scalar(
            b"taker",
            &snapshot.participant_id,
        ));
        let expected_view = CompressedRistretto(snapshot.note_view_public)
            .decompress()
            .ok_or_else(|| "participant note view key is not canonical".to_string())?;
        let expected_spend = CompressedRistretto(snapshot.note_spend_public)
            .decompress()
            .ok_or_else(|| "participant note spend key is not canonical".to_string())?;
        let mut evidence = Vec::with_capacity(values.len());
        let mut claims = BTreeSet::new();
        for value in values {
            let object = value
                .as_object()
                .ok_or_else(|| "claim materialization entry is not an object".to_string())?;
            let rfq_nullifier = body_hex32(object, "rfq_nullifier")?;
            let recipient_handle = CompressedRistretto(body_hex32(object, "recipient_handle")?)
                .decompress()
                .ok_or_else(|| "claim recipient handle is not canonical".to_string())?;
            let destination_value = object
                .get("destination")
                .and_then(Value::as_object)
                .ok_or_else(|| "claim materialization has no destination".to_string())?;
            let destination = Address {
                opening_public: hex::decode(
                    destination_value
                        .get("opening")
                        .and_then(Value::as_str)
                        .ok_or("claim destination lacks hybrid key")?,
                )
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "claim destination hybrid key length")?,
                view: CompressedRistretto(body_hex32(destination_value, "view")?)
                    .decompress()
                    .ok_or_else(|| "claim destination view key is not canonical".to_string())?,
                spend: CompressedRistretto(body_hex32(destination_value, "spend")?)
                    .decompress()
                    .ok_or_else(|| "claim destination spend key is not canonical".to_string())?,
            };
            if recipient_handle.compress() != expected_recipient.compress()
                || destination.view.compress() != expected_view.compress()
                || destination.spend.compress() != expected_spend.compress()
                || destination.opening_public != snapshot.note_opening_public
            {
                return Err("claim materialization changes the participant recipient".into());
            }
            let materialization_value = object
                .get("materialization")
                .and_then(Value::as_object)
                .ok_or_else(|| "claim response has no materialization body".to_string())?;
            let materialization = NoteClaimMaterialization {
                operation_id: body_hex32(materialization_value, "operation_id")?,
                claim_id: body_hex32(materialization_value, "claim_id")?,
                output: NoteOutput::from_body(
                    materialization_value
                        .get("output")
                        .ok_or_else(|| "claim materialization has no output".to_string())?,
                )?,
                ownership_proof_digest: body_hex32(
                    materialization_value,
                    "ownership_proof_digest",
                )?,
            };
            materialization.body()?;
            if !claims.insert(materialization.claim_id) {
                return Err("claim response repeats a materialization".into());
            }
            let proof_value = object
                .get("ownership_proof")
                .and_then(Value::as_object)
                .ok_or_else(|| "claim response has no ownership proof".to_string())?;
            let nonce = CompressedRistretto(body_hex32(proof_value, "nonce")?)
                .decompress()
                .ok_or_else(|| "claim ownership nonce is not canonical".to_string())?;
            let response = Option::<Scalar>::from(Scalar::from_canonical_bytes(body_hex32(
                proof_value,
                "response",
            )?))
            .ok_or_else(|| "claim ownership response is not canonical".to_string())?;
            evidence.push(ClaimMaterializationEvidence {
                rfq_nullifier,
                recipient_handle,
                destination,
                materialization,
                ownership_proof: ClaimOwnershipProof { nonce, response },
            });
        }
        Ok((state_root, evidence))
    }

    pub fn corporate_request_status(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
    ) -> Result<Option<CorporateRequestStatus>, String> {
        let value = self.request("GET", "/v1/snapshot", None)?;
        if fixed_hex(
            value
                .get("participant_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "participant snapshot has no participant id".to_string())?,
            "participant id",
        )? != snapshot.participant_id
        {
            return Err("corporate outbox snapshot came from another participant".into());
        }
        let entries = value
            .pointer("/mpc_outbox/entries")
            .and_then(Value::as_array)
            .ok_or_else(|| "participant snapshot has no corporate outbox".to_string())?;
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.get("request_id").and_then(Value::as_str) == Some(request_id))
        else {
            return Ok(None);
        };
        let state = entry
            .get("state")
            .and_then(|state| match state {
                Value::String(value) => Some(value.clone()),
                Value::Object(value) if value.len() == 1 => value.keys().next().cloned(),
                _ => None,
            })
            .ok_or_else(|| "corporate outbox entry has no canonical state".to_string())?;
        Ok(Some(CorporateRequestStatus {
            request_id: request_id.to_string(),
            request_digest: json_bytes32(
                entry
                    .get("request_digest")
                    .ok_or_else(|| "corporate outbox entry has no request digest".to_string())?,
                "corporate outbox request digest",
            )?,
            expires_at: entry
                .get("expires_at")
                .and_then(Value::as_u64)
                .ok_or_else(|| "corporate outbox entry has no expiry".to_string())?,
            state,
        }))
    }

    pub fn claim_corporate_cover_slot(
        &self,
        snapshot: &ParticipantSnapshot,
        quorum_healthy: bool,
        retry_after_seconds: u64,
        interval_seconds: u64,
    ) -> Result<CorporateOutboxAction, String> {
        let value = self.request(
            "POST",
            "/v1/outbox/claim",
            Some(&json!({
                "quorum_healthy": quorum_healthy,
                "retry_after_seconds": retry_after_seconds,
                "interval_seconds": interval_seconds,
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "outbox participant id",
        )?;
        if participant_id != snapshot.participant_id
            || value
                .get("local_execution_fallback")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("corporate outbox claim came from another participant".into());
        }
        let action = required_string(&value, "action")?;
        if action == "not_due" {
            return Ok(CorporateOutboxAction::NotDue);
        }
        let slot = value
            .get("slot")
            .and_then(Value::as_u64)
            .ok_or_else(|| "outbox cover response has no slot".to_string())?;
        let due_at = value
            .get("due_at")
            .and_then(Value::as_u64)
            .ok_or_else(|| "outbox cover response has no due time".to_string())?;
        match action.as_str() {
            "dummy" => Ok(CorporateOutboxAction::Dummy { slot, due_at }),
            "expire" => {
                let request_digest = fixed_hex(
                    &required_string(&value, "request_digest")?,
                    "expired outbox digest",
                )?;
                let mut signed_request = BASE64
                    .decode(required_string(&value, "signed_request")?)
                    .map_err(|_| "expired corporate request is not base64".to_string())?;
                if signed_request.is_empty()
                    || request_digest != <[u8; 32]>::from(Sha256::digest(&signed_request))
                {
                    signed_request.fill(0);
                    return Err("expired corporate request digest is invalid".into());
                }
                Ok(CorporateOutboxAction::Expire {
                    slot,
                    due_at,
                    request_id: required_string(&value, "request_id")?,
                    request_digest,
                    signed_request,
                })
            }
            "real" => {
                let mut signed_request = BASE64
                    .decode(required_string(&value, "signed_request")?)
                    .map_err(|_| "claimed corporate request is not base64".to_string())?;
                let request_digest = fixed_hex(
                    &required_string(&value, "request_digest")?,
                    "claimed request digest",
                )?;
                if signed_request.is_empty()
                    || request_digest != <[u8; 32]>::from(Sha256::digest(&signed_request))
                {
                    signed_request.fill(0);
                    return Err("claimed corporate request digest is invalid".into());
                }
                let attempt = u32::try_from(
                    value
                        .get("attempt")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| "claimed request has no attempt".to_string())?,
                )
                .map_err(|_| "claimed request attempt exceeds u32".to_string())?;
                Ok(CorporateOutboxAction::Real(ClaimedCorporateRequest {
                    slot,
                    due_at,
                    request_id: required_string(&value, "request_id")?,
                    sequence: value
                        .get("sequence")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| "claimed request has no sequence".to_string())?,
                    accepted_at: value
                        .get("accepted_at")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| "claimed request has no acceptance time".to_string())?,
                    expires_at: value
                        .get("expires_at")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| "claimed request has no expiry".to_string())?,
                    request_digest,
                    signed_request,
                    attempt,
                }))
            }
            _ => Err("corporate outbox returned an unknown action".into()),
        }
    }

    pub fn record_corporate_mpc_admission(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        request_digest: [u8; 32],
        committee_id: &str,
        job_id: &str,
    ) -> Result<(), String> {
        let value = self.request(
            "POST",
            "/v1/outbox/mpc-admission",
            Some(&json!({
                "request_id": request_id,
                "request_digest": hex::encode(request_digest),
                "committee_id": committee_id,
                "job_id": job_id,
            })),
        )?;
        if fixed_hex(
            &required_string(&value, "participant_id")?,
            "outbox participant id",
        )? != snapshot.participant_id
            || required_string(&value, "request_id")? != request_id
            || fixed_hex(
                &required_string(&value, "request_digest")?,
                "outbox request digest",
            )? != request_digest
            || value.get("status").and_then(Value::as_str) != Some("mpc_admitted")
        {
            return Err("corporate outbox recorded another MPC admission".into());
        }
        Ok(())
    }

    pub fn abort_corporate_before_reserve(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        request_digest: [u8; 32],
    ) -> Result<[u8; 32], String> {
        let value = self.request(
            "POST",
            "/v1/outbox/abort-before-reserve",
            Some(&json!({
                "request_id": request_id,
                "request_digest": hex::encode(request_digest),
            })),
        )?;
        if fixed_hex(
            &required_string(&value, "participant_id")?,
            "outbox participant id",
        )? != snapshot.participant_id
            || required_string(&value, "request_id")? != request_id
            || fixed_hex(
                &required_string(&value, "request_digest")?,
                "outbox request digest",
            )? != request_digest
            || value.get("status").and_then(Value::as_str) != Some("aborted_before_reserve")
            || value
                .get("local_execution_fallback")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("corporate outbox aborted another pre-reserve request".into());
        }
        fixed_hex(
            &required_string(&value, "state_root")?,
            "pre-reserve abort state root",
        )
    }

    pub fn reconcile_corporate_request(
        &self,
        snapshot: &ParticipantSnapshot,
        request_id: &str,
        request_digest: [u8; 32],
    ) -> Result<CorporateReconciliation, String> {
        let value = self.request(
            "POST",
            "/v1/outbox/reconcile",
            Some(&json!({
                "request_id": request_id,
                "request_digest": hex::encode(request_digest),
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "outbox participant id",
        )?;
        let returned_id = required_string(&value, "request_id")?;
        let returned_digest = fixed_hex(
            &required_string(&value, "request_digest")?,
            "outbox request digest",
        )?;
        let status = required_string(&value, "status")?;
        let queue_finalized = value
            .get("queue_finalized")
            .and_then(Value::as_bool)
            .ok_or_else(|| "outbox reconciliation has no finality flag".to_string())?;
        if participant_id != snapshot.participant_id
            || returned_id != request_id
            || returned_digest != request_digest
            || !matches!(
                status.as_str(),
                "not_reserved" | "active" | "consumed" | "released"
            )
            || queue_finalized != matches!(status.as_str(), "consumed" | "released")
            || value
                .get("local_execution_fallback")
                .and_then(Value::as_bool)
                != Some(false)
        {
            return Err("corporate outbox reconciliation is not bound to this request".into());
        }
        Ok(CorporateReconciliation {
            request_id: returned_id,
            request_digest: returned_digest,
            status,
            hold_id: fixed_hex(&required_string(&value, "hold_id")?, "DeFMI hold id")?,
            state_root: fixed_hex(&required_string(&value, "state_root")?, "DeFMI state root")?,
            ledger_height: value
                .get("ledger_height")
                .and_then(Value::as_u64)
                .ok_or_else(|| "outbox reconciliation has no ledger height".to_string())?,
            queue_finalized,
        })
    }

    fn sign(
        &self,
        operation: &str,
        body: &[u8],
        purpose: Option<&str>,
    ) -> Result<SignedBody, String> {
        let value = self.request(
            "POST",
            &format!("/v1/sign/{operation}"),
            Some(&json!({
                "body": BASE64.encode(body),
                "purpose": purpose,
            })),
        )?;
        let participant_id = fixed_hex(
            &required_string(&value, "participant_id")?,
            "participant id",
        )?;
        let public_key = fixed_hex(&required_string(&value, "public_key")?, "public key")?;
        let signature = hex::decode(required_string(&value, "signature")?)
            .map_err(|_| "participant signature is not hex".to_string())?;
        qomm_transport::application_crypto::Signature::try_from(signature.as_slice())
            .map_err(|error| error.to_string())?;
        Ok(SignedBody {
            participant_id,
            public_key,
            signature,
        })
    }

    fn verify_application_signature(
        &self,
        snapshot: &ParticipantSnapshot,
        expected: &qomm_transport::application_crypto::VerifyingKey,
        body: &[u8],
        signed: &SignedBody,
    ) -> Result<(), String> {
        if signed.participant_id != snapshot.participant_id
            || signed.public_key != expected.to_bytes()
        {
            return Err("application signature differs from the enrolled entity or purpose".into());
        }
        let signature =
            qomm_transport::application_crypto::Signature::try_from(signed.signature.as_slice())
                .map_err(|error| error.to_string())?;
        expected
            .verify_strict(body, &signature)
            .map_err(|error| error.to_string())
    }

    fn verify_signature(
        &self,
        snapshot: &ParticipantSnapshot,
        purpose: &str,
        body: &[u8],
        signed: &SignedBody,
    ) -> Result<(), String> {
        let expected = snapshot
            .public_keys
            .get(purpose)
            .ok_or_else(|| format!("participant has no {purpose} key"))?;
        if signed.participant_id != snapshot.participant_id || &signed.public_key != expected {
            return Err("participant signature came from another entity or purpose key".into());
        }
        VerifyingKey::from_bytes(expected)
            .map_err(|_| "participant public key is invalid".to_string())?
            .verify(
                body,
                &Signature::from_slice(&signed.signature).map_err(|error| error.to_string())?,
            )
            .map_err(|_| "participant service returned an invalid signature".to_string())
    }

    fn request(&self, method: &str, path: &str, value: Option<&Value>) -> Result<Value, String> {
        let body = value
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let address = self
            .authority
            .to_socket_addrs()
            .map_err(|error| error.to_string())?
            .next()
            .ok_or_else(|| format!("participant {} did not resolve", self.authority))?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout).map_err(|error| {
            format!(
                "participant {} did not accept a connection: {error}",
                self.authority
            )
        })?;
        stream
            .set_read_timeout(Some(self.timeout))
            .and_then(|_| stream.set_write_timeout(Some(self.timeout)))
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
            return Err("participant response exceeded its fixed bound".into());
        }
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| "participant returned malformed HTTP".to_string())?;
        let head = std::str::from_utf8(&response[..split])
            .map_err(|_| "participant returned non-UTF8 HTTP headers")?;
        let status = head.lines().next().unwrap_or_default();
        let value: Value = serde_json::from_slice(&response[split + 4..])
            .map_err(|error| format!("participant returned malformed JSON: {error}"))?;
        let status_code = status
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| "participant returned an invalid HTTP status".to_string())?;
        if !(200..300).contains(&status_code) {
            return Err(value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("participant rejected the request")
                .to_string());
        }
        Ok(value)
    }
}

#[derive(Clone, Debug)]
struct SignedBody {
    participant_id: [u8; 32],
    public_key: [u8; 32],
    signature: Vec<u8>,
}

fn fixed_hex(value: &str, label: &str) -> Result<[u8; 32], String> {
    let decoded = hex::decode(value).map_err(|_| format!("{label} is not hex"))?;
    decoded
        .try_into()
        .map_err(|_| format!("{label} is not 32 bytes"))
}

fn json_bytes32(value: &Value, label: &str) -> Result<[u8; 32], String> {
    let bytes = value
        .as_array()
        .ok_or_else(|| format!("{label} is not a byte array"))?;
    if bytes.len() != 32 {
        return Err(format!("{label} is not 32 bytes"));
    }
    let decoded = bytes
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| format!("{label} contains a non-byte value"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    decoded
        .try_into()
        .map_err(|_| format!("{label} is not 32 bytes"))
}

fn credit_transition_from_body(value: &Value) -> Result<CreditFacilityTransition, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "facility transition is not an object".to_string())?;
    let hex32 = |name: &str| -> Result<[u8; 32], String> {
        fixed_hex(
            object
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("facility transition has no {name}"))?,
            name,
        )
    };
    let number = |name: &str| -> Result<u64, String> {
        object
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("facility transition has no {name}"))
    };
    let kind = match object.get("kind").and_then(Value::as_str) {
        Some("hold") => CreditTransitionKind::Hold,
        Some("release") => CreditTransitionKind::Release,
        Some("consume") => CreditTransitionKind::Consume,
        _ => return Err("facility transition has an invalid kind".into()),
    };
    let transition = CreditFacilityTransition {
        operation_id: hex32("operation_id")?,
        facility_id: hex32("facility_id")?,
        hold_id: hex32("hold_id")?,
        kind,
        query_commitment: hex32("query_commitment")?,
        amount_commitment: hex32("amount_commitment")?,
        consumed_commitment: hex32("consumed_commitment")?,
        refund_commitment: hex32("refund_commitment")?,
        before_available_commitment: hex32("before_available_commitment")?,
        after_available_commitment: hex32("after_available_commitment")?,
        before_held_commitment: hex32("before_held_commitment")?,
        after_held_commitment: hex32("after_held_commitment")?,
        before_outstanding_commitment: hex32("before_outstanding_commitment")?,
        after_outstanding_commitment: hex32("after_outstanding_commitment")?,
        before_sequence: number("before_sequence")?,
        expires_at: number("expires_at")?,
        settlement_digest: hex32("settlement_digest")?,
        relation_proof_digest: hex32("relation_proof_digest")?,
    };
    transition.body()?;
    Ok(transition)
}

fn body_hex32(object: &serde_json::Map<String, Value>, name: &str) -> Result<[u8; 32], String> {
    fixed_hex(
        object
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("participant response has no {name}"))?,
        name,
    )
}

fn participant_handle_scalar(role: &[u8], participant_id: &[u8; 32]) -> u64 {
    let digest = Sha256::new()
        .chain_update(b"QOMM:DEMO:PARTICIPANT-HANDLE:v1")
        .chain_update(role)
        .chain_update(participant_id)
        .finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix")).max(1)
}

fn required_string(value: &Value, name: &str) -> Result<String, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("participant response has no {name}"))
}

fn purpose_name(purpose: KeyPurpose) -> &'static str {
    match purpose {
        KeyPurpose::Admin => "admin",
        KeyPurpose::Settlement => "settlement",
        KeyPurpose::Quote => "quote",
        KeyPurpose::MpcInput => "mpc_input",
        KeyPurpose::Emergency => "emergency",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_and_fixed_encodings_fail_closed() {
        assert!(ParticipantClient::new("https://entity:9200", Duration::from_secs(1)).is_err());
        assert!(ParticipantClient::new("http://entity:9200/path", Duration::from_secs(1)).is_err());
        assert!(ParticipantClient::new("http://entity:9200", Duration::ZERO).is_err());
        assert!(fixed_hex("00", "id").is_err());
        assert_eq!(purpose_name(KeyPurpose::MpcInput), "mpc_input");
    }
}
