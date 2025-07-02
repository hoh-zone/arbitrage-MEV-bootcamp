use std::{str::FromStr, sync::Arc, vec::Vec};

use dex_indexer::types::{Pool, Protocol};
use eyre::{ensure, eyre, Result};

use move_core_types::annotated_value::MoveStruct;
use primitive_types::U256;
use simulator::Simulator;
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    transaction::{Argument, Command, ObjectArg, ProgrammableTransaction, TransactionData},
    Identifier, TypeTag,
};
use tokio::sync::OnceCell;
use utils::{coin, new_test_sui_client, object::*};

use super::TradeCtx;
use crate::{config::*, defi::Dex};

const AFTERMATH_DEX: &str = "0xc4049b2d1cc0f6e017fda8260e4377cecd236bd7f56a54fee120816e72e2e0dd";
const POOL_REGISTRY: &str = "0xfcc774493db2c45c79f688f88d28023a3e7d98e4ee9f48bbf5c7990f651577ae";
const PROTOCOL_FEE_VAULT: &str = "0xf194d9b1bcad972e45a7dd67dd49b3ee1e3357a00a50850c52cd51bb450e13b4";
const TREASURY: &str = "0x28e499dff5e864a2eafe476269a4f5035f1c16f338da7be18b103499abf271ce";
const INSURANCE_FUND: &str = "0xf0c40d67b078000e18032334c3325c47b9ec9f3d9ae4128be820d54663d14e3b";
const REFERRAL_VAULT: &str = "0x35d35b0e5b177593d8c3a801462485572fc30861e6ce96a55af6dc4730709278";
const SLIPPAGE: u128 = 900_000_000_000_000_000;
const ONE: U256 = U256([1_000_000_000_000_000_000, 0, 0, 0]); // 10^18

#[derive(Clone)]
pub struct ObjectArgs {
    pool_registry: ObjectArg,
    protocol_fee_vault: ObjectArg,
    treasury: ObjectArg,
    insurance_fund: ObjectArg,
    referral_vault: ObjectArg,
}

static OBJ_CACHE: OnceCell<ObjectArgs> = OnceCell::const_new();

async fn get_object_args(simulator: Arc<Box<dyn Simulator>>) -> ObjectArgs {
    OBJ_CACHE
        .get_or_init(|| async {
            let pool_registry = simulator
                .get_object(&ObjectID::from_hex_literal(POOL_REGISTRY).unwrap())
                .await
                .unwrap();
            let protocol_fee_vault = simulator
                .get_object(&ObjectID::from_hex_literal(PROTOCOL_FEE_VAULT).unwrap())
                .await
                .unwrap();
            let treasury = simulator
                .get_object(&ObjectID::from_hex_literal(TREASURY).unwrap())
                .await
                .unwrap();
            let insurance_fund = simulator
                .get_object(&ObjectID::from_hex_literal(INSURANCE_FUND).unwrap())
                .await
                .unwrap();
            let referral_vault = simulator
                .get_object(&ObjectID::from_hex_literal(REFERRAL_VAULT).unwrap())
                .await
                .unwrap();

            ObjectArgs {
                pool_registry: shared_obj_arg(&pool_registry, false),
                protocol_fee_vault: shared_obj_arg(&protocol_fee_vault, false),
                treasury: shared_obj_arg(&treasury, true),
                insurance_fund: shared_obj_arg(&insurance_fund, true),
                referral_vault: shared_obj_arg(&referral_vault, false),
            }
        })
        .await
        .clone()
}

#[derive(Clone)]
pub struct Aftermath {
    pool_arg: ObjectArg,
    liquidity: u128,
    coin_in_type: String,
    coin_out_type: String,
    type_params: Vec<TypeTag>,
    pool_registry: ObjectArg,
    protocol_fee_vault: ObjectArg,
    treasury: ObjectArg,
    insurance_fund: ObjectArg,
    referral_vault: ObjectArg,
    balances: Vec<u128>,
    weights: Vec<u64>,
    swap_fee_in: u64,
    swap_fee_out: u64,
    index_in: usize,
    index_out: usize,
}

