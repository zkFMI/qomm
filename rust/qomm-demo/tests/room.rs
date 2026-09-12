use qomm_demo::protocol::LIE_PRODUCT;
use qomm_demo::room::{DemoConfig, Room};
use qomm_demo::{
    model::{Outcome, Policy, Request},
    mpc::{
        MpcQuoteEngine, MpcRound, MpcSettlementInputs, QueuedMpcRound, CORPORATE_QUEUE_RECONCILING,
        CORPORATE_QUEUE_UNAVAILABLE,
    },
};
use serde_json::json;
use std::collections::BTreeMap;

struct ReplayOnce {
    pending: Option<QueuedMpcRound>,
}

struct QueueUnavailable;

#[test]
fn completed_order_releases_only_taker_and_keeps_private_frozen_receipt() {
    let mut room = Room::new(4, 7, 2, true, 42).unwrap();
    let config = DemoConfig::default();
    assert!(room.claim("first", "taker", "First").0);
    assert!(room.claim("maker", "maker:0", "Maker").0);
    room.set_request(&json!({"asset":0,"qty":1,"direction":0,"is_real":1,"limit_price":16000})).unwrap();
    room.begin_round().unwrap();
    room.finish_round().unwrap();
    assert!(room.seat_of("first").is_some());
    room.end_round();
    assert!(room.seat_of("first").is_none());
    assert!(room.seat_of("maker").is_some());
    let receipt = room.view("first", &config, 0.0)["completed_taker"].clone();
    assert!(receipt.is_object());
    assert!(room.view("unrelated", &config, 0.0)["completed_taker"].is_null());
    assert!(room.claim("next", "taker", "Next").0);
    room.play_round().unwrap();
    assert_eq!(room.view("first", &config, 0.0)["completed_taker"], receipt);
}

impl MpcQuoteEngine for QueueUnavailable {
    fn name(&self) -> &'static str {
        "mpc"
    }

    fn note(&self) -> String {
        "test corporate queue".into()
    }

    fn robust(&self) -> bool {
        true
    }

    fn robust_reason(&self) -> &str {
        ""
    }

    fn input_check(&self) -> bool {
        true
    }

    fn quote(
        &mut self,
        _policies: &[Policy],
        _request: &Request,
        _settlement: &MpcSettlementInputs,
        _now: i64,
        _corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        Err(CORPORATE_QUEUE_UNAVAILABLE.into())
    }
}

impl MpcQuoteEngine for ReplayOnce {
    fn name(&self) -> &'static str {
        "mpc"
    }

    fn note(&self) -> String {
        "test replay".into()
    }

    fn robust(&self) -> bool {
        true
    }

    fn robust_reason(&self) -> &str {
        ""
    }

    fn input_check(&self) -> bool {
        true
    }

    fn replay_queued(&mut self) -> Result<Option<QueuedMpcRound>, String> {
        Ok(self.pending.take())
    }

    fn quote(
        &mut self,
        _policies: &[Policy],
        _request: &Request,
        _settlement: &MpcSettlementInputs,
        _now: i64,
        _corrupt: &[usize],
    ) -> Result<MpcRound, String> {
        Err("synchronous quote was not expected".into())
    }
}

fn room_with_seats() -> Room {
    let mut room = Room::new(8, 9, 2, true, 4).unwrap();
    assert!(room.claim("t", "taker", "Ann").0);
    assert!(room.claim("m", "maker:2", "Bo").0);
    assert!(room.claim("n", "node:4", "Rin").0);
    assert!(room.claim("o", "observer", "").0);
    room
}

#[test]
fn automatic_maker_refresh_keeps_the_registered_instrument() {
    let mut room = Room::new(4, 7, 2, true, 4).unwrap();
    let registered_assets = room
        .policies
        .iter()
        .map(|policy| policy.asset)
        .collect::<Vec<_>>();

    for _ in 0..2_000 {
        room.step_bots().unwrap();
        assert_eq!(
            room.policies
                .iter()
                .map(|policy| policy.asset)
                .collect::<Vec<_>>(),
            registered_assets
        );
    }
}

