#!/usr/bin/env python3
"""Does correcting the disclosure mechanism change the result that rejected it?

Two faults were found in the same published statistic. Its noise carried the
replace-one sensitivity where the audited definition removes an entity outright,
doubling it for nothing; and publishing it as an absolute value biased it upward
by the noise scale, so balanced flow read as imbalanced and makers widened
against informed flow that was not there.

Both are fixed. The question this answers is whether the rejection --- that
differentially private disclosure left users and makers measurably worse off
than publishing nothing --- was a fact about disclosure or an artefact of those
two faults. Three arms on the same runs and the same seeds: no disclosure, the
mechanism as it was, and the mechanism as it should have been.

Paired differences, because the arms share a market and a seed; the marginal
intervals of each arm are much wider than the interval of the difference, and
reporting the former would hide an effect that is there or invent one that is
not.
"""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_sim.disclosure import DPDisclosure, EntityAccountant                # noqa: E402
from qomm_sim.engine import run_arm                                          # noqa: E402
from qomm_sim.experiment import DPParams, build_probes, make_disclosure       # noqa: E402
from qomm_sim.market import (                                                # noqa: E402
    ReferenceMarket, SimConfig, build_market_makers, build_requests,
)
from qomm_sim.tapes import TapeMarket, load_bybit, requests_from_tape         # noqa: E402
from scripts.hosts import this_host                                          # noqa: E402
from scripts.smallsample import mean_ci                                       # noqa: E402

ARMS = ("none", "dp_uncorrected", "dp_corrected")


def mechanism(kind: str, cfg: SimConfig, dp: DPParams):
    if kind == "none":
        return make_disclosure("A_none", cfg, dp)
    accountants = {e: EntityAccountant(dp.epsilon_total) for e in range(cfg.n_entities)}
    old = kind == "dp_uncorrected"
    return DPDisclosure(dp.epsilon_per_window, dp.request_cap, dp.volume_cap,
                        accountants, debias=not old,
                        signed_sensitivity_factor=2.0 if old else 1.0)


def interval(values: list[float]) -> dict:
    """Student, not normal: eight seeds do not know their own variance.

    This used to multiply by 1.96 at every n. At the eight seeds these arms
    actually run, the multiplier the sample earns is 2.365 --- 21% wider, and
    wide enough to change which of these intervals excludes zero.
    """
    return mean_ci(values)


def run(tape_path: Path | None, seeds, steps, window_steps, step_ms,
        entities, layer: str) -> dict:
    per_arm: dict[str, dict[str, list]] = {a: {"fill_rate": [], "mm_pnl_per_fill": []}
                                           for a in ARMS}
    paired: dict[str, dict[str, list]] = {a: {"fill_rate": [], "mm_pnl_per_fill": []}
                                          for a in ARMS if a != "none"}
    meta = {"source": "generated"}
    for seed in seeds:
        cfg = SimConfig(steps=steps, window_steps=window_steps, seed=seed)
        dp = DPParams()
        if tape_path is None:
            market = ReferenceMarket(cfg, cfg.seed)
            requests = build_requests(cfg, market, cfg.seed + 2)
        else:
            tape = load_bybit(tape_path, cfg, steps=steps, step_ms=step_ms)
            market = TapeMarket(cfg, tape, seed=cfg.seed)
            requests, cfg, meta = requests_from_tape(
                cfg, market, tape, synthetic_entities=entities, seed=cfg.seed + 2)
        makers = build_market_makers(cfg, cfg.seed + 1)
        probes = build_probes(cfg, cfg.seed + 3, 6)

        summary = {}
        for arm in ARMS:
            result = run_arm(cfg, market, list(requests), makers, "plain_rfq",
                             mechanism(arm, cfg, dp), seed=cfg.seed + 5,
                             probes=probes, reactive=(layer == "reactive"))
            summary[arm] = result.summary()
            for metric in ("fill_rate", "mm_pnl_per_fill"):
                per_arm[arm][metric].append(summary[arm][metric])
        for arm in paired:
            for metric in ("fill_rate", "mm_pnl_per_fill"):
                paired[arm][metric].append(summary[arm][metric] - summary["none"][metric])

    return {
        "meta": meta, "layer": layer,
        "levels": {a: {m: interval(v) for m, v in d.items()} for a, d in per_arm.items()},
        "paired_against_no_disclosure": {
            a: {m: interval(v) for m, v in d.items()} for a, d in paired.items()},
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, default=ROOT / "artifacts" / "dp_effect.json")
    ap.add_argument("--tape", type=Path, default=None)
    ap.add_argument("--tape-step-ms", type=int, default=1000)
    ap.add_argument("--tape-entities", type=int, default=24)
    ap.add_argument("--seeds", type=int, default=12)
    ap.add_argument("--seed0", type=int, default=20260818)
    ap.add_argument("--steps", type=int, default=48_000)
    ap.add_argument("--window-steps", type=int, default=1_200)
    args = ap.parse_args()

    seeds = [args.seed0 + i for i in range(args.seeds)]
    result = {"host": this_host(), "python": platform.python_version(),
              "seeds": args.seeds, "arms": {}}
    result["arms"]["generated"] = run(None, seeds, args.steps, args.window_steps,
                                      args.tape_step_ms, args.tape_entities, "reactive")
    if args.tape:
        result["arms"]["tape"] = run(args.tape, seeds, 2_400, 60, args.tape_step_ms,
                                     args.tape_entities, "reactive")

    for name, arm in result["arms"].items():
        print(f"\n{name} ({arm['meta'].get('source', name)}), paired against publishing nothing:")
        for kind, metrics in arm["paired_against_no_disclosure"].items():
            for metric, stat in metrics.items():
                if stat["mean"] is None:
                    continue
                mark = "significant" if stat["excludes_zero"] else "not significant"
                print(f"  {kind:16} {metric:16} {stat['mean']:+9.4f} "
                      f"+-{stat['half_width']:.4f}  ({mark})")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
