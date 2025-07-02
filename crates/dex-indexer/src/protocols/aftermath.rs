use std::sync::Arc;

use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::annotated_value::MoveStruct;
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use simulator::Simulator;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::base_types::ObjectID,
    SuiClient,
};
use sui_types::TypeTag;

use super::{get_children_ids, get_coin_decimals};
use crate::{
    normalize_coin_type,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const AFTERMATH_POOL_CREATED: &str =
    "0xefe170ec0be4d762196bedecd7a065816576198a6527c99282a2551aaa7da38c::events::CreatedPoolEvent";

pub const AFTERMATH_SWAP_EVENT: &str =
    "0xc4049b2d1cc0f6e017fda8260e4377cecd236bd7f56a54fee120816e72e2e0dd::events::SwapEventV2";

pub fn aftermath_event_filter() -> EventFilter {
    EventFilter::MoveEventType(AFTERMATH_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct AftermathPoolCreated {
    pub pool: ObjectID,
    pub lp_type: String,
    pub token_types: Vec<String>,
    pub fees_swap_in: Vec<u64>,
    pub fees_swap_out: Vec<u64>,
    pub fees_deposit: Vec<u64>,
    pub fees_withdraw: Vec<u64>,
}

impl TryFrom<&SuiEvent> for AftermathPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;

        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let lp_type = parsed_json["lp_type"]
            .as_str()
            .ok_or_else(|| eyre!("Missing lp_type"))?;

        let token_types = parsed_json["coins"]
            .as_array()
            .ok_or_else(|| eyre!("Missing coins"))?
            .iter()
            .map(|x| {
                let token_type = x.as_str().ok_or_else(|| eyre!("Missing coin"))?;
                Ok(format!("0x{}", token_type))
            })
            .collect::<Result<Vec<String>>>()?;

        let fees_swap_in = parsed_json["fees_swap_in"]
            .as_array()
            .ok_or_else(|| eyre!("Missing fees_swap_in"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing fees_swap_in"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        let fees_swap_out = parsed_json["fees_swap_out"]
            .as_array()
            .ok_or_else(|| eyre!("Missing fees_swap_out"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing fees_swap_out"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        let fees_deposit = parsed_json["fees_deposit"]
            .as_array()
            .ok_or_else(|| eyre!("Missing fees_deposit"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing fees_deposit"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        let fees_withdraw = parsed_json["fees_withdraw"]
            .as_array()
            .ok_or_else(|| eyre!("Missing fees_withdraw"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing fees_withdraw"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        Ok(Self {
            pool,
            lp_type: format!("0x{}", lp_type),
            token_types,
            fees_swap_in,
            fees_swap_out,
            fees_deposit,
            fees_withdraw,
        })
    }
}

impl AftermathPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let mut tokens = vec![];
        for token_type in &self.token_types {
            let token_decimals = get_coin_decimals(sui, token_type).await?;
            tokens.push(Token::new(token_type, token_decimals));
        }

        let extra = PoolExtra::Aftermath {
            lp_type: self.lp_type.clone(),
            fees_swap_in: self.fees_swap_in.clone(),
            fees_swap_out: self.fees_swap_out.clone(),
            fees_deposit: self.fees_deposit.clone(),
            fees_withdraw: self.fees_withdraw.clone(),
        };

        Ok(Pool {
            protocol: Protocol::Aftermath,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AftermathSwapEvent {
    pub pool: ObjectID,
    pub coins_in: Vec<String>,
    pub coins_out: Vec<String>,
    pub amounts_in: Vec<u64>,
    pub amounts_out: Vec<u64>,
}

impl TryFrom<&SuiEvent> for AftermathSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string() == AFTERMATH_SWAP_EVENT,
            "Not a AftermathSwapEvent"
        );

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for AftermathSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == AFTERMATH_SWAP_EVENT, "Not a AftermathSwapEvent");

        event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?.try_into()
    }
}

impl TryFrom<&Value> for AftermathSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let coins_in = parsed_json["types_in"]
            .as_array()
            .ok_or_else(|| eyre!("Missing types_in"))?
            .iter()
            .map(|x| {
                let coin_type = x.as_str().ok_or_else(|| eyre!("Missing types_in"))?;
                Ok(normalize_coin_type(format!("0x{}", coin_type).as_str()))
            })
            .collect::<Result<Vec<String>>>()?;

        let coins_out = parsed_json["types_out"]
            .as_array()
            .ok_or_else(|| eyre!("Missing types_out"))?
            .iter()
            .map(|x| {
                let coin_type = x.as_str().ok_or_else(|| eyre!("Missing types_out"))?;
                Ok(normalize_coin_type(format!("0x{}", coin_type).as_str()))
            })
            .collect::<Result<Vec<String>>>()?;

        let amounts_in = parsed_json["amounts_in"]
            .as_array()
            .ok_or_else(|| eyre!("Missing amounts_in"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing amounts_in"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        let amounts_out = parsed_json["amounts_out"]
            .as_array()
            .ok_or_else(|| eyre!("Missing amounts_out"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .ok_or_else(|| eyre!("Missing amounts_out"))?
                    .parse::<u64>()
                    .map_err(|e| eyre!(e))
            })
            .collect::<Result<Vec<u64>>>()?;

        Ok(Self {
            pool,
            coins_in,
            coins_out,
            amounts_in,
            amounts_out,
        })
    }
}

impl AftermathSwapEvent {
    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::Aftermath,
            pool: Some(self.pool),
            coins_in: self.coins_in.clone(),
            coins_out: self.coins_out.clone(),
            amounts_in: self.amounts_in.clone(),
            amounts_out: self.amounts_out.clone(),
        })
    }
}

pub async fn aftermath_related_object_ids() -> Vec<String> {
    let mut res = vec![
        "0xc4049b2d1cc0f6e017fda8260e4377cecd236bd7f56a54fee120816e72e2e0dd", // Aftermath AmmV2
        "0xfcc774493db2c45c79f688f88d28023a3e7d98e4ee9f48bbf5c7990f651577ae", // PoolRegistry
        "0xf194d9b1bcad972e45a7dd67dd49b3ee1e3357a00a50850c52cd51bb450e13b4", // ProtocolFeeVault
        "0x28e499dff5e864a2eafe476269a4f5035f1c16f338da7be18b103499abf271ce", // Treasury
        "0xf0c40d67b078000e18032334c3325c47b9ec9f3d9ae4128be820d54663d14e3b", // InsuranceFund
        "0x35d35b0e5b177593d8c3a801462485572fc30861e6ce96a55af6dc4730709278", // ReferralVault
        "0x0c4a3be43155b87e13082d178b04707d30d764279c8df0c224803ae57ca78f23", // Aftermath 1
        "0x1ec6a8c5ac0b8b97c287cd34b9fc6a94b53a07c930a8505952679dc8d4b3780a", // Aftermath 2
        "0xf63c58d762286cff1ef8eab36a24c836d23ec0ca19eacbafec7a0275a09cd520", // Aftermath 3
        "0xcc9864d3e331b308875c5fc8da278ee5fdb187ec3923064801e8d2883b80eca1", // Aftermath 4
        "0xc66fabf1a9253e43c70f1cc02d40a1d18db183140ecaae2a3f58fa6b66c55acf", // Aftermath 5
        "0x3ac8d096a3ee492d40cfe5307f2df364e30b6da6cb515266bca901fc08211d89", // Aftermath 6
        "0x705b7644364a8d1c04425da3cb8eea8cdc28f58bb2c1cb8f438e4888b8de3178", // Aftermath 7
        "0xdc15721baa82ba64822d585a7349a1508f76d94ae80e899b06e48369c257750e", // Aftermath 8
        "0x0f460b32bc4aae750e803c6ce1f0e231b47f4209cd0a644990e6ab0491c68e00", // Aftermath 9
        "0x2880a6bbbd8636d9e39cd35cebf78154e3843f08cf846cadb920f3f008ce1b89", // Aftermath 10
        "0x2a3beb3c89759988ac1ae0ca3b06837ea7ac263fe82aae50c8a9c1e855224f08", // Aftermath 11
        "0x4f0a1a923dd063757fd37e04a9c2cee8980008e94433c9075c390065f98e9e4b", // Aftermath 12
        "0xdb982f402a039f196f3e13cd73795db441393b5bc6eef7a0295a333808982a7d", // Aftermath 13
        "0x712579292f80c11a0c9de4ff553d6e5c4757105e83a8a3129823d2b39e65d062", // Aftermath 14
        "0x640514f8576f8515cd7925db405140e7dedb523921da48cbae1d5d4f72347ea8", // Aftermath 15
        "0x6c0e485deedfadcd39511ec3bfda765ec9048275d4730fc2c78250526181c152", // Aftermath 16
        "0xb547b6e8b963c1d183d262080b67417c99bee2670e8bbad6efd477d75d271fa5", // Aftermath 17
        "0x418cb79536e45a7003dff6237218747656f22f3db15fac474ae54b016a2ddc33", // Aftermath 18
        "0x0625dc2cd40aee3998a1d6620de8892964c15066e0a285d8b573910ed4c75d50", // Aftermath Amm Interface
        "0xefe170ec0be4d762196bedecd7a065816576198a6527c99282a2551aaa7da38c", // Aftermath AmmV1
        "0x0b572349baf4526c92c4e5242306e07a1658fa329ae93d1b9db0fc38b8a592bb", // Aftermath Safe
        "0x2d9316f1f1a95f6d7c85a4e690ef7c359e6649773ef2c37ad7d9857adb6bef06", // ProtocolFee
        "0x64213b0e4a52bac468d4ac3f140242f70714381653a1919a6d57cd49c628207a", // Treasury
        "0x8d8bba50c626753589aa5abbc006c9fa07736f55f4e6fb57481682997c0b0d52", // Interface V2
        "0xd2b95022244757b0ab9f74e2ee2fb2c3bf29dce5590fa6993a85d64bd219d7e8", // ReferralVault
        "0xe5099fcd45747074d0ef5eabce07a9bd1c3b0c1862435bf2a09c3a81e0604373", // Core
        "0xceb3b6f35b71dbd0296cd96f8c00959c230854c7797294148b413094b9621b0e", // Treasury
        "0xa6baab1e668c7868991c1c3c11e144100f5734c407d020f72a01b9d1a8bcb97f", // Insurance
        "0xe7d60660de1c258e33987560d657d94fbf965b063cef84077afb4c702ba3c085", // TreasuryFund
        "0xc505da612b69f7e39d2c8ad591cf5675691a70209567c3520f2b90f10504eb1e", // InsuranceFund
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>();

    let parent_ids = res
        .iter()
        .map(|id| ObjectID::from_hex_literal(id).unwrap())
        .collect::<Vec<_>>();

    for id in parent_ids {
        if let Ok(children) = get_children_ids(id).await {
            res.extend(children);
        }
    }

    res
}

pub async fn aftermath_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {
    let mut result = vec![];

    let pool_obj = simulator
        .get_object(&pool.pool)
        .await
        .ok_or_else(|| eyre!("Aftermath pool not found: {}", pool.pool))?;

    let parsed_pool = {
        let layout = simulator
            .get_object_layout(&pool.pool)
            .ok_or_eyre("pool layout not found")?;

        let move_obj = pool_obj.data.try_as_move().ok_or_eyre("not a move object")?;
        MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
    };
    let type_params = parsed_pool.type_.type_params.clone();
    for type_param in type_params {
        let object_id = match type_param {
            TypeTag::Struct(s) => s.address.to_hex_literal(),
            _ => continue,
        };
        result.push(object_id.to_string());
    }

    if let Ok(children_ids) = get_children_ids(pool.pool).await {
        result.extend(children_ids);
    }

    Ok(result)
}
