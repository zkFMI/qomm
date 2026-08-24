"""The circuit does not depend on the request. This is the central claim.

Everything else in this repository is a cost or a caveat attached to one
sentence: the request is never delivered to any maker, and nothing about it is
visible in what the nodes run. That sentence is checkable mechanically --- emit
the program for different requests and compare the bytes --- and a claim this
load-bearing should not rest on reading the generator and believing it.

The generator runs in the clear. MP-SPDZ refuses a Python-level branch on a
secret, but nothing stops a generator from branching on something it derived
from the request before it emitted anything, and the result would be a program
whose shape is a function of the request. These tests are what would catch that.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent


def emit(**overrides) -> tuple[str, dict[str, int], dict]:
    """The program's digest, each party's share count, and the reference."""
    out = Path(tempfile.mkdtemp(prefix="qomm-oblivious-"))
    args = ["--n-mm", "8", "--n-parties", "7", "--n-assets", "4",
            "--out-program", str(out / "q.mpc"),
            "--out-input-dir", str(out / "in"),
            "--out-reference", str(out / "ref.json")]
    for key, value in overrides.items():
        args += [f"--{key.replace('_', '-')}", str(value)]
    run = subprocess.run([sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"), *args],
                         capture_output=True, text=True)
    assert run.returncode == 0, run.stderr[-800:]
    digest = hashlib.sha256((out / "q.mpc").read_bytes()).hexdigest()
    counts = {p.name: len(p.read_text().split())
              for p in sorted((out / "in").glob("Input-P*"))}
    return digest, counts, json.loads((out / "ref.json").read_text())


BASE = dict(user_asset=0, user_qty=100, user_dir=0, is_real=1)


@pytest.mark.parametrize("what,change", [
    ("the market asked about", dict(user_asset=3)),
    ("the size", dict(user_qty=997)),
    ("the direction", dict(user_dir=1)),
    ("whether the request is real or cover", dict(is_real=0)),
    ("a size no maker can fill", dict(user_qty=10_000_000)),
    ("the makers' policies", dict(seed=99)),
])
def test_the_program_does_not_depend_on(what: str, change: dict) -> None:
    baseline, _, _ = emit(**BASE)
    altered, _, _ = emit(**{**BASE, **change})
    assert altered == baseline, (
        f"changing {what} changed the program the nodes run. A node that "
        f"compiles the circuit would learn it from the bytes alone.")


@pytest.mark.parametrize("change", [
    dict(user_asset=3), dict(user_qty=997), dict(user_dir=1), dict(is_real=0),
    dict(user_qty=10_000_000), dict(seed=99),
])
def test_every_node_reads_the_same_number_of_values_whatever_is_asked(change: dict) -> None:
    """A share count that moved with the request would leak it to each node
    before a single round ran."""
    _, baseline, _ = emit(**BASE)
    _, altered, _ = emit(**{**BASE, **change})
    assert altered == baseline
    assert len(set(baseline.values())) == 1, \
        "the parties do not all read the same number of values"


def test_a_request_nobody_can_fill_runs_the_same_program() -> None:
    """The case worth singling out.

    If no maker is eligible --- the size is beyond every maker's limit --- the
    protocol must not take a different path. Behaving differently there would
    say "nobody could quote you", which is a statement about the size, and the
    size is the thing being hidden.
    """
    baseline, base_counts, base_ref = emit(**BASE)
    unfillable, counts, ref = emit(**{**BASE, "user_qty": 10_000_000})
    assert unfillable == baseline
    assert counts == base_counts
    # the reference says the two cases really are different, so the test above
    # is comparing a filled request against an unfilled one rather than two
    # requests that happen to be alike
    assert base_ref["eligible_count"] > 0
    assert ref["eligible_count"] == 0 or ref.get("no_eligible_maker")


def test_a_withdrawn_maker_never_quotes_however_the_gates_are_configured() -> None:
    """`--audit-gates` drops what the audit proves, and it dropped one it does not.

    The flag existed to stop the circuit paying for facts the registration audit
    already established. For the expiry that is true --- the auditor refuses an
    audit unless `now < expiry <= now + horizon`. For the active flag it was
    not: the audit proves the flag is a bit and never that it is set, and it
    could not usefully prove it is set, because a committed one is a public one
    and whether a maker is quoting at all is what the commitment hides.

    So with the flag on, a maker that had withdrawn still won tournaments. It
    stays in the circuit now, which costs one multiplication in a layer that
    already has several and no rounds at all.
    """
    import json
    import tempfile

    def eligible(flags: tuple[str, ...], active: int) -> int:
        out = Path(tempfile.mkdtemp(prefix="qomm-gates-"))
        policies = [{"asset": 0, "mid": 10, "half": 5, "slope": 1, "invcoef": 0,
                     "inv": 0, "maxqty": 900, "expiry": 10 ** 9,
                     "active": active, "use_ref": 1} for _ in range(4)]
        (out / "p.json").write_text(json.dumps(policies))
        run = subprocess.run(
            [sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"),
             "--n-mm", "4", "--policies", str(out / "p.json"),
             "--out-program", str(out / "q.mpc"),
             "--out-input-dir", str(out / "in"),
             "--out-reference", str(out / "r.json"), *flags],
            capture_output=True, text=True)
        assert run.returncode == 0, run.stderr[-500:]
        return json.loads((out / "r.json").read_text())["eligible_count"]

    for flags in ((), ("--audit-gates",)):
        assert eligible(flags, active=1) == 4
        assert eligible(flags, active=0) == 0, \
            f"a withdrawn maker was eligible with flags {flags}"
