use std::str::FromStr;

use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::language_storage::StructTag;
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use sui_sdk::rpc_types::SuiEvent;

use crate::{
    normalize_coin_type,
    types::{Protocol, SwapEvent},
};

pub const ABEX_SWAP_EVENT: &str = "0xceab84acf6bf70f503c3b0627acaff6b3f84cee0f2d7ed53d00fa6c2a168d14f::market::Swapped";

#[derive(Debug, Clone, Deserialize)]
pub struct AbexSwapEvent {
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for AbexSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let event_type = event.type_.to_string();
        ensure!(
            event_type.starts_with(ABEX_SWAP_EVENT) && event.type_.type_params.len() == 2,
            "Not an AbexSwapEvent"
        );

        let coin_in = event.type_.type_params[0].to_string();
        let coin_in = normalize_coin_type(&coin_in);
        let coin_out = event.type_.type_params[1].to_string();
        let coin_out = normalize_coin_type(&coin_out);

        Self::new(&event.parsed_json, coin_in, coin_out)
    }
}

impl TryFrom<&ShioEvent> for AbexSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        let event_type_tag = StructTag::from_str(&event.event_type).map_err(|e| eyre!(e))?;
        ensure!(
            event.event_type.starts_with(ABEX_SWAP_EVENT) && event_type_tag.type_params.len() == 2,
            "Not an AbexSwapEvent"
        );

        let coin_in = event_type_tag.type_params[0].to_string();
        let coin_in = normalize_coin_type(&coin_in);
        let coin_out = event_type_tag.type_params[1].to_string();
        let coin_out = normalize_coin_type(&coin_out);

        let parsed_json = event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?;

        Self::new(parsed_json, coin_in, coin_out)
    }
}

impl AbexSwapEvent {
    pub fn new(parsed_json: &Value, coin_in: String, coin_out: String) -> Result<Self> {
        let amount_in: u64 = parsed_json["source_amount"]
            .as_str()
            .ok_or_else(|| eyre!("Missing source_amount"))?
            .parse()?;

        let amount_out: u64 = parsed_json["dest_amount"]
            .as_str()
            .ok_or_else(|| eyre!("Missing dest_amount"))?
            .parse()?;

        Ok(Self {
            coin_in,
            coin_out,
            amount_in,
            amount_out,
        })
    }

    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::Abex,
            pool: None,
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}
