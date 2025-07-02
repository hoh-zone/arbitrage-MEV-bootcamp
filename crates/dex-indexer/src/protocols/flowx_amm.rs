//! There is no `pool_id` in the PairCreated events, so we can't backfill pools for FlowX AMM.

use eyre::{ensure, eyre, OptionExt, Result};
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use sui_sdk::{
    rpc_types::{EventFilter, SuiData, SuiEvent, SuiObjectDataOptions},
    types::base_types::ObjectID,
    SuiClient,
};

use super::get_coin_decimals;
use crate::{
    normalize_coin_type,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const FLOWX_AMM_POOL_CREATED: &str =
    "0xba153169476e8c3114962261d1edc70de5ad9781b83cc617ecc8c1923191cae0::factory::PairCreated";

pub const FLOWX_AMM_SWAP_EVENT: &str =
    "0xba153169476e8c3114962261d1edc70de5ad9781b83cc617ecc8c1923191cae0::pair::Swapped";

pub fn flowx_amm_event_filter() -> EventFilter {
    EventFilter::MoveEventType(FLOWX_AMM_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowxAmmPoolCreated {
    pub pool: ObjectID,
    pub token0: String,
    pub token1: String,
}

impl TryFrom<&SuiEvent> for FlowxAmmPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pair"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pair"))?
            .parse()?;
        let token0 = parsed_json["coin_x"].as_str().ok_or_else(|| eyre!("Missing coin_x"))?;
        let token0 = format!("0x{token0}");
        let token1 = parsed_json["coin_y"].as_str().ok_or_else(|| eyre!("Missing coin_y"))?;
        let token1 = format!("0x{token1}");

        Ok(Self { pool, token0, token1 })
    }
}

impl FlowxAmmPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let token0_decimals = get_coin_decimals(sui, &self.token0).await?;
        let token1_decimals = get_coin_decimals(sui, &self.token1).await?;

        let opts = SuiObjectDataOptions::default().with_content();

        let pool_obj = sui
            .read_api()
            .get_object_with_options(self.pool, opts)
            .await?
            .data
            .ok_or_else(|| eyre!("Pool not found"))?;

        let fee_rate: u64 = pool_obj
            .content
            .ok_or_else(|| eyre!("Pool has no content"))?
            .try_into_move()
            .ok_or_else(|| eyre!("Pool content is not Move"))?
            .fields
            .field_value("fee_rate")
            .ok_or_else(|| eyre!("Missing fee_rate"))?
            .to_string()
            .parse()?;

        let tokens = vec![
            Token::new(&self.token0, token0_decimals),
            Token::new(&self.token1, token1_decimals),
        ];
        let extra = PoolExtra::FlowxAmm { fee_rate };

        Ok(Pool {
            protocol: Protocol::FlowxAmm,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowxAmmSwapEvent {
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for FlowxAmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string() == FLOWX_AMM_SWAP_EVENT,
            "Not a FlowxAmmSwapEvent"
        );

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for FlowxAmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == FLOWX_AMM_SWAP_EVENT, "Not a FlowxAmmSwapEvent");

        event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?.try_into()
    }
}

impl TryFrom<&Value> for FlowxAmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let coin_x = parsed_json["coin_x"].as_str().ok_or_else(|| eyre!("Missing coin_x"))?;
        let coin_x = normalize_coin_type(format!("0x{coin_x}").as_str());

        let coin_y = parsed_json["coin_y"].as_str().ok_or_else(|| eyre!("Missing coin_y"))?;
        let coin_y = normalize_coin_type(format!("0x{coin_y}").as_str());

        let amount_x_in: u64 = parsed_json["amount_x_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_x_in"))?
            .parse()?;
        let amount_x_out: u64 = parsed_json["amount_x_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_x_out"))?
            .parse()?;

        let amount_y_in: u64 = parsed_json["amount_y_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_y_in"))?
            .parse()?;
        let amount_y_out: u64 = parsed_json["amount_y_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_y_out"))?
            .parse()?;

        let (coin_in, coin_out) = if amount_x_in > 0 {
            (coin_x, coin_y)
        } else {
            (coin_y, coin_x)
        };

        let (amount_in, amount_out) = if amount_x_in > 0 {
            (amount_x_in, amount_y_out)
        } else {
            (amount_y_in, amount_x_out)
        };

        Ok(Self {
            coin_in,
            coin_out,
            amount_in,
            amount_out,
        })
    }
}

impl FlowxAmmSwapEvent {
    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::FlowxAmm,
            pool: None,
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}
