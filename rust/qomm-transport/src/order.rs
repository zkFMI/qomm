//! Content-independent admission and ordering for one fixed market slot.

use crate::wire::Frame;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

const TICKET_DOMAIN: &[u8] = b"QOMM:ORDER:TICKET:v1";
const RECEIPT_DOMAIN: &[u8] = b"QOMM:ORDER:RECEIPT:v1";
const BEACON_DOMAIN: &[u8] = b"QOMM:ORDER:BEACON:v1";
const MANIFEST_DOMAIN: &[u8] = b"QOMM:ORDER:MANIFEST:v1";
const ORDER_DOMAIN: &[u8] = b"QOMM:ORDER:KEY:v1";
pub const ZERO: [u8; 32] = [0; 32];

fn digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u32).to_be_bytes());
        hash.update(part);
    }
    hash.finalize().into()
}

fn hmac(key: &[u8], body: &[u8]) -> [u8; 32] {
    let mut block = [0_u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for index in 0..64 {
        inner_pad[index] ^= block[index];
        outer_pad[index] ^= block[index];
    }
    let inner = Sha256::new()
        .chain_update(inner_pad)
        .chain_update(body)
        .finalize();
    Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize()
        .into()
}

#[derive(Clone, Debug)]
pub struct AdmissionTicket {
    pub slot: u64,
    pub ticket_id: [u8; 32],
    pub issued_at: u64,
    pub expires_at: u64,
    pub signature: Signature,
}

impl AdmissionTicket {
    pub fn unsigned(&self) -> Result<Vec<u8>, String> {
        if self.expires_at <= self.issued_at {
            return Err("ticket expiry must follow issuance".into());
        }
        let mut body = Vec::with_capacity(TICKET_DOMAIN.len() + 56);
        body.extend_from_slice(TICKET_DOMAIN);
        body.extend_from_slice(&self.slot.to_be_bytes());
        body.extend_from_slice(&self.ticket_id);
        body.extend_from_slice(&self.issued_at.to_be_bytes());
        body.extend_from_slice(&self.expires_at.to_be_bytes());
        Ok(body)
    }

    pub fn digest(&self) -> Result<[u8; 32], String> {
        Ok(digest(&[&self.unsigned()?, &self.signature.to_bytes()]))
    }

    pub fn verify(&self, authority: &VerifyingKey, now: u64) -> bool {
        self.issued_at <= now
            && now <= self.expires_at
            && self
                .unsigned()
                .is_ok_and(|body| authority.verify(&body, &self.signature).is_ok())
    }
}

pub struct AdmissionAuthority {
    signing_key: SigningKey,
    entity_key: Vec<u8>,
    issued: BTreeMap<(u64, [u8; 32]), [u8; 32]>,
}

impl AdmissionAuthority {
    pub fn new(signing_key: SigningKey, entity_key: Vec<u8>) -> Result<Self, String> {
        if entity_key.len() < 32 {
            return Err("entity nullifier key must contain at least 32 bytes".into());
        }
        Ok(Self {
            signing_key,
            entity_key,
            issued: BTreeMap::new(),
        })
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn entity_nullifier(&self, entity_id: &[u8]) -> Result<[u8; 32], String> {
        if entity_id.is_empty() {
            return Err("an empty legal-entity identifier is not admissible".into());
        }
        let mut body = Vec::with_capacity(14 + entity_id.len());
        body.extend_from_slice(b"QOMM:ENTITY:v1");
        body.extend_from_slice(entity_id);
        Ok(hmac(&self.entity_key, &body))
    }

    pub fn issue(
        &mut self,
        entity_id: &[u8],
        slot: u64,
        issued_at: Option<u64>,
        lifetime: u64,
        ticket_id: Option<[u8; 32]>,
    ) -> Result<AdmissionTicket, String> {
        if lifetime == 0 {
            return Err("ticket lifetime must be positive".into());
        }
        let issued_at = issued_at.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });
        let expires_at = issued_at
            .checked_add(lifetime)
            .ok_or_else(|| "ticket expiry overflow".to_string())?;
        let nullifier = self.entity_nullifier(entity_id)?;
        if self.issued.contains_key(&(slot, nullifier)) {
            return Err("this legal entity already has a ticket for the slot".into());
        }
        let ticket_id = ticket_id.unwrap_or_else(rand::random);
        let mut body = Vec::new();
        body.extend_from_slice(TICKET_DOMAIN);
        body.extend_from_slice(&slot.to_be_bytes());
        body.extend_from_slice(&ticket_id);
        body.extend_from_slice(&issued_at.to_be_bytes());
        body.extend_from_slice(&expires_at.to_be_bytes());
        let ticket = AdmissionTicket {
            slot,
            ticket_id,
            issued_at,
            expires_at,
            signature: self.signing_key.sign(&body),
        };
        self.issued.insert((slot, nullifier), ticket.digest()?);
        Ok(ticket)
    }
}

