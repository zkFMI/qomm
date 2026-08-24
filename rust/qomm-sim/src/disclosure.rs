//! Three ways of telling the market something, compared by the study.
//!
//! - **none** — only the counterparties see the firm price.
//! - **threshold** — an exact statement: at least K independent makers can fill
//!   at least V lots inside a band around the reference mid. No noise, but the
//!   statement is suppressed when it is false.
//! - **dp** — entity-clipped statistics released every window with discrete
//!   Laplace noise and a per-entity continual-observation budget.
//!
//! The DP mechanism uses entity-level adjacency: two datasets differ by removing
//! every request, quote, update and trade made by one legal entity. That is the
//! unit the design protects, and it is why the sensitivity is a per-entity clip
//! rather than a per-record bound --- which turns out to decide what the
//! mechanism can and cannot publish.

use std::collections::BTreeMap;

use crate::pyrandom::PyRandom;

/// Continual-observation budget, tracked per protected entity.
#[derive(Clone, Debug)]
pub struct EntityAccountant {
    pub epsilon_total: f64,
    pub spent: f64,
    pub releases: u64,
}

impl EntityAccountant {
    pub fn new(epsilon_total: f64) -> Self {
        EntityAccountant {
            epsilon_total,
            spent: 0.0,
            releases: 0,
        }
    }
    pub fn can_spend(&self, epsilon: f64) -> bool {
        self.spent + epsilon <= self.epsilon_total + 1e-12
    }
    pub fn spend(&mut self, epsilon: f64) {
        self.spent += epsilon;
        self.releases += 1;
    }
}

/// Two-sided geometric noise with scale `sensitivity / epsilon`.
pub fn discrete_laplace(epsilon: f64, sensitivity: f64, rng: &mut PyRandom) -> i64 {
    assert!(
        sensitivity > 0.0 && epsilon > 0.0,
        "sensitivity and epsilon must be positive"
    );
    let alpha = (-epsilon / sensitivity).exp();
    if alpha <= 0.0 {
        // Scale below one quantum: the mechanism degenerates to no noise, which
        // is the correct limit and avoids a log of zero.
        return 0;
    }
    let geom = |rng: &mut PyRandom| {
        let u = rng.random();
        ((-u).ln_1p() / alpha.ln()).floor() as i64
    };
    geom(rng) - geom(rng)
}

/// Recover `|S|` from a noisy `|S + N|`.
///
/// Symmetric noise biases an absolute value upward: for scale `b`,
/// `E|S+N| = |S| + b·exp(-|S|/b)`, so perfectly balanced flow publishes as `b`
/// of imbalance and makers widen against informed flow that is not there. Soft
/// thresholding at the published scale corrects it deterministically, so a
/// reader recomputes it from public figures.
pub fn debias_absolute(observed: f64, scale: f64) -> f64 {
    if scale <= 0.0 {
        return observed.abs();
    }
    (observed.abs() - scale).max(0.0)
}

/// Ground truth for one window, before any protection.
#[derive(Clone, Debug, Default)]
pub struct WindowObservation {
    pub window: usize,
    pub start_step: usize,
    pub end_step: usize,
    pub requests_by_entity: BTreeMap<usize, i64>,
    pub volume_by_entity: BTreeMap<usize, i64>,
    pub signed_volume_by_entity: BTreeMap<usize, i64>,
    pub fills: i64,
    pub requests: i64,
    pub no_quote: i64,
    pub liquidity_lots_in_band: i64,
    pub makers_in_band: i64,
    pub fills_by_bucket: [i64; 3],
    pub requests_by_bucket: [i64; 3],
}

#[derive(Clone, Debug, Default)]
pub struct ReleaseFields {
    pub noisy_requests: i64,
    pub noisy_volume: i64,
    pub noisy_signed_volume: i64,
    pub noisy_fills: i64,
    pub fill_rate: Option<f64>,
    /// Kept for error measurement only; never seen by a maker.
    pub exact_requests: i64,
    pub exact_volume: i64,
    pub exact_signed_volume: i64,
    pub exact_fills: i64,
    pub noise_scale_requests: f64,
    pub noise_scale_signed: f64,
    pub debiased: bool,
    pub min_makers: i64,
    pub min_lots: i64,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub window: usize,
    pub mode: &'static str,
    pub published: bool,
    pub fields: ReleaseFields,
    pub epsilon_spent: f64,
    pub suppressed_reason: &'static str,
}