#[test]
fn participant_capacities_replace_placeholder_balances_before_execution() {
    let mut room = Room::new(4, 7, 2, true, 9).unwrap();
    room.configure_participant_capacities(&[(50_000_000_000, 5_000); 4], (3_000_000, 300))
        .unwrap();

    assert_eq!(room.taker_portfolio.cash_total().unwrap(), 3_000_000);
    for asset in 0..room.assets.len() {
        assert_eq!(room.taker_portfolio.inventory_total(asset).unwrap(), 300);
    }
    for maker in 0..room.n_makers {
        assert_eq!(
            room.maker_portfolios[maker].cash_total().unwrap(),
            50_000_000_000
        );
        assert_eq!(
            room.maker_portfolios[maker]
                .inventory_total(room.maker_reserves[maker].asset)
                .unwrap(),
            5_000
        );
    }
}

#[test]
fn canonical_taker_holdings_replace_genesis_capacity_without_changing_makers() {
    let mut room = Room::new(4, 7, 2, true, 10).unwrap();
    room.configure_participant_capacities(&[(50_000_000_000, 5_000); 4], (3_000_000, 300))
        .unwrap();
    let canonical_inventory = (0..room.assets.len())
        .map(|asset| 300_u64 + u64::try_from(asset).unwrap())
        .collect::<Vec<_>>();

    room.configure_taker_canonical_portfolio(2_750_000, &canonical_inventory)
        .unwrap();

    assert_eq!(room.taker_portfolio.cash_available, 2_750_000);
    assert_eq!(room.taker_portfolio.cash_reserved, 0);
    assert_eq!(
        room.taker_portfolio.inventory_available,
        canonical_inventory
            .iter()
            .map(|value| i64::try_from(*value).unwrap())
            .collect::<Vec<_>>()
    );
    assert!(room
        .maker_portfolios
        .iter()
        .all(|portfolio| portfolio.cash_total().unwrap() == 50_000_000_000));
}

#[test]
fn corporate_queue_replays_without_a_second_browser_submission() {
    let mut room = Room::new(2, 9, 2, true, 44).unwrap();
    let policies = room.policies.clone();
    let request = Request {
        asset: 0,
        qty: 17,
        direction: 0,
        entity: 0,
        is_real: 1,
    };
    let settlement = MpcSettlementInputs {
        user_limit: 20_000,
        taker_securities_reserve: 0,
        taker_cash_reserve: 340_000,
        maker_securities_reserves: vec![500, 500],
        maker_cash_reserves: vec![1_000_000, 1_000_000],
    };
    let engine = ReplayOnce {
        pending: Some(QueuedMpcRound {
            policies: policies.clone(),
            request: request.clone(),
            settlement,
            market_time: 7,
            round: MpcRound {
                outcome: Outcome::default(),
                filled: false,
                masked_key: 9,
                mask: 8,
                node_shares: BTreeMap::new(),
                named: BTreeMap::new(),
                verified: true,
                detail: "verified replay".into(),
                stats: json!({"distributed": true}),
                product_handoff: None,
            },
        }),
    };
    room.install_mpc_engine(engine).unwrap();

    assert!(room.drain_mpc_queue().unwrap());
    assert_eq!(room.phase, "done");
    assert_eq!(room.last.as_ref().unwrap().request, request);
    assert!(!room.last.as_ref().unwrap().filled);
    assert_eq!(room.last.as_ref().unwrap().used_policies, policies);
    assert_eq!(room.settlements.last().unwrap().status, "released");
    assert_eq!(room.settlements.last().unwrap().reason_code, "no_maker");
    assert_eq!(
        room.last.as_ref().unwrap().engine_stats["corporate_queue_replay"]["automatic"],
        true
    );
    assert!(!room.drain_mpc_queue().unwrap());
}

