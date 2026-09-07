//! Held-out synthetic smoke test for behavior-assisted review prioritization.

use qomm_harness::{next_value, parse_value, write_pretty_json, HarnessResult};
use qomm_sim::attackers::{auc, tpr_at_fpr};
use qomm_sim::deterministic_random::DeterministicRng;
use qomm_sim::fsum::nsum;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const STRATEGIES: [Strategy; 3] = [
    Strategy {
        time: &[8, 3, 1, 1, 2, 5],
        size: &[8, 3, 1],
        instrument: &[8, 3, 1, 1],
        buy_fraction: 0.62,
        log_mean_size: 4.0,
        log_mean_interarrival_ms: 6.5,
    },
    Strategy {
        time: &[1, 2, 6, 8, 3, 1],
        size: &[2, 7, 3],
        instrument: &[2, 7, 2, 1],
        buy_fraction: 0.42,
        log_mean_size: 5.0,
        log_mean_interarrival_ms: 7.2,
    },
    Strategy {
        time: &[3, 6, 3, 2, 5, 4],
        size: &[5, 4, 2],
        instrument: &[3, 2, 6, 2],
        buy_fraction: 0.52,
        log_mean_size: 4.5,
        log_mean_interarrival_ms: 6.8,
    },
];

#[derive(Clone, Copy)]
struct Strategy {
    time: &'static [u64],
    size: &'static [u64],
    instrument: &'static [u64],
    buy_fraction: f64,
    log_mean_size: f64,
    log_mean_interarrival_ms: f64,
}

#[derive(Clone)]
struct BehaviorProfile {
    credential_id: String,
    time_histogram: Vec<f64>,
    size_histogram: Vec<f64>,
    instrument_histogram: Vec<f64>,
    buy_fraction: f64,
    log_mean_size: f64,
    log_mean_interarrival_ms: f64,
    event_count: u64,
}

struct Similarity {
    score: f64,
}

#[derive(Clone, Copy)]
struct BehaviorScreen;

impl BehaviorProfile {
    fn normalized(&self) -> HarnessResult<Self> {
        if self.credential_id.is_empty() || !(0.0..=1.0).contains(&self.buy_fraction) {
            return Err("profile identity or buy fraction is invalid".into());
        }
        if self.event_count == 0 {
            return Err("profile needs at least one event".into());
        }
        Ok(Self {
            credential_id: self.credential_id.clone(),
            time_histogram: distribution(&self.time_histogram, "time_histogram")?,
            size_histogram: distribution(&self.size_histogram, "size_histogram")?,
            instrument_histogram: distribution(&self.instrument_histogram, "instrument_histogram")?,
            buy_fraction: self.buy_fraction,
            log_mean_size: self.log_mean_size,
            log_mean_interarrival_ms: self.log_mean_interarrival_ms,
            event_count: self.event_count,
        })
    }
}

impl BehaviorScreen {
    const VERSION: &'static str = "qomm-behavior-v1";

    fn compare(self, left: &BehaviorProfile, right: &BehaviorProfile) -> HarnessResult<Similarity> {
        let left = left.normalized()?;
        let right = right.normalized()?;
        let components = [
            js_similarity(&left.time_histogram, &right.time_histogram)?,
            js_similarity(&left.size_histogram, &right.size_histogram)?,
            js_similarity(&left.instrument_histogram, &right.instrument_histogram)?,
            scalar_similarity(left.buy_fraction, right.buy_fraction, 0.20)?,
            scalar_similarity(left.log_mean_size, right.log_mean_size, 0.70)?,
            scalar_similarity(
                left.log_mean_interarrival_ms,
                right.log_mean_interarrival_ms,
                0.80,
            )?,
        ];
        // statistics.fmean/math.fsum, so preserve the distinction explicitly.
        let raw = nsum(
            [0.25, 0.20, 0.20, 0.10, 0.10, 0.15]
                .into_iter()
                .zip(components)
                .map(|(weight, value)| weight * value),
        );
        let reliability =
            ((left.event_count.min(right.event_count) as f64 / 100.0).sqrt()).min(1.0);
        let score = (0.5 + reliability * (raw - 0.5)).clamp(0.0, 1.0);
        Ok(Similarity { score })
    }

