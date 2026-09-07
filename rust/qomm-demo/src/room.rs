//! Seats and server-side projections: a browser receives only its own business.

use crate::bots::{maker_filled, step_maker, step_taker};
use crate::model::{price_one, Policy, Request, BUY, DEMO_POLICY_VALID_UNTIL, FIELDS, SELL};
use crate::mpc::{
    is_corporate_queue_pending, MpcProductHandoff, MpcQuoteEngine, MpcSettlementInputs,
};
use crate::portfolio::{MakerReserve, Portfolio, SettlementRecord, TakerReservation};
use crate::protocol::{Session, BEHAVIOURS, HONEST, LIE_PRODUCT};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const TAKER: &str = "taker";
pub const MAKER: &str = "maker";
pub const NODE: &str = "node";
pub const OBSERVER: &str = "observer";

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub reference: i64,
    pub scale: i64,
}

pub fn default_assets() -> Vec<Asset> {
    vec![
        Asset {
            name: "USD/JPY".into(),
            reference: 15_750,
            scale: 100,
        },
        Asset {
            name: "EUR/USD".into(),
            reference: 10_850,
            scale: 10_000,
        },
        Asset {
            name: "BTC/USD".into(),
            reference: 6_420_000,
            scale: 100,
        },
    ]
}

#[derive(Clone, Debug)]
pub struct Seat {
    pub id: String,
    pub kind: String,
    pub index: usize,
    pub label: String,
    pub holder: Option<String>,
    pub forced_manual: bool,
}

impl Seat {
    pub fn mode(&self) -> &'static str {
        if self.holder.is_some() || self.forced_manual {
            "manual"
        } else {
            "auto"
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Notice {
    pub at: f64,
    pub code: String,
    pub fields: Map<String, Value>,
    pub tone: String,
}

#[derive(Clone, Debug)]
pub struct RoundResult {
    pub number: u64,
    pub engine: String,
    pub request: Request,
    pub outcome: crate::model::Outcome,
    /// A verified no-fill remains a successful round but must not expose a
    /// counterparty or enter settlement as a trade.
    pub filled: bool,
    pub masked_key: i128,
    pub mask: u64,
    pub padded: usize,
    pub named: BTreeMap<usize, usize>,
    pub rejected: Vec<(usize, String, usize)>,
    pub reductions: usize,
    pub corrections: usize,
    pub aborted: bool,
    pub abort_reason: String,
    pub abort_code: String,
    pub abort_fields: BTreeMap<String, usize>,
    pub product_capacity: usize,
    pub open_capacity: usize,
    pub silent: Vec<usize>,
    pub corrupted_inputs: Vec<usize>,
    pub input_check: bool,
    pub elapsed_ms: f64,
    pub settled: bool,
    pub node_shares: BTreeMap<usize, Vec<String>>,
    pub used_policies: Vec<Policy>,
    pub verified: Option<bool>,
    pub verified_detail: String,
    pub engine_stats: Value,
    pub product_handoff: Option<MpcProductHandoff>,
}

impl RoundResult {
    pub fn unpack(&self) -> (Option<i64>, Option<usize>) {
        if !self.filled {
            return (None, None);
        }
        let Some(winner) = self.outcome.winner else {
            return (None, None);
        };
        let packed = self.masked_key - i128::from(self.mask);
        let padded = self.padded as i128;
        let cost = packed.div_euclid(padded);
        let maker = packed.rem_euclid(padded) as usize;
        if maker != winner {
            return (None, None);
        }
        (i64::try_from(cost).ok(), Some(maker))
    }
}

/// One step of a round as the browsers are shown it.
///
/// The round is computed in one go and the phases are a replay of what it
/// did: `name` is the machine key the page keys its network diagram on,
/// `note` is the English caption a transcript reader wants, and `fields`
/// carries the numbers the page needs to caption the step in its own
/// language.  Nothing in `fields` is private to a seat: the same phase is
/// broadcast to every connection.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Phase {
    pub name: String,
    pub note: String,
    pub fields: Map<String, Value>,
}

impl Phase {
    fn new(name: &str, note: impl Into<String>, fields: Value) -> Self {
        Self {
            name: name.into(),
            note: note.into(),
            fields: fields.as_object().cloned().unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DemoConfig {
    pub round_seconds: f64,
    pub step_ms: u64,
    pub auto_rounds: bool,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            round_seconds: 8.0,
            step_ms: 350,
            auto_rounds: true,
        }
    }
}

pub struct Room {
    pub assets: Vec<Asset>,
    pub n_makers: usize,
    pub n_nodes: usize,
    pub threshold: usize,
    pub input_check: bool,
    pub policies: Vec<Policy>,
    pub maker_portfolios: Vec<Portfolio>,
    pub maker_reserves: Vec<MakerReserve>,
    pub taker_portfolio: Portfolio,
    pub taker_reservation: Option<TakerReservation>,
    pub request_limit: i64,
    pub settlements: Vec<SettlementRecord>,
    pub infrastructure: Value,
    pub behaviours: BTreeMap<usize, String>,
    pub request: Request,
    pub seats: BTreeMap<String, Seat>,
    pub sessions: BTreeMap<String, String>,
    pub last: Option<RoundResult>,
    pub history: Vec<RoundResult>,
    pub notices: BTreeMap<String, Vec<Notice>>,
    pub engine: Option<Box<dyn MpcQuoteEngine>>,
    pub phase: String,
    pub phase_note: String,
    pub phase_fields: Map<String, Value>,
    /// True from the moment a round has been computed until its phases have
    /// been shown and it has settled.  A second request to start a round in
    /// that window is refused rather than queued.
    pub busy: bool,
    round_number: u64,
    now: i64,
    rng: StdRng,
}

impl Room {
    pub fn new(
        n_makers: usize,
        n_nodes: usize,
        threshold: usize,
        input_check: bool,
        seed: u64,
    ) -> Result<Self, String> {
        Session::new(n_nodes, threshold, BTreeMap::new(), seed)?;
        let assets = default_assets();
        let mut rng = StdRng::seed_from_u64(seed);
        let policies = (0..n_makers)
            .map(|index| Policy {
                asset: (index % assets.len()) as i64,
                ask_level: rng.gen_range(-15..=15),
                spread: rng.gen_range(10..=80),
                slope: rng.gen_range(0..=3),
                invcoef: 1,
                inv: rng.gen_range(-50..=50),
                maxqty: [50, 100, 200, 500][rng.gen_range(0..4)],
                expiry: DEMO_POLICY_VALID_UNTIL,
                active: 1,
                use_ref: 1,
            })
            .collect();
        let mut seats = BTreeMap::from([(
            TAKER.into(),
            Seat {
                id: TAKER.into(),
                kind: TAKER.into(),
                index: 0,
                label: String::new(),
                holder: None,
                forced_manual: false,
            },
        )]);
        for index in 0..n_makers {
            let id = format!("maker:{index}");
            seats.insert(
                id.clone(),
                Seat {
                    id,
                    kind: MAKER.into(),
                    index,
                    label: String::new(),
                    holder: None,
                    forced_manual: false,
                },
            );
        }
        for index in 0..n_nodes {
            let id = format!("node:{index}");
            seats.insert(
                id.clone(),
                Seat {
                    id,
                    kind: NODE.into(),
                    index,
                    label: String::new(),
                    holder: None,
                    forced_manual: false,
                },
            );
        }
        let maker_portfolios = vec![Portfolio::funded(assets.len()); n_makers];
        let maker_reserves = vec![MakerReserve::default(); n_makers];
        let taker_portfolio = Portfolio::funded(assets.len());
        let request_limit = assets[0].reference + (assets[0].reference / 100).max(10);
        let mut room = Self {
            assets,
            n_makers,
            n_nodes,
            threshold,
            input_check,
            policies,
            maker_portfolios,
            maker_reserves,
            taker_portfolio,
            taker_reservation: None,
            request_limit,
            settlements: Vec::new(),
            infrastructure: json!({
                "mode": "local",
                "defmi": false,
                "participant_registry": false,
            }),
            behaviours: (0..n_nodes).map(|node| (node, HONEST.into())).collect(),
            request: Request::default(),
            seats,
            sessions: BTreeMap::new(),
            last: None,
            history: Vec::new(),
            notices: BTreeMap::new(),
            engine: None,
            phase: "idle".into(),
            phase_note: String::new(),
            phase_fields: Map::new(),
            busy: false,
            round_number: 0,
            now: 0,
            rng,
        };
        for maker in 0..room.n_makers {
            room.refresh_maker_reserve(maker)?;
        }
        Ok(room)
    }

    pub fn install_mpc_engine(
        &mut self,
        mut engine: impl MpcQuoteEngine + 'static,
    ) -> Result<(), String> {
        if engine.input_check() != self.input_check {
            return Err("the MP-SPDZ circuit input-check shape differs from the room".into());
        }
        let settlement = self.maker_preauthorization_inputs()?;
        engine.preauthorize_maker_policies(&self.policies, &settlement, self.now)?;
        self.engine = Some(Box::new(engine));
        Ok(())
    }

