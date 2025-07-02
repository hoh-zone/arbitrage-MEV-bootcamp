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

pub const INTEREST_SWAP_EVENT: &str =
    "0x5c45d10c26c5fb53bfaff819666da6bc7053d2190dfa29fec311cc666ff1f4b0::core::SwapToken";

#[derive(Debug, Clone, Deserialize)]
pub struct InterestSwapEvent {
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for InterestSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let event_type = event.type_.to_string();
        ensure!(
            event_type.starts_with(INTEREST_SWAP_EVENT) && event.type_.type_params.len() == 3,
            "Not an InterestSwapEvent"
        );

        let x_to_y = event_type.contains("SwapTokenX");

        let coin_x = event.type_.type_params[1].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event.type_.type_params[2].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        let (coin_in, coin_out) = if x_to_y { (coin_x, coin_y) } else { (coin_y, coin_x) };

        Self::new(&event.parsed_json, coin_in, coin_out, x_to_y)
    }
}

impl TryFrom<&ShioEvent> for InterestSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        let event_type_tag = StructTag::from_str(&event.event_type).map_err(|e| eyre!(e))?;
        ensure!(
            event.event_type.starts_with(INTEREST_SWAP_EVENT) && event_type_tag.type_params.len() == 3,
            "Not an InterestSwapEvent"
        );

        let x_to_y = event.event_type.contains("SwapTokenX");

        let coin_x = event_type_tag.type_params[1].to_string();
        let coin_x = normalize_coin_type(&coin_x);
        let coin_y = event_type_tag.type_params[2].to_string();
        let coin_y = normalize_coin_type(&coin_y);

        let (coin_in, coin_out) = if x_to_y { (coin_x, coin_y) } else { (coin_y, coin_x) };

        let parsed_json = event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?;

        Self::new(parsed_json, coin_in, coin_out, x_to_y)
    }
}

impl InterestSwapEvent {
    pub fn new(parsed_json: &Value, coin_in: String, coin_out: String, x_to_y: bool) -> Result<Self> {
        let amount_in: u64 = if x_to_y {
            parsed_json["coin_x_in"]
                .as_str()
                .ok_or_else(|| eyre!("Missing coin_x_in"))?
                .parse()?
        } else {
            parsed_json["coin_y_in"]
                .as_str()
                .ok_or_else(|| eyre!("Missing coin_y_in"))?
                .parse()?
        };

        let amount_out: u64 = if x_to_y {
            parsed_json["coin_y_out"]
                .as_str()
                .ok_or_else(|| eyre!("Missing coin_y_out"))?
                .parse()?
        } else {
            parsed_json["coin_x_out"]
                .as_str()
                .ok_or_else(|| eyre!("Missing coin_x_out"))?
                .parse()?
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
            protocol: Protocol::Interest,
            pool: None,
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}
