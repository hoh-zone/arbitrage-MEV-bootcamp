use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use burberry::ActionSubmitter;
use eyre::{bail, ensure, Context, OptionExt, Result};
use object_pool::ObjectPool;
use simulator::{ReplaySimulator, SimulateCtx, Simulator};
use sui_json_rpc_types::SuiTransactionBlockEffectsAPI;
use sui_sdk::SuiClient;
use sui_types::{
    base_types::{ObjectID, SuiAddress},
    object::Owner,
    transaction::{GasData, TransactionData, TransactionDataAPI},
};
use tracing::{error, info, instrument};
use utils::coin;

use crate::{
    arb::{Arb, ArbResult},
    common::notification::new_tg_messages,
    types::{Action, Source},
};

use super::arb_cache::ArbItem;

pub struct Worker {
    pub _id: usize,
    pub sender: SuiAddress,

    pub arb_item_receiver: async_channel::Receiver<ArbItem>,

    pub simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>,
    pub simulator_name: String,

    pub dedicated_simulator: Option<Arc<ReplaySimulator>>,

    pub submitter: Arc<dyn ActionSubmitter<Action>>,
    pub sui: SuiClient,
    pub arb: Arc<Arb>,
}

impl Worker {
    #[tokio::main]
    pub async fn run(mut self) -> Result<()> {
        loop {
            tokio::select! {
                arb_item = self.arb_item_receiver.recv() => {
                    if let Err(error) = self.handle_arb_item(arb_item.context("arb_item channel error")?).await {
                        error!(?error, "Handle arb_item failed");
                    }
                }
                else => bail!("strategy channels undefined behavior"),
            }
        }
    }

    #[instrument(skip_all, fields(coin = %arb_item.coin.split("::").nth(2).unwrap_or(&arb_item.coin), tx = %arb_item.tx_digest))]
    pub async fn handle_arb_item(&mut self, arb_item: ArbItem) -> Result<()> {
        let ArbItem {
            coin,
            pool_id,
            tx_digest,
            sim_ctx,
            source,
        } = arb_item;

        if let Some((arb_result, elapsed)) = arbitrage_one_coin(
            self.arb.clone(),
            self.sender,
            &coin,
            pool_id,
            sim_ctx.clone(),
            false,
            source,
        )
        .await
        {
            let tx_data = match self.dry_run_tx_data(arb_result.tx_data.clone(), sim_ctx.clone()).await {
                Ok(tx_data) => tx_data,
                Err(error) => {
                    error!(?arb_result, ?error, "Dry run final tx_data failed");
                    return Ok(());
                }
            };

            let arb_tx_digest = tx_data.digest();
            let action = match arb_result.source {
                Source::Shio { bid_amount, .. } => Action::ShioSubmitBid((tx_data, bid_amount, tx_digest)),
                _ => Action::ExecutePublicTx(tx_data),
            };

            self.submitter.submit(action);

            let tg_msgs = new_tg_messages(tx_digest, arb_tx_digest, &arb_result, elapsed, &self.simulator_name);
            for tg_msg in tg_msgs {
                self.submitter.submit(tg_msg.into());
            }

            // notify dedicated simulator to update more frequently
            if let Some(dedicated_sim) = &self.dedicated_simulator {
                dedicated_sim.update_notifier.send(()).await.unwrap();
            }
        }

        Ok(())
    }

    // return a final tx_data with latest versions
    async fn dry_run_tx_data(&self, tx_data: TransactionData, sim_ctx: SimulateCtx) -> Result<TransactionData> {
        let tx_data: TransactionData = self.fix_object_refs(tx_data).await?;

        let resp = if let Some(dedicated_sim) = &self.dedicated_simulator {
            dedicated_sim.simulate(tx_data.clone(), sim_ctx).await?
        } else {
            self.simulator_pool.get().simulate(tx_data.clone(), sim_ctx).await?
        };

        let status = &resp.effects.status();
        ensure!(status.is_ok(), "Dry run result: {:?}", status);

        let bc = &resp
            .balance_changes
            .into_iter()
            .find(|bc| bc.owner == Owner::AddressOwner(self.sender))
            .ok_or_eyre("No balance change for attacker")?;
        ensure!(bc.amount > 0, "Attacker's balance not increased {:?}", bc);

        Ok(tx_data)
    }

    // Fetch the latest object ref for gas coins.
    // otherwise we need to wait until the index api to return the correct gas coins
    async fn fix_object_refs(&self, tx_data: TransactionData) -> Result<TransactionData> {
        let gas_coins = coin::get_gas_coin_refs(&self.sui, self.sender, None).await?;

        let mut tx_data = tx_data;
        let gas_data: &mut GasData = tx_data.gas_data_mut();
        gas_data.payment = gas_coins;

        Ok(tx_data)
    }
}

async fn arbitrage_one_coin(
    arb: Arc<Arb>,
    attacker: SuiAddress,
    coin_type: &str,
    pool_id: Option<ObjectID>,
    sim_ctx: SimulateCtx,
    use_gss: bool,
    source: Source,
) -> Option<(ArbResult, Duration)> {
    let start = Instant::now();
    let arb_result = match arb
        .find_opportunity(attacker, coin_type, pool_id, vec![], sim_ctx, use_gss, source)
        .await
    {
        Ok(r) => r,
        Err(error) => {
            let elapsed = start.elapsed();
            if elapsed > Duration::from_secs(1) {
                info!(elapsed = ?elapsed, %coin_type, "🥱 \x1b[31mNo opportunity: {error:#}\x1b[0m");
            } else {
                info!(elapsed = ?elapsed, %coin_type, "🥱 No opportunity: {error:#}");
            }
            return None;
        }
    };

    info!(
        elapsed = ?start.elapsed(),
        elapsed.ctx_creation = ?arb_result.create_trial_ctx_duration,
        elapsed.grid_search = ?arb_result.grid_search_duration,
        elapsed.gss = ?arb_result.gss_duration,
        cache_misses = ?arb_result.cache_misses,
        coin = %coin_type,
        "💰 Profitable opportunity found: {:?}",
        &arb_result.best_trial_result
    );

    Some((arb_result, start.elapsed()))
}
