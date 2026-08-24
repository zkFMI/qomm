#!/usr/bin/env python3
"""Generate the QOMM query-oblivious quote circuit and its MP-SPDZ inputs.

The circuit implements the pricing model defined in
``papers/kyc_private_clob_defmi/private_rfq_dex_project_proposal.md``:

    q_{i,t} = P_i(x, s_{i,t}, m_t)

Market makers pre-register a secret price policy; the user secret-shares the
request. No market maker ever receives the request itself, so a losing market
maker does not learn that a request existed.

Layer structure (this is the point of the design):

    layer 1  policy arithmetic for all M market makers   -- SIMD, depth 1
    layer 2  eligibility gates for all M                 -- SIMD, depth 1
    layer 3  best-quote selection                        -- binary tree, depth log2(M)

Everything is integer arithmetic on price ticks and lots; no secret division.
"""

from __future__ import annotations

import argparse
import json
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from qomm_transport.roles import (SLACK_BITS, check_field_width,  # noqa: E402
                                  lagrange_at_zero, shamir_split, split)
import sys
from pathlib import Path

# `use_ref` is the only switch a maker cannot already throw by setting a
# coefficient to zero: `slope`, `invcoef` and `active` all admit 0, but the
# reference price used to be added with a hard-wired coefficient of one. A
# maker in a market with no usable reference sets it to 0 and carries the
# whole level in `mid` instead.
FIELDS = ("asset", "mid", "half", "slope", "invcoef", "inv", "maxqty",
          "expiry", "active", "use_ref")

# The scalar field of ed25519, which is what a Pedersen share is an element of.
# Written here rather than imported so the generator stays runnable without the
# proof stack; `tests/test_binding_chain.py` holds the two to each other.
ED25519_ORDER = 2 ** 252 + 27742317777372353535851937790883648493

def mask_bits_for(n_values: int, value_bits: int, challenge_bits: int = 40,
                  statistical_bits: int = 40) -> int:
    """How wide the input check's mask has to be to hide what it is added to.

    `zk/input_check.py` is the other half of this; the two have to agree on the
    number or the opening does not check anything. Kept here as well because the
    circuit is what has to deal the mask, and dealing it is what sets the field.
    """
    combination = value_bits + challenge_bits + max(0, (n_values - 1).bit_length())
    return combination + statistical_bits


def sentinel_for(bit_length: int, padded_mm: int, max_cost: int) -> int:
    """Sentinel that pushes ineligible market makers out of the tournament.

    The packed key is ``cost * padded_mm + index``, so the sentinel has to be
    large enough to lose against every real quote and small enough that the
    packed key still fits the signed range of the configured bit length. Getting
    this wrong produces a silently wrong winner, so the check raises instead.
    """
    headroom = 1 << (bit_length - 2)
    sentinel = headroom // padded_mm
    if sentinel <= max_cost:
        raise ValueError(
            f"bit_length={bit_length} is too narrow: packing {padded_mm} makers with costs "
            f"up to {max_cost} needs at least "
            f"{(max_cost * padded_mm * 4).bit_length() + 1} bits")
    return sentinel


def _pow2_ceil(n: int) -> int:
    p = 1
    while p < n:
        p <<= 1
    return p


