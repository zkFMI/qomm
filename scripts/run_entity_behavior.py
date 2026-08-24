#!/usr/bin/env python3
"""Held-out synthetic smoke test for behavior-assisted review prioritization."""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_identity.behavior import (BehaviorProfile, BehaviorScreen,
                                    calibrate_threshold)
from qomm_sim.attackers import auc, tpr_at_fpr
from scripts.hosts import this_host


STRATEGIES = (
    ((8, 3, 1, 1, 2, 5), (8, 3, 1), (8, 3, 1, 1), 0.62, 4.0, 6.5),
    ((1, 2, 6, 8, 3, 1), (2, 7, 3), (2, 7, 2, 1), 0.42, 5.0, 7.2),
    ((3, 6, 3, 2, 5, 4), (5, 4, 2), (3, 2, 6, 2), 0.52, 4.5, 6.8),
)


def noisy_histogram(rng: random.Random, base, events: int, entity_noise: float):
    weights = [max(0.01, value * math.exp(rng.gauss(0, entity_noise)))
               for value in base]
    total = sum(weights)
    cumulative = []
    running = 0.0
    for weight in weights:
        running += weight / total
        cumulative.append(running)
    counts = [0] * len(weights)
    for _ in range(events):
        draw = rng.random()
        index = next((i for i, boundary in enumerate(cumulative)
                      if draw <= boundary), len(weights) - 1)
        counts[index] += 1
    return tuple(counts)


def population(seed: int, controllers: int = 80):
    rng = random.Random(seed)
    profiles = []
    controller_by_credential = {}
    kyc_by_credential = {}
    for controller in range(controllers):
        strategy = STRATEGIES[rng.randrange(len(STRATEGIES))]
        # One quarter of controllers operate three separately KYC'd entities;
        # the remainder has one. This is not ground truth available to a venue.
        legal_entities = 3 if controller % 4 == 0 else 1
        controller_shift = rng.gauss(0, 0.16)
        for legal in range(legal_entities):
            events = rng.randint(120, 320)
            credential = f"credential-{seed}-{controller}-{legal}"
            profile = BehaviorProfile(
                credential,
                noisy_histogram(rng, strategy[0], events, 0.20),
                noisy_histogram(rng, strategy[1], events, 0.20),
                noisy_histogram(rng, strategy[2], events, 0.20),
                min(0.98, max(0.02, strategy[3] + controller_shift
                              + rng.gauss(0, 0.035))),
                strategy[4] + controller_shift + rng.gauss(0, 0.10),
                strategy[5] - controller_shift + rng.gauss(0, 0.12),
                events,
            )
            profiles.append(profile)
            controller_by_credential[credential] = f"controller-{controller}"
            kyc_by_credential[credential] = f"legal-{controller}-{legal}"
    return profiles, controller_by_credential, kyc_by_credential


def labeled_scores(profiles, controllers, screen):
    values = []
    for index, left in enumerate(profiles):
        for right in profiles[index + 1:]:
            label = int(controllers[left.credential_id]
                        == controllers[right.credential_id])
            values.append((screen.compare(left, right).score, label,
                           left.credential_id, right.credential_id))
    return values


def run(seed: int, max_fpr: float) -> dict:
    screen = BehaviorScreen()
    calibration, calibration_truth, _ = population(seed)
    test, test_truth, test_kyc = population(seed + 10_000)
    calibrated = labeled_scores(calibration, calibration_truth, screen)
    threshold = calibrate_threshold(
        [(score, label) for score, label, _, _ in calibrated], max_fpr)
    tested = labeled_scores(test, test_truth, screen)
    scores = [score for score, _, _, _ in tested]
    labels = [label for _, label, _, _ in tested]
    predicted = [score >= threshold for score in scores]
    tp = sum(prediction and label for prediction, label in zip(predicted, labels))
    fp = sum(prediction and not label for prediction, label in zip(predicted, labels))
    positives = sum(labels)
    negatives = len(labels) - positives
    candidates = screen.candidates(test, test_kyc, threshold)
    candidate_pairs = {(item.left_credential, item.right_credential)
                       for item in candidates}
    if candidate_pairs != {
            (left, right) for prediction, (_, _, left, right)
            in zip(predicted, tested) if prediction}:
        raise RuntimeError("product review candidates differ from evaluation threshold")
    precision = tp / (tp + fp) if tp + fp else None
    pair_auc = auc(scores, labels)
    return {
        "host": this_host(),
        "evidence_class": "smoke_only",
        "synthetic": True,
        "model_version": screen.VERSION,
        "seed": seed,
        "calibration_pairs": len(calibrated),
        "test_pairs": len(tested),
        "positive_test_pairs": positives,
        "threshold": threshold,
        "maximum_calibration_fpr": max_fpr,
        "metrics": {
            "pair_auc": pair_auc,
            "tpr_at_1pct_fpr": tpr_at_fpr(scores, labels, 0.01),
            "threshold_tpr": tp / positives if positives else None,
            "false_positive_rate": fp / negatives if negatives else None,
            "review_precision": precision,
            "review_candidates": len(candidates),
        },
        "sealed_prediction": {
            "metric": "pair_auc",
            "point_prediction": 0.85,
            "likely_failure": "different legal entities using the same strategy look alike",
            "absolute_error": None if pair_auc is None else abs(pair_auc - 0.85),
        },
        "safety_contract": {
            "kyc_is_authoritative": True,
            "behavior_can_merge_entities": False,
            "behavior_can_deny_service": False,
            "only_output": "review_required",
        },
        "limitations": [
            "Synthetic labeled controllers are not a substitute for investigation data.",
            "Threshold is calibrated on a disjoint synthetic population.",
            "A production decision remains blocked pending external validation.",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=202608240)
    parser.add_argument("--max-fpr", type=float, default=0.01)
    args = parser.parse_args()
    payload = run(args.seed, args.max_fpr)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n",
                        encoding="utf-8")
    print(json.dumps(payload["metrics"], sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