impl Aftermath {
    pub async fn new(
        simulator: Arc<Box<dyn Simulator>>,
        pool: &Pool,
        coin_in_type: &str,
        coin_out_type: Option<String>,
    ) -> Result<Vec<Self>> {
        ensure!(pool.protocol == Protocol::Aftermath, "not a Aftermath pool");

        let pool_obj = simulator
            .get_object(&pool.pool)
            .await
            .ok_or_else(|| eyre!("pool not found: {}", pool.pool))?;

        let parsed_pool = {
            let layout = simulator
                .get_object_layout(&pool.pool)
                .ok_or_else(|| eyre!("pool layout not found"))?;

            let move_obj = pool_obj.data.try_as_move().ok_or_else(|| eyre!("not a move object"))?;
            MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
        };

        let liquidity = {
            let lp_supply = extract_struct_from_move_struct(&parsed_pool, "lp_supply")?;
            extract_u64_from_move_struct(&lp_supply, "value")? as u128
        };

        let balances = extract_u128_vec_from_move_struct(&parsed_pool, "normalized_balances")?;
        let weights = extract_u64_vec_from_move_struct(&parsed_pool, "weights")?;
        let fees_swap_in = extract_u64_vec_from_move_struct(&parsed_pool, "fees_swap_in")?;
        let fees_swap_out = extract_u64_vec_from_move_struct(&parsed_pool, "fees_swap_out")?;
        let index_in = pool.token_index(coin_in_type).unwrap();

        let mut type_params = parsed_pool.type_.type_params.clone();
        let coin_in_type_tag = TypeTag::from_str(&coin_in_type).map_err(|e| eyre!(e))?;
        type_params.push(coin_in_type_tag);
        let pool_arg = shared_obj_arg(&pool_obj, true);

        let ObjectArgs {
            pool_registry,
            protocol_fee_vault,
            treasury,
            insurance_fund,
            referral_vault,
        } = get_object_args(simulator.clone()).await;

        if let Some(coin_out_type) = coin_out_type {
            let coin_out_type_tag = TypeTag::from_str(&coin_out_type).map_err(|e| eyre!(e))?;
            type_params.push(coin_out_type_tag);

            let index_out = pool.token_index(&coin_out_type).unwrap();

            return Ok(vec![Self {
                pool_arg,
                liquidity,
                coin_in_type: coin_in_type.to_string(),
                coin_out_type,
                type_params,
                pool_registry,
                protocol_fee_vault,
                treasury,
                insurance_fund,
                referral_vault,
                balances,
                weights,
                swap_fee_in: fees_swap_in[index_in],
                swap_fee_out: fees_swap_out[index_out],
                index_in,
                index_out,
            }]);
        }

        let mut res = Vec::new();
        for (index_out, coin_out) in pool.tokens.iter().enumerate() {
            if coin_out.token_type == coin_in_type {
                continue;
            }
            let mut type_params = type_params.clone();
            let coin_out_type_tag = TypeTag::from_str(&coin_out.token_type).map_err(|e| eyre!(e))?;
            type_params.push(coin_out_type_tag);

            res.push(Self {
                pool_arg: pool_arg.clone(),
                liquidity,
                coin_in_type: coin_in_type.to_string(),
                coin_out_type: coin_out.token_type.clone(),
                type_params,
                pool_registry: pool_registry.clone(),
                protocol_fee_vault: protocol_fee_vault.clone(),
                treasury: treasury.clone(),
                insurance_fund: insurance_fund.clone(),
                referral_vault: referral_vault.clone(),
                balances: balances.clone(),
                weights: weights.clone(),
                swap_fee_in: fees_swap_in[index_in],
                swap_fee_out: fees_swap_out[index_out],
                index_in,
                index_out,
            });
        }

        Ok(res)
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
        let coin_out = self.extend_trade_tx(&mut ctx, sender, coin_in, Some(amount_in)).await?;
        ctx.transfer_arg(recipient, coin_out);

        Ok(ctx.ptb.finish())
    }

