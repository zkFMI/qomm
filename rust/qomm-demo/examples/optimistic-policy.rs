//! Explicit setup for an isolated public-development network. Uses the
//! repository's public governance keys and synthetic collateral, never a
//! production funding credential. Every mutation goes through native RPC.
use curve25519_dalek::Scalar;
use defmi::{
    avalanche::{AvalancheClient, AvalancheRpcClient},
    facility::{AccountOpening, AssetDefinition, AssetKind, QuorumAuthorizer},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use zkfmi_zk::pedersen::Pedersen;
use zkpi_defmi_sdk::optimistic::*;

fn hash(s: &str) -> [u8; 32] {
    Sha256::digest(s.as_bytes()).into()
}
fn digest(s: &str) -> Result<[u8; 32], String> {
    hex::decode(s)
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|_| "identity must be 32 bytes".into())
}
fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let challenger_mode =
        args.len() == 4 && matches!(args[2].as_str(), "--fund-challenger" | "--challenge");
    if (args.len() != 6 && !challenger_mode) || args[0] != "--public-development" {
        return Err("usage: optimistic-policy --public-development RPC_URL VENUE_HEX PROPOSER_HEX CHALLENGE_SECONDS RESPONSE_SECONDS".into());
    }
    let rpc = AvalancheRpcClient::new(&args[1], Duration::from_secs(90), true)?;
    let network = rpc.call("defmivm.network", json!({}))?;
    let chain = network["chainID"]
        .as_str()
        .ok_or("native chain ID missing")?;
    let keys = defmi::governance::public_development_keys().map_err(|e| e.to_string())?;
    let authorizer = QuorumAuthorizer::new(
        keys.iter()
            .map(|(n, k)| (n.clone(), k.verifying_key()))
            .collect(),
        3,
        1,
        chain,
    )?;
    let signers = keys
        .iter()
        .take(3)
        .map(|(n, k)| (n.clone(), k.clone()))
        .collect::<BTreeMap<_, _>>();
    if challenger_mode {
        // Published fixture key, usable only with this explicitly selected
        // public-development example; never an operator credential.
        let key = zkpi_committee::application_crypto::SigningKey::from_bytes(&[91; 64]);
        let owner = key.identity();
        let client = OptimisticClient { rpc: &rpc };
        if args[2] == "--challenge" {
            let claim_id = digest(&args[3])?;
            let receipt = client.challenge(&Challenge::signed(claim_id, &key)?)?;
            println!(
                "{}",
                json!({"challenger":hex::encode(owner),"claim_id":hex::encode(claim_id),
                "tx_id":receipt.tx_id,"height":receipt.height,"after_root":hex::encode(receipt.after_root),
                "claim":client.claim(claim_id)?})
            );
            return Ok(());
        }
        let policy = client.policy(digest(&args[3])?)?;
        let opening = AccountOpening {
            handle: owner,
            asset_id: policy.bond_asset,
            commitment: Pedersen::new(b"qomm:defmi:v1")
                .commit(&Scalar::from(1000u64), &Scalar::ZERO)
                .compress()
                .to_bytes(),
            issuance_nonce: hash("QOMM:OPTIMISTIC:RESEARCH-CHALLENGER:v1"),
        };
        let before = rpc.state_root()?;
        let approval = authorizer.approve(opening.statement()?, before, &signers)?;
        let tx = rpc.issue_account(&opening, &approval, before)?;
        let account = rpc.wait_accepted(&tx, Duration::MAX, Duration::from_millis(100))?;
        let transfer = BondTransfer {
            owner,
            asset: policy.bond_asset,
            amount: 200,
            before_balance: 1000,
            blinding: Scalar::ZERO.to_bytes(),
            withdraw: false,
        };
        let approval = authorizer.approve(
            command_digest("bond", &transfer)?,
            rpc.state_root()?,
            &signers,
        )?;
        let bond = client.transfer_bond(&transfer, &approval)?;
        println!(
            "{}",
            json!({"challenger":hex::encode(owner),"account_tx":account.tx_id,"bond_tx":bond.tx_id,
            "bond":rpc.call("defmivm.optimisticBond",json!({"assetID":hex::encode(policy.bond_asset),"ownerID":hex::encode(owner)}))?})
        );
        return Ok(());
    }
    let asset = AssetDefinition {
        asset_id: hash("QOMM:OPTIMISTIC:RESEARCH-COLLATERAL:v1"),
        code: "OPT-RESEARCH".into(),
        kind: AssetKind::Cash,
        decimals: 0,
        terms_digest: hash("Synthetic public-development collateral; no economic value"),
    };
    let before = rpc.state_root()?;
    let approval = authorizer.approve(asset.statement()?, before, &signers)?;
    let tx = rpc.issue_asset(&asset, &approval, before)?;
    let asset_receipt = rpc.wait_accepted(&tx, Duration::MAX, Duration::from_millis(100))?;
    let owner = digest(&args[3])?;
    let opening = AccountOpening {
        handle: owner,
        asset_id: asset.asset_id,
        commitment: Pedersen::new(b"qomm:defmi:v1")
            .commit(&Scalar::from(1000u64), &Scalar::ZERO)
            .compress()
            .to_bytes(),
        issuance_nonce: hash("QOMM:OPTIMISTIC:RESEARCH-OPENING:v1"),
    };
    let before = rpc.state_root()?;
    let approval = authorizer.approve(opening.statement()?, before, &signers)?;
    let tx = rpc.issue_account(&opening, &approval, before)?;
    let account_receipt = rpc.wait_accepted(&tx, Duration::MAX, Duration::from_millis(100))?;
    let client = OptimisticClient { rpc: &rpc };
    let transfer = BondTransfer {
        owner,
        asset: asset.asset_id,
        amount: 200,
        before_balance: 1000,
        blinding: Scalar::ZERO.to_bytes(),
        withdraw: false,
    };
    let approval = authorizer.approve(
        command_digest("bond", &transfer)?,
        rpc.state_root()?,
        &signers,
    )?;
    let bond_receipt = client.transfer_bond(&transfer, &approval)?;
    let policy = OptimisticPolicy {
        network: hash(&format!("QOMM:DEMO:DEFMI:{chain}")),
        application: digest(&args[2])?,
        verifier: QuoteChallengeVerifier.verifier_id(),
        proposer: owner,
        bond_asset: asset.asset_id,
        proposer_bond: 100,
        challenger_bond: 10,
        challenge_window_seconds: args[4].parse::<u64>().map_err(|e| e.to_string())?,
        response_window_seconds: args[5].parse::<u64>().map_err(|e| e.to_string())?,
    };
    let approval = authorizer.approve(
        command_digest("enroll", &policy)?,
        rpc.state_root()?,
        &signers,
    )?;
    let policy_receipt = client.enroll(&policy, &approval)?;
    let saved = client.policy(policy.digest()?)?;
    let bond = rpc.call(
        "defmivm.optimisticBond",
        json!({"assetID":hex::encode(asset.asset_id),"ownerID":hex::encode(owner)}),
    )?;
    let receipts=[asset_receipt,account_receipt,bond_receipt,policy_receipt].iter().map(|r|json!({"tx_id":r.tx_id,"block_id":r.block_id,"height":r.height,"statement":hex::encode(r.statement),"before_root":hex::encode(r.before_root),"after_root":hex::encode(r.after_root)})).collect::<Vec<_>>();
    println!("{}",serde_json::to_string_pretty(&json!({"scope":"isolated public-development network only","policy_id":hex::encode(saved.digest()?),"policy":saved,"bond":bond,"receipts":receipts})).map_err(|e|e.to_string())?);
    Ok(())
}