#[derive(Clone, Debug)]
pub struct RandomnessBeacon {
    pub round: u64,
    pub value: [u8; 32],
    pub signature: Signature,
}

impl RandomnessBeacon {
    fn unsigned(round: u64, value: &[u8; 32]) -> Vec<u8> {
        [BEACON_DOMAIN, &round.to_be_bytes(), value].concat()
    }

    pub fn sign(round: u64, value: [u8; 32], key: &SigningKey) -> Self {
        Self {
            round,
            value,
            signature: key.sign(&Self::unsigned(round, &value)),
        }
    }

    pub fn verify(&self, key: &VerifyingKey) -> bool {
        key.verify(&Self::unsigned(self.round, &self.value), &self.signature)
            .is_ok()
    }
}

#[derive(Clone, Debug)]
pub struct AdmissionReceipt {
    pub slot: u64,
    pub node: u64,
    pub ticket_digest: [u8; 32],
    pub frame_digest: [u8; 32],
    pub received_at_ns: u64,
    pub signature: Signature,
}

impl AdmissionReceipt {
    pub fn unsigned(&self) -> Vec<u8> {
        [
            RECEIPT_DOMAIN,
            &self.slot.to_be_bytes(),
            &self.node.to_be_bytes(),
            &self.ticket_digest,
            &self.frame_digest,
            &self.received_at_ns.to_be_bytes(),
        ]
        .concat()
    }

    pub fn verify(&self, key: &VerifyingKey) -> bool {
        key.verify(&self.unsigned(), &self.signature).is_ok()
    }
}

#[derive(Clone, Debug)]
pub struct BatchManifest {
    pub slot: u64,
    pub node: u64,
    pub beacon_round: u64,
    pub beacon_value: [u8; 32],
    pub ordered_ticket_digests: Vec<[u8; 32]>,
    pub ordered_frame_digests: Vec<[u8; 32]>,
    pub previous_digest: [u8; 32],
    pub signature: Signature,
}

impl BatchManifest {
    pub fn unsigned(&self) -> Result<Vec<u8>, String> {
        if self.ordered_ticket_digests.len() != self.ordered_frame_digests.len() {
            return Err("ticket and frame manifests have different lengths".into());
        }
        let mut body = Vec::new();
        body.extend_from_slice(MANIFEST_DOMAIN);
        body.extend_from_slice(&self.slot.to_be_bytes());
        body.extend_from_slice(&self.node.to_be_bytes());
        body.extend_from_slice(&self.beacon_round.to_be_bytes());
        body.extend_from_slice(&self.beacon_value);
        body.extend_from_slice(&self.previous_digest);
        body.extend_from_slice(&(self.ordered_ticket_digests.len() as u64).to_be_bytes());
        for (ticket, frame) in self
            .ordered_ticket_digests
            .iter()
            .zip(&self.ordered_frame_digests)
        {
            body.extend_from_slice(ticket);
            body.extend_from_slice(frame);
        }
        Ok(body)
    }

    pub fn digest(&self) -> Result<[u8; 32], String> {
        Ok(digest(&[&self.unsigned()?, &self.signature.to_bytes()]))
    }

    pub fn verify(&self, key: &VerifyingKey) -> bool {
        self.unsigned()
            .is_ok_and(|body| key.verify(&body, &self.signature).is_ok())
    }

    pub fn includes(&self, receipt: &AdmissionReceipt) -> bool {
        self.ordered_ticket_digests
            .iter()
            .zip(&self.ordered_frame_digests)
            .any(|(ticket, frame)| {
                ticket == &receipt.ticket_digest && frame == &receipt.frame_digest
            })
    }
}

pub struct FixedSlotSealer {
    pub slot: u64,
    pub node: u64,
    pub deadline_ns: u64,
    pub tickets: Vec<AdmissionTicket>,
    pub authority_key: VerifyingKey,
    pub beacon_key: VerifyingKey,
    pub signing_key: SigningKey,
    pub previous_digest: [u8; 32],
    frames: BTreeMap<[u8; 32], Frame>,
    receipts: BTreeMap<[u8; 32], AdmissionReceipt>,
    closed: bool,
}

