//! Real sharing and Reed--Solomon location behind the demo behaviour switches.

use crate::model::{evaluate, Outcome, Policy, Request, FIELDS};
use curve25519_dalek::scalar::Scalar;
use qomm_audit::locate::{capacity, locate, share, Verdict};
use qomm_transport::roles::{audit_node, ComputingNode, InputParty};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{BTreeMap, BTreeSet};

pub const HONEST: &str = "honest";
pub const LIE_PRODUCT: &str = "lie_product";
pub const LIE_OPEN: &str = "lie_open";
pub const LIE_INPUT: &str = "lie_input";
pub const OFFLINE: &str = "offline";
pub const DROPOUT: &str = "dropout";
pub const BEHAVIOURS: [&str; 6] = [HONEST, LIE_PRODUCT, LIE_OPEN, LIE_INPUT, DROPOUT, OFFLINE];

#[derive(Clone, Debug, Default)]
pub struct Transcript {
    pub named: BTreeMap<usize, usize>,
    pub rejected: Vec<(usize, String, usize)>,
    pub reductions: usize,
    pub corrections: usize,
    pub aborted: bool,
    pub abort_reason: String,
    pub abort_code: String,
    pub abort_fields: BTreeMap<String, usize>,
    pub product_capacity: usize,
    pub open_capacity: usize,
    pub silent: Vec<usize>,
    pub corrupted_inputs: Vec<usize>,
}

impl Transcript {
    fn stop(&mut self, code: &str, reason: impl Into<String>) {
        self.aborted = true;
        self.abort_code = code.into();
        self.abort_reason = reason.into();
    }

