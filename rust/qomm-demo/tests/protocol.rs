use qomm_demo::protocol::{DROPOUT, LIE_INPUT, LIE_OPEN, LIE_PRODUCT, OFFLINE};
use qomm_demo::room::Room;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn round(
    behaviours: &[(usize, &str)],
    input_check: bool,
    seed: u64,
) -> qomm_demo::room::RoundResult {
    let mut room = Room::new(8, 9, 2, input_check, seed).unwrap();
    for (node, behaviour) in behaviours {
        room.set_behaviour(*node, behaviour).unwrap();
    }
    room.run_round().unwrap()
}

#[test]
fn threshold_two_corrects_two_product_liars_and_names_exactly_them() {
    let honest = round(&[], true, 1);
    for liars in [vec![0], vec![0, 4], vec![8]] {
        let behaviours = liars
            .iter()
            .map(|node| (*node, LIE_PRODUCT))
            .collect::<Vec<_>>();
        let result = round(&behaviours, true, 1);
        assert!(!result.aborted);
        assert_eq!(result.named.keys().copied().collect::<Vec<_>>(), liars);
        assert_eq!(result.outcome.price, honest.outcome.price);
        assert_eq!(result.outcome.winner, honest.outcome.winner);
    }
}

#[test]
fn third_liar_and_dropout_cross_the_real_decoder_capacity() {
    let third = round(
        &[(0, LIE_PRODUCT), (4, LIE_PRODUCT), (7, LIE_PRODUCT)],
        true,
        1,
    );
    assert!(third.aborted);
    assert_eq!(third.abort_code, "beyond_capacity");
    assert_eq!(third.abort_fields["capacity"], 2);
    assert!(third.named.is_empty());

    let two = round(&[(0, LIE_PRODUCT), (1, LIE_PRODUCT)], true, 1);
    assert!(!two.aborted);
    let dropout = round(&[(3, DROPOUT), (0, LIE_PRODUCT), (1, LIE_PRODUCT)], true, 1);
    assert!(dropout.aborted);
    assert_eq!(dropout.abort_fields["capacity"], 1);
    assert_eq!(dropout.abort_fields["answered"], 8);
}

#[test]
fn offline_opening_and_input_substitution_have_distinct_fail_closed_results() {
    let offline = round(&[(2, OFFLINE)], true, 1);
    assert!(offline.aborted);
    assert_eq!(offline.abort_code, "absent");
    assert_eq!(offline.reductions, 0);

    let opening = round(&[(1, LIE_OPEN), (5, LIE_OPEN), (6, LIE_OPEN)], true, 1);
    assert!(!opening.aborted);
    assert_eq!(
        opening.named.keys().copied().collect::<Vec<_>>(),
        vec![1, 5, 6]
    );
    assert_eq!(opening.open_capacity, 3);
    assert_eq!(opening.product_capacity, 2);

    let checked = round(&[(5, LIE_INPUT)], true, 1);
    assert!(checked.aborted);
    assert_eq!(checked.abort_code, "commitment");
    assert_eq!(
        checked.rejected.iter().map(|row| row.0).collect::<Vec<_>>(),
        vec![5]
    );
    let unchecked = round(&[(5, LIE_INPUT)], false, 1);
    assert!(!unchecked.aborted);
    assert!(unchecked.named.is_empty() && unchecked.rejected.is_empty());
    assert_ne!(unchecked.outcome.price, round(&[], true, 1).outcome.price);
}

#[test]
fn decoder_never_names_an_innocent_node_and_invalid_threshold_is_refused() {
    let mut rng = StdRng::seed_from_u64(3);
    for _ in 0..40 {
        let count = rng.gen_range(0..=2);
        let mut candidates = (0..9).collect::<Vec<_>>();
        use rand::seq::SliceRandom;
        candidates.shuffle(&mut rng);
        let mut liars = candidates[..count].to_vec();
        liars.sort_unstable();
        let behaviours = liars
            .iter()
            .map(|node| (*node, LIE_PRODUCT))
            .collect::<Vec<_>>();
        let result = round(&behaviours, true, rng.gen());
        assert_eq!(result.named.keys().copied().collect::<Vec<_>>(), liars);
    }
    assert!(Room::new(8, 4, 2, true, 1)
        .err()
        .unwrap()
        .contains("cannot carry"));
}
