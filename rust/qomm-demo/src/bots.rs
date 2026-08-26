//! Automatic demo seats.  They keep an unattended room moving without
//! pretending to be a market simulator.

use crate::model::{Policy, Request, BUY};
use rand::Rng;

pub fn step_maker(policy: &mut Policy, n_assets: usize, rng: &mut impl Rng) {
    policy.ask_level = (policy.ask_level + rng.gen_range(-2..=2)).clamp(-40, 40);
    let pull = i64::from(policy.spread < 16) * 2 - i64::from(policy.spread > 90) * 2;
    policy.spread = (policy.spread + rng.gen_range(-6..=6) + pull).clamp(6, 120);
    policy.inv = (policy.inv + rng.gen_range(-4..=4)).clamp(-120, 120);
    if rng.gen_bool(0.04) {
        policy.active = if policy.active == 0 || rng.gen_bool(0.75) {
            1
        } else {
            0
        };
    }
    if n_assets > 0 && rng.gen_bool(0.05) {
        policy.asset = rng.gen_range(0..n_assets) as i64;
    }
}

pub fn maker_filled(policy: &mut Policy, request: &Request) {
    let moved = (request.qty / 8).max(1);
    policy.inv = (policy.inv
        + if request.direction == BUY {
            moved
        } else {
            -moved
        })
    .clamp(-120, 120);
}

pub fn step_taker(request: &mut Request, n_assets: usize, cover_rate: f64, rng: &mut impl Rng) {
    if n_assets > 0 {
        request.asset = rng.gen_range(0..n_assets) as i64;
    }
    const SIZES: [i64; 8] = [10, 25, 50, 100, 100, 150, 200, 400];
    request.qty = SIZES[rng.gen_range(0..SIZES.len())];
    request.direction = i64::from(rng.gen_bool(0.5));
    request.is_real = i64::from(!rng.gen_bool(cover_rate.clamp(0.0, 1.0)));
}