    fn candidates(
        self,
        profiles: &[BehaviorProfile],
        authoritative_entities: &BTreeMap<String, String>,
        threshold: f64,
    ) -> HarnessResult<Vec<(String, String, f64)>> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err("review threshold must lie in [0,1]".into());
        }
        let normalized = profiles
            .iter()
            .map(BehaviorProfile::normalized)
            .collect::<Result<Vec<_>, _>>()?;
        if normalized
            .iter()
            .map(|profile| &profile.credential_id)
            .collect::<BTreeSet<_>>()
            .len()
            != normalized.len()
        {
            return Err("credential identifiers must be unique".into());
        }
        if normalized
            .iter()
            .any(|profile| !authoritative_entities.contains_key(&profile.credential_id))
        {
            return Err("every profile needs an authoritative KYC entity".into());
        }
        let mut output = Vec::new();
        for (index, left) in normalized.iter().enumerate() {
            for right in &normalized[index + 1..] {
                if authoritative_entities[&left.credential_id]
                    == authoritative_entities[&right.credential_id]
                {
                    continue;
                }
                let similarity = self.compare(left, right)?;
                if similarity.score >= threshold {
                    output.push((
                        left.credential_id.clone(),
                        right.credential_id.clone(),
                        similarity.score,
                    ));
                }
            }
        }
        output.sort_by(|left, right| {
            right
                .2
                .total_cmp(&left.2)
                .then_with(|| left.0.cmp(&right.0))
                .then_with(|| left.1.cmp(&right.1))
        });
        Ok(output)
    }
}

fn distribution(values: &[f64], name: &str) -> HarnessResult<Vec<f64>> {
    if values.is_empty()
        || values
            .iter()
            .any(|value| *value < 0.0 || !value.is_finite())
    {
        return Err(format!("{name} must be a finite non-negative histogram").into());
    }
    let total = nsum(values.iter().copied());
    if total <= 0.0 {
        return Err(format!("{name} must contain at least one observation").into());
    }
    Ok(values.iter().map(|value| value / total).collect())
}

fn js_similarity(left: &[f64], right: &[f64]) -> HarnessResult<f64> {
    if left.len() != right.len() {
        return Err("behavior histograms must use the same schema".into());
    }
    let midpoint = left
        .iter()
        .zip(right)
        .map(|(left, right)| (left + right) / 2.0)
        .collect::<Vec<_>>();
    let divergence = |source: &[f64]| {
        nsum(
            source
                .iter()
                .zip(&midpoint)
                .filter(|(value, _)| **value > 0.0)
                .map(|(value, middle)| value * (value / middle).log2()),
        )
    };
    let js = 0.5 * divergence(left) + 0.5 * divergence(right);
    Ok((1.0 - js.max(0.0).sqrt()).clamp(0.0, 1.0))
}

fn scalar_similarity(left: f64, right: f64, scale: f64) -> HarnessResult<f64> {
    if !left.is_finite() || !right.is_finite() || scale <= 0.0 {
        return Err("behavior scalar must be finite and have positive scale".into());
    }
    Ok((-(left - right).abs() / scale).exp())
}

fn noisy_histogram(
    rng: &mut DeterministicRng,
    base: &[u64],
    events: u64,
    entity_noise: f64,
) -> Vec<f64> {
    let weights = base
        .iter()
        .map(|value| ((*value as f64) * rng.gauss(0.0, entity_noise).exp()).max(0.01))
        .collect::<Vec<_>>();
    let total = nsum(weights.iter().copied());
    let mut cumulative = Vec::with_capacity(weights.len());
    let mut running = 0.0;
    for weight in weights {
        running += weight / total;
        cumulative.push(running);
    }
    let mut counts = vec![0_u64; cumulative.len()];
    for _ in 0..events {
        let draw = rng.random();
        let index = cumulative
            .iter()
            .position(|boundary| draw <= *boundary)
            .unwrap_or(cumulative.len() - 1);
        counts[index] += 1;
    }
    counts.into_iter().map(|value| value as f64).collect()
}

type Population = (
    Vec<BehaviorProfile>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
);

