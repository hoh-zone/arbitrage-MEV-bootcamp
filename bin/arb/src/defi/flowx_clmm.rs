use std::{str::FromStr, sync::Arc};

use dex_indexer::types::{Pool, PoolExtra, Protocol};
use eyre::{bail, ensure, eyre, OptionExt, Result};
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

use super::{trade::FlashResult, TradeCtx};
use crate::{config::*, defi::Dex};

const FLOWX_CLMM: &str = "0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d";
const VERSIONED: &str = "0x67624a1533b5aff5d0dfcf5e598684350efd38134d2d245f475524c03a64e656";
const POOL_REGISTRY: &str = "0x27565d24a4cd51127ac90e4074a841bbe356cca7bf5759ddc14a975be1632abc";

static OBJ_CACHE: OnceCell<ObjectArgs> = OnceCell::const_new();

async fn get_object_args(simulator: Arc<Box<dyn Simulator>>) -> ObjectArgs {
    OBJ_CACHE
        .get_or_init(|| async {
            let pool_registry_id = ObjectID::from_hex_literal(POOL_REGISTRY).unwrap();
            let versioned_id = ObjectID::from_hex_literal(VERSIONED).unwrap();

            let pool_registry = simulator.get_object(&pool_registry_id).await.unwrap();
            let versioned = simulator.get_object(&versioned_id).await.unwrap();
            let clock = simulator.get_object(&SUI_CLOCK_OBJECT_ID).await.unwrap();

            ObjectArgs {
                pool_registry: shared_obj_arg(&pool_registry, true),
                versioned: shared_obj_arg(&versioned, false),
                clock: shared_obj_arg(&clock, false),
            }
        })
        .await
        .clone()
}

#[derive(Clone)]
pub struct ObjectArgs {
    pool_registry: ObjectArg,
    versioned: ObjectArg,
    clock: ObjectArg,
}

#[derive(Clone)]
pub struct FlowxClmm {
    pool: Pool,
    liquidity: u128,
    coin_in_type: String,
    coin_out_type: String,
    fee: u64,
    type_params: Vec<TypeTag>,
    pool_registry: ObjectArg,
    versioned: ObjectArg,
    clock: ObjectArg,
}