    fn name(&mut self, nodes: &[usize]) {
        for node in nodes {
            *self.named.entry(*node).or_default() += 1;
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProtocolResult {
    pub outcome: Outcome,
    pub transcript: Transcript,
    pub node_shares: BTreeMap<usize, Vec<String>>,
    pub used_policies: Vec<Policy>,
    pub used_request: Request,
}

pub struct Session {
    n: usize,
    threshold: usize,
    behaviours: BTreeMap<usize, String>,
    rng: StdRng,
}

impl Session {
    pub fn new(
        n: usize,
        threshold: usize,
        behaviours: BTreeMap<usize, String>,
        seed: u64,
    ) -> Result<Self, String> {
        if n < 2 * threshold + 1 {
            return Err(format!(
                "{n} nodes cannot carry a threshold of {threshold}: reconstruction needs {}",
                2 * threshold + 1
            ));
        }
        if behaviours
            .values()
            .any(|value| !BEHAVIOURS.contains(&value.as_str()))
        {
            return Err("unknown node behaviour".into());
        }
        Ok(Self {
            n,
            threshold,
            behaviours,
            rng: StdRng::seed_from_u64(seed),
        })
    }

    fn behaviour(&self, node: usize) -> &str {
        self.behaviours
            .get(&node)
            .map(String::as_str)
            .unwrap_or(HONEST)
    }

    fn locate_liars(
        &mut self,
        live: &[usize],
        degree: usize,
        liars: &[usize],
    ) -> Result<Vec<usize>, String> {
        let points = live
            .iter()
            .map(|node| Scalar::from((*node + 1) as u64))
            .collect::<Vec<_>>();
        let secret = Scalar::from(self.rng.gen_range(1_u64..1_000_000));
        let mut shares = share(&secret, degree, &points, &mut self.rng);
        for (offset, liar) in liars.iter().enumerate() {
            let position = live
                .iter()
                .position(|node| node == liar)
                .ok_or_else(|| "liar was not live".to_string())?;
            shares[position] += Scalar::from(10_000 + offset as u64);
        }
        match locate(&points, &shares, degree) {
            Verdict::Decoded { culprits, .. } => Ok(culprits
                .into_iter()
                .map(|position| live[position])
                .collect()),
            Verdict::Beyond { reason, .. } => Err(reason),
        }
    }

    pub fn run(
        mut self,
        policies: &[Policy],
        request: &Request,
        references: &[i64],
        now: i64,
        input_check: bool,
    ) -> Result<ProtocolResult, String> {
        let mut transcript = Transcript {
            silent: (0..self.n)
                .filter(|node| self.behaviour(*node) == OFFLINE)
                .collect(),
            ..Transcript::default()
        };
        let live = (0..self.n)
            .filter(|node| !matches!(self.behaviour(*node), OFFLINE | DROPOUT))
            .collect::<Vec<_>>();
        transcript.product_capacity = capacity(live.len(), 2 * self.threshold);
        transcript.open_capacity = capacity(live.len(), self.threshold);
        if !transcript.silent.is_empty() {
            let absent = transcript.silent.clone();
            transcript.stop(
                "absent",
                format!("nodes {absent:?} did not take part; additive inputs need every share"),
            );
            transcript.abort_fields.insert("n".into(), self.n);
            return Ok(ProtocolResult {
                outcome: Outcome::default(),
                transcript,
                node_shares: BTreeMap::new(),
                used_policies: policies.to_vec(),
                used_request: request.clone(),
            });
        }

        let mut values = vec![
            i128::from(request.asset),
            i128::from(request.qty),
            i128::from(request.direction),
            i128::from(request.entity),
            i128::from(request.is_real),
        ];
        for policy in policies {
            values.extend(policy.fields().map(i128::from));
        }
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
        let party = InputParty {
            name: "demo".into(),
            n_nodes: self.n,
            value_bits: 32,
            signing_key: Some(signing_key),
        };
        let mut nodes = (0..self.n).map(ComputingNode::new).collect::<Vec<_>>();
        party
            .deal(&values, &mut nodes)
            .map_err(|error| error.to_string())?;
        let held = nodes
            .iter()
            .map(|node| node.inputs.clone())
            .collect::<Vec<_>>();
        let mut stated = held.clone();
        for (node, shares) in stated.iter_mut().enumerate().take(self.n) {
            if self.behaviour(node) == LIE_INPUT {
                shares[1] += 333;
                transcript.corrupted_inputs.push(node);
            }
        }
        let mut node_shares = BTreeMap::new();
        let mask = (1_u128 << 72) - 1;
        for (node, shares) in stated.iter().enumerate().take(self.n) {
            node_shares.insert(
                node,
                shares
                    .iter()
                    .take(6)
                    .map(|value| format!("{:018x}", (*value as u128) & mask))
                    .collect(),
            );
        }
        if input_check {
            let verifying_key = party.verifying_key().expect("signed demo dealings");
            for node in &nodes {
                for position in audit_node(node, "demo", &verifying_key, &stated[node.index]) {
                    transcript.rejected.push((
                        node.index,
                        dealer_of(position).to_string(),
                        position,
                    ));
                }
            }
            if !transcript.rejected.is_empty() {
                transcript.stop(
                    "commitment",
                    "a node stated a share that its signed dealing receipt does not bind",
                );
                return Ok(ProtocolResult {
                    outcome: Outcome::default(),
                    transcript,
                    node_shares,
                    used_policies: policies.to_vec(),
                    used_request: request.clone(),
                });
            }
        }

        let effective = (0..values.len())
            .map(|position| stated.iter().map(|node| node[position]).sum::<i128>())
            .collect::<Vec<_>>();
        let clamp_asset =
            |value: i128| value.clamp(0, references.len().saturating_sub(1) as i128) as i64;
        let used_request = Request {
            asset: clamp_asset(effective[0]),
            qty: effective[1].clamp(1, i128::from(i64::MAX)) as i64,
            direction: i64::from(effective[2] != 0),
            entity: effective[3].clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            is_real: i64::from(effective[4] != 0),
        };
        let mut used_policies = Vec::new();
        for maker in 0..policies.len() {
            let start = 5 + maker * FIELDS.len();
            let mut policy = Policy::from_fields(&effective[start..start + FIELDS.len()]);
            policy.asset = clamp_asset(i128::from(policy.asset));
            used_policies.push(policy);
        }
        let outcome = evaluate(&used_policies, &used_request, references, now);

        let product_liars = live
            .iter()
            .copied()
            .filter(|node| self.behaviour(*node) == LIE_PRODUCT)
            .collect::<Vec<_>>();
        transcript.reductions = used_policies.len() * 2;
        match self.locate_liars(&live, 2 * self.threshold, &product_liars) {
            Ok(named) => {
                if !named.is_empty() {
                    transcript.corrections += transcript.reductions;
                    transcript.name(&named);
                }
            }
            Err(reason) => {
                transcript.reductions = 1;
                transcript.stop("beyond_capacity", reason);
                transcript
                    .abort_fields
                    .insert("capacity".into(), transcript.product_capacity);
                transcript
                    .abort_fields
                    .insert("answered".into(), live.len());
                return Ok(ProtocolResult {
                    outcome,
                    transcript,
                    node_shares,
                    used_policies,
                    used_request,
                });
            }
        }

        let open_liars = live
            .iter()
            .copied()
            .filter(|node| self.behaviour(*node) == LIE_OPEN)
            .collect::<Vec<_>>();
        match self.locate_liars(&live, self.threshold, &open_liars) {
            Ok(named) => {
                transcript.reductions += 1;
                if !named.is_empty() {
                    transcript.corrections += 1;
                    transcript.name(&named);
                }
            }
            Err(reason) => {
                transcript.stop("beyond_capacity", reason);
                transcript
                    .abort_fields
                    .insert("capacity".into(), transcript.open_capacity);
                transcript
                    .abort_fields
                    .insert("answered".into(), live.len());
            }
        }
        Ok(ProtocolResult {
            outcome,
            transcript,
            node_shares,
            used_policies,
            used_request,
        })
    }
}

fn dealer_of(position: usize) -> &'static str {
    if position < 5 {
        "the taker"
    } else {
        "a maker"
    }
}

pub fn named_set(transcript: &Transcript) -> BTreeSet<usize> {
    transcript.named.keys().copied().collect()
}