fn population(seed: u64, controllers: usize) -> Population {
    let mut rng = DeterministicRng::new(seed);
    let mut profiles = Vec::new();
    let mut controller_by_credential = BTreeMap::new();
    let mut kyc_by_credential = BTreeMap::new();
    for controller in 0..controllers {
        let strategy = STRATEGIES[rng.randrange(0, STRATEGIES.len() as i64) as usize];
        let legal_entities = if controller % 4 == 0 { 3 } else { 1 };
        let controller_shift = rng.gauss(0.0, 0.16);
        for legal in 0..legal_entities {
            let events = rng.randint(120, 320) as u64;
            let credential = format!("credential-{seed}-{controller}-{legal}");
            profiles.push(BehaviorProfile {
                credential_id: credential.clone(),
                time_histogram: noisy_histogram(&mut rng, strategy.time, events, 0.20),
                size_histogram: noisy_histogram(&mut rng, strategy.size, events, 0.20),
                instrument_histogram: noisy_histogram(&mut rng, strategy.instrument, events, 0.20),
                buy_fraction: (strategy.buy_fraction + controller_shift + rng.gauss(0.0, 0.035))
                    .clamp(0.02, 0.98),
                log_mean_size: strategy.log_mean_size + controller_shift + rng.gauss(0.0, 0.10),
                log_mean_interarrival_ms: strategy.log_mean_interarrival_ms - controller_shift
                    + rng.gauss(0.0, 0.12),
                event_count: events,
            });
            controller_by_credential.insert(credential.clone(), format!("controller-{controller}"));
            kyc_by_credential.insert(credential, format!("legal-{controller}-{legal}"));
        }
    }
    (profiles, controller_by_credential, kyc_by_credential)
}

type LabeledScore = (f64, u8, String, String);

fn labeled_scores(
    profiles: &[BehaviorProfile],
    controllers: &BTreeMap<String, String>,
    screen: BehaviorScreen,
) -> HarnessResult<Vec<LabeledScore>> {
    let mut values = Vec::new();
    for (index, left) in profiles.iter().enumerate() {
        for right in &profiles[index + 1..] {
            values.push((
                screen.compare(left, right)?.score,
                u8::from(controllers[&left.credential_id] == controllers[&right.credential_id]),
                left.credential_id.clone(),
                right.credential_id.clone(),
            ));
        }
    }
    Ok(values)
}

fn calibrate_threshold(scored_labels: &[(f64, u8)], max_fpr: f64) -> HarnessResult<f64> {
    if !(0.0..1.0).contains(&max_fpr) {
        return Err("false-positive cap must lie in [0,1)".into());
    }
    if scored_labels.is_empty() || scored_labels.iter().any(|(_, label)| *label > 1) {
        return Err("calibration needs binary labeled scores".into());
    }
    let negatives = scored_labels
        .iter()
        .filter(|(_, label)| *label == 0)
        .count();
    let positives = scored_labels.len() - negatives;
    if negatives == 0 || positives == 0 {
        return Err("calibration needs both positive and negative pairs".into());
    }
    let mut candidates = scored_labels
        .iter()
        .map(|(score, _)| *score)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.total_cmp(left));
    candidates.dedup_by(|left, right| *left == *right);
    candidates.push(1.000_000_000_001);
    let mut feasible = Vec::new();
    for threshold in candidates {
        let fp = scored_labels
            .iter()
            .filter(|(score, label)| *label == 0 && *score >= threshold)
            .count();
        let tp = scored_labels
            .iter()
            .filter(|(score, label)| *label == 1 && *score >= threshold)
            .count();
        let fpr = fp as f64 / negatives as f64;
        if fpr <= max_fpr {
            feasible.push((tp as f64 / positives as f64, -fpr, -threshold, threshold));
        }
    }
    Ok(feasible
        .into_iter()
        .max_by(|left, right| lexicographic_cmp(*left, *right))
        .map(|value| value.3)
        .unwrap_or(1.000_000_000_001))
}

fn lexicographic_cmp(
    left: (f64, f64, f64, f64),
    right: (f64, f64, f64, f64),
) -> std::cmp::Ordering {
    left.0
        .total_cmp(&right.0)
        .then_with(|| left.1.total_cmp(&right.1))
        .then_with(|| left.2.total_cmp(&right.2))
        .then_with(|| left.3.total_cmp(&right.3))
}

