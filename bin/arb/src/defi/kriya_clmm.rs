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
use utils::{
    coin, new_test_sui_client,
    object::{extract_u128_from_move_struct, shared_obj_arg},
};

use super::{trade::FlashResult, TradeCtx, CETUS_AGGREGATOR};
use crate::{config::*, defi::Dex};

const KRIYA_CLMM: &str = "0xbd8d4489782042c6fafad4de4bc6a5e0b84a43c6c00647ffd7062d1e2bb7549e";
const VERSION: &str = "0xf5145a7ac345ca8736cf8c76047d00d6d378f30e81be6f6eb557184d9de93c78";

#[derive(Clone)]
pub struct ObjectArgs {
    version: ObjectArg,
    clock: ObjectArg,
}

static OBJ_CACHE: OnceCell<ObjectArgs> = OnceCell::const_new();

async fn get_object_args(simulator: Arc<Box<dyn Simulator>>) -> ObjectArgs {
    OBJ_CACHE
        .get_or_init(|| async {
            let version_id = ObjectID::from_hex_literal(VERSION).unwrap();
            let version = simulator.get_object(&version_id).await.unwrap();
            let clock = simulator.get_object(&SUI_CLOCK_OBJECT_ID).await.unwrap();

            ObjectArgs {
                version: shared_obj_arg(&version, false),
                clock: shared_obj_arg(&clock, false),
            }
        })
        .await
        .clone()
}

#[derive(Clone)]
pub struct KriyaClmm {
    pool: Pool,
    pool_arg: ObjectArg,
    liquidity: u128,
    coin_in_type: String,
    coin_out_type: String,
    type_params: Vec<TypeTag>,
    version: ObjectArg,
    clock: ObjectArg,
}

