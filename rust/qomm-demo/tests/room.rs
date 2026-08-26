use qomm_demo::protocol::LIE_PRODUCT;
use qomm_demo::room::{DemoConfig, Room};
use serde_json::json;

fn room_with_seats() -> Room {
    let mut room = Room::new(8, 9, 2, true, 4).unwrap();
    assert!(room.claim("t", "taker", "Ann").0);
    assert!(room.claim("m", "maker:2", "Bo").0);
    assert!(room.claim("n", "node:4", "Rin").0);
    assert!(room.claim("o", "observer", "").0);
    room
}

#[test]
fn node_is_not_told_price_order_mask_or_any_policy() {
    let mut room = room_with_seats();
    let result = room.run_round().unwrap();
    let blob = serde_json::to_string(&room.view("n", &DemoConfig::default(), 0.0)).unwrap();
    assert!(result.outcome.price.is_some());
    assert!(!blob.contains(&result.outcome.price.unwrap().to_string()));
    assert!(!blob.contains(&format!("\"qty\":{}", result.request.qty)));
    assert!(!blob.contains(&result.mask.to_string()));
    assert!(!blob.contains("\"policy\""));
}

#[test]
fn maker_receives_only_own_policy_and_fill_only_after_taker_announces() {
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
        room.set_policy(
            maker,
            &json!({"active": if maker == 0 {1} else {0}, "asset": 0, "maxqty": 500}),
        )
        .unwrap();
    }
    room.set_request(&json!({"asset":0,"qty":100,"is_real":1}))
        .unwrap();
    let result = room.run_round().unwrap();
    assert_eq!(result.outcome.winner, Some(0));
    assert!(room.view("m", &DemoConfig::default(), 0.0)["maker"]["fill"].is_null());
    room.announce();
    assert_eq!(
        room.view("m", &DemoConfig::default(), 0.0)["maker"]["fill"]["price"],
        result.outcome.price.unwrap()
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
    let result = room.run_round().unwrap();
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
    for maker in 0..2 {
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
    room.set_request(&json!({"asset":0,"qty":80,"direction":0,"is_real":1}))
        .unwrap();
    let result = room.run_round().unwrap();
    assert_eq!(result.outcome.winner, Some(0));
    room.settle_last();
    assert_eq!(room.policies[0].inv, 10);
    assert!(room.last.as_ref().unwrap().announced);

    room.policies[0].inv = 0;
    room.set_request(&json!({"asset":0,"qty":80,"direction":0,"is_real":0}))
        .unwrap();
    room.run_round().unwrap();
    room.settle_last();
    assert_eq!(room.policies[0].inv, 0);
    assert!(!room.last.as_ref().unwrap().announced);
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
