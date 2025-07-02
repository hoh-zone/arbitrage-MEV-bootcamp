use async_trait::async_trait;
use sui_json_rpc_types::SuiObjectDataOptions;
use sui_sdk::{rpc_types::SuiProtocolConfigValue, SuiClient, SuiClientBuilder};
use sui_types::{base_types::ObjectID, object::Object, transaction::TransactionData};

use super::{SimulateCtx, SimulateResult, Simulator};

#[derive(Clone)]
pub struct HttpSimulator {
    pub client: SuiClient,
}

impl HttpSimulator {
    pub async fn new(url: impl AsRef<str>, ipc_path: &Option<String>) -> Self {
        tracing::warn!("http simulator is deprecated");

        let mut builder = SuiClientBuilder::default().max_concurrent_requests(2000);
        if let Some(ipc_path) = ipc_path {
            builder = builder.ipc_path(ipc_path).ipc_pool_size(100);
        }
        let client = builder.build(url).await.unwrap();

        Self { client }
    }

    pub async fn max_budget(&self) -> u64 {
        let cfg = self
            .client
            .read_api()
            .get_protocol_config(None)
            .await
            .expect("failed to get config");

        let Some(Some(SuiProtocolConfigValue::U64(max))) = cfg.attributes.get("max_tx_gas") else {
            panic!("failed to get max_tx_gas");
        };

        *max
    }
}

#[async_trait]
impl Simulator for HttpSimulator {
    // NOTE: Does not return object_changes
    async fn simulate(&self, tx: TransactionData, ctx: SimulateCtx) -> eyre::Result<SimulateResult> {
        let override_objects = ctx
            .override_objects
            .into_iter()
            .filter_map(|o| o.as_object().map(|obj| (obj.id(), obj.clone())))
            .collect::<Vec<_>>();

        let resp = self
            .client
            .read_api()
            .dry_run_transaction_block_override(tx, override_objects)
            .await?;

        Ok(SimulateResult {
            effects: resp.effects,
            events: resp.events,
            object_changes: vec![],
            balance_changes: resp.balance_changes,
            cache_misses: 0,
        })
    }

    fn name(&self) -> &str {
        "HttpSimulator"
    }

    async fn get_object(&self, obj_id: &ObjectID) -> Option<Object> {
        self.client
            .read_api()
            .get_object_with_options(*obj_id, SuiObjectDataOptions::bcs_lossless())
            .await
            .ok()?
            .data?
            .try_into()
            .ok()
    }
}
