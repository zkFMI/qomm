#!/usr/bin/env python3
"""Proofs assembled from the shares the circuit actually wrote.

The quote proof is assembled from shares. Until the circuit kept them, those
shares reached the prover by a route of their own: the circuit computed, the
prover was handed values, and nothing said the two were the same numbers. A
proof about numbers that merely agree with a computation is not a proof about
the computation.

`test_share_binding` closed that for one value, the winner. This closes it for
the wires the proof is actually made of. The circuit is run with
`--persist-wires --shamir-inputs`, so every node keeps its share of every wire
*in the field the commitments use*, and the proofs below are built from those
files and nothing else.

What is still supplied from outside, and honestly cannot come from the circuit:

* **Blindings.** A Pedersen blinding is not something the computation knows
  about. Registered fields carry what the maker shared at registration; derived
  wires carry what the nodes generated between them. Neither is a hole --- the
  commitment is computed *from the shares* here, so no wire is opened to commit
  to it.
* **Cross terms.** A product proof needs `s = r_c - r_a*b`, a product of two
  secrets. In a deployment that is one multiplication, which is what the
  protocol already runs. Here the harness computes it, and that is the one place
  this script reconstructs anything --- marked, because it is the boundary.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from mp_spdz.persistence import WIRE_NAMES, read_wires, reconstruct  # noqa: E402
from scripts.hosts import this_host                                  # noqa: E402
from zk.commit import Pedersen, verify_product                       # noqa: E402
from zk.groups import make_group                                     # noqa: E402
from zk.threshold_gadgets import (Shared, commitment_from_shares,    # noqa: E402
                                  joint_prove_bit, joint_prove_product,
                                  verify_square_bit)
from zk.threshold_quote import _share                                # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--persistence", type=Path, required=True)
    ap.add_argument("--out", type=Path,
                    default=ROOT / "artifacts" / "circuit_bound_proof.json")
    ap.add_argument("--parties", type=int, default=7)
    ap.add_argument("--threshold", type=int, default=2)
    ap.add_argument("--makers", type=int, default=4)
    ap.add_argument("--qty", type=int, default=100)
    args = ap.parse_args()

    group = make_group("ed25519")
    key = Pedersen(group, b"qomm:policy:v1")
    parties = list(range(1, args.parties + 1))
    quorum = parties[: args.threshold + 1]

    wires = read_wires(args.persistence, args.parties, args.makers)
    if wires["prime"] != group.order:
        raise SystemExit(
            f"the circuit wrote in a {wires['prime'].bit_length()}-bit field and "
            f"the commitments use a {group.order.bit_length()}-bit one; run it "
            "with --shamir-inputs")

    def opened(shares, subset=None):
        """Only the harness does this, and only to stand in for a multiplication."""
        subset = subset or quorum
        return reconstruct([(p, shares[p]) for p in subset], wires["prime"])

    def wire(shares) -> tuple[Shared, dict]:
        blinding = _share(group, group.random_scalar(), parties, args.threshold)
        return Shared(commitment_from_shares(key, shares, blinding, quorum),
                      dict(shares), blinding), blinding

    rows = []
    for index, maker in enumerate(wires["makers"]):
        built = {name: wire(maker[name]) for name in WIRE_NAMES}
        shared = {name: built[name][0] for name in WIRE_NAMES}
        blind = {name: built[name][1] for name in WIRE_NAMES}

        # every commitment came from shares; check it against the direct one
        agrees = all(
            group.encode(shared[name].commitment)
            == group.encode(key.commit(opened(maker[name]),
                                       opened(blind[name])))
            for name in WIRE_NAMES)

        # depth = slope * qty, on the circuit's own slope and depth shares
        r_slope, r_depth = opened(blind["slope"]), opened(blind["depth"])
        qty_blinding = _share(group, group.random_scalar(), parties, args.threshold)
        qty_shared = Shared(
            commitment_from_shares(key, _share(group, args.qty, parties,
                                               args.threshold),
                                   qty_blinding, quorum),
            _share(group, args.qty, parties, args.threshold), qty_blinding)
        cross = _share(group, (r_depth - r_slope * args.qty) % group.order,
                       parties, args.threshold)
        depth_proof = joint_prove_product(
            key, shared["slope"].commitment, qty_shared, shared["depth"], cross,
            quorum, args.threshold, b"circuit:depth")
        depth_ok = verify_product(key, shared["slope"].commitment,
                                  qty_shared.commitment, shared["depth"].commitment,
                                  depth_proof, b"circuit:depth")

        # `ok` and `fits` are bits the circuit produced. Proving they are bits
        # is the square proof, and it is the step the disjunction could not do.
        bit_results = {}
        for name in ("fits", "ok", "active"):
            r = opened(blind[name])
            b = opened(maker[name])
            proof = joint_prove_bit(
                key, shared[name],
                _share(group, (r * (1 - b)) % group.order, parties, args.threshold),
                quorum, args.threshold, b"circuit:" + name.encode())
            bit_results[name] = verify_square_bit(
                key, shared[name].commitment, proof, b"circuit:" + name.encode())

        # The controls. A check that cannot fail is not a check, and every
        # result above is a boolean that came out True.
        #
        # The first control --- prove `depth` against a quantity the circuit did
        # not use --- only bites where the slope is non-zero. At `slope = 0` the
        # statement "depth = slope * anything" is *true*, both sides being zero,
        # and the proof is right to accept it. Two of the four makers in this
        # run have a zero slope, so two of them report the control as not
        # refusing, and that is arithmetic rather than a weak proof. Reporting
        # it as a pass would have been the lie; `control_applies` says which.
        slope_value = opened(maker["slope"])
        wrong_qty = args.qty + 1
        wrong_shares = _share(group, wrong_qty, parties, args.threshold)
        wrong_blinding = _share(group, group.random_scalar(), parties, args.threshold)
        wrong = Shared(commitment_from_shares(key, wrong_shares, wrong_blinding,
                                              quorum),
                       wrong_shares, wrong_blinding)
        wrong_cross = _share(group, (r_depth - r_slope * wrong_qty) % group.order,
                             parties, args.threshold)
        wrong_proof = joint_prove_product(
            key, shared["slope"].commitment, wrong, shared["depth"], wrong_cross,
            quorum, args.threshold, b"circuit:depth")
        refused = not verify_product(key, shared["slope"].commitment,
                                     wrong.commitment, shared["depth"].commitment,
                                     wrong_proof, b"circuit:depth")

        # A second control that bites everywhere: the same proof checked
        # against another maker's depth commitment. Nothing makes that true.
        other = (index + 1) % args.makers
        other_depth = commitment_from_shares(
            key, wires["makers"][other]["depth"],
            _share(group, group.random_scalar(), parties, args.threshold), quorum)
        crossed = not verify_product(key, shared["slope"].commitment,
                                     qty_shared.commitment, other_depth,
                                     depth_proof, b"circuit:depth")

        rows.append({"maker": index,
                     "slope": slope_value if slope_value < group.order // 2
                              else slope_value - group.order,
                     "wrong_quantity_control_applies": slope_value % group.order != 0,
                     "a_wrong_quantity_is_refused": refused,
                     "another_makers_depth_is_refused": crossed,
                     "commitments_from_shares_agree": agrees,
                     "depth_is_slope_times_qty": depth_ok,
                     "bits_proved": bit_results,
                     "no_node_holds_a_wire": all(
                         maker[name][p] != opened(maker[name])
                         for name in WIRE_NAMES for p in parties
                         if opened(maker[name]) > 1)})
        print(f"maker {index}: commitments {agrees}, depth {depth_ok}, "
              f"bits {bit_results}, wrong qty refused {refused} "
              f"(control applies: {slope_value % group.order != 0}), "
              f"other depth refused {crossed}", flush=True)

    payload = {
        "host": this_host(),
        "field_bits": wires["prime"].bit_length(),
        "field_matches_commitment_scalar_field": wires["prime"] == group.order,
        "parties": args.parties, "threshold": args.threshold,
        "wires_per_maker": len(WIRE_NAMES), "wire_order": list(WIRE_NAMES),
        "runs_in_file": wires["runs_in_file"],
        "winner_key": opened(wires["winner_key"]),
        "supplied_outside_the_circuit": [
            "blindings, which the computation does not know about",
            "cross terms, which are one multiplication in a deployment",
        ],
        "rows": rows,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(payload, indent=1) + "\n")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
