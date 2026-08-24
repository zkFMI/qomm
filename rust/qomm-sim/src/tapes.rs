//! Real order flow in place of the generated kind.
//!
//! `market` draws everything: a Gaussian walk, Poisson arrivals, Pareto entity
//! activity. One rejection criterion written down before any measurement was
//! whether the results survive a different data-generating rule, and the honest
//! way to settle that is to stop generating.
//!
//! Two tapes, because no single source carries everything. *UniswapX* is
//! request-for-quote on chain: a fill names the swapper who asked and the filler
//! who won, which is the pair the simulator generates, and the venue is
//! genuinely thin. *Bybit* is a perpetual-futures tape with no identities but
//! the density the simulator assumes and a wide spread in liquidity between
//! symbols.
//!
//! What neither supplies is a maker's pricing rule, so makers stay generated:
//! real policies are unobservable, and they are the design space being swept.

use std::collections::{BTreeMap, VecDeque};

use crate::market::{py_round, Request, SimConfig};
use crate::pyrandom::PyRandom;

pub const SIZE_CEILING: i64 = 100_000;

/// One market's history, already on the simulator's step grid.
#[derive(Clone, Debug)]
pub struct Tape {
    /// Reference price per step, in ticks.
    pub mid: Vec<i64>,
    pub rows: Vec<TapeRow>,
    pub source: String,
    pub meta: BTreeMap<String, f64>,
}

#[derive(Clone, Debug)]
pub struct TapeRow {
    pub step: usize,
    pub address: String,
    pub size: i64,
    pub direction: u8,
}

impl Tape {
    pub fn steps(&self) -> usize {
        self.mid.len() - 1
    }
}

/// A market whose price path and informed fraction are measured rather than drawn.
///
/// A tape has no informedness flag --- nothing in a trade record says whether the
/// trader knew something --- so it is estimated, and the estimate stays *latent*.
/// That matters more than it looks: labelling a request informed whenever its
/// signed move cleared a threshold makes the label a deterministic function of
/// exactly the quantity attacker 5 scores on, and that attacker then reports an
/// AUC of 1.0 --- a measurement of the labelling rule, not of the attack. So
/// informedness is assigned by draw among the requests that agreed, at a rate
/// reproducing the estimated share, which is the structure the generated arm has.
pub struct TapeMarket {
    pub mid: Vec<i64>,
    pub phi: Vec<f64>,
    pub source: String,
    pub horizon: usize,
    pub agreement_rate: f64,
    pub informed_share: f64,
    pub informed_flags: Vec<bool>,
    pub edge: i64,
    pub measured_phi: Option<f64>,
}

impl TapeMarket {
    pub fn new(
        cfg: &SimConfig,
        tape: &Tape,
        horizon: usize,
        edge_percentile: f64,
        phi_window: usize,
        seed: u64,
    ) -> Self {
        let mid = tape.mid.clone();
        let move_over = |step: usize, horizon: usize| -> i64 {
            let end = (step + horizon).min(mid.len() - 1);
            mid[end] - mid[step]
        };
        let mut rng = PyRandom::new(seed);

        let agreements: Vec<bool> = tape
            .rows
            .iter()
            .map(|row| {
                let m = move_over(row.step, horizon);
                if row.direction == 0 {
                    m > 0
                } else {
                    m < 0
                }
            })
            .collect();
        let rate = if agreements.is_empty() {
            0.5
        } else {
            agreements.iter().filter(|a| **a).count() as f64 / agreements.len() as f64
        };
        // uninformed flow agrees half the time; the excess is the informed share
        let informed_share = (2.0 * rate - 1.0).clamp(0.0, 1.0);
        let mark = if rate > 0.0 {
            informed_share / rate
        } else {
            0.0
        };

        let mut informed_flags = Vec::with_capacity(tape.rows.len());
        let mut labels: Vec<(usize, bool)> = Vec::with_capacity(tape.rows.len());
        for (row, agreed) in tape.rows.iter().zip(&agreements) {
            let flag = *agreed && rng.random() < mark;
            informed_flags.push(flag);
            labels.push((row.step, flag));
        }

        // Kept for reporting only; the size of a move no longer gates the label.
        let mut moves: Vec<i64> = tape
            .rows
            .iter()
            .map(|r| move_over(r.step, horizon).abs())
            .collect();
        moves.sort_unstable();
        let edge = if moves.is_empty() {
            1
        } else {
            let index =
                ((moves.len() as f64 * edge_percentile / 100.0) as usize).min(moves.len() - 1);
            moves[index].max(1)
        };

        // phi[t] is the share of informed requests in the trailing window, held
        // flat between arrivals because there is nothing to update in between.
        let mut phi = vec![cfg.informed_base; mid.len()];
        let mut recent: VecDeque<bool> = VecDeque::with_capacity(phi_window);
        let mut cursor = 0usize;
        for (step, slot) in phi.iter_mut().enumerate() {
            while cursor < labels.len() && labels[cursor].0 <= step {
                if recent.len() == phi_window {
                    recent.pop_front();
                }
                recent.push_back(labels[cursor].1);
                cursor += 1;
            }
            if !recent.is_empty() {
                *slot = recent.iter().filter(|f| **f).count() as f64 / recent.len() as f64;
            }
        }
        let measured_phi = median(&phi);

        TapeMarket {
            mid,
            phi,
            source: tape.source.clone(),
            horizon,
            agreement_rate: rate,
            informed_share,
            informed_flags,
            edge,
            measured_phi,
        }
    }

