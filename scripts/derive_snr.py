#!/usr/bin/env python3
"""Where the disclosure signal-to-noise ratio comes from, and what moves it.

The paper argues that widening the disclosure window is a lever for about four
minutes and then stops being one, and that the quantity left over is the number
of firms in the aggregate. Both statements are arithmetic on constants that are
already in the engine and the mechanism, so this script derives them rather than
measuring them --- and checks the one number it can check, the noise scale, against
the value the disclosure runs actually used.
"""
from __future__ import annotations

import json
import math
import random
import statistics
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from scripts import hosts  # noqa: E402

from qomm_sim.experiment import DPParams                     # noqa: E402
from qomm_sim.market import SIZE_BUCKETS, SimConfig         # noqa: E402

ART = Path(__file__).resolve().parent.parent / "artifacts"


def main() -> None:
    cfg = SimConfig()
    dp = DPParams()
    cap = dp.volume_cap
    # Four fields share the per-window budget, so each gets a quarter of it.
    eps_field = dp.epsilon_per_window / 4
    noise = cap / eps_field

    mean_size = sum(w * (lo + hi) / 2
                    for w, (lo, hi) in zip((0.55, 0.33, 0.12), SIZE_BUCKETS))
    per_firm_per_s = cfg.arrival_rate * (1000 / cfg.step_ms) / cfg.n_entities

    # A firm's signed volume is a near-balanced sum, so it grows as the square
    # root of its trade count and stops being reported once it reaches the cap.
    t_star = (cap / mean_size) ** 2 / per_firm_per_s

    # The measured figure this is compared against is a median, so the ceiling
    # has to be one too: for a sum of N clipped contributions the median of the
    # absolute value is 0.674 standard deviations, against 0.798 for the mean.
    # Reporting the mean against a measured median overstates the ceiling by 18%.
    MEDIAN_OVER_SIGMA = 0.6745

    def ceiling(n: int) -> float:
        return MEDIAN_OVER_SIGMA * eps_field * math.sqrt(n)

    # N is firms **in one window**, not firms on the venue.
    #
    # The noise is drawn per window, so the sum it hides is the sum over the
    # firms that contributed to that window. This used to pass in 2,526 --- every
    # distinct address in the whole UniswapX tape --- and report a ceiling of
    # 8.47, which read as "a real venue has more than enough firms". Those 2,526
    # addresses are spread over 2,400 windows. The run's own figures put 4,485
    # requests in those windows, so the average window has 1.87 of them, and no
    # window can have more contributing firms than it has requests.
    #
    # The corrected reading reverses the conclusion: on the busiest basis the
    # tape supports, the ceiling is still below one.
    counts = {
        "generated market, entities configured": cfg.n_entities,
        "UniswapX: requests per window in the run (4485/2400)": 4485 / 2400,
        "UniswapX: distinct swappers per 150-block window, raw tape median": 11,
    }

    print(f"noise scale        = cap/eps_field = {cap}/{eps_field} = {noise:.0f}")
    print(f"clip saturates at  T* = {t_star:.0f} s ({t_star / 60:.1f} min), where"
          f" the standard deviation reaches the cap --- about 32% of firms clip"
          f" there, not all of them")
    print("signal-to-noise ceiling (median basis) = 0.6745 * eps_field * sqrt(N),")
    print("  where N is the firms contributing to ONE window:")
    for label, n in counts.items():
        print(f"  N = {n:8.2f}  {label:60s}  {ceiling(n):6.3f}")
    need = (1.0 / (MEDIAN_OVER_SIGMA * eps_field)) ** 2
    print(f"  SNR = 1 needs N = {need:.1f} firms in the same window")
    print(f"  the whole-tape count 2526 would give {ceiling(2526):.2f}, which is"
          f" what this reported before; it aggregates 2,400 windows into one.")

    # Two cross-checks, because a closed form is only worth quoting if it
    # reproduces something that was not derived from it.
    print()
    print(f"cross-check 1: the disclosure runs used a noise scale of 1200; "
          f"derived here, {noise:.0f}")
    if abs(noise - 1200) > 1:
        raise SystemExit("derived noise scale does not match the measured one")

    # Draw the clipped sums directly rather than trusting the normal
    # approximation, and check the unsaturated case against the measured median
    # imbalance of 428 --- a number this model was not fitted to.
    rng = random.Random(7)

    def drawn_median(n: int, per_firm: float, trials: int = 20_000) -> float:
        return statistics.median(
            abs(sum(max(-cap, min(cap, rng.gauss(0, per_firm))) for _ in range(n)))
            for _ in range(trials))

    saturated = drawn_median(cfg.n_entities, per_firm=10 * cap)
    closed_form = MEDIAN_OVER_SIGMA * cap * math.sqrt(cfg.n_entities)
    print(f"cross-check 2: saturated median imbalance drawn {saturated:.0f} "
          f"against closed form {closed_form:.0f}")
    if abs(saturated - closed_form) > 0.1 * closed_form:
        raise SystemExit("the closed form does not reproduce the drawn sums")

    at_60s = mean_size * math.sqrt(per_firm_per_s * 60)
    predicted = drawn_median(cfg.n_entities, per_firm=at_60s)
    print(f"cross-check 3: at a 60 s window this model predicts a median "
          f"imbalance of {predicted:.0f}; the disclosure runs measured 428")
    if not 0.6 * 428 < predicted < 1.6 * 428:
        raise SystemExit("the model does not reproduce the measured imbalance")

    # A mean is the other case, and it is the one a cold-start maker needs. Its
    # sensitivity carries the observation count in the denominator, so the window
    # that stopped mattering for a sum becomes a linear lever again.
    print()
    print("a mean instead of a sum: noise = k*R/(eps_field*n)")
    n_per_window = cfg.arrival_rate * cfg.window_steps
    means = {
        # field, range one firm's contribution can span
        "winning half-spread (ticks)": 60 - 4,
        "trade size (lots)": SIZE_BUCKETS[-1][1],
        "per-fill markout (ticks)": 2 * cfg.informed_edge_ticks,
    }
    mean_noise: dict[str, dict[str, float]] = {}
    for field, span in means.items():
        row = {}
        for label, mult in (("1 min", 1), ("10 min", 10), ("1 hour", 60)):
            n = n_per_window * mult
            row[label] = dp.request_cap * span / (eps_field * n)
        mean_noise[field] = row
        cells = "  ".join(f"{label} {v:7.3f}" for label, v in row.items())
        print(f"  {field:28s} {cells}")
    print(f"  for scale: half-spreads run 6-18 ticks, break-even at "
          f"{cfg.informed_base * cfg.informed_edge_ticks:.1f}, mean size "
          f"{mean_size:.1f} lots")

    out = ART / "snr_model.json"
    out.write_text(json.dumps({
        # the machine this was taken on, so a reader does not have to ask
        "host": hosts.this_host(),
        "volume_cap": cap, "epsilon_per_window": dp.epsilon_per_window,
        "epsilon_per_field": eps_field, "noise_scale": noise,
        "mean_size_lots": mean_size, "trades_per_firm_per_s": per_firm_per_s,
        "clip_saturation_s": t_star,
        "ceiling": {label: ceiling(n) for label, n in counts.items()},
        "firms_per_window_for_snr_1": (1.0 / (MEDIAN_OVER_SIGMA * eps_field)) ** 2,
        "ceiling_if_whole_tape_pooled": ceiling(2526),
        "firm_counts": counts,
        "mean_field_noise": mean_noise,
    }, indent=1) + "\n")
    print(f"wrote {out.name}")


if __name__ == "__main__":
    main()
