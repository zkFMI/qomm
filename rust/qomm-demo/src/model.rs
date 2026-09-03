//! Cleartext market model kept byte-for-byte in arithmetic with qomm-mpc.

use serde::{Deserialize, Serialize};

pub const FIELDS: [&str; 10] = [
    "asset",
    "ask_level",
    "spread",
    "slope",
    "invcoef",
    "inv",
    "maxqty",
    "expiry",
    "active",
    "use_ref",
];
pub const BUY: i64 = 0;
pub const SELL: i64 = 1;
/// Stable end of the public demo epoch. Production policies use a
/// governance-issued validity interval instead of this development bound.
pub const DEMO_POLICY_VALID_UNTIL: i64 = 4_102_444_800;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub asset: i64,
    pub ask_level: i64,
    pub spread: i64,
    pub slope: i64,
    pub invcoef: i64,
    pub inv: i64,
    pub maxqty: i64,
    pub expiry: i64,
    pub active: i64,
    pub use_ref: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            asset: 0,
            ask_level: 0,
            spread: 40,
            slope: 1,
            invcoef: 1,
            inv: 0,
            maxqty: 200,
            expiry: DEMO_POLICY_VALID_UNTIL,
            active: 1,
            use_ref: 1,
        }
    }
}

impl Policy {
    pub fn fields(&self) -> [i64; 10] {
        [
            self.asset,
            self.ask_level,
            self.spread,
            self.slope,
            self.invcoef,
            self.inv,
            self.maxqty,
            self.expiry,
            self.active,
            self.use_ref,
        ]
    }

    pub fn from_fields(fields: &[i128]) -> Self {
        let to_i64 = |value: i128| value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        Self {
            asset: to_i64(fields[0]),
            ask_level: to_i64(fields[1]),
            spread: to_i64(fields[2]),
            slope: to_i64(fields[3]),
            invcoef: to_i64(fields[4]),
            inv: to_i64(fields[5]),
            maxqty: to_i64(fields[6]),
            expiry: to_i64(fields[7]),
            active: to_i64(fields[8]),
            use_ref: to_i64(fields[9]),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Request {
    pub asset: i64,
    pub qty: i64,
    pub direction: i64,
    pub entity: i64,
    pub is_real: i64,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            asset: 0,
            qty: 100,
            direction: BUY,
            entity: 0,
            is_real: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Quote {
    pub maker: usize,
    pub ask: i64,
    pub bid: i64,
    pub eligible: bool,
    pub reason: String,
    pub cost: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Outcome {
    pub quotes: Vec<Quote>,
    pub winner: Option<usize>,
    pub price: Option<i64>,
    pub cost: Option<i64>,
    pub eligible: usize,
}

pub fn price_one(policy: &Policy, request: &Request, reference: &[i64]) -> (i64, i64) {
    let reference = usize::try_from(request.asset)
        .ok()
        .and_then(|asset| reference.get(asset))
        .copied()
        .unwrap_or(0);
    let anchor = policy.use_ref * reference + policy.ask_level;
    let depth = policy.slope * request.qty;
    let skew = policy.invcoef * policy.inv;
    (anchor + depth + skew, anchor - policy.spread - depth + skew)
}

pub fn ineligible_reason(policy: &Policy, request: &Request, now: i64) -> String {
    if policy.active == 0 {
        "switched off".into()
    } else if policy.asset != request.asset {
        "different market".into()
    } else if request.qty > policy.maxqty {
        format!("size {} above its limit {}", request.qty, policy.maxqty)
    } else if policy.expiry <= now {
        "policy expired".into()
    } else {
        String::new()
    }
}

pub fn evaluate(policies: &[Policy], request: &Request, reference: &[i64], now: i64) -> Outcome {
    let mut outcome = Outcome::default();
    let mut best = None;
    for (maker, policy) in policies.iter().enumerate() {
        let (ask, bid) = price_one(policy, request, reference);
        let reason = ineligible_reason(policy, request, now);
        let cost = if request.direction == SELL { -bid } else { ask };
        outcome.quotes.push(Quote {
            maker,
            ask,
            bid,
            eligible: reason.is_empty(),
            reason: reason.clone(),
            cost: reason.is_empty().then_some(cost),
        });
        if !reason.is_empty() {
            continue;
        }
        outcome.eligible += 1;
        if best.is_none_or(|current| cost < current) {
            best = Some(cost);
            outcome.winner = Some(maker);
        }
    }
    outcome.cost = best;
    outcome.price = best.map(|cost| {
        if request.direction == SELL {
            -cost
        } else {
            cost
        }
    });
    outcome
}
