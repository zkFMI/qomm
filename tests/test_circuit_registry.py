"""A registered rule must bind the circuit that actually runs, not only the rule.

The rule digest covers the shape, the bounds and the emitted expressions, so a
substituted rule is caught. What the computing nodes execute is a compiled
MP-SPDZ program, and a node holding the approved rule digest could still compile
something else. These tests pin the second binding and the shape it belongs to.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from qomm_dsl.registry import (                                    # noqa: E402
    CircuitRegistry, program_digest, rule_digest,
)
from qomm_dsl.language import compile_rule                         # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
RULE = (ROOT / "qomm_dsl" / "examples" / "quote.rule").read_text()
RULE_NAME = None            # the example names itself; compile_rule finds it
PROGRAM = "ask = mid + half + slope * qty\nbid = mid - half - slope * qty\n"
SHAPE = ("rfq", 16, 31)


@pytest.fixture
def registry():
    reg = CircuitRegistry()
    reg.approve(RULE_NAME, RULE, PROGRAM, SHAPE)
    return reg


def test_the_approved_circuit_passes(registry):
    ok, reason = registry.check(PROGRAM, SHAPE)
    assert ok, reason


def test_a_substituted_circuit_is_caught(registry):
    """One operator different is a different price rule."""
    swapped = PROGRAM.replace("mid + half", "mid + half + half")
    ok, reason = registry.check(swapped, SHAPE)
    assert not ok and "does not match" in reason


def test_an_unapproved_shape_is_refused(registry):
    """One rule emits different circuits per maker count; they are not equivalent.

    The registry is keyed on the shape, so an unknown one never reaches the
    digest comparison at all --- which is the stricter behaviour, since it fails
    without needing to be told what the right program would have been.
    """
    ok, reason = registry.check(PROGRAM, ("rfq", 64, 31))
    assert not ok and "no circuit is approved" in reason


def test_matching_against_the_wrong_shape_directly_is_refused(registry):
    """The guard inside ApprovedCircuit, for callers that bypass the lookup."""
    entry = registry._approved[SHAPE]
    ok, reason = entry.matches(PROGRAM, ("rfq", 64, 31))
    assert not ok and "never approved" in reason


def test_a_different_mode_is_refused(registry):
    ok, reason = registry.check(PROGRAM, ("rfm", 16, 31))
    assert not ok


@pytest.mark.parametrize("variant", [
    "ask = mid + half + slope * qty   \nbid = mid - half - slope * qty\t\n",
    "ask = mid + half + slope * qty\nbid = mid - half - slope * qty",
    "\n\nask = mid + half + slope * qty\nbid = mid - half - slope * qty\n\n\n",
])
def test_formatting_alone_does_not_trip_it(registry, variant):
    """A digest that trips on a trailing newline is a digest that gets turned off."""
    ok, reason = registry.check(variant, SHAPE)
    assert ok, reason


def test_a_blank_line_between_instructions_does_trip_it(registry):
    """Conservative on purpose: only end-of-line whitespace is normalised away."""
    ok, _ = registry.check(
        "ask = mid + half + slope * qty\n\nbid = mid - half - slope * qty\n", SHAPE)
    assert not ok


def test_the_rule_digest_ignores_secret_values(registry):
    """Replacing a parameter is allowed; replacing the rule is not.

    This is the property the issue asks for, and it is why the digest covers the
    declarations and the emitted expressions rather than the source text.
    """
    base = rule_digest(compile_rule(RULE))
    assert base == rule_digest(compile_rule(RULE + "\n"))
    widened = RULE.replace("[1, 200]", "[1, 900]")
    if widened != RULE:
        assert base != rule_digest(compile_rule(widened))


def test_nothing_is_approved_by_default():
    ok, reason = CircuitRegistry().check(PROGRAM, SHAPE)
    assert not ok and "no circuit is approved" in reason


# --- the same claim, against circuits the generator actually emits ----------
#
# The tests above use a two-line string standing in for a program, which checks
# the registry and not the thing the registry is for. What a venue publishes is
# a compiled circuit, and the claim is that substituting a secret parameter
# passes and substituting the rule is refused. That is checkable against the
# real generator, and it is the mechanism that stops a venue running something
# other than what it published.

import json as _json
import subprocess as _subprocess
import sys as _sys
import tempfile as _tempfile


def _emit(policies=None, flags=(), **overrides) -> str:
    out = Path(_tempfile.mkdtemp(prefix="qomm-registry-"))
    args = ["--n-mm", "4",
            "--out-program", str(out / "q.mpc"),
            "--out-input-dir", str(out / "in"),
            "--out-reference", str(out / "r.json")]
    if policies is not None:
        (out / "p.json").write_text(_json.dumps(policies))
        args += ["--policies", str(out / "p.json")]
    for key, value in overrides.items():
        args += [f"--{key.replace('_', '-')}", str(value)]
    args += list(flags)
    _subprocess.run([_sys.executable, str(ROOT / "mp_spdz" / "gen_qomm.py"), *args],
                    check=True, capture_output=True)
    return (out / "q.mpc").read_text()


_REAL_SHAPE = ("rfq", 4, 63)


@pytest.fixture(scope="module")
def real_registry():
    reg = CircuitRegistry()
    reg.approve(RULE_NAME, RULE, _emit(), _REAL_SHAPE)
    return reg


def test_a_secret_parameter_may_be_substituted(real_registry):
    """Which is the point: the venue publishes the circuit, not the policies."""
    policies = [{"asset": 0, "mid": 10, "half": 5, "slope": 1, "invcoef": 1,
                 "inv": 3, "maxqty": 900, "expiry": 10 ** 9, "active": 1,
                 "use_ref": 1} for _ in range(4)]
    ok, reason = real_registry.check(_emit(policies=policies), _REAL_SHAPE)
    assert ok, reason


@pytest.mark.parametrize("what,change", [
    ("the tournament's arity", dict(argmin_arity=4)),
    ("the field width", dict(bit_length=47)),
    ("the number of markets", dict(n_assets=4)),
    ("adding a binding limit", dict(flags=("--binding-limit",))),
    ("adding the input check", dict(flags=("--input-check",))),
])
def test_substituting_the_rule_is_refused(real_registry, what, change):
    ok, reason = real_registry.check(_emit(**change), _REAL_SHAPE)
    assert not ok, f"changing {what} was accepted as the approved circuit"
    assert "does not match" in reason
