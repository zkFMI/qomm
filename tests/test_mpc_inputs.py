"""No computing node holds a whole request or a whole policy.

This is the property the design has always claimed and the code did not have:
the request was read with `get_input_from(0)` and each maker's policy with
`get_input_from(i % N_PARTIES)`, so node 0 held the request --- `is_real`
included --- and one node held each policy in the clear. The circuit was
therefore a benchmark of party-supplied inputs, not of a venue that hides the
request from the people running it.

Checking it here rather than reading the generator is deliberate. The property
is about what ends up in `Player-Data/Input-P*-0`, which is the thing an
operator can actually look at, so that is what the tests read.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from qomm_transport.roles import SLACK_BITS, check_field_width, split  # noqa: E402

N_PARTIES = 7
USER = {"asset": 1, "qty": 100, "dir": 0, "entity": 7}
N_ASSETS = 4


@pytest.fixture(scope="module")
def generated(tmp_path_factory):
    out = tmp_path_factory.mktemp("mpc")
    subprocess.run(
        [sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"),
         "--n-mm", "4", "--n-parties", str(N_PARTIES),
         "--n-assets", str(N_ASSETS),
         "--user-asset", str(USER["asset"]), "--user-qty", str(USER["qty"]),
         "--user-dir", str(USER["dir"]), "--user-entity", str(USER["entity"]),
         "--out-program", str(out / "q.mpc"),
         "--out-input-dir", str(out / "in"),
         "--out-reference", str(out / "ref.json")],
        check=True, capture_output=True)
    files = {p: [int(v) for v in (out / "in" / f"Input-P{p}-0").read_text().split()]
             for p in range(N_PARTIES)}
    return files, (out / "q.mpc").read_text(), json.loads((out / "ref.json").read_text())


def test_every_node_holds_the_same_number_of_shares(generated):
    files, _, _ = generated
    counts = {p: len(v) for p, v in files.items()}
    assert len(set(counts.values())) == 1, (
        f"the nodes hold different numbers of inputs: {counts}. One holding more "
        "is one that was given something the others were not.")


def test_no_node_holds_a_request_value(generated):
    files, _, _ = generated
    for party, values in files.items():
        for name, secret in USER.items():
            assert secret not in values, (
                f"node {party} holds the request's {name} ({secret}) in the clear")
        assert 1 not in values and 0 not in values, (
            f"node {party} holds a bare flag; `is_real` is one bit and a share "
            "of it must not be that bit")


def test_no_node_holds_a_policy_value(generated):
    """Policy fields are small. A share is not, and that is the whole point."""
    files, _, _ = generated
    floor = 1 << (SLACK_BITS // 2)
    for party, values in files.items():
        small = [v for v in values if 0 <= v < floor]
        assert not small, (
            f"node {party} holds {len(small)} value(s) small enough to be a "
            f"policy field rather than a share of one: {small[:5]}")


def test_the_program_reads_a_share_from_every_node(generated):
    _, source, _ = generated
    assert "def secret_input():" in source
    assert "for _p in range(N_PARTIES):" in source.split("def secret_input():")[1][:200], (
        "the reconstruction does not loop over every node")
    assert "sint.get_input_from(_p)" in source, (
        "the program does not read from every node")
    body = source.split("def secret_input():")[1]
    body = body[body.index("return total"):]
    assert "get_input_from" not in body, (
        "something outside `secret_input` still reads a party's input directly, "
        "which is the shape this test exists to prevent")


@pytest.mark.parametrize("flags", [
    (), ("--input-check",), ("--shamir-inputs",),
    ("--input-check", "--shamir-inputs"),
])
def test_there_is_exactly_one_secret_input(flags, tmp_path_factory):
    """Two definitions of one name, and the second wins silently.

    The input check emitted a `secret_input` that recorded each party's share
    into `check_store`, and Shamir inputs emitted another straight after that
    applied the Lagrange coefficients. Asking for both --- which is the
    combination `BINDING.md` recommends --- gave a circuit whose check ran over
    an array nothing had written to. The rounds and the bytes were real and the
    property was not, and nothing failed, because a check over zeros passes.

    One definition carrying both options is the fix. This is the test that
    would have caught it: not what the definition does, but that there is one.
    """
    out = tmp_path_factory.mktemp("one-input")
    subprocess.run(
        [sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"),
         "--n-mm", "4", "--n-parties", str(N_PARTIES), *flags,
         "--out-program", str(out / "q.mpc"),
         "--out-input-dir", str(out / "in"),
         "--out-reference", str(out / "ref.json")],
        check=True, capture_output=True)
    source = (out / "q.mpc").read_text()
    assert source.count("def secret_input():") == 1, \
        f"{source.count('def secret_input():')} definitions with flags {flags}"

    body = source.split("def secret_input():")[1]
    body = body[:body.index("return total")]
    wants_check = "--input-check" in flags
    wants_lagrange = "--shamir-inputs" in flags
    assert ("check_store[_p][_k] = _s" in body) == wants_check, \
        "the check store is written exactly when the check is asked for"
    assert ("LAGRANGE[_p]" in body) == wants_lagrange, \
        "the Lagrange coefficients are applied exactly when Shamir inputs are"


def test_the_shares_reconstruct_the_value():
    for value in (0, 1, 100, -50, 1_599_845):
        shares = split(value, N_PARTIES, 32)
        assert sum(shares) == value
        assert len(shares) == N_PARTIES


def test_one_share_moves_with_the_randomness_and_not_with_the_value():
    """Two values, one share each, drawn many times: the distributions overlap."""
    first = [split(0, N_PARTIES, 32)[0] for _ in range(200)]
    second = [split(10 ** 6, N_PARTIES, 32)[0] for _ in range(200)]
    assert len(set(first)) == 200, "a share repeated; the randomness is not fresh"
    assert min(first) < max(second) and min(second) < max(first), (
        "the two ranges do not overlap, so one share separates the values")


def test_a_field_too_narrow_for_the_shares_is_refused():
    check_field_width(N_PARTIES, 32, 128)
    with pytest.raises(ValueError, match="cannot hold"):
        check_field_width(N_PARTIES, 32, 64)


# --- a node that inputs something other than what it was dealt --------------
#
# These shares are additive, so a node that lies about its share shifts the sum
# and the circuit computes on a request nobody sent. Stopping that needs an
# input-consistency check inside the protocol and there is not one here. What
# there is, is the same thing the slot receipts give elsewhere: the dealt value
# is signed, so a node that put in a different one can be shown to have done it.

from cryptography.hazmat.primitives.asymmetric.ed25519 import (               # noqa: E402
    Ed25519PrivateKey,
)

from qomm_transport.roles import (                                            # noqa: E402
    ComputingNode, InputParty, audit_node, dealt_body,
)


def _dealt(values=(100, 1, 0)):
    signing = Ed25519PrivateKey.generate()
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    party = InputParty("trader", N_PARTIES, 32, signing_key=signing)
    party.deal(values, nodes)
    return signing, nodes, party


def test_an_honest_node_audits_clean():
    signing, nodes, party = _dealt()
    for node in nodes:
        assert audit_node(node, "trader", signing.public_key(), node.inputs) == []


def test_a_node_that_changed_a_share_is_named():
    signing, nodes, party = _dealt()
    lying = list(nodes[3].inputs)
    lying[1] += 1
    assert audit_node(nodes[3], "trader", signing.public_key(), lying) == [1]


def test_a_share_cannot_be_moved_to_another_node_or_position():
    signing, nodes, party = _dealt()
    # node 3 tries to pass off node 4's share, receipt and all
    borrowed = ComputingNode(3, list(nodes[4].inputs), list(nodes[4].receipts))
    assert audit_node(borrowed, "trader", signing.public_key(), borrowed.inputs), (
        "a share signed for one node passed as another's")

    reordered = ComputingNode(0, list(reversed(nodes[0].inputs)),
                              list(nodes[0].receipts))
    assert audit_node(reordered, "trader", signing.public_key(), reordered.inputs), (
        "shares reordered in the stream still audited clean")


def test_another_dealer_cannot_sign_for_this_one():
    signing, nodes, party = _dealt()
    impostor = Ed25519PrivateKey.generate()
    forged = ComputingNode(0, list(nodes[0].inputs),
                           [impostor.sign(dealt_body("trader", 0, i, v))
                            for i, v in enumerate(nodes[0].inputs)])
    assert audit_node(forged, "trader", signing.public_key(), forged.inputs) == \
        list(range(len(forged.inputs)))


def test_a_dealer_without_a_key_leaves_nothing_to_audit():
    """Said out loud rather than left to be discovered: no key, no attribution."""
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    InputParty("trader", N_PARTIES, 32).deal([100], nodes)
    signing = Ed25519PrivateKey.generate()
    assert audit_node(nodes[0], "trader", signing.public_key(), nodes[0].inputs) == [0]


# --- and a dealer that deals shares which do not add up ---------------------
#
# The signatures above make a lying node attributable and say nothing about a
# lying dealer: a trader that signs shares summing to something other than what
# it meant gets a circuit computing on that something else, and every signature
# checks out. Commitments close it, and they close it publicly -- no dealer
# cooperation after the fact, and no node has to be honest about what it holds.

from zk.commit import Pedersen                                                # noqa: E402
from zk.groups import make_group                                              # noqa: E402

from qomm_transport.roles import Dealing, check_share                         # noqa: E402


@pytest.fixture(scope="module")
def key():
    return Pedersen(make_group("ed25519"), b"qomm:transport:v1")


def test_an_honest_dealing_adds_up_and_every_node_can_check_its_own(key):
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    dealing, blindings = InputParty("trader", N_PARTIES, 32).deal_committed(
        100, nodes, key)
    assert dealing.adds_up(key)
    for i, node in enumerate(nodes):
        assert check_share(key, dealing, i, node.inputs[0], blindings[i])


def test_a_node_handed_a_share_that_is_not_its_own_knows_before_computing(key):
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    dealing, blindings = InputParty("trader", N_PARTIES, 32).deal_committed(
        100, nodes, key)
    assert not check_share(key, dealing, 3, nodes[3].inputs[0] + 1, blindings[3])


def test_a_dealing_whose_shares_do_not_sum_to_the_value_is_visible(key):
    """The dealer's lie, which no signature over the shares would catch."""
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    dealing, blindings = InputParty("trader", N_PARTIES, 32).deal_committed(
        100, nodes, key)
    order = key.group.order
    forged = Dealing(key.commit(101, sum(blindings) % order),
                     dealing.share_commitments)
    assert not forged.adds_up(key), "a dealing that sums to something else passed"


def test_swapping_one_share_commitment_breaks_the_sum(key):
    nodes = [ComputingNode(i) for i in range(N_PARTIES)]
    dealing, _ = InputParty("trader", N_PARTIES, 32).deal_committed(100, nodes, key)
    swapped = list(dealing.share_commitments)
    swapped[0] = swapped[1]
    assert not Dealing(dealing.value_commitment, swapped).adds_up(key)