impl FixedSlotSealer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        slot: u64,
        node: u64,
        deadline_ns: u64,
        tickets: Vec<AdmissionTicket>,
        authority_key: VerifyingKey,
        beacon_key: VerifyingKey,
        signing_key: SigningKey,
        previous_digest: [u8; 32],
    ) -> Result<Self, String> {
        let ids = tickets
            .iter()
            .map(|ticket| ticket.ticket_id)
            .collect::<BTreeSet<_>>();
        if ids.len() != tickets.len() {
            return Err("the expected ticket list contains a duplicate".into());
        }
        if tickets.iter().any(|ticket| ticket.slot != slot) {
            return Err("a ticket belongs to another slot".into());
        }
        Ok(Self {
            slot,
            node,
            deadline_ns,
            tickets,
            authority_key,
            beacon_key,
            signing_key,
            previous_digest,
            frames: BTreeMap::new(),
            receipts: BTreeMap::new(),
            closed: false,
        })
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn admit(
        &mut self,
        ticket: &AdmissionTicket,
        frame: Frame,
        now_ns: u64,
    ) -> Result<AdmissionReceipt, String> {
        if self.closed {
            return Err("the slot is already closed".into());
        }
        if now_ns > self.deadline_ns {
            return Err("the frame arrived after the sealed deadline".into());
        }
        if !ticket.verify(&self.authority_key, now_ns / 1_000_000_000) {
            return Err("the admission ticket is invalid or expired".into());
        }
        let ticket_digest = ticket.digest()?;
        let expected = self
            .tickets
            .iter()
            .map(AdmissionTicket::digest)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if !expected.contains(&ticket_digest) {
            return Err("the ticket was not in the slot's precommitted population".into());
        }
        if u64::from(frame.slot) != self.slot || u64::from(frame.node) != self.node {
            return Err("the frame belongs to another slot or node".into());
        }
        let raw = frame.encode();
        let frame_digest = Sha256::digest(raw).into();
        if let Some(prior) = self.frames.get(&ticket_digest) {
            if prior.encode() != raw {
                return Err("one ticket attempted to replace its admitted frame".into());
            }
            return Ok(self.receipts[&ticket_digest].clone());
        }
        let mut receipt = AdmissionReceipt {
            slot: self.slot,
            node: self.node,
            ticket_digest,
            frame_digest,
            received_at_ns: now_ns,
            signature: Signature::from_bytes(&[0; 64]),
        };
        receipt.signature = self.signing_key.sign(&receipt.unsigned());
        self.frames.insert(ticket_digest, frame);
        self.receipts.insert(ticket_digest, receipt.clone());
        Ok(receipt)
    }

    pub fn close(
        &mut self,
        beacon: &RandomnessBeacon,
        now_ns: u64,
    ) -> Result<(Vec<Frame>, BatchManifest), String> {
        if self.closed {
            return Err("the slot is already closed".into());
        }
        if now_ns <= self.deadline_ns {
            return Err("the batch cannot close before its deadline".into());
        }
        if !beacon.verify(&self.beacon_key) {
            return Err("the ordering beacon signature is invalid".into());
        }
        if beacon.round <= self.slot {
            return Err("ordering randomness must be generated after the slot".into());
        }
        let missing = self
            .tickets
            .iter()
            .filter(|ticket| {
                ticket
                    .digest()
                    .is_ok_and(|digest| !self.frames.contains_key(&digest))
            })
            .count();
        if missing != 0 {
            return Err(format!(
                "fixed population incomplete: {missing} cover or request frame(s) missing"
            ));
        }
        let mut ordered = self.tickets.clone();
        ordered.sort_by_key(|ticket| {
            Sha256::new()
                .chain_update(ORDER_DOMAIN)
                .chain_update(self.slot.to_be_bytes())
                .chain_update(beacon.round.to_be_bytes())
                .chain_update(beacon.value)
                .chain_update(ticket.ticket_id)
                .finalize()
                .to_vec()
        });
        let ticket_digests = ordered
            .iter()
            .map(AdmissionTicket::digest)
            .collect::<Result<Vec<_>, _>>()?;
        let frames = ticket_digests
            .iter()
            .map(|digest| self.frames[digest].clone())
            .collect::<Vec<_>>();
        let frame_digests = frames
            .iter()
            .map(|frame| Sha256::digest(frame.encode()).into())
            .collect();
        let mut manifest = BatchManifest {
            slot: self.slot,
            node: self.node,
            beacon_round: beacon.round,
            beacon_value: beacon.value,
            ordered_ticket_digests: ticket_digests,
            ordered_frame_digests: frame_digests,
            previous_digest: self.previous_digest,
            signature: Signature::from_bytes(&[0; 64]),
        };
        manifest.signature = self.signing_key.sign(&manifest.unsigned()?);
        self.closed = true;
        Ok((frames, manifest))
    }
}

pub fn prove_omission(
    receipt: &AdmissionReceipt,
    manifest: &BatchManifest,
    sealer_key: &VerifyingKey,
) -> bool {
    receipt.slot == manifest.slot
        && receipt.node == manifest.node
        && receipt.verify(sealer_key)
        && manifest.verify(sealer_key)
        && !manifest.includes(receipt)
}