#[test]
fn replay_reconciliation_keeps_the_local_reserve_projection() {
    let mut room = Room::new(1, 9, 2, true, 46).unwrap();
    let policies = room.policies.clone();
    let request = Request {
        asset: 0,
        qty: 10,
        direction: 0,
        entity: 0,
        is_real: 1,
    };
    let pending =
        format!("{CORPORATE_QUEUE_RECONCILING}: validator rejected a transient settlement");
    room.install_mpc_engine(ReplayOnce {
        pending: Some(QueuedMpcRound {
            policies,
            request,
            settlement: MpcSettlementInputs {
                user_limit: 20_000,
                taker_securities_reserve: 0,
                taker_cash_reserve: 200_000,
                maker_securities_reserves: vec![500],
                maker_cash_reserves: vec![1_000_000],
            },
            market_time: 7,
            round: MpcRound {
                outcome: Outcome::default(),
                filled: false,
                masked_key: 0,
                mask: 0,
                node_shares: BTreeMap::new(),
                named: BTreeMap::new(),
                verified: false,
                detail: pending,
                stats: json!({"corporate_reconciliation": true}),
                product_handoff: None,
            },
        }),
    })
    .unwrap();

    let available_before = room.taker_portfolio.cash_available;
    assert!(room.drain_mpc_queue().unwrap());

    assert_eq!(room.last.as_ref().unwrap().abort_code, "queued");
    assert_eq!(room.settlements.last().unwrap().status, "queued");
    assert_eq!(room.taker_portfolio.cash_reserved, 200_000);
    assert_eq!(
        room.taker_portfolio.cash_available,
        available_before - 200_000
    );
}

#[test]
fn corporate_queue_keeps_the_taker_reserve_until_replay() {
    let mut room = Room::new(1, 9, 2, true, 45).unwrap();
    room.set_forced_manual("taker", true);
    room.set_forced_manual("maker:0", true);
    room.set_request(&json!({
        "asset": 0,
        "qty": 10,
        "direction": 0,
        "is_real": 1,
        "limit_price": 20_000
    }))
    .unwrap();
    room.install_mpc_engine(QueueUnavailable).unwrap();

    let available_before = room.taker_portfolio.cash_available;
    room.play_round().unwrap();

    assert_eq!(room.last.as_ref().unwrap().abort_code, "queued");
    assert_eq!(room.settlements.last().unwrap().status, "queued");
    assert_eq!(room.settlements.last().unwrap().reason_code, "mpc_queued");
    let held = room
        .taker_reservation
        .as_ref()
        .expect("queued RFQ retains its Taker reservation");
    assert_eq!(held.amount, 200_000);
    assert_eq!(room.taker_portfolio.cash_reserved, held.amount);
    assert_eq!(
        room.taker_portfolio.cash_available,
        available_before - held.amount
    );
}

#[test]
fn node_is_not_told_price_order_mask_or_any_policy() {
    let mut room = room_with_seats();
    let result = room.run_round().unwrap();
    let view = room.view("n", &DemoConfig::default(), 0.0);
    let blob = serde_json::to_string(&view).unwrap();
    assert!(result.outcome.price.is_some());
    assert!(!blob.contains(&result.outcome.price.unwrap().to_string()));
    assert!(!blob.contains(&format!("\"qty\":{}", result.request.qty)));
    assert!(!blob.contains(&result.mask.to_string()));
    assert!(!blob.contains("\"policy\""));
    assert!(!blob.contains("\"portfolio\""));
    assert!(!blob.contains("cash_available"));
    assert_eq!(view["node"]["custody"], false);
}

