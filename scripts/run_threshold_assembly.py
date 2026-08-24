#!/usr/bin/env python3
"""What the proofs cost when a quorum assembles them instead of one machine.

Two measurements. The range proof, which was the piece the paper recorded as an
open problem; and the whole quote proof built on it, which is the claim the
paper actually made --- "assembled by a quorum of computing nodes from shares",
where what was assembled that way had been one Pedersen opening.

The paper recorded joint assembly of the range proofs as an open problem and
then priced the missing piece --- shares of the bits --- at fifteen rounds. This
measures the other half: given those shares, what the assembly and the resulting
proof actually cost, against the ordinary local proof it replaces.

Two things are being compared and they are not the same proof. The local path
proves each bit with a Chaum--Pedersen disjunction; the assembled path cannot,
because choosing which branch to simulate is a decision made from the bit, and a
node holds only a share of it. It proves `b*b = b` instead, which is the same
statement over a prime field and is linear in the witness. So the comparison
below is between two constructions, not between two implementations of one, and
the size difference is a property of that substitution rather than of the
threshold setting.
"""
from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.hosts import this_host                                # noqa: E402
from zk.commit import Pedersen, prove_range, verify_range          # noqa: E402
from zk.groups import make_group                                   # noqa: E402
from zk.quote_proof import (FIELDS, MakerWitness, QuoteProver,      # noqa: E402
                            QuoteVerifier)
from zk.threshold_quote import (deal_quote_shares,                 # noqa: E402
                                joint_prove_quote)
from zk.threshold_range import (deal_bits, joint_prove_range,      # noqa: E402
                                verify_threshold_range)


def timed(fn, repeats: int) -> dict:
    samples = []
    for _ in range(repeats):
        t0 = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - t0) * 1000)
    return {"median_ms": statistics.median(samples), "n": len(samples),
            "min_ms": min(samples), "max_ms": max(samples),
            "sd_ms": statistics.stdev(samples) if len(samples) > 1 else 0.0}


