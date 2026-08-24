#!/usr/bin/env python3
"""What a block-range question is worth, and the width at which it stops being.

The maker in this design never receives the request. It sees its own fills and
nothing else, so it knows the numerator of its hit rate and can never know the
denominator. That denominator is the one quantity the venue holds exclusively:
the *fill* count is on-chain and exact --- settlement is one instruction per
trade at 18.2M-65.7M gas --- and a request that settles nothing leaves no trace
at all, which is what the unsettled-request attack falling to AUC 0.500 means.

So the question is over requests, over a range of blocks, counting distinct
entities rather than events. Predictions are in
`artifacts/block_range_query_prediction.json`, written before this ran:

  P1  sensitivity 1 at every width, against `cap x windows` for an event count
  P2  sd 1.357 entities at epsilon 1
  P3  usefulness is non-monotonic: noise below, saturation above
  P4  the generated market saturates within 5 windows and cannot evaluate this

P4 is the one worth running. If it holds, the generated market is the wrong
instrument for the same reason section 14 found the band count was, and only the
tape can say anything.
"""
from __future__ import annotations

import argparse
import json
import math
import random
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.hosts import this_host                                   # noqa: E402
from qomm_sim import lab                                              # noqa: E402
from qomm_sim.disclosure import EntityAccountant                      # noqa: E402
from qomm_sim.engine import run_arm                                   # noqa: E402
from qomm_sim.experiment import DPParams, make_disclosure             # noqa: E402
from qomm_sim.queries import (BlockRangeQuery, SENSITIVITY,           # noqa: E402
                              answer_block_range_query,
                              event_count_sensitivity, expected_distinct,
                              noise_scale, windows_in_range)

SATURATION = 0.95


def windows_of(setup, seed: int):
    disclosure = make_disclosure("A_none", setup.cfg, DPParams())
    res = run_arm(setup.cfg, setup.market, setup.requests, setup.makers,
                  "qomm_rfq", disclosure, seed=seed)
    return res.windows


def curve(windows, enrolled: int) -> list[dict]:
    """True distinct count against range width, averaged over start positions."""
    rows = []
    total = len(windows)
    for width in range(1, total + 1):
        counts = []
        for start in range(0, total - width + 1):
            span = windows[start:start + width]
            counts.append(len({e for w in span for e in w.requests_by_entity}))
        mean = statistics.fmean(counts)
        rows.append({
            "width_windows": width,
            "true_distinct_mean": mean,
            "fraction_of_enrolment": mean / enrolled if enrolled else 0.0,
            "starts": len(counts),
        })
    return rows


def saturation_width(rows, enrolled: int) -> int | None:
    for row in rows:
        if enrolled and row["true_distinct_mean"] >= SATURATION * enrolled:
            return row["width_windows"]
    return None


def measured_sensitivity(windows) -> dict:
    """P1, at several widths: drop the busiest entity and see what moves."""
    out = []
    for width in (1, 5, 10, len(windows)):
        if width > len(windows):
            continue
        span = windows[:width]
        present = {e for w in span for e in w.requests_by_entity}
        if not present:
            continue
        counted = {}
        for w in span:
            for e, n in w.requests_by_entity.items():
                counted[e] = counted.get(e, 0) + n
        busiest = max(counted, key=counted.get)
        without = len(present - {busiest})
        out.append({
            "width_windows": width,
            "entity_count_moves_by": len(present) - without,
            "that_entity_made_requests": counted[busiest],
            "event_count_would_move_by_at_most":
                event_count_sensitivity(width, max(counted.values())),
        })
    return {"rows": out,
            "entity_sensitivity_is_flat": all(r["entity_count_moves_by"] == SENSITIVITY
                                              for r in out)}


def noise_check(windows, epsilon: float, draws: int, seed: int) -> dict:
    rng = random.Random(seed)
    query = BlockRangeQuery(windows[0].start_step, windows[-1].end_step)
    truth = len({e for w in windows_in_range(windows, query)
                 for e in w.requests_by_entity})
    errors = []
    for _ in range(draws):
        answer = answer_block_range_query(windows, query, epsilon,
                                          EntityAccountant(epsilon_total=1e9), rng)
        errors.append(answer.count - truth)
    p = math.exp(-epsilon / SENSITIVITY)
    return {
        "epsilon": epsilon,
        "true_distinct": truth,
        "sd_measured": (sum(e * e for e in errors) / len(errors)) ** 0.5,
        "sd_predicted": math.sqrt(2 * p) / (1 - p),
        "noise_scale_entities": noise_scale(epsilon),
        "draws": draws,
    }


