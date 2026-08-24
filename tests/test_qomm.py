"""Checks on the measuring instruments themselves, before trusting their output."""

from __future__ import annotations

import math
import random
import statistics
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_sim.attackers import auc, tpr_at_fpr                       # noqa: E402
from qomm_sim.audit import clopper_pearson                            # noqa: E402
from qomm_sim.disclosure import (                                     # noqa: E402
    DPDisclosure, EntityAccountant, PrivacyBudgetExceeded, ThresholdDisclosure,
    WindowObservation, discrete_laplace,
)
from qomm_sim.engine import run_arm                                    # noqa: E402
from qomm_sim.market import (                                          # noqa: E402
    INV_SCALE, ReferenceMarket, SimConfig, build_market_makers, build_requests,
)


# --- the circuit and the simulator must price identically -------------------

def test_the_reference_is_internally_consistent(tmp_path):
    """The reference's own summary agrees with its own quote list.

    It was named for a cross-check it does not perform --- the docstring said
    this held `gen_qomm`'s reference against `MarketMaker.quote` --- and the
    two could not be compared anyway: the simulator's half spread is a function
    of its informed-flow estimate and its eligibility turns on an inventory
    limit, while the generator prices a committed snapshot whose half spread is
    a field and whose eligibility turns on an expiry and an active flag. Two
    abstractions, on purpose.

    What this checks is real and narrow: that the winner the reference reports
    is the winner its own quotes imply. The second opinion the oracle needs ---
    the rule as stated, written independently --- is in
    `test_reference_has_a_second_opinion.py`.
    """
    program = tmp_path / "p.mpc"
    inputs = tmp_path / "in"
    reference = tmp_path / "ref.json"
    subprocess.run(
        [sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"),
         "--n-mm", "8", "--mode", "rfq", "--user-qty", "100",
         "--out-program", str(program), "--out-input-dir", str(inputs),
         "--out-reference", str(reference)],
        check=True, capture_output=True,
    )
    import json
    ref = json.loads(reference.read_text())
    eligible = [q for q in ref["quotes"] if q["eligible"]]
    assert eligible, "fixture must contain at least one eligible maker"
    assert ref["best_ask"] == min(q["ask"] for q in eligible)
    assert ref["best_bid"] == max(q["bid"] for q in eligible)
    # a buy takes the best ask
    assert ref["best_price"] == ref["best_ask"]
    assert ref["best_mm"] == ref["best_ask_mm"]


