use eyre::{eyre, Result};
use serde::Deserialize;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::base_types::ObjectID,
    SuiClient,
};

use super::get_coin_decimals;
use crate::types::{Pool, PoolExtra, Protocol, Token};

const DEEPBOOK_V2_POOL_CREATED: &str = "0xdee9::clob_v2::PoolCreated";

pub fn deepbook_v2_event_filter() -> EventFilter {
    EventFilter::MoveEventType(DEEPBOOK_V2_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeepbookV2PoolCreated {
    pub pool: ObjectID,
    pub base_asset: String,
    pub quote_asset: String,
    pub taker_fee_rate: u64,
    pub maker_rebate_rate: u64,
    pub tick_size: u64,
    pub lot_size: u64,
}

impl TryFrom<&SuiEvent> for DeepbookV2PoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let base_asset = parsed_json["base_asset"]
            .as_object()
            .ok_or_else(|| eyre!("Missing base_asset"))?;
        let base_asset = base_asset["name"].as_str().ok_or_else(|| eyre!("Missing base_asset"))?;

        let quote_asset = parsed_json["quote_asset"]
            .as_object()
            .ok_or_else(|| eyre!("Missing quote_asset"))?;
        let quote_asset = quote_asset["name"]
            .as_str()
            .ok_or_else(|| eyre!("Missing quote_asset"))?;

        let taker_fee_rate: u64 = parsed_json["taker_fee_rate"]
            .as_str()
            .ok_or_else(|| eyre!("Missing taker_fee_rate"))?
            .parse()?;

        let maker_rebate_rate: u64 = parsed_json["maker_rebate_rate"]
            .as_str()
            .ok_or_else(|| eyre!("Missing maker_rebate_rate"))?
            .parse()?;

        let tick_size: u64 = parsed_json["tick_size"]
            .as_str()
            .ok_or_else(|| eyre!("Missing tick_size"))?
            .parse()?;

        let lot_size: u64 = parsed_json["lot_size"]
            .as_str()
            .ok_or_else(|| eyre!("Missing lot_size"))?
            .parse()?;

        Ok(Self {
            pool,
            base_asset: format!("0x{}", base_asset),
            quote_asset: format!("0x{}", quote_asset),
            taker_fee_rate,
            maker_rebate_rate,
            tick_size,
            lot_size,
        })
    }
}

impl DeepbookV2PoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let base_asset_decimals = get_coin_decimals(sui, &self.base_asset).await?;
        let quote_asset_decimals = get_coin_decimals(sui, &self.quote_asset).await?;

        let tokens = vec![
            Token::new(&self.base_asset, base_asset_decimals),
            Token::new(&self.quote_asset, quote_asset_decimals),
        ];

        let extra = PoolExtra::DeepbookV2 {
            taker_fee_rate: self.taker_fee_rate,
            maker_rebate_rate: self.maker_rebate_rate,
            tick_size: self.tick_size,
            lot_size: self.lot_size,
        };

        Ok(Pool {
            protocol: Protocol::DeepbookV2,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}