    async fn build_swap_args(
        &self,
        ctx: &mut TradeCtx,
        coin_in_arg: Argument,
        amount_in: u64,
    ) -> Result<Vec<Argument>> {
        let pool_arg = ctx.obj(self.pool_arg).map_err(|e| eyre!(e))?;

        let pool_registry_arg = ctx.obj(self.pool_registry).map_err(|e| eyre!(e))?;

        let protocol_fee_vault_arg = ctx.obj(self.protocol_fee_vault).map_err(|e| eyre!(e))?;

        let treasury_arg = ctx.obj(self.treasury).map_err(|e| eyre!(e))?;

        let insurance_fund_arg = ctx.obj(self.insurance_fund).map_err(|e| eyre!(e))?;

        let referral_vault_arg = ctx.obj(self.referral_vault).map_err(|e| eyre!(e))?;

        let amount_out = self.expect_amount_out(amount_in)?;
        let expect_amount_out = ctx.pure(amount_out).map_err(|e| eyre!(e))?;
        let slippage = ctx.pure(SLIPPAGE as u64).map_err(|e| eyre!(e))?;

        Ok(vec![
            pool_arg,
            pool_registry_arg,
            protocol_fee_vault_arg,
            treasury_arg,
            insurance_fund_arg,
            referral_vault_arg,
            coin_in_arg,
            expect_amount_out,
            slippage,
        ])
    }

    #[inline]
    fn expect_amount_out(&self, amount_in: u64) -> Result<u64> {
        let amount_out = calculate_expected_out(
            self.balances[self.index_in],
            self.balances[self.index_out],
            self.weights[self.index_in],
            self.weights[self.index_out],
            self.swap_fee_in,
            self.swap_fee_out,
            amount_in,
        )?;

        Ok(amount_out)
    }
}

#[async_trait::async_trait]
impl Dex for Aftermath {
    async fn extend_trade_tx(
        &self,
        ctx: &mut TradeCtx,
        _sender: SuiAddress,
        coin_in: Argument,
        amount_in: Option<u64>,
    ) -> Result<Argument> {
        let amount_in = amount_in.ok_or_else(|| eyre!("amount_in is required"))?;

        let package = ObjectID::from_hex_literal(AFTERMATH_DEX)?;
        let module = Identifier::new("swap").map_err(|e| eyre!(e))?;
        let function = Identifier::new("swap_exact_in").map_err(|e| eyre!(e))?;
        let type_arguments = self.type_params.clone();
        let arguments = self.build_swap_args(ctx, coin_in, amount_in).await?;
        ctx.command(Command::move_call(package, module, function, type_arguments, arguments));

        let last_idx = ctx.last_command_idx();
        Ok(Argument::Result(last_idx))
    }

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

    fn coin_in_type(&self) -> String {
        self.coin_in_type.clone()
    }

    fn coin_out_type(&self) -> String {
        self.coin_out_type.clone()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Aftermath
    }

    fn liquidity(&self) -> u128 {
        self.liquidity
    }

    fn object_id(&self) -> ObjectID {
        self.pool_arg.id()
    }

    fn flip(&mut self) {
        std::mem::swap(&mut self.coin_in_type, &mut self.coin_out_type);
    }

    fn is_a2b(&self) -> bool {
        false
    }
}

