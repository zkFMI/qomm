#!/usr/bin/env python3
"""Record the three times Issue #129 asks to be kept apart.

    price answered    the user has an executable quote
    proof complete    the correctness proof for that quote exists
    settlement ready  the proof verified and a quorum of receipts is in

The Issue is explicit that allowing settlement before the proof is finished
would give up the guarantee, so the three have to be measured separately rather
than reported as one latency. If the proof does not finish inside an RFS update
interval, audited RFS is not met at that interval, and that verdict is printed.
"""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey  # noqa: E402

from qomm_audit.receipts import (                                                # noqa: E402
    GENESIS, AuditLedger, SlotSpec, digest, sign_receipt,
)
from zk.groups import make_group
from scripts import hosts  # noqa: E402
from scripts.measure import exact, render, summarise          # noqa: E402                                                  # noqa: E402
from zk.commit import Pedersen
from zk.quote_proof import (
    FIELDS, MakerWitness, QuoteProver, QuoteVerifier,
)               # noqa: E402

SENTINEL = 1 << 20


def one_slot(root: Path, group, n_mm: int, bit_length: int, delay_ms: float,
             nodes: int, quorum: int, slot: int, makers) -> dict:
    stamps = {}
    stamps["request"] = time.perf_counter()

    proc = subprocess.run(
        [sys.executable, str(HERE / "run_qomm.py"), "--mp-spdz-root", str(root),
         "--mode", "rfq", "--n-mm", str(n_mm), "--bit-length", str(bit_length),
         "--delay-ms", str(delay_ms), "--repeats", "1"],
        capture_output=True, text=True)
    stamps["price"] = time.perf_counter()
    try:
        quote = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return {"error": "mpc failed", "stderr": proc.stderr[-800:]}
    if not quote.get("verified"):
        return {"error": "mpc result did not match the cleartext reference"}

    prover, verifier = QuoteProver(group), QuoteVerifier(group)
    proof, public = prover.prove(makers, qty=100, direction=0, now=1_000,
                                 sentinel=SENTINEL, n_slots=n_mm)
    stamps["proof"] = time.perf_counter()

    ok, message = verifier.verify(proof, public)
    keys = {node: Ed25519PrivateKey.generate() for node in range(nodes)}
    ledger = AuditLedger({node: key.public_key() for node, key in keys.items()})
    makers_digest = digest(b"makers", *[f"MM-{i}".encode() for i in range(n_mm)])
    spec = SlotSpec(slot=slot, mm_set_digest=makers_digest,
                    market_digest=digest(b"market", slot.to_bytes(4, "big")),
                    deadline=10_000, required_receipts=quorum)
    ledger.open_slot(spec)
    result_digest = digest(b"result", str(proof.winner_value).encode())
    for node in range(nodes):
        ledger.record(sign_receipt(keys[node], node, spec, prev_state_digest=GENESIS,
                                   new_state_digest=digest(b"state", result_digest),
                                   result_digest=result_digest, emitted_at=1))
    settled, findings = ledger.settle(slot, now=2)
    stamps["settle"] = time.perf_counter()

    return {
        "mpc_rounds": quote["measured_rounds"], "mpc_mb": quote["measured_mb"],
        "proof_verified": ok, "proof_message": message,
        "receipts_settled": settled is not None, "findings": len(findings),
        "price_ms": (stamps["price"] - stamps["request"]) * 1000,
        "proof_ms": (stamps["proof"] - stamps["price"]) * 1000,
        "settle_ms": (stamps["settle"] - stamps["proof"]) * 1000,
        "total_ms": (stamps["settle"] - stamps["request"]) * 1000,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mp-spdz-root", type=Path,
                    default=Path(os.environ.get("MP_SPDZ_ROOT", "")))
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--n-mm", type=int, default=16)
    ap.add_argument("--bit-length", type=int, default=31)
    ap.add_argument("--delays", type=float, nargs="+", default=[1, 15])
    # Five, because five or more is what the plan fixed in advance. It was
    # three, and three is what got measured --- the deviation came from a
    # default that disagreed with the pre-registration, not from a decision.
    ap.add_argument("--slots", type=int, default=5)
    ap.add_argument("--nodes", type=int, default=7)
    ap.add_argument("--quorum", type=int, default=5)
    ap.add_argument("--rfs-interval-ms", type=float, default=1000.0)
    args = ap.parse_args()

    group = make_group("ed25519")
    import random
    rng = random.Random(5)
    # Policies as they go on the record: values and the blindings that hide
    # them. The quote proof is about registered policies, so a witness without
    # its registration blindings is not one.
    key = Pedersen(group, b"qomm:quote:v1")
    makers = [MakerWitness(mid=100_000 + rng.randint(-15, 15), half=rng.randint(5, 40),
                           slope=rng.choice([0, 1, 2]), invcoef=rng.choice([0, 1, 2]),
                           inv=rng.randint(0, 50), maxqty=rng.choice([200, 400]),
                           expiry=1_000 + rng.randint(1, 600), active=1,
                           blindings={f: key.random_blinding() for f in FIELDS})
              for _ in range(args.n_mm)]

    rows = []
    for delay in args.delays:
        samples = [one_slot(args.mp_spdz_root, group, args.n_mm, args.bit_length,
                            delay, args.nodes, args.quorum, slot, makers)
                   for slot in range(args.slots)]
        good = [s for s in samples if "error" not in s]
        if not good:
            print(f"  delay {delay}ms: all slots failed: {samples[0]}")
            continue
        row = {
            "delay_ms": delay,
            "price": summarise(s["price_ms"] for s in good),
            "proof": summarise(s["proof_ms"] for s in good),
            "settle": summarise(s["settle_ms"] for s in good),
            "total": summarise(s["total_ms"] for s in good),
            "proof_verified": all(s["proof_verified"] for s in good),
            "receipts_settled": all(s["receipts_settled"] for s in good),
            "mpc_rounds": good[0]["mpc_rounds"],
        }
        row["audited_rfs_met"] = row["total"]["mean"] <= args.rfs_interval_ms
        rows.append(row)
        # The keys here are the summaries built just above, not the raw
        # per-sample fields they were made from --- an earlier version printed
        # the sample names and raised before it could write anything.
        print(f"  {delay:g}ms one way: priced {render(row['price'], 1)} ms | "
              f"proved +{render(row['proof'], 1)} ms | "
              f"settleable +{render(row['settle'], 1)} ms | "
              f"total {render(row['total'], 1)} ms | "
              f"meets an audited {args.rfs_interval_ms:g}ms RFS slot="
              f"{row['audited_rfs_met']}", flush=True)

    payload = {"host": hosts.this_host(),
               "config": {k: (str(v) if isinstance(v, Path) else v)
                          for k, v in vars(args).items()}, "rows": rows}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
