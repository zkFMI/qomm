#!/usr/bin/env python3
"""What a slow quote costs, measured against what the price was worth anyway.

A cross-region committee answers in about 24 s (`placement_intercontinental.json`),
and the question that decides whether that matters is not the number of seconds
but whether the price moved in them. UniswapX fills carry a rate that really was
executable at a block, so the drift over a gap can be compared against the
dispersion the same market already shows *within* one block --- spread, size
impact, fee tier. If the drift sits inside that, a quote that took 24 s is not
distinguishable from one that took none.

Ethereum blocks are 12 s, so 24 s is two of them. This said 26 s throughout,
including in the artifact key, which was two blocks priced at 13 s each; the
number measured was always the two-block one.

**What the sample conditions on, and which way it leans.** Every observation
here is a *fill*: an order that settled. An order that did not settle leaves no
`Fill` log on Ethereum, so it cannot be in this file, and a quote that went stale
is exactly the kind of order that does not settle. The sample is therefore
selected on an outcome downstream of the thing being measured, and the direction
is not neutral --- drift that was large enough to stop a trade is missing from
the drift estimate. The figures below are a **lower bound** on what a stale quote
faces, and the bound leans the way the design would prefer. Nothing available in
this dataset removes that; only a feed of unfilled orders would.

Crypto on Ethereum L1 is the harshest case available in the other direction: an
instrument that moves less makes a slow quote cheaper, never dearer.

The fills are not shipped --- they are a few hundred megabytes recovered from an
archive node --- so this fails rather than substituting anything.
"""

from __future__ import annotations

import argparse
import collections
import json
import math
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.hosts import this_host                                # noqa: E402

BLOCK_SECONDS = 12.0


def series(path: Path) -> dict[tuple[str, str], list[tuple[int, float]]]:
    """One log-rate series per token pair, both directions folded together."""
    by_pair: dict[tuple[str, str], list[tuple[int, float]]] = collections.defaultdict(list)
    with path.open() as fh:
        for line in fh:
            record = json.loads(line)
            legs = record.get("legs") or []
            outs = [leg for leg in legs if leg.get("out")]
            ins = [leg for leg in legs if not leg.get("out")]
            if len(outs) != 1 or len(ins) != 1:
                continue
            amount_out, amount_in = outs[0]["amount"], ins[0]["amount"]
            if amount_out <= 0 or amount_in <= 0:
                continue
            token_out, token_in = outs[0]["token"], ins[0]["token"]
            flip = token_in > token_out
            key = (token_out, token_in) if flip else (token_in, token_out)
            rate = amount_out / amount_in
            by_pair[key].append((record["block"], math.log(1 / rate if flip else rate)))
    return by_pair


def basis_points(values: list[float]) -> dict | None:
    if not values:
        return None
    return {"median_bp": 1e4 * statistics.median(values), "n": len(values)}


def measure(rows: list[tuple[int, float]], gaps: list[int]) -> dict:
    per_block: dict[int, list[float]] = collections.defaultdict(list)
    for block, log_rate in rows:
        per_block[block].append(log_rate)
    blocks = sorted(per_block)

    within = []
    for block in blocks:
        seen = per_block[block]
        within.extend(abs(seen[i + 1] - seen[i]) for i in range(len(seen) - 1))

    across = {}
    for gap in gaps:
        moves = [abs(statistics.median(per_block[block + gap])
                     - statistics.median(per_block[block]))
                 for block in blocks if block + gap in per_block]
        across[gap] = basis_points(moves)

    result = {"observations": len(rows), "blocks": len(blocks),
              "within_block": basis_points(within), "across_blocks": across}
    floor = result["within_block"]
    for gap, moved in across.items():
        # A thin pair can have a zero within-block floor --- every trade in a
        # block at the same rate. There is no ratio to a zero floor, and
        # reporting one would be reporting an infinity as a measurement.
        if floor and moved and floor["median_bp"] > 0:
            result.setdefault("ratio_to_within_block", {})[gap] = (
                moved["median_bp"] / floor["median_bp"])
    return result


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fills", type=Path, required=True,
                    help="uniswapx_amounts.jsonl (not shipped; see the docstring)")
    ap.add_argument("--out", type=Path, default=ROOT / "artifacts" / "staleness.json")
    ap.add_argument("--pairs", type=int, default=4)
    ap.add_argument("--gaps", type=int, nargs="+", default=[2, 8, 25])
    args = ap.parse_args()

    if not args.fills.exists():
        raise SystemExit(f"{args.fills} is not here. This measurement needs the "
                         f"UniswapX fills, which are not shipped with the "
                         f"repository; nothing is substituted for them.")

    by_pair = series(args.fills)
    busiest = sorted(by_pair.items(), key=lambda kv: -len(kv[1]))[:args.pairs]
    result = {"host": this_host(), "block_seconds": BLOCK_SECONDS,
              "gaps_blocks": args.gaps,
              "gaps_seconds": [g * BLOCK_SECONDS for g in args.gaps],
              "pairs_seen": len(by_pair), "rows": []}
    for key, rows in busiest:
        row = {"pair": list(key)}
        row.update(measure(rows, args.gaps))
        result["rows"].append(row)
        floor = row["within_block"]["median_bp"]
        print(f"{key[0][:10]}../{key[1][:10]}..  n={row['observations']}")
        print(f"   within one block            {floor:6.1f} bp")
        for gap in args.gaps:
            moved = row["across_blocks"][gap]
            if moved:
                print(f"   {gap:2d} blocks (~{gap * BLOCK_SECONDS:4.0f} s)"
                      f"        {moved['median_bp']:6.1f} bp"
                      f"   = {moved['median_bp'] / floor:.2f}x the within-block floor")
        print()

    ratios = [row["ratio_to_within_block"][2] for row in result["rows"]
              if row.get("ratio_to_within_block", {}).get(2)]
    result["median_ratio_at_24s"] = statistics.median(ratios) if ratios else None
    result["conditioned_on"] = (
        "orders that settled; an order that went stale and did not settle leaves "
        "no Fill log, so the drift estimate is a lower bound")
    print(f"24 s of drift is {result['median_ratio_at_24s']:.2f}x the dispersion "
          f"the market already has inside one block")

    # The four busiest pairs were a post-hoc choice. Whether the answer depends
    # on it is a question the same data answers, so it is answered rather than
    # left to the reader.
    sensitivity = {}
    for k in (2, 4, 8, 16, 32):
        subset = sorted(by_pair.items(), key=lambda kv: -len(kv[1]))[:k]
        rs = []
        for _, rows in subset:
            m = measure(rows, [2])
            r = m.get("ratio_to_within_block", {}).get(2)
            if r:
                rs.append(r)
        sensitivity[k] = {"pairs": len(rs),
                          "median_ratio": statistics.median(rs) if rs else None}
    result["pair_count_sensitivity"] = sensitivity
    print("\nhow much the choice of four pairs mattered:")
    for k, v in sensitivity.items():
        got = v["median_ratio"]
        print(f"  busiest {k:2d} pairs: median ratio "
              f"{got:.2f}" if got else f"  busiest {k:2d} pairs: none")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