    pub fn move_over(&self, step: usize, horizon: usize) -> i64 {
        let end = (step + horizon).min(self.mid.len() - 1);
        self.mid[end] - self.mid[step]
    }
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = ordered.len() / 2;
    Some(if ordered.len() % 2 == 1 {
        ordered[mid]
    } else {
        0.5 * (ordered[mid - 1] + ordered[mid])
    })
}

/// Put real sizes on the simulator's lot scale without reshaping them.
///
/// Absolute size is not comparable --- a tape is in tokens or contracts, the
/// simulator in lots --- but the *shape* is what matters, because the entity caps
/// and a maker's `max_qty` bite on the tail. One multiplicative factor fixed by
/// the median moves the distribution onto the right scale and leaves the tail
/// where it is. Anything above the ceiling is clipped, and how often that
/// happens is recorded rather than hidden.
pub fn rescale_sizes(raw: &[f64], target_median: i64, ceiling: i64) -> Vec<i64> {
    let positive: Vec<f64> = raw.iter().copied().filter(|v| *v > 0.0).collect();
    if positive.is_empty() {
        return vec![1; raw.len()];
    }
    let factor = target_median as f64 / median(&positive).unwrap();
    raw.iter()
        .map(|v| py_round(v * factor).clamp(1, ceiling))
        .collect()
}

/// What the entity column of a tape means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entities {
    /// UniswapX carries a real address per request, so an entity is an address
    /// holding one wallet. That is the *least* favourable setting for a
    /// per-entity cap --- with one wallet each there is nothing for it to
    /// collapse --- and saying so is the point.
    PerAddress,
    /// A Bybit tape has no identities, so entities are dealt round-robin rather
    /// than drawn, which keeps the assignment from smuggling a second generated
    /// distribution in on top of the real arrivals.
    RoundRobin(usize),
}

pub struct TapeRequests {
    pub requests: Vec<Request>,
    pub cfg: SimConfig,
    pub meta: BTreeMap<String, f64>,
    pub entity_kind: &'static str,
}

pub fn requests_from_tape(
    cfg: &SimConfig,
    market: &TapeMarket,
    tape: &Tape,
    entities: Entities,
    wallets_per_entity: usize,
    seed: u64,
) -> TapeRequests {
    let mut rng = PyRandom::new(seed);

    // Insertion order, deduplicated, exactly as Python's dict.fromkeys gives.
    let mut order: Vec<&str> = Vec::new();
    let mut seen: BTreeMap<&str, ()> = BTreeMap::new();
    for row in &tape.rows {
        if seen.insert(row.address.as_str(), ()).is_none() {
            order.push(row.address.as_str());
        }
    }

    let (entity_of, n_entities, entity_kind) = match entities {
        Entities::RoundRobin(n) => {
            rng.shuffle(&mut order);
            let map: BTreeMap<&str, usize> =
                order.iter().enumerate().map(|(i, a)| (*a, i % n)).collect();
            (map, n, "assigned round-robin (the tape has no identities)")
        }
        Entities::PerAddress => {
            let map: BTreeMap<&str, usize> =
                order.iter().enumerate().map(|(i, a)| (*a, i)).collect();
            let n = order.len();
            (map, n, "one entity per observed address")
        }
    };

    let requests: Vec<Request> = tape
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let entity = entity_of[row.address.as_str()];
            let wallet = entity * wallets_per_entity
                + if wallets_per_entity > 1 {
                    rng.randrange(0, wallets_per_entity as i64) as usize
                } else {
                    0
                };
            let informed = market.informed_flags[index];
            Request {
                step: row.step,
                entity,
                wallet,
                size: row.size,
                direction: row.direction,
                informed,
                signal: if informed {
                    market.move_over(row.step, market.horizon)
                } else {
                    0
                },
            }
        })
        .collect();

    let out_cfg = SimConfig {
        steps: tape.steps(),
        n_entities,
        wallets_per_entity,
        ..*cfg
    };
    let share =
        requests.iter().filter(|r| r.informed).count() as f64 / requests.len().max(1) as f64;
    let mut meta = tape.meta.clone();
    meta.insert("requests".into(), requests.len() as f64);
    meta.insert("entities".into(), n_entities as f64);
    meta.insert("wallets_per_entity".into(), wallets_per_entity as f64);
    meta.insert("informed_share_measured".into(), share);
    meta.insert("agreement_rate_measured".into(), market.agreement_rate);
    meta.insert("informed_share_estimated".into(), market.informed_share);
    meta.insert("informed_base_assumed".into(), cfg.informed_base);
    meta.insert("edge_ticks_measured".into(), market.edge as f64);
    meta.insert("edge_ticks_assumed".into(), cfg.informed_edge_ticks);

    TapeRequests {
        requests,
        cfg: out_cfg,
        meta,
        entity_kind,
    }
}