def sizes(group, proof, kind: str) -> dict:
    points = len(proof.bit_commitments)
    if kind == "local":
        points += 2 * len(proof.bit_proofs)          # t0, t1
        scalars = 4 * len(proof.bit_proofs)          # c_real, c_fake, z_real, z_fake
    else:
        points += 2 * len(proof.bit_proofs)          # t_factor, t_product
        scalars = 3 * len(proof.bit_proofs)          # z_b, z_rb, z_s
    points += 1                                       # linkage t
    scalars += 2                                      # linkage z_value, z_blinding
    point_bytes = len(group.encode(proof.bit_commitments[0]))
    scalar_bytes = (group.order.bit_length() + 7) // 8
    return {"points": points, "scalars": scalars,
            "bytes": points * point_bytes + scalars * scalar_bytes}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path,
                    default=ROOT / "artifacts" / "threshold_assembly.json")
    ap.add_argument("--widths", type=int, nargs="+", default=[8, 16, 24, 26, 32])
    ap.add_argument("--parties", type=int, default=7)
    ap.add_argument("--threshold", type=int, default=2)
    ap.add_argument("--repeats", type=int, default=5)
    ap.add_argument("--makers", type=int, nargs="+", default=[2, 4, 8])
    ap.add_argument("--group", default="ed25519")
    args = ap.parse_args()

    group = make_group(args.group)
    key = Pedersen(group, b"qomm:quote:v1")
    parties = list(range(1, args.parties + 1))
    quorum = parties[: args.threshold + 1]

    rows = []
    for width in args.widths:
        value = (1 << width) - 1 if width < 40 else 12345
        blinding = group.random_scalar()
        shares = deal_bits(key, value, blinding, width, parties, args.threshold)

        joint, transcript = joint_prove_range(key, shares, quorum, b"ctx")
        local = prove_range(key, shares.commitment, value, blinding, width, b"ctx")
        assert verify_threshold_range(key, shares.commitment, joint, b"ctx")
        assert verify_range(key, shares.commitment, local, b"ctx")

        row = {
            "width": width,
            "quorum": len(quorum),
            "assemble": timed(
                lambda: joint_prove_range(key, shares, quorum, b"ctx"), args.repeats),
            "verify_assembled": timed(
                lambda: verify_threshold_range(key, shares.commitment, joint, b"ctx"),
                args.repeats),
            "prove_local": timed(
                lambda: prove_range(key, shares.commitment, value, blinding, width,
                                    b"ctx"), args.repeats),
            "verify_local": timed(
                lambda: verify_range(key, shares.commitment, local, b"ctx"),
                args.repeats),
            "size_assembled": sizes(group, joint, "threshold"),
            "size_local": sizes(group, local, "local"),
            "no_node_holds_the_value": all(
                shares.value[p] != value for p in parties),
            "verified_by_ordinary_verifier": True,
        }
        # The loop below runs every quorum member in one process, one after the
        # other, so `assemble` is total CPU across the quorum and not wall clock.
        # In a deployment the members run at once, and what a node waits for is
        # its own share of the work plus the rounds. Both are reported, because
        # quoting the first as if it were the second would overstate the cost by
        # the size of the quorum.
        row["assemble_over_local"] = (row["assemble"]["median_ms"]
                                      / row["prove_local"]["median_ms"])
        row["assemble_per_node_ms"] = row["assemble"]["median_ms"] / len(quorum)
        row["per_node_over_local"] = (row["assemble_per_node_ms"]
                                      / row["prove_local"]["median_ms"])
        row["bytes_over_local"] = (row["size_assembled"]["bytes"]
                                   / row["size_local"]["bytes"])
        rows.append(row)
        print(f"width {width:3d}: assemble {row['assemble']['median_ms']:7.1f} ms "
              f"vs local {row['prove_local']['median_ms']:7.1f} ms "
              f"({row['assemble_over_local']:.2f}x total, "
              f"{row['per_node_over_local']:.2f}x per node)   "
              f"{row['size_assembled']['bytes']:5d} B vs {row['size_local']['bytes']:5d} B "
              f"({row['bytes_over_local']:.3f}x)", flush=True)

    # --- and the whole quote proof, which is what the range proof was for ---
    quote_rows = []
    for n_makers in args.makers:
        makers = [MakerWitness(mid=10_000, half=20 + i, slope=1 + i, invcoef=1,
                               inv=3, maxqty=500, expiry=2_000, active=1,
                               blindings={f: key.random_blinding() for f in FIELDS})
                  for i in range(n_makers)]
        settings = dict(qty=100, direction=0, now=1_000, sentinel=1 << 20,
                        n_slots=8)
        shares = deal_quote_shares(key, makers, parties=parties,
                                   threshold=args.threshold, **settings)
        assembled, public = joint_prove_quote(
            key, shares, makers, quorum, now=settings["now"],
            sentinel=settings["sentinel"], n_slots=settings["n_slots"],
            direction=settings["direction"])
        ok, why = QuoteVerifier(group, key, assembled=True).verify(assembled, public)
        assert ok, why
        prover = QuoteProver(group, key)
        local, local_public = prover.prove(makers, **settings)
        ok, why = QuoteVerifier(group, key).verify(local, local_public)
        assert ok, why

        row = {
            "makers": n_makers,
            "assemble": timed(lambda: joint_prove_quote(
                key, shares, makers, quorum, now=settings["now"],
                sentinel=settings["sentinel"], n_slots=settings["n_slots"],
                direction=settings["direction"]), args.repeats),
            "prove_local": timed(lambda: prover.prove(makers, **settings),
                                 args.repeats),
            "verify_assembled": timed(lambda: QuoteVerifier(
                group, key, assembled=True).verify(assembled, public), args.repeats),
            "verify_local": timed(lambda: QuoteVerifier(group, key).verify(
                local, local_public), args.repeats),
        }
        row["assemble_per_node_ms"] = row["assemble"]["median_ms"] / len(quorum)
        row["per_node_over_local"] = (row["assemble_per_node_ms"]
                                      / row["prove_local"]["median_ms"])
        row["verify_over_local"] = (row["verify_assembled"]["median_ms"]
                                    / row["verify_local"]["median_ms"])
        quote_rows.append(row)
        print(f"quote, {n_makers:2d} makers: per node "
              f"{row['assemble_per_node_ms']:7.1f} ms vs local "
              f"{row['prove_local']['median_ms']:7.1f} ms "
              f"({row['per_node_over_local']:.2f}x)   verify "
              f"{row['verify_assembled']['median_ms']:7.1f} vs "
              f"{row['verify_local']['median_ms']:7.1f} ms "
              f"({row['verify_over_local']:.2f}x)", flush=True)

    payload = {"host": this_host(), "group": args.group,
               "quote_rows": quote_rows,
               "assemble_is": "total CPU across the quorum, run serially in one "
                              "process; a deployment runs the members at once, so "
                              "assemble_per_node_ms is what a node waits for",
               "parties": args.parties, "threshold": args.threshold,
               "bit_proof_assembled": "square: b*b = b, linear in the witness",
               "bit_proof_local": "disjunction: the branch is chosen from the bit",
               "rows": rows}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=1) + "\n")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
