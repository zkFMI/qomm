"""Conservative behavioral screening for multi-credential probing.

The result is only a review candidate. It never merges legal entities, spends
their privacy budget, denies a request, or substitutes for KYC evidence.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Mapping, Sequence


def _distribution(values: Sequence[float], name: str) -> tuple[float, ...]:
    if not values or any(value < 0 or not math.isfinite(value) for value in values):
        raise ValueError(f"{name} must be a finite non-negative histogram")
    total = sum(values)
    if total <= 0:
        raise ValueError(f"{name} must contain at least one observation")
    return tuple(value / total for value in values)


def _js_similarity(left: Sequence[float], right: Sequence[float]) -> float:
    if len(left) != len(right):
        raise ValueError("behavior histograms must use the same schema")
    midpoint = [(a + b) / 2 for a, b in zip(left, right)]

    def divergence(source):
        return sum(value * math.log2(value / middle)
                   for value, middle in zip(source, midpoint) if value > 0)

    js = 0.5 * divergence(left) + 0.5 * divergence(right)
    return max(0.0, min(1.0, 1.0 - math.sqrt(max(0.0, js))))


def _scalar_similarity(left: float, right: float, scale: float) -> float:
    if not math.isfinite(left) or not math.isfinite(right) or scale <= 0:
        raise ValueError("behavior scalar must be finite and have positive scale")
    return math.exp(-abs(left - right) / scale)


@dataclass(frozen=True)
class BehaviorProfile:
    credential_id: str
    time_histogram: tuple[float, ...]
    size_histogram: tuple[float, ...]
    instrument_histogram: tuple[float, ...]
    buy_fraction: float
    log_mean_size: float
    log_mean_interarrival_ms: float
    event_count: int

    def normalized(self) -> "BehaviorProfile":
        if not self.credential_id or not 0 <= self.buy_fraction <= 1:
            raise ValueError("profile identity or buy fraction is invalid")
        if self.event_count <= 0:
            raise ValueError("profile needs at least one event")
        return BehaviorProfile(
            self.credential_id,
            _distribution(self.time_histogram, "time_histogram"),
            _distribution(self.size_histogram, "size_histogram"),
            _distribution(self.instrument_histogram, "instrument_histogram"),
            self.buy_fraction, self.log_mean_size,
            self.log_mean_interarrival_ms, self.event_count,
        )


@dataclass(frozen=True)
class Similarity:
    score: float
    components: Mapping[str, float]
    reliability: float


@dataclass(frozen=True)
class ReviewCandidate:
    left_credential: str
    right_credential: str
    score: float
    threshold: float
    reason_components: Mapping[str, float]
    disposition: str = "review_required"
    automatic_action: bool = False


class BehaviorScreen:
    """Versioned, interpretable score with a fail-safe KYC boundary."""

    VERSION = "qomm-behavior-v1"
    WEIGHTS = {
        "time": 0.25,
        "size": 0.20,
        "instrument": 0.20,
        "side": 0.10,
        "mean_size": 0.10,
        "interarrival": 0.15,
    }

    def compare(self, left: BehaviorProfile, right: BehaviorProfile) -> Similarity:
        left = left.normalized()
        right = right.normalized()
        components = {
            "time": _js_similarity(left.time_histogram, right.time_histogram),
            "size": _js_similarity(left.size_histogram, right.size_histogram),
            "instrument": _js_similarity(
                left.instrument_histogram, right.instrument_histogram),
            "side": _scalar_similarity(left.buy_fraction, right.buy_fraction, 0.20),
            "mean_size": _scalar_similarity(
                left.log_mean_size, right.log_mean_size, 0.70),
            "interarrival": _scalar_similarity(
                left.log_mean_interarrival_ms,
                right.log_mean_interarrival_ms, 0.80),
        }
        raw = sum(self.WEIGHTS[name] * value for name, value in components.items())
        reliability = min(1.0, math.sqrt(min(left.event_count, right.event_count) / 100))
        # Sparse histories shrink to an uninformative 0.5 rather than creating
        # confident matches from a handful of coincident events.
        score = 0.5 + reliability * (raw - 0.5)
        return Similarity(max(0.0, min(1.0, score)), components, reliability)

    def candidates(self, profiles: Sequence[BehaviorProfile],
                   authoritative_entities: Mapping[str, str],
                   threshold: float) -> list[ReviewCandidate]:
        if not 0 <= threshold <= 1:
            raise ValueError("review threshold must lie in [0,1]")
        normalized = [profile.normalized() for profile in profiles]
        if len({profile.credential_id for profile in normalized}) != len(normalized):
            raise ValueError("credential identifiers must be unique")
        if any(profile.credential_id not in authoritative_entities
               for profile in normalized):
            raise ValueError("every profile needs an authoritative KYC entity")
        output = []
        for index, left in enumerate(normalized):
            for right in normalized[index + 1:]:
                # Same KYC entity is already linked authoritatively and needs
                # no behavior-based allegation.
                if (authoritative_entities[left.credential_id]
                        == authoritative_entities[right.credential_id]):
                    continue
                similarity = self.compare(left, right)
                if similarity.score >= threshold:
                    output.append(ReviewCandidate(
                        left.credential_id, right.credential_id,
                        similarity.score, threshold, similarity.components))
        return sorted(output, key=lambda item: (-item.score,
                                                item.left_credential,
                                                item.right_credential))


def calibrate_threshold(scored_labels: Sequence[tuple[float, int]],
                        max_false_positive_rate: float = 0.01) -> float:
    """Choose the most sensitive threshold that stays inside a labeled FPR cap."""
    if not 0 <= max_false_positive_rate < 1:
        raise ValueError("false-positive cap must lie in [0,1)")
    if not scored_labels or any(label not in (0, 1) for _, label in scored_labels):
        raise ValueError("calibration needs binary labeled scores")
    negatives = sum(label == 0 for _, label in scored_labels)
    positives = sum(label == 1 for _, label in scored_labels)
    if negatives == 0 or positives == 0:
        raise ValueError("calibration needs both positive and negative pairs")
    candidates = sorted({score for score, _ in scored_labels}, reverse=True)
    candidates.append(1.000000000001)
    feasible = []
    for threshold in candidates:
        fp = sum(label == 0 and score >= threshold for score, label in scored_labels)
        tp = sum(label == 1 and score >= threshold for score, label in scored_labels)
        fpr = fp / negatives
        if fpr <= max_false_positive_rate:
            feasible.append((tp / positives, -fpr, -threshold, threshold))
    if not feasible:
        return 1.000000000001
    return max(feasible)[3]
