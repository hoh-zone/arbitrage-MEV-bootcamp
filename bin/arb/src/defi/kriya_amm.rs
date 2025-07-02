use std::sync::Arc;

use dex_indexer::types::{Pool, Protocol};
use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::annotated_value::MoveStruct;
use simulator::Simulator;
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    transaction::{Argument, Command, ObjectArg, ProgrammableTransaction, TransactionData},
    Identifier, TypeTag,
};
use utils::{coin, new_test_sui_client, object::*};

use super::{TradeCtx, CETUS_AGGREGATOR};
use crate::{config::*, defi::Dex};

#[derive(Clone)]
pub struct KriyaAmm {
    pool: Pool,
    pool_arg: ObjectArg,
    liquidity: u128,
    coin_in_type: String,
    coin_out_type: String,
    type_params: Vec<TypeTag>,
}

impl KriyaAmm {
    pub async fn new(simulator: Arc<Box<dyn Simulator>>, pool: &Pool, coin_in_type: &str) -> Result<Self> {
        ensure!(pool.protocol == Protocol::KriyaAmm, "not a KriyaAmm pool");

        let pool_obj = simulator
            .get_object(&pool.pool)
            .await
            .ok_or_else(|| eyre!("pool not found: {}", pool.pool))?;

        let parsed_pool = {
            let layout = simulator
                .get_object_layout(&pool.pool)
                .ok_or_eyre("pool layout not found")?;

            let move_obj = pool_obj.data.try_as_move().ok_or_eyre("not a move object")?;
            MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
        };

        let is_swap_enabled = extract_bool_from_move_struct(&parsed_pool, "is_swap_enabled")?;
        ensure!(is_swap_enabled, "swap is not enabled");

        let liquidity = {
            let lsp_supply = extract_struct_from_move_struct(&parsed_pool, "lsp_supply")?;
            extract_u64_from_move_struct(&lsp_supply, "value")? as u128
        };

        let coin_out_type = if pool.token0_type() == coin_in_type {
            pool.token1_type().to_string()
        } else {
            pool.token0_type().to_string()
        };

        let type_params = parsed_pool.type_.type_params.clone();

        let pool_arg = shared_obj_arg(&pool_obj, true);

        Ok(Self {
            pool: pool.clone(),
            pool_arg,
            liquidity,
            coin_in_type: coin_in_type.to_string(),
            coin_out_type,
            type_params,
        })
    }

    async fn build_swap_tx(
        &self,
        sender: SuiAddress,
        recipient: SuiAddress,
        coin_in: ObjectRef,
        amount_in: u64,
    ) -> Result<ProgrammableTransaction> {
        let mut ctx = TradeCtx::default();

        let coin_in = ctx.split_coin(coin_in, amount_in)?;
        let coin_out = self.extend_trade_tx(&mut ctx, sender, coin_in, None).await?;
        ctx.transfer_arg(recipient, coin_out);

        Ok(ctx.ptb.finish())
    }

    /*
    public fun swap_a2b<CoinA, CoinB>(
        pool: &mut Pool<CoinA, CoinB>,
        coin_a: Coin<CoinA>,
        ctx: &mut TxContext,
    ): Coin<CoinB>
    */
    fn build_swap_args(&self, ctx: &mut TradeCtx, coin_in_arg: Argument) -> Result<Vec<Argument>> {
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;

        Ok(vec![pool_arg, coin_in_arg])
    }
}

#[async_trait::async_trait]
impl Dex for KriyaAmm {
    async fn extend_trade_tx(
        &self,
        ctx: &mut TradeCtx,
        _sender: SuiAddress,
        coin_in: Argument,
        _amount_in: Option<u64>,
    ) -> Result<Argument> {
        let function = if self.is_a2b() { "swap_a2b" } else { "swap_b2a" };

        let package = ObjectID::from_hex_literal(CETUS_AGGREGATOR)?;
        let module = Identifier::new("kriya_amm").map_err(|e| eyre!(e))?;
        let function = Identifier::new(function).map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_swap_args(ctx, coin_in)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

    fn coin_in_type(&self) -> String {
        self.coin_in_type.clone()
    }

    fn coin_out_type(&self) -> String {
        self.coin_out_type.clone()
    }

    fn protocol(&self) -> Protocol {
        Protocol::KriyaAmm
    }

    fn liquidity(&self) -> u128 {
        self.liquidity
    }

    fn object_id(&self) -> ObjectID {
        self.pool.pool
    }

    fn flip(&mut self) {
        std::mem::swap(&mut self.coin_in_type, &mut self.coin_out_type);
    }

    fn is_a2b(&self) -> bool {
        self.pool.token_index(&self.coin_in_type) == Some(0)
    }

    // For testing
    async fn swap_tx(&self, sender: SuiAddress, recipient: SuiAddress, amount_in: u64) -> Result<TransactionData> {
        let sui = new_test_sui_client().await;

        let coin_in = coin::get_coin(&sui, sender, &self.coin_in_type, amount_in).await?;

        let pt = self
            .build_swap_tx(sender, recipient, coin_in.object_ref(), amount_in)
            .await?;

        let gas_coins = coin::get_gas_coin_refs(&sui, sender, Some(coin_in.coin_object_id)).await?;
        let gas_price = sui.read_api().get_reference_gas_price().await?;
        let tx_data = TransactionData::new_programmable(sender, gas_coins, pt, GAS_BUDGET, gas_price);

        Ok(tx_data)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use itertools::Itertools;
    use object_pool::ObjectPool;
    use simulator::{DBSimulator, HttpSimulator, Simulator};
    use tracing::info;

    use super::*;
    use crate::{
        config::tests::{TEST_ATTACKER, TEST_HTTP_URL},
        defi::{indexer_searcher::IndexerDexSearcher, DexSearcher},
    };

    #[tokio::test]
    async fn test_kriya_amm_swap_tx() {
        mev_logger::init_console_logger_with_directives(None, &["arb=debug", "dex_indexer=debug"]);

        let http_simulator = HttpSimulator::new(TEST_HTTP_URL, &None).await;

        let owner = SuiAddress::from_str(TEST_ATTACKER).unwrap();
        let recipient =
            SuiAddress::from_str("0x0cbe287984143ef232336bb39397bd10607fa274707e8d0f91016dceb31bb829").unwrap();
        let token_in_type = "0x2::sui::SUI";
        let token_out_type = "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN";
        let amount_in = 10000;

        let simulator_pool = Arc::new(ObjectPool::new(1, move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { Box::new(DBSimulator::new_test(true).await) as Box<dyn Simulator> })
        }));

        // find dexes and swap
        let searcher = IndexerDexSearcher::new(TEST_HTTP_URL, simulator_pool).await.unwrap();
        let dexes = searcher
            .find_dexes(token_in_type, Some(token_out_type.into()))
            .await
            .unwrap();
        info!("🧀 dexes_len: {}", dexes.len());
        let dex = dexes
            .into_iter()
            .filter(|dex| dex.protocol() == Protocol::KriyaAmm)
            .sorted_by(|a, b| a.liquidity().cmp(&b.liquidity()))
            .last()
            .unwrap();
        let tx_data = dex.swap_tx(owner, recipient, amount_in).await.unwrap();
        info!("🧀 tx_data: {:?}", tx_data);

        let response = http_simulator.simulate(tx_data, Default::default()).await.unwrap();
        info!("🧀 {:?}", response);
    }
}