/// An estimate of the informed fraction and its variance. Infinite variance
/// means the channel said nothing usable, which is a result rather than a bug.
pub type PublicSignal = (Option<f64>, f64);

pub enum Disclosure {
    None,
    Threshold { min_makers: i64, min_lots: i64 },
    Dp(Box<DpDisclosure>),
}

pub struct DpDisclosure {
    pub epsilon_per_window: f64,
    pub request_cap: i64,
    pub volume_cap: i64,
    pub accountants: BTreeMap<usize, EntityAccountant>,
    pub n_fields: f64,
    pub debias: bool,
    /// Three fields take one entity's cap, which is what the audited adjacency
    /// calls for. The signed field alone took twice that --- the replace-one
    /// figure --- which doubled its noise for no gain in privacy.
    pub signed_sensitivity_factor: f64,
}

/// A satisfied depth statement says the market is not stressed, which shifts the
/// estimate modestly below the population base. It is one bit, so the residual
/// variance stays wide; setting it to zero would be wrong, because the statement
/// is about depth and not about who is trading.
const CALM_ESTIMATE: f64 = 0.24;
const CALM_VARIANCE: f64 = 0.16;

impl Disclosure {
    pub fn name(&self) -> &'static str {
        match self {
            Disclosure::None => "A_none",
            Disclosure::Threshold { .. } => "B_threshold",
            Disclosure::Dp(_) => "C_dp",
        }
    }

    pub fn release(&mut self, obs: &WindowObservation, rng: &mut PyRandom) -> Release {
        match self {
            Disclosure::None => Release {
                window: obs.window,
                mode: "none",
                published: false,
                fields: ReleaseFields::default(),
                epsilon_spent: 0.0,
                suppressed_reason: "arm A publishes nothing",
            },
            Disclosure::Threshold {
                min_makers,
                min_lots,
            } => {
                let holds =
                    obs.makers_in_band >= *min_makers && obs.liquidity_lots_in_band >= *min_lots;
                if !holds {
                    return Release {
                        window: obs.window,
                        mode: "B_threshold",
                        published: false,
                        fields: ReleaseFields::default(),
                        epsilon_spent: 0.0,
                        suppressed_reason: "threshold statement not satisfied",
                    };
                }
                Release {
                    window: obs.window,
                    mode: "B_threshold",
                    published: true,
                    fields: ReleaseFields {
                        min_makers: *min_makers,
                        min_lots: *min_lots,
                        ..ReleaseFields::default()
                    },
                    epsilon_spent: 0.0,
                    suppressed_reason: "",
                }
            }
            Disclosure::Dp(dp) => dp.release(obs, rng),
        }
    }

    pub fn public_signal(&self, release: &Release) -> PublicSignal {
        match self {
            Disclosure::None => (None, f64::INFINITY),
            Disclosure::Threshold { .. } => {
                if release.published {
                    (Some(CALM_ESTIMATE), CALM_VARIANCE)
                } else {
                    (None, f64::INFINITY)
                }
            }
            Disclosure::Dp(dp) => dp.public_signal(release),
        }
    }

    pub fn epsilon_spent_max(&self) -> f64 {
        match self {
            Disclosure::Dp(dp) => dp.accountants.values().map(|a| a.spent).fold(0.0, f64::max),
            _ => 0.0,
        }
    }
}

impl DpDisclosure {
    pub fn new(
        epsilon_per_window: f64,
        request_cap: i64,
        volume_cap: i64,
        entities: usize,
        epsilon_total: f64,
        debias: bool,
    ) -> Self {
        DpDisclosure {
            epsilon_per_window,
            request_cap,
            volume_cap,
            accountants: (0..entities)
                .map(|e| (e, EntityAccountant::new(epsilon_total)))
                .collect(),
            n_fields: 4.0,
            debias,
            signed_sensitivity_factor: 1.0,
        }
    }

