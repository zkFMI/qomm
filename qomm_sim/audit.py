"""Two-world audit of the entity-level DP claim.

An attacker that predicts "entity e was active" from "this window was busy" beats
a coin flip even under perfect differential privacy, because individual activity
and aggregate activity are correlated in the population. That correlation is not
a privacy failure, so an AUC-style attack cannot by itself confirm or refute the
epsilon claim.

The test that does match the definition is the two-world game the definition is
written in. Fix everything except whether one entity's contributions are present:

    D  = the window as observed
    D' = the same window with every request, quote and trade of entity e removed

Run the release mechanism many times on each world and measure how well any
threshold rule separates them. The best such rule gives an empirical lower bound

    eps_emp = max( ln(TPR_lo / FPR_hi), ln((1-FPR_lo) / (1-TPR_hi)) )

using one-sided Clopper-Pearson bounds so that the estimate is not inflated by
sampling noise. If eps_emp ever exceeds the declared epsilon, the implementation
does not deliver what it claims.
"""

from __future__ import annotations

import math
import random
from dataclasses import dataclass

from .disclosure import DPDisclosure, EntityAccountant, WindowObservation


def _beta_ppf(alpha: float, a: float, b: float) -> float:
    """Inverse regularised incomplete beta by bisection (no SciPy dependency)."""
    if a <= 0:
        return 0.0
    if b <= 0:
        return 1.0
    lo, hi = 0.0, 1.0
    for _ in range(200):
        mid = 0.5 * (lo + hi)
        if _betainc(a, b, mid) < alpha:
            lo = mid
        else:
            hi = mid
    return 0.5 * (lo + hi)


def _betainc(a: float, b: float, x: float) -> float:
    """Regularised incomplete beta via the continued fraction."""
    if x <= 0:
        return 0.0
    if x >= 1:
        return 1.0
    lbeta = math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
    front = math.exp(lbeta + a * math.log(x) + b * math.log(1 - x))
    if x < (a + 1) / (a + b + 2):
        return front * _betacf(a, b, x) / a
    return 1.0 - math.exp(
        math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
        + b * math.log(1 - x) + a * math.log(x)
    ) * _betacf(b, a, 1 - x) / b


def _betacf(a: float, b: float, x: float, iterations: int = 200) -> float:
    tiny = 1e-30
    qab, qap, qam = a + b, a + 1.0, a - 1.0
    c = 1.0
    d = 1.0 - qab * x / qap
    if abs(d) < tiny:
        d = tiny
    d = 1.0 / d
    h = d
    for m in range(1, iterations + 1):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < tiny:
            d = tiny
        c = 1.0 + aa / c
        if abs(c) < tiny:
            c = tiny
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < tiny:
            d = tiny
        c = 1.0 + aa / c
        if abs(c) < tiny:
            c = tiny
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < 1e-12:
            break
    return h


def clopper_pearson(k: int, n: int, alpha: float = 0.05) -> tuple[float, float]:
    lower = 0.0 if k == 0 else _beta_ppf(alpha / 2, k, n - k + 1)
    upper = 1.0 if k == n else _beta_ppf(1 - alpha / 2, k + 1, n - k)
    return lower, upper


@dataclass(frozen=True)
class AuditResult:
    window: int
    entity: int
    trials: int
    declared_epsilon: float
    field_epsilon: float
    empirical_epsilon: float
    best_threshold: float
    within_claim: bool
    entity_requests: int
    entity_volume: int


def _drop_entity(obs: WindowObservation, entity: int) -> WindowObservation:
    return WindowObservation(
        window=obs.window, start_step=obs.start_step, end_step=obs.end_step,
        requests_by_entity={k: v for k, v in obs.requests_by_entity.items() if k != entity},
        volume_by_entity={k: v for k, v in obs.volume_by_entity.items() if k != entity},
        signed_volume_by_entity={k: v for k, v in obs.signed_volume_by_entity.items()
                                 if k != entity},
        fills_by_entity={k: v for k, v in obs.fills_by_entity.items() if k != entity},
        fills=obs.fills - obs.fills_by_entity.get(entity, 0),
        requests=obs.requests - obs.requests_by_entity.get(entity, 0),
        no_quote=obs.no_quote,
        liquidity_lots_in_band=obs.liquidity_lots_in_band, makers_in_band=obs.makers_in_band,
        fills_by_bucket=obs.fills_by_bucket, requests_by_bucket=obs.requests_by_bucket,
    )


def audit_window(
    obs: WindowObservation,
    entity: int,
    epsilon_per_window: float,
    request_cap: int,
    volume_cap: int,
    trials: int = 4000,
    seed: int = 1,
    n_entities: int = 64,
    n_fields: int = 4,
    field: str = "noisy_requests",
    signed_sensitivity_factor: float = 1.0,
) -> AuditResult:
    """Distinguish "entity present" from "entity absent" using one released field.

    Only the request count used to be testable here, which left the other three
    fields carrying an epsilon claim that nothing had ever bound. The signed
    volume matters most of the three, because it is the one whose noise scale is
    in question.
    """
    rng = random.Random(seed)
    world_in = obs
    world_out = _drop_entity(obs, entity)

    def sample(world: WindowObservation) -> list[float]:
        values = []
        for _ in range(trials):
            accountants = {e: EntityAccountant(1e9) for e in range(n_entities)}
            mech = DPDisclosure(epsilon_per_window, request_cap, volume_cap, accountants,
                                signed_sensitivity_factor=signed_sensitivity_factor)
            release = mech.release(world, rng)
            values.append(float(release.fields[field]))
        return values

    samples_in = sample(world_in)
    samples_out = sample(world_out)

    best_eps = 0.0
    best_threshold = 0.0
    candidates = sorted({round(v) for v in samples_in + samples_out})
    # The best threshold is chosen after looking at the samples, so a single
    # 95% interval per threshold is not enough: scanning many thresholds would
    # report a violation on a correct mechanism roughly one time in twenty.
    # Bonferroni over the candidate set keeps the bound valid.
    alpha = 0.05 / max(1, len(candidates))
    for threshold in candidates:
        k_in = sum(1 for v in samples_in if v >= threshold)
        k_out = sum(1 for v in samples_out if v >= threshold)
        tpr_lo, tpr_hi = clopper_pearson(k_in, trials, alpha)
        fpr_lo, fpr_hi = clopper_pearson(k_out, trials, alpha)
        for num, den in ((tpr_lo, fpr_hi), (1 - fpr_hi, 1 - tpr_lo)):
            if den > 0 and num > 0:
                best = math.log(num / den)
                if best > best_eps:
                    best_eps = best
                    best_threshold = threshold
    # The audited statistic is one of n_fields released per window, so the
    # binding claim for it is epsilon/n_fields, not the whole-window budget.
    field_epsilon = epsilon_per_window / n_fields
    return AuditResult(
        window=obs.window, entity=entity, trials=trials,
        declared_epsilon=epsilon_per_window,
        field_epsilon=field_epsilon,
        empirical_epsilon=best_eps, best_threshold=best_threshold,
        within_claim=best_eps <= field_epsilon + 1e-9,
        entity_requests=obs.requests_by_entity.get(entity, 0),
        entity_volume=obs.volume_by_entity.get(entity, 0),
    )