/// One symbol-day from the Bybit public trading archive.
///
/// Timestamp resolution changes with the era --- tenths of a millisecond before
/// late 2021 and whole seconds after --- so `step_ms` should be at least a second
/// on the later files, or every trade in a second lands on one step.
pub fn load_bybit(
    text: &str,
    cfg: &SimConfig,
    name: &str,
    steps: Option<usize>,
    step_ms: Option<u64>,
    max_rows: Option<usize>,
) -> Result<Tape, String> {
    let step_ms = step_ms.unwrap_or(cfg.step_ms);
    let total_steps = steps.unwrap_or(cfg.steps);

    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().ok_or("empty file")?.split(',').collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|h| *h == name)
            .ok_or_else(|| format!("no '{name}' column"))
    };
    let (ts, price_at, size_at, side_at) = (
        column("timestamp")?,
        column("price")?,
        column("size")?,
        column("side")?,
    );

    let mut trades: Vec<(f64, f64, f64, u8)> = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() <= price_at {
            continue;
        }
        trades.push((
            parts[ts].parse().map_err(|_| "bad timestamp")?,
            parts[price_at].parse().map_err(|_| "bad price")?,
            parts[size_at].parse().map_err(|_| "bad size")?,
            // Bybit states the aggressor: a Buy is the taker lifting the offer,
            // which is the simulator's direction 0.
            u8::from(parts[side_at] != "Buy"),
        ));
        if max_rows.is_some_and(|m| trades.len() >= m) {
            break;
        }
    }
    if trades.is_empty() {
        return Err(format!("{name} yielded no trades"));
    }

    // These files are written newest-first. Reading them in file order silently
    // produces negative step indices, so the sort is not optional.
    trades.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let span_s = total_steps as f64 * step_ms as f64 / 1000.0;
    let base = trades[0].0;
    trades.retain(|t| t.0 - base <= span_s);

    let steps_of: Vec<usize> = trades
        .iter()
        .map(|t| (((t.0 - base) * 1000.0 / step_ms as f64) as usize).min(total_steps))
        .collect();
    // Clamping a negative index to zero would turn a tape read in the wrong
    // order into a tape where every trade happened at once --- which still loads,
    // still runs, and reports a market that never existed.
    if steps_of.first().is_some_and(|s| *s != 0) || steps_of.windows(2).any(|w| w[0] > w[1]) {
        return Err(
            "trades are not in time order after sorting; the tape's own \
                    ordering changed or the sort was lost"
                .into(),
        );
    }

    let prices: Vec<f64> = trades.iter().map(|t| t.1).collect();
    let tick = median(&prices).unwrap() / cfg.ref_mid0 as f64;
    let mut by_step: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
    for (step, price) in steps_of.iter().zip(&prices) {
        by_step.entry(*step).or_default().push(*price);
    }
    let mut mid = Vec::with_capacity(total_steps + 1);
    let mut last = cfg.ref_mid0;
    for step in 0..=total_steps {
        if let Some(values) = by_step.get(&step) {
            last = py_round(median(values).unwrap() / tick).max(1);
        }
        mid.push(last);
    }

    let sizes: Vec<f64> = trades.iter().map(|t| t.2).collect();
    let lots = rescale_sizes(&sizes, 40, SIZE_CEILING);
    let rows: Vec<TapeRow> = steps_of
        .iter()
        .zip(&lots)
        .zip(&trades)
        .enumerate()
        .map(|(i, ((step, lot), trade))| TapeRow {
            step: *step,
            address: format!("taker:{i}"),
            size: *lot,
            direction: trade.3,
        })
        .collect();

    let span = trades.last().unwrap().0 - trades[0].0;
    let meta: BTreeMap<String, f64> = [
        ("trades".to_string(), rows.len() as f64),
        ("step_ms".to_string(), step_ms as f64),
        ("span_s".to_string(), span),
        (
            "arrival_per_s".to_string(),
            rows.len() as f64 / span.max(1e-9),
        ),
        ("tick_value".to_string(), tick),
        (
            "sizes_over_largest_bucket".to_string(),
            lots.iter().filter(|v| **v > 400).count() as f64,
        ),
        (
            "sizes_at_ceiling".to_string(),
            lots.iter().filter(|v| **v >= SIZE_CEILING).count() as f64,
        ),
    ]
    .into_iter()
    .collect();

    Ok(Tape {
        mid,
        rows,
        source: format!("bybit:{name}"),
        meta,
    })
}
