"""Confidence intervals for the sample sizes this work actually has.

Most of the reported effects rest on eight to twenty-four runs. A normal
quantile is the right multiplier when the standard deviation is known, and it is
not known here --- it is estimated from the same handful of runs. At n = 8 the
normal multiplier 1.96 is 17% short of Student's 2.365, which is the difference
between an interval that excludes zero and one that does not for at least one
number this project reports.

Built on the incomplete beta already in `qomm_sim.audit` rather than SciPy, to
keep the analysis a stdlib program like the rest of it.
"""

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from qomm_sim.audit import _beta_ppf  # noqa: E402


def t_critical(n: int, alpha: float = 0.05) -> float:
    """Two-sided Student-t multiplier for a mean of `n` observations.

    Via the beta relation: for T ~ t(v), P(|T| > t) = I_{v/(v+t^2)}(v/2, 1/2),
    so the quantile inverts a beta at `alpha` and solves back for t.
    """
    if n < 2:
        raise ValueError("an interval needs at least two observations")
    v = n - 1
    x = _beta_ppf(alpha, v / 2.0, 0.5)
    return math.sqrt(v * (1.0 / x - 1.0))


def mean_ci(values, alpha: float = 0.05) -> dict:
    """Mean and half-width, with the multiplier the sample size earns."""
    values = list(values)
    n = len(values)
    if n == 0:
        return {"mean": None, "half_width": None, "excludes_zero": None, "n": 0}
    mean = math.fsum(values) / n
    if n < 2:
        return {"mean": mean, "half_width": 0.0, "excludes_zero": None, "n": 1,
                "multiplier": None}
    sd = math.sqrt(math.fsum((v - mean) ** 2 for v in values) / (n - 1))
    t = t_critical(n, alpha)
    half = t * sd / math.sqrt(n)
    return {"mean": mean, "half_width": half, "excludes_zero": abs(mean) > half,
            "n": n, "multiplier": t}


if __name__ == "__main__":
    # against the published table, which is the only way to trust a hand-rolled
    # quantile
    for n, want in [(2, 12.706), (3, 4.303), (5, 2.776), (8, 2.365),
                    (13, 2.179), (21, 2.086), (25, 2.064), (121, 1.980)]:
        got = t_critical(n)
        flag = "ok" if abs(got - want) < 5e-4 else "MISMATCH"
        print(f"n={n:4d}  df={n-1:4d}  t={got:.4f}  table={want:.4f}  {flag}")
