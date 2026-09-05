//! Idempotent bootstrap for the public Docker demonstration's DeFMI domain.
//!
//! This module registers the legal entities and the service-specific 3-of-7
//! MPC committee on the actual Avalanche-backed DeFMI ledger.  Governance
//! signing keys here deliberately match the public local-development genesis;
//! they are never suitable for a production network.

use crate::participant_client::{KybPresentationRequest, ParticipantClient, ParticipantSnapshot};
use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use qomm_defmi::asset_link::prove as prove_asset_link;
use qomm_defmi::avalanche::{
    AvalancheClient, AvalancheNoteBridge, AvalancheRpcClient, CanonicalCreditFacility,
    CanonicalCreditHold, CanonicalNoteReservation, CanonicalStandingNotePool,
    StandingNotePoolAllocationPreview, StandingPoolProductSettlementRequest,
};
use qomm_defmi::facility::{
    reserve_handle_for, AssetDefinition, AssetKind, CreditFacilityGrant, CreditTransitionKind,
    GuarantorDefinition, GuarantorKind, ReservationRole, ZERO,
};
use qomm_defmi::facility::{
    AdmissionBatchPlan, AdmissionCommitteePlan, CreditFacilityTransition, QuorumApproval,
    QuorumAuthorizer, ReservationAuthorization,
};
use qomm_defmi::note_chain::{
    standing_note_pool_delegation_digest, standing_note_pool_id,
    standing_pool_product_settlement_statement, verify_claim_materialization, CsdIssuerDefinition,
    NoteIssuance, NoteOutput, NoteReservationEscrow, NoteSettlementOrder, NoteSpend,
    ProductNoteNoFillReleaseOrder, ProductNoteReleaseOrder, ProductNoteSettlementOrder,
    StandingNotePoolAllocation, StandingNotePoolRegistration,
};
use qomm_defmi::notes::{Address, NoteLedger, Wallet};
use qomm_defmi::participant::{
    KeyPurpose, MpcService, MpcServiceKind, MpcServiceMember, ParticipantKeys, ParticipantRecord,
    ParticipantRole, ParticipantServiceBinding, ParticipantStatus, PurposeKey, RegisterParticipant,
    RegistryConfiguration, ServiceStatus,
};
use qomm_defmi::product::{verify_note_reservation, verify_taker_reservation, IdentityEvidence};
use qomm_defmi::product_evidence::{MpcNoFillEvidence, ProductSettlementEvidence};
use qomm_defmi::settlement_verifier::SettlementVerifierConfig;
use qomm_mpc::program::{
    PRODUCT_QUOTE_ELIGIBILITY_BITS, PRODUCT_QUOTE_SPAN_BITS, PRODUCT_ZKPI_AMOUNT_BITS,
    PRODUCT_ZKPI_PRICE_BITS,
};
use qomm_proofs::kyb::{cohort_id, KybPresentation, SignedCohortRegistry};
use qomm_transport::frost_cluster::{
    sign_reserve_context, sign_reserve_payment, ReserveMandateRef,
};
use qomm_transport::mandate::{Direction, MakerPolicyMandate, TakerExecutionMandate};
use qomm_transport::order::{
    verify_admission_lane, CertifiedAdmissionLane, NodeAdmissionAttestation, OrderedAdmission,
};
use qomm_transport::proof_client::ProofPartyRpc;
use qomm_zk::pedersen::Pedersen;
use qomm_zkpi::typed::{
    AuthorizationScope, ExecutionContext, OperationKind, TradeDirection, TypedInstruction,
};
use qomm_zkpi::{frost, typed_wire, Bounds, Issuer, Openings, Venue};
use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_core::OsRng;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zkpi_defmi_sdk::finality::{CanonicalReadback, ReadbackKind};

const MAX_RPC_BYTES: usize = 1 << 20;
const ACCEPTANCE_TIMEOUT: Duration = Duration::from_secs(180);
const ACCEPTANCE_POLL: Duration = Duration::from_millis(200);
const DEMO_INFRASTRUCTURE_VALID_UNTIL: u64 = 4_102_444_800;
// Once a signed Taker mandate expires, the refund right must survive an
// arbitrarily long gateway or MPC outage.  A short grace period can strand a
// canonical reserve forever precisely when the recovery path is needed most.
const AUTOMATIC_EXPIRY_RELEASE_VALID_UNTIL: u64 = i64::MAX as u64;

/// Public-demo receipt authority shared by the local DeFMI container and the
/// seven proof nodes.  This deterministic key is intentionally non-secret and
/// must never be used outside the checked-in demonstration network.
pub fn development_receipt_signing_key() -> SigningKey {
    SigningKey::from_bytes(&digest("QOMM:DEMO:DEFMI-RESERVATION-RECEIPT:v1"))
}

pub fn development_receipt_public() -> [u8; 32] {
    development_receipt_signing_key().verifying_key().to_bytes()
}

#[derive(Clone, Debug)]
pub struct DefmiBootstrapConfig {
    pub rpc_endpoint: String,
    pub maker_endpoints: Vec<String>,
    pub taker_endpoint: String,
    pub mpc_operator_endpoints: Vec<String>,
    pub program_digest: [u8; 32],
}

#[derive(Clone, Debug, Serialize)]
pub struct DefmiBootstrapReport {
    pub chain_id: String,
    pub defmi_id: String,
    pub domain_id: String,
    pub service_id: String,
    pub participant_count: usize,
    pub operator_count: usize,
    pub maker_participant_ids: Vec<String>,
    pub taker_participant_id: String,
    pub registered_transactions: Vec<String>,
    pub state_root: String,
    pub governance: &'static str,
    /// Participant-owned pre-trade capacities seed the browser projection;
    /// they are not part of the public infrastructure description.
    #[serde(skip_serializing)]
    pub maker_capacities: Vec<(u64, u64)>,
    #[serde(skip_serializing)]
    pub taker_capacity: (u64, u64),
    /// Native verifier inputs stay inside the Rust process and are omitted
    /// from the public infrastructure snapshot.  Only proof digests and
    /// scope-local commitments are shown in the browser.
    #[serde(skip_serializing)]
    pub kyb: DefmiKybBundle,
}

#[derive(Clone, Debug)]
pub struct DefmiKybBundle {
    pub registry: SignedCohortRegistry,
    pub trusted_issuer: VerifyingKey,
    pub scope: Vec<u8>,
    pub context: Vec<u8>,
    pub required_cohort: String,
    pub presentations: BTreeMap<[u8; 32], KybPresentation>,
}

/// Authoritative market epoch attached to the already-bootstrapped legal
/// entity registry.  It registers the governance-pinned proof key and each
/// fixed admission population before any private MPC input is executed.
pub struct DefmiMarketEpoch {
    rpc: AvalancheRpcClient,
    authorizer: QuorumAuthorizer,
    governance_keys: BTreeMap<String, SigningKey>,
    venue_id: [u8; 32],
    defmi_id: [u8; 32],
    epoch: u64,
    registry_digest: Option<[u8; 32]>,
    committee_keys: Option<Vec<[u8; 32]>>,
}

#[derive(Clone, Debug)]
pub struct DefmiAdmissionReceipt {
    pub epoch: u64,
    pub batch_id: [u8; 32],
    pub admission_digest: [u8; 32],
    pub after_state_root: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct MakerStandingPoolReceipt {
    pub pool: CanonicalStandingNotePool,
    pub facility_id: [u8; 32],
    pub source_note_id: [u8; 32],
}

pub struct MakerStandingPoolRequest<'a> {
    pub participant: &'a ParticipantClient,
    pub snapshot: &'a ParticipantSnapshot,
    pub mandate: &'a MakerPolicyMandate,
    pub maximum_amount: u64,
    pub maximum_blinding: u64,
    pub now: u64,
    pub existing_only: bool,
}

#[derive(Clone, Debug)]
pub struct TakerFundingReceipt {
    pub source_note_id: [u8; 32],
    pub asset_id: [u8; 32],
    pub amount: u64,
}

