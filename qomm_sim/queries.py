"""Questions an asker pays for, instead of statistics somebody decided to publish.

The disclosure this replaces publishes four sums every window: request count,
volume, signed volume, fills. Three things went wrong with that shape and all of
them are structural rather than fixable by tuning.

**The sums have the wrong sensitivity.** Removing one firm moves a volume sum by
its whole cap, so the noise is `cap / epsilon` --- 1200 at the audited budget,
against a median true imbalance of 428. A *count of firms* moves by 1, because a
firm contributes 0 or 1 to it. Same epsilon, 300x less noise.

**Somebody has to choose what to publish.** A threshold statement needs a `K` and
a `V`; a band needs a centre. In this design that somebody is not defined: the
seven node operators are chosen for their independence and must not be the ones
setting market parameters, and no other role owns it. An asker naming their own
range needs no such person.

**Everyone pays for a release nobody asked for.** Charging every enrolled firm
every window is what a data-independent schedule costs, and it buys a fixed
number of windows --- 40 --- after which the feed is silent forever. Charging the
*asker* removes the schedule: a new entrant spends its own budget locating the
market and then stops, and nobody else's is touched.

**Refusing is safe here, and that is not obvious.** The release mechanism could
not refuse on budget without leaking, because the budget it checked belonged to
the firms *in the data*. An asker's budget is a function of the asker's own past
questions, which the asker already knows and the makers' data does not affect.
So "you are out of budget" says nothing about the market.

What this does not do is bound anything by itself. Epsilon spent is epsilon
spent, and a well-funded asker buys as much of it as it is sold. The ceiling has
to be a ceiling; pricing decides who gets what is under it.
"""

from __future__ import annotations

import random
from dataclasses import dataclass
from typing import Sequence

from .disclosure import EntityAccountant, discrete_laplace


@dataclass(frozen=True)
class RangeQuery:
    """How many eligible makers quoted inside `[low, high]`.

    The bounds are the asker's own and are public: they are the question. What
    is secret is the answer, and the makers' quotes it is computed from.

    `snapshot` names which record to answer against. Answering against the live
    book hands whoever asked the current market, which is what someone trading
    against it would pay for; answering against an older one is still what a new
    entrant needs --- the measured drift over 24 s is 1.01x the dispersion the
    market already carries inside one block --- and worth much less to anyone
    else. Zero is the newest, and larger numbers are further back.
    """

    low: int
    high: int
    snapshot: int = 0

    def __post_init__(self):
        if self.high < self.low:
            raise ValueError(f"an empty range: [{self.low}, {self.high}]")
        if self.snapshot < 0:
            raise ValueError("a snapshot index counts backwards from the newest")


@dataclass
class Answer:
    count: int | None
    epsilon_spent: float
    refused: str | None = None
    #: Which windows a block-range answer was actually computed over, as an
    #: inclusive pair. The asker names blocks; the ledger has window
    #: granularity, so the two need not agree and the answer says which it got.
    #: `None` for queries that are not over a range of time.
    covered: tuple[int, int] | None = None


#: One firm is inside a range or it is not, so removing it moves the count by
#: one. This is the whole reason the query shape is cheaper than the sums.
SENSITIVITY = 1


def answer_range_query(quotes: Sequence[int], query: RangeQuery, epsilon: float,
                       asker: EntityAccountant, rng: random.Random,
                       eligible: Sequence[bool] | None = None) -> Answer:
    """Count the eligible makers inside the range, with noise, charged to `asker`.

    One firm, one contribution: `quotes` is one quote per maker, so a maker that
    is not eligible or not inside adds nothing and no maker adds more than one.
    """
    if not asker.can_spend(epsilon):
        # Safe to say out loud: it is the asker's own budget, not the market's.
        return Answer(None, 0.0, "the asker's query budget is spent")
    asker.spend(epsilon)

    if eligible is None:
        eligible = [True] * len(quotes)
    if len(eligible) != len(quotes):
        raise ValueError("one eligibility flag per quote")
    true_count = sum(1 for q, ok in zip(quotes, eligible)
                     if ok and query.low <= q <= query.high)
    noisy = true_count + discrete_laplace(epsilon, SENSITIVITY, rng)
    return Answer(max(0, noisy), epsilon)


