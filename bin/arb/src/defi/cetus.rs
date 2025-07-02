use std::sync::Arc;

use dex_indexer::types::{Pool, Protocol};
use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::annotated_value::MoveStruct;
use simulator::Simulator;
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    transaction::{Argument, Command, ObjectArg, ProgrammableTransaction, TransactionData},
    Identifier, TypeTag, SUI_CLOCK_OBJECT_ID,
};
use tokio::sync::OnceCell;
use utils::{coin, new_test_sui_client, object::*};

use super::{trade::FlashResult, TradeCtx};
use crate::{config::*, defi::Dex};

const CETUS_DEX: &str = "0xeffc8ae61f439bb34c9b905ff8f29ec56873dcedf81c7123ff2f1f67c45ec302";
const CONFIG: &str = "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";
const PARTNER: &str = "0x639b5e433da31739e800cd085f356e64cae222966d0f1b11bd9dc76b322ff58b";

#[derive(Clone)]
pub struct ObjectArgs {
    config: ObjectArg,
    partner: ObjectArg,
    clock: ObjectArg,
}

static OBJ_CACHE: OnceCell<ObjectArgs> = OnceCell::const_new();

async fn get_object_args(simulator: Arc<Box<dyn Simulator>>) -> ObjectArgs {
    OBJ_CACHE
        .get_or_init(|| async {
            let config_id = ObjectID::from_hex_literal(CONFIG).unwrap();
            let partner_id = ObjectID::from_hex_literal(PARTNER).unwrap();

            let config = simulator.get_object(&config_id).await.unwrap();
            let partner = simulator.get_object(&partner_id).await.unwrap();
            let clock = simulator.get_object(&SUI_CLOCK_OBJECT_ID).await.unwrap();

            ObjectArgs {
                config: shared_obj_arg(&config, false),
                partner: shared_obj_arg(&partner, true),
                clock: shared_obj_arg(&clock, false),
            }
        })
        .await
        .clone()
}

#[derive(Clone)]
pub struct Cetus {
    pool: Pool,
    pool_arg: ObjectArg,
    liquidity: u128,
    coin_in_type: String,
    coin_out_type: String,
    type_params: Vec<TypeTag>,
    config: ObjectArg,
    partner: ObjectArg,
    clock: ObjectArg,
}

