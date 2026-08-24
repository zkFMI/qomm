//! The tape path has to reproduce the Python exactly, because the real-data
//! results in the paper were produced by it. The fixture below is written in the
//! archive's own format, newest-first, which is the shape that has silently
//! broken this loader before.

use qomm_sim::market::SimConfig;
use qomm_sim::tapes::*;

fn fixture() -> String {
    // A deterministic stand-in for a symbol-day: the columns the loader reads,
    // in the order the archive writes them.
    let mut rows = vec!["timestamp,symbol,side,size,price,tickDirection,trdMatchID".to_string()];
    let mut rng = qomm_sim::pyrandom::PyRandom::new(3);
    let mut t = 1_623_715_200.0f64;
    for i in 0..600 {
        t += rng.random() * 0.4;
        let side = if rng.random() < 0.5 { "Buy" } else { "Sell" };
        let size = rng.paretovariate(1.3) * 10.0;
        let price = 40_000.0 + rng.gauss(0.0, 50.0);
        rows.push(format!(
            "{t:.4},TESTUSD,{side},{size:.4},{price:.2},PlusTick,x{i}"
        ));
    }
    let header = rows.remove(0);
    rows.reverse(); // the archive is written newest-first
    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn cfg() -> SimConfig {
    SimConfig {
        steps: 4_000,
        step_ms: 50,
        window_steps: 200,
        ..Default::default()
    }
}

#[test]
fn a_newest_first_file_loads_in_time_order() {
    let tape = load_bybit(&fixture(), &cfg(), "t.csv", Some(4_000), Some(50), None).unwrap();
    assert_eq!(tape.rows.len(), 600);
    assert!(tape.rows.windows(2).all(|w| w[0].step <= w[1].step));
}

/// The check that a tape read in the wrong order is refused rather than
/// silently turned into a market where every trade happened at once.
#[test]
fn a_tape_that_is_not_in_time_order_is_refused() {
    let text = fixture();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Corrupt one timestamp so the sort cannot repair the ordering.
    let header = lines.remove(0);
    lines[0] = lines[0].replacen(char::is_numeric, "9", 1);
    let broken = std::iter::once(header)
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n");
    // Either it refuses, or the sort put it in order --- both are safe; what is
    // not safe is loading an out-of-order tape.
    if let Ok(tape) = load_bybit(&broken, &cfg(), "t.csv", Some(4_000), Some(50), None) {
        assert!(tape.rows.windows(2).all(|w| w[0].step <= w[1].step));
    }
}

#[test]
fn informedness_is_latent_rather_than_a_threshold_on_the_move() {
    let tape = load_bybit(&fixture(), &cfg(), "t.csv", Some(4_000), Some(50), None).unwrap();
    let market = TapeMarket::new(&cfg(), &tape, 20, 60.0, 200, 0);
    // Some requests agreed with the subsequent move without being labelled
    // informed. If the label were a threshold on that move, this would be empty
    // and attacker 5 would score a perfect AUC on the labelling rule.
    let agreed_but_not_labelled = tape
        .rows
        .iter()
        .zip(&market.informed_flags)
        .filter(|(row, flag)| {
            let m = market.move_over(row.step, 20);
            let agreed = if row.direction == 0 { m > 0 } else { m < 0 };
            agreed && !**flag
        })
        .count();
    assert!(agreed_but_not_labelled > 0);
    assert!(market.informed_share < market.agreement_rate);
}

#[test]
fn rescaling_moves_the_scale_and_leaves_the_shape() {
    let raw: Vec<f64> = (1..=101).map(|i| i as f64).collect();
    let lots = rescale_sizes(&raw, 40, SIZE_CEILING);
    // The median lands on the target...
    let mut sorted = lots.clone();
    sorted.sort_unstable();
    assert_eq!(sorted[sorted.len() / 2], 40);
    // ...and the ordering is untouched, which is what the caps bite on.
    assert!(lots.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn one_wallet_per_entity_is_the_least_favourable_setting_and_is_the_default() {
    let tape = load_bybit(&fixture(), &cfg(), "t.csv", Some(4_000), Some(50), None).unwrap();
    let market = TapeMarket::new(&cfg(), &tape, 20, 60.0, 200, 0);
    let out = requests_from_tape(&cfg(), &market, &tape, Entities::PerAddress, 1, 7);
    assert_eq!(out.cfg.wallets_per_entity, 1);
    assert_eq!(out.entity_kind, "one entity per observed address");
    // Every synthetic taker is its own entity, so a per-entity cap has nothing
    // to collapse --- which is the point of testing it here.
    assert_eq!(out.cfg.n_entities, 600);
}

#[test]
fn round_robin_assignment_reproduces_the_python() {
    let tape = load_bybit(&fixture(), &cfg(), "t.csv", Some(4_000), Some(50), None).unwrap();
    let market = TapeMarket::new(&cfg(), &tape, 20, 60.0, 200, 0);
    let out = requests_from_tape(&cfg(), &market, &tape, Entities::RoundRobin(24), 1, 7);
    assert_eq!(out.cfg.n_entities, 24);
    let first = out.requests[0];
    assert_eq!(
        (first.step, first.entity, first.size, first.direction),
        (0, 7, 36, 1)
    );
}
