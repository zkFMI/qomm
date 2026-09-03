//! Stable DeFMI rail identifiers used by every layer of the public demo.
//!
//! Keeping these derivations in one module is important: a portfolio readback,
//! a signed mandate and the MPC settlement must never derive different assets
//! from the same room index.

use sha2::{Digest, Sha256};

pub fn traded_asset_id(asset: i64) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"QOMM:DEMO:TRADED-ASSET:v1")
        .chain_update(asset.to_be_bytes())
        .finalize()
        .into()
}

pub fn cash_asset_id() -> [u8; 32] {
    Sha256::new()
        .chain_update(b"QOMM:DEMO:CASH-ASSET:v1")
        .finalize()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rails_are_stable_and_domain_separated() {
        assert_eq!(traded_asset_id(0), traded_asset_id(0));
        assert_ne!(traded_asset_id(0), traded_asset_id(1));
        assert_ne!(traded_asset_id(0), cash_asset_id());
    }
}
