//! Seats and server-side projections: a browser receives only its own business.

use crate::bots::{maker_filled, step_maker, step_taker};
use crate::model::{Policy, Request, BUY, FIELDS, SELL};
use crate::mpc::MpcEngine;
use crate::protocol::{Session, BEHAVIOURS, HONEST, LIE_PRODUCT};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use serde_json::{json, Map, Value};
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
    pub announced: bool,
    pub node_shares: BTreeMap<usize, Vec<String>>,
    pub used_policies: Vec<Policy>,
    pub verified: Option<bool>,
    pub verified_detail: String,
    pub engine_stats: Value,
}

impl RoundResult {
    pub fn unpack(&self) -> (Option<i64>, Option<usize>) {
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
    pub behaviours: BTreeMap<usize, String>,
    pub request: Request,
    pub seats: BTreeMap<String, Seat>,
    pub sessions: BTreeMap<String, String>,
    pub last: Option<RoundResult>,
    pub history: Vec<RoundResult>,
    pub notices: BTreeMap<String, Vec<Notice>>,
    pub engine: Option<MpcEngine>,
    pub phase: String,
    pub phase_note: String,
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
                expiry: 1_000_000_000,
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
        Ok(Self {
            assets,
            n_makers,
            n_nodes,
            threshold,
            input_check,
            policies,
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
            round_number: 0,
            now: 0,
            rng,
        })
    }

    pub fn install_mpc_engine(&mut self, engine: MpcEngine) -> Result<(), String> {
        if engine.input_check() != self.input_check {
            return Err("the MP-SPDZ circuit input-check shape differs from the room".into());
        }
        self.engine = Some(engine);
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
                    "Rust share layer and qomm-zk decoder; tournament cleartext".into(),
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

    pub fn set_policy(&mut self, maker: usize, values: &Value) -> Result<(), String> {
        let policy = self
            .policies
            .get_mut(maker)
            .ok_or_else(|| "unknown maker".to_string())?;
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
        policy.maxqty = policy.maxqty.max(0);
        policy.spread = policy.spread.max(2);
        Ok(())
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
        self.request.qty = self.request.qty.max(1);
        self.request.direction = if self.request.direction == 0 {
            BUY
        } else {
            SELL
        };
        self.request.is_real = i64::from(self.request.is_real != 0);
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
        self.phase = "deal".into();
        let started = Instant::now();
        let request = self.request.clone();
        let policies = self.policies.clone();
        let reference = self
            .assets
            .iter()
            .map(|asset| asset.reference)
            .collect::<Vec<_>>();
        let result = if self.engine.is_some() {
            let robust = self.engine.as_ref().is_some_and(MpcEngine::robust);
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
            match self
                .engine
                .as_mut()
                .expect("checked MPC engine")
                .quote(&policies, &request, self.now, &corrupt)
            {
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
                        masked_key: round.masked_key,
                        mask: round.mask,
                        padded: self.padded(),
                        named: round.named,
                        rejected: Vec::new(),
                        reductions: 0,
                        corrections,
                        aborted,
                        abort_reason: aborted.then(|| round.detail.clone()).unwrap_or_default(),
                        abort_code: aborted.then_some("mismatch".into()).unwrap_or_default(),
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
                        announced: false,
                        node_shares: round.node_shares,
                        used_policies: policies,
                        verified: Some(round.verified),
                        verified_detail: round.detail,
                        engine_stats: stats,
                    }
                }
                Err(error) => RoundResult {
                    number: self.round_number,
                    engine: "mpc".into(),
                    request,
                    outcome: crate::model::Outcome::default(),
                    masked_key: i128::from(self.rng.gen::<u32>()),
                    mask: self.rng.gen::<u32>() as u64,
                    padded: self.padded(),
                    named: BTreeMap::new(),
                    rejected: Vec::new(),
                    reductions: 0,
                    corrections: 0,
                    aborted: true,
                    abort_reason: error.clone(),
                    abort_code: "engine".into(),
                    abort_fields: BTreeMap::new(),
                    product_capacity: 0,
                    open_capacity: 0,
                    silent: Vec::new(),
                    corrupted_inputs: Vec::new(),
                    input_check: self.input_check,
                    elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                    announced: false,
                    node_shares: BTreeMap::new(),
                    used_policies: policies,
                    verified: Some(false),
                    verified_detail: error.clone(),
                    engine_stats: json!({"error": error}),
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
                    i128::from(cost) * self.padded() as i128 + winner as i128 + i128::from(mask)
                }
            } else {
                i128::from(self.rng.gen::<u32>())
            };
            RoundResult {
                number: self.round_number,
                engine: "sim".into(),
                request,
                outcome: protocol.outcome,
                masked_key,
                mask,
                padded: self.padded(),
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
                announced: false,
                node_shares: protocol.node_shares,
                used_policies: protocol.used_policies,
                verified: None,
                verified_detail: String::new(),
                engine_stats: json!({}),
            }
        };
        self.phase = "done".into();
        self.phase_note = format!("round {}", result.number);
        self.last = Some(result.clone());
        self.history.push(result.clone());
        if self.history.len() > 20 {
            self.history.drain(..self.history.len() - 20);
        }
        if result.aborted {
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

    pub fn announce(&mut self) {
        let Some((winner, number)) = self.last.as_mut().and_then(|last| {
            last.outcome.winner.map(|winner| {
                last.announced = true;
                (winner, last.number)
            })
        }) else {
            return;
        };
        self.note(
            &format!("maker:{winner}"),
            "you_won",
            "good",
            json!({"number": number}),
        );
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
    pub fn step_bots(&mut self) {
        for maker in 0..self.n_makers {
            if self.seats[&format!("maker:{maker}")].mode() == "auto" {
                step_maker(&mut self.policies[maker], self.assets.len(), &mut self.rng);
            }
        }
        if self.seats[TAKER].mode() == "auto" {
            step_taker(&mut self.request, self.assets.len(), 0.35, &mut self.rng);
        }
    }

    /// A cover round computes identically but has no settlement side effect.
    pub fn settle_last(&mut self) {
        let Some((winner, request, aborted)) = self.last.as_ref().and_then(|result| {
            result
                .outcome
                .winner
                .map(|winner| (winner, result.request.clone(), result.aborted))
        }) else {
            return;
        };
        if aborted || request.is_real == 0 {
            return;
        }
        if self.seats[&format!("maker:{winner}")].mode() == "auto" {
            maker_filled(&mut self.policies[winner], &request);
        }
        if self.seats[TAKER].mode() == "auto" {
            self.announce();
        }
    }

    pub fn play_round(&mut self) -> Result<RoundResult, String> {
        self.step_bots();
        self.run_round()?;
        self.settle_last();
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
            (
                "next_round_in".into(),
                if self.seats[TAKER].mode() == "manual" {
                    Value::Null
                } else {
                    json!(next_round_in.max(0.0))
                },
            ),
            ("public".into(), self.public_view(result)),
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
            "verified_detail": result.verified_detail,
            "engine_stats": result.engine_stats,
            "inert": result.engine_stats.get("inert_behaviours").is_some(),
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
            "pending": {
                "asset": self.request.asset,
                "qty": self.request.qty,
                "direction": self.request.direction,
                "is_real": self.request.is_real,
            }
        });
        if let Some(result) = result {
            let (cost, maker) = result.unpack();
            value["last"] = json!({
                "number": result.number,
                "asset": result.request.asset,
                "qty": result.request.qty,
                "direction": result.request.direction,
                "is_real": result.request.is_real,
                "mask": result.mask.to_string(),
                "price": result.outcome.price,
                "winner": result.outcome.winner,
                "unpacked_cost": cost,
                "unpacked_maker": maker,
                "eligible": result.outcome.eligible,
                "announced": result.announced,
            });
        }
        value
    }