def build_program(
    n_mm: int,
    n_parties: int,
    mode: str,
    rfs_steps: int,
    disclose: str,
    now_t: int,
    ref_mid: int,
    band_bps: int,
    threshold_k: int,
    threshold_v: int,
    public_check: bool,
    n_requests: int = 1,
    n_assets: int = 1,
    ref_table: list | None = None,
    maker_assets: list | None = None,
    public_maker_assets: bool = False,
    audit_gates: bool = False,
    bit_length: int = 63,
    argmin_arity: int = 2,
    lagrange: list | None = None,
    price_conditionals: int = 0,
    edabit: bool = False,
    trunc_pr: bool = False,
    input_check: bool = False,
    check_mode: str = "aggregate",
    binding_limit: bool = False,
    challenge_bits: int = 64,
    check_coefficients: list | None = None,
    check_repeats: int = 7,
    stop_after: str = "tournament",
    persist_wires: bool = False,
    reference: str = "anchored",
    range_query: bool = False,
    query_lo: int = 0,
    query_hi: int = 0,
) -> str:
    """Emit the .mpc source. ``n_mm`` must be a power of two (padded by caller).

    ``stop_after`` cuts the circuit short at a named layer, so the rounds a layer
    is responsible for can be read off as the increment between two compiles
    rather than argued for. Each cut still opens something that depends on
    everything above it: a layer whose result nothing reads is a layer the
    compiler is free to delete, and a deleted layer costs nothing, which would
    make the attribution come out backwards.
    """
    if stop_after != "tournament" and mode != "rfq":
        raise ValueError(f"--stop-after names layers of the RFQ circuit; "
                         f"mode {mode} is built differently")
    ref_table = ref_table or [ref_mid]
    # a fixture stands in for the Fiat-Shamir derivation, which needs the
    # commitments; the cost does not depend on which coefficients they are.
    check_coefficients = check_coefficients or [
        1 + (617 * k) % ((1 << 6) - 1) for k in range(64)]
    maker_assets = maker_assets or [i % n_assets for i in range(n_mm)]
    lines: list[str] = []
    w = lines.append

    w('"""QOMM: query-oblivious quote evaluation (generated; do not edit).')
    w("")
    w(f"mode={mode} M={n_mm} parties={n_parties} disclose={disclose} "
      f"bits={bit_length} argmin_arity={argmin_arity} edabit={edabit} "
      f"trunc_pr={trunc_pr} "
      f"price_conditionals={price_conditionals}"
      + (f" rfs_steps={rfs_steps}" if mode == "rfs" else ""))
    w('"""')
    w("")
    w(f"program.set_bit_length({bit_length})")
    if edabit:
        w("# push comparison bit generation into preprocessing")
        w("program.use_edabit(True)")
    if trunc_pr:
        w("# probabilistic truncation: the mask is value-width plus a statistical")
        w("# gap rather than a whole field element, which is the difference that")
        w("# a wide field makes to a comparison.")
        w("program.use_trunc_pr = True")
    w("")
    w(f"M = {n_mm}")
    w(f"N_PARTIES = {n_parties}")
    w(f"LARGE = {sentinel_for(bit_length, n_mm, 8 * max(ref_table))}")
    w(f"NOW_T = {now_t}")
    if range_query:
        w("# The asker's range. Public, because it is their own question.")
        w(f"QUERY_LO = {query_lo}")
        w(f"QUERY_HI = {query_hi}")
    w(f"N_ASSETS = {n_assets}")
    w("# Public reference price per asset. The table is public; which entry the")
    w("# request selects is not, so the selection has to be oblivious.")
    w(f"REF_TABLE = {ref_table}")
    w(f"MAKER_ASSET = {maker_assets}")
    w(f"REF_MID = {ref_mid}   # only used for the sentinel scale")
    w("")
    # The mode says which check, and `input_check` says whether there is one.
    # Branching on the mode alone put the check machinery into every program
    # the moment per-party became the default, which the input-shape test
    # caught by finding a reconstruction that no longer started at node 0.
    if input_check and check_mode == "per-party":
        # every value the circuit reads through `secret_input`: the request's
        # four fields per request, `is_real`, the trader's output mask, and each
        # maker's policy. If this and the program disagree the Array write
        # fails, which is the failure we want rather than a silent short read.
        n_checked_values = 4 * n_requests + 2 + n_mm * len(FIELDS)
        w(f"CHALLENGE_BITS = {challenge_bits}")
        w("# ---- the input check's challenge, and why it is where it is ------")
        w("# The coefficients have to be unpredictable at the moment a node")
        w("# fixes its input, and the input is fixed when this program reads")
        w("# it. An earlier version derived them from the dealer's")
        w("# commitments, which are published before that --- so a node that")
        w("# had seen them could feed x_1 + c_2*k and x_2 - c_1*k and the")
        w("# combination cancelled identically, every time, with no security")
        w("# parameter to raise.")
        w("#")
        w("# So the shares are kept as they are read, one random value is")
        w("# opened once every input is in, and the coefficients are its")
        w("# powers. Public times secret is local, so the combination still")
        w("# costs no communication; the price is that one opening.")
        w("#")
        w("# Taking the check modulo the MPC prime is what makes the bound")
        w("# clean: sum_k rho^k e_k + e_m = 0 is a degree-m polynomial in rho,")
        w("# so a fixed non-zero error survives with probability at most m/p.")
        w("# It also removes the width budget entirely --- nothing has to avoid")
        w("# reducing, because the statement is modulo p on both sides.")
        w("N_CHECKED = %d" % n_checked_values)
        w("check_store = [Array(N_CHECKED, sint) for _ in range(N_PARTIES)]")
        w("check_pos = [0]")
        w("")
        pass    # the one definition is emitted below, once both options are known
    if lagrange is not None:
        # Shamir inputs. A party holds f(p+1) of a degree-t polynomial whose
        # constant term is the value, so reconstruction is a public linear
        # combination --- free in rounds, and the same number of reads as the
        # additive form it replaces. Interpolating from all n points rather
        # than t+1 is what keeps the input count and the circuit shape the same.
        #
        # It buys the only thing it is for: the share a node feeds is the share
        # the dealer committed to in `zk/policy_audit.py`, so the policy that
        # was audited is the policy that was computed on. It costs a field ---
        # a share is a scalar, so the MPC prime has to be the group order.
        #
        # It does not cost a threshold, which it looks like it should. Additive
        # sharing across all n needs every node to reconstruct an input, where
        # Shamir needs t+1 --- but `secret_input` hands the value to MP-SPDZ,
        # which holds it as a degree-t sharing from that point on. Any t+1 nodes
        # could always reconstruct it. The additive layer was buying a stronger
        # guarantee than the protocol underneath it, which is not a guarantee.
        w("LAGRANGE = [" + ", ".join(str(c) for c in lagrange) + "]")

    # One definition, not one per option.
    #
    # There used to be two: the input check emitted a `secret_input` that
    # recorded each party's share into `check_store`, and Shamir inputs emitted
    # another one straight after that applied the Lagrange coefficients. The
    # second silently replaced the first, so asking for both --- which is what
    # `BINDING.md` recommends and what the composed arm measured --- produced a
    # circuit whose check ran over an array nothing had written to. The rounds
    # and the bytes were real; the property was not.
    #
    # Emitting one definition that carries both options is the structural fix.
    # Two definitions of the same name cannot silently displace each other if
    # there is only ever one.
    checking = input_check and check_mode == "per-party"
    w("def secret_input():")
    if checking:
        w("    _k = check_pos[0]")
    w("    total = None")
    w("    for _p in range(N_PARTIES):")
    w("        _s = sint.get_input_from(_p)")
    if checking:
        # what the node fed, before the public coefficient is applied: the
        # check is about the share, and the coefficient is not the node's
        w("        check_store[_p][_k] = _s")
    if lagrange is not None:
        w("        _s = LAGRANGE[_p] * _s")
    w("        total = _s if total is None else total + _s")
    if checking:
        w("    check_pos[0] += 1")
    w("    return total")
    w("")
    w("# ---- user request, shared by the trader across every node ----")
    w(f"N_REQ = {n_requests}")
    w("req_asset = Array(N_REQ, sint)")
    w("req_qty = Array(N_REQ, sint)")
    w("req_dir = Array(N_REQ, sint)   # 0 = user buys (takes ask), 1 = user sells")
    w("req_entity = Array(N_REQ, sint)")
    w("for r in range(N_REQ):")
    w("    req_asset[r] = secret_input()")
    w("    req_qty[r] = secret_input()")
    w("    req_dir[r] = secret_input()")
    w("    req_entity[r] = secret_input()")
    w("u_asset = req_asset[0]")
    w("u_qty = req_qty[0]")
    w("u_dir = req_dir[0]")
    w("u_entity = req_entity[0]")
    w("# Every slot runs on the fixed schedule whether or not a real request")
    w("# arrived. The flag is secret and never branched on, so the circuit shape,")
    w("# the round count and the byte count are identical either way; it only")
    w("# stops a dummy slot from moving market-maker state.")
    w("u_is_real = secret_input()")
    w("")
    w("# The trader's one-time mask. The answer leaves the circuit as")
    w("# `best_key + mask` opened to everyone, which is uniform to everyone but")
    w("# the trader, who subtracts. `reveal_to(0)` handed the winning price and")
    w("# the winning maker to computing node 0 in the clear -- the one party")
    w("# that is not supposed to learn the answer to a request it cannot read.")
    w("u_mask = secret_input()")
    if binding_limit:
        w("")
        w("# ---- the taker's acceptance level, committed with the request ----")
        w("# Today a quote is an offer: the taker reads it and decides. That is")
        w("# what makes probing free --- ask, read, walk away, repeat. A")
        w("# committed level turns the offer into an order: a quote at or inside")
        w("# it IS a trade, so the only way to learn the market is better than")
        w("# some level is to trade at it.")
        w("#")
        w("# What stays free is the other direction. A taker can raise the level")
        w("# from below and learn `worse than this` each time at no cost, exactly")
        w("# as an unfilled limit order in a public book tells you the market is")
        w("# worse than where you posted. So this does not stop probing; it puts")
        w("# the leak at the same place a central limit order book already has")
        w("# it, and no further --- and `L` is committed rather than displayed,")
        w("# so it is one step better than the book.")
        w("u_limit = secret_input()")
        w("fill_mask = secret_input()")
    w("")
    w("# ---- market-maker price policies, one column per field ----")
    for f in FIELDS:
        w(f"col_{f} = Array(M, sint)")
    w("")
    w("# Each maker deals its own policy to every node. The previous form gave")
    w("# maker i entirely to node i % N_PARTIES, which is a policy in the clear")
    w("# at one of the nodes it is supposed to be hidden from.")
    w("for i in range(M):")
    for f in FIELDS:
        w(f"    col_{f}[i] = secret_input()")
    w("")
    if input_check and check_mode == "per-party":
        w("")
        w("# ---- input check, one opening per node ----------------------------")
        w("# `zk/input_check.py` build_per_party / verify_per_party is the other")
        w("# half. It combines the same powers of the same rho into the share")
        w("# commitments `roles.Dealing` already publishes, so a failing opening")
        w("# names a node from data anybody has.")
        w("_rho = sint.get_random_int(CHALLENGE_BITS).reveal()")
        w("print_ln('QOMM_CHALLENGE=%s', _rho)")
        w("for _p in range(N_PARTIES):")
        w("    _c = cint(1)")
        w("    _acc = sint(0)")
        w("    for _k in range(N_CHECKED):")
        w("        _c = _c * _rho")
        w("        _acc = _acc + check_store[_p][_k] * _c")
        w("    # the mask is this node's own input and is not split, which is")
        w("    # why it hides with one uniform field element rather than the")
        w("    # width budget the integer version needed")
        w("    _acc = _acc + sint.get_input_from(_p)")
        w("    print_ln('QOMM_PER_PARTY_CHECK_0_%s=%s', _p, _acc.reveal())")
        w("")
    elif input_check:
        w("")
        w("# ---- input check: one random linear combination -------------------")
        w("# The coefficients are public and derived from the commitments the")
        w("# dealer published, so a node choosing what to substitute cannot see")
        w("# them first. Public times secret is local, so the combination costs")
        w("# no communication at all; the opening below is the whole price.")
        w("# `zk/input_check.py` is the other half --- it combines the same")
        w("# coefficients into the commitments and checks this opening against it.")
        w("# Repetition rather than wider coefficients: at 127 bits the budget is")
        w("# challenge + hiding <= 41, so soundness is bought back by opening")
        w("# several independent combinations, which cost one round together")
        w("# because none of them waits on another.")
        w(f"CHECK_COEFF = {check_coefficients}")
        w(f"CHECK_REPEATS = {check_repeats}")
        w("check_masks = [secret_input() for _ in range(CHECK_REPEATS)]")
        w("for _r in range(CHECK_REPEATS):")
        w("    combination = check_masks[_r]")
        w("    _k = 0")
        w("    for i in range(M):")
        for f in FIELDS:
            w(f"        combination = combination + col_{f}[i] * "
              f"CHECK_COEFF[(_r * 7919 + _k) % len(CHECK_COEFF)]")
            w("        _k += 1")
        w("    print_ln('QOMM_INPUT_CHECK_%s=%s', _r, combination.reveal())")
        w("")
    w("idx = Array(M, sint)")
    w("for i in range(M):")
    w("    idx[i] = sint(i)")
    w("wide_idx = Array(M * N_REQ, sint)")
    w("for r in range(N_REQ):")
    w("    for i in range(M):")
    w("        wide_idx[r * M + i] = sint(i)")
    w("")
    w("")
    if reference == "none":
        # No reference term in the price rule at all.
        #
        # A maker in this design does not hold a standing registration --- its
        # policy is a secret input it re-deals whenever it likes, measured at
        # 39.6 full changes a second and 352 single-field ones. So the reason an
        # anchor exists elsewhere, that a committed price goes stale as the
        # market moves, does not apply: a maker prices on its own account and
        # withdraws by setting `active` to zero, which is one field.
        #
        # Removing it also removes a mismatch. The circuit computed
        # `anchored = mid + use_ref * ref` while the quote proof's statement is
        # `ask = mid + half + depth + skew`, so the two were about different
        # rules and `shares_from_circuit` refuses the difference. With no
        # reference they are the same rule.
        #
        # And `REF_TABLE` was a compile-time literal, so every quote was as old
        # as the compilation. There is nothing left to be old.
        w("# No reference price. The maker carries the whole level in `mid` and")
        w("# re-deals it as often as it likes; nothing here is standing.")
        w("ref_secret_per_request = None")
        w("ref_secret = None")
        w("asset_onehot = [req_asset[0] == sint(a) for a in range(N_ASSETS)]")
        w("")
    else:
        w("# ---- oblivious reference-price lookup ----")
    w("# ref = sum_a (asset == a) * REF_TABLE[a]. Each term multiplies a secret")
    w("# bit by a public constant, which costs nothing, so the whole lookup is")
    w("# N_ASSETS equality tests in one layer. Selecting the row publicly would")
    w("# announce which market the request is for, which is the thing to avoid.")
    w("ref_secret_per_request = Array(N_REQ, sint)")
    w("asset_onehot = None")
    w("for r in range(N_REQ):")
    w("    onehot = [req_asset[r] == sint(a) for a in range(N_ASSETS)]")
    w("    if r == 0:")
    w("        asset_onehot = onehot")
    w("    acc = onehot[0] * REF_TABLE[0]")
    w("    for a in range(1, N_ASSETS):")
    w("        acc = acc + onehot[a] * REF_TABLE[a]")
    w("    ref_secret_per_request[r] = acc")
    w("ref_secret = ref_secret_per_request[0]")
    w("")
    if public_maker_assets:
        w("# gather the one-hot bit for each maker's publicly known market")
        w("asset_gate = Array(M, sint)")
        w("for i in range(M):")
        w("    asset_gate[i] = asset_onehot[MAKER_ASSET[i]]")
        w("")
    w("# One job serves N_REQ requests. Every per-maker vector is widened to")
    w("# N_REQ*M so the comparison layers are shared: rounds are a property of the")
    w("# job, not of the request, which is what makes batching worth doing.")
    w("WIDE = N_REQ * M")
    w("def tile_makers(vec):")
    w("    out = Array(WIDE, sint)")
    w("    for r in range(N_REQ):")
    w("        out.assign(vec, r * M)")
    w("    return out.get_vector()")
    w("def spread_request(arr):")
    w("    out = Array(WIDE, sint)")
    w("    for r in range(N_REQ):")
    w("        out.assign(arr[r].expand_to_vector(M), r * M)")
    w("    return out.get_vector()")
    w("qty_v = spread_request(req_qty)")
    w("asset_v = spread_request(req_asset)")
    w("dir_v = spread_request(req_dir)")
    w("")
    w("inv_state = Array(M, sint)")
    w("inv_state.assign(col_inv.get_vector())")
    w("")
    w("")
    w("def pack_key(cost, index_vec):")
    w('    """Pack the tie-breaking index into the low bits.')
    w("")
    w("    Two things fall out of this. Keys become unique, so a strict comparison")
    w("    is enough and no tie-breaking logic is needed. And the winning index")
    w("    travels inside the value, so the tournament no longer has to carry a")
    w("    second secret array and pay a second multiplication at every level.")
    w('    """')
    w("    return cost * M + index_vec")
    w("")
    w("")
    w("# The packed key is only ever unpacked after it is opened, by whoever")
    w("# received it. Doing it in secret would cost a division and a modulo.")
    w("")
    w("")
    w("def min_tree(keys, n):")
    w('    """Binary tournament: depth log2(n), one comparison and one select per level."""')
    w("    cur = Array(n, sint)")
    w("    cur.assign(keys)")
    w("    size = n")
    w("    while size > 1:")
    w("        half = size // 2")
    w("        a = cur.get_vector(0, half)")
    w("        b = cur.get_vector(half, half)")
    w("        cur.assign((a < b).if_else(a, b), 0)")
    w("        size = half")
    w("    return cur[0]")
    w("")
    w("")
    w("def min_kary(keys, n, arity):")
    w('    """Arity-k tournament: depth log_k(n) levels, k(k-1) comparisons per group.')
    w("")
    w("    Trades comparison count for circuit depth. Depth costs round trips, which")
    w("    are the binding constraint over a wide area; comparison count costs")
    w("    bandwidth, which is not. The best arity therefore depends on the link, so")
    w("    it is left as a measured parameter rather than a fixed choice.")
    w('    """')
    w("    cur = Array(n, sint)")
    w("    cur.assign(keys)")
    w("    size = n")
    w("    while size > 1:")
    w("        size = kary_level(cur, size, arity)")
    w("    return cur[0]")
    w("")
    w("")
    w("def kary_level(cur, size, arity):")
    w('    """One level, in place. Split out so the fill fold can stop one short."""')
    w("    a = min(arity, size)")
    w("    groups = size // a")
    w("    # block p holds the p-th member of every group, so each block is contiguous")
    w("    blocks = [cur.get_vector(p * groups, groups) for p in range(a)]")
    w("    ranks = []")
    w("    for p in range(a):")
    w("        rank = None")
    w("        for q in range(a):")
    w("            if p == q:")
    w("                continue")
    w("            less = blocks[q] < blocks[p]")
    w("            rank = less if rank is None else rank + less")
    w("        ranks.append(rank)")
    w("    # the rank is at most arity-1, so the equality does not need the")
    w("    # full key width. Comparing at full width was costing more rounds")
    w("    # than the extra tree level it was supposed to remove.")
    w("    rank_bits = max(2, (a - 1).bit_length() + 1)")
    w("    winner = None")
    w("    for p in range(a):")
    w("        selected = ranks[p].equal(0, rank_bits)")
    w("        term = selected * blocks[p]")
    w("        winner = term if winner is None else winner + term")
    w("    cur.assign(winner, 0)")
    w("    return groups")
    w("")
    w("")
    w("def argmin(keys, n):")
    if argmin_arity <= 2:
        w("    return min_tree(keys, n)")
    else:
        w(f"    return min_kary(keys, n, {argmin_arity})")
    w("")
    w("")
    if binding_limit:
        w("def argmin_fill(keys, n, limit):")
        w('    """The winner and whether it beats the limit, at the same depth.')
        w("")
        w("    `min(a, b) <= L` is decided by `a <= L` and `b <= L`, and both")
        w("    operands exist before the last tournament level runs. So the last")
        w("    level compares three pairs in one layer where it used to compare")
        w("    one, and selects twice in one layer where it used to select once.")
        w("    The fill comparison stops being a standalone comparison paying")
        w("    full depth after the tournament and becomes extra lanes inside a")
        w("    layer that was going to run anyway.")
        w("")
        w("    Comparisons here are `<=` where the tournament used `<`. The two")
        w("    differ only on a tie, and keys cannot tie: a key is")
        w("    `cost * WIDE + index` and the index is unique per lane.")
        w('    """')
        w("    cur = Array(n, sint)")
        w("    cur.assign(keys)")
        w("    size = n")
        if argmin_arity <= 2:
            w("    while size > 2:")
            w("        half = size // 2")
            w("        a = cur.get_vector(0, half)")
            w("        b = cur.get_vector(half, half)")
            w("        cur.assign((a <= b).if_else(a, b), 0)")
            w("        size = half")
            w("    if size == 1:")
            w("        # a single maker leaves no last level to fold into")
            w("        return cur[0], cur[0] <= limit")
            w("    left = Array(3, sint)")
            w("    left.assign_vector(cur.get_vector(0, 1), 0)")
            w("    left.assign_vector(cur.get_vector(0, 1), 1)")
            w("    left.assign_vector(cur.get_vector(1, 1), 2)")
            w("    right = Array(3, sint)")
            w("    right.assign_vector(cur.get_vector(1, 1), 0)")
            w("    right.assign_vector(limit, 1)")
            w("    right.assign_vector(limit, 2)")
            w("    bits = Array(3, sint)")
            w("    bits.assign(left.get_vector() <= right.get_vector())")
            w("    first = bits[0]")
            w("    return (first.if_else(cur[0], cur[1]),")
            w("            first.if_else(bits[1], bits[2]))")
        else:
            w(f"    arity = {argmin_arity}")
            w("    while size > arity:")
            w("        size = kary_level(cur, size, arity)")
            w("    if size == 1:")
            w("        return cur[0], cur[0] <= limit")
            w("    # The last level with one more comparison per finalist. Cell")
            w("    # (p, q) of the square holds `finalists[q] <= finalists[p]`")
            w("    # off the diagonal, which is p's rank term, and")
            w("    # `finalists[p] <= limit` on it, which is whether p fills. All")
            w("    # of them in one layer, then the two selects in the next.")
            w("    finalists = [cur.get_vector(p, 1) for p in range(size)]")
            w("    left = Array(size * size, sint)")
            w("    right = Array(size * size, sint)")
            w("    for p in range(size):")
            w("        for q in range(size):")
            w("            left.assign_vector(finalists[q], p * size + q)")
            w("            right.assign_vector(finalists[p] if p != q else limit,")
            w("                                p * size + q)")
            w("    cell = Array(size * size, sint)")
            w("    cell.assign(left.get_vector() <= right.get_vector())")
            w("    rank_bits = max(2, (size - 1).bit_length() + 1)")
            w("    winner = None")
            w("    filled = None")
            w("    for p in range(size):")
            w("        rank = None")
            w("        for q in range(size):")
            w("            if p == q:")
            w("                continue")
            w("            term = cell[p * size + q]")
            w("            rank = term if rank is None else rank + term")
            w("        chosen = rank.equal(0, rank_bits)")
            w("        prize = chosen * finalists[p]")
            w("        fits = chosen * cell[p * size + p]")
            w("        winner = prize if winner is None else winner + prize")
            w("        filled = fits if filled is None else filled + fits")
            w("    return winner, filled")
        w("")
        w("")
    if persist_wires:
        w("# Where the joint prover reads the wires from. The vectors inside")
        w("# `quote_layer` are local to it, and the proof is about all of them,")
        w("# so they are parked here rather than returned --- which would change")
        w("# a signature three other modes share.")
        for name in ("mid", "half", "slope", "invcoef", "inv", "maxqty",
                     "expiry", "active", "depth", "skew", "ask", "bid",
                     "fits", "ok",
                     # the proof's gates want the signed margins, not the bits:
                     # `fits` in the circuit is `qty <= maxqty`, and what a range
                     # proof is about is `maxqty - qty`. Both are kept, because
                     # they are different things and calling them one name is
                     # how the two sides drifted apart in the first place.
                     "fits_margin", "fresh_margin", "fresh_bit",
                     # `holds * value` for each gate, which is a multiplication
                     # the circuit can do and the prover cannot
                     "fits_product", "fresh_product",
                     "both", "gated", "cost"):
            w(f"W_{name} = Array(WIDE, sint)")
        w("W_key = Array(WIDE, sint)")
        w("W_qty = Array(1, sint)")
        w("")
    w("def quote_layer(inv_vec, ref_secret, now_t):")
    w('    """One evaluation of P_i(x, s_i, m_t) for every market maker at once."""')
    w("    mid = tile_makers(col_mid.get_vector())")
    w("    half = tile_makers(col_half.get_vector())")
    w("    slope = tile_makers(col_slope.get_vector())")
    w("    invcoef = tile_makers(col_invcoef.get_vector())")
    w("    maxqty = tile_makers(col_maxqty.get_vector())")
    w("    expiry = tile_makers(col_expiry.get_vector())")
    w("    active = tile_makers(col_active.get_vector())")
    w("    asset_mm = tile_makers(col_asset.get_vector())")
    w("    # layer 1: price policy (2 SIMD multiplications, depth 1)")
    w("    skew = invcoef * tile_makers(inv_vec)")
    w("    depth = slope * qty_v")
    w("    # mid is the maker's offset from the reference for its own asset,")
    w("    # unless the maker switched the reference off, in which case mid is")
    w("    # the level itself. One more multiplication in a layer that already")
    w("    # has two, so the depth --- and the round count --- does not move.")
    if reference == "none":
        w("    # the level is the maker's own; nothing is added to it")
        w("    anchored = mid")
    else:
        w("    use_ref = tile_makers(col_use_ref.get_vector())")
        w("    anchored = mid + use_ref * spread_request(ref_secret_per_request)")
    if price_conditionals:
        w(f"    # {price_conditionals} conditional(s) on secrets in the price rule.")
        w("    # A branch on a secret is a comparison, and comparisons are what")
        w("    # rounds are made of --- but every maker is evaluated at once, so")
        w("    # this costs its depth once rather than once per maker.")
        for index in range(price_conditionals):
            bound = 200 * (index + 1)
            if index % 2 == 0:
                w(f"    skew = (skew > sint({-bound})).if_else(skew, sint({-bound}))")
            else:
                w(f"    skew = (skew < sint({bound})).if_else(skew, sint({bound}))")
    w("    ask = anchored + half + depth + skew")
    w("    bid = anchored - half - depth + skew")
    if persist_wires:
        for name in ("mid", "half", "slope", "invcoef", "maxqty", "expiry",
                     "active", "depth", "skew", "ask", "bid"):
            w(f"    W_{name}.assign({name})")
        w("    W_inv.assign(tile_makers(inv_vec))")
        w("    W_fits_margin.assign(maxqty - qty_v)")
        w("    W_fresh_margin.assign(expiry - sint(now_t) - 1)")
    if stop_after in ("price", "direction"):
        w("    # Cut before the eligibility layer. Nothing below this line is built,")
        w("    # so the rounds this circuit costs are the price layer's own.")
        w("    return ask, bid, None, maxqty")
    else:
        w("    # layer 2: eligibility (3 SIMD comparisons + 3 SIMD multiplications, depth 1+cmp)")
        if public_maker_assets:
            w("    # The market each maker serves is public business information; only the")
            w("    # *user's* asset is secret. So the asset gate is a public index into the")
            w("    # secret one-hot vector, which costs no communication at all, instead of")
            w("    # an equality test per maker.")
            w("    g_asset = tile_makers(asset_gate.get_vector())")
        else:
            w("    g_asset = asset_mm == asset_v")
        w("    g_qty = qty_v <= maxqty")
        if persist_wires:
            w("    W_fits.assign(g_qty)")
            w("    _fresh_bit = expiry > sint(now_t)")
            w("    W_fresh_bit.assign(_fresh_bit)")
            w("    # holds * value for each gate: one multiplication each, and")
            w("    # the wire a product proof is about")
            w("    W_fits_product.assign(g_qty * (maxqty - qty_v))")
            w("    W_fresh_product.assign(_fresh_bit * (expiry - sint(now_t) - 1))")
            w("    W_both.assign(g_qty * _fresh_bit)")
        if audit_gates:
            w("    # The expiry is proved at registration: the auditor refuses an audit")
            w("    # unless `now < expiry <= now + horizon`, so re-checking it here pays")
            w("    # for the same fact twice.")
            w("    #")
            w("    # The active flag is *not*. The audit proves it is a bit and never")
            w("    # that it is set, and it could not usefully prove it is set --- a")
            w("    # committed one is a public one, and whether a maker is quoting at")
            w("    # all is what the commitment is hiding. So it stays in the circuit.")
            w("    # This flag used to drop it too, and with it dropped a maker that had")
            w("    # withdrawn still won tournaments.")
            w("    ok = active * g_asset * g_qty")
            if persist_wires:
                w("    W_ok.assign(ok)")
        else:
            w("    g_exp = expiry > sint(now_t)")
            w("    ok = active * g_asset * g_qty * g_exp")
            if persist_wires:
                w("    W_ok.assign(ok)")
        w("    return ask, bid, ok, maxqty")
    w("")
    w("")

    if disclose == "threshold":
        w(f"BAND = {band_bps} * REF_MID // 10000")
        w(f"THRESHOLD_K = {threshold_k}")
        w(f"THRESHOLD_V = {threshold_v}")
        w("")
        w("")
        w("def threshold_disclosure(ask, ok, maxqty, ref_secret):")
        w('    """ZK-style threshold statement: >=K independent MMs, >=V size, inside the band."""')
        w("    lo = ask >= (ref_secret - BAND).expand_to_vector(M)")
        w("    hi = ask <= (ref_secret + BAND).expand_to_vector(M)")
        w("    in_band = lo * hi")
        w("    elig = ok * in_band")
        w("    size_v = elig * maxqty")
        w("    elig_a = Array(M, sint)")
        w("    elig_a.assign(elig)")
        w("    size_a = Array(M, sint)")
        w("    size_a.assign(size_v)")
        w("    n_ok = elig_a[0]")
        w("    vol = size_a[0]")
        w("    for i in range(1, M):")
        w("        n_ok = n_ok + elig_a[i]")
        w("        vol = vol + size_a[i]")
        w("    return (n_ok >= sint(THRESHOLD_K)) * (vol >= sint(THRESHOLD_V))")
        w("")
        w("")

    if mode == "rfq":
        w("ask, bid, ok, maxqty = quote_layer(inv_state.get_vector(), ref_secret, NOW_T)")
        # Each cut opens one value that every layer above it feeds, so nothing
        # already built can be optimised away and the increment between two cuts
        # is the layer between them.
        if stop_after == "price":
            w("# stage cut: open one value that both priced sides feed")
            w("stage_out = Array(WIDE, sint)")
            w("stage_out.assign(ask + bid)")
            w("stage_out.get_vector().reveal_to(0)")
            return "\n".join(lines) + "\n"
        w("# direction stays secret: minimise the user's cost on whichever side applies")
        w("cost = dir_v.if_else(-bid, ask)")
        if stop_after == "direction":
            w("# stage cut: the selection is built, the gates are not")
            w("stage_out = Array(WIDE, sint)")
            w("stage_out.assign(cost)")
            w("stage_out.get_vector().reveal_to(0)")
            return "\n".join(lines) + "\n"
        if persist_wires:
            w("W_cost.assign(cost)")
            w("W_gated.assign(ok * (cost - sint(LARGE)))")
            w("W_qty[0] = qty_v.get_vector(0, 1)")
        w("cost = ok.if_else(cost, sint(LARGE))")
        if stop_after == "gates":
            w("# stage cut: everything but the tournament")
            w("stage_out = Array(WIDE, sint)")
            w("stage_out.assign(cost)")
            w("stage_out.get_vector().reveal_to(0)")
            return "\n".join(lines) + "\n"
        w("wide_keys = Array(WIDE, sint)")
        w("wide_keys.assign(pack_key(cost, wide_idx.get_vector()))")
        if persist_wires:
            w("W_key.assign(wide_keys.get_vector())")
        if binding_limit:
            w("# The comparison is against the packed key, not the price: a key")
            w("# is cost*WIDE + maker, so `cost <= L` is `key <= L*WIDE + WIDE-1`")
            w("# and no unpacking is needed. WIDE is public, so the scaling is")
            w("# local.")
            w("limit_key = u_limit * WIDE + (WIDE - 1)")
            w("# The fill comparison rides the tournament's last layer rather")
            w("# than running after it. Measured at +9 rounds standalone.")
            w("best_key, fill = argmin_fill(wide_keys.get_vector(0, M), M,")
            w("                             limit_key)")
        else:
            w("best_key = argmin(wide_keys.get_vector(0, M), M)")
        w("for r in range(1, N_REQ):")
        w("    (argmin(wide_keys.get_vector(r * M, M), M) + u_mask).reveal()")
        w("# one opened value carries both the winning price and the winning")
        w("# maker, under the trader's mask")
        if binding_limit:
            w("# Both outputs go back under the trader's masks. Revealing `fill`")
            w("# in the clear would say which slots traded, which is precisely")
            w("# what the `is_real` cover traffic exists to hide --- a public")
            w("# fill bit would mark every cover slot as cover.")
            w("print_ln('QOMM_MASKED_FILL=%s', (fill + fill_mask).reveal())")
            w("print_ln('QOMM_MASKED_KEY=%s', (fill * best_key + u_mask).reveal())")
        else:
            w("print_ln('QOMM_MASKED_KEY=%s', (best_key + u_mask).reveal())")
        w("")
        w("# Each node keeps its *share* of the answer, written where the joint")
        w("# prover reads it. This is the binding the design has been missing:")
        w("# the quote proof is assembled from shares, and until now those")
        w("# shares were supplied to the prover separately from the ones the")
        w("# circuit computed on, so nothing said they were the same numbers.")
        w("# Writing them here and reading them there makes it one value")
        w("# crossing a named interface rather than two that agree.")
        if persist_wires:
            # Every wire the joint prover needs, in a fixed order both sides
            # agree on. Writing only the winner bound one number; the proof is
            # about all of them, and a share that reaches the prover by another
            # route is a share nothing says the circuit computed.
            #
            # `mp_spdz/persistence.py` reads this back and
            # `zk/threshold_quote.shares_from_circuit` names the order. Adding
            # a wire here without adding it there shifts every later index,
            # which is why the count is written first and checked on the way in.
            w("wires = [best_key, W_qty[0]]")
            w("for _m in range(M):")
            w("    wires += [W_mid[_m], W_half[_m], W_slope[_m], W_invcoef[_m],")
            w("              W_inv[_m], W_maxqty[_m], W_expiry[_m], W_active[_m],")
            w("              W_depth[_m], W_skew[_m], W_ask[_m], W_bid[_m],")
            w("              W_fits[_m], W_ok[_m], W_key[_m],")
            w("              W_fits_margin[_m], W_fresh_margin[_m], W_fresh_bit[_m],")
            w("              W_fits_product[_m], W_fresh_product[_m],")
            w("              W_both[_m], W_gated[_m], W_cost[_m]]")
            w("sint.write_to_file(wires)")
        else:
            w("sint.write_to_file([best_key])")

        if range_query:
            w("")
            w("# ---- one range query, named by whoever asked ----")
            w("#")
            w("# Nobody publishes a statistic here and nothing decides a")
            w("# threshold in advance. An asker names a price range and gets")
            w("# back how many eligible makers quoted inside it, with noise")
            w("# added outside. A firm contributes 0 or 1 to that count, so the")
            w("# sensitivity is 1 --- against 300 for the volume fields, which")
            w("# is why those needed noise of 1200 against a signal of 428.")
            w("#")
            w("# The bounds are public: they are what the asker asked. What")
            w("# they cost is measured --- about 9.6 rounds for the one range,")
            w("# so 0.14 s on a metro committee and 0.60 s across regions. A")
            w("# grid of them would be flat in depth only if comparisons")
            w("# batched, and they do not: 128 ranges is 1,257 rounds. One")
            w("# range per run is the shape that works, and it is also the")
            w("# shape that charges the asker for exactly what they asked.")
            w("Q_LO = sint(QUERY_LO)")
            w("Q_HI = sint(QUERY_HI)")
            w("inside = (ask >= Q_LO.expand_to_vector(M)) * \\")
            w("         (ask <= Q_HI.expand_to_vector(M))")
            w("counted = ok * inside")
            w("count_a = Array(M, sint)")
            w("count_a.assign(counted)")
            w("firms = count_a[0]")
            w("for i in range(1, M):")
            w("    firms = firms + count_a[i]")
            w("print_ln('QOMM_RANGE_COUNT=%s', firms.reveal())")
        if disclose == "threshold":
            w("pub = threshold_disclosure(ask, ok, maxqty, ref_secret)")
            w("print_ln('QOMM_DISCLOSE=%s', pub.reveal())")

    elif mode == "rfm":
        w("ask, bid, ok, maxqty = quote_layer(inv_state.get_vector(), ref_secret, NOW_T)")
        w("# two-sided: the direction is never supplied at all")
        w("ask_cost = ok.if_else(ask, sint(LARGE))")
        w("bid_cost = ok.if_else(-bid, sint(LARGE))")
        w("ask_key = argmin(pack_key(ask_cost, idx.get_vector()), M)")
        w("bid_key = argmin(pack_key(bid_cost, idx.get_vector()), M)")
        w("print_ln('QOMM_MASKED_ASK=%s', (ask_key + u_mask).reveal())")
        w("print_ln('QOMM_MASKED_BID=%s', (bid_key + u_mask).reveal())")
        if public_check:
            w("print_ln('QOMM_ASK_KEY=%s', ask_key.reveal())")
            w("print_ln('QOMM_BID_KEY=%s', bid_key.reveal())")
        if range_query:
            w("")
            w("# ---- one range query, named by whoever asked ----")
            w("#")
            w("# Nobody publishes a statistic here and nothing decides a")
            w("# threshold in advance. An asker names a price range and gets")
            w("# back how many eligible makers quoted inside it, with noise")
            w("# added outside. A firm contributes 0 or 1 to that count, so the")
            w("# sensitivity is 1 --- against 300 for the volume fields, which")
            w("# is why those needed noise of 1200 against a signal of 428.")
            w("#")
            w("# The bounds are public: they are what the asker asked. What")
            w("# they cost is measured --- about 9.6 rounds for the one range,")
            w("# so 0.14 s on a metro committee and 0.60 s across regions. A")
            w("# grid of them would be flat in depth only if comparisons")
            w("# batched, and they do not: 128 ranges is 1,257 rounds. One")
            w("# range per run is the shape that works, and it is also the")
            w("# shape that charges the asker for exactly what they asked.")
            w("Q_LO = sint(QUERY_LO)")
            w("Q_HI = sint(QUERY_HI)")
            w("inside = (ask >= Q_LO.expand_to_vector(M)) * \\")
            w("         (ask <= Q_HI.expand_to_vector(M))")
            w("counted = ok * inside")
            w("count_a = Array(M, sint)")
            w("count_a.assign(counted)")
            w("firms = count_a[0]")
            w("for i in range(1, M):")
            w("    firms = firms + count_a[i]")
            w("print_ln('QOMM_RANGE_COUNT=%s', firms.reveal())")
        if disclose == "threshold":
            w("pub = threshold_disclosure(ask, ok, maxqty, ref_secret)")
            w("print_ln('QOMM_DISCLOSE=%s', pub.reveal())")

    elif mode == "rfs":
        w(f"RFS_STEPS = {rfs_steps}")
        w("# Each step depends on the previous winner's inventory: a genuine serial chain.")
        w("for step in range(RFS_STEPS):")
        w("    # the reference moves with the slot, still without revealing the asset")
        w("    ask, bid, ok, maxqty = quote_layer(inv_state.get_vector(), ref_secret + step, NOW_T)")
        w("    cost = dir_v.if_else(-bid, ask)")
        w("    cost = ok.if_else(cost, sint(LARGE))")
        w("    keys = pack_key(cost, idx.get_vector())")
        w("    best_key = argmin(keys, M)")
        w("    best_key.reveal_to(0)")
        w("    # winner absorbs the flow, so the next quote sees a moved inventory.")
        w("    # Keys are unique, so matching the key identifies the winner without")
        w("    # opening the index or paying a secret division.")
        w("    won = keys == best_key.expand_to_vector(M)")
        w("    signed_qty = u_dir.if_else(qty_v, -qty_v)")
        w("    real_v = u_is_real.expand_to_vector(M)")
        w("    inv_state.assign(inv_state.get_vector() + won * signed_qty * real_v, 0)")
        if public_check:
            w("    print_ln('QOMM_RFS_STEP_%s_KEY=%s', step, best_key.reveal())")
        if range_query:
            w("")
            w("# ---- one range query, named by whoever asked ----")
            w("#")
            w("# Nobody publishes a statistic here and nothing decides a")
            w("# threshold in advance. An asker names a price range and gets")
            w("# back how many eligible makers quoted inside it, with noise")
            w("# added outside. A firm contributes 0 or 1 to that count, so the")
            w("# sensitivity is 1 --- against 300 for the volume fields, which")
            w("# is why those needed noise of 1200 against a signal of 428.")
            w("#")
            w("# The bounds are public: they are what the asker asked. What")
            w("# they cost is measured --- about 9.6 rounds for the one range,")
            w("# so 0.14 s on a metro committee and 0.60 s across regions. A")
            w("# grid of them would be flat in depth only if comparisons")
            w("# batched, and they do not: 128 ranges is 1,257 rounds. One")
            w("# range per run is the shape that works, and it is also the")
            w("# shape that charges the asker for exactly what they asked.")
            w("Q_LO = sint(QUERY_LO)")
            w("Q_HI = sint(QUERY_HI)")
            w("inside = (ask >= Q_LO.expand_to_vector(M)) * \\")
            w("         (ask <= Q_HI.expand_to_vector(M))")
            w("counted = ok * inside")
            w("count_a = Array(M, sint)")
            w("count_a.assign(counted)")
            w("firms = count_a[0]")
            w("for i in range(1, M):")
            w("    firms = firms + count_a[i]")
            w("print_ln('QOMM_RANGE_COUNT=%s', firms.reveal())")
        if disclose == "threshold":
            w("pub = threshold_disclosure(ask, ok, maxqty, ref_secret)")
            w("print_ln('QOMM_DISCLOSE=%s', pub.reveal())")
    else:
        raise ValueError(f"unknown mode {mode}")

    return "\n".join(lines) + "\n"