def arm(name: str, setup, seeds: int, epsilon: float, draws: int) -> dict:
    per_seed = []
    for k in range(seeds):
        windows = windows_of(setup, setup.cfg.seed + 5 + k)
        enrolled = len({e for w in windows for e in w.requests_by_entity})
        rows = curve(windows, enrolled)
        per_seed.append({
            "enrolled_seen": enrolled,
            "windows": len(windows),
            "saturation_width": saturation_width(rows, enrolled),
            "curve": rows,
            "sensitivity": measured_sensitivity(windows),
            "noise": noise_check(windows, epsilon, draws, setup.cfg.seed + 900 + k),
        })
    widths = [s["saturation_width"] for s in per_seed if s["saturation_width"]]
    return {
        "arm": name,
        "meta": setup.meta,
        "seeds": seeds,
        "enrolment_seen_median": statistics.median(s["enrolled_seen"] for s in per_seed),
        "saturation_width_median": statistics.median(widths) if widths else None,
        "saturation_width_all": widths,
        "sensitivity_flat_every_seed": all(s["sensitivity"]["entity_sensitivity_is_flat"]
                                           for s in per_seed),
        "sd_measured_median": statistics.median(s["noise"]["sd_measured"] for s in per_seed),
        "sd_predicted": per_seed[0]["noise"]["sd_predicted"],
        "per_seed": per_seed,
    }


