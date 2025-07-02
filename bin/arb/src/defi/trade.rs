use std::{
    collections::HashSet,
    fmt,
    ops::{Deref, DerefMut},
    str::FromStr,
    sync::Arc,
};

use ::utils::coin;
use eyre::{ensure, eyre, Result};
use object_pool::ObjectPool;
use simulator::{SimulateCtx, Simulator};
use sui_json_rpc_types::SuiExecutionStatus;
use sui_sdk::rpc_types::SuiTransactionBlockEffectsAPI;
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    object::{Object, Owner},
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    transaction::{Argument, Command, ObjectArg, TransactionData},
    Identifier, TypeTag, SUI_FRAMEWORK_PACKAGE_ID,
};
use tracing::instrument;

use super::{navi::Navi, shio::Shio, Dex};
use crate::{config::*, types::Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeType {
    Swap,
    Flashloan,
}

#[derive(Debug, Clone)]
pub struct FlashResult {
    pub coin_out: Argument,
    pub receipt: Argument,
    pub pool: Option<Argument>,
}

#[derive(Clone)]
pub struct Trader {
    simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>,
    shio: Arc<Shio>,
    navi: Arc<Navi>,
}

#[derive(Default)]
pub struct TradeCtx {
    pub ptb: ProgrammableTransactionBuilder,
    pub command_count: u16,
}

#[derive(Default, Debug, Clone)]
pub struct TradeResult {
    pub amount_out: u64,
    pub gas_cost: i64,
    pub cache_misses: u64,
}

impl Trader {
    pub async fn new(simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>) -> Result<Self> {
        let shio = Arc::new(Shio::new().await?);
        let simulator = simulator_pool.get();
        let navi = Arc::new(Navi::new(simulator).await?);

        Ok(Self {
            simulator_pool,
            shio,
            navi,
        })
    }

    #[instrument(name = "result", skip_all, fields(
        len = %format!("{:<2}", path.path.len()),
        paths = %path.path.iter().map(|d| {
            let coin_in = d.coin_in_type().split("::").last().unwrap().to_string();
            let coin_out = d.coin_out_type().split("::").last().unwrap().to_string();
            format!("{:?}:{}:{}", d.protocol(), coin_in, coin_out)
        }).collect::<Vec<_>>().join(" ")
    ))]
    pub async fn get_trade_result(
        &self,
        path: &Path,
        sender: SuiAddress,
        amount_in: u64,
        trade_type: TradeType,
        gas_coins: Vec<ObjectRef>,
        mut sim_ctx: SimulateCtx,
    ) -> Result<TradeResult> {
        ensure!(!path.is_empty(), "empty path");
        let gas_price = sim_ctx.epoch.gas_price;

        let (tx_data, mocked_coin_in) = match trade_type {
            TradeType::Swap => {
                self.get_swap_trade_tx(path, sender, amount_in, gas_coins, gas_price)
                    .await?
            }
            TradeType::Flashloan => {
                self.get_flashloan_trade_tx(path, sender, amount_in, gas_coins, gas_price, Source::Public)
                    .await?
            }
        };

        if let Some(mocked_coin_in) = mocked_coin_in {
            sim_ctx.with_borrowed_coin((mocked_coin_in, amount_in));
        }

        let resp = self.simulator_pool.get().simulate(tx_data.clone(), sim_ctx).await?;
        let status = resp.effects.status();

        match status {
            SuiExecutionStatus::Success => {}
            SuiExecutionStatus::Failure { error } => {
                // ignore "MoveAbort"
                if !error.contains("MoveAbort") && !error.contains("InsufficientCoinBalance") {
                    tracing::error!("status: {:?}", status);
                }
            }
        }

        ensure!(status.is_ok(), "{:?}", status);

        let gas_cost = resp.effects.gas_cost_summary().net_gas_usage();
        let coin_in = TypeTag::from_str(&path.coin_in_type()).map_err(|_| eyre!("invalid coin_in_type"))?;
        let coin_out = TypeTag::from_str(&path.coin_out_type()).map_err(|_| eyre!("invalid coin_out_type"))?;
        let out_is_native = coin::is_native_coin(&path.coin_out_type());

        let mut amount_out = i128::MIN;
        for bc in &resp.balance_changes {
            if bc.owner == Owner::AddressOwner(sender) && bc.coin_type == coin_out {
                amount_out = bc.amount;
                if coin_in == coin_out && out_is_native {
                    amount_out = amount_out + amount_in as i128 + gas_cost as i128;
                }

                ensure!(amount_out >= 0, "negative amount_out {}", amount_out);
                break;
            }
        }
        ensure!(amount_out != i128::MIN, "no balance change for owner: {:?}", sender);

        Ok(TradeResult {
            amount_out: amount_out as u64,
            gas_cost,
            cache_misses: resp.cache_misses,
        })
    }

    pub async fn get_swap_trade_tx(
        &self,
        path: &Path,
        sender: SuiAddress,
        amount_in: u64,
        gas_coins: Vec<ObjectRef>,
        gas_price: u64,
    ) -> Result<(TransactionData, Option<Object>)> {
        ensure!(!path.is_empty(), "empty path");
        let mut ctx = TradeCtx::default();

        // 1. prepare coin_in
        let mocked_sui = coin::mocked_sui(sender, amount_in);
        let coin_in = mocked_sui.compute_object_reference();

        // 2. swap
        let mut coin_in_arg = ctx.split_coin(coin_in, amount_in)?;
        for (i, dex) in path.path.iter().enumerate() {
            let amount_in = if i == 0 { Some(amount_in) } else { None };
            coin_in_arg = dex.extend_trade_tx(&mut ctx, sender, coin_in_arg, amount_in).await?;
        }

        // 3. transfer the coin_out to recipient
        ctx.transfer_arg(sender, coin_in_arg);
        let tx = ctx.ptb.finish();

        let tx_data = TransactionData::new_programmable(sender, gas_coins, tx, GAS_BUDGET, gas_price);

        Ok((tx_data, Some(mocked_sui)))
    }

    pub async fn get_flashloan_trade_tx(
        &self,
        path: &Path,
        sender: SuiAddress,
        amount_in: u64,
        gas_coins: Vec<ObjectRef>,
        gas_price: u64,
        source: Source,
    ) -> Result<(TransactionData, Option<Object>)> {
        ensure!(!path.is_empty(), "empty path");
        let first_dex = &path.path[0];

        let mut ctx = TradeCtx::default();

        // 1. flashloan
        let flash_res = if first_dex.support_flashloan() {
            first_dex.extend_flashloan_tx(&mut ctx, amount_in).await?
        } else {
            self.navi.extend_flashloan_tx(&mut ctx, amount_in)?
        };

        // 2. swap
        let mut coin_in_arg = flash_res.coin_out;
        let dex_iter: Box<dyn Iterator<Item = &Box<dyn Dex>> + Send> = if first_dex.support_flashloan() {
            Box::new(path.path.iter().skip(1))
        } else {
            Box::new(path.path.iter())
        };
        for (i, dex) in dex_iter.enumerate() {
            let amount_in = if i == 0 { Some(amount_in) } else { None };
            coin_in_arg = dex.extend_trade_tx(&mut ctx, sender, coin_in_arg, amount_in).await?;
        }

        // 3. repay flashloan
        let coin_profit = if first_dex.support_flashloan() {
            first_dex.extend_repay_tx(&mut ctx, coin_in_arg, flash_res).await?
        } else {
            self.navi.extend_repay_tx(&mut ctx, coin_in_arg, flash_res)?
        };

        // 4. submit bid
        if source.is_shio() {
            let amount_arg = ctx.pure(source.bid_amount()).map_err(|e| eyre!(e))?;
            let coin_bid = ctx.split_coin_arg(coin_profit, amount_arg);
            self.shio.submit_bid(&mut ctx, coin_bid, source.bid_amount())?;
        }

        // 5. transfer the profit to recipient
        ctx.transfer_arg(sender, coin_profit);

        let tx = ctx.ptb.finish();

        // 6. finalize
        let mut tx_data =
            TransactionData::new_programmable(sender, gas_coins.clone(), tx.clone(), GAS_BUDGET, gas_price);

        if let Some(opp_tx_digest) = source.opp_tx_digest() {
            // A Bid MUST have a lexicologically larger transaction digest comparing to opportunity transaction's.
            let mut gas_budget = GAS_BUDGET;
            while tx_data.digest() <= opp_tx_digest {
                gas_budget += 1;
                tx_data =
                    TransactionData::new_programmable(sender, gas_coins.clone(), tx.clone(), gas_budget, gas_price);
            }
        };

        Ok((tx_data, None))
    }
}

