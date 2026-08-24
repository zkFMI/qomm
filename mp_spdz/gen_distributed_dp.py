#!/usr/bin/env python3
"""Generate the fixed-shape distributed DP publication circuit."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_audit.distributed_dp import DpMechanism  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--epsilon-micros", type=int, required=True)
    parser.add_argument("--sensitivity", type=int, required=True)
    parser.add_argument("--support", type=int, default=128)
    parser.add_argument("--n-parties", type=int, default=7)
    parser.add_argument("--budget-total-micros", type=int, required=True)
    parser.add_argument("--budget-spent-micros", type=int, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    mechanism = DpMechanism(args.epsilon_micros, args.sensitivity, args.support)
    source = mechanism.mp_spdz_source(
        n_parties=args.n_parties,
        budget_total_micros=args.budget_total_micros,
        budget_spent_micros=args.budget_spent_micros)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(source, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
