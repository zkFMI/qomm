use qomm_demo::bots::{maker_filled, step_maker, step_taker};
use qomm_demo::model::{Policy, Request, BUY, SELL};
use rand::rngs::StdRng;
use rand::SeedableRng;

#[test]
fn maker_motion_stays_inside_the_python_demo_bounds() {
    let mut maker = Policy {
        ask_level: 40,
        spread: 120,
        inv: 120,
        ..Policy::default()
    };
    let mut rng = StdRng::seed_from_u64(4);
    for _ in 0..2_000 {
        step_maker(&mut maker, 3, &mut rng);
        assert!((-40..=40).contains(&maker.ask_level));
        assert!((6..=120).contains(&maker.spread));
        assert!((-120..=120).contains(&maker.inv));
        assert!((0..3).contains(&maker.asset));
        assert!(matches!(maker.active, 0 | 1));
    }
}

#[test]
fn a_real_fill_moves_inventory_in_the_direction_the_python_demo_uses() {
    let mut maker = Policy::default();
    maker_filled(
        &mut maker,
        &Request {
            qty: 80,
            direction: BUY,
            ..Request::default()
        },
    );
    assert_eq!(maker.inv, 10);
    maker_filled(
        &mut maker,
        &Request {
            qty: 160,
            direction: SELL,
            ..Request::default()
        },
    );
    assert_eq!(maker.inv, -10);
}

#[test]
fn automatic_taker_emits_real_and_cover_requests_of_the_same_shape() {
    let mut request = Request::default();
    let mut rng = StdRng::seed_from_u64(9);
    let mut kinds = [false; 2];
    for _ in 0..200 {
        step_taker(&mut request, 3, 0.35, &mut rng);
        assert!((0..3).contains(&request.asset));
        assert!([10, 25, 50, 100, 150, 200, 400].contains(&request.qty));
        assert!(matches!(request.direction, BUY | SELL));
        assert!(matches!(request.is_real, 0 | 1));
        kinds[request.is_real as usize] = true;
    }
    assert_eq!(kinds, [true, true]);
}