impl KriyaClmm {
    pub async fn new(simulator: Arc<Box<dyn Simulator>>, pool: &Pool, coin_in_type: &str) -> Result<Self> {
        ensure!(pool.protocol == Protocol::KriyaClmm, "not a KriyaClmm pool");

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

        let liquidity = extract_u128_from_move_struct(&parsed_pool, "liquidity")?;

        let coin_out_type = if pool.token0_type() == coin_in_type {
            pool.token1_type().to_string()
        } else {
            pool.token0_type().to_string()
        };

        let type_params = parsed_pool.type_.type_params.clone();

        let pool_arg = shared_obj_arg(&pool_obj, true);
        let ObjectArgs { version, clock } = get_object_args(simulator).await;

        Ok(Self {
            pool: pool.clone(),
            liquidity,
            coin_in_type: coin_in_type.to_string(),
            coin_out_type,
            type_params,
            pool_arg,
            version,
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
        pool: &mut Pool<CoinA, CoinB>,
        coin_a: Coin<CoinA>,
        version: &Version,
        clock: &Clock,
        ctx: &mut TxContext,
    ): Coin<CoinB>
    */
    fn build_swap_args(&self, ctx: &mut TradeCtx, coin_in_arg: Argument) -> Result<Vec<Argument>> {
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;
        let version_arg = ctx.obj(self.version).map_err(|e| eyre!(e))?;
        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;

        Ok(vec![pool_arg, coin_in_arg, version_arg, clock_arg])
    }

    /*
    public fun flash_swap<T0, T1>(
        _pool: &mut Pool<T0, T1>,
        _a2b: bool,
        _by_amount_in: bool,
        _amount: u64,
        _sqrt_price_limit: u128,
        _clock: &Clock,
        _version: &Version,
        _ctx: &TxContext
    ) : (Balance<T0>, Balance<T1>, FlashSwapReceipt)
    */
    fn build_flashloan_args(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<Vec<Argument>> {
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;
        let a2b = ctx.pure(self.is_a2b()).map_err(|e| eyre!(e))?;
        let by_amount_in = ctx.pure(true).map_err(|e| eyre!(e))?;
        let amount = ctx.pure(amount_in).map_err(|e| eyre!(e))?;

        let sqrt_price_limit = if self.is_a2b() {
            MIN_SQRT_PRICE_X64
        } else {
            MAX_SQRT_PRICE_X64
        };
        let sqrt_price_limit = ctx.pure(sqrt_price_limit).map_err(|e| eyre!(e))?;

        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;
        let version_arg = ctx.obj(self.version).map_err(|e| eyre!(e))?;

        Ok(vec![
            pool_arg,
            a2b,
            by_amount_in,
            amount,
            sqrt_price_limit,
            clock_arg,
            version_arg,
        ])
    }

    /*
    public fun repay_flash_swap<T0, T1>(
        _pool: &mut Pool<T0, T1>,
        _receipt: FlashSwapReceipt,
        _balance_a: Balance<T0>,
        _balance_b: Balance<T1>,
        _version: &Version,
        _ctx: &TxContext
    )
    */
    fn build_repay_args(&self, ctx: &mut TradeCtx, coin: Argument, receipt: Argument) -> Result<Vec<Argument>> {
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;

        let (balance_a, balance_b) = if self.is_a2b() {
            (
                ctx.coin_into_balance(coin, self.type_params[0].clone())?,
                ctx.balance_zero(self.type_params[1].clone())?,
            )
        } else {
            (
                ctx.balance_zero(self.type_params[0].clone())?,
                ctx.coin_into_balance(coin, self.type_params[1].clone())?,
            )
        };

        let version_arg = ctx.obj(self.version).map_err(|e| eyre!(e))?;
        Ok(vec![pool_arg, receipt, balance_a, balance_b, version_arg])
    }
}

#[async_trait::async_trait]
impl Dex for KriyaClmm {
    fn support_flashloan(&self) -> bool {
        true
    }

    async fn extend_flashloan_tx(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<FlashResult> {
        let package = ObjectID::from_hex_literal(KRIYA_CLMM)?;
        let module = Identifier::new("trade").map_err(|e| eyre!(e))?;
        let function = Identifier::new("flash_swap").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_flashloan_args(ctx, amount_in)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();

        // `flash_swap` returns (Balance<T0>, Balance<T1>, FlashSwapReceipt)
        let (received_balance_in, received_balance_out) = if self.is_a2b() {
            (Argument::NestedResult(last_idx, 0), Argument::NestedResult(last_idx, 1))
        } else {
            (Argument::NestedResult(last_idx, 1), Argument::NestedResult(last_idx, 0))
        };
        let receipt = Argument::NestedResult(last_idx, 2);

        let (coin_in_type, coin_out_type) = if self.is_a2b() {
            (self.type_params[0].clone(), self.type_params[1].clone())
        } else {
            (self.type_params[1].clone(), self.type_params[0].clone())
        };
        ctx.balance_destroy_zero(received_balance_in, coin_in_type)?;
        let coin_out = ctx.coin_from_balance(received_balance_out, coin_out_type)?;
        Ok(FlashResult {
            coin_out,
            receipt,
            pool: None,
        })
    }

    async fn extend_repay_tx(&self, ctx: &mut TradeCtx, coin: Argument, flash_res: FlashResult) -> Result<Argument> {
        let package = ObjectID::from_hex_literal(KRIYA_CLMM)?;
        let module = Identifier::new("trade").map_err(|e| eyre!(e))?;
        let receipt = flash_res.receipt;

        // get repay_amount and split coin
        let repay_amount = {
            // returns (coin_a_debt: u64, coin_b_debt: u64)
            let function = Identifier::new("swap_receipt_debts").map_err(|e| eyre!(e))?;
            let type_arguments = vec![];
            let arguments = vec![receipt];
            ctx.command(Command::move_call(
                package,
                module.clone(),
                function,
                type_arguments,
                arguments,
            ));

            let last_idx = ctx.last_command_idx();
            if self.is_a2b() {
                Argument::NestedResult(last_idx, 0)
            } else {
                Argument::NestedResult(last_idx, 1)
            }
        };
        let repay_coin = ctx.split_coin_arg(coin, repay_amount);

        // repay
        let function = Identifier::new("repay_flash_swap").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_repay_args(ctx, repay_coin, receipt)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        Ok(coin)
    }

    async fn extend_trade_tx(
        &self,
        ctx: &mut TradeCtx,
        _sender: SuiAddress,
        coin_in: Argument,
        _amount_in: Option<u64>,
    ) -> Result<Argument> {
        let function = if self.is_a2b() { "swap_a2b" } else { "swap_b2a" };

        let package = ObjectID::from_hex_literal(CETUS_AGGREGATOR)?;
        let module = Identifier::new("kriya_clmm").map_err(|e| eyre!(e))?;
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
        Protocol::KriyaClmm
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
    async fn test_kriya_clmm_swap_tx() {
        mev_logger::init_console_logger_with_directives(None, &["arb=debug", "dex_indexer=debug"]);

        let http_simulator = HttpSimulator::new(TEST_HTTP_URL, &None).await;

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
            .filter(|dex| dex.protocol() == Protocol::KriyaClmm)
            .sorted_by(|a, b| a.liquidity().cmp(&b.liquidity()))
            .last()
            .unwrap();
        let tx_data = dex.swap_tx(owner, recipient, amount_in).await.unwrap();
        info!("🧀 tx_data: {:?}", tx_data);

        let response = http_simulator.simulate(tx_data, Default::default()).await.unwrap();
        info!("🧀 {:?}", response);
    }
}
