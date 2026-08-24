#!/usr/bin/env python3
"""How many probes it takes to read a maker's inventory off its own quotes.

`DEPLOYMENT.md` calls the per-entity cap "the only defence" against this attack
and tells an operator to set the level from the measured probe count. The paper
says "about ten probes" three times, marked as measured. Nothing in `artifacts/`
carried that measurement: the probing attacker was only ever run at the full
budget, 240. So the number the cap is supposed to be set from did not exist, and
this is it.

The attack correlates the midpoint of a firm two-sided quote with the maker's
net inventory --- the half spread cancels, so the midpoint moves with inventory
and nothing else. Its error falls like the square root of the budget, so a small
budget makes the estimate noisy rather than absent, and the question is where it
stops being noise.

Query-obliviousness does not touch this. A firm price is what the protocol is
built to return, and returning it is what leaks. That is why the answer matters:
it is the one attack the cryptography does not address.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_sim import attackers as atk                                 # noqa: E402
from qomm_sim.engine import run_arm                                   # noqa: E402
from qomm_sim.experiment import DPParams, build_probes, make_disclosure  # noqa: E402
from qomm_sim.market import (                                         # noqa: E402
    ReferenceMarket, SimConfig, build_market_makers, build_requests,
)

BUDGETS = [4, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96, 128, 192, 240]

# A correlation estimated from four points is large by construction: the
# estimator is biased upward when the sample is small, and the first run of this
# script reported |r| = 0.93 at four probes and 0.55 at two hundred and forty.
# Reading that as "the attack works at four" would be reading the bias. The test
# is therefore whether the correlation is distinguishable from zero at that
# sample size, not whether it is large.
CONFIDENCE = 0.95


def significant(r: float, n: int) -> bool:
    """Is this correlation distinguishable from zero at `n` points?

    The t statistic of a correlation is `r sqrt(n-2) / sqrt(1-r^2)` on `n-2`
    degrees of freedom. The critical value is approximated by the normal
    quantile with a small-sample correction, which is enough to separate "the
    attacker can act on this" from "the estimator is flattering itself"; the
    two verdicts here are three budgets apart, so the approximation does not
    decide anything close.
    """
    if n < 4 or abs(r) >= 1.0:
        return False
    t_stat = abs(r) * math.sqrt(n - 2) / math.sqrt(1 - r * r)
    degrees = n - 2
    critical = 1.96 * (1 + 2.0 / degrees)
    return t_stat > critical


def one_seed(seed: int, probes_per_window: int) -> dict[int, float]:
    cfg = SimConfig(seed=seed)
    market = ReferenceMarket(cfg, seed)
    makers = build_market_makers(cfg, seed + 1)
    requests = build_requests(cfg, market, seed + 2)
    probes = build_probes(cfg, seed + 3, probes_per_window)
    result = run_arm(cfg, market, requests, makers, "qomm",
                     make_disclosure("A_none", cfg, DPParams()),
                     seed=seed + 5, probes=probes, reactive=False)
    out: dict[int, float] = {}
    for budget in BUDGETS:
        report = atk.probing_entity(result, cfg, probe_budget=budget)
        value = report.extra.get("net_inventory_corr_from_best_quote")
        if value is not None:
            out[budget] = abs(float(value))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--seeds", type=int, default=24)
    ap.add_argument("--probes-per-window", type=int, default=4)
    ap.add_argument("--out", type=Path, default=ROOT / "artifacts" / "probe_budget.json")
    args = ap.parse_args()

    by_budget: dict[int, list[float]] = {b: [] for b in BUDGETS}
    for seed in range(args.seeds):
        for budget, value in one_seed(seed, args.probes_per_window).items():
            by_budget[budget].append(value)

    rows = []
    print(f"{'probes':>7} {'median |r|':>11} {'p25':>7} {'p75':>7} "
          f"{'significant':>12} {'seeds':>6}")
    for budget in BUDGETS:
        values = sorted(by_budget[budget])
        if not values:
            continue
        median = statistics.median(values)
        low = values[len(values) // 4]
        high = values[(3 * len(values)) // 4]
        share = sum(significant(v, budget) for v in values) / len(values)
        print(f"{budget:>7} {median:>11.3f} {low:>7.3f} {high:>7.3f} "
              f"{share:>11.0%} {len(values):>6}")
        rows.append({"probes": budget, "median_abs_corr": round(median, 4),
                     "p25": round(low, 4), "p75": round(high, 4),
                     "share_significant": round(share, 3), "seeds": len(values)})

    # the first budget at which a majority of seeds give a correlation the
    # attacker could act on
    usable = next((r["probes"] for r in rows if r["share_significant"] >= 0.5), None)
    print(f"\nfirst budget at which most seeds give a correlation "
          f"distinguishable from zero: "
          f"{usable if usable is not None else 'none in the range'}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "what": "how many probes recover a maker's net inventory from its own "
                "two-sided quotes",
        "why": "DEPLOYMENT.md tells an operator to set the per-entity cap from "
               "this number and the number was not measured anywhere",
        "attack": "correlate the midpoint of a firm two-sided quote with the "
                  "maker's net inventory; the half spread cancels",
        "verdict": "a correlation distinguishable from zero at 95% for most seeds",
        "first_usable_budget": usable,
        "seeds": args.seeds,
        "probes_per_window": args.probes_per_window,
        "rows": rows,
    }, indent=2) + "\n")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
