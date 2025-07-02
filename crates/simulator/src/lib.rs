mod db_simulator;
mod http_simulator;

use async_trait::async_trait;
use eyre::Result;
use move_core_types::annotated_value::MoveStructLayout;
use sui_json_rpc_types::{BalanceChange, SuiTransactionBlockEffects, SuiTransactionBlockEvents};
use sui_types::{
    base_types::ObjectID,
    committee::EpochId,
    messages_checkpoint::CheckpointTimestamp,
    object::Object,
    sui_system_state::sui_system_state_summary::SuiSystemStateSummary,
    transaction::{ObjectReadResult, TransactionData},
};

pub use db_simulator::{DBSimulator, ReplaySimulator};
pub use http_simulator::HttpSimulator;

#[derive(Debug, Clone)]
pub struct SimulateResult {
    pub effects: SuiTransactionBlockEffects,
    pub events: SuiTransactionBlockEvents,
    pub object_changes: Vec<ObjectReadResult>,
    pub balance_changes: Vec<BalanceChange>,
    pub cache_misses: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SimEpoch {
    pub epoch_id: EpochId,
    pub epoch_start_timestamp: CheckpointTimestamp,
    pub epoch_duration_ms: u64,
    pub gas_price: u64,
}

impl From<SuiSystemStateSummary> for SimEpoch {
    fn from(summary: SuiSystemStateSummary) -> Self {
        Self {
            epoch_id: summary.epoch,
            epoch_start_timestamp: summary.epoch_start_timestamp_ms,
            epoch_duration_ms: summary.epoch_duration_ms,
            gas_price: summary.reference_gas_price,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SimulateCtx {
    pub epoch: SimEpoch,
    pub override_objects: Vec<ObjectReadResult>,
    // (coin, amount)
    // assume we have this coin (flashloaned) during execution
    pub borrowed_coin: Option<(Object, u64)>,
}

impl SimulateCtx {
    pub fn new(epoch: SimEpoch, override_objects: Vec<ObjectReadResult>) -> Self {
        Self {
            epoch,
            override_objects,
            borrowed_coin: None,
        }
    }

    pub fn with_borrowed_coin(&mut self, borrowed_coin: (Object, u64)) {
        self.borrowed_coin = Some(borrowed_coin);
    }

    pub fn with_gas_price(&mut self, gas_price: u64) {
        self.epoch.gas_price = gas_price;
    }
}

impl SimEpoch {
    pub fn is_stale(&self) -> bool {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64)
            < self.epoch_start_timestamp + self.epoch_duration_ms
    }
}

#[async_trait]
pub trait Simulator: Sync + Send {
    async fn simulate(&self, tx: TransactionData, ctx: SimulateCtx) -> Result<SimulateResult>;
    async fn get_object(&self, obj_id: &ObjectID) -> Option<Object>;
    fn name(&self) -> &str;

    fn get_object_layout(&self, _: &ObjectID) -> Option<MoveStructLayout> {
        None
    }
}