    fn maker_view(&self, maker: usize, result: Option<&RoundResult>) -> Value {
        let mut value = json!({
            "policy": self.policies[maker],
            "fields": FIELDS,
            "fill": Value::Null,
            "told_nothing": true,
        });
        if let Some(result) = result {
            if result.outcome.winner == Some(maker) && result.announced && !result.aborted {
                value["fill"] = json!({
                    "number": result.number,
                    "asset": result.request.asset,
                    "qty": result.request.qty,
                    "direction": result.request.direction,
                    "price": result.outcome.price,
                });
                value["told_nothing"] = json!(false);
            }
        }
        value
    }

    fn node_view(&self, node: usize, result: Option<&RoundResult>) -> Value {
        let Some(result) = result else {
            return json!({
                "behaviour": self.behaviours[&node], "shares": [],
                "named_me": false, "times_named": 0,
            });
        };
        json!({
            "behaviour": self.behaviours[&node],
            "shares": result.node_shares.get(&node).cloned().unwrap_or_default(),
            "named_me": result.named.contains_key(&node),
            "times_named": result.named.get(&node).copied().unwrap_or(0),
            "rejected_me": result.rejected.iter().any(|(candidate,_,_)| *candidate == node),
            "silent_me": result.silent.contains(&node),
        })
    }

    fn observer_view(&self, result: Option<&RoundResult>) -> Value {
        let mut value = json!({
            "behaviours": self.behaviours.iter().map(|(node,behaviour)| (node.to_string(),json!(behaviour))).collect::<Map<_,_>>(),
            "policies": self.policies,
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