    fn release(&mut self, obs: &WindowObservation, rng: &mut PyRandom) -> Release {
        let active: Vec<usize> = obs
            .requests_by_entity
            .iter()
            .filter(|(_, c)| **c > 0)
            .map(|(e, _)| *e)
            .collect();
        if active.iter().any(|e| {
            self.accountants
                .get(e)
                .is_none_or(|a| !a.can_spend(self.epsilon_per_window))
        }) {
            return Release {
                window: obs.window,
                mode: "C_dp",
                published: false,
                fields: ReleaseFields::default(),
                epsilon_spent: 0.0,
                suppressed_reason: "entity privacy budget exhausted",
            };
        }
        for e in &active {
            if let Some(a) = self.accountants.get_mut(e) {
                a.spend(self.epsilon_per_window);
            }
        }

        let eps = self.epsilon_per_window / self.n_fields;
        let clipped_requests: i64 = obs
            .requests_by_entity
            .values()
            .map(|c| (*c).min(self.request_cap))
            .sum();
        let clipped_volume: i64 = obs
            .volume_by_entity
            .values()
            .map(|v| (*v).min(self.volume_cap))
            .sum();
        let clipped_signed: i64 = obs
            .signed_volume_by_entity
            .values()
            .map(|v| (*v).clamp(-self.volume_cap, self.volume_cap))
            .sum();
        let clipped_fills = obs.fills.min(clipped_requests);

        let request_cap = self.request_cap as f64;
        let volume_cap = self.volume_cap as f64;
        let signed_sensitivity = self.signed_sensitivity_factor * volume_cap;

        let noisy_requests = (clipped_requests + discrete_laplace(eps, request_cap, rng)).max(0);
        let noisy_volume = (clipped_volume + discrete_laplace(eps, volume_cap, rng)).max(0);
        let noisy_signed = clipped_signed + discrete_laplace(eps, signed_sensitivity, rng);
        let noisy_fills = (clipped_fills + discrete_laplace(eps, request_cap, rng)).max(0);

        Release {
            window: obs.window,
            mode: "C_dp",
            published: true,
            fields: ReleaseFields {
                noisy_requests,
                noisy_volume,
                noisy_signed_volume: noisy_signed,
                noisy_fills,
                fill_rate: if noisy_requests > 0 {
                    Some(noisy_fills as f64 / noisy_requests as f64)
                } else {
                    None
                },
                exact_requests: clipped_requests,
                exact_volume: clipped_volume,
                exact_signed_volume: clipped_signed,
                exact_fills: clipped_fills,
                noise_scale_requests: request_cap / eps,
                noise_scale_signed: signed_sensitivity / eps,
                debiased: self.debias,
                min_makers: 0,
                min_lots: 0,
            },
            epsilon_spent: self.epsilon_per_window,
            suppressed_reason: "",
        }
    }

    /// Signed order-flow imbalance is the public proxy for informed flow.
    fn public_signal(&self, release: &Release) -> PublicSignal {
        if !release.published {
            return (None, f64::INFINITY);
        }
        let volume = release.fields.noisy_volume;
        if volume <= 0 {
            return (None, f64::INFINITY);
        }
        let signed = release.fields.noisy_signed_volume as f64;
        let magnitude = if release.fields.debiased {
            debias_absolute(signed, release.fields.noise_scale_signed)
        } else {
            signed.abs()
        };
        let imbalance = magnitude / (volume.max(1) as f64);
        let estimate = imbalance.clamp(0.0, 0.95);
        // sampling variance plus the DP noise contribution
        let noise_sd = release.fields.noise_scale_signed * 2.0f64.sqrt();
        let var = 0.02 + (noise_sd / (volume.max(1) as f64)).powi(2);
        (Some(estimate), var)
    }
}
