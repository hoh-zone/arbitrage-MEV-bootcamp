use std::{str::FromStr, sync::Arc};

use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::language_storage::StructTag;
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use simulator::Simulator;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::base_types::ObjectID,
    SuiClient,
};

use super::{get_coin_decimals, get_pool_coins_type};
use crate::{
    get_coin_in_out_v2, normalize_coin_type,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const KRIYA_AMM_POOL_CREATED: &str =
    "0xa0eba10b173538c8fecca1dff298e488402cc9ff374f8a12ca7758eebe830b66::spot_dex::PoolCreatedEvent";

pub const KRIYA_AMM_SWAP_EVENT: &str =
    "0xa0eba10b173538c8fecca1dff298e488402cc9ff374f8a12ca7758eebe830b66::spot_dex::SwapEvent";

pub fn kriya_amm_event_filter() -> EventFilter {
    EventFilter::MoveEventType(KRIYA_AMM_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct KriyaAmmPoolCreated {
    pub pool: ObjectID,
    pub lp_fee_percent: u64,
    pub protocol_fee_percent: u64,
}

impl TryFrom<&SuiEvent> for KriyaAmmPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let lp_fee_percent: u64 = parsed_json["lp_fee_percent"]
            .as_str()
            .ok_or_else(|| eyre!("Missing lp_fee_percent"))?
            .parse()?;

        let protocol_fee_percent: u64 = parsed_json["protocol_fee_percent"]
            .as_str()
            .ok_or_else(|| eyre!("Missing protocol_fee_percent"))?
            .parse()?;

        Ok(Self {
            pool,
            lp_fee_percent,
            protocol_fee_percent,
        })
    }
}

impl KriyaAmmPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let (token0_type, token1_type) = get_pool_coins_type(sui, self.pool).await?;

        let token0_decimals = get_coin_decimals(sui, &token0_type).await?;
        let token1_decimals = get_coin_decimals(sui, &token1_type).await?;

        let tokens = vec![
            Token::new(&token0_type, token0_decimals),
            Token::new(&token1_type, token1_decimals),
        ];
        let extra = PoolExtra::KriyaAmm {
            lp_fee_percent: self.lp_fee_percent,
            protocol_fee_percent: self.protocol_fee_percent,
        };

        Ok(Pool {
            protocol: Protocol::KriyaAmm,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KriyaAmmSwapEvent {
    pub pool: ObjectID,
    pub coin_in: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for KriyaAmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string().starts_with(KRIYA_AMM_SWAP_EVENT) && event.type_.type_params.len() == 1,
            "Not a KriyaAmmSwapEvent"
        );

        let coin_in = event.type_.type_params[0].to_string();
        let coin_in = normalize_coin_type(&coin_in);

        Self::new(&event.parsed_json, coin_in)
    }
}

impl TryFrom<&ShioEvent> for KriyaAmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        let event_type_tag = StructTag::from_str(&event.event_type).map_err(|e| eyre!(e))?;

        ensure!(
            event.event_type.starts_with(KRIYA_AMM_SWAP_EVENT) && event_type_tag.type_params.len() == 1,
            "Not a KriyaAmmSwapEvent"
        );

        let coin_in = event_type_tag.type_params[0].to_string();
        let coin_in = normalize_coin_type(&coin_in);
        let parsed_json = event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?;

        Self::new(parsed_json, coin_in)
    }
}

impl KriyaAmmSwapEvent {
    pub fn new(parsed_json: &Value, coin_in: String) -> Result<Self> {
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;
        let amount_in = parsed_json["amount_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_in"))?
            .parse()?;
        let amount_out = parsed_json["amount_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_out"))?
            .parse()?;

        Ok(Self {
            pool,
            coin_in,
            amount_in,
            amount_out,
        })
    }

    #[allow(dead_code)]
    pub async fn to_swap_event_v1(&self, sui: &SuiClient) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_pool_coins_type(sui, self.pool).await?;
        let coin_out = if self.coin_in == coin_a { coin_b } else { coin_a };
        let coin_out = normalize_coin_type(&coin_out);

        Ok(SwapEvent {
            protocol: Protocol::KriyaAmm,
            pool: Some(self.pool),
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }

    pub async fn to_swap_event_v2(&self, provider: Arc<dyn Simulator>) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_coin_in_out_v2!(self.pool, provider, true);

        let (coin_in, coin_out) = if coin_a == self.coin_in {
            (coin_a, coin_b)
        } else {
            (coin_b, coin_a)
        };

        Ok(SwapEvent {
            protocol: Protocol::KriyaAmm,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn kriya_amm_related_object_ids() -> Vec<String> {
    vec![
        "0xa0eba10b173538c8fecca1dff298e488402cc9ff374f8a12ca7758eebe830b66", // Kriya Dex
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn test_swap_event_http() {
        use super::*;
        use simulator::HttpSimulator;

        let provider = HttpSimulator::new("", &None).await;

        let swap_event = KriyaAmmSwapEvent {
            pool: ObjectID::from_str("0x367e02acb99632e18db69c3e93d89d21eb721e1d1fcebc0f6853667337450acc").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            coin_in: "0x2::sui::SUI".to_string(),
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x2::sui::SUI";
        let expected_b = "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_swap_event_db() {
        use super::*;
        use simulator::DBSimulator;

        let provider = DBSimulator::new_default_slow().await;

        let swap_event = KriyaAmmSwapEvent {
            pool: ObjectID::from_str("0x367e02acb99632e18db69c3e93d89d21eb721e1d1fcebc0f6853667337450acc").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            coin_in: "0x2::sui::SUI".to_string(),
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x2::sui::SUI";
        let expected_b = "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }
}
