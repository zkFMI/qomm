"""Questions an asker pays for, and what that changes.

Three properties, each one a thing the published-statistic shape got wrong.

The count has sensitivity one, where a volume sum has the whole cap --- so the
same epsilon buys an answer that is useful instead of one buried under noise
1200 against a signal of 428.

Nobody decides what is published. The asker names the range, so the `K` and `V`
a threshold statement needs, which no role in this design owns, are not needed.

And refusing on budget is safe, which it was not before. The release mechanism
checked the budget of the firms *in the data*, so "withheld" reported on them
with certainty. An asker's budget is a function of the asker's own questions.
"""

import random

import pytest

from qomm_sim.disclosure import EntityAccountant
from qomm_sim.queries import (RangeQuery, SENSITIVITY, answer_range_query,
                              noise_scale, questions_affordable)

QUOTES = [100_000 + d for d in (-40, -30, -12, -5, 0, 3, 8, 15, 40, 120)]


def asker(total=100.0):
    return EntityAccountant(epsilon_total=total)


# --- what it costs to be wrong ---------------------------------------------

def test_one_firm_moves_the_count_by_one(key=None):
    """The sensitivity claim, checked rather than asserted.

    Every field in the published shape had sensitivity equal to a cap. This one
    is a count of firms, so the worst a firm can do is be in or out.
    """
    rng = random.Random(0)
    a = asker()
    query = RangeQuery(99_960, 100_040)
    inside = sum(1 for q in QUOTES if query.low <= q <= query.high)
    for drop in range(len(QUOTES)):
        without = QUOTES[:drop] + QUOTES[drop + 1:]
        moved = abs(inside - sum(1 for q in without
                                 if query.low <= q <= query.high))
        assert moved <= SENSITIVITY, f"dropping maker {drop} moved it by {moved}"
    assert a.spent == 0.0


def test_the_noise_is_small_enough_to_read(key=None):
    """At the audited budget the sums were noise 1200 against signal 428."""
    assert noise_scale(1.0) == 1.0
    assert noise_scale(0.25) == 4.0
    # 200 answers, and the median should sit on the true count
    rng = random.Random(7)
    query = RangeQuery(99_960, 100_040)
    true = sum(1 for q in QUOTES if query.low <= q <= query.high)
    got = [answer_range_query(QUOTES, query, 1.0, asker(), rng).count
           for _ in range(200)]
    got.sort()
    assert abs(got[len(got) // 2] - true) <= 1, (got[len(got) // 2], true)


# --- who pays ---------------------------------------------------------------

def test_the_asker_pays_and_nobody_else_does():
    rng = random.Random(1)
    new_entrant, incumbent = asker(5.0), asker(5.0)
    for _ in range(3):
        answer_range_query(QUOTES, RangeQuery(0, 10 ** 9), 1.0, new_entrant, rng)
    assert new_entrant.spent == pytest.approx(3.0)
    assert incumbent.spent == 0.0, "a firm that asked nothing was charged"


def test_running_out_says_so_and_says_nothing_about_the_market():
    """The failure the release mechanism had, and why it does not recur.

    There, the budget checked belonged to the firms in the data, so the
    withheld/published bit reported their presence with certainty. Here it is
    the asker's own budget: the same refusal comes back whatever the market is.
    """
    rng = random.Random(2)
    spent = EntityAccountant(epsilon_total=1.0, spent=1.0, releases=1)
    quiet = [1, 2, 3]
    busy = list(range(100_000, 100_050))
    first = answer_range_query(quiet, RangeQuery(0, 10 ** 9), 1.0, spent, rng)
    second = answer_range_query(busy, RangeQuery(0, 10 ** 9), 1.0, spent, rng)
    assert first.count is None and second.count is None
    assert first.refused == second.refused, (
        "the refusal differed with the market, which makes it a disclosure")


def test_a_budget_buys_a_countable_number_of_questions():
    assert questions_affordable(10.0, 1.0) == 10
    assert questions_affordable(10.0, 0.25) == 40
    with pytest.raises(ValueError):
        questions_affordable(10.0, 0.0)


# --- the shape of the question ---------------------------------------------

def test_an_empty_or_backwards_range_is_refused():
    with pytest.raises(ValueError):
        RangeQuery(100, 50)
    with pytest.raises(ValueError):
        RangeQuery(0, 10, snapshot=-1)


def test_only_eligible_makers_are_counted():
    rng = random.Random(3)
    query = RangeQuery(0, 10 ** 9)
    none_eligible = answer_range_query(QUOTES, query, 4.0, asker(), rng,
                                       eligible=[False] * len(QUOTES))
    assert none_eligible.count <= 2, none_eligible.count


def test_the_snapshot_is_part_of_the_question():
    """Answering against the live book is a different disclosure from
    answering against an old one, so which is asked for is explicit."""
    assert RangeQuery(0, 1).snapshot == 0
    assert RangeQuery(0, 1, snapshot=12).snapshot == 12


# --- what a question costs --------------------------------------------------

def test_splitting_a_question_does_not_change_its_price():
    """The property the whole pricing rests on.

    If ten small questions cost less than one large one, everybody asks ten. If
    they cost more, the price is on the asking rather than on the learning. It
    has to be the same, and it is the same because the cost depends only on the
    endpoints.
    """
    from qomm_sim.queries import Pricing
    pricing = Pricing()
    whole = pricing.price(2.0, 4.0)
    pieces, spent = 0.0, 2.0
    for _ in range(8):
        pieces += pricing.price(spent, 0.5)
        spent += 0.5
    assert pieces == pytest.approx(whole, rel=1e-12)


def test_the_price_rises_steeply_with_what_has_been_learned():
    from qomm_sim.queries import Pricing
    pricing = Pricing(base=1.0, steepness=0.5, epsilon_max=40.0)
    locating = pricing.total_for(7.0)          # a new entrant finding the market
    extracting = pricing.total_for(30.0)       # somebody reading the book
    assert extracting / locating > 10_000, (locating, extracting)
    # and the marginal question gets dearer, monotonically
    marginals = [pricing.price(e, 1.0) for e in range(0, 30)]
    assert marginals == sorted(marginals)


def test_the_ceiling_is_not_for_sale():
    from qomm_sim.queries import Pricing
    pricing = Pricing(epsilon_max=10.0)
    assert pricing.affordable(9.0, purse=10 ** 12, epsilon=0.5)
    assert not pricing.affordable(9.0, purse=10 ** 12, epsilon=2.0), (
        "an unlimited purse bought past the ceiling")
    with pytest.raises(ValueError):
        pricing.total_for(10.5)