#[test]
fn maker_receives_only_own_policy_and_is_filled_automatically_after_match() {
    let mut room = room_with_seats();
    room.set_policy(2, &json!({"ask_level": 7, "spread": 33}))
        .unwrap();
    room.set_policy(5, &json!({"ask_level": -21, "spread": 44}))
        .unwrap();
    room.run_round().unwrap();
    let view = room.view("m", &DemoConfig::default(), 0.0);
    assert_eq!(view["maker"]["policy"]["ask_level"], 7);
    let blob = serde_json::to_string(&view).unwrap();
    assert!(!blob.contains("-21") && !blob.contains("\"spread\":44"));

    let mut room = Room::new(8, 9, 2, true, 11).unwrap();
    room.claim("m", "maker:0", "");
    room.claim("t", "taker", "");
    for maker in 0..8 {
        room.set_forced_manual(&format!("maker:{maker}"), true);
        room.set_policy(
            maker,
            &json!({"active": if maker == 0 {1} else {0}, "asset": 0, "maxqty": 500}),
        )
        .unwrap();
    }
    room.set_request(&json!({"asset":0,"qty":100,"is_real":1,"limit_price":100_000}))
        .unwrap();
    let result = room.play_round().unwrap();
    assert_eq!(result.outcome.winner, Some(0));
    assert_eq!(
        room.view("m", &DemoConfig::default(), 0.0)["maker"]["fill"]["price"],
        result.outcome.price.unwrap()
    );
    assert_eq!(
        room.view("m", &DemoConfig::default(), 0.0)["maker"]["settlement"]["status"],
        "settled"
    );
}

#[test]
fn behaviour_is_private_to_node_and_observer_and_taker_alone_unpacks() {
    let mut room = room_with_seats();
    room.set_behaviour(4, LIE_PRODUCT).unwrap();
    let node = room.view("n", &DemoConfig::default(), 0.0);
    assert_eq!(
        node["seats"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|seat| seat.get("behaviour").is_some())
            .count(),
        1
    );
    let taker = room.view("t", &DemoConfig::default(), 0.0);
    assert_eq!(
        taker["seats"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|seat| seat.get("behaviour").is_some())
            .count(),
        0
    );
    let observer = room.view("o", &DemoConfig::default(), 0.0);
    assert_eq!(
        observer["seats"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|seat| seat.get("behaviour").is_some())
            .count(),
        9
    );

    room.set_behaviour(4, "honest").unwrap();
    room.set_request(&json!({"limit_price": 100_000})).unwrap();
    let result = room.run_round().unwrap();
    assert!(result.filled);
    assert_eq!(
        result.unpack(),
        (result.outcome.cost, result.outcome.winner)
    );
    assert_eq!(
        room.view("n", &DemoConfig::default(), 0.0)["public"]["masked_key"],
        result.masked_key.to_string()
    );
    assert_ne!(result.masked_key, i128::from(result.outcome.cost.unwrap()));
}

#[test]
fn seat_cannot_be_taken_twice_and_second_claim_releases_first() {
    let mut room = Room::new(8, 9, 2, true, 1).unwrap();
    assert!(room.claim("a", "node:0", "Ann").0);
    let refused = room.claim("b", "node:0", "Bo");
    assert!(!refused.0 && refused.1.contains("Ann"));
    assert_eq!(room.seats["node:0"].mode(), "manual");
    room.release("a");
    assert_eq!(room.seats["node:0"].mode(), "auto");
    assert!(room.claim("b", "node:0", "Bo").0);

    room.claim("a", "maker:1", "");
    room.claim("a", "node:2", "");
    assert!(room.seats["maker:1"].holder.is_none());
    assert_eq!(room.seats["node:2"].holder.as_deref(), Some("a"));
}

