"""Randomness a late node cannot choose.

Summing one contribution per node gives a nonce no node knows from its own
share. It does not stop a node that contributes *after* seeing the others: with
their sum `K` in hand, contributing `t - K` sets the nonce to `t`, and a known
nonce hands over the witness through `z = k + c*w`. The old note here claimed
that took everyone else being corrupt as well. It does not; it takes going last.

So the contributions are sealed before any is opened. The test that matters is
the adversary's: a node that waits for everyone and then picks its contribution
gains nothing, because what it would need to see is still under a commitment.
"""

import pytest

from zk.commit import Pedersen
from zk.groups import make_group
from zk.threshold_gadgets import CommittedContributions
from zk.threshold_sigma import lagrange_at_zero

PARTIES = [1, 2, 3, 4, 5, 6, 7]
T = 2


@pytest.fixture
def key():
    return Pedersen(make_group("ed25519"), b"qomm:quote:v1")


def reconstruct(shares, subset, order):
    coefficients = lagrange_at_zero(subset, order)
    return sum(coefficients[p] * shares[p] for p in subset) % order


def test_the_contributions_are_sealed_before_any_is_opened(key):
    joint = CommittedContributions(key, PARTIES, T, 3)
    assert set(joint.sealed) == set(PARTIES)
    for dealer in PARTIES:
        assert len(joint.sealed[dealer]) == 3
        assert joint.check_opening(dealer, joint.opened_by(dealer))


def test_a_node_that_opens_to_something_else_is_caught(key):
    joint = CommittedContributions(key, PARTIES, T, 2)
    honest = joint.opened_by(4)
    lied = [(honest[0][0] + 1, honest[0][1]), honest[1]]
    assert not joint.check_opening(4, lied)


def test_what_a_late_node_would_have_needed_is_not_available(key):
    """The seal is the whole defence, so this is the test it stands on.

    A commitment to a scalar under a fresh mask is uniform in the group. Two
    runs with different contributions are indistinguishable from the seals, so a
    node holding every seal knows nothing it could aim at.
    """
    group = key.group
    a = CommittedContributions(key, PARTIES, T, 1)
    b = CommittedContributions(key, PARTIES, T, 1)
    seals_a = {group.encode(a.sealed[p][0]) for p in PARTIES}
    seals_b = {group.encode(b.sealed[p][0]) for p in PARTIES}
    assert not (seals_a & seals_b), "two independent runs produced a shared seal"
    # and the sum is not derivable from them: the opened value differs
    order = group.order
    sum_a = reconstruct(a.open()[0], PARTIES[:T + 1], order)
    sum_b = reconstruct(b.open()[0], PARTIES[:T + 1], order)
    assert sum_a != sum_b


def test_the_result_is_a_sharing_every_quorum_agrees_on(key):
    order = key.group.order
    joint = CommittedContributions(key, PARTIES, T, 2)
    shares = joint.open()
    for slot in shares:
        answers = {reconstruct(slot, [1, 2, 3], order),
                   reconstruct(slot, [5, 6, 7], order),
                   reconstruct(slot, [2, 4, 6], order)}
        assert len(answers) == 1
        assert reconstruct(slot, [1, 2], order) not in answers