# Where things sit in the stream `build_inputs` writes. The request's four
# fields, then `is_real`, then the trader's mask (and its limit pair when the
# binding limit is on), then each maker's policy in `FIELDS` order.
#
# This lives beside the function that lays the stream out, and nowhere else.
# Something audited elsewhere --- a state chain, a policy band --- has to be
# tied to the commitment that was actually dealt for it, and finding that
# commitment by counting again somewhere else is precisely how the gap
# `BINDING.md` opens with came about.
REQUEST_FIELDS = 4


def position_of(maker: int, field: str, *, n_requests: int = 1,
                binding_limit: bool = False) -> int:
    """Where one maker's one policy field sits in the stream the circuit reads."""
    if field not in FIELDS:
        raise ValueError(f"{field} is not a policy field; expected one of {FIELDS}")
    base = n_requests * REQUEST_FIELDS + 2 + (2 if binding_limit else 0)
    return base + maker * len(FIELDS) + FIELDS.index(field)


def commitment_at(bound, maker: int, field: str, **kw):
    """The dealt commitment for one maker's one field.

    `bound` is a `qomm_transport.binding.BoundInputs`. This is the object the
    Rust `state_audit` takes when it refuses a chain about any other inventory.
    """
    return bound.values[position_of(maker, field, **kw)].commitment.commitment