#[test]
fn automatic_real_fill_moves_the_book_and_cover_does_not() {
    let mut room = Room::new(2, 9, 2, true, 12).unwrap();
    room.set_forced_manual("taker", true);
    for maker in 0..2 {
        room.set_forced_manual(&format!("maker:{maker}"), true);
        room.set_policy(
            maker,
            &json!({
                "active": if maker == 0 {1} else {0},
                "asset": 0,
                "ask_level": 0,
                "inv": 0,
                "maxqty": 500
            }),
        )
        .unwrap();
    }
    room.set_request(&json!({"asset":0,"qty":80,"direction":0,"is_real":1,"limit_price":100_000}))
        .unwrap();
    let maker_cash_before = room.maker_portfolios[0].cash_total().unwrap();
    let taker_inventory_before = room.taker_portfolio.inventory_total(0).unwrap();
    let result = room.play_round().unwrap();
    assert_eq!(result.outcome.winner, Some(0));
    assert_eq!(room.policies[0].inv, 10);
    assert!(room.last.as_ref().unwrap().settled);
    let cash = 80 * result.outcome.price.unwrap();
    assert_eq!(
        room.maker_portfolios[0].cash_total().unwrap(),
        maker_cash_before + cash
    );
    assert_eq!(
        room.taker_portfolio.inventory_total(0).unwrap(),
        taker_inventory_before + 80
    );

    room.policies[0].inv = 0;
    room.set_request(&json!({"asset":0,"qty":80,"direction":0,"is_real":0}))
        .unwrap();
    let maker_before_cover = room.maker_portfolios[0].clone();
    let taker_before_cover = room.taker_portfolio.clone();
    room.play_round().unwrap();
    assert_eq!(room.policies[0].inv, 0);
    assert!(!room.last.as_ref().unwrap().settled);
    assert_eq!(room.maker_portfolios[0], maker_before_cover);
    assert_eq!(room.taker_portfolio, taker_before_cover);
    assert_eq!(room.settlements.last().unwrap().status, "cover");
}

#[test]
fn price_limit_releases_the_taker_hold_and_insufficient_cash_refuses_submission() {
    let mut room = Room::new(1, 9, 2, true, 27).unwrap();
    room.set_forced_manual("taker", true);
    room.set_forced_manual("maker:0", true);
    room.set_policy(
        0,
        &json!({"active":1,"asset":0,"ask_level":0,"slope":0,"invcoef":0,"maxqty":500}),
    )
    .unwrap();
    room.set_request(&json!({"asset":0,"qty":10,"direction":0,"is_real":1,"limit_price":1}))
        .unwrap();
    let before = room.taker_portfolio.clone();
    room.play_round().unwrap();
    assert_eq!(room.settlements.last().unwrap().status, "released");
    assert_eq!(room.taker_portfolio, before);
    assert!(room.taker_reservation.is_none());

    room.taker_portfolio.cash_available = 5;
    room.set_request(&json!({"asset":0,"qty":10,"direction":0,"is_real":1,"limit_price":100}))
        .unwrap();
    let error = room.play_round().unwrap_err();
    assert!(error.contains("signed limit needs"));
    assert_eq!(room.taker_portfolio.cash_available, 5);
    assert_eq!(room.taker_portfolio.cash_reserved, 0);
}

#[test]
fn a_round_is_replayed_as_phases_and_settles_at_the_settle_phase() {
    let mut room = Room::new(2, 9, 2, true, 21).unwrap();
    room.set_forced_manual("taker", true);
    for maker in 0..2 {
        room.set_forced_manual(&format!("maker:{maker}"), true);
        room.set_policy(
            maker,
            &json!({"active": 1, "asset": 0, "ask_level": 0, "inv": 0, "maxqty": 500}),
        )
        .unwrap();
    }
    room.set_request(&json!({"asset":0,"qty":50,"direction":0,"is_real":1,"limit_price":100_000}))
        .unwrap();
    let cash_before = room.taker_portfolio.cash_available;
    assert_eq!(room.view("x", &DemoConfig::default(), 0.0)["phase"], "idle");

    let phases = room.begin_round().unwrap();
    assert_eq!(
        phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        ["deal", "check", "reduce", "open"]
    );
    assert!(room.busy);
    assert_eq!(room.phase, "deal");
    assert_eq!(phases[0].fields["values"], 5 + 2 * 10);
    assert_eq!(phases[0].fields["nodes"], 9);
    assert_eq!(phases[2].fields["degree"], 4);
    assert_eq!(phases[2].fields["products"], 4);
    // two products per policy, plus the final opening
    assert_eq!(phases[2].fields["reductions"], 5);
    assert_eq!(
        phases[3].fields["masked_key"],
        room.last.as_ref().unwrap().masked_key.to_string()
    );
    // The reserve is taken, and nothing has settled yet while phases are shown.
    assert!(room.taker_portfolio.cash_reserved > 0);
    assert_eq!(
        room.taker_portfolio.cash_available,
        cash_before - room.taker_portfolio.cash_reserved
    );
    assert!(room.settlements.is_empty());
    assert_eq!(
        room.begin_round().unwrap_err(),
        "a round is already in progress"
    );

    for phase in &phases[1..] {
        room.set_phase(phase);
        let view = room.view("x", &DemoConfig::default(), 0.0);
        assert_eq!(view["phase"], phase.name);
        assert_eq!(view["busy"], true);
        assert_eq!(view["phase_fields"], json!(phase.fields));
    }

    room.finish_round().unwrap();
    assert_eq!(room.phase, "settle");
    assert_eq!(room.phase_fields["status"], "settled");
    assert_eq!(
        room.phase_fields["state_root"],
        room.settlements.last().unwrap().state_root
    );
    assert_eq!(room.taker_portfolio.cash_reserved, 0);
    assert!(room.busy);

    room.end_round();
    assert_eq!(room.phase, "done");
    assert!(!room.busy);
    let view = room.view("x", &DemoConfig::default(), 0.0);
    assert_eq!(view["phase_fields"]["number"], 1);
    assert_eq!(view["busy"], false);
}

