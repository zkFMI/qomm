"""The two worlds the audit compares must be the two worlds the release respects.

The disclosure claims add/remove-one-entity adjacency: two windows are
neighbours when one entity's whole contribution is present in one and absent in
the other. Three of the four released fields are built from per-entity maps and
clipped per entity, so removing an entity moves them by at most a cap.

The fill count was not. It was a bare total, clipped against the *request* sum,
and the audit's `_drop_entity` copied it across unchanged --- so the neighbour
the audit tested was one where an entity's requests vanished while its fills
stayed. Nothing measured the field whose sensitivity was wrong, and the
sensitivity was wrong by a factor that grows with the number of entities.
"""

import random

from qomm_sim.audit import _drop_entity
from qomm_sim.disclosure import (DPDisclosure, EntityAccountant, WindowObservation)


def window(fills_by_entity: dict[int, int], requests_by_entity: dict[int, int]):
    return WindowObservation(
        window=0, start_step=0, end_step=100,
        requests_by_entity=dict(requests_by_entity),
        volume_by_entity={e: 10 for e in requests_by_entity},
        signed_volume_by_entity={e: 0 for e in requests_by_entity},
        fills_by_entity=dict(fills_by_entity),
        fills=sum(fills_by_entity.values()), requests=sum(requests_by_entity.values()),
        no_quote=0, liquidity_lots_in_band=5, makers_in_band=3,
        fills_by_bucket=(1, 1, 1), requests_by_bucket=(1, 1, 1),
    )


def disclosure(entities, cap=3):
    return DPDisclosure(
        epsilon_per_window=1.0, request_cap=cap, volume_cap=1000,
        accountants={e: EntityAccountant(epsilon_total=1000.0) for e in entities})


def clipped_fill_count(d, obs):
    """The quantity the noise is calibrated against, before the noise."""
    return sum(min(c, d.request_cap) for c in obs.fills_by_entity.values())


def test_removing_one_entity_moves_the_fill_count_by_at_most_its_cap():
    # One entity takes every fill in the window while the others only ask. That
    # is an ordinary window --- one maker wins the flow --- and it is the shape
    # that breaks a total clipped against a sum.
    entities = list(range(8))
    requests = {e: 100 for e in entities}
    fills = {0: 100, **{e: 0 for e in entities[1:]}}
    obs = window(fills, requests)
    d = disclosure(entities)

    here = clipped_fill_count(d, obs)
    for victim in entities:
        there = clipped_fill_count(d, _drop_entity(obs, victim))
        assert abs(here - there) <= d.request_cap, (
            f"dropping entity {victim} moved the clipped fill count by "
            f"{abs(here - there)}, past a cap of {d.request_cap}: the noise "
            f"calibrated to that cap is short by a factor of "
            f"{abs(here - there) / d.request_cap:.0f}")


def test_the_audits_neighbour_actually_removes_the_entity_from_every_field():
    obs = window({0: 7, 1: 2}, {0: 9, 1: 5})
    without = _drop_entity(obs, 0)
    assert 0 not in without.fills_by_entity, "the entity's fills survived the removal"
    assert without.fills == 2, (
        f"the neighbour still reports {without.fills} fills after removing an "
        "entity that had seven of them")


def test_the_fill_field_is_not_more_exposed_than_the_request_field():
    # Both are counts capped at the same R, so neither should need more noise
    # than the other --- but only if both are clipped the same way.
    entities = list(range(6))
    obs = window({0: 30, 1: 30}, {e: 30 for e in entities})
    d = disclosure(entities)
    fill_move = max(abs(clipped_fill_count(d, obs)
                        - clipped_fill_count(d, _drop_entity(obs, e))) for e in entities)
    req_here = sum(min(c, d.request_cap) for c in obs.requests_by_entity.values())
    req_move = max(abs(req_here - sum(min(c, d.request_cap) for c in
                                      _drop_entity(obs, e).requests_by_entity.values()))
                   for e in entities)
    assert fill_move <= req_move, (
        f"the fill field moves by {fill_move} where the request field moves by "
        f"{req_move}, and both carry the same noise")


