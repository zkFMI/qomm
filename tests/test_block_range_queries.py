"""Asking about a stretch of time, and the half of the question that is free.

The price band needed a centre and nobody in this design is allowed to choose
one. A block range needs nobody: two public block heights against a published
window grid.

The property that makes the shape work is that the count is over *entities*
rather than over request events. An entity contributes 0 or 1 to it however
wide the range, so the noise is flat in the range width; an event count's
sensitivity grows with it. Widening a question therefore only ever trades
resolution for accuracy, which is what somebody deciding whether to join a
venue wants and the opposite of what an event count would give them.

And the fill count is not here on purpose. Settlement is one on-chain
instruction per trade, so the count is exact and public to anyone with a node
--- a noised version would protect nothing and be worse than free. What is
secret is the request count, because a request that settles nothing never
reaches the chain.
"""

import random

import pytest

from qomm_sim.disclosure import EntityAccountant
from qomm_sim.queries import (DEFAULT_SETTLEMENT_LAG, BlockRangeQuery,
                              SENSITIVITY, answer_block_range_query,
                              event_count_sensitivity, expected_distinct,
                              informative_span, noise_scale, windows_in_range)


class FakeWindow:
    """Only the fields a range query reads."""

    def __init__(self, window, start_step, end_step, requests_by_entity):
        self.window = window
        self.start_step = start_step
        self.end_step = end_step
        self.requests_by_entity = requests_by_entity


def ledger(per_window, width=100):
    """One window per entry, `per_window[i]` being that window's entities."""
    return [FakeWindow(i, i * width, i * width + width - 1,
                       {e: 3 for e in entities})
            for i, entities in enumerate(per_window)]


def asker(total=100.0):
    return EntityAccountant(epsilon_total=total)


# --- the property the shape rests on ---------------------------------------

def test_sensitivity_is_flat_in_the_range_width():
    """One entity moves the count by one however many windows it appears in.

    This is the claim that separates it from a count of request events, and it
    is checked at a width where an event count would already be 40x worse.
    """
    windows = ledger([{1, 2, 3}] * 40)
    query = BlockRangeQuery(0, 40 * 100)
    covered = windows_in_range(windows, query)
    assert len(covered) == 40

    with_entity = len({e for w in covered for e in w.requests_by_entity})
    dropped = [FakeWindow(w.window, w.start_step, w.end_step,
                          {k: v for k, v in w.requests_by_entity.items() if k != 2})
               for w in covered]
    without = len({e for w in dropped for e in w.requests_by_entity})
    assert with_entity - without == SENSITIVITY == 1


def test_an_event_count_would_grow_with_the_range():
    """The comparison that makes the choice a choice rather than a habit."""
    assert event_count_sensitivity(1, 3) == 3
    assert event_count_sensitivity(40, 3) == 120
    # Flat against linear is the whole of it.
    assert noise_scale(1.0) == 1.0
    assert event_count_sensitivity(40, 3) / 1.0 == 120.0


# --- what the asker is told about what it got ------------------------------

def test_the_answer_names_the_windows_it_covered():
    """The asker names blocks; the ledger has windows. Answers say which."""
    windows = ledger([{1}, {2}, {3}, {4}])
    # 150..349 wholly contains only window 2 (200..299).
    answer = answer_block_range_query(windows, BlockRangeQuery(150, 349), 1.0,
                                      asker(), random.Random(0))
    assert answer.covered == (2, 2)


def test_a_partly_covered_window_is_not_counted():
    """Otherwise two askers naming boundaries inside one window pay the same
    epsilon for answers about different data."""
    windows = ledger([{1, 2, 3}])
    assert windows_in_range(windows, BlockRangeQuery(0, 98)) == []
    assert len(windows_in_range(windows, BlockRangeQuery(0, 99))) == 1


def test_a_range_with_no_whole_window_is_refused_and_free():
    """The window grid is public, so the refusal reports on the question."""
    windows = ledger([{1, 2}])
    acc = asker()
    answer = answer_block_range_query(windows, BlockRangeQuery(0, 10), 1.0,
                                      acc, random.Random(0))
    assert answer.count is None
    assert answer.epsilon_spent == 0.0
    assert acc.spent == 0.0
    assert "whole window" in answer.refused


# --- every refusal is about the question, never about the market -----------

def test_the_live_range_is_refused():
    """Answering about the block being built hands over the live market."""
    windows = ledger([{1}] * 5)
    answer = answer_block_range_query(
        windows, BlockRangeQuery(0, 500), 1.0, asker(), random.Random(0),
        now=500 + DEFAULT_SETTLEMENT_LAG - 1)
    assert answer.count is None
    assert "last" in answer.refused


def test_refusing_on_budget_is_a_fact_about_the_asker():
    windows = ledger([{1, 2}])
    acc = EntityAccountant(epsilon_total=0.5)
    answer = answer_block_range_query(windows, BlockRangeQuery(0, 99), 1.0,
                                      acc, random.Random(0))
    assert answer.count is None
    assert "asker" in answer.refused


def test_the_market_never_changes_whether_it_answers():
    """Two ledgers that differ in every entity, same question, same reply."""
    quiet = ledger([{1}] * 4)
    busy = ledger([set(range(50))] * 4)
    for windows in (quiet, busy):
        answer = answer_block_range_query(windows, BlockRangeQuery(0, 399),
                                          1.0, asker(), random.Random(0))
        assert answer.refused is None
        assert answer.covered == (0, 3)


# --- the noise is the one that was proved ----------------------------------

def test_the_noise_matches_the_sensitivity_one_scale():
    """Empirical sd against `sqrt(2p)/(1-p)` at p = exp(-epsilon)."""
    import math
    windows = ledger([{1, 2, 3, 4, 5}])
    rng = random.Random(20260824)
    truth = 5
    draws = []
    for _ in range(4_000):
        answer = answer_block_range_query(windows, BlockRangeQuery(0, 99), 1.0,
                                          asker(1e9), rng)
        draws.append(answer.count - truth)
    p = math.exp(-1.0)
    want = math.sqrt(2 * p) / (1 - p)
    got = (sum(d * d for d in draws) / len(draws)) ** 0.5
    # Clamping at zero pulls it in a little; the tolerance covers that.
    assert abs(got - want) < 0.15 * want, f"{got} against {want}"


# --- and the reason a wider range is not simply better ---------------------

def test_the_count_saturates_at_the_enrolment():
    assert expected_distinct(24, 0.2, 0) == 0.0
    assert expected_distinct(24, 0.2, 10_000) == pytest.approx(24.0)
    assert expected_distinct(24, 0.2, 5) < expected_distinct(24, 0.2, 10)


def test_a_venue_can_have_no_width_worth_buying():
    """An enrolment small enough never clears its own noise before it
    saturates. That is a fact about the venue, and the mechanism reports it
    rather than selling into it."""
    lower, upper = informative_span(24, 0.2, 1.0)
    assert lower <= upper
    tiny_lower, tiny_upper = informative_span(2, 0.9, 0.05)
    assert tiny_lower > tiny_upper


def test_an_empty_range_is_not_a_question():
    with pytest.raises(ValueError):
        BlockRangeQuery(500, 499)
