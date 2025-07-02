use std::{str::FromStr, sync::Arc};

use eyre::{bail, ensure, eyre, OptionExt, Result};
use move_core_types::{
    annotated_value::{MoveFieldLayout, MoveStruct, MoveStructLayout, MoveTypeLayout, MoveValue},
    language_storage::StructTag,
};
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use simulator::Simulator;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::{base_types::ObjectID, dynamic_field::derive_dynamic_field_id, TypeTag},
    SuiClient,
};
use sui_types::{dynamic_field::extract_field_from_move_struct, object::Object, Identifier};
use tracing::warn;

use super::get_coin_decimals;
use crate::{
    move_field_layout, move_struct_layout, move_type_layout_struct, normalize_coin_type,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const BLUE_MOVE_POOL_CREATED: &str =
    "0xb24b6789e088b876afabca733bed2299fbc9e2d6369be4d1acfa17d8145454d9::swap::Created_Pool_Event";

pub const BLUE_MOVE_SWAP_EVENT: &str =
    "0xb24b6789e088b876afabca733bed2299fbc9e2d6369be4d1acfa17d8145454d9::swap::Swap_Event";

const BLUE_MOVE_DEX_INFO: &str = "0x3f2d9f724f4a1ce5e71676448dc452be9a6243dac9c5b975a588c8c867066e92";

pub fn blue_move_event_filter() -> EventFilter {
    EventFilter::MoveEventType(BLUE_MOVE_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlueMovePoolCreated {
    pub pool: ObjectID,
    pub token0: String,
    pub token1: String,
}

impl TryFrom<&SuiEvent> for BlueMovePoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;
        let token0 = parsed_json["token_x_name"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_x_name"))?;
        let token0 = format!("0x{token0}");
        let token1 = parsed_json["token_y_name"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_y_name"))?;
        let token1 = format!("0x{token1}");

        Ok(Self { pool, token0, token1 })
    }
}

impl BlueMovePoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let token0_decimals = get_coin_decimals(sui, &self.token0).await?;
        let token1_decimals = get_coin_decimals(sui, &self.token1).await?;

        let tokens = vec![
            Token::new(&self.token0, token0_decimals),
            Token::new(&self.token1, token1_decimals),
        ];
        let extra = PoolExtra::None;

        Ok(Pool {
            protocol: Protocol::BlueMove,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlueMoveSwapEvent {
    pub pool: ObjectID,
    pub coin_in: String,
    pub coin_out: String,
    pub amount_in: u64,
    pub amount_out: u64,
}

impl TryFrom<&SuiEvent> for BlueMoveSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string().starts_with(BLUE_MOVE_SWAP_EVENT),
            "Not a BlueMoveSwapEvent"
        );

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for BlueMoveSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(
            event.event_type.starts_with(BLUE_MOVE_SWAP_EVENT),
            "Not a BlueMoveSwapEvent"
        );

        event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?.try_into()
    }
}

impl TryFrom<&Value> for BlueMoveSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let coin_x_in = parsed_json["token_x_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_x_in"))?;
        let amount_x_in: u64 = parsed_json["amount_x_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_x_in"))?
            .parse()?;
        let coin_y_in = parsed_json["token_y_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_y_in"))?;
        let amount_y_in: u64 = parsed_json["amount_y_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_y_in"))?
            .parse()?;
        let coin_x_out = parsed_json["token_x_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_x_out"))?;
        let amount_x_out: u64 = parsed_json["amount_x_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_x_out"))?
            .parse()?;
        let coin_y_out = parsed_json["token_y_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing token_y_out"))?;
        let amount_y_out: u64 = parsed_json["amount_y_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_y_out"))?
            .parse()?;

        let (coin_in, coin_out, amount_in, amount_out) = if amount_x_in > 0 {
            (coin_x_in, coin_y_out, amount_x_in, amount_y_out)
        } else {
            (coin_y_in, coin_x_out, amount_y_in, amount_x_out)
        };

        let coin_in = normalize_coin_type(&format!("0x{coin_in}"));
        let coin_out = normalize_coin_type(&format!("0x{coin_out}"));

        Ok(Self {
            pool,
            coin_in,
            coin_out,
            amount_in,
            amount_out,
        })
    }
}