def real_identity_arm(path: Path, epsilon: float, widths, samples: int,
                      seed: int) -> dict:
    """The only source here with real requesting identities.

    The simulator assigns entities round robin, which is the least skewed
    assignment there is and so the one that saturates fastest --- P4 held on it
    and the result was a property of the assignment. UniswapX names the swapper
    on every fill, so the question can be asked of a real population.

    The comparison that matters is not whether the entity count correlates with
    the fill count --- it must, both rise with activity --- but whether what is
    LEFT after the fill count is bigger than the noise. The fill count is free
    from the chain, so only the residual is what the epsilon buys. That residual
    is flow concentration: the same 500 fills from 500 takers and from 20 takers
    are different venues, and it is the difference a maker pricing adverse
    selection wants.
    """
    import bisect

    rows = []
    with path.open() as handle:
        for line in handle:
            record = json.loads(line)
            rows.append((record["block"], record["swapper"]))
    rows.sort()
    blocks = [b for b, _ in rows]
    swappers = [s for _, s in rows]
    lo_block, hi_block = blocks[0], blocks[-1]

    out = []
    for width in widths:
        if hi_block - lo_block <= width:
            continue
        rng = random.Random(seed + width)
        starts = rng.sample(range(lo_block, hi_block - width), samples)
        pairs = []
        for start in starts:
            i = bisect.bisect_left(blocks, start)
            j = bisect.bisect_left(blocks, start + width)
            seg = swappers[i:j]
            pairs.append((len(seg), len(set(seg))))
        fills = [f for f, _ in pairs]
        distinct = [d for _, d in pairs]
        mf, md = statistics.fmean(fills), statistics.fmean(distinct)
        sxy = sum((f - mf) * (d - md) for f, d in pairs)
        sxx = sum((f - mf) ** 2 for f in fills)
        syy = sum((d - md) ** 2 for d in distinct)
        if not sxx or not syy:
            continue
        slope = sxy / sxx
        resid = [d - (md + slope * (f - mf)) for f, d in pairs]
        resid_sd = statistics.pstdev(resid)
        out.append({
            "width_blocks": width,
            "width_hours_at_12s": width * 12 / 3600,
            "mean_fills": mf,
            "mean_distinct_swappers": md,
            "r2_against_public_fill_count": (sxy * sxy) / (sxx * syy),
            "slope_distinct_per_fill": slope,
            "residual_sd_entities": resid_sd,
            "dp_noise_sd_entities": math.sqrt(2 * math.exp(-epsilon / SENSITIVITY))
                                    / (1 - math.exp(-epsilon / SENSITIVITY)),
            "signal_to_noise": resid_sd / (math.sqrt(2 * math.exp(-epsilon / SENSITIVITY))
                                           / (1 - math.exp(-epsilon / SENSITIVITY))),
            "samples": samples,
        })

    counts: dict[str, int] = {}
    for _, swapper in rows:
        counts[swapper] = counts.get(swapper, 0) + 1
    tally = sorted(counts.values())
    return {
        "arm": "uniswapx_real_identities",
        "source": path.name,
        "fills": len(rows),
        "distinct_swappers": len(counts),
        "fills_per_swapper": len(rows) / len(counts),
        "swappers_appearing_once": sum(1 for v in tally if v == 1) / len(tally),
        "top_1pct_share_of_fills": sum(tally[-max(1, len(tally) // 100):]) / len(rows),
        "block_span": hi_block - lo_block,
        "saturates": False,
        "why_not": "48,000 swappers over 1.43M blocks with 60% appearing once. "
                   "The pool is unbounded against any range an asker names, so "
                   "the count is close to linear in width. The one-window "
                   "saturation on the simulator was the round-robin assignment.",
        "what_this_cannot_say": "UniswapX records fills, so these are settled "
                                "requests. QOMM's denominator includes requests "
                                "that settled nothing, and no chain records "
                                "those. If they come from a different "
                                "population the relation here does not carry.",
        "rows": out,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path,
                    default=ROOT / "artifacts" / "block_range_query.json")
    ap.add_argument("--seeds", type=int, default=5)
    ap.add_argument("--epsilon", type=float, default=1.0)
    ap.add_argument("--draws", type=int, default=4_000)
    ap.add_argument("--tape", type=Path,
                    default=ROOT / "artifacts" / "tapes" / "LTCUSDT2021-06-15.csv")
    ap.add_argument("--uniswapx", type=Path,
                    default=ROOT / "artifacts" / "tapes" / "uniswapx_amounts.jsonl")
    ap.add_argument("--samples", type=int, default=400)
    args = ap.parse_args()

    arms = [arm("generated", lab.build(), args.seeds, args.epsilon, args.draws)]
    if args.tape.exists():
        arms.append(arm("tape", lab.build(tape=args.tape), args.seeds,
                        args.epsilon, args.draws))
    real = None
    if args.uniswapx.exists():
        real = real_identity_arm(args.uniswapx, args.epsilon,
                                 (100, 600, 3_000, 7_200, 50_000),
                                 args.samples, 20260824)

    payload = {
        "host": this_host(),
        "question": "whether a block-range question about distinct requesting "
                    "entities carries anything, and at what range width",
        "prediction": "artifacts/block_range_query_prediction.json",
        "saturation_definition": f"width at which the true distinct count "
                                 f"reaches {SATURATION:.0%} of the entities seen",
        "not_measured_on_purpose": "the fill count. Settlement publishes one "
                                   "on-chain instruction per trade, so that "
                                   "count is exact and free to anyone with a "
                                   "node; a noised version would protect "
                                   "nothing and be worse than the free answer.",
        "arms": arms,
        "real_identities": real,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=1) + "\n")

    for a in arms:
        print(f"\n--- {a['arm']} ({a['meta'].get('source', '?')}) ---")
        print(f"  entities seen        {a['enrolment_seen_median']:.0f}")
        print(f"  saturation width     {a['saturation_width_median']} windows "
              f"(predicted <= 5 for generated)")
        print(f"  sensitivity flat     {a['sensitivity_flat_every_seed']}")
        print(f"  sd measured          {a['sd_measured_median']:.3f} "
              f"against {a['sd_predicted']:.3f} predicted")
        row = a["per_seed"][0]["curve"]
        for r in row[:8]:
            print(f"    width {r['width_windows']:3d}  "
                  f"{r['true_distinct_mean']:6.2f} entities  "
                  f"{r['fraction_of_enrolment']:.0%}")
    if real:
        print(f"\n--- {real['arm']} ({real['source']}) ---")
        print(f"  {real['fills']:,} fills, {real['distinct_swappers']:,} swappers, "
              f"{real['swappers_appearing_once']:.1%} appear once")
        print("  the public fill count is free; only the residual is bought")
        for r in real["rows"]:
            print(f"    {r['width_blocks']:6d} blocks ({r['width_hours_at_12s']:5.1f} h)  "
                  f"R^2 {r['r2_against_public_fill_count']:.4f}  "
                  f"residual {r['residual_sd_entities']:7.2f}  "
                  f"noise {r['dp_noise_sd_entities']:.2f}  "
                  f"SNR {r['signal_to_noise']:6.1f}")
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
