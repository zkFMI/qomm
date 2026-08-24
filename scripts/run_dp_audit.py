#!/usr/bin/env python3
"""Audit the entity-level DP claim on windows produced by a real simulation run."""

from __future__ import annotations

import argparse
import json
import sys
from concurrent.futures import ProcessPoolExecutor
from dataclasses import asdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from scripts import hosts  # noqa: E402
from qomm_sim.audit import audit_window                       # noqa: E402
from qomm_sim.experiment import DPParams, make_disclosure      # noqa: E402
from qomm_sim.engine import run_arm                            # noqa: E402
from qomm_sim.market import (                                  # noqa: E402
    ReferenceMarket, SimConfig, build_market_makers, build_requests,
)


def _job(payload: tuple) -> dict:
    obs, entity, eps, request_cap, volume_cap, trials, seed, n_entities, field = payload
    row = asdict(audit_window(obs, entity, eps, request_cap, volume_cap,
                              trials=trials, seed=seed, n_entities=n_entities,
                              field=field))
    row["field"] = field
    return row


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--steps", type=int, default=48_000)
    ap.add_argument("--window-steps", type=int, default=1200)
    ap.add_argument("--n-entities", type=int, default=24)
    ap.add_argument("--arrival-rate", type=float, default=0.15)
    ap.add_argument("--epsilons", type=float, nargs="+", default=[0.25, 1.0, 4.0])
    ap.add_argument("--trials", type=int, default=4000)
    ap.add_argument("--windows", type=int, default=6, help="windows sampled per epsilon")
    ap.add_argument("--entities", type=int, default=4, help="busiest entities audited")
    # All four released fields, not just the request count.
    #
    # Only the request count was ever bound here, and the field that was not
    # bound was the field whose sensitivity was wrong: the fill count was a bare
    # total clipped against the request sum, so removing an entity moved it by
    # more than the cap its noise was calibrated to. `_drop_entity` copied the
    # total across unchanged, so the two worlds the audit compared were
    # identical in exactly that field. An audit that cannot see a field is not
    # evidence about it.
    ap.add_argument("--fields", nargs="+",
                    default=["noisy_requests", "noisy_volume",
                             "noisy_signed_volume", "noisy_fills"])
    ap.add_argument("--seed", type=int, default=20260818)
    ap.add_argument("--workers", type=int, default=0)
    args = ap.parse_args()

    cfg = SimConfig(steps=args.steps, window_steps=args.window_steps,
                    n_entities=args.n_entities, arrival_rate=args.arrival_rate,
                    seed=args.seed)
    market = ReferenceMarket(cfg, cfg.seed)
    makers = build_market_makers(cfg, cfg.seed + 1)
    requests = build_requests(cfg, market, cfg.seed + 2)
    dp = DPParams(epsilon_per_window=1.0, epsilon_total=1e9)
    result = run_arm(cfg, market, requests, makers, "qomm_rfq",
                     make_disclosure("C_dp", cfg, dp), seed=cfg.seed + 5)

    windows = sorted(result.windows, key=lambda w: -w.requests)[:args.windows]
    jobs = []
    for eps in args.epsilons:
        for index, obs in enumerate(windows):
            busiest = sorted(obs.requests_by_entity.items(), key=lambda kv: -kv[1])
            for entity, _count in busiest[:args.entities]:
                for field in args.fields:
                    jobs.append((obs, entity, eps, dp.request_cap, dp.volume_cap,
                                 args.trials, args.seed + 17 * index,
                                 cfg.n_entities, field))

    print(f"auditing {len(jobs)} (window, entity, epsilon) cells", flush=True)
    rows = []
    with ProcessPoolExecutor(max_workers=args.workers or None) as pool:
        for index, row in enumerate(pool.map(_job, jobs, chunksize=1), 1):
            rows.append(row)
            if index % 10 == 0 or index == len(jobs):
                print(f"  {index}/{len(jobs)}", flush=True)

    by_eps: dict[float, dict] = {}
    for row in rows:
        bucket = by_eps.setdefault(row["declared_epsilon"], {
            "declared_epsilon": row["declared_epsilon"],
            "field_epsilon": row["field_epsilon"], "cells": 0,
            "max_empirical_epsilon": 0.0, "violations": 0})
        bucket["cells"] += 1
        bucket["max_empirical_epsilon"] = max(bucket["max_empirical_epsilon"],
                                              row["empirical_epsilon"])
        bucket["violations"] += 0 if row["within_claim"] else 1
        # Per field as well as pooled: a pooled maximum hides which field is
        # closest to its claim, and that is the thing worth knowing.
        per = bucket.setdefault("by_field", {}).setdefault(
            row["field"], {"cells": 0, "max_empirical_epsilon": 0.0, "violations": 0})
        per["cells"] += 1
        per["max_empirical_epsilon"] = max(per["max_empirical_epsilon"],
                                           row["empirical_epsilon"])
        per["violations"] += 0 if row["within_claim"] else 1

    payload = {

        # the machine this was taken on, so a reader does

        # not have to ask

        "host": hosts.this_host(),
        "config": {"steps": args.steps, "window_steps": args.window_steps,
                   "n_entities": args.n_entities, "arrival_rate": args.arrival_rate,
                   "trials": args.trials, "request_cap": dp.request_cap,
                   "volume_cap": dp.volume_cap,
                   "fields": args.fields,
                   "note": "epsilon is split across 4 released fields, and every "
                           "one of them is audited: the empirical lower bound "
                           "for each is compared against epsilon/4, the claim "
                           "that actually binds that field. Only the request "
                           "count used to be audited, and the fill count --- the "
                           "one whose sensitivity was wrong --- was the one "
                           "nothing looked at"},
        "rows": rows,
        "by_epsilon": sorted(by_eps.values(), key=lambda b: b["declared_epsilon"]),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(payload["by_epsilon"], indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