impl BlueMoveSwapEvent {
    pub async fn to_swap_event(&self) -> Result<SwapEvent> {
        Ok(SwapEvent {
            protocol: Protocol::BlueMove,
            pool: Some(self.pool),
            coins_in: vec![self.coin_in.clone()],
            coins_out: vec![self.coin_out.clone()],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn blue_move_related_object_ids() -> Vec<String> {
    let mut result = vec![
        "0x08cd33481587d4c4612865b164796d937df13747d8c763b8a178c87e3244498f", // BlueMoveDex6
        "0xb24b6789e088b876afabca733bed2299fbc9e2d6369be4d1acfa17d8145454d9", // BlueMoveDex
        "0x7be61b62d902f3fe78d0a5e20b81b4715a47ff06cae292db8991dfea422cf57e", // BlueMove 9
        "0x41d5f1c14825d92c93cdae3508705cc31582c8aaaca501aaa4970054fd3b5b2d", // Version Manage
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>();

    // Dynamic IDs
    result.extend(dex_related_ids());

    result
}

fn dex_related_ids() -> Vec<String> {
    let dex_id = ObjectID::from_hex_literal(BLUE_MOVE_DEX_INFO).unwrap();

    let key_tag = TypeTag::from_str("0x02::dynamic_object_field::Wrapper<0x02::object::ID>").unwrap();
    let key_bytes: Vec<u8> = bcs::to_bytes(&dex_id).unwrap();
    let child = derive_dynamic_field_id(dex_id, &key_tag, &key_bytes).unwrap();

    vec![dex_id, child].into_iter().map(|id| id.to_string()).collect()
}

pub async fn blue_move_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {
    let mut res = vec![];

    let parent_id = pool.pool;
    let type_tag = TypeTag::from_str("0x02::dynamic_object_field::Wrapper<0x02::object::ID>").map_err(|e| eyre!(e))?;
    let key_bytes = bcs::to_bytes(&parent_id)?;

    // dynamic child
    let child_id = derive_dynamic_field_id(parent_id, &type_tag, &key_bytes)?;
    res.push(child_id.to_string());

    // dynamic grandson
    {
        let layout = pool_dynamic_child_layout();

        let parse_grandson_id = |child: &Object| -> Result<String> {
            let move_obj = child.data.try_as_move().ok_or_eyre("Not a Move object")?;
            let move_struct = MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?;
            let value = extract_field_from_move_struct(&move_struct, "value").ok_or_eyre("Missing value")?;
            match value {
                MoveValue::Address(addr) => Ok(addr.to_hex_literal()),
                _ => bail!("Invalid value"),
            }
        };

        if let Some(child_obj) = simulator.get_object(&child_id).await {
            match parse_grandson_id(&child_obj) {
                Ok(id) => res.push(id),
                Err(e) => {
                    warn!("Failed to parse: {child_id}, error: {e}");
                }
            }
        }
    }

    // dex info dynamic children
    {
        let dex_id = ObjectID::from_hex_literal(BLUE_MOVE_DEX_INFO).map_err(|e| eyre!(e))?;
        let type_tag1 =
            TypeTag::from_str("0x02::dynamic_object_field::Wrapper<0x01::string::String>").map_err(|e| eyre!(e))?;

        // BlueMove-{CoinA}-{CoinB}-LP
        let coin_a = format_coin_type_for_derive(&pool.tokens[0].token_type);
        let coin_b = format_coin_type_for_derive(&pool.tokens[1].token_type);
        let key_value = format!("BlueMove-{}-{}-LP", coin_a, coin_b);

        let key_bytes = bcs::to_bytes(&key_value)?;
        let child_id = derive_dynamic_field_id(dex_id, &type_tag1, &key_bytes)?;
        res.push(child_id.to_string());
    }

    Ok(res)
}

fn pool_dynamic_child_layout() -> MoveStructLayout {
    MoveStructLayout {
        type_: StructTag::from_str(
            "0x02::dynamic_field::Field<0x02::dynamic_object_field::Wrapper<0x02::object::ID>, 0x02::object::ID>",
        )
        .map_err(|e| eyre!(e))
        .unwrap(),
        fields: Box::new(vec![
            move_field_layout!(
                "id",
                move_type_layout_struct!(move_struct_layout!(
                    StructTag::from_str("0x02::object::UID").map_err(|e| eyre!(e)).unwrap(),
                    vec![move_field_layout!(
                        "id",
                        move_type_layout_struct!(move_struct_layout!(
                            StructTag::from_str("0x02::object::ID").map_err(|e| eyre!(e)).unwrap(),
                            vec![move_field_layout!("bytes", MoveTypeLayout::Address)]
                        ))
                    )]
                ))
            ),
            move_field_layout!(
                "name",
                move_type_layout_struct!(move_struct_layout!(
                    StructTag::from_str("0x02::dynamic_object_field::Wrapper<0x02::object::ID>")
                        .map_err(|e| eyre!(e))
                        .unwrap(),
                    vec![move_field_layout!("name", MoveTypeLayout::Address)]
                ))
            ),
            move_field_layout!("value", MoveTypeLayout::Address),
        ]),
    }
}

#[inline]
fn format_coin_type_for_derive(coin_type: &str) -> String {
    let coin_tag = TypeTag::from_str(coin_type).unwrap();
    format!("{}", coin_tag.to_canonical_display(false))
}

#[cfg(test)]
mod tests {

    use mev_logger::LevelFilter;
    use simulator::DBSimulator;

    use super::*;

    #[tokio::test]
    async fn test_blue_move_pool_children_ids() {
        mev_logger::init_console_logger(Some(LevelFilter::INFO));

        let pool = Pool {
            protocol: Protocol::BlueMove,
            pool: ObjectID::from_str("0xe057718861803021cb3b40ec1514b37c8f1fa36636b2dcb9de01e16009db121c").unwrap(),
            tokens: vec![
                Token::new("0x2::sui::SUI", 9),
                Token::new(
                    "0xed4504e791e1dad7bf93b41e089b4733c27f35fde505693e18186c2ba8e2e14b::suib::SUIB",
                    9,
                ),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);

        let children_ids = blue_move_pool_children_ids(&pool, simulator).await.unwrap();
        println!("{:?}", children_ids);
    }
}
