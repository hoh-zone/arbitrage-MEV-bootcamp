use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use sui_core::execution_cache::ExecutionCacheWrite;
use sui_types::{
    base_types::ObjectID,
    object::Object,
    storage::ObjectStore,
    transaction::{TransactionData, TransactionDataAPI},
};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::{SimulateCtx, SimulateResult, Simulator};

use super::DBSimulator;

// A special purpose simulator
// to ensure execution is always using latest state
pub struct ReplaySimulator {
    fallback: DBSimulator,

    // channel for notifying us to update
    pub update_notifier: Arc<Sender<()>>,
}

impl ReplaySimulator {
    pub async fn new_slow(
        store_path: &str,
        config_path: &str,
        long_interval: Duration,  // if no tx submitted by us recently, use this interval
        short_interval: Duration, // if we have submitted a tx recently, update more frequently
    ) -> Self {
        let authority_store = DBSimulator::new_authority_store(store_path, config_path).await;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let db_simulator = DBSimulator::new(authority_store, None, None, true).await;
        let cache_writeback = db_simulator.store.clone();

        std::thread::Builder::new()
            .name("replay-update-thread".to_string())
            .spawn(move || Self::spawn_update_loop(rx, cache_writeback, short_interval, long_interval))
            .unwrap();

        Self {
            fallback: db_simulator,
            update_notifier: Arc::new(tx),
        }
    }

    #[tokio::main]
    async fn spawn_update_loop(
        mut receiver: Receiver<()>,
        cache_writeback: Arc<dyn ExecutionCacheWrite>,
        short_interval: Duration,
        long_interval: Duration,
    ) {
        let mut quick_update_times = 0;
        let mut current_interval = long_interval;
        loop {
            // Sleep for the current interval
            tokio::time::sleep(current_interval).await;

            // Check if we received any update notifications
            while receiver.try_recv().is_ok() {
                quick_update_times = 50;
            }

            // Update the cache
            cache_writeback.update_underlying(true);

            // Update interval based on quick_update_times
            if quick_update_times > 0 {
                current_interval = short_interval;
                quick_update_times -= 1;
            } else {
                current_interval = long_interval;
            }
        }
    }
}

#[async_trait]
impl Simulator for ReplaySimulator {
    async fn simulate(&self, tx: TransactionData, ctx: SimulateCtx) -> eyre::Result<SimulateResult> {
        // always make sure gas coins are up to date
        let gas_ids = tx.gas().iter().map(|obj| obj.0).collect::<Vec<_>>();
        let latest = self.fallback.store.store.multi_get_objects(&gas_ids);
        let gas_coins = latest
            .into_iter()
            .filter_map(|obj| obj.map(|o| (o.id(), o)))
            .collect::<Vec<_>>();
        self.fallback.store.reload_cached(gas_coins);

        self.fallback.simulate(tx, ctx).await
    }

    async fn get_object(&self, obj_id: &ObjectID) -> Option<Object> {
        self.fallback.get_object(obj_id).await
    }

    fn name(&self) -> &str {
        "ReplaySimulator"
    }
}