#[derive(Clone, Debug)]
pub struct TakerNoteReservationReceipt {
    pub reservation: CanonicalNoteReservation,
    pub facility_id: [u8; 32],
    pub reserve_receipt_digest: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct DefmiStandingPoolAllocationPreview {
    pub allocation: StandingNotePoolAllocationPreview,
    pub allocation_approval: QuorumApproval,
}

#[derive(Clone, Debug, Serialize)]
pub struct DefmiProductSettlementReceipt {
    pub transaction_id: String,
    pub block_id: String,
    pub height: u64,
    pub statement: [u8; 32],
    pub before_state_root: [u8; 32],
    pub after_state_root: [u8; 32],
    pub maker_hold_id: [u8; 32],
    pub taker_hold_id: [u8; 32],
    pub claim_ids: Vec<[u8; 32]>,
    /// Canonical resources re-read after consensus acceptance. Every entry is
    /// pinned to `after_state_root` and is consumed by the application SDK's
    /// finality gate.
    pub canonical_readbacks: Vec<CanonicalReadback>,
}

impl DefmiMarketEpoch {
    pub fn connect(
        rpc_endpoint: &str,
        chain_id: &str,
        venue_id: [u8; 32],
        defmi_id: [u8; 32],
    ) -> Result<Self, String> {
        if chain_id.is_empty() || venue_id == [0; 32] || defmi_id == [0; 32] {
            return Err("DeFMI market epoch lacks its chain, venue, or domain".into());
        }
        let rpc = docker_rpc_client(rpc_endpoint, Duration::from_secs(30))?;
        let (authorizer, governance_keys) = development_committee(chain_id)?;
        Ok(Self {
            rpc,
            authorizer,
            governance_keys,
            venue_id,
            defmi_id,
            // The epoch is derived from the complete verifier configuration
            // during registration. Wall-clock process start must never create
            // another canonical market or invalidate existing Maker pools.
            epoch: 0,
            registry_digest: None,
            committee_keys: None,
        })
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn venue_id(&self) -> [u8; 32] {
        self.venue_id
    }

    pub fn defmi_id(&self) -> [u8; 32] {
        self.defmi_id
    }

    pub fn state_root(&self) -> Result<[u8; 32], String> {
        self.rpc.state_root()
    }

    /// Read the consensus-owned admission cursor immediately before a fresh
    /// RFQ is signed. Corporate queue sequence numbers and cover slots are
    /// deliberately separate namespaces and must never be used as a DeFMI
    /// venue ordering cursor, especially after a gateway restart or dummy slot.
    pub fn next_admission_sequence(&self) -> Result<u64, String> {
        if self.epoch == 0 {
            return Err("DeFMI verifier must be registered before admission ordering".into());
        }
        Ok(self
            .rpc
            .admission_cursor(self.venue_id, self.epoch)?
            .next_sequence)
    }

    pub fn standing_note_pool(
        &self,
        pool_id: [u8; 32],
    ) -> Result<CanonicalStandingNotePool, String> {
        AvalancheNoteBridge::new(&self.authorizer, &self.rpc).standing_note_pool(pool_id)
    }

    pub fn credit_facility(
        &self,
        facility_id: [u8; 32],
    ) -> Result<CanonicalCreditFacility, String> {
        AvalancheNoteBridge::new(&self.authorizer, &self.rpc).credit_facility(facility_id)
    }

    pub fn credit_hold(&self, hold_id: [u8; 32]) -> Result<CanonicalCreditHold, String> {
        AvalancheNoteBridge::new(&self.authorizer, &self.rpc).credit_hold(hold_id)
    }

    pub fn note_reservation(&self, hold_id: [u8; 32]) -> Result<CanonicalNoteReservation, String> {
        AvalancheNoteBridge::new(&self.authorizer, &self.rpc).note_reservation(hold_id)
    }

    pub fn note_output(&self, note_id: [u8; 32]) -> Result<NoteOutput, String> {
        Ok(AvalancheNoteBridge::new(&self.authorizer, &self.rpc)
            .note(note_id)?
            .output)
    }

    pub fn maker_facility_id(
        entity_commitment: [u8; 32],
        asset_id: [u8; 32],
    ) -> Result<[u8; 32], String> {
        if entity_commitment == ZERO || asset_id == ZERO {
            return Err("Maker facility lacks its entity or asset".into());
        }
        Ok(hash_parts(&[
            b"QOMM:DEMO:MAKER-FACILITY:v2",
            &entity_commitment,
            &asset_id,
        ]))
    }

    /// Issue the demo's opening custody balance exactly once. Subsequent RFQs
    /// consume the unspent change notes created from this source; this method
    /// never mints a fresh balance merely because an MPC retry or new RFQ
    /// occurred.
    pub fn ensure_taker_funding(
        &self,
        snapshot: &ParticipantSnapshot,
        entity_commitment: [u8; 32],
        asset_id: [u8; 32],
        kind: AssetKind,
        now: u64,
    ) -> Result<TakerFundingReceipt, String> {
        if snapshot.role != "taker"
            || snapshot.participant_id == ZERO
            || entity_commitment == ZERO
            || asset_id == ZERO
        {
            return Err("Taker funding request has another participant or empty scope".into());
        }
        let amount = match kind {
            AssetKind::Cash => snapshot.cash,
            AssetKind::Security
            | AssetKind::Fund
            | AssetKind::Commodity
            | AssetKind::Carbon
            | AssetKind::Other => snapshot.inventory,
        };
        if amount == 0 {
            return Err("Taker has no opening custody balance for this rail".into());
        }
        let asset = AssetDefinition {
            asset_id,
            code: match kind {
                AssetKind::Cash => "QOMM-DEMO-CASH".into(),
                _ => format!("QOMM-DEMO-SEC-{}", &hex::encode(asset_id)[..8]),
            },
            kind,
            decimals: 0,
            terms_digest: hash_parts(&[b"QOMM:DEMO:ASSET-TERMS:v1", &asset_id]),
        };
        self.verify_or_register_asset(&asset)?;
        let csd_key = SigningKey::from_bytes(&hash_parts(&[
            b"QOMM:DEMO:CSD-ISSUER-KEY:v1",
            &self.defmi_id,
            &asset_id,
        ]));
        let csd = CsdIssuerDefinition {
            issuer_id: hash_parts(&[b"QOMM:DEMO:CSD-ISSUER:v1", &self.defmi_id, &asset_id]),
            code: format!("DEFMI-DEMO-CSD-{}", &hex::encode(asset_id)[..8]),
            jurisdiction: "JP".into(),
            operator_entity_commitment: hash_parts(&[b"QOMM:DEMO:CSD-OPERATOR:v1", &self.defmi_id]),
            public_key: csd_key.verifying_key().to_bytes(),
            permitted_asset_ids: vec![asset_id],
            policy_digest: hash_parts(&[b"QOMM:DEMO:CSD-POLICY:v1", &self.defmi_id, &asset_id]),
            valid_from: 1,
            valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
        };
        self.verify_or_register_csd(&csd)?;
        let funding_id = hash_parts(&[
            b"QOMM:DEMO:TAKER-OPENING-FUNDING:v1",
            &self.defmi_id,
            &entity_commitment,
            &asset_id,
        ]);
        let source = self.verify_or_issue_note(
            snapshot,
            asset_id,
            amount,
            deterministic_scalar(&[b"QOMM:DEMO:TAKER-OPENING-BLINDING:v1", &funding_id]),
            funding_id,
            false,
            &csd,
            &csd_key,
            now,
        )?;
        self.verify_or_issue_note(
            snapshot,
            asset_id,
            amount
                .checked_add(1)
                .ok_or_else(|| "Taker decoy amount overflowed".to_string())?,
            deterministic_scalar(&[b"QOMM:DEMO:TAKER-DECOY-BLINDING:v1", &funding_id]),
            funding_id,
            true,
            &csd,
            &csd_key,
            now,
        )?;
        Ok(TakerFundingReceipt {
            source_note_id: source.note_id,
            asset_id,
            amount,
        })
    }

    /// Atomically replace several wallet-owned anonymous notes with their
    /// commitment sum when no single note can cover the next reservation.
    /// Values and ownership openings remain inside the participant container;
    /// DeFMI receives only verified spend projections and one sum commitment.
    fn materialize_taker_claims(
        &self,
        participant: &ParticipantClient,
        snapshot: &ParticipantSnapshot,
        corporate_request_id: &str,
        corporate_request_digest: [u8; 32],
    ) -> Result<usize, String> {
        let (proof_root, evidence) = participant.create_claim_materializations(
            snapshot,
            corporate_request_id,
            corporate_request_digest,
        )?;
        if evidence.is_empty() {
            if self.rpc.state_root()? != proof_root {
                return Err("DeFMI changed while checking Taker settlement claims".into());
            }
            return Ok(0);
        }
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let mut materialized = 0_usize;
        for (index, evidence) in evidence.into_iter().enumerate() {
            let canonical = bridge.note_claim(evidence.materialization.claim_id)?;
            if canonical.status != "active" {
                return Err("participant attempted to rematerialize a final note claim".into());
            }
            if index == 0 && canonical.state_root != proof_root {
                return Err("Taker claim proof was built over a stale DeFMI root".into());
            }
            let claim = canonical.claim()?;
            verify_claim_materialization(
                &claim,
                evidence.rfq_nullifier,
                &evidence.recipient_handle,
                &evidence.destination,
                &evidence.materialization,
                &evidence.ownership_proof,
            )?;
            let before = self.rpc.state_root()?;
            if canonical.state_root != before {
                // Earlier materializations in this same loop legitimately
                // advance the root. Re-read this independent claim before
                // approving its one-use output.
                let refreshed = bridge.note_claim(evidence.materialization.claim_id)?;
                if refreshed.status != "active" || refreshed.claim()? != claim {
                    return Err("Taker claim changed before materialization".into());
                }
            }
            let statement = evidence.materialization.statement()?;
            let approval = approve(&self.authorizer, &self.governance_keys, statement, before)?;
            let accepted = bridge.materialize_note_claim(&evidence.materialization, &approval)?;
            let output = bridge.note(evidence.materialization.output.note_id)?;
            let final_claim = bridge.note_claim(evidence.materialization.claim_id)?;
            if accepted.after_root != output.state_root
                || output.output != evidence.materialization.output
                || final_claim.state_root != accepted.after_root
                || final_claim.status != "materialized"
                || final_claim.materialization != statement
            {
                return Err("DeFMI accepted a different Taker claim materialization".into());
            }
            materialized += 1;
        }
        Ok(materialized)
    }

    fn consolidate_notes_if_needed(
        &self,
        participant: &ParticipantClient,
        snapshot: &ParticipantSnapshot,
        asset_id: [u8; 32],
        consolidation_id: [u8; 32],
        minimum_amount: u64,
        deadline: u64,
    ) -> Result<(), String> {
        let Some(evidence) = participant.create_note_consolidation(
            snapshot,
            asset_id,
            consolidation_id,
            minimum_amount,
        )?
        else {
            return Ok(());
        };
        if evidence.participant_id != snapshot.participant_id
            || evidence.asset_id != asset_id
            || evidence.consolidation_id != consolidation_id
        {
            return Err("note consolidation returned another participant or scope".into());
        }
        let key = Pedersen::new(b"qomm:defmi:v1");
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let (proof_root, ledger, canonical) = bridge.note_ledger(asset_id, key, 64, 16_384)?;
        if evidence.state_root != proof_root || self.rpc.state_root()? != proof_root {
            return Err("note consolidation proof was built over a stale DeFMI root".into());
        }
        let mut spends = Vec::with_capacity(evidence.spends.len());
        for spend in &evidence.spends {
            let ring = spend
                .ring
                .iter()
                .map(|note_id| {
                    canonical
                        .iter()
                        .position(|output| output.note_id == *note_id)
                        .ok_or_else(|| {
                            "note-consolidation ring is absent from canonical DeFMI state"
                                .to_string()
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let ring_locks = ring
                .iter()
                .map(|index| canonical[*index].lock_id)
                .collect::<Vec<_>>();
            let outputs = spend
                .outputs
                .iter()
                .map(NoteOutput::to_note)
                .collect::<Result<Vec<_>, _>>()?;
            let projected = NoteSpend::from_verified(
                &ledger,
                &ring,
                &spend.proof,
                &outputs,
                asset_id,
                &ring_locks,
                ZERO,
                &[ZERO],
                &spend.context,
                &mut OsRng,
            )?;
            if projected.ring != spend.ring || projected.outputs != spend.outputs {
                return Err("note-consolidation projection differs from its complete proof".into());
            }
            spends.push(projected);
        }
        let order = NoteSettlementOrder {
            operation_id: hash_parts(&[
                b"QOMM:DEMO:NOTE-CONSOLIDATION-OP:v1",
                &self.defmi_id,
                &consolidation_id,
                &proof_root,
            ]),
            nullifier: hash_parts(&[
                b"QOMM:DEMO:NOTE-CONSOLIDATION-NULLIFIER:v1",
                &self.defmi_id,
                &consolidation_id,
            ]),
            deadline,
            payment_instruction_digest: hash_parts(&[
                b"QOMM:DEMO:NOTE-CONSOLIDATION-INSTRUCTION:v1",
                &snapshot.participant_id,
                &consolidation_id,
            ]),
            market_statement_digest: hash_parts(&[
                b"QOMM:DEMO:NOTE-CONSOLIDATION-MARKET:v1",
                &self.venue_id,
                &consolidation_id,
            ]),
            dvp_proof_digest: hash_parts(&[
                b"QOMM:DEMO:NOTE-CONSOLIDATION-PROOF:v1",
                &proof_root,
                &evidence.consolidated_output.value_commitment,
            ]),
            spends,
            consolidated_output: Some(evidence.consolidated_output.clone()),
        };
        let before = self.rpc.state_root()?;
        if before != proof_root {
            return Err("DeFMI changed before note consolidation approval".into());
        }
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            order.statement()?,
            before,
        )?;
        let accepted = bridge.settle(&order, &approval)?;
        let canonical_output = bridge.note(evidence.consolidated_output.note_id)?;
        if accepted.after_root != canonical_output.state_root
            || canonical_output.output != evidence.consolidated_output
            || self.rpc.state_root()? != accepted.after_root
        {
            return Err("DeFMI accepted a different note-consolidation result".into());
        }
        for spend in &order.spends {
            let serial = bridge.note_serial(spend.serial_point)?;
            if !serial.spent || serial.state_root != accepted.after_root {
                return Err("DeFMI did not consume every note-consolidation input".into());
            }
        }
        Ok(())
    }

    fn ensure_taker_facility(
        &self,
        snapshot: &ParticipantSnapshot,
        entity_commitment: [u8; 32],
        asset_id: [u8; 32],
        kind: AssetKind,
    ) -> Result<(CanonicalCreditFacility, u64, Scalar), String> {
        let cap_amount = match kind {
            AssetKind::Cash => snapshot.cash,
            AssetKind::Security
            | AssetKind::Fund
            | AssetKind::Commodity
            | AssetKind::Carbon
            | AssetKind::Other => snapshot.inventory,
        };
        if cap_amount == 0 {
            return Err("Taker aggregate facility cannot have an empty cap".into());
        }
        let facility_id = hash_parts(&[
            b"QOMM:DEMO:TAKER-FACILITY:v1",
            &self.defmi_id,
            &entity_commitment,
            &asset_id,
        ]);
        let guarantor_key = SigningKey::from_bytes(&hash_parts(&[
            b"QOMM:DEMO:TAKER-GUARANTOR-KEY:v1",
            &self.defmi_id,
            &entity_commitment,
            &asset_id,
        ]));
        let guarantor = GuarantorDefinition {
            guarantor_id: hash_parts(&[
                b"QOMM:DEMO:TAKER-GUARANTOR:v1",
                &self.defmi_id,
                &entity_commitment,
                &asset_id,
            ]),
            kind: GuarantorKind::SelfGuaranteed,
            name: format!("Taker reserve {}", &hex::encode(entity_commitment)[..12]),
            public_key: guarantor_key.verifying_key().to_bytes(),
            risk_policy_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-RISK-POLICY:v1",
                &self.defmi_id,
                &asset_id,
            ]),
        };
        match self.rpc.guarantor_snapshot(guarantor.guarantor_id) {
            Ok(existing) => {
                if existing.definition != guarantor || !existing.active {
                    return Err("canonical guarantor differs from the Taker facility".into());
                }
            }
            Err(error) if not_found(&error) => {
                let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    guarantor.statement()?,
                    before,
                )?;
                bridge.register_guarantor(&guarantor, &approval)?;
            }
            Err(error) => return Err(error),
        }
        let cap_blinding =
            deterministic_scalar(&[b"QOMM:DEMO:TAKER-FACILITY-CAP-BLINDING:v1", &facility_id]);
        let cap_commitment = Pedersen::new(b"qomm:defmi:v1")
            .commit_u64(cap_amount, &cap_blinding)
            .compress()
            .to_bytes();
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let facility = match bridge.credit_facility(facility_id) {
            Ok(existing) => {
                if existing.facility.guarantor_id != guarantor.guarantor_id
                    || existing.facility.beneficiary_commitment != entity_commitment
                    || existing.facility.rail_asset_id != asset_id
                    || existing.facility.cap_commitment != cap_commitment
                    || existing.facility.risk_policy_digest != guarantor.risk_policy_digest
                    || existing.facility.status
                        != qomm_defmi::facility::CreditFacilityStatus::Active
                {
                    return Err("canonical Taker facility differs from its aggregate cap".into());
                }
                existing
            }
            Err(error) if not_found(&error) => {
                let collateral_blinding = cap_blinding + Scalar::ONE;
                let mut grant = CreditFacilityGrant {
                    operation_id: hash_parts(&[b"QOMM:DEMO:TAKER-FACILITY-OP:v1", &facility_id]),
                    facility_id,
                    guarantor_id: guarantor.guarantor_id,
                    beneficiary_commitment: entity_commitment,
                    rail_asset_id: asset_id,
                    cap_commitment,
                    available_commitment: cap_commitment,
                    held_commitment: ZERO,
                    outstanding_commitment: ZERO,
                    collateral_commitment: Pedersen::new(b"qomm:defmi:v1")
                        .commit_u64(
                            cap_amount
                                .checked_add(1)
                                .ok_or_else(|| "Taker collateral amount overflowed".to_string())?,
                            &collateral_blinding,
                        )
                        .compress()
                        .to_bytes(),
                    risk_policy_digest: guarantor.risk_policy_digest,
                    relation_proof_digest: hash_parts(&[
                        b"QOMM:DEMO:TAKER-FACILITY-GRANT-PROOF:v1",
                        &facility_id,
                    ]),
                    valid_from: 1,
                    valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
                    nonce: hash_parts(&[b"QOMM:DEMO:TAKER-FACILITY-NONCE:v1", &facility_id]),
                    guarantor_signature: Signature::from_bytes(&[0; 64]),
                };
                grant.guarantor_signature = guarantor_key.sign(&grant.guarantor_message()?);
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    grant.statement()?,
                    before,
                )?;
                bridge.grant_credit_facility(&grant, &approval)?;
                bridge.credit_facility(facility_id)?
            }
            Err(error) => return Err(error),
        };
        Ok((facility, cap_amount, cap_blinding))
    }

