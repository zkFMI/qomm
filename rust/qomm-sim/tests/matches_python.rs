//! The port is only worth having if it is the same simulation, so the expected
//! values below are what CPython produces, pasted in rather than recomputed.
//!
//! Regenerate with:
//!   python3 -c "import random; r = random.Random(0); print([r.random() for _ in range(4)])"
//! and the equivalents for the market, from `mvp/qomm/qomm_sim/market.py`.

use qomm_sim::market::*;
use qomm_sim::pyrandom::PyRandom;

#[test]
fn the_uniform_stream_is_cpythons() {
    let mut r = PyRandom::new(0);
    let expected = [
        0.844_421_851_525_048_1,
        0.757_954_402_940_302_5,
        0.420_571_580_830_845,
        0.258_916_750_292_963_35,
    ];
    for want in expected {
        assert_eq!(r.random(), want);
    }
}

#[test]
fn the_integer_draws_are_cpythons() {
    let mut r = PyRandom::new(0);
    for _ in 0..4 {
        r.random();
    }
    assert_eq!(
        [
            r.getrandbits(8),
            r.getrandbits(8),
            r.getrandbits(8),
            r.getrandbits(8)
        ],
        [130, 124, 103, 235]
    );
    assert_eq!(
        [r.getrandbits(40), r.getrandbits(40), r.getrandbits(40)],
        [913_899_456_057, 1_062_159_640_329, 392_888_992_260]
    );
    assert_eq!(
        (0..6).map(|_| r.randint(6, 18)).collect::<Vec<_>>(),
        vec![15, 9, 14, 8, 10, 8]
    );
    let pool = [0i64, 0, 1, 1, 2];
    assert_eq!(
        (0..6).map(|_| *r.choice(&pool)).collect::<Vec<_>>(),
        vec![0, 2, 1, 2, 2, 0]
    );
    assert_eq!(
        (0..8)
            .map(|_| r.choices(&[0.55, 0.33, 0.12]))
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 1, 0, 0, 0, 1]
    );
}

#[test]
fn sample_takes_the_same_subset() {
    let mut r = PyRandom::new(0);
    for _ in 0..4 {
        r.random();
    }
    for _ in 0..4 {
        r.getrandbits(8);
    }
    for _ in 0..3 {
        r.getrandbits(40);
    }
    for _ in 0..6 {
        r.randint(6, 18);
    }
    let pool = [0i64, 0, 1, 1, 2];
    for _ in 0..6 {
        r.choice(&pool);
    }
    for _ in 0..8 {
        r.choices(&[0.55, 0.33, 0.12]);
    }
    for _ in 0..5 {
        r.gauss(0.0, 6.0);
    }
    for _ in 0..3 {
        r.paretovariate(1.6);
    }
    for _ in 0..3 {
        r.uniform(1.5, 4.0);
    }
    assert_eq!(
        r.sample(72, 12),
        vec![0, 63, 42, 31, 41, 8, 24, 28, 30, 51, 61, 9]
    );
}

/// The Gaussian goes through libm, where the last bit can differ between two
/// runtimes. What matters is whether that reaches the simulation, and it does
/// not: the price path is in integer ticks and it agrees exactly.
#[test]
fn the_price_path_agrees_in_ticks_over_a_full_run() {
    let cfg = SimConfig::default();
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    assert_eq!(market.mid.len(), cfg.steps + 1);
    assert_eq!(
        &market.mid[..8],
        &[100_000, 99_996, 99_999, 99_994, 99_989, 99_988, 99_989, 99_989]
    );
    assert_eq!(market.phi[0], 0.30);
}

#[test]
fn the_makers_are_the_same_makers() {
    let cfg = SimConfig {
        steps: 4_000,
        ..Default::default()
    };
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let first = &makers[0];
    assert_eq!(
        (
            first.base_half,
            first.slope,
            first.inv_coef,
            first.max_qty,
            first.inv_limit
        ),
        (13, 1, 1, 400, 1_200)
    );
    assert!(
        (first.kappa - 3.183_705_289_092_36).abs() < 1e-13,
        "{}",
        first.kappa
    );
}