/// Get an estimate for amount_out using the spot price.
pub fn calculate_expected_out(
    balance_in: u128,
    balance_out: u128,
    weight_in: u64,
    weight_out: u64,
    swap_fee_in: u64,
    swap_fee_out: u64,
    amount_in: u64,
) -> Result<u64> {
    // Get spot price with fees
    let spot_price = calc_spot_price_fixed_with_fees(
        U256::from(balance_in),
        U256::from(balance_out),
        U256::from(weight_in),
        U256::from(weight_out),
        U256::from(swap_fee_in),
        U256::from(swap_fee_out),
    )?;

    // Calculate expected amount out
    Ok(convert_fixed_to_int(div_down(
        convert_int_to_fixed(amount_in),
        spot_price,
    )?))
}

// Helper functions
fn convert_int_to_fixed(a: u64) -> U256 {
    U256::from(a) * ONE
}

fn convert_fixed_to_int(a: U256) -> u64 {
    (a / ONE).low_u64()
}

fn div_down(a: U256, b: U256) -> Result<U256> {
    if b.is_zero() {
        return Err(eyre!("Division by zero"));
    }
    Ok((a * ONE) / b)
}

fn mul_down(a: U256, b: U256) -> Result<U256> {
    Ok((a * b) / ONE)
}

fn complement(x: U256) -> U256 {
    if x < ONE {
        ONE - x
    } else {
        U256::zero()
    }
}

// Calculate spot price with fees
fn calc_spot_price_fixed_with_fees(
    balance_in: U256,
    balance_out: U256,
    weight_in: U256,
    weight_out: U256,
    swap_fee_in: U256,
    swap_fee_out: U256,
) -> Result<U256> {
    // First calculate spot price without fees
    let spot_price_no_fees = calc_spot_price(balance_in, balance_out, weight_in, weight_out)?;

    // Then apply fees
    let fees_scalar = mul_down(complement(swap_fee_in), complement(swap_fee_out))?;

    div_down(spot_price_no_fees, fees_scalar)
}

// Calculate spot price without fees
fn calc_spot_price(balance_in: U256, balance_out: U256, weight_in: U256, weight_out: U256) -> Result<U256> {
    div_down(
        div_down(balance_in * ONE, weight_in)?,
        div_down(balance_out * ONE, weight_out)?,
    )
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use object_pool::ObjectPool;
    use simulator::{DBSimulator, Simulator};
    use tracing::info;

    use super::*;
    use crate::{
        config::tests::{TEST_ATTACKER, TEST_HTTP_URL},
        defi::{indexer_searcher::IndexerDexSearcher, DexSearcher},
    };

    #[tokio::test]
    async fn test_aftermath_swap_tx() {
        mev_logger::init_console_logger_with_directives(None, &["arb=debug"]);

        let simulator_pool = Arc::new(ObjectPool::new(1, move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { Box::new(DBSimulator::new_test(true).await) as Box<dyn Simulator> })
        }));

        let owner = SuiAddress::from_str(TEST_ATTACKER).unwrap();
        let recipient =
            SuiAddress::from_str("0x0cbe287984143ef232336bb39397bd10607fa274707e8d0f91016dceb31bb829").unwrap();
        let token_in_type = "0x2::sui::SUI";
        let token_out_type = "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN";
        let amount_in = 1000000000;

        // find dexes and swap
        let searcher = IndexerDexSearcher::new(TEST_HTTP_URL, simulator_pool.clone())
            .await
            .unwrap();
        let dexes = searcher
            .find_dexes(token_in_type, Some(token_out_type.into()))
            .await
            .unwrap();
        info!("🧀 dexes_len: {}", dexes.len());
        let dex = dexes
            .into_iter()
            .filter(|dex| dex.protocol() == Protocol::Aftermath)
            .max_by_key(|dex| dex.liquidity())
            .unwrap();
        let tx_data = dex.swap_tx(owner, recipient, amount_in).await.unwrap();
        info!("🧀 tx_data: {:?}", tx_data);

        let simulator = simulator_pool.get();
        let response = simulator.simulate(tx_data, Default::default()).await.unwrap();
        info!("🧀 {:?}", response);
    }
}