    /// Replace the explanatory room's bootstrap balances with the capacities
    /// read from the legal-entity participant modules.  This must happen
    /// before an MPC engine is installed: Maker policy reserves are rebuilt
    /// against those real caps, and an already queued Taker request is then
    /// restored by `drain_mpc_queue` from its signed durable envelope.
    pub fn configure_participant_capacities(
        &mut self,
        maker_capacities: &[(u64, u64)],
        taker_capacity: (u64, u64),
    ) -> Result<(), String> {
        if self.engine.is_some()
            || self.taker_reservation.is_some()
            || self.last.is_some()
            || !self.history.is_empty()
            || self.busy
        {
            return Err("participant capacities can only be configured before execution".into());
        }
        if maker_capacities.len() != self.n_makers {
            return Err("participant capacity count differs from the Maker count".into());
        }
        let convert = |(cash, inventory): (u64, u64)| -> Result<Portfolio, String> {
            Portfolio::funded_with(
                self.assets.len(),
                i64::try_from(cash)
                    .map_err(|_| "participant cash capacity exceeds the demo integer range")?,
                i64::try_from(inventory)
                    .map_err(|_| "participant inventory capacity exceeds the demo integer range")?,
            )
        };
        let replacement_makers = maker_capacities
            .iter()
            .copied()
            .map(convert)
            .collect::<Result<Vec<_>, _>>()?;
        let replacement_taker = convert(taker_capacity)?;
        let previous_makers = std::mem::replace(&mut self.maker_portfolios, replacement_makers);
        let previous_reserves = std::mem::replace(
            &mut self.maker_reserves,
            vec![MakerReserve::default(); self.n_makers],
        );
        let previous_taker = std::mem::replace(&mut self.taker_portfolio, replacement_taker);
        for maker in 0..self.n_makers {
            if let Err(error) = self.refresh_maker_reserve(maker) {
                self.maker_portfolios = previous_makers;
                self.maker_reserves = previous_reserves;
                self.taker_portfolio = previous_taker;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Replace only the Taker projection with holdings reconstructed by its
    /// participant module from canonical DeFMI settlement claims. Capacity is
    /// a genesis/facility limit; it is not a current balance after prior DvP.
    pub fn configure_taker_canonical_portfolio(
        &mut self,
        cash: u64,
        inventory: &[u64],
    ) -> Result<(), String> {
        if self.engine.is_some()
            || self.taker_reservation.is_some()
            || self.last.is_some()
            || !self.history.is_empty()
            || self.busy
        {
            return Err("canonical Taker holdings can only be configured before execution".into());
        }
        if inventory.len() != self.assets.len() {
            return Err("canonical Taker inventory differs from the room asset count".into());
        }
        let replacement = Portfolio {
            cash_available: i64::try_from(cash)
                .map_err(|_| "canonical Taker cash exceeds the demo integer range")?,
            cash_reserved: 0,
            inventory_available: inventory
                .iter()
                .copied()
                .map(|value| {
                    i64::try_from(value)
                        .map_err(|_| "canonical Taker inventory exceeds the demo integer range")
                })
                .collect::<Result<Vec<_>, _>>()?,
            inventory_reserved: vec![0; inventory.len()],
        };
        replacement.validate()?;
        self.taker_portfolio = replacement;
        Ok(())
    }

    pub fn set_infrastructure(&mut self, value: Value) -> Result<(), String> {
        if !value.is_object() {
            return Err("infrastructure description must be a JSON object".into());
        }
        self.infrastructure = value;
        Ok(())
    }

    pub fn configure_input_check(&mut self, enabled: bool) -> Result<(), String> {
        if self
            .engine
            .as_ref()
            .is_some_and(|engine| engine.input_check() != enabled)
        {
            return Err(
                "changing input_check would require a different compiled MPC circuit".into(),
            );
        }
        self.input_check = enabled;
        Ok(())
    }

    fn engine_description(&self) -> (&str, String, bool, String) {
        self.engine.as_ref().map_or_else(
            || {
                (
                    "sim",
                    "Rust share layer and zkfmi-zk decoder; tournament cleartext".into(),
                    true,
                    String::new(),
                )
            },
            |engine| {
                (
                    engine.name(),
                    engine.note(),
                    engine.robust(),
                    engine.robust_reason().into(),
                )
            },
        )
    }

    pub fn new_session(&mut self) -> String {
        format!(
            "{:016x}{:08x}",
            self.rng.gen::<u64>(),
            self.rng.gen::<u32>()
        )
    }

    pub fn claim(&mut self, session: &str, seat_id: &str, label: &str) -> (bool, String) {
        if seat_id == OBSERVER {
            self.release(session);
            self.sessions.insert(session.into(), OBSERVER.into());
            return (true, String::new());
        }
        let Some(seat) = self.seats.get(seat_id) else {
            return (false, format!("no seat called {seat_id}"));
        };
        if seat
            .holder
            .as_deref()
            .is_some_and(|holder| holder != session)
        {
            return (
                false,
                format!(
                    "{seat_id} is taken by {}",
                    if seat.label.is_empty() {
                        "someone"
                    } else {
                        &seat.label
                    }
                ),
            );
        }
        self.release(session);
        let claimed_label = {
            let seat = self.seats.get_mut(seat_id).expect("checked seat");
            seat.holder = Some(session.into());
            if !label.is_empty() {
                seat.label = label.chars().take(24).collect();
            }
            seat.label.clone()
        };
        self.sessions.insert(session.into(), seat_id.into());
        self.note(
            seat_id,
            "claimed",
            "info",
            json!({"seat": seat_id, "label": claimed_label}),
        );
        (true, String::new())
    }

    pub fn release(&mut self, session: &str) {
        if let Some(previous) = self.sessions.remove(session) {
            if previous != OBSERVER {
                if let Some(seat) = self.seats.get_mut(&previous) {
                    if seat.holder.as_deref() == Some(session) {
                        seat.holder = None;
                    }
                }
            }
        }
    }

    pub fn seat_of(&self, session: &str) -> Option<&Seat> {
        self.sessions
            .get(session)
            .filter(|seat| seat.as_str() != OBSERVER)
            .and_then(|seat| self.seats.get(seat))
    }

    fn default_limit(&self, asset: usize, direction: i64) -> i64 {
        let reference = self.assets[asset].reference;
        let margin = (reference / 100).max(10);
        if direction == BUY {
            reference.saturating_add(margin)
        } else {
            reference.saturating_sub(margin).max(1)
        }
    }

    fn maker_cash_requirement(&self, policy: &Policy) -> Result<i64, String> {
        if policy.active == 0 || policy.maxqty <= 0 {
            return Ok(0);
        }
        let references = self
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let mut maximum = 0_i64;
        for quantity in 1..=policy.maxqty {
            let request = Request {
                asset: policy.asset,
                qty: quantity,
                direction: SELL,
                entity: 0,
                is_real: 1,
            };
            let (_, bid) = price_one(policy, &request, &references);
            let required = quantity
                .checked_mul(bid.max(0))
                .ok_or_else(|| "Maker cash reservation overflowed".to_string())?;
            maximum = maximum.max(required);
        }
        Ok(maximum)
    }

    fn refresh_maker_reserve(&mut self, maker: usize) -> Result<(), String> {
        let policy = self
            .policies
            .get(maker)
            .ok_or_else(|| "unknown maker".to_string())?
            .clone();
        let asset = usize::try_from(policy.asset)
            .ok()
            .filter(|asset| *asset < self.assets.len())
            .ok_or_else(|| "Maker policy names an unknown asset".to_string())?;
        let wanted_inventory = if policy.active == 0 { 0 } else { policy.maxqty };
        let wanted_cash = self.maker_cash_requirement(&policy)?;
        let policy_key = hex::encode(Sha256::digest(
            serde_json::to_vec(&policy).map_err(|error| error.to_string())?,
        ));
        let old = self
            .maker_reserves
            .get(maker)
            .ok_or_else(|| "unknown Maker reserve".to_string())?
            .clone();
        if old.policy_key == policy_key
            && old.asset == asset
            && old.standing_inventory == wanted_inventory
            && old.standing_cash == wanted_cash
        {
            // Same registered policy, same standing mandate: the remaining
            // balance already reflects the fills DeFMI and the MPC nodes carry.
            return Ok(());
        }
        let mut candidate = self
            .maker_portfolios
            .get(maker)
            .ok_or_else(|| "unknown Maker portfolio".to_string())?
            .clone();

        candidate.cash_reserved = candidate
            .cash_reserved
            .checked_sub(old.cash)
            .ok_or_else(|| "Maker cash reserve underflowed".to_string())?;
        candidate.cash_available = candidate
            .cash_available
            .checked_add(old.cash)
            .ok_or_else(|| "Maker cash release overflowed".to_string())?;
        if old.asset >= candidate.inventory_available.len() {
            return Err("stored Maker reserve names an unknown asset".into());
        }
        candidate.inventory_reserved[old.asset] = candidate.inventory_reserved[old.asset]
            .checked_sub(old.inventory)
            .ok_or_else(|| "Maker inventory reserve underflowed".to_string())?;
        candidate.inventory_available[old.asset] = candidate.inventory_available[old.asset]
            .checked_add(old.inventory)
            .ok_or_else(|| "Maker inventory release overflowed".to_string())?;

        if candidate.cash_available < wanted_cash
            || candidate.inventory_available[asset] < wanted_inventory
        {
            return Err(format!(
                "Maker {maker} cannot reserve policy maximum: needs {wanted_inventory} units and {wanted_cash} cash units"
            ));
        }
        candidate.cash_available -= wanted_cash;
        candidate.cash_reserved += wanted_cash;
        candidate.inventory_available[asset] -= wanted_inventory;
        candidate.inventory_reserved[asset] += wanted_inventory;
        candidate.validate()?;
        self.maker_portfolios[maker] = candidate;
        self.maker_reserves[maker] = MakerReserve {
            asset,
            inventory: wanted_inventory,
            cash: wanted_cash,
            standing_inventory: wanted_inventory,
            standing_cash: wanted_cash,
            policy_key,
        };
        Ok(())
    }

    fn effective_policies(&self, request: &Request) -> (Vec<Policy>, BTreeMap<usize, String>) {
        let references = self
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let mut policies = self.policies.clone();
        let mut reasons = BTreeMap::new();
        for (maker, policy) in policies.iter_mut().enumerate() {
            if policy.active == 0 || policy.asset != request.asset || request.qty > policy.maxqty {
                continue;
            }
            let reserve = &self.maker_reserves[maker];
            let reason = if reserve.asset != request.asset as usize {
                Some("pre-reserve belongs to another asset")
            } else if request.direction == BUY && reserve.inventory < request.qty {
                Some("pre-reserved inventory is too small")
            } else if request.direction == SELL {
                let (_, bid) = price_one(policy, request, &references);
                let needed = request.qty.checked_mul(bid.max(0));
                if needed.is_none_or(|needed| needed > reserve.cash) {
                    Some("pre-reserved cash is too small")
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(reason) = reason {
                policy.active = 0;
                reasons.insert(maker, reason.to_string());
            }
        }
        (policies, reasons)
    }

    /// Sign and reserve the current Taker maximum before the RFQ enters MPC.
    /// This is the only public direct-round prerequisite; settlement never
    /// asks the Taker or Maker for another signature after a match is known.
    pub fn prepare_taker_reservation(&mut self) -> Result<(), String> {
        if self.taker_reservation.is_some() {
            return Err("the previous Taker reservation is still active".into());
        }
        if self.request.is_real == 0 {
            return Ok(());
        }
        let asset = usize::try_from(self.request.asset)
            .ok()
            .filter(|asset| *asset < self.assets.len())
            .ok_or_else(|| "Taker request names an unknown asset".to_string())?;
        if self.request_limit <= 0 {
            return Err("Taker price limit must be positive".into());
        }
        let mut portfolio = self.taker_portfolio.clone();
        let (rail, amount) = if self.request.direction == BUY {
            let amount = self
                .request
                .qty
                .checked_mul(self.request_limit)
                .ok_or_else(|| "Taker cash reservation overflowed".to_string())?;
            if portfolio.cash_available < amount {
                return Err(format!(
                    "Taker has {} cash units available but the signed limit needs {amount}",
                    portfolio.cash_available
                ));
            }
            portfolio.cash_available -= amount;
            portfolio.cash_reserved += amount;
            ("cash".to_string(), amount)
        } else {
            if portfolio.inventory_available[asset] < self.request.qty {
                return Err(format!(
                    "Taker has {} units available but the request needs {}",
                    portfolio.inventory_available[asset], self.request.qty
                ));
            }
            portfolio.inventory_available[asset] -= self.request.qty;
            portfolio.inventory_reserved[asset] += self.request.qty;
            ("inventory".to_string(), self.request.qty)
        };
        portfolio.validate()?;
        self.taker_portfolio = portfolio;
        self.taker_reservation = Some(TakerReservation {
            round: self.round_number + 1,
            asset,
            direction: self.request.direction,
            quantity: self.request.qty,
            limit_price: self.request_limit,
            amount,
            rail,
        });
        Ok(())
    }

    fn release_taker_reservation(&mut self) -> Result<Option<TakerReservation>, String> {
        let Some(reservation) = self.taker_reservation.take() else {
            return Ok(None);
        };
        if reservation.rail == "cash" {
            self.taker_portfolio.cash_reserved = self
                .taker_portfolio
                .cash_reserved
                .checked_sub(reservation.amount)
                .ok_or_else(|| "Taker cash reserve underflowed".to_string())?;
            self.taker_portfolio.cash_available = self
                .taker_portfolio
                .cash_available
                .checked_add(reservation.amount)
                .ok_or_else(|| "Taker cash release overflowed".to_string())?;
        } else {
            self.taker_portfolio.inventory_reserved[reservation.asset] =
                self.taker_portfolio.inventory_reserved[reservation.asset]
                    .checked_sub(reservation.amount)
                    .ok_or_else(|| "Taker inventory reserve underflowed".to_string())?;
            self.taker_portfolio.inventory_available[reservation.asset] =
                self.taker_portfolio.inventory_available[reservation.asset]
                    .checked_add(reservation.amount)
                    .ok_or_else(|| "Taker inventory release overflowed".to_string())?;
        }
        self.taker_portfolio.validate()?;
        Ok(Some(reservation))
    }

    fn portfolio_view(&self, portfolio: &Portfolio) -> Value {
        json!({
            "cash": {
                "available": portfolio.cash_available,
                "reserved": portfolio.cash_reserved,
                "total": portfolio.cash_total().unwrap_or_default(),
            },
            "inventory": self.assets.iter().enumerate().map(|(asset, market)| json!({
                "asset": asset,
                "name": market.name,
                "available": portfolio.inventory_available[asset],
                "reserved": portfolio.inventory_reserved[asset],
                "total": portfolio.inventory_total(asset).unwrap_or_default(),
            })).collect::<Vec<_>>(),
        })
    }

    fn ledger_root(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"QOMM:DEMO:DEFMI-STATE:v1");
        hash.update(self.round_number.to_be_bytes());
        for portfolio in self
            .maker_portfolios
            .iter()
            .chain(std::iter::once(&self.taker_portfolio))
        {
            hash.update(portfolio.cash_available.to_be_bytes());
            hash.update(portfolio.cash_reserved.to_be_bytes());
            for value in portfolio
                .inventory_available
                .iter()
                .chain(portfolio.inventory_reserved.iter())
            {
                hash.update(value.to_be_bytes());
            }
        }
        for reserve in &self.maker_reserves {
            hash.update((reserve.asset as u64).to_be_bytes());
            hash.update(reserve.inventory.to_be_bytes());
            hash.update(reserve.cash.to_be_bytes());
        }
        hex::encode(hash.finalize())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_settlement(
        &mut self,
        round: u64,
        status: &str,
        reason_code: &str,
        detail: &str,
        maker: Option<usize>,
        asset: usize,
        direction: i64,
        quantity: i64,
        price: Option<i64>,
        cash: Option<i64>,
        limit_price: i64,
    ) {
        let record = SettlementRecord {
            round,
            status: status.into(),
            reason_code: reason_code.into(),
            detail: detail.into(),
            maker,
            asset,
            direction,
            quantity,
            price,
            cash,
            limit_price,
            automatic: status == "settled",
            state_root: self.ledger_root(),
        };
        if let Some(existing) = self
            .settlements
            .iter_mut()
            .find(|existing| existing.round == round)
        {
            *existing = record;
        } else {
            self.settlements.push(record);
        }
        if self.settlements.len() > 40 {
            self.settlements.drain(..self.settlements.len() - 40);
        }
    }

    pub fn set_policy(&mut self, maker: usize, values: &Value) -> Result<(), String> {
        let previous = self
            .policies
            .get(maker)
            .ok_or_else(|| "unknown maker".to_string())?
            .clone();
        let previous_portfolio = self.maker_portfolios[maker].clone();
        let previous_reserve = self.maker_reserves[maker].clone();
        let policy = self.policies.get_mut(maker).expect("checked Maker policy");
        let object = values
            .as_object()
            .ok_or_else(|| "policy values must be an object".to_string())?;
        let mut fields = policy.fields();
        for (index, name) in FIELDS.iter().enumerate() {
            if let Some(value) = object.get(*name).and_then(Value::as_i64) {
                fields[index] = value;
            }
        }
        *policy = Policy::from_fields(&fields.map(i128::from));
        policy.asset = policy
            .asset
            .clamp(0, self.assets.len().saturating_sub(1) as i64);
        policy.maxqty = policy.maxqty.clamp(0, 500);
        policy.spread = policy.spread.max(2);
        if let Err(error) = self.refresh_maker_reserve(maker) {
            self.policies[maker] = previous;
            return Err(error);
        }
        let settlement = self.maker_preauthorization_inputs()?;
        if let Some(engine) = self.engine.as_mut() {
            if let Err(error) =
                engine.preauthorize_maker_policies(&self.policies, &settlement, self.now)
            {
                self.policies[maker] = previous;
                self.maker_portfolios[maker] = previous_portfolio;
                self.maker_reserves[maker] = previous_reserve;
                return Err(format!(
                    "Maker policy was not activated because pre-trade authority failed: {error}"
                ));
            }
        }
        Ok(())
    }

    /// The Maker amounts handed to the MPC engine are the signed standing
    /// maxima, not the remaining balances: the mandate, the DeFMI pool, and the
    /// nodes' resident remainder shares are keyed by the maximum, while fills
    /// advance the pool sequence without re-signing anything.
    fn maker_preauthorization_inputs(&self) -> Result<MpcSettlementInputs, String> {
        let settlement = MpcSettlementInputs {
            user_limit: 0,
            taker_securities_reserve: 0,
            taker_cash_reserve: 0,
            maker_securities_reserves: self
                .maker_reserves
                .iter()
                .map(|reserve| reserve.standing_inventory)
                .collect(),
            maker_cash_reserves: self
                .maker_reserves
                .iter()
                .map(|reserve| reserve.standing_cash)
                .collect(),
        };
        settlement.validate(self.n_makers)?;
        Ok(settlement)
    }

    pub fn set_behaviour(&mut self, node: usize, behaviour: &str) -> Result<(), String> {
        if node >= self.n_nodes || !BEHAVIOURS.contains(&behaviour) {
            return Err("unknown behaviour".into());
        }
        self.behaviours.insert(node, behaviour.into());
        Ok(())
    }

    pub fn set_request(&mut self, values: &Value) -> Result<(), String> {
        let object = values
            .as_object()
            .ok_or_else(|| "request values must be an object".to_string())?;
        let previous_asset = self.request.asset;
        let previous_direction = self.request.direction;
        if let Some(value) = object.get("asset").and_then(Value::as_i64) {
            self.request.asset = value;
        }
        if let Some(value) = object.get("qty").and_then(Value::as_i64) {
            self.request.qty = value;
        }
        if let Some(value) = object.get("direction").and_then(Value::as_i64) {
            self.request.direction = value;
        }
        if let Some(value) = object.get("is_real").and_then(Value::as_i64) {
            self.request.is_real = value;
        }
        self.request.asset = self
            .request
            .asset
            .clamp(0, self.assets.len().saturating_sub(1) as i64);
        self.request.qty = self.request.qty.clamp(1, 500);
        self.request.direction = if self.request.direction == 0 {
            BUY
        } else {
            SELL
        };
        self.request.is_real = i64::from(self.request.is_real != 0);
        if previous_asset != self.request.asset || previous_direction != self.request.direction {
            self.request_limit = self.default_limit(
                usize::try_from(self.request.asset).expect("clamped request asset"),
                self.request.direction,
            );
        }
        if let Some(value) = object.get("limit_price").and_then(Value::as_i64) {
            self.request_limit = value.max(1);
        }
        Ok(())
    }

    pub fn set_forced_manual(&mut self, seat: &str, manual: bool) {
        if let Some(seat) = self.seats.get_mut(seat) {
            seat.forced_manual = manual;
        }
    }

    fn padded(&self) -> usize {
        self.n_makers.max(1).next_power_of_two()
    }

    pub fn run_round(&mut self) -> Result<RoundResult, String> {
        self.round_number += 1;
        self.now += 1;
        let started = Instant::now();
        let request = self.request.clone();
        let (policies, reserve_reasons) = self.effective_policies(&request);
        let padded = self.padded();
        let reference = self
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let settlement_inputs = self
            .engine
            .is_some()
            .then(|| self.mpc_settlement_inputs(&request))
            .transpose()?;
        let mut result = if let Some(engine) = self.engine.as_mut() {
            let robust = engine.robust();
            let corrupt = self
                .behaviours
                .iter()
                .filter_map(|(node, behaviour)| (behaviour == LIE_PRODUCT).then_some(*node))
                .collect::<Vec<_>>();
            let inert = self
                .behaviours
                .iter()
                .filter_map(|(node, behaviour)| {
                    (behaviour != HONEST && !(robust && behaviour == LIE_PRODUCT)).then_some(*node)
                })
                .collect::<Vec<_>>();
            match engine.quote(
                &policies,
                &request,
                settlement_inputs
                    .as_ref()
                    .expect("external MPC has settlement inputs"),
                self.now,
                &corrupt,
            ) {
                Ok(round) => {
                    let mut stats = round.stats;
                    if !inert.is_empty() {
                        stats["inert_behaviours"] = json!(inert);
                    }
                    let aborted = !round.verified;
                    let corrections = round.named.len();
                    RoundResult {
                        number: self.round_number,
                        engine: "mpc".into(),
                        request,
                        outcome: round.outcome,
                        filled: round.filled,
                        masked_key: round.masked_key,
                        mask: round.mask,
                        padded,
                        named: round.named,
                        rejected: Vec::new(),
                        reductions: 0,
                        corrections,
                        aborted,
                        abort_reason: if aborted {
                            round.detail.clone()
                        } else {
                            String::new()
                        },
                        abort_code: if aborted {
                            "mismatch".into()
                        } else {
                            String::new()
                        },
                        abort_fields: BTreeMap::new(),
                        product_capacity: qomm_audit::locate::capacity(
                            self.n_nodes,
                            2 * self.threshold,
                        ),
                        open_capacity: qomm_audit::locate::capacity(self.n_nodes, self.threshold),
                        silent: Vec::new(),
                        corrupted_inputs: Vec::new(),
                        input_check: self.input_check,
                        elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                        settled: false,
                        node_shares: round.node_shares,
                        used_policies: policies,
                        verified: Some(round.verified),
                        verified_detail: round.detail,
                        engine_stats: stats,
                        product_handoff: round.product_handoff,
                    }
                }
                Err(error) => RoundResult {
                    number: self.round_number,
                    engine: "mpc".into(),
                    request,
                    outcome: crate::model::Outcome::default(),
                    filled: false,
                    masked_key: i128::from(self.rng.gen::<u32>()),
                    mask: self.rng.gen::<u32>() as u64,
                    padded,
                    named: BTreeMap::new(),
                    rejected: Vec::new(),
                    reductions: 0,
                    corrections: 0,
                    aborted: true,
                    abort_reason: error.clone(),
                    abort_code: if is_corporate_queue_pending(&error) {
                        "queued".into()
                    } else {
                        "engine".into()
                    },
                    abort_fields: BTreeMap::new(),
                    product_capacity: 0,
                    open_capacity: 0,
                    silent: Vec::new(),
                    corrupted_inputs: Vec::new(),
                    input_check: self.input_check,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                    settled: false,
                    node_shares: BTreeMap::new(),
                    used_policies: policies,
                    verified: Some(false),
                    verified_detail: error.clone(),
                    engine_stats: json!({"error": error}),
                    product_handoff: None,
                },
            }
        } else {
            let session = Session::new(
                self.n_nodes,
                self.threshold,
                self.behaviours.clone(),
                self.rng.gen(),
            )?;
            let protocol =
                session.run(&policies, &request, &reference, self.now, self.input_check)?;
            let mask = self.rng.gen::<u32>() as u64;
            let masked_key = if let (Some(cost), Some(winner)) =
                (protocol.outcome.cost, protocol.outcome.winner)
            {
                if protocol.transcript.aborted {
                    i128::from(self.rng.gen::<u32>())
                } else {
                    i128::from(cost) * padded as i128 + winner as i128 + i128::from(mask)
                }
            } else {
                i128::from(self.rng.gen::<u32>())
            };
            let filled = !protocol.transcript.aborted
                && request.is_real != 0
                && match (request.direction, protocol.outcome.price) {
                    (BUY, Some(price)) => price <= self.request_limit,
                    (SELL, Some(price)) => price >= self.request_limit,
                    _ => false,
                };
            RoundResult {
                number: self.round_number,
                engine: "sim".into(),
                request,
                outcome: protocol.outcome,
                filled,
                masked_key,
                mask,
                padded,
                named: protocol.transcript.named,
                rejected: protocol.transcript.rejected,
                reductions: protocol.transcript.reductions,
                corrections: protocol.transcript.corrections,
                aborted: protocol.transcript.aborted,
                abort_reason: protocol.transcript.abort_reason,
                abort_code: protocol.transcript.abort_code,
                abort_fields: protocol.transcript.abort_fields,
                product_capacity: protocol.transcript.product_capacity,
                open_capacity: protocol.transcript.open_capacity,
                silent: protocol.transcript.silent,
                corrupted_inputs: protocol.transcript.corrupted_inputs,
                input_check: self.input_check,
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                settled: false,
                node_shares: protocol.node_shares,
                used_policies: protocol.used_policies,
                verified: None,
                verified_detail: String::new(),
                engine_stats: json!({}),
                product_handoff: None,
            }
        };
        for quote in &mut result.outcome.quotes {
            if let Some(reason) = reserve_reasons.get(&quote.maker) {
                quote.reason.clone_from(reason);
            }
        }
        self.last = Some(result.clone());
        self.history.push(result.clone());
        if self.history.len() > 20 {
            self.history.drain(..self.history.len() - 20);
        }
        if result.aborted && result.abort_code == "queued" {
            // The public view intentionally reduces every post-reserve failure
            // to one reconciliation state.  Preserve the internal cause at the
            // coordinator boundary so operators can distinguish a node outage
            // from a proof, policy, or DeFMI consistency failure without
            // logging the plaintext RFQ.
            eprintln!(
                "qomm-demo round {} awaiting reconciliation: {}",
                result.number, result.abort_reason
            );
            self.note_all("reserve_queued", "warn", json!({"number": result.number}));
        } else if result.aborted {
            self.note_all(
                "stopped",
                "bad",
                json!({
                    "number": result.number,
                    "why": result.abort_code,
                    "detail": result.abort_reason,
                    "fields": result.abort_fields,
                }),
            );
        } else if result.corrections > 0 {
            self.note_all(
                "corrected",
                "warn",
                json!({
                    "number": result.number,
                    "corrections": result.corrections,
                    "reductions": result.reductions,
                    "named": result.named.keys().copied().collect::<Vec<_>>(),
                }),
            );
        } else {
            self.note_all(
                "finished",
                "info",
                json!({"number": result.number, "real": result.request.is_real != 0}),
            );
        }
        if !result.rejected.is_empty() {
            self.note_all(
                "refused",
                "bad",
                json!({
                    "who": result.rejected.iter().map(|row| row.0).collect::<Vec<_>>()
                }),
            );
        } else if !result.corrupted_inputs.is_empty() && !result.input_check {
            self.note_all("unchecked", "bad", json!({"who": result.corrupted_inputs}));
        }
        Ok(result)
    }

    /// A Taker reserve kept for a replay that will never come: the round
    /// ended awaiting canonical reconciliation, but the corporate request has
    /// since been finalized on canonical state without a replay (the
    /// participant module released it after the gateway's no-fill refund lost
    /// a race to its own).  Nothing in the queue can be claimed, so the local
    /// projection is released here and the round recorded as released; the
    /// next signed request is admitted again.  A `consumed` entry the room
    /// never settled is left alone and reported, since the fill it would
    /// need is not known here.
    fn drop_finalized_taker_reservation(&mut self) -> Result<bool, String> {
        if self.taker_reservation.is_none() {
            return Ok(false);
        }
        let finalized = match self.engine.as_mut() {
            Some(engine) => engine.retained_corporate_finalized()?,
            None => None,
        };
        let Some(status) = finalized else {
            return Ok(false);
        };
        if status != "released" {
            eprintln!(
                "qomm-demo: the retained corporate request is `{status}` on canonical state but the room never settled it; the Taker reserve is kept for the operator"
            );
            return Ok(false);
        }
        let Some(reservation) = self.release_taker_reservation()? else {
            return Ok(false);
        };
        self.record_settlement(
            reservation.round,
            "released",
            "corporate_finalized",
            "the corporate outbox entry was released on canonical state without a replay; the retained Taker reserve is dropped",
            None,
            reservation.asset,
            reservation.direction,
            reservation.quantity,
            None,
            None,
            reservation.limit_price,
        );
        eprintln!(
            "qomm-demo round {} released on canonical state; the retained Taker reserve is dropped",
            reservation.round
        );
        Ok(true)
    }

    /// Let the distributed engine replay one participant-owned corporate
    /// request without requiring another browser click. The replayed request
    /// carries its original policy/request/reserve snapshot; current screen
    /// controls are never substituted into an older queued RFQ.
    pub fn drain_mpc_queue(&mut self) -> Result<bool, String> {
        if self.busy {
            return Ok(false);
        }
        let started = Instant::now();
        let queued = match self.engine.as_mut() {
            Some(engine) => engine.replay_queued()?,
            None => None,
        };
        let Some(queued) = queued else {
            return self.drop_finalized_taker_reservation();
        };
        let asset = usize::try_from(queued.request.asset)
            .ok()
            .filter(|asset| *asset < self.assets.len())
            .ok_or_else(|| "queued request names an unknown asset".to_string())?;
        let (rail, amount) = if queued.request.direction == BUY {
            let expected = queued
                .request
                .qty
                .checked_mul(queued.settlement.user_limit)
                .ok_or_else(|| "queued Taker cash reserve overflowed".to_string())?;
            if queued.settlement.taker_cash_reserve != expected
                || queued.settlement.taker_securities_reserve != 0
            {
                return Err("queued buy differs from its signed cash reserve".into());
            }
            ("cash".to_string(), expected)
        } else if queued.request.direction == SELL {
            if queued.settlement.taker_securities_reserve != queued.request.qty
                || queued.settlement.taker_cash_reserve != 0
            {
                return Err("queued sell differs from its signed inventory reserve".into());
            }
            ("inventory".to_string(), queued.request.qty)
        } else {
            return Err("queued request direction is outside buy or sell".into());
        };
        if amount <= 0 || queued.settlement.user_limit <= 0 {
            return Err("queued request has no valid Taker reserve".into());
        }
        let result_number = if let Some(reservation) = self.taker_reservation.as_ref() {
            if reservation.asset != asset
                || reservation.direction != queued.request.direction
                || reservation.quantity != queued.request.qty
                || reservation.limit_price != queued.settlement.user_limit
                || reservation.amount != amount
                || reservation.rail != rail
            {
                return Err("queued request differs from the retained Taker reserve".into());
            }
            reservation.round
        } else {
            // The corporate queue and DeFMI reserve survive a gateway restart,
            // while this explanatory portfolio does not. Restore only the
            // matching local projection from the queued signed envelope.
            let round = self
                .round_number
                .checked_add(1)
                .ok_or_else(|| "round number overflowed while replaying the queue".to_string())?;
            if rail == "cash" {
                if self.taker_portfolio.cash_available < amount {
                    return Err(
                        "local Taker projection cannot restore the queued cash reserve".into(),
                    );
                }
                self.taker_portfolio.cash_available -= amount;
                self.taker_portfolio.cash_reserved += amount;
            } else {
                if self.taker_portfolio.inventory_available[asset] < amount {
                    return Err(
                        "local Taker projection cannot restore the queued inventory reserve".into(),
                    );
                }
                self.taker_portfolio.inventory_available[asset] -= amount;
                self.taker_portfolio.inventory_reserved[asset] += amount;
            }
            self.taker_portfolio.validate()?;
            self.taker_reservation = Some(TakerReservation {
                round,
                asset,
                direction: queued.request.direction,
                quantity: queued.request.qty,
                limit_price: queued.settlement.user_limit,
                amount,
                rail,
            });
            round
        };
        self.round_number = self.round_number.max(result_number);
        self.now = self.now.max(queued.market_time);
        let mut round = queued.round;
        let aborted = !round.verified;
        let abort_code = if aborted && is_corporate_queue_pending(&round.detail) {
            "queued"
        } else if aborted {
            "mismatch"
        } else {
            ""
        };
        let corrections = round.named.len();
        round.stats["corporate_queue_replay"] = json!({
            "automatic": true,
            "original_market_time": queued.market_time,
            "current_controls_substituted": false,
            "settlement_input": {
                "user_limit": queued.settlement.user_limit,
                "taker_securities_reserve": queued.settlement.taker_securities_reserve,
                "taker_cash_reserve": queued.settlement.taker_cash_reserve,
            },
        });
        let result = RoundResult {
            number: result_number,
            engine: "mpc".into(),
            request: queued.request,
            outcome: round.outcome,
            filled: round.filled,
            masked_key: round.masked_key,
            mask: round.mask,
            padded: self.padded(),
            named: round.named,
            rejected: Vec::new(),
            reductions: 0,
            corrections,
            aborted,
            abort_reason: if aborted {
                round.detail.clone()
            } else {
                String::new()
            },
            abort_code: abort_code.into(),
            abort_fields: BTreeMap::new(),
            product_capacity: qomm_audit::locate::capacity(self.n_nodes, 2 * self.threshold),
            open_capacity: qomm_audit::locate::capacity(self.n_nodes, self.threshold),
            silent: Vec::new(),
            corrupted_inputs: Vec::new(),
            input_check: self.input_check,
            elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            settled: false,
            node_shares: round.node_shares,
            used_policies: queued.policies,
            verified: Some(round.verified),
            verified_detail: round.detail,
            engine_stats: round.stats,
            product_handoff: round.product_handoff,
        };
        self.last = Some(result.clone());
        if let Some(existing) = self
            .history
            .iter_mut()
            .find(|existing| existing.number == result.number)
        {
            *existing = result.clone();
        } else {
            self.history.push(result.clone());
        }
        if self.history.len() > 20 {
            self.history.drain(..self.history.len() - 20);
        }
        if result.aborted {
            eprintln!(
                "qomm-demo replay round {} awaiting reconciliation: {}",
                result.number, result.abort_reason
            );
            self.note_all(
                "stopped",
                "bad",
                json!({
                    "number": result.number,
                    "detail": result.abort_reason,
                    "corporate_queue_replay": true,
                }),
            );
        } else {
            self.note_all(
                "finished",
                "info",
                json!({
                    "number": result.number,
                    "real": true,
                    "corporate_queue_replay": true,
                }),
            );
        }
        self.settle_last()?;
        self.end_round();
        Ok(true)
    }

    fn mpc_settlement_inputs(&self, request: &Request) -> Result<MpcSettlementInputs, String> {
        let mut taker_securities_reserve = 0;
        let mut taker_cash_reserve = 0;
        if request.is_real != 0 {
            let reserve = self
                .taker_reservation
                .as_ref()
                .ok_or_else(|| "real MPC request has no signed Taker reserve".to_string())?;
            if reserve.direction == BUY {
                taker_cash_reserve = reserve.amount;
            } else {
                taker_securities_reserve = reserve.amount;
            }
        }
        let settlement = MpcSettlementInputs {
            user_limit: self.request_limit,
            taker_securities_reserve,
            taker_cash_reserve,
            maker_securities_reserves: self
                .maker_reserves
                .iter()
                .map(|reserve| reserve.standing_inventory)
                .collect(),
            maker_cash_reserves: self
                .maker_reserves
                .iter()
                .map(|reserve| reserve.standing_cash)
                .collect(),
        };
        settlement.validate(self.n_makers)?;
        Ok(settlement)
    }

    pub fn note(&mut self, seat: &str, code: &str, tone: &str, fields: Value) {
        let line = Notice {
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            code: code.into(),
            fields: fields.as_object().cloned().unwrap_or_default(),
            tone: tone.into(),
        };
        let notices = self.notices.entry(seat.into()).or_default();
        notices.push(line);
        if notices.len() > 40 {
            notices.drain(..notices.len() - 40);
        }
    }

    pub fn note_all(&mut self, code: &str, tone: &str, fields: Value) {
        let seats = self
            .seats
            .keys()
            .cloned()
            .chain(std::iter::once(OBSERVER.into()))
            .collect::<Vec<_>>();
        for seat in seats {
            self.note(&seat, code, tone, fields.clone());
        }
    }

    /// Move only the seats that are not currently held by a person.
    pub fn step_bots(&mut self) -> Result<(), String> {
        let previous_policies = self.policies.clone();
        let previous_portfolios = self.maker_portfolios.clone();
        let previous_reserves = self.maker_reserves.clone();
        for maker in 0..self.n_makers {
            if self.seats[&format!("maker:{maker}")].mode() == "auto" {
                let previous = self.policies[maker].clone();
                step_maker(&mut self.policies[maker], self.assets.len(), &mut self.rng);
                // A Maker policy is compiled and registered for one instrument.
                // Automatic quote refreshes may move prices and inventory state,
                // but must not silently switch the circuit's bound instrument.
                self.policies[maker].asset = previous.asset;
                if self.refresh_maker_reserve(maker).is_err() {
                    self.policies[maker] = previous;
                }
            }
        }
        if self.seats[TAKER].mode() == "auto" {
            step_taker(&mut self.request, self.assets.len(), 0.35, &mut self.rng);
            self.request_limit = self.default_limit(
                usize::try_from(self.request.asset).expect("bot emits a valid asset"),
                self.request.direction,
            );
        }
        let settlement = self.maker_preauthorization_inputs()?;
        if let Some(engine) = self.engine.as_mut() {
            if let Err(error) =
                engine.preauthorize_maker_policies(&self.policies, &settlement, self.now)
            {
                self.policies = previous_policies;
                self.maker_portfolios = previous_portfolios;
                self.maker_reserves = previous_reserves;
                return Err(format!(
                    "scheduled Maker policy refresh failed before the fixed slot: {error}"
                ));
            }
        }
        Ok(())
    }

    /// A cover round computes identically but has no settlement side effect.
    pub fn settle_last(&mut self) -> Result<(), String> {
        let result = self
            .last
            .as_ref()
            .ok_or_else(|| "no completed round to settle".to_string())?
            .clone();
        let asset = usize::try_from(result.request.asset)
            .ok()
            .filter(|asset| *asset < self.assets.len())
            .ok_or_else(|| "completed request names an unknown asset".to_string())?;
        if result.request.is_real == 0 {
            self.record_settlement(
                result.number,
                "cover",
                "cover",
                "",
                None,
                asset,
                result.request.direction,
                result.request.qty,
                None,
                None,
                self.request_limit,
            );
            return Ok(());
        }
        let reservation = self
            .taker_reservation
            .as_ref()
            .ok_or_else(|| "a real request reached settlement without a Taker reserve".to_string())?
            .clone();
        if reservation.round != result.number
            || reservation.asset != asset
            || reservation.direction != result.request.direction
            || reservation.quantity != result.request.qty
        {
            return Err("Taker reservation does not bind the completed request".into());
        }
        if result.aborted && result.abort_code == "queued" {
            self.record_settlement(
                result.number,
                "queued",
                "mpc_queued",
                &result.abort_reason,
                None,
                asset,
                result.request.direction,
                result.request.qty,
                None,
                None,
                reservation.limit_price,
            );
            return Ok(());
        }
        if result.aborted
            || !result.filled
            || result.outcome.winner.is_none()
            || result.outcome.price.is_none()
        {
            self.release_taker_reservation()?;
            let (reason_code, detail) = if result.aborted {
                ("mpc_aborted", result.abort_reason.as_str())
            } else if result.outcome.winner.is_some() && result.outcome.price.is_some() {
                ("price_limit", "")
            } else {
                ("no_maker", "")
            };
            self.record_settlement(
                result.number,
                "released",
                reason_code,
                detail,
                None,
                asset,
                result.request.direction,
                result.request.qty,
                None,
                None,
                reservation.limit_price,
            );
            self.note(
                TAKER,
                "reserve_released",
                "warn",
                json!({"number": result.number, "reason": reason_code}),
            );
            return Ok(());
        }
        let maker = result.outcome.winner.expect("checked winner");
        let price = result.outcome.price.expect("checked price");
        let within_limit = if result.request.direction == BUY {
            price <= reservation.limit_price
        } else {
            price >= reservation.limit_price
        };
        if !within_limit {
            self.release_taker_reservation()?;
            self.record_settlement(
                result.number,
                "released",
                "price_limit",
                "",
                Some(maker),
                asset,
                result.request.direction,
                result.request.qty,
                Some(price),
                None,
                reservation.limit_price,
            );
            self.note(
                TAKER,
                "limit_released",
                "warn",
                json!({"number": result.number}),
            );
            return Ok(());
        }
        let cash = result
            .request
            .qty
            .checked_mul(price)
            .ok_or_else(|| "settlement cash amount overflowed".to_string())?;
        if cash < 0 {
            return Err("settlement price cannot produce negative cash".into());
        }

        let mut maker_portfolio = self.maker_portfolios[maker].clone();
        let mut maker_reserve = self.maker_reserves[maker].clone();
        let mut taker_portfolio = self.taker_portfolio.clone();
        if result.request.direction == BUY {
            if reservation.rail != "cash"
                || reservation.amount < cash
                || taker_portfolio.cash_reserved < reservation.amount
                || maker_reserve.asset != asset
                || maker_reserve.inventory < result.request.qty
                || maker_portfolio.inventory_reserved[asset] < result.request.qty
            {
                return Err("pre-trade reserves do not cover the matched buy".into());
            }
            taker_portfolio.cash_reserved -= reservation.amount;
            taker_portfolio.cash_available = taker_portfolio
                .cash_available
                .checked_add(reservation.amount - cash)
                .ok_or_else(|| "Taker cash refund overflowed".to_string())?;
            taker_portfolio.inventory_available[asset] = taker_portfolio.inventory_available[asset]
                .checked_add(result.request.qty)
                .ok_or_else(|| "Taker received inventory overflowed".to_string())?;
            maker_reserve.inventory -= result.request.qty;
            maker_portfolio.inventory_reserved[asset] -= result.request.qty;
            maker_portfolio.cash_available = maker_portfolio
                .cash_available
                .checked_add(cash)
                .ok_or_else(|| "Maker received cash overflowed".to_string())?;
        } else {
            if reservation.rail != "inventory"
                || taker_portfolio.inventory_reserved[asset] < reservation.amount
                || maker_reserve.asset != asset
                || maker_reserve.cash < cash
                || maker_portfolio.cash_reserved < cash
            {
                return Err("pre-trade reserves do not cover the matched sell".into());
            }
            taker_portfolio.inventory_reserved[asset] -= reservation.amount;
            maker_reserve.cash -= cash;
            maker_portfolio.cash_reserved -= cash;
            maker_portfolio.inventory_available[asset] = maker_portfolio.inventory_available[asset]
                .checked_add(result.request.qty)
                .ok_or_else(|| "Maker received inventory overflowed".to_string())?;
            taker_portfolio.cash_available = taker_portfolio
                .cash_available
                .checked_add(cash)
                .ok_or_else(|| "Taker received cash overflowed".to_string())?;
        }
        maker_portfolio.validate()?;
        taker_portfolio.validate()?;
        self.maker_portfolios[maker] = maker_portfolio;
        self.maker_reserves[maker] = maker_reserve;
        self.taker_portfolio = taker_portfolio;
        self.taker_reservation = None;
        maker_filled(&mut self.policies[maker], &result.request);
        if let Some(last) = self.last.as_mut() {
            last.settled = true;
        }
        if let Some(history) = self
            .history
            .iter_mut()
            .rev()
            .find(|row| row.number == result.number)
        {
            history.settled = true;
        }
        self.record_settlement(
            result.number,
            "settled",
            "automatic_dvp",
            "",
            Some(maker),
            asset,
            result.request.direction,
            result.request.qty,
            Some(price),
            Some(cash),
            reservation.limit_price,
        );
        self.note(
            &format!("maker:{maker}"),
            "you_won",
            "good",
            json!({"number": result.number, "settled": true}),
        );
        self.note(TAKER, "settled", "good", json!({"number": result.number}));
        Ok(())
    }

    /// The steps a computed round went through, each with what happened.
    ///
    /// The whole round is arithmetic that takes milliseconds; the phases are
    /// how the room shows it afterwards, one broadcast per step, so that a
    /// person can watch where the order is.  The captions are built from the
    /// transcript, so an aborted round stops at the step that stopped it.
    pub fn phases_of(&self, result: &RoundResult) -> Vec<Phase> {
        let n_values = 5 + self.n_makers * FIELDS.len();
        let mut steps = vec![Phase::new(
            "deal",
            format!(
                "{n_values} values split into {} shares each, one share per node",
                self.n_nodes
            ),
            json!({"values": n_values, "nodes": self.n_nodes}),
        )];
        let abort = json!({
            "aborted": true,
            "why": result.abort_code,
            "detail": result.abort_reason,
            "fields": result.abort_fields,
        });
        if result.aborted && result.abort_code == "absent" {
            steps.push(Phase::new("check", result.abort_reason.clone(), abort));
            return steps;
        }
        if result.input_check {
            let rejected = result
                .rejected
                .iter()
                .map(|(node, _, _)| *node)
                .collect::<Vec<_>>();
            if rejected.is_empty() {
                steps.push(Phase::new(
                    "check",
                    format!(
                        "{n_values} x {} shares against the commitments they were dealt under",
                        self.n_nodes
                    ),
                    json!({"values": n_values, "nodes": self.n_nodes, "skipped": false,
                           "rejected": []}),
                ));
            } else {
                let mut fields = abort.clone();
                fields["rejected"] = json!(rejected);
                fields["skipped"] = json!(false);
                steps.push(Phase::new("check", result.abort_reason.clone(), fields));
            }
        } else {
            steps.push(Phase::new(
                "check",
                "skipped: nothing binds a node to the share it was dealt",
                json!({"skipped": true, "rejected": []}),
            ));
        }
        if result.aborted && !result.rejected.is_empty() {
            return steps;
        }
        let degree = 2 * self.threshold;
        let named = result.named.keys().copied().collect::<Vec<_>>();
        // Two products per policy is what the share layer multiplies; the
        // transcript's `reductions` also counts the final opening, so the
        // caption names the products and the field keeps the raw count.
        let products = 2 * result.used_policies.len();
        let mut fields = json!({
            "products": products,
            "reductions": result.reductions,
            "degree": degree,
            "corrections": result.corrections,
            "named": named,
            "aborted": result.aborted,
            "engine": result.engine,
            "engine_stats": result.engine_stats,
        });
        let mut note = if result.engine == "mpc" {
            format!(
                "MP-SPDZ ran the circuit: {} rounds, {} MB",
                result.engine_stats.get("rounds").unwrap_or(&Value::Null),
                result.engine_stats.get("mb").unwrap_or(&Value::Null)
            )
        } else {
            format!("{products} products opened at degree {degree} and decoded")
        };
        if result.corrections > 0 {
            note.push_str(&format!(
                "; corrected {}, named {}",
                result.corrections,
                named
                    .iter()
                    .map(|node| format!("node {node}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        } else if result.aborted {
            note.clone_from(&result.abort_reason);
            fields["why"] = json!(result.abort_code);
            fields["detail"] = json!(result.abort_reason);
            fields["fields"] = json!(result.abort_fields);
        }
        steps.push(Phase::new("reduce", note, fields));
        if !result.aborted {
            steps.push(Phase::new(
                "open",
                "the key opens under the taker's mask; only the taker can subtract it",
                json!({"masked_key": result.masked_key.to_string()}),
            ));
        }
        steps
    }

    pub fn set_phase(&mut self, phase: &Phase) {
        self.phase.clone_from(&phase.name);
        self.phase_note.clone_from(&phase.note);
        self.phase_fields.clone_from(&phase.fields);
    }

    fn settle_phase(&self) -> Phase {
        let number = self.last.as_ref().map(|result| result.number).unwrap_or(0);
        match self
            .settlements
            .iter()
            .rev()
            .find(|settlement| settlement.round == number)
        {
            Some(settlement) => Phase::new(
                "settle",
                format!("{}: {}", settlement.status, settlement.reason_code),
                json!({
                    "status": settlement.status,
                    "reason": settlement.reason_code,
                    "state_root": settlement.state_root,
                    "automatic": settlement.automatic,
                }),
            ),
            None => Phase::new("settle", "no settlement record", json!({})),
        }
    }

    /// Compute a round and stand at its first phase.
    ///
    /// Moves the unattended seats, takes the taker's pre-trade reserve, runs
    /// the protocol, and returns every phase the round went through.  The
    /// room is `busy` from here until [`Room::end_round`]; the server walks
    /// the returned phases with [`Room::set_phase`], broadcasting each.
    pub fn begin_round(&mut self) -> Result<Vec<Phase>, String> {
        if self.busy {
            return Err("a round is already in progress".into());
        }
        self.step_bots()?;
        self.prepare_taker_reservation()?;
        let result = match self.run_round() {
            Ok(result) => result,
            Err(error) => {
                let _ = self.release_taker_reservation();
                return Err(error);
            }
        };
        let phases = self.phases_of(&result);
        self.busy = true;
        if let Some(first) = phases.first() {
            self.set_phase(first);
        }
        Ok(phases)
    }

    /// Settle the computed round and stand at the `settle` phase.
    pub fn finish_round(&mut self) -> Result<(), String> {
        let settled = self.settle_last();
        match &settled {
            Ok(()) => {
                let phase = self.settle_phase();
                self.set_phase(&phase);
            }
            Err(error) => {
                self.set_phase(&Phase::new("done", error.clone(), json!({"error": error})));
                self.busy = false;
            }
        }
        settled
    }

    /// Leave the round: phase `done`, the room free for the next one.
    pub fn end_round(&mut self) {
        let (number, ms) = self
            .last
            .as_ref()
            .map(|result| (result.number, (result.elapsed_ms * 10.0).round() / 10.0))
            .unwrap_or((0, 0.0));
        self.set_phase(&Phase::new(
            "done",
            format!("round {number}"),
            json!({"number": number, "ms": ms}),
        ));
        self.busy = false;
    }

    /// One whole round with no pause between its phases.
    pub fn play_round(&mut self) -> Result<RoundResult, String> {
        self.begin_round()?;
        self.finish_round()?;
        self.end_round();
        self.last
            .clone()
            .ok_or_else(|| "round completed without a result".to_string())
    }

    pub fn view(&self, session: &str, config: &DemoConfig, next_round_in: f64) -> Value {
        let seat_id = self.sessions.get(session).cloned();
        let watching = seat_id.as_deref() == Some(OBSERVER);
        let seat = self.seat_of(session);
        let result = self.last.as_ref();
        let (engine, engine_note, robust, robust_reason) = self.engine_description();
        let mut payload = Map::from_iter([
            ("type".into(), json!("view")),
            ("session".into(), json!(session)),
            ("seat".into(), json!(seat_id)),
            (
                "kind".into(),
                json!(seat
                    .map(|seat| seat.kind.as_str())
                    .or(watching.then_some(OBSERVER))),
            ),
            ("index".into(), json!(seat.map(|seat| seat.index))),
            (
                "label".into(),
                json!(seat.map(|seat| seat.label.as_str()).unwrap_or("")),
            ),
            (
                "config".into(),
                json!({
                    "n_makers": self.n_makers,
                    "n_nodes": self.n_nodes,
                    "threshold": self.threshold,
                    "input_check": self.input_check,
                    "round_seconds": config.round_seconds,
                    "step_ms": config.step_ms,
                    "auto_rounds": config.auto_rounds,
                    "engine": engine,
                    "engine_note": engine_note,
                    "robust": robust,
                    "robust_reason": robust_reason,
                }),
            ),
            (
                "assets".into(),
                json!(self
                    .assets
                    .iter()
                    .map(|asset| json!({
                        "name": asset.name,
                        "reference": asset.reference,
                        "scale": asset.scale,
                    }))
                    .collect::<Vec<_>>()),
            ),
            ("phase".into(), json!(self.phase)),
            ("phase_note".into(), json!(self.phase_note)),
            ("phase_fields".into(), json!(self.phase_fields)),
            ("busy".into(), json!(self.busy)),
            (
                "next_round_in".into(),
                if self.seats[TAKER].mode() == "manual" {
                    Value::Null
                } else {
                    json!(next_round_in.max(0.0))
                },
            ),
            ("public".into(), self.public_view(result)),
            ("infrastructure".into(), self.infrastructure.clone()),
            (
                "history".into(),
                json!(self
                    .history
                    .iter()
                    .skip(self.history.len().saturating_sub(8))
                    .map(|result| self.public_view(Some(result)))
                    .collect::<Vec<_>>()),
            ),
            ("seats".into(), self.seats_view(session, watching)),
            (
                "notices".into(),
                json!(self
                    .notices
                    .get(seat_id.as_deref().unwrap_or(OBSERVER))
                    .map(|rows| rows
                        .iter()
                        .skip(rows.len().saturating_sub(12))
                        .cloned()
                        .collect::<Vec<_>>())
                    .unwrap_or_default()),
            ),
        ]);
        if seat.is_some_and(|seat| seat.kind == TAKER) {
            payload.insert("taker".into(), self.taker_view(result));
        }
        if let Some(seat) = seat.filter(|seat| seat.kind == MAKER) {
            payload.insert("maker".into(), self.maker_view(seat.index, result));
        }
        if let Some(seat) = seat.filter(|seat| seat.kind == NODE) {
            payload.insert("node".into(), self.node_view(seat.index, result));
        }
        if watching {
            payload.insert("observer".into(), self.observer_view(result));
        }
        Value::Object(payload)
    }

    fn public_view(&self, result: Option<&RoundResult>) -> Value {
        let Some(result) = result else {
            return json!({});
        };
        let settlement = self
            .settlements
            .iter()
            .rev()
            .find(|settlement| settlement.round == result.number);
        json!({
            "number": result.number,
            "engine": result.engine,
            "masked_key": result.masked_key.to_string(),
            "reductions": result.reductions,
            "corrections": result.corrections,
            "named": result.named.keys().copied().collect::<Vec<_>>(),
            "named_counts": result.named.iter().map(|(node,count)| (node.to_string(),json!(count))).collect::<Map<_,_>>(),
            "aborted": result.aborted,
            "abort_reason": result.abort_reason,
            "abort_code": result.abort_code,
            "abort_fields": result.abort_fields,
            "product_capacity": result.product_capacity,
            "open_capacity": result.open_capacity,
            "silent": result.silent,
            "rejected": result.rejected.iter().map(|(node,dealer,position)| json!({
                "node": node, "dealer": dealer, "position": position,
            })).collect::<Vec<_>>(),
            "ms": (result.elapsed_ms * 10.0).round() / 10.0,
            "verified": result.verified,
            // The verifier's internal detail can contain the unpacked key or
            // why a private order did not fill. Every seat receives this
            // projection, so expose integrity only, never execution content.
            "verified_detail": result.verified.map(|verified| if verified {
                "MPC output verification passed"
            } else {
                "MPC output verification failed"
            }).unwrap_or(""),
            "engine_stats": result.engine_stats,
            "inert": result.engine_stats.get("inert_behaviours").is_some(),
            "settlement": settlement.map(|settlement| json!({
                "status": settlement.status,
                "state_root": settlement.state_root,
                "automatic": settlement.automatic,
            })),
        })
    }

    fn seats_view(&self, session: &str, observer: bool) -> Value {
        json!(self
            .seats
            .values()
            .map(|seat| {
                let mine = seat.holder.as_deref() == Some(session);
                let mut value = json!({
                    "id": seat.id,
                    "kind": seat.kind,
                    "index": seat.index,
                    "mode": seat.mode(),
                    "held": seat.holder.is_some(),
                    "label": if seat.holder.is_some() { seat.label.as_str() } else { "" },
                    "mine": mine,
                    "forced_manual": seat.forced_manual,
                });
                if seat.kind == NODE && (mine || observer) {
                    value["behaviour"] = json!(self.behaviours[&seat.index]);
                }
                value
            })
            .collect::<Vec<_>>())
    }

    fn taker_view(&self, result: Option<&RoundResult>) -> Value {
        let mut value = json!({
            "portfolio": self.portfolio_view(&self.taker_portfolio),
            "reservation": self.taker_reservation,
            "pending": {
                "asset": self.request.asset,
                "qty": self.request.qty,
                "direction": self.request.direction,
                "is_real": self.request.is_real,
                "limit_price": self.request_limit,
            }
        });
        if let Some(result) = result {
            let (cost, maker) = result.unpack();
            let (price, winner) = if result.filled {
                (result.outcome.price, result.outcome.winner)
            } else {
                (None, None)
            };
            value["last"] = json!({
                "number": result.number,
                "asset": result.request.asset,
                "qty": result.request.qty,
                "direction": result.request.direction,
                "is_real": result.request.is_real,
                "mask": result.mask.to_string(),
                "price": price,
                "winner": winner,
                "unpacked_cost": cost,
                "unpacked_maker": maker,
                "eligible": result.outcome.eligible,
                "settled": result.settled,
            });
            value["settlement"] = self
                .settlements
                .iter()
                .rev()
                .find(|settlement| settlement.round == result.number)
                .map_or(Value::Null, |settlement| json!(settlement));
        }
        value
    }

    fn maker_view(&self, maker: usize, result: Option<&RoundResult>) -> Value {
        let mut value = json!({
            "policy": self.policies[maker],
            "portfolio": self.portfolio_view(&self.maker_portfolios[maker]),
            "reserve": self.maker_reserves[maker],
            "fields": FIELDS,
            "fill": Value::Null,
            "told_nothing": true,
        });
        if let Some(result) = result {
            if result.outcome.winner == Some(maker) && result.settled && !result.aborted {
                value["fill"] = json!({
                    "number": result.number,
                    "asset": result.request.asset,
                    "qty": result.request.qty,
                    "direction": result.request.direction,
                    "price": result.outcome.price,
                });
                value["told_nothing"] = json!(false);
            }
            value["settlement"] = self
                .settlements
                .iter()
                .rev()
                .find(|settlement| {
                    settlement.round == result.number && settlement.maker == Some(maker)
                })
                .map_or(Value::Null, |settlement| json!(settlement));
        }
        value
    }

    fn node_view(&self, node: usize, result: Option<&RoundResult>) -> Value {
        let Some(result) = result else {
            return json!({
                "behaviour": self.behaviours[&node], "shares": [],
                "named_me": false, "times_named": 0,
                "custody": false,
                "job": Value::Null,
            });
        };
        let settlement = self
            .settlements
            .iter()
            .rev()
            .find(|settlement| settlement.round == result.number);
        json!({
            "behaviour": self.behaviours[&node],
            "shares": result.node_shares.get(&node).cloned().unwrap_or_default(),
            "named_me": result.named.contains_key(&node),
            "times_named": result.named.get(&node).copied().unwrap_or(0),
            "rejected_me": result.rejected.iter().any(|(candidate,_,_)| *candidate == node),
            "silent_me": result.silent.contains(&node),
            "custody": false,
            "job": {
                "round": result.number,
                "request_share": result.node_shares.get(&node).and_then(|shares| shares.first()),
                "policy_share_count": self.n_makers,
                "matching": if result.aborted { "aborted" } else { "complete" },
                "proof": if result.aborted { "rejected" } else { "verified" },
                "settlement": settlement.map(|settlement| settlement.status.as_str()).unwrap_or("pending"),
                "state_root": settlement.map(|settlement| settlement.state_root.as_str()),
            },
        })
    }

    fn observer_view(&self, result: Option<&RoundResult>) -> Value {
        let mut value = json!({
            "behaviours": self.behaviours.iter().map(|(node,behaviour)| (node.to_string(),json!(behaviour))).collect::<Map<_,_>>(),
            "policies": self.policies,
            "taker_portfolio": self.portfolio_view(&self.taker_portfolio),
            "maker_portfolios": self.maker_portfolios.iter().enumerate().map(|(maker, portfolio)| json!({
                "maker": maker,
                "portfolio": self.portfolio_view(portfolio),
                "reserve": self.maker_reserves[maker],
            })).collect::<Vec<_>>(),
            "settlements": self.settlements.iter().rev().take(8).cloned().collect::<Vec<_>>(),
        });
        if let Some(result) = result {
            value["request"] = json!({
                "asset": result.request.asset,
                "qty": result.request.qty,
                "direction": result.request.direction,
                "is_real": result.request.is_real,
            });
            value["price"] = json!(result.outcome.price);
            value["winner"] = json!(result.outcome.winner);
            value["eligible"] = json!(result.outcome.eligible);
            value["mask"] = json!(result.mask.to_string());
            value["quotes"] = json!(result.outcome.quotes.iter().map(|quote| json!({
                "maker": quote.maker,
                "ask": quote.ask,
                "bid": quote.bid,
                "eligible": quote.eligible,
                "reason": quote.reason,
                "asset": result.used_policies.get(quote.maker).map(|policy| policy.asset).unwrap_or(0),
            })).collect::<Vec<_>>());
        }
        value
    }
}