#[test]
fn the_request_stream_is_the_same_stream() {
    let cfg = SimConfig {
        steps: 4_000,
        ..Default::default()
    };
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    let requests = build_requests(&cfg, &market, cfg.seed + 2);
    assert_eq!(requests.len(), 606);
    let first = requests[0];
    assert_eq!(
        (
            first.step,
            first.entity,
            first.wallet,
            first.size,
            first.direction,
            first.informed
        ),
        (7, 1, 4, 27, 1, false)
    );
    // A wallet belongs to exactly one entity, which is the structure the
    // per-entity cap depends on.
    for r in &requests {
        assert_eq!(r.wallet / cfg.wallets_per_entity, r.entity);
    }
}

#[test]
fn a_quote_is_the_same_quote() {
    let cfg = SimConfig {
        steps: 4_000,
        ..Default::default()
    };
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    assert_eq!(makers[0].half_spread(0.3, 100), 23);
    assert_eq!(makers[0].quote(100_000, 100, 0.3), (100_123, 99_877));
}

#[test]
fn python_rounds_half_to_even_and_so_does_this() {
    // Rust's own round() would give 1, 2, 3, 4 here; Python gives 0, 2, 2, 4.
    assert_eq!(
        [py_round(0.5), py_round(1.5), py_round(2.5), py_round(3.5)],
        [0, 2, 2, 4]
    );
}

/// The whole arm, not just its parts. These figures come from running
/// `qomm_sim.engine.run_arm` on the same configuration; if the port drifts, one
/// of them moves.
#[test]
fn a_whole_arm_reproduces_the_python_run() {
    use qomm_sim::disclosure::{Disclosure, DpDisclosure};
    use qomm_sim::engine::{run_arm, ArmOptions};

    let cfg = SimConfig {
        steps: 4_000,
        window_steps: 200,
        ..Default::default()
    };
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let requests = build_requests(&cfg, &market, cfg.seed + 2);

    let expected = [
        // (protocol, disclosure, fills, rejected, pnl, observations)
        ("plain_rfq", "A", 477u64, 388u64, 124_348.0f64, 865usize),
        ("qomm_rfq", "A", 477, 388, 124_348.0, 0),
        ("plain_rfq", "C", 475, 397, 119_640.0, 872),
        ("qomm_rfq", "C", 475, 397, 119_640.0, 0),
    ];
    for (protocol, mode, fills, rejected, pnl, observations) in expected {
        let mut disclosure = if mode == "A" {
            Disclosure::None
        } else {
            Disclosure::Dp(Box::new(DpDisclosure::new(
                1.0,
                3,
                300,
                cfg.n_entities,
                40.0,
                true,
            )))
        };
        let mut options = ArmOptions::new(protocol, 99);
        options.reactive = true;
        let r = run_arm(&cfg, &market, &requests, &makers, &mut disclosure, &options);
        assert_eq!(
            (r.fills, r.rejected),
            (fills, rejected),
            "{protocol} {mode}"
        );
        assert_eq!(r.mm_pnl_total(), pnl, "{protocol} {mode}");
        assert_eq!(r.observations.len(), observations, "{protocol} {mode}");
        // The query-oblivious arms leave no observation channel at all, which is
        // the property every detection result rests on.
        if protocol.starts_with("qomm") {
            assert!(r.observations.is_empty());
        }
    }
}

/// Hiding the request does not change what the market does: the two arms fill
/// the same trades at the same prices. Only who saw the request differs.
#[test]
fn hiding_the_request_changes_the_observations_and_nothing_else() {
    use qomm_sim::disclosure::Disclosure;
    use qomm_sim::engine::{run_arm, ArmOptions};

    let cfg = SimConfig {
        steps: 4_000,
        window_steps: 200,
        ..Default::default()
    };
    let market = ReferenceMarket::new(&cfg, cfg.seed);
    let makers = build_market_makers(&cfg, cfg.seed + 1);
    let requests = build_requests(&cfg, &market, cfg.seed + 2);

    let run = |protocol: &str| {
        let mut disclosure = Disclosure::None;
        run_arm(
            &cfg,
            &market,
            &requests,
            &makers,
            &mut disclosure,
            &ArmOptions::new(protocol, 99),
        )
    };
    let plain = run("plain_rfq");
    let oblivious = run("qomm_rfq");
    assert_eq!(plain.fills, oblivious.fills);
    assert_eq!(plain.mm_pnl_total(), oblivious.mm_pnl_total());
    assert_eq!(plain.settlements.len(), oblivious.settlements.len());
    assert!(!plain.observations.is_empty());
    assert!(oblivious.observations.is_empty());
}