fn run_experiment(seed: u64, max_fpr: f64) -> HarnessResult<Value> {
    let screen = BehaviorScreen;
    let (calibration, calibration_truth, _) = population(seed, 80);
    let (test, test_truth, test_kyc) = population(seed + 10_000, 80);
    let calibrated = labeled_scores(&calibration, &calibration_truth, screen)?;
    let threshold = calibrate_threshold(
        &calibrated
            .iter()
            .map(|(score, label, _, _)| (*score, *label))
            .collect::<Vec<_>>(),
        max_fpr,
    )?;
    let tested = labeled_scores(&test, &test_truth, screen)?;
    let scores = tested.iter().map(|row| row.0).collect::<Vec<_>>();
    let labels = tested.iter().map(|row| row.1).collect::<Vec<_>>();
    let predicted = scores
        .iter()
        .map(|score| *score >= threshold)
        .collect::<Vec<_>>();
    let tp = predicted
        .iter()
        .zip(&labels)
        .filter(|(prediction, label)| **prediction && **label == 1)
        .count();
    let fp = predicted
        .iter()
        .zip(&labels)
        .filter(|(prediction, label)| **prediction && **label == 0)
        .count();
    let positives = labels.iter().filter(|label| **label == 1).count();
    let negatives = labels.len() - positives;
    let candidates = screen.candidates(&test, &test_kyc, threshold)?;
    let candidate_pairs = candidates
        .iter()
        .map(|(left, right, _)| (left.clone(), right.clone()))
        .collect::<BTreeSet<_>>();
    let predicted_pairs = predicted
        .iter()
        .zip(&tested)
        .filter(|(prediction, _)| **prediction)
        .map(|(_, (_, _, left, right))| (left.clone(), right.clone()))
        .collect::<BTreeSet<_>>();
    if candidate_pairs != predicted_pairs {
        return Err("product review candidates differ from evaluation threshold".into());
    }
    let precision = (tp + fp > 0).then(|| tp as f64 / (tp + fp) as f64);
    let pair_auc = auc(&scores, &labels);
    Ok(json!({
        "host": zkfmi_measure::hosts::this_host(),
        "evidence_class": "smoke_only",
        "synthetic": true,
        "model_version": BehaviorScreen::VERSION,
        "seed": seed,
        "calibration_pairs": calibrated.len(),
        "test_pairs": tested.len(),
        "positive_test_pairs": positives,
        "threshold": threshold,
        "maximum_calibration_fpr": max_fpr,
        "metrics": {
            "pair_auc": pair_auc,
            "tpr_at_1pct_fpr": tpr_at_fpr(&scores, &labels, 0.01),
            "threshold_tpr": (positives > 0).then(|| tp as f64 / positives as f64),
            "false_positive_rate": (negatives > 0).then(|| fp as f64 / negatives as f64),
            "review_precision": precision,
            "review_candidates": candidates.len(),
        },
        "sealed_prediction": {
            "metric": "pair_auc",
            "point_prediction": 0.85,
            "likely_failure": "different legal entities using the same strategy look alike",
            "absolute_error": pair_auc.map(|value| (value - 0.85).abs()),
        },
        "safety_contract": {
            "kyc_is_authoritative": true,
            "behavior_can_merge_entities": false,
            "behavior_can_deny_service": false,
            "only_output": "review_required",
        },
        "limitations": [
            "Synthetic labeled controllers are not a substitute for investigation data.",
            "Threshold is calibrated on a disjoint synthetic population.",
            "A production decision remains blocked pending external validation.",
        ],
    }))
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("run_entity_behavior: {error}");
        std::process::exit(1);
    }
}

fn execute() -> HarnessResult<()> {
    let mut out = None::<PathBuf>;
    let mut seed = 202_608_240_u64;
    let mut max_fpr = 0.01_f64;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--out") => out = Some(PathBuf::from(next_value(&mut args, "--out")?)),
            Some("--seed") => seed = parse_value(next_value(&mut args, "--seed")?, "--seed")?,
            Some("--max-fpr") => {
                max_fpr = parse_value(next_value(&mut args, "--max-fpr")?, "--max-fpr")?;
            }
            Some("-h" | "--help") => {
                println!(
                    "usage: run_entity_behavior [-h] --out OUT [--seed SEED] [--max-fpr MAX_FPR]"
                );
                return Ok(());
            }
            Some(value) => return Err(format!("unrecognized argument: {value}").into()),
            None => return Err("argument is not valid UTF-8".into()),
        }
    }
    let out = out.ok_or("the following arguments are required: --out")?;
    let payload = run_experiment(seed, max_fpr)?;
    write_pretty_json(Some(&out), &payload)?;
    println!(
        "{}",
        compact_metrics_json(
            payload
                .get("metrics")
                .ok_or("payload has no metrics object")?
        )?
    );
    Ok(())
}

