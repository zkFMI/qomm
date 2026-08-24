import pytest

from qomm_identity.behavior import (BehaviorProfile, BehaviorScreen,
                                    calibrate_threshold)


def profile(name, time=(8, 2), size=(7, 3), instruments=(9, 1),
            buy=0.6, mean_size=4.0, interarrival=6.0, events=200):
    return BehaviorProfile(name, time, size, instruments, buy, mean_size,
                           interarrival, events)


def test_behavior_only_creates_review_candidates_and_never_merges_kyc():
    screen = BehaviorScreen()
    left = profile("left")
    similar = profile("similar", time=(80, 20), size=(70, 30),
                      instruments=(90, 10), buy=0.61)
    different = profile("different", time=(1, 9), size=(1, 9),
                        instruments=(1, 9), buy=0.1, mean_size=8,
                        interarrival=10)
    candidates = screen.candidates(
        [left, similar, different],
        {"left": "legal-a", "similar": "legal-b", "different": "legal-c"},
        threshold=0.8)
    assert [(item.left_credential, item.right_credential)
            for item in candidates] == [("left", "similar")]
    assert candidates[0].disposition == "review_required"
    assert candidates[0].automatic_action is False

    # An authoritative KYC match is not re-alleged by behavior.
    assert screen.candidates(
        [left, similar], {"left": "legal-a", "similar": "legal-a"}, 0.0) == []


def test_sparse_profiles_shrink_toward_uncertainty():
    screen = BehaviorScreen()
    dense = screen.compare(profile("a"), profile("b"))
    sparse = screen.compare(profile("a", events=1), profile("b", events=1))
    assert dense.score > sparse.score
    assert sparse.score == pytest.approx(0.55, abs=0.01)


def test_calibration_obeys_false_positive_cap():
    labeled = [(0.99, 1), (0.95, 1), (0.91, 1),
               (0.90, 0), (0.80, 0), (0.70, 0), (0.60, 0)]
    threshold = calibrate_threshold(labeled, max_false_positive_rate=0.0)
    assert threshold == 0.91
    assert all(score < threshold for score, label in labeled if label == 0)


def test_profile_validation_fails_closed():
    with pytest.raises(ValueError, match="histogram"):
        profile("bad", time=(0, 0)).normalized()
