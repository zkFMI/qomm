//! Demo custody model.
//!
//! The browser never mutates balances.  It asks the Rust room to register a
//! Maker policy or submit a Taker request, and this module keeps the resulting
//! available/reserved split.  Values are deliberately plain integers because
//! the demo explains the state machine; the production DeFMI path represents
//! the same values as commitments and account-free notes.

use serde::Serialize;

pub const STARTING_CASH: i64 = 50_000_000_000;
pub const STARTING_INVENTORY: i64 = 5_000;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Portfolio {
    pub cash_available: i64,
    pub cash_reserved: i64,
    pub inventory_available: Vec<i64>,
    pub inventory_reserved: Vec<i64>,
}

impl Portfolio {
    pub fn funded(n_assets: usize) -> Self {
        Self {
            cash_available: STARTING_CASH,
            cash_reserved: 0,
            inventory_available: vec![STARTING_INVENTORY; n_assets],
            inventory_reserved: vec![0; n_assets],
        }
    }

    pub fn cash_total(&self) -> Result<i64, String> {
        self.cash_available
            .checked_add(self.cash_reserved)
            .ok_or_else(|| "cash total overflowed".to_string())
    }

    pub fn inventory_total(&self, asset: usize) -> Result<i64, String> {
        let available = *self
            .inventory_available
            .get(asset)
            .ok_or_else(|| "unknown portfolio asset".to_string())?;
        let reserved = *self
            .inventory_reserved
            .get(asset)
            .ok_or_else(|| "unknown portfolio asset".to_string())?;
        available
            .checked_add(reserved)
            .ok_or_else(|| "inventory total overflowed".to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.cash_available < 0
            || self.cash_reserved < 0
            || self.inventory_available.len() != self.inventory_reserved.len()
            || self
                .inventory_available
                .iter()
                .chain(self.inventory_reserved.iter())
                .any(|value| *value < 0)
        {
            return Err("portfolio conservation invariant failed".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct MakerReserve {
    pub asset: usize,
    pub inventory: i64,
    pub cash: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TakerReservation {
    pub round: u64,
    pub asset: usize,
    pub direction: i64,
    pub quantity: i64,
    pub limit_price: i64,
    pub amount: i64,
    pub rail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SettlementRecord {
    pub round: u64,
    pub status: String,
    pub reason_code: String,
    pub detail: String,
    pub maker: Option<usize>,
    pub asset: usize,
    pub direction: i64,
    pub quantity: i64,
    pub price: Option<i64>,
    pub cash: Option<i64>,
    pub limit_price: i64,
    pub automatic: bool,
    pub state_root: String,
}
