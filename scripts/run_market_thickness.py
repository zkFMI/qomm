#!/usr/bin/env python3
"""Smoke comparison across maker count and requested order size.

Every plain/QOMM pair uses the same reference market, requests, makers, probes
and random seed. This is synthetic smoke evidence, not a production forecast.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from dataclasses import asdict, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_sim.attackers import passive_observer, pretrade_attributes
from qomm_sim.disclosure import NoDisclosure
from qomm_sim.engine import ProbeResult, run_arm
from qomm_sim.experiment import build_probes
from qomm_sim.market import (ReferenceMarket, SimConfig, build_market_makers,
                             build_requests)
from scripts.hosts import this_host


REGIMES = {
    "small": (1, 20, 10),
    "medium": (21, 100, 50),
    "large": (101, 400, 200),
}


def pearson(xs: list[float], ys: list[float]) -> float | None:
    if len(xs) < 4 or len(xs) != len(ys):
        return None
    xbar = statistics.fmean(xs)
    ybar = statistics.fmean(ys)
    numerator = sum((x - xbar) * (y - ybar) for x, y in zip(xs, ys))
    xden = sum((x - xbar) ** 2 for x in xs)
    yden = sum((y - ybar) ** 2 for y in ys)
    if xden <= 0 or yden <= 0:
        return None
    return numerator / math.sqrt(xden * yden)


def force_regime(requests, low: int, high: int):
    width = high - low + 1
    return [replace(request, size=low + (request.size - 1) % width)
            for request in requests]


def individual_inventory_correlation(probes: list[ProbeResult]) -> float | None:
    """Mean recovery of each maker's inventory from the prices an arm reveals."""
    if len(probes) < 4:
        return None
    maker_ids = sorted(probes[0].per_mm_inventory)
    correlations = []
    for maker_id in maker_ids:
        scores, truth = [], []
        for probe in probes:
            quotes = probe.per_mm_quotes
            if quotes is not None and maker_id in quotes:
                ask, bid = quotes[maker_id]
                score = 0.5 * (ask + bid) - probe.ref_mid
            elif probe.best_ask is not None and probe.best_bid is not None:
                # QOMM reveals only the winning buy and sell prices. Applying
                # that aggregate midpoint to a named maker is the strongest
                # price-only estimate available without per-maker quotes.
                score = 0.5 * (probe.best_ask + probe.best_bid) - probe.ref_mid
            else:
                continue
            scores.append(float(score))
            truth.append(float(probe.per_mm_inventory[maker_id]))
        value = pearson(scores, truth)
        if value is not None:
            correlations.append(abs(value))
    return statistics.fmean(correlations) if correlations else None


def mean(values):
    finite = [value for value in values if value is not None and math.isfinite(value)]
    return statistics.fmean(finite) if finite else None


