#!/usr/bin/env python3
"""What the best possible disclosure is worth to a market maker.

The disclosure work rests on one sentence, in `market.py`:

    The premium is proportional to the maker's *estimate* of the informed
    fraction. A better estimate is worth money, which is the channel through
    which public market information can improve market-maker profitability.

Every negative result about disclosure so far has been about the *mechanism* ---
a noise scale of 1200 against a median true imbalance of 428, a signal-to-noise
ceiling that needs 35 firms in one window. Those are arguments that the
disclosure on offer is too noisy to use. They leave open that a better one would
help.

This closes it. The arm below hands the maker the window's true informed
fraction with near-zero variance. No mechanism beats that, private or not, so it
is the ceiling on what any disclosure of this quantity could be worth. If the
ceiling is negative, no amount of engineering the mechanism reaches a positive.

**What this can and cannot establish.** The maker here does not choose how to
use the figure: `BeliefState.combined` substitutes it by precision weighting, and
an exact signal has enough weight to overwrite the maker's own estimate
entirely. So this measures *naive substitution of the market-wide figure for the
maker's own conditional estimate*, not the value of the information to a maker
free to use it as it likes. A maker free to ignore it cannot be worse off for
having it --- that is a theorem, not a measurement --- so any loss here is a
property of the prescribed use, of competition, or of both.

Which is why the arms below separate them. Giving the figure to **one** maker
isolates whatever private value it has; giving it to **all** adds the effect of
every maker narrowing at once. Measured, the single recipient gains nothing
detectable and the collective loses, which points at the prescribed use rather
than at competition --- and leaves the value of the information *well used*
unmeasured, because nothing here optimises over uses.

The mechanism suggested by the direction is the winner's curse: a maker's own
estimate is formed on the flow it filled, and it fills by winning an auction, so
that flow is more informed than the market's average. The market-wide figure is
then the wrong conditioning. That is an explanation of the sign, offered as one,
not something these runs establish.
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.hosts import this_host                                  # noqa: E402
from scripts.smallsample import mean_ci                              # noqa: E402
from qomm_sim.disclosure import DisclosureMechanism, Release         # noqa: E402
from qomm_sim.engine import run_arm                                  # noqa: E402
from qomm_sim.experiment import DPParams, SimConfig, make_disclosure  # noqa: E402
from qomm_sim.market import (ReferenceMarket, build_market_makers,   # noqa: E402
                             build_requests)


class Oracle(DisclosureMechanism):
    """The true informed fraction, free and exact.

    `reaches` is the set of makers it reaches; `None` is everybody. One maker
    and every maker are different experiments and the difference is the point.
    """

    name = "Z_oracle"

    def __init__(self, phi_by_window: dict[int, float], reaches=None):
        self.phi = phi_by_window
        self.reaches = reaches

    def release(self, obs, rng):
        return Release(obs.window, self.name, True,
                       {"phi": self.phi.get(obs.window, 0.45)}, 0.0, None)

    def public_signal(self, release):
        if not release.published:
            return None, float("inf")
        return release.fields["phi"], 1e-4


def one(seed: int, arm: str, steps: int, window_steps: int) -> dict:
    cfg = SimConfig(steps=steps, window_steps=window_steps, seed=seed)
    market = ReferenceMarket(cfg, cfg.seed)
    makers = build_market_makers(cfg, cfg.seed + 1)
    requests = build_requests(cfg, market, cfg.seed + 2)
    by_window: dict[int, list[int]] = {}
    for r in requests:
        by_window.setdefault(r.step // cfg.window_steps, []).append(r.informed)
    phi = {k: sum(v) / len(v) for k, v in by_window.items() if v}
    if arm == "oracle":
        disclosure = Oracle(phi)
    elif arm == "oracle_one":
        disclosure = Oracle(phi, reaches={0})
    else:
        disclosure = make_disclosure(arm, cfg, DPParams())
    res = run_arm(cfg, market, requests, makers, "qomm_rfq", disclosure,
                  seed=cfg.seed + 5)
    total = sum(res.mm_pnl.values())
    return {
        "mm0_pnl": res.mm_pnl.get(0, 0.0),
        "others_pnl": sum(v for k, v in res.mm_pnl.items() if k != 0),
        "fill_rate": res.fills / max(1, res.requests),
        "mm_pnl_per_fill": total / max(1, res.fills),
        "mm_pnl_total": total,
        "user_cost_ticks": (statistics.fmean(res.user_cost_ticks)
                            if res.user_cost_ticks else 0.0),
        "true_phi": statistics.fmean(phi.values()) if phi else 0.0,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path,
                    default=ROOT / "artifacts" / "disclosure_ceiling.json")
    ap.add_argument("--seeds", type=int, default=12)
    ap.add_argument("--seed0", type=int, default=11)
    ap.add_argument("--steps", type=int, default=48_000)
    ap.add_argument("--window-steps", type=int, default=1_200)
    args = ap.parse_args()

    seeds = [args.seed0 + 7 * i for i in range(args.seeds)]
    fields = ("fill_rate", "mm_pnl_per_fill", "mm_pnl_total", "user_cost_ticks",
              "mm0_pnl", "others_pnl")
    paired = {f: [] for f in fields}
    paired_one = {f: [] for f in fields}
    rows = []
    for seed in seeds:
        none = one(seed, "A_none", args.steps, args.window_steps)
        oracle = one(seed, "oracle", args.steps, args.window_steps)
        single = one(seed, "oracle_one", args.steps, args.window_steps)
        for f in fields:
            paired[f].append(oracle[f] - none[f])
            paired_one[f].append(single[f] - none[f])
        rows.append({"seed": seed, "none": none, "oracle": oracle,
                     "oracle_one": single})
        print(f"seed {seed}: pnl/fill {none['mm_pnl_per_fill']:8.1f} -> "
              f"{oracle['mm_pnl_per_fill']:8.1f}   user "
              f"{none['user_cost_ticks']:6.1f} -> {oracle['user_cost_ticks']:6.1f}",
              flush=True)

    summary = {f: mean_ci(paired[f]) for f in fields}
    summary_one = {f: mean_ci(paired_one[f]) for f in fields}
    payload = {
        "host": this_host(),
        "question": "what the best possible disclosure of the informed fraction "
                    "is worth to a market maker",
        "arm": "the true phi, exact and free, which no mechanism beats",
        "seeds": len(seeds), "steps": args.steps,
        "true_phi_mean": statistics.fmean(r["none"]["true_phi"] for r in rows),
        "everyone_informed_minus_none": summary,
        "one_maker_informed_minus_none": summary_one,
        "what_this_measures": "naive substitution of the market-wide figure for "
                              "the maker's own conditional estimate, which is "
                              "what BeliefState.combined prescribes; not the "
                              "value of the information to a maker free to use "
                              "it as it likes",
        "reading": "a maker's own estimate is formed on the flow it won, which "
                   "is more informed than the market's average; the market-wide "
                   "figure makes it quote for average flow and win worse flow",
        "rows": rows,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=1) + "\n")
    print()
    for label, table in (("everyone informed", summary),
                         ("one maker informed", summary_one)):
        print(f"  --- {label} ---")
        for f in fields:
            st = table[f]
            print(f"  {f:18s} {st['mean']:+14.2f} +- {st['half_width']:12.2f}   "
                  f"{'excludes zero' if st['excludes_zero'] else 'crosses zero'}")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
