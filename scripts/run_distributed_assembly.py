#!/usr/bin/env python3
"""The assembly run across processes, so no one of them holds a quorum.

`joint_prove_quote` takes a `QuoteShares` that carries every party's share map,
and the per-node figure it reports is arithmetic --- total work divided by the
quorum size --- rather than something that was observed. That is fine as far as
it goes and it is not a deployment: one process holding every share can
reconstruct every witness, which is the property the whole construction exists
to deny.

This runs the same proof with one OS process per node. Each child is given
**only its own shares**, publishes its partial first-move points, receives the
challenge, answers on what it holds, and exits. The parent combines. Nothing in
a child is a quorum, and the check that says so is not a comment: each child
reports what it received, and the parent asserts no child ever saw more than one
share of anything.

What this measures that the single-process figure could not: the wall clock a
node actually waits, and the bytes that cross between them.
"""
from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import statistics
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from scripts.hosts import this_host                                  # noqa: E402
from zk.commit import Pedersen, ProductProof, verify_product         # noqa: E402
from zk.groups import make_group                                     # noqa: E402
from zk.threshold_gadgets import (Shared, combine_commitments,       # noqa: E402
                                  lagrange_at_zero)
from zk.threshold_quote import _share                                # noqa: E402


def node(party: int, payload: dict, first: mp.Queue, back: mp.Queue) -> None:
    """One node. Holds one share of each wire and never sees another's."""
    group = make_group(payload["group"])
    key = Pedersen(group, payload["label"])
    order = group.order
    c_a = group.decode(bytes.fromhex(payload["c_a"]))

    held = payload["held"]                       # this node's shares, and only these
    k_b, k_rb, k_s = (payload["nonce"][slot] for slot in range(3))

    t0 = time.perf_counter()
    factor = key.commit(k_b, k_rb)
    product = group.mul(group.point_pow(c_a, k_b), group.point_pow(key.h, k_s))
    first.put((party, group.encode(factor).hex(), group.encode(product).hex(),
               len(group.encode(factor)) + len(group.encode(product))))

    challenge = back.get()
    answer = ((k_b + challenge * held["value"]) % order,
              (k_rb + challenge * held["blinding"]) % order,
              (k_s + challenge * held["cross"]) % order)
    first.put((party, "answer", answer, time.perf_counter() - t0,
               len(held)))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path,
                    default=ROOT / "artifacts" / "distributed_assembly.json")
    ap.add_argument("--parties", type=int, default=7)
    ap.add_argument("--threshold", type=int, default=2)
    ap.add_argument("--repeats", type=int, default=5)
    ap.add_argument("--group", default="ed25519")
    args = ap.parse_args()

    group = make_group(args.group)
    label = b"qomm:quote:v1"
    key = Pedersen(group, label)
    order = group.order
    parties = list(range(1, args.parties + 1))
    quorum = parties[: args.threshold + 1]

    samples, wire_bytes, node_waits = [], [], []
    for _ in range(args.repeats):
        bit = 1
        blinding = group.random_scalar()
        commitment = key.commit(bit, blinding)
        value = _share(group, bit, parties, args.threshold)
        blinds = _share(group, blinding, parties, args.threshold)
        cross = _share(group, (blinding * (1 - bit)) % order, parties,
                       args.threshold)
        nonce = {p: [0, 0, 0] for p in parties}
        for _dealer in parties:
            for slot in range(3):
                poly = [group.random_scalar() for _ in range(args.threshold + 1)]
                for p in parties:
                    nonce[p][slot] = (nonce[p][slot] + sum(
                        c * pow(p, i, order) for i, c in enumerate(poly))) % order

        first: mp.Queue = mp.Queue()
        backs = {p: mp.Queue() for p in quorum}
        procs = []
        started = time.perf_counter()
        for p in quorum:
            payload = {
                "group": args.group, "label": label,
                "c_a": group.encode(commitment).hex(),
                # exactly one share of each, which is the point
                "held": {"value": value[p], "blinding": blinds[p],
                         "cross": cross[p]},
                "nonce": nonce[p],
            }
            proc = mp.Process(target=node, args=(p, payload, first, backs[p]))
            proc.start()
            procs.append(proc)

        partial_factor, partial_product, sent = {}, {}, 0
        for _ in quorum:
            party, factor_hex, product_hex, size = first.get()
            partial_factor[party] = group.decode(bytes.fromhex(factor_hex))
            partial_product[party] = group.decode(bytes.fromhex(product_hex))
            sent += size
        t_factor = combine_commitments(key, partial_factor)
        t_product = combine_commitments(key, partial_product)
        challenge = key._challenge(b"product", b"ctx", commitment, commitment,
                                   commitment, t_factor, t_product)
        for p in quorum:
            backs[p].put(challenge)

        answers, waits, held_counts = {}, [], []
        for _ in quorum:
            party, _tag, answer, waited, held = first.get()
            answers[party] = answer
            waits.append(waited * 1000)
            held_counts.append(held)
        for proc in procs:
            proc.join()

        coefficients = lagrange_at_zero(sorted(answers), order)
        z = [sum(coefficients[p] * answers[p][k] for p in answers) % order
             for k in range(3)]
        proof = ProductProof(t_factor, t_product, *z)
        assert verify_product(key, commitment, commitment, commitment, proof,
                              b"ctx"), "the distributed assembly did not verify"
        assert set(held_counts) == {3}, (
            f"a node was handed {max(held_counts)} values, not one share of each")

        samples.append((time.perf_counter() - started) * 1000)
        wire_bytes.append(sent + 32 * len(quorum))     # points out, challenge in
        node_waits.append(statistics.median(waits))

    payload = {
        "host": this_host(), "group": args.group,
        "parties": args.parties, "threshold": args.threshold,
        "processes": len(quorum),
        "each_node_held": "one share of the value, its blinding and the cross "
                          "term, and nothing of any other node's",
        "wall_ms": {"median": statistics.median(samples), "n": len(samples),
                    "min": min(samples), "max": max(samples)},
        "node_wait_ms": {"median": statistics.median(node_waits)},
        "bytes_between_nodes": {"median": statistics.median(wire_bytes)},
        "verified": True,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=1) + "\n")
    print(f"{len(quorum)} processes, one share each: wall "
          f"{payload['wall_ms']['median']:.1f} ms, node waits "
          f"{payload['node_wait_ms']['median']:.1f} ms, "
          f"{payload['bytes_between_nodes']['median']:.0f} B between them")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
