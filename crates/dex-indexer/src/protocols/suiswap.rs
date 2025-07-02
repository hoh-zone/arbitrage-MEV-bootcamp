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

pub const SUISWAP_SWAP_EVENT: &str =
    "0x361dd589b98e8fcda9a7ee53b85efabef3569d00416640d2faa516e3801d7ffc::pool::SwapTokenEvent";

#[derive(Debug, Clone, Deserialize)]
pub struct SuiswapSwapEvent {
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for SuiswapSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string().starts_with(SUISWAP_SWAP_EVENT) && event.type_.type_params.len() == 2,
            "Not a SuiswapSwapEvent"
        );

        let coin_x = event.type_.type_params[0].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event.type_.type_params[1].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        Self::new(&event.parsed_json, coin_x, coin_y)
    }
}

impl TryFrom<&ShioEvent> for SuiswapSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        let event_type_tag = StructTag::from_str(&event.event_type).map_err(|e| eyre!(e))?;
        ensure!(
            event.event_type.starts_with(SUISWAP_SWAP_EVENT) && event_type_tag.type_params.len() == 2,
            "Not a SuiswapSwapEvent"
        );

        let coin_x = event_type_tag.type_params[0].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event_type_tag.type_params[1].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        let parsed_json = event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?;

        Self::new(parsed_json, coin_x, coin_y)
    }
}

impl SuiswapSwapEvent {
    pub fn new(parsed_json: &Value, coin_x: String, coin_y: String) -> Result<Self> {
        let amount_in: u64 = parsed_json["in_amount"]
            .as_str()
            .ok_or_else(|| eyre!("Missing in_amount"))?
            .parse()?;
        let amount_out: u64 = parsed_json["out_amount"]
            .as_str()
            .ok_or_else(|| eyre!("Missing out_amount"))?
            .parse()?;
        let x_to_y: bool = parsed_json["x_to_y"].as_bool().ok_or_else(|| eyre!("Missing x_to_y"))?;

        let (coin_in, coin_out) = if x_to_y { (coin_x, coin_y) } else { (coin_y, coin_x) };

        Ok(Self {
            coin_in,
            coin_out,
            amount_in,
            amount_out,
        })
    }

    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::SuiSwap,
            pool: None,
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}
