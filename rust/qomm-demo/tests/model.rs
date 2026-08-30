use qomm_demo::model::{evaluate, Policy, Request, BUY, SELL};
use qomm_mpc::inputs::{build_inputs, finish_reference, parse_policies, InputConfig};
use qomm_mpc::program::{sentinel_for, CheckMode, Mode, Reference};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::Value;

#[test]
fn demo_prices_exactly_what_the_rust_circuit_generator_prices() {
    let references = [15_750_i64, 10_850, 6_420_000];
    for seed in 0..25_u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let policies = (0..8)
            .map(|_| Policy {
                asset: rng.gen_range(0..3),
                ask_level: rng.gen_range(-15..=15),
                spread: rng.gen_range(5..=40),
                slope: rng.gen_range(0..=3),
                invcoef: rng.gen_range(0..=2),
                inv: rng.gen_range(-50..=50),
                maxqty: [50, 100, 200, 500][rng.gen_range(0..4)],
                expiry: [0, 1_000_000_000][rng.gen_range(0..2)],
                active: [0, 1, 1, 1][rng.gen_range(0..4)],
                use_ref: 1,
            })
            .collect::<Vec<_>>();
        let request = Request {
            asset: rng.gen_range(0..3),
            qty: [10, 50, 100, 200, 400][rng.gen_range(0..5)],
            direction: [BUY, SELL][rng.gen_range(0..2)],
            entity: 0,
            is_real: 1,
        };
        let policy_json = serde_json::to_string(&policies).unwrap();
        let parsed = parse_policies(&policy_json).unwrap();
        let ref_i128 = references.map(i128::from);
        let config = InputConfig {
            n_mm: 8,
            n_real_mm: 8,
            n_parties: 7,
            is_real: 1,
            n_requests: 1,
            n_assets: 3,
            ref_table: &ref_i128,
            user_asset: request.asset as usize,
            user_qty: i128::from(request.qty),
            user_dir: i128::from(request.direction),
            user_entity: 0,
            now_t: 1,
            seed: seed as i128,
            audit_gates: false,
            value_bits: 64,
            field_bits: 128,
            use_ref: 1,
            reference: Reference::Anchored,
            input_check: false,
            check_mode: CheckMode::Aggregate,
            binding_limit: false,
            user_limit: 100_000,
            user_limit_blinding: 1,
            user_qty_blinding: 1,
            check_coefficients: &[],
            check_repeats: 7,
            policies: Some(&parsed),
            shamir_inputs: false,
            shamir_threshold: 2,
            dvp: None,
            quote_proof: None,
        };
        let mut generated = build_inputs(&config).unwrap();
        let sentinel =
            sentinel_for(63, 8, 8 * i128::from(*references.iter().max().unwrap())).unwrap();
        finish_reference(&mut generated, &config, sentinel, Mode::Rfq).unwrap();
        let ours = evaluate(&policies, &request, &references, 1);
        assert_eq!(ours.winner.unwrap_or(0), generated.best_mm(), "seed {seed}");
        assert_eq!(
            ours.price.map(i128::from),
            generated.best_price(),
            "seed {seed}"
        );
        let reference: Value = serde_json::from_str(&generated.reference_json()).unwrap();
        let expected = reference["quotes"].as_array().unwrap();
        assert_eq!(ours.quotes.len(), expected.len());
        for (ours, expected) in ours.quotes.iter().zip(expected) {
            assert_eq!(ours.ask, expected["ask"].as_i64().unwrap());
            assert_eq!(ours.bid, expected["bid"].as_i64().unwrap());
            assert_eq!(ours.eligible, expected["eligible"].as_bool().unwrap());
        }
    }
}

#[test]
fn selling_maximises_bid_and_buying_minimises_ask() {
    let policies = vec![
        Policy {
            ask_level: 10,
            spread: 20,
            slope: 0,
            invcoef: 0,
            inv: 0,
            ..Policy::default()
        },
        Policy {
            ask_level: 30,
            spread: 60,
            slope: 0,
            invcoef: 0,
            inv: 0,
            ..Policy::default()
        },
    ];
    let buying = evaluate(
        &policies,
        &Request {
            qty: 1,
            direction: BUY,
            ..Request::default()
        },
        &[1_000],
        0,
    );
    let selling = evaluate(
        &policies,
        &Request {
            qty: 1,
            direction: SELL,
            ..Request::default()
        },
        &[1_000],
        0,
    );
    assert_eq!((buying.winner, buying.price), (Some(0), Some(1_010)));
    assert_eq!((selling.winner, selling.price), (Some(0), Some(990)));
}