def test_inventory_skew_enters_the_quote_midpoint():
    """The probing attacker's estimator is only valid if this identity holds."""
    cfg = SimConfig(steps=10, n_mm=4)
    makers = build_market_makers(cfg, cfg.seed + 1)
    mm = next(m for m in makers if m.inv_coef > 0)
    mm.inventory = 320
    ask, bid = mm.quote(100_000, 50, 0.3)
    midpoint = 0.5 * (ask + bid) - 100_000
    assert midpoint == pytest.approx(mm.inv_coef * (mm.inventory // INV_SCALE))


# --- the DP mechanism must have the distribution it claims ------------------

def test_discrete_laplace_variance_matches_the_scale():
    rng = random.Random(11)
    epsilon, sensitivity = 0.5, 3.0
    samples = [discrete_laplace(epsilon, sensitivity, rng) for _ in range(200_000)]
    b = sensitivity / epsilon
    # two-sided geometric with parameter exp(-1/b): var = 2p/(1-p)^2, p = exp(-1/b)
    p = math.exp(-1.0 / b)
    expected = 2 * p / (1 - p) ** 2
    assert statistics.fmean(samples) == pytest.approx(0.0, abs=0.2)
    assert statistics.pvariance(samples) == pytest.approx(expected, rel=0.05)


def _observation(counts: dict[int, int]) -> WindowObservation:
    return WindowObservation(
        window=0, start_step=0, end_step=100,
        requests_by_entity=dict(counts),
        volume_by_entity={k: 50 * v for k, v in counts.items()},
        signed_volume_by_entity={k: 20 * v for k, v in counts.items()},
        fills_by_entity=dict(counts),
        fills=sum(counts.values()), requests=sum(counts.values()), no_quote=0,
        liquidity_lots_in_band=2000, makers_in_band=9,
        fills_by_bucket=(1, 1, 1), requests_by_bucket=(1, 1, 1),
    )


def test_entity_budget_stops_publication():
    accountants = {0: EntityAccountant(1.0), 1: EntityAccountant(1.0)}
    mech = DPDisclosure(0.6, 3, 300, accountants)
    rng = random.Random(3)
    obs = _observation({0: 2, 1: 1})
    assert mech.release(obs, rng).published
    second = mech.release(obs, rng)
    assert not second.published
    assert "budget" in second.suppressed_reason


def test_entity_clipping_bounds_one_entity_contribution():
    """One entity cannot move the released count by more than the cap."""
    rng = random.Random(5)
    cap = 3
    heavy = _observation({0: 50, 1: 1})
    without = _observation({1: 1})
    mech_a = DPDisclosure(1e6, cap, 300, {e: EntityAccountant(1e9) for e in range(4)})
    mech_b = DPDisclosure(1e6, cap, 300, {e: EntityAccountant(1e9) for e in range(4)})
    a = mech_a.release(heavy, rng).fields["exact_requests"]
    b = mech_b.release(without, rng).fields["exact_requests"]
    assert a - b == cap


def test_threshold_disclosure_suppresses_a_thin_market():
    thin = WindowObservation(
        window=0, start_step=0, end_step=10, requests_by_entity={0: 1},
        volume_by_entity={0: 10}, signed_volume_by_entity={0: 10},
        fills_by_entity={0: 1}, fills=1, requests=1, no_quote=0,
        liquidity_lots_in_band=10, makers_in_band=1,
        fills_by_bucket=(1, 0, 0), requests_by_bucket=(1, 0, 0),
    )
    release = ThresholdDisclosure().release(thin, random.Random(1))
    assert not release.published
    assert release.epsilon_spent == 0.0


# --- the scoring functions must agree with hand-computed values -------------

def test_auc_matches_hand_computed_values():
    assert auc([0.1, 0.4, 0.35, 0.8], [0, 0, 1, 1]) == pytest.approx(0.75)
    assert auc([1, 1, 1, 1], [0, 1, 0, 1]) == pytest.approx(0.5)   # all ties
    assert auc([1, 2], [0, 1]) == pytest.approx(1.0)
    assert auc([1, 1], [1, 1]) is None                              # one class only


def test_tpr_at_fpr_is_monotone_in_separation():
    labels = [1] * 50 + [0] * 50
    separated = [1.0] * 50 + [0.0] * 50
    overlapping = [0.5] * 100
    assert tpr_at_fpr(separated, labels) == pytest.approx(1.0)
    assert tpr_at_fpr(overlapping, labels) <= 0.1


def test_clopper_pearson_brackets_the_point_estimate():
    lower, upper = clopper_pearson(30, 100)
    assert lower < 0.30 < upper
    assert clopper_pearson(0, 100)[0] == 0.0
    assert clopper_pearson(100, 100)[1] == 1.0


# --- the leakage model must actually differ between arms --------------------

@pytest.mark.parametrize("protocol,sees_direction,sees_size", [
    ("plain_rfq", True, True),
    ("plain_rfm", False, True),
    ("plain_rfs", False, False),
    ("qomm_rfq", None, None),
])
def test_protocol_leakage_matches_the_documented_table(protocol, sees_direction, sees_size):
    cfg = SimConfig(steps=3_000, n_mm=8, n_entities=6, window_steps=1_000)
    market = ReferenceMarket(cfg, cfg.seed)
    makers = build_market_makers(cfg, cfg.seed + 1)
    requests = build_requests(cfg, market, cfg.seed + 2)
    from qomm_sim.disclosure import NoDisclosure
    result = run_arm(cfg, market, requests, makers, protocol, NoDisclosure(), seed=1)
    if sees_direction is None:
        assert result.observations == []
        return
    assert result.observations, "a plain protocol must leak the request to makers"
    assert all((o.direction is not None) == sees_direction for o in result.observations)
    assert all((o.size is not None) == sees_size for o in result.observations)


def test_reactive_layer_changes_realised_flow():
    cfg = SimConfig(steps=6_000, n_mm=8, n_entities=6, window_steps=1_000)
    market = ReferenceMarket(cfg, cfg.seed)
    makers = build_market_makers(cfg, cfg.seed + 1)
    requests = build_requests(cfg, market, cfg.seed + 2)
    from qomm_sim.disclosure import NoDisclosure
    replay = run_arm(cfg, market, requests, makers, "qomm_rfq", NoDisclosure(), seed=1)
    reactive = run_arm(cfg, market, requests, makers, "qomm_rfq", NoDisclosure(),
                       seed=1, reactive=True)
    assert len(reactive.truth) > len(replay.truth), "rejected flow must come back"
