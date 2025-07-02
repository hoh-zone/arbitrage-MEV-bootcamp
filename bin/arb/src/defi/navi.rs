use std::{str::FromStr, sync::Arc};

use eyre::{eyre, OptionExt, Result};
use simulator::Simulator;
use sui_sdk::SUI_COIN_TYPE;
use sui_types::{
    base_types::ObjectID,
    transaction::{Argument, Command, ObjectArg},
    Identifier, TypeTag, SUI_CLOCK_OBJECT_ID,
};
use utils::object::shared_obj_arg;

use super::{trade::FlashResult, TradeCtx};

const NAVI_PROTOCOL: &str = "0x834a86970ae93a73faf4fff16ae40bdb72b91c47be585fff19a2af60a19ddca3";
const NAVI_POOL: &str = "0x96df0fce3c471489f4debaaa762cf960b3d97820bd1f3f025ff8190730e958c5";
const NAVI_CONFIG: &str = "0x3672b2bf471a60c30a03325f104f92fb195c9d337ba58072dce764fe2aa5e2dc";
const NAVI_STORAGE: &str = "0xbb4e2f4b6205c2e2a2db47aeb4f830796ec7c005f88537ee775986639bc442fe";

#[derive(Clone)]
pub struct Navi {
    sui_coin_type: TypeTag,
    pool: ObjectArg,
    config: ObjectArg,
    storage: ObjectArg,
    clock: ObjectArg,
}

impl Navi {
    // Objects are fetched only once during initialization, without affecting the arbitrage performance.
    pub async fn new(simulator: Arc<Box<dyn Simulator>>) -> Result<Self> {
        let pool = simulator
            .get_object(&ObjectID::from_hex_literal(NAVI_POOL)?)
            .await
            .ok_or_eyre("navi pool not found")?;
        let config = simulator
            .get_object(&ObjectID::from_hex_literal(NAVI_CONFIG)?)
            .await
            .ok_or_eyre("navi config not found")?;
        let storage = simulator
            .get_object(&ObjectID::from_hex_literal(NAVI_STORAGE)?)
            .await
            .ok_or_eyre("navi storage not found")?;
        let clock = simulator
            .get_object(&SUI_CLOCK_OBJECT_ID)
            .await
            .ok_or_eyre("sui clock not found")?;

        Ok(Self {
            sui_coin_type: TypeTag::from_str(SUI_COIN_TYPE).unwrap(),
            pool: shared_obj_arg(&pool, true),
            config: shared_obj_arg(&config, false),
            storage: shared_obj_arg(&storage, true),
            clock: shared_obj_arg(&clock, false),
        })
    }

    /*
    public fun flash_loan_with_ctx<CoinType>(
        config: &FlashLoanConfig,
        pool: &mut Pool<CoinType>,
        amount: u64,
        ctx: &mut TxContext
    ): (Balance<CoinType>, FlashLoanReceipt<CoinType>)
    */
    pub fn extend_flashloan_tx(&self, ctx: &mut TradeCtx, amount_in: u64) -> Result<FlashResult> {
        let package = ObjectID::from_hex_literal(NAVI_PROTOCOL)?;
        let module = Identifier::new("lending").map_err(|e| eyre!(e))?;
        let function = Identifier::new("flash_loan_with_ctx").map_err(|e| eyre!(e))?;
        let type_arguments = vec![self.sui_coin_type.clone()];

        let arguments = vec![
            ctx.obj(self.config).map_err(|e| eyre!(e))?,
            ctx.obj(self.pool).map_err(|e| eyre!(e))?,
            ctx.pure(amount_in).map_err(|e| eyre!(e))?,
        ];

        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));
        let last_idx = ctx.last_command_idx();

        let balance_out = Argument::NestedResult(last_idx, 0);
        let coin_out = ctx.coin_from_balance(balance_out, self.sui_coin_type.clone())?;

        Ok(FlashResult {
            coin_out,
            receipt: Argument::NestedResult(last_idx, 1),
            pool: None,
        })
    }

    /*
    public fun flash_repay_with_ctx<CoinType>(
        clock: &Clock,
        storage: &mut Storage,
        pool: &mut Pool<CoinType>,
        receipt: FlashLoanReceipt<CoinType>,
        repay_balance: Balance<CoinType>,
        ctx: &mut TxContext
    ): Balance<CoinType>
    */
    pub fn extend_repay_tx(&self, ctx: &mut TradeCtx, coin: Argument, flash_res: FlashResult) -> Result<Argument> {
        let package = ObjectID::from_hex_literal(NAVI_PROTOCOL)?;
        let module = Identifier::new("lending").map_err(|e| eyre!(e))?;
        let function = Identifier::new("flash_repay_with_ctx").map_err(|e| eyre!(e))?;
        let type_arguments = vec![self.sui_coin_type.clone()];

        let repay_balance = ctx.coin_into_balance(coin, self.sui_coin_type.clone())?;

        let arguments = vec![
            ctx.obj(self.clock).map_err(|e| eyre!(e))?,
            ctx.obj(self.storage).map_err(|e| eyre!(e))?,
            ctx.obj(self.pool).map_err(|e| eyre!(e))?,
            flash_res.receipt,
            repay_balance,
        ];

        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));
        let last_idx = ctx.last_command_idx();
        let balance = Argument::Result(last_idx);
        let coin = ctx.coin_from_balance(balance, self.sui_coin_type.clone())?;

        Ok(coin)
    }
}