def build_inputs(
    n_mm: int,
    n_real_mm: int,
    n_parties: int,
    is_real: int,
    n_requests: int,
    n_assets: int,
    ref_table: list,
    user_asset: int,
    user_qty: int,
    user_dir: int,
    user_entity: int,
    now_t: int,
    ref_mid: int,
    seed: int,
    audit_gates: bool = False,
    value_bits: int = 32,
    field_bits: int = 128,
    use_ref: int = 1,
    reference: str = "anchored",
    input_check: bool = False,
    check_mode: str = "aggregate",
    binding_limit: bool = False,
    user_limit: int = 100000,
    check_coefficients: list | None = None,
    check_repeats: int = 7,
    policies_in: list | None = None,
    shamir_prime: int | None = None,
    shamir_threshold: int = 2,
    deal_hook=None,
) -> tuple[dict[int, list[int]], dict]:
    """Deterministic policy fixture. Padding slots are inactive.

    `deal_hook(value, position) -> list[int]` takes over the dealing. It exists
    so that the party that commits to a value can be the party that shares it:
    `zk/binding.py` passes a hook that commits, shares, records the commitment
    and returns the shares, so the audit is produced *by* the dealing instead of
    beside it. Beside it is what went wrong --- the audit sharing Shamir over the
    group order while the circuit read additive shares over the integers, with
    nothing anywhere comparing the two. One place knows the order of the input
    stream, and this is it.

    `policies_in` replaces the fixture with policies someone else chose --- a
    person editing them in the demo, a tape replayed from a venue --- so the
    circuit can be run against a market that is not this function's own random
    draw. Everything else, the dealing and the cleartext reference included, is
    unchanged, which is the point: a supplied market and the fixture have to go
    through the same code or the reference stops being a check.

    Returns (per-party input list, cleartext reference used to check the result).
    """
    rng = random.Random(seed)
    per_party: dict[int, list[int]] = {p: [] for p in range(n_parties)}

    # An input party splits its value and hands one share to each node, in the
    # order `secret_input()` reads them. Party p's file therefore holds one
    # share of every secret and no whole value of anything -- which
    # `tests/test_mpc_inputs.py` checks rather than trusts.
    if shamir_prime is None:
        check_field_width(n_parties, value_bits, field_bits)

    # The sharing draws from its own stream. Sharing off `rng` would make the
    # policy fixture depend on how many values had been split before it, so the
    # same seed would stop meaning the same market.
    share_rng = random.Random(seed ^ 0x5EED)

    def deal(value: int, width: int | None = None) -> None:
        if deal_hook is not None:
            for party, share in enumerate(deal_hook(int(value),
                                                    len(per_party[0]))):
                per_party[party].append(share)
            return
        if shamir_prime is not None:
            # The sharing the policy audit commits to, dealt so that it is also
            # the sharing the circuit reads. `width` has no meaning here --- a
            # share is a uniform field element, so there is no slack to size.
            for party, share in enumerate(
                    shamir_split(int(value), n_parties, shamir_threshold,
                                 shamir_prime, share_rng)):
                per_party[party].append(share)
            return
        for party, share in enumerate(split(int(value), n_parties,
                                            value_bits if width is None else width,
                                            share_rng)):
            per_party[party].append(share)

    for _ in range(n_requests):
        for value in (user_asset, user_qty, user_dir, user_entity):
            deal(value)
    deal(int(is_real))
    # The trader's mask. Drawn here because the fixture stands in for the
    # trader; a deployment draws it on the trader's own machine and keeps it.
    mask = share_rng.randrange(1 << value_bits)
    deal(mask)
    if binding_limit:
        # The level the taker will trade at, and a second mask so the fill bit
        # comes back to the taker alone. Both are inputs like any other, so the
        # input check covers them and a node cannot move the taker's level.
        deal(int(user_limit))
        deal(share_rng.randrange(1 << value_bits))

    policies = []
    for i in range(n_mm):
        if i < n_real_mm and policies_in is not None:
            supplied = policies_in[i]
            pol = {f: int(supplied[f]) for f in FIELDS}
        elif i < n_real_mm:
            # makers are spread across every asset, so the requested market is
            # one of several the same circuit serves
            asset = i % n_assets
            pol = {
                "asset": asset,
                "mid": rng.randint(-15, 15),   # offset from the asset's reference
                "half": rng.randint(5, 40),
                "slope": rng.randint(0, 3),
                "invcoef": rng.randint(0, 2),
                "inv": rng.randint(-50, 50),
                "maxqty": rng.choice([50, 100, 200, 500]),
                "expiry": now_t + rng.randint(1, 600),
                "active": 1,
                "use_ref": (use_ref[i % len(use_ref)]
                            if isinstance(use_ref, (list, tuple))
                            else use_ref),
            }
        else:  # padding to the next power of two
            pol = {
                "asset": 0, "mid": 0, "half": 0, "slope": 0, "invcoef": 0,
                "inv": 0, "maxqty": 0, "expiry": 0, "active": 0, "use_ref": 0,
            }
        policies.append(pol)
        for f in FIELDS:
            deal(pol[f])

    if input_check:
        # The mask the combination is opened under, read last because the circuit
        # reads it last. Without it the opening is one linear equation in the
        # policy, and enough of them solve for it.
        #
        # It is much wider than a policy field, and that width is the whole
        # reason this needs a wider prime than the default: it is dealt like
        # every other input, so the field has to hold `n_parties` shares of it.
        n_values = n_mm * len(FIELDS) + n_requests * 4 + 2
        # the width the mask has to cover is set by the coefficients the
        # circuit will actually multiply by, not by a constant
        check_bits = (max(check_coefficients).bit_length()
                      if check_coefficients else 6)
        if check_mode == "per-party":
            # One mask per node, read only from that node. This is the only
            # input here that is not split, and it is why the per-party check
            # needs 160 bits of field where the aggregate one needs 164: the
            # mask does not pay the share slack that splitting costs.
            # The mask has to hide a combination of SHARES, not of values, so
            # it is `SLACK_BITS` wider than the aggregate one at the same
            # coefficient width --- and it is read from one node rather than
            # split across them, so it does not pay that slack a second time.
            # Net at 31-bit values and 40-bit coefficients: 160 bits against
            # 164. At the wider `value_bits` a deployment actually compiles
            # with, both grow together.
            # One mask per node, uniform in the field. The check is taken
            # modulo the MPC prime, so nothing has to avoid reducing and the
            # width budget that forced 164 bits does not apply: a single
            # uniform field element hides the combination exactly.
            check_field_width(n_parties, value_bits, field_bits)
            for party in range(n_parties):
                per_party[party].append(share_rng.randrange(1 << (field_bits - 1)))
        else:
            width = mask_bits_for(n_values, value_bits, challenge_bits=6,
                                  statistical_bits=35)
            check_field_width(n_parties, width, field_bits)
            for _ in range(check_repeats):
                deal(share_rng.randrange(1 << width), width=width)

    # cleartext reference (what the circuit must reproduce)
    best_price = None
    best_mm = None
    best_ask = None
    best_bid = None
    best_ask_mm = None
    best_bid_mm = None
    quotes = []
    for i, pol in enumerate(policies):
        skew = pol["invcoef"] * pol["inv"]
        depth = pol["slope"] * user_qty
        # The cleartext model has to be the same rule the circuit runs, or the
        # run's own verification compares two different computations and reports
        # the difference as a failure of the circuit.
        anchor = pol["mid"] if reference == "none" else (
            pol.get("use_ref", 1) * ref_table[user_asset] + pol["mid"])
        ask = anchor + pol["half"] + depth + skew
        bid = anchor - pol["half"] - depth + skew
        ok = (pol["asset"] == user_asset and user_qty <= pol["maxqty"]
              and pol["active"] == 1)
        if not audit_gates:
            ok = ok and pol["expiry"] > now_t
        quotes.append({"mm": i, "ask": ask, "bid": bid, "eligible": ok})
        if not ok:
            continue
        if best_ask is None or ask < best_ask:
            best_ask, best_ask_mm = ask, i
        if best_bid is None or bid > best_bid:
            best_bid, best_bid_mm = bid, i
        cost = -bid if user_dir == 1 else ask
        if best_price is None or cost < best_price:
            best_price, best_mm = cost, i
    reference = {
        "best_cost": best_price,
        "best_price": (-best_price if user_dir == 1 and best_price is not None else best_price),
        "best_mm": best_mm,
        "best_ask": best_ask,
        "best_bid": best_bid,
        "best_ask_mm": best_ask_mm,
        "best_bid_mm": best_bid_mm,
        "eligible_count": sum(1 for q in quotes if q["eligible"]),
        "quotes": quotes,
        # what the trader subtracts from the opened value. In a deployment this
        # never leaves the trader; here the fixture stands in for it.
        "mask": mask,
    }
    return per_party, reference