impl FlowxClmm {
    pub async fn new(simulator: Arc<Box<dyn Simulator>>, pool: &Pool, coin_in_type: &str) -> Result<Self> {
        ensure!(pool.protocol == Protocol::FlowxClmm, "not a FlowxClmm pool");

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

        let coin_out_type = if let Some(0) = pool.token_index(coin_in_type) {
            pool.token1_type()
        } else {
            pool.token0_type()
        };

        let fee = if let PoolExtra::FlowxClmm { fee_rate } = pool.extra {
            fee_rate
        } else {
            bail!("invalid pool extra");
        };

        let type_params = vec![
            TypeTag::from_str(coin_in_type).map_err(|e| eyre!(e))?,
            TypeTag::from_str(&coin_out_type).map_err(|e| eyre!(e))?,
        ];

        let ObjectArgs {
            pool_registry,
            versioned,
            clock,
        } = get_object_args(simulator).await;

        Ok(Self {
            pool: pool.clone(),
            liquidity,
            coin_in_type: coin_in_type.to_string(),
            coin_out_type,
            fee,
            type_params,
            pool_registry,
            versioned,
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
    fun swap_exact_input<X, Y>(
        pool_registry: &mut PoolRegistry,
        fee: u64,
        coin_in: Coin<X>,
        amount_out_min: u64,
        sqrt_price_limit: u128,
        deadline: u64,
        versioned: &mut Versioned,
        clock: &Clock,
        ctx: &mut TxContext
    ): Coin<Y>
    */
    fn build_swap_args(&self, ctx: &mut TradeCtx, coin_in_arg: Argument) -> Result<Vec<Argument>> {
        let pool_registry_arg = ctx.obj(self.pool_registry).map_err(|e| eyre!(e))?;
        let fee = ctx.pure(self.fee).map_err(|e| eyre!(e))?;
        let amount_out_min = ctx.pure(0u64).map_err(|e| eyre!(e))?;

        let sqrt_price_limit = if self.is_a2b() {
            MIN_SQRT_PRICE_X64 + 1
        } else {
            MAX_SQRT_PRICE_X64 - 1
        };

        let sqrt_price_limit = ctx.pure(sqrt_price_limit).map_err(|e| eyre!(e))?;
        let deadline = ctx
            .pure(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
                    + 18000,
            )
            .map_err(|e| eyre!(e))?;

        let versioned_arg = ctx.obj(self.versioned).map_err(|e| eyre!(e))?;
        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;

        Ok(vec![
            pool_registry_arg,
            fee,
            coin_in_arg,
            amount_out_min,
            sqrt_price_limit,
            deadline,
            versioned_arg,
            clock_arg,
        ])
    }

    /*
    public fun swap<T0, T1>(
        _pool: &mut Pool<T0, T1>,
        _a2b: bool,
        _by_amount_in: bool,
        _amount: u64,
        _sqrt_price_limit: u128,
        _versioned: &Versioned,
        _clock: &Clock,
        _ctx: &TxContext
    ) : (Balance<T0>, Balance<T1>, SwapReceipt);
    */
    fn build_flashloan_args(&self, ctx: &mut TradeCtx, pool_arg: Argument, amount_in: u64) -> Result<Vec<Argument>> {
        let a2b = ctx.pure(self.is_a2b()).map_err(|e| eyre!(e))?;
        let by_amount_in = ctx.pure(true).map_err(|e| eyre!(e))?;
        let amount = ctx.pure(amount_in).map_err(|e| eyre!(e))?;

        let sqrt_price_limit = if self.is_a2b() {
            MIN_SQRT_PRICE_X64 + 1
        } else {
            MAX_SQRT_PRICE_X64 - 1
        };
        let sqrt_price_limit = ctx.pure(sqrt_price_limit).map_err(|e| eyre!(e))?;

        let versioned_arg = ctx.obj(self.versioned).map_err(|e| eyre!(e))?;
        let clock_arg = ctx.obj(self.clock).map_err(|e| eyre!(e))?;

        Ok(vec![
            pool_arg,
            a2b,
            by_amount_in,
            amount,
            sqrt_price_limit,
            versioned_arg,
            clock_arg,
        ])
    }

    /*
    public fun pay<T0, T1>(
        _pool: &mut Pool<T0, T1>,
        _receipt: SwapReceipt,
        _balance_a: Balance<T0>,
        _balance_b: Balance<T1>,
        _versioned: &Versioned,
        _ctx: &TxContext
    )
    */
    fn build_repay_args(
        &self,
        ctx: &mut TradeCtx,
        pool: Argument,
        coin: Argument,
        receipt: Argument,
    ) -> Result<Vec<Argument>> {
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

        let versioned_arg = ctx.obj(self.versioned).map_err(|e| eyre!(e))?;
        Ok(vec![pool, receipt, balance_a, balance_b, versioned_arg])
    }

    fn borrow_mut_pool(&self, ctx: &mut TradeCtx) -> Result<Argument> {
        let package = ObjectID::from_hex_literal(FLOWX_CLMM)?;
        let module = Identifier::new("pool_manager").map_err(|e| eyre!(e))?;
        let function = Identifier::new("borrow_mut_pool").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();

        let arguments = {
            let pool_registry = ctx.obj(self.pool_registry).map_err(|e| eyre!(e))?;
            let fee = ctx.pure(self.fee).map_err(|e| eyre!(e))?;
            vec![pool_registry, fee]
        };

        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        Ok(Argument::Result(ctx.last_command_idx()))
    }
}

#[async_trait::async_trait]
impl Dex for FlowxClmm {
    fn support_flashloan(&self) -> bool {
        false
    }

    async fn extend_flashloan_tx(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<FlashResult> {
        let pool = self.borrow_mut_pool(ctx)?;

        let package = ObjectID::from_hex_literal(FLOWX_CLMM)?;
        let module = Identifier::new("pool").map_err(|e| eyre!(e))?;
        let function = Identifier::new("swap").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone(); // CoinA, CoinB
        let arguments = self.build_flashloan_args(ctx, pool, amount_in)?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();

        // `swap` returns (Balance<T0>, Balance<T1>, SwapReceipt)
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
            pool: Some(pool),
        })
    }

    async fn extend_repay_tx(&self, ctx: &mut TradeCtx, coin: Argument, flash_res: FlashResult) -> Result<Argument> {
        let package = ObjectID::from_hex_literal(FLOWX_CLMM)?;
        let module = Identifier::new("pool").map_err(|e| eyre!(e))?;
        let receipt = flash_res.receipt;
        let pool = flash_res.pool.ok_or_eyre("missing pool")?;

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
        let function = Identifier::new("pay").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_repay_args(ctx, pool, repay_coin, receipt)?;
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
        let package = ObjectID::from_hex_literal(FLOWX_CLMM)?;
        let module = Identifier::new("swap_router").map_err(|e| eyre!(e))?;
        let function = Identifier::new("swap_exact_input").map_err(|e| eyre!(e))?;
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
        Protocol::FlowxClmm
    }

    fn liquidity(&self) -> u128 {
        self.liquidity
    }

    fn object_id(&self) -> ObjectID {
        self.pool.pool
    }

    fn flip(&mut self) {
        std::mem::swap(&mut self.coin_in_type, &mut self.coin_out_type);
        self.type_params.reverse();
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
    use simulator::DBSimulator;
    use simulator::HttpSimulator;
    use simulator::Simulator;
    use tracing::info;

    use super::*;
    use crate::{
        config::tests::{TEST_ATTACKER, TEST_HTTP_URL},
        defi::{indexer_searcher::IndexerDexSearcher, DexSearcher},
    };

    #[tokio::test]
    async fn test_flowx_swap_tx() {
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
            .filter(|dex| dex.protocol() == Protocol::FlowxClmm)
            .sorted_by(|a, b| a.liquidity().cmp(&b.liquidity()))
            .last()
            .unwrap();
        let tx_data = dex.swap_tx(owner, recipient, amount_in).await.unwrap();
        info!("🧀 tx_data: {:?}", tx_data);

        let response = http_simulator.simulate(tx_data, Default::default()).await.unwrap();
        info!("🧀 {:?}", response);
    }
}