#[test]
fn phase_replay_stops_where_the_round_stopped() {
    let mut room = Room::new(2, 9, 2, true, 22).unwrap();
    room.set_behaviour(3, "offline").unwrap();
    let absent = room.run_round().unwrap();
    let phases = room.phases_of(&absent);
    assert_eq!(
        phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        ["deal", "check"]
    );
    assert_eq!(phases[1].fields["why"], "absent");

    room.set_behaviour(3, "lie_input").unwrap();
    let refused = room.run_round().unwrap();
    let phases = room.phases_of(&refused);
    assert_eq!(
        phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        ["deal", "check"]
    );
    assert_eq!(phases[1].fields["rejected"], json!([3]));

    room.set_behaviour(3, LIE_PRODUCT).unwrap();
    let corrected = room.run_round().unwrap();
    let phases = room.phases_of(&corrected);
    assert_eq!(
        phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        ["deal", "check", "reduce", "open"]
    );
    assert_eq!(phases[2].fields["named"], json!([3]));
    assert!(phases[2].note.contains("named node 3"));

    room.configure_input_check(false).unwrap();
    room.set_behaviour(3, "honest").unwrap();
    let unchecked = room.run_round().unwrap();
    let phases = room.phases_of(&unchecked);
    assert_eq!(phases[1].fields["skipped"], true);

    // Phase fields are broadcast to every seat, so they must carry nothing a
    // node may not know: no price, no quantity, no winner.
    room.claim("n", "node:4", "");
    let mut room_with_phase = room;
    room_with_phase.set_phase(&phases[3]);
    let blob =
        serde_json::to_string(&room_with_phase.view("n", &DemoConfig::default(), 0.0)).unwrap();
    assert!(!blob.contains(&format!("\"qty\":{}", unchecked.request.qty)));
    assert!(!blob.contains(&unchecked.outcome.price.unwrap().to_string()));
}

#[test]
fn views_carry_bounded_history_and_only_their_own_notices() {
    let mut room = Room::new(2, 9, 2, true, 13).unwrap();
    room.claim("maker", "maker:0", "M");
    room.claim("observer", "observer", "");
    for _ in 0..10 {
        room.run_round().unwrap();
    }
    let maker = room.view("maker", &DemoConfig::default(), 0.0);
    let observer = room.view("observer", &DemoConfig::default(), 0.0);
    assert_eq!(maker["history"].as_array().unwrap().len(), 8);
    assert!(maker["notices"].as_array().unwrap().len() <= 12);
    assert_eq!(observer["history"].as_array().unwrap().len(), 8);
    assert_ne!(maker["notices"], observer["notices"]);
}