impl Cetus {
    pub async fn new(simulator: Arc<Box<dyn Simulator>>, pool: &Pool, coin_in_type: &str) -> Result<Self> {
        ensure!(pool.protocol == Protocol::Cetus, "not a Cetus pool");

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

        let is_pause = extract_bool_from_move_struct(&parsed_pool, "is_pause")?;
        ensure!(!is_pause, "pool is paused");

        let liquidity = extract_u128_from_move_struct(&parsed_pool, "liquidity")?;

        let coin_out_type = if pool.token0_type() == coin_in_type {
            pool.token1_type().to_string()
        } else {
            pool.token0_type().to_string()
        };

        let type_params = parsed_pool.type_.type_params.clone();

        let pool_arg = shared_obj_arg(&pool_obj, true);
        let ObjectArgs { config, partner, clock } = get_object_args(simulator).await;

        Ok(Self {
            pool: pool.clone(),
            liquidity,
            coin_in_type: coin_in_type.to_string(),
            coin_out_type,
            type_params,
            pool_arg,
            config,
            partner,
            clock,
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
    fun swap_a2b<CoinA, CoinB>(
        config: &GlobalConfig,
        pool: &mut Pool<CoinA, CoinB>,
        partner: &mut Partner,
        coin_a: Coin<CoinA>,
        clock: &Clock,
        ctx: &mut TxContext
    ): Coin<CoinB>
    */
    fn build_swap_args(&self, ctx: &mut TradeCtx, coin_in_arg: Argument) -> Result<Vec<Argument>> {
        let config_arg = ctx.obj(self.config).map_err(|e| eyre!(e))?;
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;
        let partner_arg = ctx.obj(self.partner).map_err(|e| eyre!(e))?;
        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;

        Ok(vec![config_arg, pool_arg, partner_arg, coin_in_arg, clock_arg])
    }

    /*
    public fun flash_swap_a2b<CoinA, CoinB>(
        config: &GlobalConfig,
        pool: &mut Pool<CoinA, CoinB>,
        partner: &mut Partner,
        amount: u64,
        by_amount_in: bool,
        clock: &Clock,
        ctx: &mut TxContext
    ): (Coin<CoinB>, FlashSwapReceipt<CoinA, CoinB>, u64) {
    */
    fn build_flashloan_args(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<Vec<Argument>> {
        let config_arg = ctx.obj(self.config).map_err(|e| eyre!(e))?;

        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;
        let partner_arg = ctx.obj(self.partner).map_err(|e| eyre!(e))?;

        let amount = ctx.pure(amount_in).map_err(|e| eyre!(e))?;
        let by_amount_in = ctx.pure(true).map_err(|e| eyre!(e))?;

        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;

        Ok(vec![config_arg, pool_arg, partner_arg, amount, by_amount_in, clock_arg])
    }

    /*
    public fun repay_flash_swap_a2b<CoinA, CoinB>(
        config: &GlobalConfig,
        pool: &mut Pool<CoinA, CoinB>,
        partner: &mut Partner,
        coin_a: Coin<CoinA>,
        receipt: FlashSwapReceipt<CoinA, CoinB>,
        ctx: &mut TxContext,
    ): Coin<CoinA>;
    */
    fn build_repay_args(&self, ctx: &mut TradeCtx, coin: Argument, receipt: Argument) -> Result<Vec<Argument>> {
        let config_arg = ctx.obj(self.config).map_err(|e| eyre!(e))?;
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;
        let partner_arg = ctx.obj(self.partner).map_err(|e| eyre!(e))?;

        Ok(vec![config_arg, pool_arg, partner_arg, coin, receipt])
    }
}

#[async_trait::async_trait]
impl Dex for Cetus {
    fn support_flashloan(&self) -> bool {
        true
    }

    async fn extend_flashloan_tx(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<FlashResult> {
        let function = if self.is_a2b() {
            "flash_swap_a2b"
        } else {
            "flash_swap_b2a"
        };

        let package = ObjectID::from_hex_literal(CETUS_DEX)?;
        let module = Identifier::new("cetus").map_err(|e| eyre!(e))?;
        let function = Identifier::new(function).map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_flashloan_args(ctx, amount_in)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();

        Ok(FlashResult {
            coin_out: Argument::NestedResult(last_idx, 0),
            receipt: Argument::NestedResult(last_idx, 1),
            pool: None,
        })
    }

    async fn extend_repay_tx(&self, ctx: &mut TradeCtx, coin: Argument, flash_res: FlashResult) -> Result<Argument> {
        let function = if self.is_a2b() {
            "repay_flash_swap_a2b"
        } else {
            "repay_flash_swap_b2a"
        };

        let package = ObjectID::from_hex_literal(CETUS_DEX)?;
        let module = Identifier::new("cetus").map_err(|e| eyre!(e))?;
        let function = Identifier::new(function).map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_repay_args(ctx, coin, flash_res.receipt)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

    async fn extend_trade_tx(
        &self,
        ctx: &mut TradeCtx,
        _sender: SuiAddress,
        coin_in: Argument,
        _amount_in: Option<u64>,
    ) -> Result<Argument> {
        let function = if self.is_a2b() { "swap_a2b" } else { "swap_b2a" };

        let package = ObjectID::from_hex_literal(CETUS_DEX)?;
        let module = Identifier::new("cetus").map_err(|e| eyre!(e))?;
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
        Protocol::Cetus
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
    use std::{str::FromStr, time::Instant};

    use itertools::Itertools;
    use object_pool::ObjectPool;
    use simulator::{DBSimulator, SimulateCtx, Simulator};
    use sui_sdk::SuiClientBuilder;
    use tracing::info;

    use super::*;
    use crate::{
        common::get_latest_epoch,
        config::tests::{TEST_ATTACKER, TEST_HTTP_URL},
        defi::{indexer_searcher::IndexerDexSearcher, DexSearcher},
    };

    // cargo test --package arb --bin arb --all-features -- defi::cetus::tests::test_cetus_swap_tx --exact --show-output
    #[tokio::test]
    async fn test_cetus_swap_tx() {
        mev_logger::init_console_logger_with_directives(None, &["arb=debug", "dex_indexer=debug"]);

        let owner = SuiAddress::from_str(TEST_ATTACKER).unwrap();
        let recipient =
            SuiAddress::from_str("0x0cbe287984143ef232336bb39397bd10607fa274707e8d0f91016dceb31bb829").unwrap();
        let token_in_type = "0x2::sui::SUI";
        let token_out_type = "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP";
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
            .filter(|dex| dex.protocol() == Protocol::Cetus)
            .sorted_by(|a, b| a.liquidity().cmp(&b.liquidity()))
            .last()
            .unwrap();
        let tx_data = dex.swap_tx(owner, recipient, amount_in).await.unwrap();
        info!("🧀 tx_data: {:?}", tx_data);

        let start = Instant::now();
        let db_sim = DBSimulator::new_slow(
            "/home/ubuntu/sui-nick/db/live/store",
            "/home/ubuntu/sui-nick/fullnode.yaml",
            None,
            None,
        )
        .await;
        info!("DBSimulator::new cost: {:?}", start.elapsed());

        let sui = SuiClientBuilder::default().build(TEST_HTTP_URL).await.unwrap();
        let epoch = get_latest_epoch(&sui).await.unwrap();
        let ctx = SimulateCtx::new(epoch, vec![]);

        let start = Instant::now();
        let db_res = db_sim.simulate(tx_data, ctx).await.unwrap();
        info!("🧀 DB simulate cost {:?}, {:?}", start.elapsed(), db_res);
    }
}
