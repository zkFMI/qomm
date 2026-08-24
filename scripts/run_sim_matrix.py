#!/usr/bin/env python3
"""Multi-seed phase-3 comparison, one process per (seed, arm).

The proposal forbids judging the comparison from a small number of convenient
runs, so every cell is repeated over independent seeds and reported with a
confidence interval.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from concurrent.futures import ProcessPoolExecutor
from dataclasses import asdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from qomm_sim import attackers as atk                       # noqa: E402
from qomm_sim.experiment import DPParams, build_probes, make_disclosure  # noqa: E402
from qomm_sim.engine import run_arm                          # noqa: E402
from qomm_sim.market import (                                # noqa: E402
    ReferenceMarket, SimConfig, build_market_makers, build_requests,
)
from qomm_sim.tapes import (                                 # noqa: E402
    TapeMarket, load_bybit, load_uniswapx, requests_from_tape,
)
from scripts.smallsample import mean_ci                      # noqa: E402


def _tape_from(spec: dict, cfg: SimConfig):
    """Load a tape inside the worker, since a loaded tape is not worth pickling."""
    kind = spec["kind"]
    if kind == "bybit":
        return load_bybit(Path(spec["path"]), cfg, steps=spec.get("steps"),
                          step_ms=spec.get("step_ms"), max_rows=spec.get("max_rows"))
    if kind == "uniswapx":
        return load_uniswapx(Path(spec["path"]), cfg, steps=spec.get("steps"),
                             step_blocks=spec.get("step_blocks", 1))
    raise ValueError(f"unknown tape kind {kind!r}")


def one_cell(job: dict) -> dict:
    cfg = SimConfig(**job["cfg"])
    dp = DPParams(**job["dp"])
    protocol, disclosure_name, layer = job["protocol"], job["disclosure"], job["layer"]
    tape_meta = None
    if job.get("tape"):
        tape = _tape_from(job["tape"], cfg)
        market = TapeMarket(cfg, tape)
        requests, cfg, tape_meta = requests_from_tape(
            cfg, market, tape, synthetic_entities=job["tape"].get("synthetic_entities"),
            seed=cfg.seed + 2)
        makers = build_market_makers(cfg, cfg.seed + 1)
        probes = build_probes(cfg, cfg.seed + 3, job["probes_per_window"])
    else:
        market = ReferenceMarket(cfg, cfg.seed)
        makers = build_market_makers(cfg, cfg.seed + 1)
        probes = build_probes(cfg, cfg.seed + 3, job["probes_per_window"])
        # Both layers start from the same stream. In the reactive layer the
        # agents may change behaviour, so the realised flow diverges.
        requests = build_requests(cfg, market, cfg.seed + 2)
    disclosure = make_disclosure(disclosure_name, cfg, dp)
    result = run_arm(cfg, market, requests, makers, protocol, disclosure,
                     seed=cfg.seed + 5, probes=probes,
                     reactive=(layer == "reactive"))
    row = result.summary()
    if tape_meta is not None:
        row["tape"] = tape_meta
    row.update({"layer": layer, "seed": cfg.seed,
                "epsilon_per_window": dp.epsilon_per_window,
                "epsilon_total": dp.epsilon_total})
    row["attacks"] = [
        atk.passive_observer(result, cfg).as_dict(),
        atk.pretrade_attributes(result, cfg).as_dict(),
        atk.window_shift_observer(result, cfg).as_dict(),
        atk.probing_entity(result, cfg, probe_budget=len(probes)).as_dict(),
        atk.colluding_wallets(result, cfg, wallet_limit=4, entity_limit=4).as_dict(),
        atk.external_info_observer(result, cfg, market).as_dict(),
    ]
    return row


def aggregate(rows: list[dict]) -> list[dict]:
    """Mean and 95% interval over seeds, per arm and per tape.

    The tape is part of the key because a sweep across symbols is a sweep across
    market thickness, and averaging a market that trades nine times a second
    with one that trades once every twelve seconds would answer neither of the
    questions either was included to answer.
    """
    groups: dict[tuple, list[dict]] = {}
    for row in rows:
        source = (row.get("tape") or {}).get("source", "generated")
        key = (source, row["layer"], row["protocol"], row["disclosure"],
               row["epsilon_per_window"])
        groups.setdefault(key, []).append(row)

    numeric = [
        "fill_rate", "no_quote_rate", "user_cost_mean_ticks", "mm_pnl_per_fill",
        "mm_markout_50ms_mean", "mm_markout_1s_mean", "mm_markout_10s_mean",
        "quote_continuation", "suppression_rate", "epsilon_spent_max",
        "release_requests_mae", "release_signed_volume_mae",
    ]
    attack_numeric = {
        "A1_passive_observer": ("auc", "advantage_over_prior", "tpr_at_5pct_fpr"),
        "A1b_pretrade_attributes": ("direction_accuracy", "direction_prior",
                                    "size_bucket_accuracy", "size_bucket_prior"),
        "A2_window_shift": ("auc", "advantage_over_prior"),
        "A3_probing_entity": ("net_inventory_corr_from_best_quote",
                              "own_inventory_corr_from_per_mm_quotes"),
        "A4_colluding_wallets": ("corr_under_wallet_limit", "corr_under_entity_limit",
                                 "probes_needed_net_corr_0.8",
                                 "probes_needed_per_mm_corr_0.8"),
        "A5_external_info": ("auc", "advantage_over_prior"),
    }

    out = []
    for key, items in sorted(groups.items(), key=lambda kv: str(kv[0])):
        source, layer, protocol, disclosure, eps = key
        summary = {"source": source,
                   "layer": layer, "protocol": protocol, "disclosure": disclosure,
                   "epsilon_per_window": eps, "n_seeds": len(items)}
        for field in numeric:
            values = [i[field] for i in items if i.get(field) is not None]
            summary[field] = _mean_ci(values)
        for name, fields in attack_numeric.items():
            for report in items[0]["attacks"]:
                if report["attacker"] != name:
                    continue
                for field in fields:
                    values = [
                        r[field] for i in items for r in i["attacks"]
                        if r["attacker"] == name and r.get(field) is not None
                    ]
                    summary[f"{name}.{field}"] = _mean_ci(values)
        out.append(summary)
    return out


def _mean_ci(values: list[float]) -> dict | None:
    """Per-symbol cells run five seeds. 1.96 is not the multiplier for five.

    At n = 5 the multiplier is 2.776 --- 42% wider than the normal one this
    used, which is the difference between a starred cell and an unstarred one
    for two of the three the run reports.
    """
    if not values:
        return None
    ci = mean_ci(values)
    out = {"mean": ci["mean"], "ci95": ci["half_width"] or 0.0, "n": ci["n"]}
    if ci.get("multiplier") is not None:
        out["multiplier"] = ci["multiplier"]
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--seeds", type=int, default=20)
    ap.add_argument("--seed0", type=int, default=20260818)
    ap.add_argument("--steps", type=int, default=48_000)
    ap.add_argument("--n-mm", type=int, default=16)
    ap.add_argument("--n-entities", type=int, default=24)
    ap.add_argument("--arrival-rate", type=float, default=0.15)
    ap.add_argument("--window-steps", type=int, default=1200)
    ap.add_argument("--epsilons", type=float, nargs="+", default=[1.0])
    ap.add_argument("--epsilon-total", type=float, default=40.0)
    ap.add_argument("--protocols", nargs="+",
                    default=["plain_rfq", "plain_rfm", "plain_rfs",
                             "qomm_rfq", "qomm_rfm", "qomm_rfs"])
    ap.add_argument("--disclosures", nargs="+", default=["A_none", "B_threshold", "C_dp"])
    ap.add_argument("--layers", nargs="+", default=["replay", "reactive"])
    ap.add_argument("--probes-per-window", type=int, default=6)
    ap.add_argument("--workers", type=int, default=0)
    ap.add_argument("--tape", choices=("bybit", "uniswapx"), default=None,
                    help="replace the generated market with a real tape")
    ap.add_argument("--tape-paths", type=Path, nargs="+", default=None,
                    help="one or more tapes; the matrix is swept over each. "
                         "Several symbols is how the thin-market rejection "
                         "criterion gets answered, since real liquidity spans "
                         "two orders of magnitude across the venue's listings")
    ap.add_argument("--tape-step-ms", type=int, default=None,
                    help="bybit only; at least 1000 on files from late 2021 on, "
                         "whose timestamps are whole seconds")
    ap.add_argument("--tape-step-blocks", type=int, default=1,
                    help="uniswapx only; one step is this many blocks of 12 s")
    ap.add_argument("--tape-max-rows", type=int, default=None)
    ap.add_argument("--tape-entities", type=int, default=None,
                    help="assign this many entities round-robin; omit to use one "
                         "entity per observed address, which only a chain tape has")
    args = ap.parse_args()

    jobs = []
    for seed_index in range(args.seeds):
        seed = args.seed0 + 1_000 * seed_index
        cfg = dict(steps=args.steps, n_mm=args.n_mm, n_entities=args.n_entities,
                   arrival_rate=args.arrival_rate, window_steps=args.window_steps, seed=seed)
        for eps in args.epsilons:
            dp = dict(epsilon_per_window=eps, epsilon_total=args.epsilon_total,
                      request_cap=3, volume_cap=300)
            for layer in args.layers:
                for protocol in args.protocols:
                    for disclosure in args.disclosures:
                        # arms without DP do not depend on epsilon: run them once
                        if disclosure != "C_dp" and eps != args.epsilons[0]:
                            continue
                        specs = [None]
                        if args.tape:
                            specs = [{"kind": args.tape, "path": str(path),
                                      "steps": args.steps, "step_ms": args.tape_step_ms,
                                      "step_blocks": args.tape_step_blocks,
                                      "max_rows": args.tape_max_rows,
                                      "synthetic_entities": args.tape_entities}
                                     for path in (args.tape_paths or [])]
                            if not specs:
                                ap.error("--tape needs --tape-paths")
                        for tape_spec in specs:
                            jobs.append({"tape": tape_spec,
                                         "cfg": cfg, "dp": dp, "protocol": protocol,
                                         "disclosure": disclosure, "layer": layer,
                                         "probes_per_window": args.probes_per_window})

    workers = args.workers or None
    print(f"running {len(jobs)} cells", flush=True)
    rows = []
    with ProcessPoolExecutor(max_workers=workers) as pool:
        for index, row in enumerate(pool.map(one_cell, jobs, chunksize=1), 1):
            rows.append(row)
            if index % 25 == 0 or index == len(jobs):
                print(f"  {index}/{len(jobs)}", flush=True)

    payload = {
        "config": {"steps": args.steps, "n_mm": args.n_mm, "n_entities": args.n_entities,
                   "arrival_rate": args.arrival_rate, "window_steps": args.window_steps,
                   "seeds": args.seeds, "epsilons": args.epsilons,
                   "epsilon_total": args.epsilon_total,
                   "probes_per_window": args.probes_per_window},
        "rows": rows,
        "aggregate": aggregate(rows),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
