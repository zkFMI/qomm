"""The cleartext reference, checked against an implementation written from the
specification rather than from it.

Every MPC run is verified against `gen_qomm`'s cleartext reference, so a bug in
the circuit is caught. A bug the circuit and the reference *share* is not: they
are written by the same author in the same file, and the check compares them to
each other. The reference is the oracle, and an oracle with no second opinion is
an assertion.

`test_qomm.py` carried a test whose name and docstring said it compared the
reference against the simulator's `MarketMaker.quote`. It did not --- it checked
the reference against itself, that `best_ask` was the smallest eligible ask and
so on, which holds however wrong the asks are. And the two could never have been
compared directly: the simulator models a maker whose half spread is a function
of its informed-flow estimate and whose eligibility turns on an inventory limit,
while the generator prices a committed snapshot whose half spread is a field and
whose eligibility turns on an expiry and an active flag. They are different
abstractions on purpose.

So this is the second opinion: the pricing rule as `DSL.md` states it, written
out here from the statement, and held against the reference over policies drawn
at random. Anything the two disagree about is a transcription error in one of
them, which is the class of bug the run-time check cannot see.
"""

from __future__ import annotations

import json
import random
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

FIELDS = ("asset", "mid", "half", "slope", "invcoef", "inv", "maxqty",
          "expiry", "active", "use_ref")


def priced(policy: dict, qty: int, asset: int, reference: int, now: int) -> dict:
    """The rule as stated: an anchor, a half spread, a depth term and a skew.

    Written from the statement of the rule and not from the generator. The
    ordering matters --- the skew enters both sides with the same sign, which is
    what makes the midpoint of a two-sided quote carry the inventory and is why
    the probing attack of section 8 works at all.
    """
    anchor = policy["mid"] + (policy["use_ref"] * reference)
    depth = policy["slope"] * qty
    skew = policy["invcoef"] * policy["inv"]
    eligible = (policy["asset"] == asset
                and qty <= policy["maxqty"]
                and policy["active"] == 1
                and policy["expiry"] > now)
    return {"ask": anchor + policy["half"] + depth + skew,
            "bid": anchor - policy["half"] - depth + skew,
            "eligible": eligible}


def generate(policies: list[dict], qty: int, asset: int, reference: int) -> dict:
    out = Path(tempfile.mkdtemp(prefix="qomm-second-opinion-"))
    (out / "policies.json").write_text(json.dumps(policies))
    subprocess.run(
        [sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"),
         "--n-mm", str(len(policies)), "--n-assets", "4",
         "--user-qty", str(qty), "--user-asset", str(asset),
         "--ref-table", ",".join([str(reference)] * 4),
         "--policies", str(out / "policies.json"),
         "--out-program", str(out / "q.mpc"),
         "--out-input-dir", str(out / "in"),
         "--out-reference", str(out / "ref.json")],
        check=True, capture_output=True)
    return json.loads((out / "ref.json").read_text())


def a_policy(rng: random.Random, asset: int) -> dict:
    return {"asset": asset, "mid": rng.randrange(-2000, 2000),
            "half": rng.randrange(1, 200), "slope": rng.randrange(0, 16),
            "invcoef": rng.randrange(0, 8), "inv": rng.randrange(-4000, 4000),
            "maxqty": rng.randrange(1, 1000), "expiry": 10 ** 9,
            "active": 1, "use_ref": rng.choice([0, 1])}


def test_the_reference_agrees_with_the_rule_as_stated() -> None:
    rng = random.Random(20260823)
    reference, now = 100_000, 0
    for _ in range(12):
        asset = rng.randrange(4)
        policies = [a_policy(rng, rng.randrange(4)) for _ in range(8)]
        # at least one maker in the asked-for market, or the comparison is
        # between two implementations that both do nothing
        policies[0]["asset"] = asset
        policies[0]["maxqty"] = 1000
        qty = rng.randrange(1, 900)

        ref = generate(policies, qty, asset, reference)
        for record in ref["quotes"]:
            mine = priced(policies[record["mm"]], qty, asset, reference, now)
            assert record["ask"] == mine["ask"], (
                f"ask disagrees for maker {record['mm']}: reference "
                f"{record['ask']}, rule as stated {mine['ask']}")
            assert record["bid"] == mine["bid"]
            assert record["eligible"] == mine["eligible"], (
                f"eligibility disagrees for maker {record['mm']}")


def test_the_winner_is_the_one_the_rule_picks() -> None:
    """The tournament, independently. The reference reports a winner; this
    recomputes it from the quotes the rule gives rather than from the ones the
    reference reported, so an error in both the quote and the selection cannot
    cancel."""
    rng = random.Random(7)
    reference, now = 100_000, 0
    for _ in range(8):
        asset = rng.randrange(4)
        policies = [a_policy(rng, asset) for _ in range(8)]
        qty = rng.randrange(1, 500)
        for policy in policies:
            policy["maxqty"] = max(policy["maxqty"], qty)

        ref = generate(policies, qty, asset, reference)
        mine = [priced(p, qty, asset, reference, now) for p in policies]
        eligible = [(q["ask"], i) for i, q in enumerate(mine) if q["eligible"]]
        assert eligible
        best_ask, best_mm = min(eligible)
        assert ref["best_ask"] == best_ask
        assert ref["best_ask_mm"] == best_mm