    /// Lock the Taker's exact maximum on DeFMI before any private MPC input is
    /// revealed. The participant service owns the note keys; proof parties own
    /// the 3-of-7 reserve key; this coordinator receives only public proofs.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_taker_note<T: ProofPartyRpc>(
        &self,
        participant: &ParticipantClient,
        snapshot: &ParticipantSnapshot,
        corporate_request_id: &str,
        corporate_request_digest: [u8; 32],
        mandate: &TakerExecutionMandate,
        maximum_amount: u64,
        maximum_blinding: u64,
        admission: &CertifiedAdmissionLane,
        admission_receipt: &DefmiAdmissionReceipt,
        presentation: &KybPresentation,
        registry: &SignedCohortRegistry,
        trusted_issuer: &VerifyingKey,
        identity_scope: &[u8],
        identity_context: &[u8],
        required_cohort: &str,
        proof_parties: &mut [T],
        frost_public: &frost::keys::PublicKeyPackage,
        now: u64,
    ) -> Result<TakerNoteReservationReceipt, String> {
        mandate.verify(
            presentation,
            registry,
            trusted_issuer,
            identity_scope,
            identity_context,
            required_cohort,
            now,
        )?;
        if snapshot.role != "taker"
            || snapshot.participant_id == ZERO
            // Expiry cleanup deliberately survives an immutable MPC-service
            // upgrade.  The old venue remains bound by the Taker signature
            // and by the canonical hold's `query_commitment == mandate.digest`;
            // requiring it to equal the coordinator's current venue would
            // strand every active reserve created by the prior program.
            || mandate.venue_id == ZERO
            || mandate.defmi_id != self.defmi_id
            || mandate.entity_commitment != presentation.entity_commitment()
            || mandate.taker_public
                != *snapshot
                    .public_keys
                    .get("settlement")
                    .ok_or_else(|| "Taker participant has no settlement key".to_string())?
            || maximum_amount == 0
        {
            return Err("Taker note reservation differs from its signed participant scope".into());
        }
        let amount_blinding = Scalar::from(maximum_blinding);
        let key = Pedersen::new(b"qomm:defmi:v1");
        let amount_commitment = key
            .commit_u64(maximum_amount, &amount_blinding)
            .compress()
            .to_bytes();
        if amount_commitment != mandate.maximum_amount_commitment {
            return Err("Taker note opening differs from its signed maximum".into());
        }
        let mandate_digest = mandate.digest()?;
        let asset_kind = match mandate.direction {
            Direction::TakerBuys => AssetKind::Cash,
            Direction::TakerSells => AssetKind::Security,
        };
        self.ensure_taker_funding(
            snapshot,
            mandate.entity_commitment,
            mandate.reserve_asset_id,
            asset_kind,
            now,
        )?;
        let (facility, _, _) = self.ensure_taker_facility(
            snapshot,
            mandate.entity_commitment,
            mandate.reserve_asset_id,
            asset_kind,
        )?;
        let facility_id = facility.facility.facility_id;
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        match bridge.note_reservation(mandate.reserve_id) {
            Ok(existing) => {
                let hold = bridge.credit_hold(mandate.reserve_id)?;
                if existing.status != "active"
                    || existing.asset_id != mandate.reserve_asset_id
                    || existing.amount_commitment != amount_commitment
                    || existing.reserve_receipt_digest == ZERO
                    || hold.facility_id != facility_id
                    || hold.query_commitment != mandate_digest
                    || hold.amount_commitment != amount_commitment
                    || hold.expires_at != mandate.deadline
                {
                    return Err("existing Taker reservation belongs to another RFQ".into());
                }
                return Ok(TakerNoteReservationReceipt {
                    reserve_receipt_digest: existing.reserve_receipt_digest,
                    reservation: existing,
                    facility_id,
                });
            }
            Err(error) if not_found(&error) => {}
            Err(error) => return Err(error),
        }
        // A crashed coordinator may have committed the anonymous covenant and
        // failed only during MPC proof completion. Check that idempotent hold
        // before looking for another unlocked source note: the original value
        // is then correctly absent from the wallet because it is already in
        // the DeFMI escrow output.
        self.materialize_taker_claims(
            participant,
            snapshot,
            corporate_request_id,
            corporate_request_digest,
        )?;
        self.consolidate_notes_if_needed(
            participant,
            snapshot,
            mandate.reserve_asset_id,
            mandate.reserve_id,
            maximum_amount,
            mandate.deadline,
        )?;
        let ordered = OrderedAdmission {
            venue_id: self.venue_id,
            epoch: admission_receipt.epoch,
            slot: admission.slot,
            sequence: admission.sequence,
            ticket_id: admission.ticket_id,
            batch_digest: admission.cluster_digest,
            order_digest: admission.order_digest,
            rfq_nullifier: mandate.rfq_nullifier,
            taker_entity_commitment: mandate.entity_commitment,
            taker_mandate_digest: mandate_digest,
            expires_at: mandate.deadline,
        };
        if ordered.certified_digest()? != admission_receipt.admission_digest
            || admission.digest(self.venue_id, admission_receipt.epoch)?
                != admission_receipt.admission_digest
        {
            return Err("Taker reserve is bound to another ordered admission".into());
        }
        if corporate_request_id != hex::encode(mandate.rfq_nullifier) {
            return Err("Taker reserve names another participant-owned corporate RFQ".into());
        }
        let facility_evidence = participant.create_facility_hold_evidence(
            snapshot,
            corporate_request_id,
            corporate_request_digest,
        )?;
        let transition = facility_evidence.transition;
        let relation_proof = facility_evidence.relation_proof;
        let before = self.rpc.state_root()?;
        let facility = bridge.credit_facility(facility_id)?;
        if facility_evidence.participant_id != snapshot.participant_id
            || facility_evidence.state_root != before
            || facility.state_root != before
            || transition.operation_id
                != hash_parts(&[b"QOMM:DEMO:TAKER-HOLD-OP:v1", &mandate.reserve_id])
            || transition.facility_id != facility_id
            || transition.hold_id != mandate.reserve_id
            || transition.kind != CreditTransitionKind::Hold
            || transition.query_commitment != mandate_digest
            || transition.amount_commitment != amount_commitment
            || transition.before_available_commitment != facility.facility.available_commitment
            || transition.before_held_commitment != facility.facility.held_commitment
            || transition.before_outstanding_commitment != facility.facility.outstanding_commitment
            || transition.before_sequence != facility.facility.sequence
            || transition.expires_at != mandate.deadline
            || transition.consumed_commitment != ZERO
            || transition.refund_commitment != ZERO
            || transition.settlement_digest != ZERO
        {
            return Err(
                "participant facility proof differs from canonical DeFMI or the RFQ".into(),
            );
        }
        relation_proof.verify(&transition)?;
        let evidence = participant.create_note_reservation_spend(
            snapshot,
            mandate.reserve_asset_id,
            mandate.reserve_id,
            maximum_amount,
            maximum_blinding,
            amount_commitment,
        )?;
        let (proof_root, ledger, canonical) =
            bridge.note_ledger(mandate.reserve_asset_id, key.clone(), 64, 16_384)?;
        if proof_root != before || evidence.state_root != before || self.rpc.state_root()? != before
        {
            return Err(
                "Taker note proof and facility transition use different DeFMI roots".into(),
            );
        }
        let ring = evidence
            .ring
            .iter()
            .map(|note_id| {
                canonical
                    .iter()
                    .position(|output| output.note_id == *note_id)
                    .ok_or_else(|| {
                        "Taker proof ring is absent from canonical DeFMI state".to_string()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ring_locks = ring
            .iter()
            .map(|index| canonical[*index].lock_id)
            .collect::<Vec<_>>();
        let notes = evidence
            .outputs
            .iter()
            .map(NoteOutput::to_note)
            .collect::<Result<Vec<_>, _>>()?;
        let delegation_digest = hash_parts(&[
            b"QOMM:DEMO:DELEGATED-TAKER-RESERVATION:v1",
            &mandate.reserve_id,
            &mandate_digest,
            &self.venue_id,
            &self.defmi_id,
            &mandate.reserve_asset_id,
            &[mandate.direction as u8],
            &mandate.deadline.to_be_bytes(),
        ]);
        let escrow = NoteReservationEscrow::from_verified(
            &ledger,
            &ring,
            &evidence.proof,
            &notes,
            mandate.reserve_asset_id,
            &ring_locks,
            &[mandate.reserve_id, ZERO],
            &transition,
            delegation_digest,
            &evidence.context,
            &mut OsRng,
        )?;
        if escrow.spend.ring != evidence.ring || escrow.spend.outputs != evidence.outputs {
            return Err("Taker escrow projection differs from the participant proof".into());
        }
        let issuer = Issuer::new(key.clone(), Bounds::default());
        let taker_handle = CompressedRistretto(mandate.taker_handle)
            .decompress()
            .ok_or_else(|| "Taker handle is not canonical".to_string())?;
        let reserve_handle = reserve_handle_for(&facility_id);
        let nonce = hash_parts(&[
            b"QOMM:DEMO:TAKER-RESERVE-ZKPI-NONCE:v1",
            &mandate.reserve_id,
        ]);
        let quote_key = u64::from_be_bytes(
            hash_parts(&[b"QOMM:DEMO:TAKER-RESERVE-QUOTE-KEY:v1", &mandate.reserve_id])[..8]
                .try_into()
                .expect("eight bytes"),
        )
        .max(1);
        let (payment_digest, payment_openings, partial) = issuer
            .build_for_asset_id_with_openings(
                maximum_amount,
                1,
                mandate.reserve_asset_id,
                taker_handle,
                reserve_handle,
                mandate.deadline,
                nonce,
                quote_key,
                Openings {
                    amount: amount_blinding,
                    price: Scalar::random(&mut OsRng),
                    asset: Scalar::random(&mut OsRng),
                },
                &mut OsRng,
            )
            .map_err(|error| error.to_string())?;
        if partial.digest().as_slice() != payment_digest.as_slice() {
            return Err("reserve issuer returned a mismatched payment digest".into());
        }
        let payment_signature = sign_reserve_payment(
            proof_parties,
            &[1, 4, 7],
            frost_public,
            &partial,
            ReserveMandateRef::Taker(mandate),
        )?;
        let payment = partial.sealed(payment_signature);
        let context = ExecutionContext {
            operation: OperationKind::Reserve,
            scope: AuthorizationScope::Taker,
            direction: match mandate.direction {
                Direction::TakerBuys => TradeDirection::TakerBuys,
                Direction::TakerSells => TradeDirection::TakerSells,
            },
            venue_id: self.venue_id,
            defmi_id: self.defmi_id,
            maker_handle: G * deterministic_scalar(&[
                b"QOMM:DEMO:TAKER-RESERVE-MAKER-PLACEHOLDER:v1",
                &mandate.reserve_id,
            ]),
            taker_handle,
            reserve_handle,
            maker_reservation_id: ZERO,
            maker_reservation_sequence: 0,
            taker_reservation_id: mandate.reserve_id,
            taker_reservation_sequence: transition.before_sequence,
            rfq_nullifier: mandate.rfq_nullifier,
            taker_mandate_digest: mandate_digest,
            maker_policy_digest: ZERO,
            maker_mandate_digest: ZERO,
            maker_reserve_receipt_digest: ZERO,
            taker_reserve_receipt_digest: ZERO,
            quote_proof_digest: ZERO,
            market_statement_digest: ZERO,
            before_state_root: before,
        };
        let typed = TypedInstruction {
            pq_authorization: None,
            authorization: sign_reserve_context(
                proof_parties,
                &[1, 4, 7],
                frost_public,
                &payment,
                &context,
                ReserveMandateRef::Taker(mandate),
            )?,
            payment,
            context,
        };
        let asset_link = prove_asset_link(
            &issuer.key,
            mandate.reserve_asset_id,
            &typed.payment.asset_commitment,
            &payment_openings.asset,
            &mut OsRng,
        )?;
        let mut authorization = ReservationAuthorization {
            role: ReservationRole::Taker,
            entity_commitment: mandate.entity_commitment,
            asset_id: mandate.reserve_asset_id,
            direction: mandate.direction as u8,
            authorization_digest: mandate_digest,
            mandate_digest,
            typed_reserve_digest: Sha256::digest(typed_wire::encode(&typed)).into(),
            reserve_nullifier: typed.payment.nullifier(),
            asset_link_proof_digest: asset_link
                .digest(&mandate.reserve_asset_id, &typed.payment.asset_commitment),
            limit_price_commitment: mandate.limit_price_commitment,
            escrow_digest: ZERO,
            rfq_nullifier: mandate.rfq_nullifier,
            policy_version: 0,
            admission_ticket_id: ordered.ticket_id,
            admission_slot: ordered.slot,
            admission_receipt_digest: ordered.certified_digest()?,
            admission_epoch: ordered.epoch,
            admission_sequence: ordered.sequence,
            admission_batch_id: admission_receipt.batch_id,
        };
        authorization.escrow_digest = escrow.statement(&transition, &authorization)?;
        let identity = IdentityEvidence {
            presentation,
            registry,
            trusted_issuer,
            scope: identity_scope,
            context: identity_context,
            required_cohort,
        };
        verify_taker_reservation(
            &transition,
            &authorization,
            &typed,
            mandate,
            &ordered,
            &identity,
            now,
        )?;
        let venue = Venue::new(issuer.key.clone(), &issuer.bounds, frost_public.clone());
        verify_note_reservation(
            &transition,
            &relation_proof,
            &authorization,
            &escrow,
            &typed,
            &venue,
            &asset_link,
            Some(&ordered),
            now,
        )?;
        let reserve_receipt_digest = authorization.statement(&transition)?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            reserve_receipt_digest,
            before,
        )?;
        let accepted = bridge.reserve_product(&transition, &authorization, &escrow, &approval)?;
        let reservation = bridge.note_reservation(mandate.reserve_id)?;
        if accepted.after_root != reservation.state_root
            || reservation.status != "active"
            || reservation.escrow_note_id != escrow.escrow_note_id
            || reservation.delegation_digest != delegation_digest
        {
            return Err("DeFMI accepted a different Taker note reservation".into());
        }
        Ok(TakerNoteReservationReceipt {
            reservation,
            facility_id,
            reserve_receipt_digest,
        })
    }

    /// Release an admitted Taker reservation as soon as the registered seven
    /// node committee certifies zero fills. No post-match Taker signature is
    /// requested: the pre-RFQ mandate already committed the one-time fill mask,
    /// and every validator checks its opening plus all resident signatures.
    #[allow(clippy::too_many_arguments)]
    pub fn release_taker_no_fill(
        &self,
        snapshot: &ParticipantSnapshot,
        mandate: &TakerExecutionMandate,
        reservation_receipt: &TakerNoteReservationReceipt,
        admission: &CertifiedAdmissionLane,
        admission_receipt: &DefmiAdmissionReceipt,
        maximum_amount: u64,
        maximum_blinding: u64,
        evidence: &MpcNoFillEvidence,
        now: u64,
    ) -> Result<CanonicalNoteReservation, String> {
        evidence.validate_encoding()?;
        mandate.verify_signature_at(now)?;
        if evidence.mandate()?.digest()? != mandate.digest()?
            || snapshot.role != "taker"
            || snapshot.participant_id == ZERO
            || mandate.venue_id != self.venue_id
            || mandate.defmi_id != self.defmi_id
            || mandate.reserve_id != reservation_receipt.reservation.hold_id
            || mandate.reserve_asset_id != reservation_receipt.reservation.asset_id
            || mandate.maximum_amount_commitment
                != reservation_receipt.reservation.amount_commitment
            || mandate.admission_slot != admission.slot
            || admission.sequence == 0
            || admission_receipt.epoch != self.epoch
            || admission_receipt.admission_digest != admission.digest(self.venue_id, self.epoch)?
        {
            return Err(
                "no-fill release differs from its participant, reserve, or admission".into(),
            );
        }
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let canonical_reservation = bridge.note_reservation(mandate.reserve_id)?;
        if canonical_reservation.status == "released" {
            return Ok(canonical_reservation);
        }
        if canonical_reservation.status != "active"
            || canonical_reservation.escrow_note_id
                != reservation_receipt.reservation.escrow_note_id
            || canonical_reservation.asset_id != mandate.reserve_asset_id
            || canonical_reservation.amount_commitment != mandate.maximum_amount_commitment
            || canonical_reservation.reserve_receipt_digest
                != reservation_receipt.reserve_receipt_digest
        {
            return Err("canonical Taker reservation is not available for a no-fill refund".into());
        }

        let key = Pedersen::new(b"qomm:defmi:v1");
        let amount_blinding = Scalar::from(maximum_blinding);
        if maximum_amount == 0
            || key
                .commit_u64(maximum_amount, &amount_blinding)
                .compress()
                .to_bytes()
                != mandate.maximum_amount_commitment
        {
            return Err("no-fill refund opening differs from the signed Taker maximum".into());
        }
        let facility = bridge.credit_facility(reservation_receipt.facility_id)?;
        let hold = bridge.credit_hold(mandate.reserve_id)?;
        let proof_root = self.rpc.state_root()?;
        if facility.state_root != proof_root
            || hold.state_root != proof_root
            || canonical_reservation.state_root != proof_root
            || hold.status != "active"
            || hold.facility_id != reservation_receipt.facility_id
            || hold.query_commitment != mandate.digest()?
            || hold.amount_commitment != mandate.maximum_amount_commitment
            || hold.expires_at != mandate.deadline
        {
            return Err("Taker refund was prepared over stale or different DeFMI state".into());
        }
        let cap_amount = match mandate.direction {
            Direction::TakerBuys => snapshot.cash,
            Direction::TakerSells => snapshot.inventory,
        };
        if maximum_amount > cap_amount {
            return Err("Taker reserve exceeds its aggregate facility".into());
        }
        let cap_blinding = deterministic_scalar(&[
            b"QOMM:DEMO:TAKER-FACILITY-CAP-BLINDING:v1",
            &reservation_receipt.facility_id,
        ]);
        // Only the cap is opened here.  Earlier settled RFQs have moved value
        // from this facility's available balance into its outstanding balance,
        // so the release is expressed homomorphically against the canonical
        // before-commitments instead of assuming an untouched facility.
        if facility.facility.cap_commitment
            != key
                .commit_u64(cap_amount, &cap_blinding)
                .compress()
                .to_bytes()
        {
            return Err("Taker refund cannot open the aggregate facility state".into());
        }

        let covenant_scalar = |label: &[u8]| {
            let mut value = Scalar::from_bytes_mod_order(
                Sha256::new()
                    .chain_update(b"QOMM:DEMO:NOTE-RESERVATION-ADDRESS:v1")
                    .chain_update(label)
                    .chain_update(mandate.reserve_id)
                    .finalize()
                    .into(),
            );
            if value == Scalar::ZERO {
                value = Scalar::ONE;
            }
            value
        };
        let covenant = Wallet::from_parts(covenant_scalar(b"view"), covenant_scalar(b"spend"));
        let (note_root, ledger, canonical_notes) =
            bridge.note_ledger(mandate.reserve_asset_id, key.clone(), 64, 16_384)?;
        if note_root != proof_root {
            return Err("Taker refund note pool changed after its facility snapshot".into());
        }
        let escrow_index = canonical_notes
            .iter()
            .position(|output| output.note_id == canonical_reservation.escrow_note_id)
            .ok_or_else(|| "Taker escrow note is absent from canonical DeFMI state".to_string())?;
        let opening = ledger
            .scan(&covenant, &key)
            .into_iter()
            .find_map(|(index, opening)| (index == escrow_index).then_some(opening))
            .ok_or_else(|| "automatic covenant cannot open the Taker escrow note".to_string())?;
        if opening.value != maximum_amount || opening.blinding != amount_blinding {
            return Err("Taker escrow note differs from the pre-RFQ reserve opening".into());
        }
        let serial = (G * opening.serial).compress().to_bytes();
        let serial_status = self.rpc.note_serial_snapshot(serial)?;
        if serial_status.state_root != proof_root || serial_status.spent {
            return Err("Taker escrow serial was already consumed".into());
        }
        let decoy_index = canonical_notes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != escrow_index)
            .min_by_key(|(_, output)| output.note_id)
            .map(|(index, _)| index)
            .ok_or_else(|| "Taker refund has no canonical anonymity decoy".to_string())?;
        let mut ring = vec![escrow_index, decoy_index];
        ring.sort_by_key(|index| canonical_notes[*index].note_id);
        let ring_locks = ring
            .iter()
            .map(|index| canonical_notes[*index].lock_id)
            .collect::<Vec<_>>();
        let eligibility = ring_locks
            .iter()
            .map(|lock| *lock == mandate.reserve_id)
            .collect::<Vec<_>>();
        let recipient = Address {
            view: CompressedRistretto(snapshot.note_view_public)
                .decompress()
                .ok_or_else(|| "Taker refund view key is not canonical".to_string())?,
            spend: CompressedRistretto(snapshot.note_spend_public)
                .decompress()
                .ok_or_else(|| "Taker refund spend key is not canonical".to_string())?,
        };
        let evidence_digest = evidence.digest()?;
        let output_blinding = deterministic_scalar(&[
            b"QOMM:DEMO:NO-FILL-REFUND-BLINDING:v1",
            &mandate.reserve_id,
            &evidence_digest,
        ]);
        let context = [
            b"QOMM:DEMO:NOTE-NO-FILL-RELEASE:v1".as_slice(),
            &mandate.reserve_id,
            &evidence_digest,
        ]
        .concat();
        let mut proof_rng = StdRng::from_seed(hash_parts(&[
            b"QOMM:DEMO:NO-FILL-REFUND-PROOF-RNG:v1",
            &mandate.reserve_id,
            &evidence_digest,
        ]));
        let generated = ledger
            .build_spend_constrained_with_blindings(
                &ring,
                escrow_index,
                &opening,
                &key.g,
                &Scalar::ZERO,
                &[(recipient, maximum_amount)],
                &[output_blinding],
                &eligibility,
                &context,
                &mut proof_rng,
            )
            .map_err(str::to_string)?;
        let spend = NoteSpend::from_verified(
            &ledger,
            &ring,
            &generated.proof,
            &generated.notes,
            mandate.reserve_asset_id,
            &ring_locks,
            mandate.reserve_id,
            &[ZERO],
            &context,
            &mut proof_rng,
        )?;

        let transition = CreditFacilityTransition {
            operation_id: hash_parts(&[
                b"QOMM:DEMO:TAKER-NO-FILL-RELEASE-OP:v1",
                &mandate.reserve_id,
                &evidence_digest,
            ]),
            facility_id: reservation_receipt.facility_id,
            hold_id: mandate.reserve_id,
            kind: CreditTransitionKind::Release,
            query_commitment: mandate.digest()?,
            amount_commitment: mandate.maximum_amount_commitment,
            consumed_commitment: ZERO,
            refund_commitment: ZERO,
            before_available_commitment: facility.facility.available_commitment,
            after_available_commitment: shifted_commitment(
                facility.facility.available_commitment,
                mandate.maximum_amount_commitment,
                true,
            )?,
            before_held_commitment: facility.facility.held_commitment,
            after_held_commitment: shifted_commitment(
                facility.facility.held_commitment,
                mandate.maximum_amount_commitment,
                false,
            )?,
            before_outstanding_commitment: facility.facility.outstanding_commitment,
            after_outstanding_commitment: facility.facility.outstanding_commitment,
            before_sequence: facility.facility.sequence,
            expires_at: mandate.deadline,
            settlement_digest: ZERO,
            relation_proof_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-NO-FILL-CREDIT-RELATION:v1",
                &mandate.reserve_id,
                &evidence_digest,
            ]),
        };
        transition.body()?;
        let release_deadline = mandate
            .deadline
            .checked_add(3_600)
            .ok_or_else(|| "Taker no-fill release deadline overflowed".to_string())?;
        let release = ProductNoteReleaseOrder {
            transition,
            role: ReservationRole::Taker,
            reserve_receipt_digest: reservation_receipt.reserve_receipt_digest,
            typed_instruction_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-NO-FILL-TYPED-EVIDENCE:v1",
                &evidence_digest,
            ]),
            release_nullifier: hash_parts(&[
                b"QOMM:DEMO:TAKER-NO-FILL-RELEASE-NULLIFIER:v1",
                &mandate.rfq_nullifier,
            ]),
            release_deadline,
            asset_id: mandate.reserve_asset_id,
            asset_link_proof_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-NO-FILL-ASSET-LINK:v1",
                &mandate.reserve_asset_id,
                &evidence_digest,
            ]),
            escrow_note_id: canonical_reservation.escrow_note_id,
            spend,
        };
        let order = ProductNoteNoFillReleaseOrder {
            release,
            venue_id: self.venue_id,
            defmi_id: self.defmi_id,
            admission_epoch: admission_receipt.epoch,
            admission_sequence: admission.sequence,
            no_fill_evidence_digest: evidence_digest,
        };
        let statement = order.statement()?;
        if self.rpc.state_root()? != proof_root {
            return Err("DeFMI changed before the Taker no-fill refund was authorized".into());
        }
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            statement,
            proof_root,
        )?;
        let accepted = bridge.release_product_no_fill(&order, evidence, &approval)?;
        let released = bridge.note_reservation(mandate.reserve_id)?;
        let released_hold = bridge.credit_hold(mandate.reserve_id)?;
        let released_facility = bridge.credit_facility(reservation_receipt.facility_id)?;
        if accepted.after_root != released.state_root
            || released.state_root != released_hold.state_root
            || released.state_root != released_facility.state_root
            || released.status != "released"
            || released.settlement_digest != statement
            || released_hold.status != "released"
            || released_hold.settlement_digest != ZERO
            || released_facility.facility.available_commitment
                != released_facility.facility.cap_commitment
            || released_facility.facility.held_commitment != ZERO
            || released_facility.facility.sequence != facility.facility.sequence.saturating_add(1)
        {
            return Err("DeFMI accepted a different Taker no-fill refund".into());
        }
        Ok(released)
    }

    /// Refund an anonymous Taker covenant after its signed RFQ deadline. The
    /// expiry is the authorization: neither trading party nor the MPC
    /// coordinator can block the refund by withholding a post-match signature.
    pub fn release_expired_taker(
        &self,
        snapshot: &ParticipantSnapshot,
        mandate: &TakerExecutionMandate,
        maximum_amount: u64,
        maximum_blinding: u64,
        now: u64,
    ) -> Result<CanonicalNoteReservation, String> {
        mandate.verify_signature()?;
        let release_deadline = AUTOMATIC_EXPIRY_RELEASE_VALID_UNTIL;
        if now <= mandate.deadline {
            return Err("Taker reservation has not reached its expiry-release time".into());
        }
        if snapshot.role != "taker" {
            return Err("expired release was not submitted by a Taker participant".into());
        }
        if snapshot.participant_id == ZERO {
            return Err("expired release has no Taker participant identity".into());
        }
        // The MPC service id is deliberately upgradeable.  A queued request
        // remains bound to the non-zero venue that signed it, while the
        // canonical DeFMI hold and its query commitment prevent substitution.
        // Requiring the retired service id to equal the current one would
        // strand valid reservations after every gateway deployment.
        if mandate.venue_id == ZERO {
            return Err("expired release has no signed venue".into());
        }
        if mandate.defmi_id != self.defmi_id {
            return Err("expired release targets another DeFMI domain".into());
        }
        if mandate.taker_public
            != *snapshot
                .public_keys
                .get("settlement")
                .ok_or_else(|| "Taker participant has no settlement key".to_string())?
        {
            return Err("expired release uses another Taker settlement key".into());
        }
        if maximum_amount == 0 {
            return Err("expired release has an empty signed maximum".into());
        }
        let key = Pedersen::new(b"qomm:defmi:v1");
        let amount_blinding = Scalar::from(maximum_blinding);
        if key
            .commit_u64(maximum_amount, &amount_blinding)
            .compress()
            .to_bytes()
            != mandate.maximum_amount_commitment
        {
            return Err("expired release opening differs from the signed maximum".into());
        }
        let facility_id = hash_parts(&[
            b"QOMM:DEMO:TAKER-FACILITY:v1",
            &self.defmi_id,
            &mandate.entity_commitment,
            &mandate.reserve_asset_id,
        ]);
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let canonical_reservation = bridge.note_reservation(mandate.reserve_id)?;
        if canonical_reservation.status == "released" {
            return Ok(canonical_reservation);
        }
        if canonical_reservation.status != "active"
            || canonical_reservation.asset_id != mandate.reserve_asset_id
            || canonical_reservation.amount_commitment != mandate.maximum_amount_commitment
            || canonical_reservation.reserve_receipt_digest == ZERO
        {
            return Err("canonical Taker reservation is not available for expiry refund".into());
        }
        let facility = bridge.credit_facility(facility_id)?;
        let hold = bridge.credit_hold(mandate.reserve_id)?;
        let proof_root = self.rpc.state_root()?;
        if facility.state_root != proof_root
            || hold.state_root != proof_root
            || canonical_reservation.state_root != proof_root
            || hold.status != "active"
            || hold.facility_id != facility_id
            || hold.query_commitment != mandate.digest()?
            || hold.amount_commitment != mandate.maximum_amount_commitment
            || hold.expires_at != mandate.deadline
        {
            return Err("expired refund was prepared over stale or different DeFMI state".into());
        }
        let cap_amount = match mandate.direction {
            Direction::TakerBuys => snapshot.cash,
            Direction::TakerSells => snapshot.inventory,
        };
        if maximum_amount > cap_amount {
            return Err("Taker reserve exceeds its aggregate facility".into());
        }
        let cap_blinding =
            deterministic_scalar(&[b"QOMM:DEMO:TAKER-FACILITY-CAP-BLINDING:v1", &facility_id]);
        // Only the cap is opened here.  Earlier settled RFQs have moved value
        // from this facility's available balance into its outstanding balance,
        // so the release is expressed homomorphically against the canonical
        // before-commitments instead of assuming an untouched facility.
        if facility.facility.cap_commitment
            != key
                .commit_u64(cap_amount, &cap_blinding)
                .compress()
                .to_bytes()
        {
            return Err("expired refund cannot open the aggregate facility state".into());
        }
        let covenant_scalar = |label: &[u8]| {
            let mut value = Scalar::from_bytes_mod_order(
                Sha256::new()
                    .chain_update(b"QOMM:DEMO:NOTE-RESERVATION-ADDRESS:v1")
                    .chain_update(label)
                    .chain_update(mandate.reserve_id)
                    .finalize()
                    .into(),
            );
            if value == Scalar::ZERO {
                value = Scalar::ONE;
            }
            value
        };
        let covenant = Wallet::from_parts(covenant_scalar(b"view"), covenant_scalar(b"spend"));
        let (note_root, ledger, canonical_notes) =
            bridge.note_ledger(mandate.reserve_asset_id, key.clone(), 64, 16_384)?;
        if note_root != proof_root {
            return Err("expired refund note pool changed after its facility snapshot".into());
        }
        let escrow_index = canonical_notes
            .iter()
            .position(|output| output.note_id == canonical_reservation.escrow_note_id)
            .ok_or_else(|| "Taker escrow note is absent from canonical DeFMI state".to_string())?;
        let opening = ledger
            .scan(&covenant, &key)
            .into_iter()
            .find_map(|(index, opening)| (index == escrow_index).then_some(opening))
            .ok_or_else(|| "automatic covenant cannot open the expired Taker escrow".to_string())?;
        if opening.value != maximum_amount || opening.blinding != amount_blinding {
            return Err("expired escrow differs from the pre-RFQ reserve opening".into());
        }
        let serial = (G * opening.serial).compress().to_bytes();
        let serial_status = self.rpc.note_serial_snapshot(serial)?;
        if serial_status.state_root != proof_root || serial_status.spent {
            return Err("expired Taker escrow serial was already consumed".into());
        }
        let decoy_index = canonical_notes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != escrow_index)
            .min_by_key(|(_, output)| output.note_id)
            .map(|(index, _)| index)
            .ok_or_else(|| "expired Taker refund has no anonymity decoy".to_string())?;
        let mut ring = vec![escrow_index, decoy_index];
        ring.sort_by_key(|index| canonical_notes[*index].note_id);
        let ring_locks = ring
            .iter()
            .map(|index| canonical_notes[*index].lock_id)
            .collect::<Vec<_>>();
        let eligibility = ring_locks
            .iter()
            .map(|lock| *lock == mandate.reserve_id)
            .collect::<Vec<_>>();
        let recipient = Address {
            view: CompressedRistretto(snapshot.note_view_public)
                .decompress()
                .ok_or_else(|| "Taker refund view key is not canonical".to_string())?,
            spend: CompressedRistretto(snapshot.note_spend_public)
                .decompress()
                .ok_or_else(|| "Taker refund spend key is not canonical".to_string())?,
        };
        let expiry_digest = hash_parts(&[
            b"QOMM:DEMO:TAKER-EXPIRY-RELEASE:v1",
            &mandate.reserve_id,
            &mandate.deadline.to_be_bytes(),
        ]);
        let output_blinding = deterministic_scalar(&[
            b"QOMM:DEMO:EXPIRY-REFUND-BLINDING:v1",
            &mandate.reserve_id,
            &expiry_digest,
        ]);
        let context = [
            b"QOMM:DEMO:NOTE-EXPIRY-RELEASE:v1".as_slice(),
            &mandate.reserve_id,
            &expiry_digest,
        ]
        .concat();
        let mut proof_rng = StdRng::from_seed(hash_parts(&[
            b"QOMM:DEMO:EXPIRY-REFUND-PROOF-RNG:v1",
            &mandate.reserve_id,
            &expiry_digest,
        ]));
        let generated = ledger
            .build_spend_constrained_with_blindings(
                &ring,
                escrow_index,
                &opening,
                &key.g,
                &Scalar::ZERO,
                &[(recipient, maximum_amount)],
                &[output_blinding],
                &eligibility,
                &context,
                &mut proof_rng,
            )
            .map_err(str::to_string)?;
        let spend = NoteSpend::from_verified(
            &ledger,
            &ring,
            &generated.proof,
            &generated.notes,
            mandate.reserve_asset_id,
            &ring_locks,
            mandate.reserve_id,
            &[ZERO],
            &context,
            &mut proof_rng,
        )?;
        let transition = CreditFacilityTransition {
            operation_id: hash_parts(&[
                b"QOMM:DEMO:TAKER-EXPIRY-RELEASE-OP:v1",
                &mandate.reserve_id,
            ]),
            facility_id,
            hold_id: mandate.reserve_id,
            kind: CreditTransitionKind::Release,
            query_commitment: mandate.digest()?,
            amount_commitment: mandate.maximum_amount_commitment,
            consumed_commitment: ZERO,
            refund_commitment: ZERO,
            before_available_commitment: facility.facility.available_commitment,
            after_available_commitment: shifted_commitment(
                facility.facility.available_commitment,
                mandate.maximum_amount_commitment,
                true,
            )?,
            before_held_commitment: facility.facility.held_commitment,
            after_held_commitment: shifted_commitment(
                facility.facility.held_commitment,
                mandate.maximum_amount_commitment,
                false,
            )?,
            before_outstanding_commitment: facility.facility.outstanding_commitment,
            after_outstanding_commitment: facility.facility.outstanding_commitment,
            before_sequence: facility.facility.sequence,
            expires_at: mandate.deadline,
            settlement_digest: ZERO,
            relation_proof_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-EXPIRY-CREDIT-RELATION:v1",
                &mandate.reserve_id,
            ]),
        };
        transition.body()?;
        let order = ProductNoteReleaseOrder {
            transition,
            role: ReservationRole::Taker,
            reserve_receipt_digest: canonical_reservation.reserve_receipt_digest,
            typed_instruction_digest: expiry_digest,
            release_nullifier: hash_parts(&[
                b"QOMM:DEMO:TAKER-EXPIRY-RELEASE-NULLIFIER:v1",
                &mandate.rfq_nullifier,
            ]),
            release_deadline,
            asset_id: mandate.reserve_asset_id,
            asset_link_proof_digest: hash_parts(&[
                b"QOMM:DEMO:TAKER-EXPIRY-ASSET-LINK:v1",
                &mandate.reserve_asset_id,
                &expiry_digest,
            ]),
            escrow_note_id: canonical_reservation.escrow_note_id,
            spend,
        };
        let statement = order.statement()?;
        if self.rpc.state_root()? != proof_root {
            return Err("DeFMI changed before the expiry refund was authorized".into());
        }
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            statement,
            proof_root,
        )?;
        let accepted = bridge.release_product(&order, &approval)?;
        let released = bridge.note_reservation(mandate.reserve_id)?;
        let released_hold = bridge.credit_hold(mandate.reserve_id)?;
        let released_facility = bridge.credit_facility(facility_id)?;
        if accepted.after_root != released.state_root
            || released.state_root != released_hold.state_root
            || released.state_root != released_facility.state_root
            || released.status != "released"
            || released.settlement_digest != statement
            || released_hold.status != "released"
            || released_facility.facility.available_commitment
                != shifted_commitment(
                    facility.facility.available_commitment,
                    mandate.maximum_amount_commitment,
                    true,
                )?
            || released_facility.facility.held_commitment
                != shifted_commitment(
                    facility.facility.held_commitment,
                    mandate.maximum_amount_commitment,
                    false,
                )?
            || released_facility.facility.outstanding_commitment
                != facility.facility.outstanding_commitment
        {
            return Err("DeFMI accepted a different Taker expiry refund".into());
        }
        Ok(released)
    }

    /// Materialize one signed Maker maximum as an anonymous parent note on
    /// the canonical DeFMI ledger. The Maker participant service creates the
    /// spend proof with its own view/spend keys; this coordinator sees only
    /// the public proof and independently verifies it against one immutable
    /// Avalanche root before governance signs the registration.
    pub fn ensure_maker_standing_pool(
        &self,
        request: MakerStandingPoolRequest<'_>,
    ) -> Result<MakerStandingPoolReceipt, String> {
        let MakerStandingPoolRequest {
            participant,
            snapshot,
            mandate,
            maximum_amount,
            maximum_blinding,
            now,
            existing_only,
        } = request;
        mandate.verify_signature_at(now)?;
        if snapshot.role != "maker"
            || snapshot.participant_id == ZERO
            || mandate.venue_id != self.venue_id
            || mandate.defmi_id != self.defmi_id
            || mandate.maker_public
                != *snapshot
                    .public_keys
                    .get("quote")
                    .ok_or_else(|| "Maker participant has no quote key".to_string())?
            || maximum_amount == 0
        {
            return Err("Maker pool request differs from its signed participant scope".into());
        }
        let participant_capacity = match mandate.direction {
            qomm_transport::mandate::Direction::TakerBuys => snapshot.inventory,
            qomm_transport::mandate::Direction::TakerSells => snapshot.cash,
        };
        if maximum_amount > participant_capacity {
            return Err(
                "Maker signed maximum exceeds the participant-owned inventory or cash capacity"
                    .into(),
            );
        }
        let key = Pedersen::new(b"qomm:defmi:v1");
        let amount_blinding = Scalar::from(maximum_blinding);
        let expected_maximum = key
            .commit_u64(maximum_amount, &amount_blinding)
            .compress()
            .to_bytes();
        if expected_maximum != mandate.maximum_amount_commitment {
            return Err("Maker pool opening differs from the signed maximum".into());
        }
        let mandate_digest = mandate.digest()?;
        let pool_id = standing_note_pool_id(
            mandate.entity_commitment,
            mandate.policy_digest,
            mandate_digest,
            mandate.asset_id,
            mandate.direction as u8,
        )?;
        // DeFMI permits one aggregate guarantor facility for one legal entity
        // and rail.  Quote policies are replaceable children of that stable
        // capacity; creating another facility for every price tick would
        // duplicate the Maker's guarantee.
        let facility_id = Self::maker_facility_id(mandate.entity_commitment, mandate.asset_id)?;
        let facility_domain = hash_parts(&[
            b"QOMM:DEMO:MAKER-FACILITY-BLINDING-DOMAIN:v1",
            &self.defmi_id,
            &mandate.entity_commitment,
            &mandate.asset_id,
        ]);
        let facility_statement = hash_parts(&[
            b"QOMM:DEMO:MAKER-FACILITY-CAP:v1",
            &facility_id,
            &participant_capacity.to_be_bytes(),
        ]);
        let facility_approval = participant.entity_approval(
            snapshot,
            facility_domain,
            KeyPurpose::Quote,
            facility_statement,
        )?;
        let mut facility_blinding = Scalar::from_bytes_mod_order(hash_parts(&[
            b"QOMM:DEMO:MAKER-FACILITY-BLINDING:v1",
            &facility_approval.participant_id,
            &facility_approval.statement,
            &facility_approval.signature,
        ]));
        if facility_blinding == Scalar::ZERO {
            facility_blinding = Scalar::ONE;
        }
        match self.standing_note_pool(pool_id) {
            Ok(existing) => {
                // The market epoch is derived from the whole quote registry, so
                // another Maker's policy change moves the epoch while this pool
                // is unchanged.  The pool stays valid as long as the committee
                // that delegated it is the committee registered for the current
                // epoch; the VM verifies its allocations under the pool's own
                // epoch and the quote proof under the order's epoch.
                if existing.committee_epoch != self.epoch {
                    let current = self
                        .rpc
                        .settlement_verifier_snapshot(self.venue_id, self.epoch)?;
                    let delegated = self
                        .rpc
                        .settlement_verifier_snapshot(self.venue_id, existing.committee_epoch)?;
                    if delegated.config.frost_public_package != current.config.frost_public_package
                        || delegated.config.defmi_id != current.config.defmi_id
                        || delegated.config.venue_id != current.config.venue_id
                    {
                        return Err(
                            "canonical standing pool was delegated to another proof committee"
                                .into(),
                        );
                    }
                }
                verify_existing_pool(
                    &existing,
                    mandate,
                    mandate_digest,
                    expected_maximum,
                    existing.committee_epoch,
                )?;
                self.verify_or_create_facility(
                    facility_id,
                    mandate,
                    participant_capacity,
                    facility_blinding,
                    !existing_only,
                )?;
                return Ok(MakerStandingPoolReceipt {
                    source_note_id: existing.current_pool_note_id,
                    pool: existing,
                    facility_id,
                });
            }
            Err(error) if not_found(&error) && existing_only => {
                return Err(
                    "queued RFQ refers to a Maker standing pool that was not registered before the outage"
                        .into(),
                )
            }
            Err(error) if not_found(&error) => {}
            Err(error) => return Err(error),
        }

        let asset = AssetDefinition {
            asset_id: mandate.asset_id,
            code: match mandate.direction {
                qomm_transport::mandate::Direction::TakerBuys => {
                    format!("QOMM-DEMO-SEC-{}", &hex::encode(mandate.asset_id)[..8])
                }
                qomm_transport::mandate::Direction::TakerSells => "QOMM-DEMO-CASH".into(),
            },
            kind: match mandate.direction {
                qomm_transport::mandate::Direction::TakerBuys => AssetKind::Security,
                qomm_transport::mandate::Direction::TakerSells => AssetKind::Cash,
            },
            decimals: 0,
            terms_digest: hash_parts(&[b"QOMM:DEMO:ASSET-TERMS:v1", &mandate.asset_id]),
        };
        self.verify_or_register_asset(&asset)?;

        let csd_key = SigningKey::from_bytes(&hash_parts(&[
            b"QOMM:DEMO:CSD-ISSUER-KEY:v1",
            &self.defmi_id,
            &mandate.asset_id,
        ]));
        let csd = CsdIssuerDefinition {
            issuer_id: hash_parts(&[
                b"QOMM:DEMO:CSD-ISSUER:v1",
                &self.defmi_id,
                &mandate.asset_id,
            ]),
            code: format!("DEFMI-DEMO-CSD-{}", &hex::encode(mandate.asset_id)[..8]),
            jurisdiction: "JP".into(),
            operator_entity_commitment: hash_parts(&[b"QOMM:DEMO:CSD-OPERATOR:v1", &self.defmi_id]),
            public_key: csd_key.verifying_key().to_bytes(),
            permitted_asset_ids: vec![mandate.asset_id],
            policy_digest: hash_parts(&[
                b"QOMM:DEMO:CSD-POLICY:v1",
                &self.defmi_id,
                &mandate.asset_id,
            ]),
            valid_from: 1,
            valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
        };
        self.verify_or_register_csd(&csd)?;
        self.verify_or_create_facility(
            facility_id,
            mandate,
            participant_capacity,
            facility_blinding,
            true,
        )?;

        let source = self.verify_or_issue_note(
            snapshot,
            mandate.asset_id,
            maximum_amount,
            amount_blinding,
            pool_id,
            false,
            &csd,
            &csd_key,
            now,
        )?;
        self.verify_or_issue_note(
            snapshot,
            mandate.asset_id,
            maximum_amount
                .checked_add(1)
                .ok_or_else(|| "Maker decoy amount overflowed".to_string())?,
            deterministic_scalar(&[b"QOMM:DEMO:MAKER-DECOY-BLINDING:v1", &pool_id]),
            pool_id,
            true,
            &csd,
            &csd_key,
            now,
        )?;

        let evidence = participant.create_standing_pool_spend(
            snapshot,
            mandate.asset_id,
            source.note_id,
            pool_id,
            expected_maximum,
        )?;
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let (proof_root, ledger, canonical) =
            bridge.note_ledger(mandate.asset_id, key, 64, 16_384)?;
        if evidence.state_root != proof_root {
            return Err("Maker proof was built over another DeFMI root".into());
        }
        let ring = evidence
            .ring
            .iter()
            .map(|note_id| {
                canonical
                    .iter()
                    .position(|output| output.note_id == *note_id)
                    .ok_or_else(|| {
                        "Maker proof ring is absent from canonical DeFMI state".to_string()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ring_locks = ring
            .iter()
            .map(|index| canonical[*index].lock_id)
            .collect::<Vec<_>>();
        let notes = evidence
            .outputs
            .iter()
            .map(NoteOutput::to_note)
            .collect::<Result<Vec<_>, _>>()?;
        let spend = NoteSpend::from_verified(
            &ledger,
            &ring,
            &evidence.proof,
            &notes,
            mandate.asset_id,
            &ring_locks,
            ZERO,
            &[pool_id, ZERO],
            &evidence.context,
            &mut OsRng,
        )?;
        if spend.ring != evidence.ring || spend.outputs != evidence.outputs {
            return Err("Maker proof projection differs from the participant evidence".into());
        }
        let delegation_digest = standing_note_pool_delegation_digest(
            pool_id,
            self.venue_id,
            self.defmi_id,
            self.epoch,
            mandate.valid_until,
        )?;
        let registration = StandingNotePoolRegistration {
            operation_id: hash_parts(&[b"QOMM:DEMO:STANDING-POOL-OP:v1", &pool_id]),
            pool_id,
            venue_id: self.venue_id,
            defmi_id: self.defmi_id,
            entity_commitment: mandate.entity_commitment,
            policy_digest: mandate.policy_digest,
            mandate_digest,
            asset_id: mandate.asset_id,
            direction: mandate.direction as u8,
            maximum_amount_commitment: expected_maximum,
            pool_note_id: spend.outputs[0].note_id,
            delegation_digest,
            committee_epoch: self.epoch,
            valid_until: mandate.valid_until,
            spend,
        };
        let before = self.rpc.state_root()?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            registration.statement()?,
            before,
        )?;
        bridge.register_standing_note_pool(&registration, &approval)?;
        let pool = self.standing_note_pool(pool_id)?;
        verify_existing_pool(&pool, mandate, mandate_digest, expected_maximum, self.epoch)?;
        Ok(MakerStandingPoolReceipt {
            pool,
            facility_id,
            source_note_id: source.note_id,
        })
    }

    fn verify_or_register_asset(&self, asset: &AssetDefinition) -> Result<(), String> {
        match self.rpc.asset_snapshot(asset.asset_id) {
            Ok(existing) => {
                if existing.definition != *asset || !existing.active {
                    return Err("canonical DeFMI asset differs from the Maker mandate rail".into());
                }
                Ok(())
            }
            Err(error) if not_found(&error) => {
                let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    asset.statement()?,
                    before,
                )?;
                bridge.register_asset(asset, &approval)?;
                let existing = self.rpc.asset_snapshot(asset.asset_id)?;
                if existing.definition != *asset || !existing.active {
                    return Err("registered DeFMI asset differs from canonical state".into());
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn verify_or_register_csd(&self, issuer: &CsdIssuerDefinition) -> Result<(), String> {
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        match bridge.csd_issuer(issuer.issuer_id) {
            Ok(existing) => {
                if existing.definition != *issuer || existing.status != "active" {
                    return Err("canonical CSD issuer differs from the demo issuer".into());
                }
                Ok(())
            }
            Err(error) if not_found(&error) => {
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    issuer.statement()?,
                    before,
                )?;
                bridge.register_csd_issuer(issuer, &approval)?;
                let existing = bridge.csd_issuer(issuer.issuer_id)?;
                if existing.definition != *issuer || existing.status != "active" {
                    return Err("registered CSD issuer differs from canonical state".into());
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn verify_or_create_facility(
        &self,
        facility_id: [u8; 32],
        mandate: &MakerPolicyMandate,
        maximum_amount: u64,
        maximum_blinding: Scalar,
        allow_create: bool,
    ) -> Result<CanonicalCreditFacility, String> {
        let guarantor_key = SigningKey::from_bytes(&hash_parts(&[
            b"QOMM:DEMO:MAKER-GUARANTOR-KEY:v1",
            &self.defmi_id,
            &mandate.entity_commitment,
            &mandate.asset_id,
        ]));
        let guarantor = GuarantorDefinition {
            guarantor_id: hash_parts(&[
                b"QOMM:DEMO:MAKER-GUARANTOR:v1",
                &self.defmi_id,
                &mandate.entity_commitment,
                &mandate.asset_id,
            ]),
            kind: GuarantorKind::SelfGuaranteed,
            name: format!(
                "Maker reserve {}",
                &hex::encode(mandate.entity_commitment)[..12]
            ),
            public_key: guarantor_key.verifying_key().to_bytes(),
            risk_policy_digest: hash_parts(&[
                b"QOMM:DEMO:MAKER-RISK-POLICY:v1",
                &self.defmi_id,
                &mandate.asset_id,
            ]),
        };
        match self.rpc.guarantor_snapshot(guarantor.guarantor_id) {
            Ok(existing) => {
                if existing.definition != guarantor || !existing.active {
                    return Err("canonical guarantor differs from the Maker facility".into());
                }
            }
            Err(error) if not_found(&error) && !allow_create => return Err(
                "queued RFQ refers to a Maker guarantor that was not registered before the outage"
                    .into(),
            ),
            Err(error) if not_found(&error) => {
                let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    guarantor.statement()?,
                    before,
                )?;
                bridge.register_guarantor(&guarantor, &approval)?;
            }
            Err(error) => return Err(error),
        }
        let expected_cap = Pedersen::new(b"qomm:defmi:v1")
            .commit_u64(maximum_amount, &maximum_blinding)
            .compress()
            .to_bytes();
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        match bridge.credit_facility(facility_id) {
            Ok(existing) => {
                let facility = &existing.facility;
                if facility.guarantor_id != guarantor.guarantor_id
                    || facility.beneficiary_commitment != mandate.entity_commitment
                    || facility.rail_asset_id != mandate.asset_id
                    || facility.cap_commitment != expected_cap
                    || facility.risk_policy_digest != guarantor.risk_policy_digest
                    || facility.status != qomm_defmi::facility::CreditFacilityStatus::Active
                {
                    return Err("canonical Maker facility differs from its signed maximum".into());
                }
                Ok(existing)
            }
            Err(error) if not_found(&error) && !allow_create => Err(
                "queued RFQ refers to a Maker facility that was not registered before the outage"
                    .into(),
            ),
            Err(error) if not_found(&error) => {
                let collateral_blinding = maximum_blinding + Scalar::ONE;
                let mut grant = CreditFacilityGrant {
                    operation_id: hash_parts(&[b"QOMM:DEMO:MAKER-FACILITY-OP:v1", &facility_id]),
                    facility_id,
                    guarantor_id: guarantor.guarantor_id,
                    beneficiary_commitment: mandate.entity_commitment,
                    rail_asset_id: mandate.asset_id,
                    cap_commitment: expected_cap,
                    available_commitment: expected_cap,
                    held_commitment: ZERO,
                    outstanding_commitment: ZERO,
                    collateral_commitment: Pedersen::new(b"qomm:defmi:v1")
                        .commit_u64(
                            maximum_amount
                                .checked_add(1)
                                .ok_or_else(|| "Maker collateral amount overflowed".to_string())?,
                            &collateral_blinding,
                        )
                        .compress()
                        .to_bytes(),
                    risk_policy_digest: guarantor.risk_policy_digest,
                    relation_proof_digest: hash_parts(&[
                        b"QOMM:DEMO:MAKER-FACILITY-GRANT-PROOF:v1",
                        &facility_id,
                    ]),
                    valid_from: 1,
                    valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
                    nonce: hash_parts(&[b"QOMM:DEMO:MAKER-FACILITY-NONCE:v1", &facility_id]),
                    guarantor_signature: Signature::from_bytes(&[0; 64]),
                };
                grant.guarantor_signature = guarantor_key.sign(&grant.guarantor_message()?);
                let before = self.rpc.state_root()?;
                let approval = approve(
                    &self.authorizer,
                    &self.governance_keys,
                    grant.statement()?,
                    before,
                )?;
                bridge.grant_credit_facility(&grant, &approval)?;
                bridge.credit_facility(facility_id)
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_or_issue_note(
        &self,
        snapshot: &ParticipantSnapshot,
        asset_id: [u8; 32],
        amount: u64,
        blinding: Scalar,
        pool_id: [u8; 32],
        decoy: bool,
        issuer: &CsdIssuerDefinition,
        issuer_key: &SigningKey,
        now: u64,
    ) -> Result<NoteOutput, String> {
        let key = Pedersen::new(b"qomm:defmi:v1");
        let address = if decoy {
            Address {
                view: G * deterministic_scalar(&[b"QOMM:DEMO:MAKER-DECOY-VIEW:v1", &pool_id]),
                spend: G * deterministic_scalar(&[b"QOMM:DEMO:MAKER-DECOY-SPEND:v1", &pool_id]),
            }
        } else {
            Address {
                view: CompressedRistretto(snapshot.note_view_public)
                    .decompress()
                    .ok_or_else(|| "Maker note view key is not canonical".to_string())?,
                spend: CompressedRistretto(snapshot.note_spend_public)
                    .decompress()
                    .ok_or_else(|| "Maker note spend key is not canonical".to_string())?,
            }
        };
        let discriminator = if decoy {
            b"decoy".as_slice()
        } else {
            b"source".as_slice()
        };
        let seed = hash_parts(&[b"QOMM:DEMO:MAKER-NOTE-RNG:v1", &pool_id, discriminator]);
        let mut rng = StdRng::from_seed(seed);
        let ledger = NoteLedger::new(key.clone(), 64);
        let note = ledger.build_note(
            &address,
            amount,
            key.commit_u64(amount, &blinding),
            &blinding,
            &mut rng,
        );
        let output = NoteOutput::from_note(&note, asset_id, ZERO)?;
        match self.rpc.note_snapshot(output.note_id) {
            Ok(existing) => {
                if existing.output != output {
                    return Err("canonical source note differs from deterministic issuance".into());
                }
                return Ok(output);
            }
            Err(error) if not_found(&error) => {}
            Err(error) => return Err(error),
        }
        let mut issuance = NoteIssuance {
            operation_id: hash_parts(&[b"QOMM:DEMO:MAKER-NOTE-OP:v1", &pool_id, discriminator]),
            issuance_nonce: hash_parts(&[
                b"QOMM:DEMO:MAKER-NOTE-NONCE:v1",
                &pool_id,
                discriminator,
            ]),
            issuer_id: issuer.issuer_id,
            issued_at: now,
            output: output.clone(),
            proof_digest: hash_parts(&[
                b"QOMM:DEMO:CSD-ISSUANCE-EVIDENCE:v1",
                &pool_id,
                discriminator,
            ]),
            issuer_signature: Signature::from_bytes(&[0; 64]),
        };
        issuance.issuer_signature = issuer_key.sign(&issuance.issuer_message()?);
        issuance.verify_issuer(issuer, now)?;
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let before = self.rpc.state_root()?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            issuance.statement()?,
            before,
        )?;
        bridge.issue_note(&issuance, &approval)?;
        let canonical = self.rpc.note_snapshot(output.note_id)?;
        if canonical.output != output {
            return Err("issued source note differs from canonical state".into());
        }
        Ok(output)
    }

    /// Apply the already proof-committee-authorized split to the canonical
    /// Avalanche state. Governance signs only the exact reservation statement
    /// over the current root; a concurrent winner that consumed the same
    /// parent note or facility sequence is rejected by the VM compare-and-swap.
    pub fn allocate_standing_note_pool(
        &self,
        transition: &CreditFacilityTransition,
        authorization: &ReservationAuthorization,
        allocation: &StandingNotePoolAllocation,
    ) -> Result<CanonicalStandingNotePool, String> {
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let before = self.rpc.state_root()?;
        let statement = authorization.statement(transition)?;
        let approval = approve(&self.authorizer, &self.governance_keys, statement, before)?;
        let accepted =
            bridge.allocate_standing_note_pool(transition, authorization, allocation, &approval)?;
        let canonical = bridge.standing_note_pool(allocation.pool_id)?;
        if accepted.after_root != canonical.state_root
            || canonical.current_pool_note_id != allocation.remainder_note.note_id
            || canonical.sequence != allocation.expected_pool_sequence.saturating_add(1)
        {
            return Err("DeFMI accepted a different standing-pool successor".into());
        }
        Ok(canonical)
    }

    /// Replay the winning Maker allocation without committing it. The
    /// returned governance approval is retained byte-for-byte so the final
    /// atomic transaction can prove that both the preview and the committed
    /// split name the same canonical pre-state.
    pub fn preview_standing_note_pool_allocation(
        &self,
        transition: &CreditFacilityTransition,
        authorization: &ReservationAuthorization,
        allocation: &StandingNotePoolAllocation,
    ) -> Result<DefmiStandingPoolAllocationPreview, String> {
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let before = self.rpc.state_root()?;
        let statement = authorization.statement(transition)?;
        let allocation_approval =
            approve(&self.authorizer, &self.governance_keys, statement, before)?;
        let preview = bridge.preview_standing_note_pool_allocation(
            transition,
            authorization,
            allocation,
            &allocation_approval,
        )?;
        if self.rpc.state_root()? != before {
            return Err("standing-pool preview mutated canonical DeFMI state".into());
        }
        Ok(DefmiStandingPoolAllocationPreview {
            allocation: preview,
            allocation_approval,
        })
    }

    /// Submit one verifier-complete, account-free product settlement and then
    /// read every affected covenant back from canonical Avalanche state.  No
    /// Maker or Taker signature is requested here: both legs were delegated by
    /// their pre-trade reservations and the proof committee already authorized
    /// the typed zkPI.
    pub fn settle_product(
        &self,
        order: &ProductNoteSettlementOrder,
        evidence: &ProductSettlementEvidence,
    ) -> Result<DefmiProductSettlementReceipt, String> {
        order.body()?;
        evidence.validate_encoding()?;
        if order.venue_id != self.venue_id || order.defmi_id != self.defmi_id {
            return Err("product settlement belongs to another DeFMI market".into());
        }
        let maker = order
            .reservations
            .iter()
            .find(|reservation| reservation.role == ReservationRole::Maker)
            .ok_or_else(|| "product settlement has no Maker reservation".to_string())?;
        let taker = order
            .reservations
            .iter()
            .find(|reservation| reservation.role == ReservationRole::Taker)
            .ok_or_else(|| "product settlement has no Taker reservation".to_string())?;
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let before = self.rpc.state_root()?;
        let statement = order.statement()?;
        let approval = approve(&self.authorizer, &self.governance_keys, statement, before)?;
        let accepted = bridge.settle_product(order, evidence, &approval)?;
        if accepted.statement != statement || accepted.before_root != before {
            return Err("DeFMI accepted another product-settlement statement".into());
        }

        let maker_reservation = bridge.note_reservation(maker.transition.hold_id)?;
        let taker_reservation = bridge.note_reservation(taker.transition.hold_id)?;
        let maker_hold = bridge.credit_hold(maker.transition.hold_id)?;
        let taker_hold = bridge.credit_hold(taker.transition.hold_id)?;
        let mut canonical_readbacks = Vec::new();
        for (name, reservation, hold) in [
            ("Maker", &maker_reservation, &maker_hold),
            ("Taker", &taker_reservation, &taker_hold),
        ] {
            if reservation.state_root != accepted.after_root
                || reservation.status != "consumed"
                || reservation.settlement_digest != statement
                || hold.state_root != accepted.after_root
                || hold.status != "consumed"
                || hold.settlement_digest != order.settlement.statement()?
            {
                return Err(format!(
                    "canonical {name} reservation did not reach consumed finality"
                ));
            }
            canonical_readbacks.push(
                CanonicalReadback::new(
                    ReadbackKind::NoteReservation,
                    reservation.hold_id,
                    reservation.state_root,
                )
                .map_err(|error| error.to_string())?,
            );
            canonical_readbacks.push(
                CanonicalReadback::new(ReadbackKind::CreditHold, hold.hold_id, hold.state_root)
                    .map_err(|error| error.to_string())?,
            );
        }
        let mut claim_ids = Vec::new();
        for spend in &order.settlement.spends {
            for claim in &spend.claims {
                let canonical = bridge.note_claim(claim.claim_id)?;
                if canonical.state_root != accepted.after_root
                    || canonical.status != "active"
                    || canonical.settlement_digest != statement
                    || canonical.source_hold_id != spend.hold_id
                {
                    return Err("DeFMI stored another confidential settlement claim".into());
                }
                claim_ids.push(canonical.claim_id);
                canonical_readbacks.push(
                    CanonicalReadback::new(
                        ReadbackKind::NoteClaim,
                        canonical.claim_id,
                        canonical.state_root,
                    )
                    .map_err(|error| error.to_string())?,
                );
            }
        }
        if claim_ids.len() != 4 {
            return Err("product settlement did not create four delivery/refund claims".into());
        }
        Ok(DefmiProductSettlementReceipt {
            transaction_id: accepted.tx_id,
            block_id: accepted.block_id,
            height: accepted.height,
            statement,
            before_state_root: accepted.before_root,
            after_state_root: accepted.after_root,
            maker_hold_id: maker.transition.hold_id,
            taker_hold_id: taker.transition.hold_id,
            claim_ids,
            canonical_readbacks,
        })
    }

    /// Commit the Maker standing-pool split and both anonymous DvP legs as one
    /// Avalanche transaction. A stale pool sequence, concurrent reservation,
    /// invalid typed zkPI, or invalid DvP proof rejects the whole transaction;
    /// no Maker child hold can survive a failed settlement.
    pub fn settle_product_with_standing_pool(
        &self,
        transition: &CreditFacilityTransition,
        authorization: &ReservationAuthorization,
        allocation: &StandingNotePoolAllocation,
        preview: &DefmiStandingPoolAllocationPreview,
        order: &ProductNoteSettlementOrder,
        evidence: &ProductSettlementEvidence,
    ) -> Result<DefmiProductSettlementReceipt, String> {
        order.body()?;
        evidence.validate_encoding()?;
        if order.venue_id != self.venue_id || order.defmi_id != self.defmi_id {
            return Err("product settlement belongs to another DeFMI market".into());
        }
        let current_root = self.rpc.state_root()?;
        if preview.allocation.before_state_root != current_root
            || preview.allocation_approval.before_root != current_root
        {
            return Err("standing-pool allocation preview is stale".into());
        }
        let maker = order
            .reservations
            .iter()
            .find(|reservation| reservation.role == ReservationRole::Maker)
            .ok_or_else(|| "product settlement has no Maker reservation".to_string())?;
        let taker = order
            .reservations
            .iter()
            .find(|reservation| reservation.role == ReservationRole::Taker)
            .ok_or_else(|| "product settlement has no Taker reservation".to_string())?;
        let statement = standing_pool_product_settlement_statement(
            transition,
            authorization,
            allocation,
            order,
            evidence.digest()?,
        )?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            statement,
            current_root,
        )?;
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let accepted = bridge.settle_standing_pool_product(
            StandingPoolProductSettlementRequest {
                allocation_transition: transition,
                allocation_authorization: authorization,
                allocation,
                allocation_approval: &preview.allocation_approval,
                order,
                evidence,
            },
            &approval,
        )?;
        if accepted.statement != statement || accepted.before_root != current_root {
            return Err("DeFMI accepted another atomic product-settlement statement".into());
        }

        let order_statement = order.statement()?;
        let settlement_statement = order.settlement.statement()?;
        let maker_reservation = bridge.note_reservation(maker.transition.hold_id)?;
        let taker_reservation = bridge.note_reservation(taker.transition.hold_id)?;
        let maker_hold = bridge.credit_hold(maker.transition.hold_id)?;
        let taker_hold = bridge.credit_hold(taker.transition.hold_id)?;
        let mut canonical_readbacks = Vec::new();
        for (name, reservation, hold) in [
            ("Maker", &maker_reservation, &maker_hold),
            ("Taker", &taker_reservation, &taker_hold),
        ] {
            if reservation.state_root != accepted.after_root
                || reservation.status != "consumed"
                || reservation.settlement_digest != order_statement
                || hold.state_root != accepted.after_root
                || hold.status != "consumed"
                || hold.settlement_digest != settlement_statement
            {
                return Err(format!(
                    "canonical {name} reservation did not reach atomic consumed finality"
                ));
            }
            canonical_readbacks.push(
                CanonicalReadback::new(
                    ReadbackKind::NoteReservation,
                    reservation.hold_id,
                    reservation.state_root,
                )
                .map_err(|error| error.to_string())?,
            );
            canonical_readbacks.push(
                CanonicalReadback::new(ReadbackKind::CreditHold, hold.hold_id, hold.state_root)
                    .map_err(|error| error.to_string())?,
            );
        }
        let canonical_pool = bridge.standing_note_pool(allocation.pool_id)?;
        if canonical_pool.state_root != accepted.after_root
            || canonical_pool.current_pool_note_id != allocation.remainder_note.note_id
            || canonical_pool.sequence != allocation.expected_pool_sequence.saturating_add(1)
        {
            return Err("canonical Maker standing pool differs from its atomic successor".into());
        }
        canonical_readbacks.push(
            CanonicalReadback::new(
                ReadbackKind::StandingNotePool,
                canonical_pool.pool_id,
                canonical_pool.state_root,
            )
            .map_err(|error| error.to_string())?,
        );
        let mut claim_ids = Vec::new();
        for spend in &order.settlement.spends {
            for claim in &spend.claims {
                let canonical = bridge.note_claim(claim.claim_id)?;
                if canonical.state_root != accepted.after_root
                    || canonical.status != "active"
                    || canonical.settlement_digest != order_statement
                    || canonical.source_hold_id != spend.hold_id
                {
                    return Err("DeFMI stored another atomic confidential settlement claim".into());
                }
                claim_ids.push(canonical.claim_id);
                canonical_readbacks.push(
                    CanonicalReadback::new(
                        ReadbackKind::NoteClaim,
                        canonical.claim_id,
                        canonical.state_root,
                    )
                    .map_err(|error| error.to_string())?,
                );
            }
        }
        if claim_ids.len() != 4 {
            return Err("atomic product settlement did not create four claims".into());
        }
        Ok(DefmiProductSettlementReceipt {
            transaction_id: accepted.tx_id,
            block_id: accepted.block_id,
            height: accepted.height,
            statement,
            before_state_root: accepted.before_root,
            after_state_root: accepted.after_root,
            maker_hold_id: maker.transition.hold_id,
            taker_hold_id: taker.transition.hold_id,
            claim_ids,
            canonical_readbacks,
        })
    }

    pub fn register_verifier(
        &mut self,
        registry_digest: [u8; 32],
        frost_public: &frost::keys::PublicKeyPackage,
        pq_committee: &qomm_zkpi::QuorumPolicy,
        _now: u64,
        existing_only: bool,
    ) -> Result<[u8; 32], String> {
        if registry_digest == [0; 32] {
            return Err("DeFMI settlement verifier has an empty policy registry".into());
        }
        let frost_public_package = frost_public
            .serialize()
            .map_err(|_| "FROST public package cannot be serialized")?;
        let verifier_eligibility_bits = (PRODUCT_QUOTE_ELIGIBILITY_BITS - 2) as u16;
        let verifier_span_bits = PRODUCT_QUOTE_SPAN_BITS as u16;
        let verifier_amount_bits = PRODUCT_ZKPI_AMOUNT_BITS as u16;
        let verifier_price_bits = PRODUCT_ZKPI_PRICE_BITS as u16;
        let epoch_digest = hash_parts(&[
            b"QOMM:DEMO:MARKET-EPOCH:v4",
            &self.venue_id,
            &self.defmi_id,
            &registry_digest,
            &frost_public_package,
            &pq_committee.digest().map_err(|error| error.to_string())?,
            &verifier_eligibility_bits.to_be_bytes(),
            &verifier_span_bits.to_be_bytes(),
            &verifier_amount_bits.to_be_bytes(),
            &verifier_price_bits.to_be_bytes(),
        ]);
        self.epoch = u64::from_be_bytes(
            epoch_digest[..8]
                .try_into()
                .map_err(|_| "DeFMI market epoch digest is malformed")?,
        )
        .max(1);
        self.committee_keys = None;
        let config = SettlementVerifierConfig {
            venue_id: self.venue_id,
            defmi_id: self.defmi_id,
            epoch: self.epoch,
            quote_registry_digest: registry_digest,
            // The complete-quote circuit consumes two sign bits internally.
            quote_eligibility_bits: verifier_eligibility_bits,
            quote_span_bits: verifier_span_bits,
            amount_bits: verifier_amount_bits,
            price_bits: verifier_price_bits,
            max_horizon: 3_600,
            frost_public_package,
            pq_committee: pq_committee.clone(),
            valid_from: 1,
            valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
        };
        match self
            .rpc
            .settlement_verifier_snapshot(self.venue_id, self.epoch)
        {
            Ok(existing) => {
                if existing.config != config || existing.statement != config.statement()? {
                    return Err(
                        "canonical settlement verifier differs from the local MPC market".into(),
                    );
                }
                self.registry_digest = Some(registry_digest);
                return Ok(existing.state_root);
            }
            Err(error) if not_found(&error) && existing_only => {
                return Err(
                    "queued RFQ refers to a settlement verifier that was not registered before the outage"
                        .into(),
                )
            }
            Err(error) if not_found(&error) => {}
            Err(error) => return Err(error),
        }
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        let before = self.rpc.state_root()?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            config.statement()?,
            before,
        )?;
        bridge.register_settlement_verifier(&config, &approval)?;
        self.registry_digest = Some(registry_digest);
        self.rpc.state_root()
    }

    pub fn register_admission(
        &mut self,
        attestations: &[NodeAdmissionAttestation],
        node_keys: &[VerifyingKey],
        expires_at: u64,
    ) -> Result<DefmiAdmissionReceipt, String> {
        if self.registry_digest.is_none() {
            return Err("DeFMI verifier must be registered before admission".into());
        }
        let certified = verify_admission_lane(attestations, node_keys)?;
        let raw_keys = node_keys
            .iter()
            .map(VerifyingKey::to_bytes)
            .collect::<Vec<_>>();
        let bridge = AvalancheNoteBridge::new(&self.authorizer, &self.rpc);
        if self.committee_keys.as_deref() != Some(raw_keys.as_slice()) {
            if self.committee_keys.is_some() {
                return Err("DeFMI admission node keys changed inside one market epoch".into());
            }
            let committee = AdmissionCommitteePlan {
                operation_id: hash_parts(&[
                    b"QOMM:DEMO:ADMISSION-COMMITTEE:OP:v1",
                    &self.venue_id,
                    &self.epoch.to_be_bytes(),
                ]),
                venue_id: self.venue_id,
                epoch: self.epoch,
                node_keys: raw_keys.clone(),
                // Committee membership belongs to the registered market
                // epoch, not to one coordinator process or RFQ. Stable bounds
                // make an exact restart retry byte-for-byte idempotent.
                valid_from: 1,
                valid_until: DEMO_INFRASTRUCTURE_VALID_UNTIL,
            };
            let before = self.rpc.state_root()?;
            let approval = approve(
                &self.authorizer,
                &self.governance_keys,
                committee.statement()?,
                before,
            )?;
            bridge.register_admission_committee(&committee, &approval)?;
            self.committee_keys = Some(raw_keys);
        }
        let admission_digest = certified.digest(self.venue_id, self.epoch)?;
        let batch_id = hash_parts(&[
            b"QOMM:DEMO:ADMISSION-BATCH:v1",
            &self.venue_id,
            &self.epoch.to_be_bytes(),
            &certified.slot.to_be_bytes(),
            &certified.sequence.to_be_bytes(),
            &certified.cluster_digest,
        ]);
        let batch = AdmissionBatchPlan {
            operation_id: hash_parts(&[b"QOMM:DEMO:ADMISSION-BATCH:OP:v1", &batch_id]),
            batch_id,
            venue_id: self.venue_id,
            epoch: self.epoch,
            slot: certified.slot,
            batch_digest: certified.cluster_digest,
            order_digest: certified.order_digest,
            first_sequence: certified.sequence,
            admission_digests: vec![admission_digest],
            expires_at,
        };
        let before = self.rpc.state_root()?;
        let approval = approve(
            &self.authorizer,
            &self.governance_keys,
            batch.statement()?,
            before,
        )?;
        bridge.register_admission_batch(&batch, &[attestations.to_vec()], &approval)?;
        Ok(DefmiAdmissionReceipt {
            epoch: self.epoch,
            batch_id,
            admission_digest,
            after_state_root: self.rpc.state_root()?,
        })
    }
}

#[derive(Clone)]
struct Entity {
    client: ParticipantClient,
    snapshot: ParticipantSnapshot,
    role: ParticipantRole,
}

pub fn bootstrap(config: DefmiBootstrapConfig) -> Result<DefmiBootstrapReport, String> {
    if config.maker_endpoints.len() != 4 || config.mpc_operator_endpoints.len() != 7 {
        return Err("DeFMI demo bootstrap requires four Makers and seven MPC operators".into());
    }
    let rpc = docker_rpc_client(&config.rpc_endpoint, Duration::from_secs(30))?;
    let network = rpc.call("defmivm.network", json!({}))?;
    let chain_id = network
        .get("chainID")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "DeFMI network did not expose its chain ID".to_string())?
        .to_string();
    let (authorizer, governance_keys) = development_committee(&chain_id)?;
    let domain_id = digest(&format!("QOMM:DEMO:DEFMI-DOMAIN:{chain_id}"));
    let defmi_id = digest(&format!("QOMM:DEMO:DEFMI:{chain_id}"));
    let now = unix_seconds()?;
    // Checked-in demo participants and their signed KYB registry represent
    // infrastructure configuration, not one process lifetime. Stable bounds
    // make the registry id and scope binding restart-idempotent; real
    // deployments replace these with governance-issued lifecycle epochs.
    let valid_from = 1;
    let valid_until = DEMO_INFRASTRUCTURE_VALID_UNTIL;
    let mut transactions = Vec::new();

    let mut entities = Vec::new();
    for endpoint in &config.maker_endpoints {
        entities.push(load_entity(endpoint, "maker", ParticipantRole::Maker)?);
    }
    entities.push(load_entity(
        &config.taker_endpoint,
        "taker",
        ParticipantRole::Taker,
    )?);
    // A deployed MPC program is immutable. Circuit upgrades register a new
    // service and new participant bindings rather than mutating the service
    // that authenticated earlier admissions.
    let service_id: [u8; 32] = Sha256::new()
        .chain_update(b"QOMM:DEMO:MPC-SERVICE:v2")
        .chain_update(config.program_digest)
        .finalize()
        .into();
    let kyb_signing =
        SigningKey::from_bytes(&digest(&format!("QOMM:DEMO:KYB-ISSUER:{chain_id}:v1")));
    let trusted_issuer = kyb_signing.verifying_key();
    let required_cohort = cohort_id("JP", "regulated-dealer", 2);
    let registry = SignedCohortRegistry::issue(
        &required_cohort,
        1,
        valid_until,
        entities
            .iter()
            .map(|entity| {
                curve25519_dalek::ristretto::CompressedRistretto(entity.snapshot.kyb_public_point)
                    .decompress()
                    .ok_or_else(|| "participant KYB public point is not canonical".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        &kyb_signing,
    )
    .map_err(str::to_string)?;
    let scope = [b"QOMM:DEMO:KYB-SCOPE:v1".as_slice(), &service_id].concat();
    let context = [
        b"QOMM:DEMO:KYB-CONTEXT:v1".as_slice(),
        &defmi_id,
        &service_id,
    ]
    .concat();
    let mut presentations = BTreeMap::new();
    for entity in &entities {
        let presentation = entity.client.kyb_presentation(KybPresentationRequest {
            snapshot: &entity.snapshot,
            registry: &registry,
            trusted_issuer: &trusted_issuer,
            scope: &scope,
            context: &context,
            required_cohort: &required_cohort,
            now,
        })?;
        presentations.insert(entity.snapshot.participant_id, presentation);
    }
    let mut operators = Vec::new();
    for endpoint in &config.mpc_operator_endpoints {
        operators.push(load_entity(
            endpoint,
            "mpc_operator",
            ParticipantRole::MpcOperator,
        )?);
    }

    let configuration = RegistryConfiguration {
        operation_id: digest("QOMM:DEMO:REGISTRY:OPERATION:v1"),
        domain_id,
        template_digest: digest("QOMM:DEMO:PARTICIPANT-TEMPLATE:v1"),
        schema_digest: digest("QOMM:DEMO:PARTICIPANT-SCHEMA:v1"),
        template_version: 1,
    };
    let participant_registry = rpc.participant_registry_snapshot()?;
    match participant_registry.get("configuration") {
        Some(Value::Null) | None => {
            let statement = configuration
                .statement()
                .map_err(|error| error.to_string())?;
            let before = rpc.state_root()?;
            let approval = approve(&authorizer, &governance_keys, statement, before)?;
            let tx = rpc.issue_participant_registry(&configuration, &approval, before)?;
            wait(&rpc, &tx)?;
            transactions.push(tx);
        }
        Some(existing) => verify_configuration(existing, &configuration)?,
    }

    for entity in entities.iter().chain(&operators) {
        let registration = registration(entity, valid_from, valid_until)?;
        match rpc.participant_snapshot(entity.snapshot.participant_id) {
            Ok(existing) => verify_participant(&existing, entity)?,
            Err(error) if error.contains("not found") => {
                let statement = registration
                    .statement()
                    .map_err(|error| error.to_string())?;
                let before = rpc.state_root()?;
                let approval = approve(&authorizer, &governance_keys, statement, before)?;
                let tx = rpc.issue_participant(&registration, &approval, before)?;
                wait(&rpc, &tx)?;
                transactions.push(tx);
            }
            Err(error) => return Err(error),
        }
    }

    let mut members = operators
        .iter()
        .enumerate()
        .map(|(index, entity)| {
            Ok(MpcServiceMember {
                node_id: digest(&format!("QOMM:DEMO:MPC-NODE:{index}")),
                operator_participant_id: entity.snapshot.participant_id,
                public_key: *entity
                    .snapshot
                    .public_keys
                    .get("mpc_input")
                    .ok_or_else(|| "MPC operator has no input key".to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    members.sort_by_key(|member| member.node_id);
    let service = MpcService {
        operation_id: Sha256::new()
            .chain_update(b"QOMM:DEMO:MPC-SERVICE:OPERATION:v2")
            .chain_update(service_id)
            .finalize()
            .into(),
        service_id,
        kind: MpcServiceKind::QommMatching,
        program_digest: config.program_digest,
        schema_digest: digest("QOMM:DEMO:MPC-INPUT-SCHEMA:v1"),
        committee_epoch: 1,
        threshold: 3,
        members,
        valid_from,
        valid_until,
        sequence: 0,
        status: ServiceStatus::Active,
    };
    match rpc.mpc_service_snapshot(service_id) {
        Ok(existing) => verify_service(&existing, &service)?,
        Err(error) if error.contains("not found") => {
            let statement = service.statement().map_err(|error| error.to_string())?;
            let before = rpc.state_root()?;
            let approval = approve(&authorizer, &governance_keys, statement, before)?;
            let tx = rpc.issue_mpc_service(&service, &approval, before)?;
            wait(&rpc, &tx)?;
            transactions.push(tx);
        }
        Err(error) => return Err(error),
    }

    for entity in &entities {
        // Binding a participant to a new immutable MPC program is a real
        // participant state transition.  The participant may already have
        // bindings for older program versions, so zero is only correct for
        // the first ever binding.  Read the consensus-owned compare-and-swap
        // cursor immediately before constructing and signing this binding.
        let participant = rpc.participant_snapshot(entity.snapshot.participant_id)?;
        let expected_participant_sequence = participant
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "DeFMI participant snapshot has no sequence".to_string())?;
        let binding = ParticipantServiceBinding {
            operation_id: digest(&format!(
                "QOMM:DEMO:SERVICE-BINDING:OPERATION:{}:{}",
                hex::encode(service_id),
                hex::encode(entity.snapshot.participant_id)
            )),
            binding_id: service_binding_id(service_id, entity.snapshot.participant_id),
            participant_id: entity.snapshot.participant_id,
            service_id,
            service_epoch: 1,
            input_public_key: *entity
                .snapshot
                .public_keys
                .get("mpc_input")
                .ok_or_else(|| "participant has no MPC input key".to_string())?,
            capability_digest: digest("QOMM:DEMO:QOMM-MATCHING-CAPABILITY:v1"),
            valid_from,
            valid_until,
            expected_participant_sequence,
            sequence: 0,
            active: true,
        };
        match rpc.participant_service_binding_snapshot(binding.binding_id) {
            Ok(existing) => verify_binding(&existing, &binding)?,
            Err(error) if error.contains("not found") => {
                let statement = binding.statement().map_err(|error| error.to_string())?;
                let entity_approval = entity.client.entity_approval(
                    &entity.snapshot,
                    domain_id,
                    KeyPurpose::MpcInput,
                    statement,
                )?;
                let before = rpc.state_root()?;
                let approval = approve(&authorizer, &governance_keys, statement, before)?;
                let tx = rpc.issue_participant_service_binding(
                    &binding,
                    &entity_approval,
                    &approval,
                    before,
                )?;
                wait(&rpc, &tx)?;
                transactions.push(tx);
            }
            Err(error) => return Err(error),
        }
    }

    Ok(DefmiBootstrapReport {
        chain_id,
        defmi_id: hex::encode(defmi_id),
        domain_id: hex::encode(domain_id),
        service_id: hex::encode(service_id),
        participant_count: entities.len() + operators.len(),
        operator_count: operators.len(),
        maker_participant_ids: entities[..config.maker_endpoints.len()]
            .iter()
            .map(|entity| hex::encode(entity.snapshot.participant_id))
            .collect(),
        taker_participant_id: hex::encode(
            entities
                .last()
                .ok_or_else(|| "DeFMI bootstrap has no Taker entity".to_string())?
                .snapshot
                .participant_id,
        ),
        registered_transactions: transactions,
        state_root: hex::encode(rpc.state_root()?),
        governance: "public local-development 3-of-7 genesis committee",
        maker_capacities: entities[..config.maker_endpoints.len()]
            .iter()
            .map(|entity| (entity.snapshot.cash, entity.snapshot.inventory))
            .collect(),
        taker_capacity: entities
            .last()
            .map(|entity| (entity.snapshot.cash, entity.snapshot.inventory))
            .ok_or_else(|| "DeFMI bootstrap has no Taker capacity".to_string())?,
        kyb: DefmiKybBundle {
            registry,
            trusted_issuer,
            scope,
            context,
            required_cohort,
            presentations,
        },
    })
}

pub fn service_binding_id(service_id: [u8; 32], participant_id: [u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"QOMM:DEMO:SERVICE-BINDING:v2");
    hash.update(service_id);
    hash.update(participant_id);
    hash.finalize().into()
}

fn load_entity(
    endpoint: &str,
    expected_role: &str,
    role: ParticipantRole,
) -> Result<Entity, String> {
    let client = ParticipantClient::new(endpoint, Duration::from_secs(15))?;
    let snapshot = client.snapshot()?;
    if snapshot.role != expected_role {
        return Err(format!(
            "participant {} reports role {}, expected {expected_role}",
            snapshot.label, snapshot.role
        ));
    }
    Ok(Entity {
        client,
        snapshot,
        role,
    })
}

fn registration(
    entity: &Entity,
    valid_from: u64,
    valid_until: u64,
) -> Result<RegisterParticipant, String> {
    let key = |purpose: &str| -> Result<PurposeKey, String> {
        Ok(PurposeKey {
            public_key: *entity
                .snapshot
                .public_keys
                .get(purpose)
                .ok_or_else(|| format!("participant has no {purpose} key"))?,
            epoch: 1,
        })
    };
    let roles = match entity.role {
        ParticipantRole::Maker => {
            BTreeSet::from([ParticipantRole::Maker, ParticipantRole::BrokerDealer])
        }
        role => BTreeSet::from([role]),
    };
    let participant_id = entity.snapshot.participant_id;
    Ok(RegisterParticipant {
        operation_id: digest(&format!(
            "QOMM:DEMO:REGISTER:{}",
            hex::encode(participant_id)
        )),
        participant: ParticipantRecord {
            participant_id,
            legal_entity_credential_commitment: digest(&format!(
                "QOMM:DEMO:KYB:{}",
                hex::encode(participant_id)
            )),
            credential_issuer_id: digest("QOMM:DEMO:KYB-ISSUER:v1"),
            credential_scheme_digest: digest("QOMM:DEMO:KYB-SCHEME:v1"),
            jurisdiction: "JP".into(),
            roles,
            keys: ParticipantKeys {
                admin: key("admin")?,
                settlement: key("settlement")?,
                quote: key("quote")?,
                mpc_input: key("mpc_input")?,
                emergency: key("emergency")?,
            },
            policy_digest: digest(&format!(
                "QOMM:DEMO:PARTICIPANT-POLICY:{}",
                hex::encode(participant_id)
            )),
            valid_from,
            valid_until,
            sequence: 0,
            status: ParticipantStatus::Active,
        },
    })
}

fn verify_configuration(existing: &Value, expected: &RegistryConfiguration) -> Result<(), String> {
    require_hex(existing, "domainID", expected.domain_id)?;
    require_hex(existing, "templateDigest", expected.template_digest)?;
    require_hex(existing, "schemaDigest", expected.schema_digest)?;
    if existing.get("templateVersion").and_then(Value::as_u64)
        != Some(u64::from(expected.template_version))
    {
        return Err("existing DeFMI participant template version differs from the demo".into());
    }
    Ok(())
}

fn verify_participant(existing: &Value, entity: &Entity) -> Result<(), String> {
    require_hex(existing, "participantID", entity.snapshot.participant_id)?;
    let expected_role = match entity.role {
        ParticipantRole::Maker => "maker",
        ParticipantRole::Taker => "taker",
        ParticipantRole::MpcOperator => "mpc_operator",
        _ => return Err("unexpected demo participant role".into()),
    };
    if !existing
        .get("roles")
        .and_then(Value::as_array)
        .is_some_and(|roles| {
            roles
                .iter()
                .any(|role| role.as_str() == Some(expected_role))
        })
    {
        return Err("existing DeFMI participant has another role".into());
    }
    for (json_name, purpose) in [
        ("admin", "admin"),
        ("settlement", "settlement"),
        ("quote", "quote"),
        ("mpcInput", "mpc_input"),
        ("emergency", "emergency"),
    ] {
        let expected = *entity
            .snapshot
            .public_keys
            .get(purpose)
            .ok_or_else(|| format!("participant has no {purpose} key"))?;
        let key = existing
            .pointer(&format!("/keys/{json_name}"))
            .ok_or_else(|| format!("existing participant has no {json_name} key"))?;
        require_hex(key, "publicKey", expected)?;
    }
    Ok(())
}

fn verify_service(existing: &Value, expected: &MpcService) -> Result<(), String> {
    require_hex(existing, "serviceID", expected.service_id)?;
    require_hex(existing, "programDigest", expected.program_digest)?;
    require_hex(existing, "schemaDigest", expected.schema_digest)?;
    if existing.get("threshold").and_then(Value::as_u64) != Some(u64::from(expected.threshold))
        || existing
            .get("members")
            .and_then(Value::as_array)
            .map(Vec::len)
            != Some(expected.members.len())
    {
        return Err("existing DeFMI MPC committee differs from the running service".into());
    }
    Ok(())
}

fn verify_binding(existing: &Value, expected: &ParticipantServiceBinding) -> Result<(), String> {
    require_hex(existing, "bindingID", expected.binding_id)?;
    require_hex(existing, "participantID", expected.participant_id)?;
    require_hex(existing, "serviceID", expected.service_id)?;
    require_hex(existing, "inputPublicKey", expected.input_public_key)
}

fn require_hex(value: &Value, name: &str, expected: [u8; 32]) -> Result<(), String> {
    if value.get(name).and_then(Value::as_str) == Some(hex::encode(expected).as_str()) {
        Ok(())
    } else {
        Err(format!(
            "existing DeFMI {name} differs from the running demo"
        ))
    }
}

fn development_committee(
    domain: &str,
) -> Result<(QuorumAuthorizer, BTreeMap<String, SigningKey>), String> {
    let keys = (0..7)
        .map(|index| {
            (
                format!("node-{index}"),
                SigningKey::from_bytes(&digest(&format!("key:{index}"))),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let nodes = keys
        .iter()
        .map(|(name, key)| (name.clone(), key.verifying_key()))
        .collect::<BTreeMap<String, VerifyingKey>>();
    Ok((QuorumAuthorizer::new(nodes, 3, 1, domain)?, keys))
}

fn approve(
    authorizer: &QuorumAuthorizer,
    keys: &BTreeMap<String, SigningKey>,
    statement: [u8; 32],
    before_root: [u8; 32],
) -> Result<QuorumApproval, String> {
    let signers = keys
        .iter()
        .take(3)
        .map(|(name, key)| (name.clone(), key.clone()))
        .collect::<BTreeMap<_, _>>();
    authorizer.approve(statement, before_root, &signers)
}

fn wait(client: &AvalancheRpcClient, transaction: &str) -> Result<(), String> {
    let accepted = client.wait_accepted(transaction, ACCEPTANCE_TIMEOUT, ACCEPTANCE_POLL)?;
    let actual = client.state_root()?;
    if accepted.after_root != actual {
        return Err("accepted DeFMI root differs from the canonical state root".into());
    }
    Ok(())
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

/// `base + delta` (or `base - delta`) on compressed Pedersen commitments, for
/// facility transitions expressed against canonical before-values.
fn shifted_commitment(base: [u8; 32], delta: [u8; 32], add: bool) -> Result<[u8; 32], String> {
    let base = CompressedRistretto(base)
        .decompress()
        .ok_or_else(|| "facility commitment is not canonical".to_string())?;
    let delta = CompressedRistretto(delta)
        .decompress()
        .ok_or_else(|| "hold amount commitment is not canonical".to_string())?;
    Ok(if add { base + delta } else { base - delta }
        .compress()
        .to_bytes())
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

fn not_found(error: &str) -> bool {
    error.to_ascii_lowercase().contains("not found")
}

fn verify_existing_pool(
    pool: &CanonicalStandingNotePool,
    mandate: &MakerPolicyMandate,
    mandate_digest: [u8; 32],
    maximum_amount_commitment: [u8; 32],
    committee_epoch: u64,
) -> Result<(), String> {
    if pool.venue_id != mandate.venue_id
        || pool.defmi_id != mandate.defmi_id
        || pool.entity_commitment != mandate.entity_commitment
        || pool.policy_digest != mandate.policy_digest
        || pool.mandate_digest != mandate_digest
        || pool.asset_id != mandate.asset_id
        || pool.direction != mandate.direction as u8
        || pool.maximum_amount_commitment != maximum_amount_commitment
        || pool.committee_epoch != committee_epoch
        || pool.valid_until != mandate.valid_until
    {
        return Err("canonical standing pool differs from its signed Maker mandate".into());
    }
    Ok(())
}

fn unix_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "system clock is before Unix epoch".to_string())
}

pub(crate) fn docker_rpc_client(
    endpoint: &str,
    timeout: Duration,
) -> Result<AvalancheRpcClient, String> {
    let authority = endpoint
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix("/rpc"))
        .filter(|value| !value.is_empty() && value.contains(':') && !value.contains('/'))
        .ok_or_else(|| "Docker DeFMI endpoint must be http://host:port/rpc".to_string())?
        .to_string();
    AvalancheRpcClient::with_transport(
        "http://localhost:9650/rpc",
        timeout,
        true,
        move |body, timeout| rpc_post(&authority, body, timeout),
    )
}

fn rpc_post(authority: &str, body: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
    let address = authority
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| "Docker DeFMI endpoint did not resolve".to_string())?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("Docker DeFMI endpoint did not accept a connection: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|_| stream.set_write_timeout(Some(timeout)))
        .map_err(|error| error.to_string())?;
    let request = format!(
        "POST /rpc HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(|error| error.to_string())?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut response = Vec::new();
    Read::by_ref(&mut stream)
        .take((MAX_RPC_BYTES + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    if response.len() > MAX_RPC_BYTES {
        return Err("Docker DeFMI RPC response exceeded one MiB".into());
    }
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "Docker DeFMI RPC returned malformed HTTP".to_string())?;
    let head = std::str::from_utf8(&response[..split])
        .map_err(|_| "Docker DeFMI RPC returned malformed headers")?;
    if !head.lines().next().unwrap_or_default().contains(" 200 ") {
        return Err("Docker DeFMI RPC returned a non-success status".into());
    }
    Ok(response[split + 4..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_committee_matches_the_checked_in_genesis() {
        let (authorizer, keys) = development_committee("chain").unwrap();
        let statement = digest("statement");
        let before = digest("before");
        let approval = approve(&authorizer, &keys, statement, before).unwrap();
        assert!(authorizer.verify(&statement, &before, &approval));
        assert_eq!(approval.approvals.len(), 3);
    }

    #[test]
    fn service_binding_ids_are_stable_and_entity_specific() {
        assert_eq!(
            service_binding_id([1; 32], [7; 32]),
            service_binding_id([1; 32], [7; 32])
        );
        assert_ne!(
            service_binding_id([1; 32], [7; 32]),
            service_binding_id([1; 32], [8; 32])
        );
        assert_ne!(
            service_binding_id([1; 32], [7; 32]),
            service_binding_id([2; 32], [7; 32])
        );
    }
}