def finish_reference(reference: dict, *, padded: int, bit_length: int,
                     ref_table: list, is_real: int, n_assets: int,
                     user_asset: int, real_mm: int, mode: str) -> dict:
    """Everything the checker needs that the arithmetic did not produce.

    `build_inputs` computes what the circuit must reproduce; this is the packing
    and the no-quote convention around it. Split out because two callers need
    it --- the generator and `scripts/run_binding_chain.py`, which deals in
    process --- and a second copy of the sentinel rule would be a second
    definition of what "no eligible maker" means.
    """
    # the circuit opens one packed key, so the reference carries the same packing
    if reference["best_cost"] is not None:
        reference["best_key"] = reference["best_cost"] * padded + reference["best_mm"]
    if reference["best_ask"] is not None:
        reference["ask_key"] = reference["best_ask"] * padded + reference["best_ask_mm"]
        reference["bid_key"] = (-reference["best_bid"]) * padded + reference["best_bid_mm"]
    # With no eligible maker the circuit legitimately answers "no quote": every
    # key is the sentinel, so the smallest is the one at index zero. The
    # reference has to expect that rather than treat it as a mismatch.
    sentinel = sentinel_for(bit_length, padded, 8 * max(ref_table))
    if reference["best_cost"] is None:
        reference["no_eligible_maker"] = True
        reference["best_cost"] = sentinel
        reference["best_mm"] = 0
        reference["best_key"] = sentinel * padded
    else:
        reference["no_eligible_maker"] = False
    if reference["best_ask"] is None:
        reference["best_ask"] = sentinel
        reference["best_ask_mm"] = 0
        reference["ask_key"] = sentinel * padded
        reference["best_bid"] = -sentinel
        reference["best_bid_mm"] = 0
        reference["bid_key"] = sentinel * padded
    reference["is_real"] = is_real
    reference["n_assets"] = n_assets
    reference["ref_table"] = ref_table
    reference["user_asset"] = user_asset
    reference["padded_mm"] = padded
    reference["real_mm"] = real_mm
    reference["mode"] = mode
    return reference


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n-mm", type=int, default=16)
    ap.add_argument("--n-parties", type=int, default=7)
    ap.add_argument("--mode", choices=("rfq", "rfm", "rfs"), default="rfq")
    ap.add_argument("--rfs-steps", type=int, default=5)
    ap.add_argument("--disclose", choices=("none", "threshold"), default="none")
    ap.add_argument("--now-t", type=int, default=1000)
    ap.add_argument("--ref-mid", type=int, default=100000)
    ap.add_argument("--n-requests", type=int, default=1,
                    help="requests served by one job; rounds are per job, so batching "
                         "is the way to lower rounds per quote")
    ap.add_argument("--public-maker-assets", action="store_true",
                    help="treat which market a maker serves as public, so the asset "
                         "gate becomes a free lookup instead of an equality test")
    ap.add_argument("--reference", choices=("anchored", "none"),
                    default="anchored",
                    help="whether the price rule adds a reference price at all. "
                         "`none` removes it, which is what the quote proof's "
                         "statement covers and what a maker that re-deals its "
                         "own level needs")
    ap.add_argument("--use-ref", type=int, default=1,
                    help="whether makers anchor on the public reference price. "
                         "The quote proof's statement has no reference term, so "
                         "0 is the configuration its `ask` matches; see "
                         "`shares_from_circuit`, which refuses the mismatch "
                         "rather than proving the wrong rule")
    ap.add_argument("--persist-wires", action="store_true",
                    help="write each node's share of every wire the joint "
                         "prover needs, not only the winner. The prover reads "
                         "them from the same files, so the shares it proves "
                         "about are the shares the circuit computed on")
    ap.add_argument("--audit-gates", action="store_true",
                    help="drop the expiry gate from the circuit; the "
                         "registration-time policy audit refuses an audit whose "
                         "expiry is outside the horizon, so the circuit would be "
                         "paying for the same fact twice. The active flag stays: "
                         "the audit proves it is a bit and not that it is set")
    ap.add_argument("--n-assets", type=int, default=1,
                    help="assets the one circuit serves; the requested one stays secret")
    ap.add_argument("--band-bps", type=int, default=20)
    ap.add_argument("--threshold-k", type=int, default=5)
    ap.add_argument("--threshold-v", type=int, default=1000)
    ap.add_argument("--user-qty", type=int, default=100)
    ap.add_argument("--user-dir", type=int, default=0)
    ap.add_argument("--user-asset", type=int, default=0)
    ap.add_argument("--user-entity", type=int, default=42)
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--field-bits", type=int, default=128,
                    help="width of the MPC field the shares are summed in; "
                         "must match what the program is compiled for")
    ap.add_argument("--bit-length", type=int, default=63)
    ap.add_argument("--price-conditionals", type=int, default=0,
                    help="conditionals on secrets to add to the price rule. The "
                         "language already allows these through min and max; this "
                         "is here to price them in rounds, which is the only cost "
                         "the language cannot derive on its own")
    ap.add_argument("--argmin-arity", type=int, default=2,
                    help="2 = binary tournament; larger trades comparisons for depth; "
                         "set to the padded market-maker count for a single all-pairs layer")
    ap.add_argument("--check-repeats", type=int, default=7,
                    help="independent combinations; soundness is challenge bits "
                         "times this, and they open in one round")
    ap.add_argument("--binding-limit", action="store_true",
                    help="the taker commits an acceptance level and a quote at "
                         "or inside it is a trade, so learning that the market "
                         "is better than a level costs a fill")
    ap.add_argument("--user-limit", type=int, default=100000,
                    help="the taker's acceptance level, for the fixture")
    ap.add_argument("--check-coefficients", type=Path, default=None,
                    help="JSON list of public coefficients. The default is a "
                         "FIXTURE and is not sound: the whole argument is that "
                         "the coefficients come after the commitments, so a "
                         "deployment derives them from the published "
                         "commitments and passes them here.")
    ap.add_argument("--check-mode", choices=("aggregate", "per-party"),
                    default="per-party",
                    help="per-party opens one combination per node and names the "
                         "node that substituted. `aggregate` is the earlier form "
                         "and is UNSOUND as emitted; it is kept because the cost "
                         "table was taken against it, and it refuses to run "
                         "without --unsound-check-for-measurement")
    ap.add_argument("--unsound-check-for-measurement", action="store_true",
                    help="permit --check-mode aggregate, which is unsound. Only "
                         "reason to: reproducing the cost baseline the "
                         "optimisation history was measured against")
    ap.add_argument("--input-check", action="store_true",
                    help="emit the random linear combination that binds the "
                         "inputs to the published commitments")
    ap.add_argument("--trunc-pr", action="store_true",
                    help="probabilistic truncation for comparisons")
    ap.add_argument("--edabit", action="store_true",
                    help="generate comparison bits in preprocessing")
    ap.add_argument("--is-real", type=int, default=1, choices=(0, 1),
                    help="0 emits a cover slot: same circuit, no state change")
    ap.add_argument("--no-public-check", action="store_true")
    ap.add_argument("--stop-after", default="tournament",
                    choices=("price", "direction", "gates", "tournament"),
                    help="cut the circuit after a named layer, so the rounds that "
                         "layer costs can be read as an increment between compiles. "
                         "RFQ only: the other modes have a different shape and "
                         "attributing their cost would need its own cuts.")
    ap.add_argument("--inputs-only", action="store_true",
                    help="skip emitting the circuit; a resident service compiles "
                         "it once and only the inputs change per request")
    ap.add_argument("--out-program", type=Path, required=True)
    ap.add_argument("--out-input-dir", type=Path, required=True)
    ap.add_argument("--shamir-inputs", action="store_true",
                    help="deal inputs as Shamir shares over the commitment "
                         "group's scalar field instead of additively over the "
                         "integers, so the shares the nodes feed are the ones "
                         "`zk/policy_audit.py` commits to. Requires running the "
                         "MPC over that prime: --field-bits 253 and -P the order")
    ap.add_argument("--ref-table", default=None,
                    help="public reference price per asset, comma separated. "
                         "Without it the prices are spread 5000 apart, which "
                         "makes a wrong asset selection obvious in a test and "
                         "means nothing in a market")
    ap.add_argument("--policies", type=Path, default=None,
                    help="a JSON list of policy objects to price instead of the "
                         "seeded fixture, one per real maker. Each needs every "
                         "field the fixture has")
    ap.add_argument("--out-reference", type=Path, required=True)
    args = ap.parse_args()
    coefficients = (json.loads(args.check_coefficients.read_text())
                    if args.check_coefficients else None)
    if args.input_check and args.check_mode != "per-party":
        # A warning printed to stderr is a warning nobody reads in a pipeline.
        # The mode stays available because the cost table was measured against
        # it, and it stops unless somebody says that is why they want it.
        message = ("the AGGREGATE input check is unsound as emitted. Its "
                   "coefficients are fixed before the circuit reads its inputs, "
                   "so a node that has seen them can substitute two values "
                   "whose errors cancel --- see "
                   "artifacts/coefficient_timing_flaw.json. --check-mode "
                   "per-party draws its challenge after the input phase.")
        if not args.unsound_check_for_measurement:
            print(f"error: {message} Pass "
                  f"--unsound-check-for-measurement if you are reproducing the "
                  f"cost baseline; there is no other reason to.", file=sys.stderr)
            return 7
        print(f"WARNING: {message} Emitting it because "
              f"--unsound-check-for-measurement was given.", file=sys.stderr)

    padded = _pow2_ceil(args.n_mm)
    # The commitment group's scalar field, when the inputs are its shares. One
    # place, so the generator, the dealer and the audit cannot disagree about
    # which prime they are over --- a disagreement there is a wrong answer and
    # not an error.
    shamir_prime = lagrange = None
    if args.shamir_inputs:
        shamir_prime = ED25519_ORDER
        lagrange = lagrange_at_zero(args.n_parties, shamir_prime)

    # spread the reference prices apart so a wrong asset selection is obvious
    ref_table = [args.ref_mid + 5_000 * a for a in range(args.n_assets)]
    if args.ref_table:
        # A caller with its own markets --- the demo, a replayed tape --- says
        # what they are worth. The table is public and is compiled into the
        # program, so changing it is a change of shape and forces a recompile,
        # which is the right cost for public data and the wrong one to hide.
        ref_table = [int(v) for v in args.ref_table.split(",")]
        if len(ref_table) != args.n_assets:
            print(f"error: --ref-table has {len(ref_table)} entries for "
                  f"{args.n_assets} assets", file=sys.stderr)
            return 6
    if not 0 <= args.user_asset < args.n_assets:
        print(f"error: --user-asset must be below --n-assets", file=sys.stderr)
        return 5
    try:
        sentinel_for(args.bit_length, padded, 8 * max(ref_table))
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 4
    # A resident service compiles the circuit once and then only the inputs
    # change, so building the program text again per request is pure waste ---
    # and it is the larger half of what generation costs.
    src = "" if args.inputs_only else build_program(
        n_mm=padded,
        n_parties=args.n_parties,
        mode=args.mode,
        rfs_steps=args.rfs_steps,
        disclose=args.disclose,
        now_t=args.now_t,
        ref_mid=args.ref_mid,
        band_bps=args.band_bps,
        threshold_k=args.threshold_k,
        threshold_v=args.threshold_v,
        public_check=not args.no_public_check,
        n_requests=args.n_requests,
        n_assets=args.n_assets,
        ref_table=ref_table,
        maker_assets=[i % args.n_assets for i in range(padded)],
        public_maker_assets=args.public_maker_assets,
        audit_gates=args.audit_gates,
        persist_wires=args.persist_wires,
        reference=args.reference,
        bit_length=args.bit_length,
        argmin_arity=(padded if args.argmin_arity <= 0 else args.argmin_arity),
        lagrange=lagrange,
        stop_after=args.stop_after,
        price_conditionals=args.price_conditionals,
        edabit=args.edabit,
        trunc_pr=args.trunc_pr,
        input_check=args.input_check,
        check_mode=args.check_mode,
        check_coefficients=coefficients,
        binding_limit=args.binding_limit,
        check_repeats=args.check_repeats,
    )
    if not args.inputs_only:
        args.out_program.parent.mkdir(parents=True, exist_ok=True)
        args.out_program.write_text(src, encoding="utf-8")

    policies = None
    if args.policies:
        policies = json.loads(args.policies.read_text(encoding="utf-8"))
        if len(policies) < args.n_mm:
            raise SystemExit(f"--policies has {len(policies)} entries for "
                             f"{args.n_mm} makers")
    per_party, reference = build_inputs(
        n_mm=padded,
        n_real_mm=args.n_mm,
        n_parties=args.n_parties,
        is_real=args.is_real,
        n_requests=args.n_requests,
        n_assets=args.n_assets,
        ref_table=ref_table,
        user_asset=args.user_asset,
        user_qty=args.user_qty,
        user_dir=args.user_dir,
        user_entity=args.user_entity,
        now_t=args.now_t,
        ref_mid=args.ref_mid,
        seed=args.seed,
        audit_gates=args.audit_gates,
        # The widest value an input party splits, and the field the shares are
        # reconstructed in. Both are declared rather than assumed, because a
        # field too narrow for the shares would wrap the sum and the circuit
        # would compute on a different request than the one that was sent.
        value_bits=args.bit_length + 1,
        field_bits=args.field_bits,
        input_check=args.input_check,
        check_mode=args.check_mode,
        check_coefficients=coefficients,
        binding_limit=args.binding_limit,
        user_limit=args.user_limit,
        check_repeats=args.check_repeats,
        policies_in=policies,
        shamir_prime=shamir_prime,
        shamir_threshold=(args.n_parties - 1) // 2,
        use_ref=args.use_ref,
        reference=args.reference,
    )
    args.out_input_dir.mkdir(parents=True, exist_ok=True)
    for party, values in per_party.items():
        path = args.out_input_dir / f"Input-P{party}-0"
        path.write_text(" ".join(str(v) for v in values) + "\n", encoding="utf-8")

    finish_reference(reference, padded=padded, bit_length=args.bit_length,
                     ref_table=ref_table, is_real=args.is_real,
                     n_assets=args.n_assets, user_asset=args.user_asset,
                     real_mm=args.n_mm, mode=args.mode)
    args.out_reference.write_text(json.dumps(reference, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"padded_mm": padded, "real_mm": args.n_mm, "mode": args.mode,
                      "best_price": reference["best_price"], "best_mm": reference["best_mm"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
