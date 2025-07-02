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

pub const BABY_SWAP_EVENT: &str =
    "0x227f865230dd4fc947321619f56fee37dc7ac582eb22e3eab29816f717512d9d::liquidity_pool::EventSwap";

#[derive(Debug, Clone, Deserialize)]
pub struct BabySwapEvent {
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for BabySwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let event_type = event.type_.to_string();
        ensure!(
            event_type.starts_with(BABY_SWAP_EVENT) && event.type_.type_params.len() == 3,
            "Not a BabySwapEvent"
        );

        let coin_x = event.type_.type_params[0].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event.type_.type_params[1].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        Self::new(&event.parsed_json, coin_x, coin_y)
    }
}

impl TryFrom<&ShioEvent> for BabySwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        let event_type_tag = StructTag::from_str(&event.event_type).map_err(|e| eyre!(e))?;
        ensure!(
            event.event_type.starts_with(BABY_SWAP_EVENT) && event_type_tag.type_params.len() == 3,
            "Not a BabySwapEvent"
        );

        let coin_x = event_type_tag.type_params[0].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event_type_tag.type_params[1].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        let parsed_json = event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?;

        Self::new(parsed_json, coin_x, coin_y)
    }
}

impl BabySwapEvent {
    pub fn new(parsed_json: &Value, coin_x: String, coin_y: String) -> Result<Self> {
        let x_in: u64 = parsed_json["x_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing x_in"))?
            .parse()?;
        let x_out: u64 = parsed_json["x_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing x_out"))?
            .parse()?;
        let y_in: u64 = parsed_json["y_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing y_in"))?
            .parse()?;
        let y_out: u64 = parsed_json["y_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing y_out"))?
            .parse()?;

        let (coin_in, coin_out, amount_in, amount_out) = if x_in == 0 {
            (coin_y, coin_x, y_in, x_out)
        } else {
            (coin_x, coin_y, x_in, y_out)
        };

        Ok(Self {
            coin_in,
            coin_out,
            amount_in,
            amount_out,
        })
    }

    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::BabySwap,
            pool: None,
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}
