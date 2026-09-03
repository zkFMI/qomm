use qomm_demo::mpc::MpcEngine;
use qomm_demo::room::Room;

/// This is an explicit deployment gate rather than a normal unit test: it
/// needs an MP-SPDZ checkout, its official compiler, and the native libSPDZ
/// engine. Run it with `--ignored --test-threads=1` on the native target.
#[test]
#[ignore = "requires MP_SPDZ_ROOT and a native MP-SPDZ build"]
fn seven_party_mpc_engine_opens_one_consistent_verified_result() {
    let root = std::env::var("MP_SPDZ_ROOT").expect("MP_SPDZ_ROOT is required");
    let mut room = Room::new(8, 7, 2, true, 7).expect("room configuration");
    let references = room
        .assets
        .iter()
        .map(|asset| asset.reference)
        .collect::<Vec<_>>();
    let engine = MpcEngine::new(root, 7, 2, 8, &references, 63, true)
        .expect("compile the real MP-SPDZ program");
    room.install_mpc_engine(engine)
        .expect("install the real MP-SPDZ engine");

    // Use the same pre-trade path as the product: the Taker signs and reserves
    // its maximum before the request enters MPC. Calling `run_round()` directly
    // would intentionally bypass that boundary and must fail closed.
    let result = room
        .play_round()
        .expect("reserve and run one real seven-party round");
    assert!(!result.aborted, "{}", result.verified_detail);
    assert_eq!(result.verified, Some(true), "{}", result.verified_detail);
    assert!(result.outcome.winner.is_some());
    assert!(result.outcome.price.is_some());
}