def noise_scale(epsilon: float) -> float:
    """What a query's answer is uncertain by, in firms."""
    return SENSITIVITY / epsilon


def questions_affordable(total: float, epsilon: float) -> int:
    """How many questions a budget of `total` buys at `epsilon` each."""
    if epsilon <= 0:
        raise ValueError("a question at zero epsilon is a question that is free")
    return int(total // epsilon)


# --- what a question costs --------------------------------------------------

@dataclass(frozen=True)
class Pricing:
    """What an asker pays, rising with how much it has already learned.

    A flat price per question is the wrong shape. Locating the market takes a
    handful of coarse questions and is the thing this exists for; reconstructing
    a maker's book takes many fine ones and is the thing it exists to make
    unattractive. Both cost the same epsilon per question, so the price has to do
    the separating, and it has to do it by *how much has been learned* rather
    than by how many times someone asked.

    The total to have spent `e` is `base * (exp(k*e) - 1)`, so one question
    costs the difference between its endpoints. Two properties follow and both
    matter:

    **Splitting a question does not change its price.** The cost depends only on
    the endpoints, so asking for `e` in one go or in ten costs the same. A price
    that did not have this would be a price on the asking rather than on the
    learning, and the first thing anyone would do is slice.

    **The cap is not a price.** Beyond `epsilon_max` nothing is for sale at any
    figure. Pricing decides who gets what is under the ceiling; it cannot be the
    ceiling, because epsilon spent is epsilon spent however much was paid for it.
    """

    base: float = 1.0
    steepness: float = 0.5
    epsilon_max: float = 40.0

    def total_for(self, epsilon: float) -> float:
        """What it costs to have spent `epsilon` in all."""
        if epsilon < 0:
            raise ValueError("negative epsilon is not a purchase")
        if epsilon > self.epsilon_max:
            raise ValueError(
                f"{epsilon} is past the ceiling of {self.epsilon_max}; the "
                "ceiling is not for sale")
        import math
        return self.base * (math.expm1(self.steepness * epsilon))

    def price(self, spent: float, epsilon: float) -> float:
        """What one more question costs an asker that has already spent `spent`."""
        return self.total_for(spent + epsilon) - self.total_for(spent)

    def affordable(self, spent: float, purse: float, epsilon: float) -> bool:
        try:
            return self.price(spent, epsilon) <= purse
        except ValueError:
            return False


# --- asking about a stretch of time rather than a stretch of price ----------
#
# The price band above needs a centre, and the centre needs somebody to choose
# it. A block range needs nobody: the asker names two block heights, both
# public, and the window boundaries they snap to are a published schedule. That
# removes the last of the three defects at the top of this file without adding
# a role the node operators are not allowed to hold.
#
# What it must NOT be asked about is the fill count. That one is already exact
# and public: the stated adversary "sees what a settlement layer publishes",
# and settlement is one on-chain instruction per trade at 18.2M-65.7M gas, so
# anyone with a node counts them without a wallet-to-entity map. Selling a
# noised version protects nothing --- the asker can difference it against the
# chain --- and hands an honest asker a worse answer than the free one. The
# secret half is the REQUEST count: a request that settles nothing leaves no
# on-chain trace, which is exactly what the unsettled-request attack falling to
# AUC 0.500 means. So the venue holds the denominator, and only the denominator.


@dataclass(frozen=True)
class BlockRangeQuery:
    """How many distinct entities asked for a quote between two block heights.

    Distinct *entities*, not requests, and the difference is the whole design.
    An entity contributes 0 or 1 to this count however many requests it made
    and however many windows the range spans, so the sensitivity is 1 for every
    range. A count of request events has sensitivity `cap x windows_covered`
    and grows as the asker widens the question; this one does not. Widening
    therefore trades resolution for accuracy in one direction only, which is
    the shape a new maker locating a venue actually wants.

    The bounds are in the ledger's own step index, which is what a block height
    indexes in deployment. They are the question, so they are public; the
    answer is not.
    """

    from_block: int
    to_block: int

    def __post_init__(self):
        if self.to_block < self.from_block:
            raise ValueError(
                f"an empty range: [{self.from_block}, {self.to_block}]")


#: How far behind the present a range has to end. Answering about the block
#: still being built hands over the live market, which is what someone trading
#: against it would pay for rather than what someone deciding whether to join
#: needs. This is the block-range analogue of `RangeQuery.snapshot`.
DEFAULT_SETTLEMENT_LAG = 1_200


def event_count_sensitivity(windows_covered: int, cap: int) -> int:
    """What counting request *events* over a range would cost, for comparison.

    Here so the claim in `BlockRangeQuery` is checkable rather than asserted:
    one entity may contribute up to `cap` in each window it appears in.
    """
    if windows_covered < 0 or cap < 0:
        raise ValueError("a negative range or a negative cap is not a question")
    return windows_covered * cap


def windows_in_range(windows, query: BlockRangeQuery):
    """The windows lying wholly inside the range, newest last.

    Wholly inside, because a partly covered window would make the answer's
    support depend on where the asker put the boundary, and two askers naming
    boundaries inside the same window would be answering about different data
    while paying the same epsilon.
    """
    return [w for w in windows
            if w.start_step >= query.from_block and w.end_step <= query.to_block]


def answer_block_range_query(windows, query: BlockRangeQuery, epsilon: float,
                             asker: EntityAccountant, rng: random.Random,
                             now: int | None = None,
                             lag: int = DEFAULT_SETTLEMENT_LAG) -> Answer:
    """Count the distinct entities that asked inside the range, with noise.

    Every refusal here is a function of the question and the public schedule
    --- the asker's own budget, the range's width against the window grid, the
    lag against the clock --- and none of them of the market. That is what makes
    refusing safe, and it is the property the schedule-driven release could not
    have.
    """
    if now is not None and query.to_block > now - lag:
        return Answer(None, 0.0,
                      f"the range ends inside the last {lag} blocks",
                      None)
    if not asker.can_spend(epsilon):
        return Answer(None, 0.0, "the asker's query budget is spent", None)

    covered = windows_in_range(windows, query)
    if not covered:
        # Public: the window grid is published, so this says nothing about the
        # data. Charge nothing --- there was no answer to protect.
        return Answer(None, 0.0,
                      "no whole window lies inside the range", None)

    asker.spend(epsilon)
    entities = set()
    for window in covered:
        entities.update(window.requests_by_entity)
    noisy = len(entities) + discrete_laplace(epsilon, SENSITIVITY, rng)
    return Answer(max(0, noisy), epsilon,
                  None, (covered[0].window, covered[-1].window))


def expected_distinct(enrolled: int, appearance_rate: float,
                      windows_covered: int) -> float:
    """`N(1 - exp(-lambda W))` --- what the true count tends to.

    The reason a wider range is not simply better. The count is bounded by the
    enrolment, which is public, so once nearly every entity has appeared the
    answer stops carrying anything and the asker is paying epsilon for a
    figure it already knew. Somebody asking has to be told where that is.
    """
    import math
    if enrolled < 0 or appearance_rate < 0 or windows_covered < 0:
        raise ValueError("negative enrolment, rate or span")
    return enrolled * (-math.expm1(-appearance_rate * windows_covered))


def informative_span(enrolled: int, appearance_rate: float, epsilon: float,
                     saturation: float = 0.95) -> tuple[int, int]:
    """The range widths worth buying, as an inclusive pair of window counts.

    The lower end is where the true count clears the noise; the upper is where
    it reaches `saturation` of the enrolment and stops moving. An empty band
    --- upper below lower --- means this venue has no width at which the
    question is worth its epsilon, and that is a fact about the venue.
    """
    import math
    if appearance_rate <= 0:
        raise ValueError("an entity that never appears has no span")
    floor = noise_scale(epsilon)
    lower = 1
    while lower < 10_000 and expected_distinct(enrolled, appearance_rate, lower) < floor:
        lower += 1
    upper = math.ceil(-math.log1p(-saturation) / appearance_rate)
    return lower, upper