fn compact_metrics_json(value: &Value) -> HarnessResult<String> {
    let metrics = value.as_object().ok_or("metrics is not an object")?;
    Ok(format!(
        "{{\"false_positive_rate\": {}, \"pair_auc\": {}, \"review_candidates\": {}, \"review_precision\": {}, \"threshold_tpr\": {}, \"tpr_at_1pct_fpr\": {}}}",
        metrics["false_positive_rate"],
        metrics["pair_auc"],
        metrics["review_candidates"],
        metrics["review_precision"],
        metrics["threshold_tpr"],
        metrics["tpr_at_1pct_fpr"],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> BehaviorProfile {
        BehaviorProfile {
            credential_id: name.into(),
            time_histogram: vec![8.0, 2.0],
            size_histogram: vec![7.0, 3.0],
            instrument_histogram: vec![9.0, 1.0],
            buy_fraction: 0.6,
            log_mean_size: 4.0,
            log_mean_interarrival_ms: 6.0,
            event_count: 200,
        }
    }

    #[test]
    fn behavior_only_creates_review_candidates_and_never_merges_kyc() {
        let screen = BehaviorScreen;
        let left = profile("left");
        let mut similar = profile("similar");
        similar.time_histogram = vec![80.0, 20.0];
        similar.size_histogram = vec![70.0, 30.0];
        similar.instrument_histogram = vec![90.0, 10.0];
        similar.buy_fraction = 0.61;
        let mut different = profile("different");
        different.time_histogram = vec![1.0, 9.0];
        different.size_histogram = vec![1.0, 9.0];
        different.instrument_histogram = vec![1.0, 9.0];
        different.buy_fraction = 0.1;
        different.log_mean_size = 8.0;
        different.log_mean_interarrival_ms = 10.0;

        let entities = BTreeMap::from([
            ("left".into(), "legal-a".into()),
            ("similar".into(), "legal-b".into()),
            ("different".into(), "legal-c".into()),
        ]);
        let candidates = screen
            .candidates(&[left.clone(), similar.clone(), different], &entities, 0.8)
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            (&candidates[0].0, &candidates[0].1),
            (&"left".to_string(), &"similar".to_string())
        );

        let same_kyc = BTreeMap::from([
            ("left".into(), "legal-a".into()),
            ("similar".into(), "legal-a".into()),
        ]);
        assert!(screen
            .candidates(&[left, similar], &same_kyc, 0.0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sparse_profiles_shrink_toward_uncertainty() {
        let screen = BehaviorScreen;
        let dense = screen.compare(&profile("a"), &profile("b")).unwrap().score;
        let mut sparse_a = profile("a");
        sparse_a.event_count = 1;
        let mut sparse_b = profile("b");
        sparse_b.event_count = 1;
        let sparse = screen.compare(&sparse_a, &sparse_b).unwrap().score;
        assert!(dense > sparse);
        assert!((sparse - 0.55).abs() <= 0.01, "sparse score was {sparse}");
    }

    #[test]
    fn calibration_obeys_false_positive_cap() {
        let labeled = [
            (0.99, 1),
            (0.95, 1),
            (0.91, 1),
            (0.90, 0),
            (0.80, 0),
            (0.70, 0),
            (0.60, 0),
        ];
        let threshold = calibrate_threshold(&labeled, 0.0).unwrap();
        assert_eq!(threshold, 0.91);
        assert!(labeled
            .iter()
            .filter(|(_, label)| *label == 0)
            .all(|(score, _)| *score < threshold));
    }

    #[test]
    fn profile_validation_fails_closed() {
        let mut invalid = profile("bad");
        invalid.time_histogram = vec![0.0, 0.0];
        let error = invalid.normalized().err().unwrap().to_string();
        assert!(error.contains("histogram"), "unexpected error: {error}");
    }
}