def run(seeds: int, steps: int) -> dict:
    rows = []
    for seed_offset in range(seeds):
        seed = 202_608_240 + seed_offset
        for n_mm in (4, 8, 16):
            cfg = SimConfig(steps=steps, n_mm=n_mm, n_entities=24,
                            arrival_rate=0.15, window_steps=max(200, steps // 8),
                            seed=seed)
            market = ReferenceMarket(cfg, seed)
            makers = build_market_makers(cfg, seed + 1)
            base_requests = build_requests(cfg, market, seed + 2)
            for regime, (low, high, probe_size) in REGIMES.items():
                requests = force_regime(base_requests, low, high)
                probes = build_probes(cfg, seed + 3, per_window=6,
                                      probe_size=probe_size)
                for protocol in ("plain_rfq", "qomm_rfq"):
                    result = run_arm(
                        cfg, market, list(requests), makers, protocol,
                        NoDisclosure(), seed=seed + 5, probes=probes,
                        reactive=False)
                    passive = passive_observer(result, cfg, linkage_rho=0.5,
                                               seed=seed + 7)
                    attributes = pretrade_attributes(result, cfg)
                    summary = result.summary()
                    rows.append({
                        "seed": seed,
                        "n_mm": n_mm,
                        "order_regime": regime,
                        "order_range": [low, high],
                        "protocol": protocol,
                        "requests": summary["requests"],
                        "fills": summary["fills"],
                        "fill_rate": summary["fill_rate"],
                        "no_quote_rate": summary["no_quote_rate"],
                        "user_cost_mean_ticks": summary["user_cost_mean_ticks"],
                        "mm_pnl_per_fill": summary["mm_pnl_per_fill"],
                        "quote_continuation": summary["quote_continuation"],
                        "unsettled_request_auc": passive.auc,
                        "unsettled_request_tpr_at_5pct_fpr": passive.tpr_at_5pct_fpr,
                        "direction_accuracy": attributes.extra["direction_accuracy"],
                        "direction_prior": attributes.extra["direction_prior"],
                        "size_bucket_accuracy": attributes.extra["size_bucket_accuracy"],
                        "size_bucket_prior": attributes.extra["size_bucket_prior"],
                        "individual_inventory_correlation": (
                            individual_inventory_correlation(result.probe_results)),
                        "probe_count": len(result.probe_results),
                    })

    cells = []
    for n_mm in (4, 8, 16):
        for regime in REGIMES:
            cell = [row for row in rows
                    if row["n_mm"] == n_mm and row["order_regime"] == regime]
            plain = [row for row in cell if row["protocol"] == "plain_rfq"]
            qomm = [row for row in cell if row["protocol"] == "qomm_rfq"]
            cells.append({
                "n_mm": n_mm,
                "order_regime": regime,
                "paired_seeds": seeds,
                "plain": {
                    metric: mean([row[metric] for row in plain])
                    for metric in ("fill_rate", "no_quote_rate",
                                   "user_cost_mean_ticks", "mm_pnl_per_fill",
                                   "unsettled_request_auc",
                                   "individual_inventory_correlation")
                },
                "qomm": {
                    metric: mean([row[metric] for row in qomm])
                    for metric in ("fill_rate", "no_quote_rate",
                                   "user_cost_mean_ticks", "mm_pnl_per_fill",
                                   "unsettled_request_auc",
                                   "individual_inventory_correlation")
                },
                "paired_fill_rate_delta": mean([
                    q["fill_rate"] - p["fill_rate"]
                    for p, q in zip(plain, qomm)
                ]),
            })

    correlations_plain = [row["individual_inventory_correlation"] for row in rows
                          if row["protocol"] == "plain_rfq"]
    correlations_qomm = [row["individual_inventory_correlation"] for row in rows
                         if row["protocol"] == "qomm_rfq"]
    plain_corr = mean(correlations_plain)
    qomm_corr = mean(correlations_qomm)
    reduction = (None if plain_corr in (None, 0) or qomm_corr is None
                 else 1 - qomm_corr / plain_corr)
    return {
        "host": this_host(),
        "evidence_class": "smoke_only",
        "synthetic": True,
        "config": {"seeds": seeds, "steps": steps, "maker_counts": [4, 8, 16],
                   "order_regimes": {name: list(bounds[:2])
                                     for name, bounds in REGIMES.items()}},
        "sealed_prediction": {
            "metric": "individual_inventory_correlation",
            "prediction": "QOMM is at least 30 percent lower than plain RFQ",
            "likely_failure": "large orders in four-maker markets lose utility first",
        },
        "prediction_readout": {
            "plain_mean": plain_corr,
            "qomm_mean": qomm_corr,
            "relative_reduction": reduction,
            "prediction_met": reduction is not None and reduction >= 0.30,
        },
        "cells": cells,
        "rows": rows,
        "limitations": [
            "Synthetic smoke run; no production promotion is permitted.",
            "Behavioral responses and disclosure are disabled to isolate request routing.",
            "Three paired seeds are not a powered confirmation cohort.",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--seeds", type=int, default=3)
    parser.add_argument("--steps", type=int, default=6_000)
    args = parser.parse_args()
    if args.seeds < 1 or args.steps < 1_000:
        raise SystemExit("at least one seed and 1,000 steps are required")
    payload = run(args.seeds, args.steps)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n",
                        encoding="utf-8")
    print(json.dumps(payload["prediction_readout"], sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
