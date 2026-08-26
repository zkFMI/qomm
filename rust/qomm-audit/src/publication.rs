//! Quorum certificate for privacy-preserving public market statistics.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const DOMAIN: &[u8] = b"QOMM:PUBLICATION-CERTIFICATE:v1";
pub const ZERO: [u8; 32] = [0; 32];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationStatement {
    pub venue: String,
    pub epoch: u64,
    pub slot_start: u64,
    pub slot_end: u64,
    pub source_digest: [u8; 32],
    pub rule_digest: [u8; 32],
    pub mechanism_digest: [u8; 32],
    pub private_input_commitment: [u8; 32],
    pub transcript_digest: [u8; 32],
    pub output_name: String,
    pub output_value: i64,
    pub epsilon_micros: u64,
    pub delta_numerator: u64,
    pub delta_denominator: u128,
    pub budget_total_micros: u64,
    pub budget_before_micros: u64,
    pub budget_after_micros: u64,
    pub previous_certificate: [u8; 32],
}

impl PublicationStatement {
    pub fn validate(&self) -> Result<(), String> {
        if self.venue.is_empty() || self.output_name.is_empty() {
            return Err("venue and output name are required".into());
        }
        if self.slot_end < self.slot_start {
            return Err("invalid epoch or slot range".into());
        }
        if self.epsilon_micros == 0 {
            return Err("epsilon must be positive".into());
        }
        if self.delta_denominator == 0 || u128::from(self.delta_numerator) >= self.delta_denominator
        {
            return Err("delta must be a proper non-negative fraction".into());
        }
        if self.budget_before_micros > self.budget_after_micros
            || self.budget_after_micros > self.budget_total_micros
        {
            return Err("invalid privacy budget transition".into());
        }
        if self.budget_after_micros - self.budget_before_micros != self.epsilon_micros {
            return Err("privacy budget transition does not equal epsilon spent".into());
        }
        Ok(())
    }

    pub fn body(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let value = json!({
            "venue": self.venue,
            "epoch": self.epoch,
            "slot_start": self.slot_start,
            "slot_end": self.slot_end,
            "source_digest": hex::encode(self.source_digest),
            "rule_digest": hex::encode(self.rule_digest),
            "mechanism_digest": hex::encode(self.mechanism_digest),
            "private_input_commitment": hex::encode(self.private_input_commitment),
            "transcript_digest": hex::encode(self.transcript_digest),
            "output_name": self.output_name,
            "output_value": self.output_value,
            "epsilon_micros": self.epsilon_micros,
            "delta_numerator": self.delta_numerator,
            "delta_denominator": self.delta_denominator,
            "budget_total_micros": self.budget_total_micros,
            "budget_before_micros": self.budget_before_micros,
            "budget_after_micros": self.budget_after_micros,
            "previous_certificate": hex::encode(self.previous_certificate),
        });
        let mut body = DOMAIN.to_vec();
        body.extend(serde_json::to_vec(&value).map_err(|error| error.to_string())?);
        Ok(body)
    }

    pub fn digest(&self) -> Result<[u8; 32], String> {
        Ok(Sha256::digest(self.body()?).into())
    }
}

#[derive(Clone, Debug)]
pub struct NodeSignature {
    pub node_id: String,
    pub signature: Signature,
}

#[derive(Clone, Debug)]
pub struct PublicationCertificate {
    pub statement: PublicationStatement,
    pub signatures: Vec<NodeSignature>,
}

impl PublicationCertificate {
    pub fn digest(&self) -> Result<[u8; 32], String> {
        let mut hash = Sha256::new();
        hash.update(self.statement.body()?);
        let mut signatures = self.signatures.clone();
        signatures.sort_by(|left, right| left.node_id.cmp(&right.node_id));
        for signed in signatures {
            hash.update(signed.node_id.as_bytes());
            hash.update(signed.signature.to_bytes());
        }
        Ok(hash.finalize().into())
    }

    pub fn verify(
        &self,
        registry: &BTreeMap<String, VerifyingKey>,
        threshold: usize,
        previous: Option<&PublicationCertificate>,
    ) -> bool {
        if self.statement.validate().is_err() || !(1..=registry.len()).contains(&threshold) {
            return false;
        }
        match previous {
            None if self.statement.previous_certificate != ZERO => return false,
            Some(previous) => {
                if previous.digest().ok() != Some(self.statement.previous_certificate)
                    || self.statement.epoch <= previous.statement.epoch
                    || self.statement.budget_before_micros != previous.statement.budget_after_micros
                {
                    return false;
                }
            }
            None => {}
        }
        let Ok(body) = self.statement.body() else {
            return false;
        };
        let mut seen = BTreeSet::new();
        self.signatures
            .iter()
            .filter(|signed| {
                seen.insert(signed.node_id.clone())
                    && registry
                        .get(&signed.node_id)
                        .is_some_and(|key| key.verify(&body, &signed.signature).is_ok())
            })
            .count()
            >= threshold
    }
}

pub fn certify(
    statement: PublicationStatement,
    signers: &BTreeMap<String, SigningKey>,
) -> Result<PublicationCertificate, String> {
    let body = statement.body()?;
    Ok(PublicationCertificate {
        statement,
        signatures: signers
            .iter()
            .map(|(node_id, key)| NodeSignature {
                node_id: node_id.clone(),
                signature: key.sign(&body),
            })
            .collect(),
    })
}