impl TradeCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn command(&mut self, cmd: Command) {
        self.ptb.command(cmd);
        self.command_count += 1;
    }

    pub fn transfer_arg(&mut self, recipient: SuiAddress, coin_arg: Argument) {
        self.ptb.transfer_arg(recipient, coin_arg);
        self.command_count += 1;
    }

    pub fn last_command_idx(&self) -> u16 {
        self.command_count - 1
    }

    pub fn split_coin(&mut self, coin: ObjectRef, amount: u64) -> Result<Argument> {
        let coin_arg = self.obj(ObjectArg::ImmOrOwnedObject(coin)).map_err(|e| eyre!(e))?;
        let amount_arg = self.pure(amount).map_err(|e| eyre!(e))?;

        Ok(self.split_coin_arg(coin_arg, amount_arg))
    }

    pub fn split_coin_arg(&mut self, coin: Argument, amount: Argument) -> Argument {
        self.command(Command::SplitCoins(coin, vec![amount]));
        let last_idx = self.last_command_idx();
        Argument::Result(last_idx)
    }

    // sui::balance::destroy_zero(balance);
    pub fn balance_destroy_zero(&mut self, balance: Argument, coin_type: TypeTag) -> Result<()> {
        self.build_command(
            SUI_FRAMEWORK_PACKAGE_ID,
            "balance",
            "destroy_zero",
            vec![coin_type],
            vec![balance],
        )?;

        Ok(())
    }

    // sui::balance::zero<CoinTypeTag>();
    pub fn balance_zero(&mut self, coin_type: TypeTag) -> Result<Argument> {
        self.build_command(SUI_FRAMEWORK_PACKAGE_ID, "balance", "zero", vec![coin_type], vec![])?;

        let last_idx = self.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

    // sui::coin::from_balance(balance, ctx)
    pub fn coin_from_balance(&mut self, balance: Argument, coin_type: TypeTag) -> Result<Argument> {
        self.build_command(
            SUI_FRAMEWORK_PACKAGE_ID,
            "coin",
            "from_balance",
            vec![coin_type],
            vec![balance],
        )?;

        let last_idx = self.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

    // sui::coin::into_balance(coin);
    pub fn coin_into_balance(&mut self, coin: Argument, coin_type: TypeTag) -> Result<Argument> {
        self.build_command(
            SUI_FRAMEWORK_PACKAGE_ID,
            "coin",
            "into_balance",
            vec![coin_type],
            vec![coin],
        )?;

        let last_idx = self.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

    #[inline]
    fn build_command(
        &mut self,
        package: ObjectID,
        module: &str,
        function: &str,
        type_arguments: Vec<TypeTag>,
        arguments: Vec<Argument>,
    ) -> Result<()> {
        let module = Identifier::new(module).map_err(|e| eyre!(e))?;
        let function = Identifier::new(function).map_err(|e| eyre!(e))?;
        self.command(Command::move_call(package, module, function, type_arguments, arguments));

        Ok(())
    }
}

impl Deref for TradeCtx {
    type Target = ProgrammableTransactionBuilder;

    fn deref(&self) -> &Self::Target {
        &self.ptb
    }
}

impl DerefMut for TradeCtx {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ptb
    }
}

impl PartialEq for TradeResult {
    fn eq(&self, other: &Self) -> bool {
        self.amount_out == other.amount_out
    }
}

impl PartialOrd for TradeResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.amount_out.partial_cmp(&other.amount_out)
    }
}

#[derive(Default, Clone)]
pub struct Path {
    pub path: Vec<Box<dyn Dex>>,
}

impl Path {
    pub fn new(path: Vec<Box<dyn Dex>>) -> Self {
        Self { path }
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    pub fn is_disjoint(&self, other: &Self) -> bool {
        let a = self.path.iter().collect::<HashSet<_>>();
        let b = other.path.iter().collect::<HashSet<_>>();
        a.is_disjoint(&b)
    }

    pub fn coin_in_type(&self) -> String {
        self.path[0].coin_in_type()
    }

    pub fn coin_out_type(&self) -> String {
        self.path.last().unwrap().coin_out_type()
    }

    pub fn contains_pool(&self, pool_id: Option<ObjectID>) -> bool {
        if let Some(pool_id) = pool_id {
            self.path.iter().any(|dex| dex.object_id() == pool_id)
        } else {
            false
        }
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path_str: Vec<String> = self.path.iter().map(|dex| format!("{:?}", dex)).collect();
        write!(f, "[{}]", path_str.join(", "))
    }
}