# --- whether a window is published at all ------------------------------------

def test_whether_a_window_is_published_does_not_depend_on_who_was_in_it():
    """The published/withheld bit is a release. It has to be data-independent.

    The mechanism used to charge only the entities that were active in the
    window and withhold when one of *those* was out of budget. That makes the
    bit a report on the private data with certainty: take an entity that spent
    its budget earlier, and the window is withheld exactly when it trades and
    published when it does not. Two neighbouring worlds, probabilities one and
    zero, no finite epsilon.
    """
    rng = random.Random(0)
    entities = [0, 1]
    obs_with = window({0: 1, 1: 1}, {0: 1, 1: 1})
    obs_without = window({1: 1}, {1: 1})

    def published(obs):
        d = disclosure(entities)
        # entity 0 has already spent everything; entity 1 has room
        d.accountants[0] = EntityAccountant(epsilon_total=1.0, spent=1.0, releases=1)
        d.accountants[1] = EntityAccountant(epsilon_total=1000.0)
        return d.release(obs, rng).published

    assert published(obs_with) == published(obs_without), (
        "the window is withheld when entity 0 trades and published when it "
        "does not, so the bit names the entity outright")


def test_the_budget_is_charged_whether_or_not_an_entity_traded():
    """The cost of a data-independent schedule, stated rather than avoided.

    Charging only active entities is cheaper, and it is what made the schedule
    private-data-dependent. So enrolment buys a fixed number of windows and
    sitting out does not save any of them.
    """
    rng = random.Random(0)
    d = disclosure([0, 1])
    d.accountants = {0: EntityAccountant(epsilon_total=1000.0),
                     1: EntityAccountant(epsilon_total=1000.0)}
    # only entity 0 trades
    d.release(window({0: 1}, {0: 1}), rng)
    assert d.accountants[1].releases == 1, (
        "an entity that sat the window out was not charged for it, which is "
        "what makes the schedule depend on who traded")


# --- and the audit has to be able to see it ----------------------------------

def test_the_two_world_game_catches_the_clipping_that_broke_the_fill_field():
    """A fix nothing measures is a fix nobody can check.

    The unit tests above bound the sensitivity by arithmetic. This one runs the
    audit that the paper reports --- the two-world distinguisher --- against the
    same window, on the field that was wrong, and shows both halves: the fixed
    clipping sits inside its claim, and the old one is past it by enough that
    the audit could not have missed it if it had ever been pointed there.
    """
    from qomm_sim import disclosure as module
    from qomm_sim.audit import audit_window

    entities, cap, eps = list(range(8)), 3, 1.0
    obs = window({0: 100, **{e: 0 for e in entities[1:]}}, {e: 100 for e in entities})

    def empirical():
        return audit_window(obs, 0, eps, cap, 300, trials=1500, seed=1,
                            n_entities=len(entities), field="noisy_fills")

    fixed = empirical()
    assert fixed.within_claim, (
        f"the corrected clipping leaks {fixed.empirical_epsilon:.4f} against a "
        f"per-field claim of {fixed.field_epsilon}")

    # The old expression, restored just long enough to be measured.
    def old_release(self, obs, rng):
        for a in self.accountants.values():
            a.spend(self.epsilon_per_window)
        per_field = self.epsilon_per_window / self.n_fields
        clipped = min(obs.fills, sum(min(c, self.request_cap)
                                     for c in obs.requests_by_entity.values()))
        noisy = max(0, clipped + module.discrete_laplace(per_field,
                                                         self.request_cap, rng))
        return module.Release(obs.window, self.name, True,
                              {"noisy_fills": noisy}, self.epsilon_per_window, None)

    original = module.DPDisclosure.release
    module.DPDisclosure.release = old_release
    try:
        broken = empirical()
    finally:
        module.DPDisclosure.release = original

    assert not broken.within_claim, (
        "the audit passes the old clipping too, so it is not evidence about "
        "this field at all")
    assert broken.empirical_epsilon > 4 * fixed.empirical_epsilon, (
        f"old {broken.empirical_epsilon:.4f} vs fixed {fixed.empirical_epsilon:.4f}")
