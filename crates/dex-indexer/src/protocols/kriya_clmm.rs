use std::sync::{Arc, OnceLock};

use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::annotated_value::{MoveStruct, MoveStructLayout};
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use simulator::Simulator;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::base_types::ObjectID,
    SuiClient, SuiClientBuilder,
};
use sui_types::{dynamic_field::derive_dynamic_field_id, TypeTag};
use utils::object::{
    extract_object_id_from_move_struct, extract_struct_from_move_struct,
};

use super::{get_coin_decimals, get_pool_coins_type, SUI_RPC_NODE};
use crate::{
    get_coin_in_out_v2,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const KRIYA_CLMM_POOL_CREATED: &str =
    "0xf6c05e2d9301e6e91dc6ab6c3ca918f7d55896e1f1edd64adc0e615cde27ebf1::create_pool::PoolCreatedEvent";

pub const KRIYA_CLMM_SWAP_EVENT: &str =
    "0xf6c05e2d9301e6e91dc6ab6c3ca918f7d55896e1f1edd64adc0e615cde27ebf1::trade::SwapEvent";

static KRIYA_CLMM_POOL_LAYOUT: OnceLock<MoveStructLayout> = OnceLock::new();

pub fn kriya_clmm_event_filter() -> EventFilter {
    EventFilter::MoveEventType(KRIYA_CLMM_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct KriyaClmmPoolCreated {
    pub pool: ObjectID,
    pub token0: String,
    pub token1: String,
    pub fee_rate: u64,
}

impl TryFrom<&SuiEvent> for KriyaClmmPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let typex = parsed_json["type_x"]
            .as_object()
            .ok_or_else(|| eyre!("Missing type_x"))?;
        let token0 = typex["name"].as_str().ok_or_else(|| eyre!("Missing type_x"))?;

        let typey = parsed_json["type_y"]
            .as_object()
            .ok_or_else(|| eyre!("Missing type_y"))?;
        let token1 = typey["name"].as_str().ok_or_else(|| eyre!("Missing type_y"))?;

        let fee_rate: u64 = parsed_json["fee_rate"]
            .as_str()
            .ok_or_else(|| eyre!("Missing fee_rate"))?
            .parse()?;

        Ok(Self {
            pool,
            token0: format!("0x{}", token0),
            token1: format!("0x{}", token1),
            fee_rate,
        })
    }
}

impl KriyaClmmPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let token0_decimals = get_coin_decimals(sui, &self.token0).await?;
        let token1_decimals = get_coin_decimals(sui, &self.token1).await?;

        let tokens = vec![
            Token::new(&self.token0, token0_decimals),
            Token::new(&self.token1, token1_decimals),
        ];
        let extra = PoolExtra::KriyaClmm {
            fee_rate: self.fee_rate,
        };

        Ok(Pool {
            protocol: Protocol::KriyaClmm,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KriyaClmmSwapEvent {
    pub pool: ObjectID,
    pub amount_in: u64,
    pub amount_out: u64,
    pub a2b: bool,
}

impl TryFrom<&SuiEvent> for KriyaClmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string() == KRIYA_CLMM_SWAP_EVENT,
            "Not a KriyaClmmSwapEvent"
        );

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for KriyaClmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == KRIYA_CLMM_SWAP_EVENT, "Not a KriyaClmmSwapEvent");

        event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?.try_into()
    }
}

impl TryFrom<&Value> for KriyaClmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let amount_x: u64 = parsed_json["amount_x"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_x"))?
            .parse()?;

        let amount_y: u64 = parsed_json["amount_y"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_y"))?
            .parse()?;

        let a2b = parsed_json["x_for_y"]
            .as_bool()
            .ok_or_else(|| eyre!("Missing x_for_y"))?;

        let (amount_in, amount_out) = if a2b {
            (amount_x, amount_y)
        } else {
            (amount_y, amount_x)
        };

        Ok(Self {
            pool,
            amount_in,
            amount_out,
            a2b,
        })
    }
}

impl KriyaClmmSwapEvent {
    #[allow(dead_code)]
    pub async fn to_swap_event_v1(&self, sui: &SuiClient) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_pool_coins_type(sui, self.pool).await?;
        let (coin_in, coin_out) = if self.a2b { (coin_a, coin_b) } else { (coin_b, coin_a) };

        Ok(SwapEvent {
            protocol: Protocol::KriyaClmm,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }

    // https://suiscan.xyz/mainnet/tx/EmDQqPrUeQDgk8bbM7YquDW4gF6PHCfpL4D41MoHbQW1
    pub async fn to_swap_event_v2(&self, provider: Arc<dyn Simulator>) -> Result<SwapEvent> {
        let (coin_in, coin_out) = get_coin_in_out_v2!(self.pool, provider, self.a2b);

        Ok(SwapEvent {
            protocol: Protocol::KriyaClmm,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn kriya_clmm_related_object_ids() -> Vec<String> {
    vec![
        "0xbd8d4489782042c6fafad4de4bc6a5e0b84a43c6c00647ffd7062d1e2bb7549e", // KriyaClmm
        "0xf5145a7ac345ca8736cf8c76047d00d6d378f30e81be6f6eb557184d9de93c78", // Version
        "0xf6c05e2d9301e6e91dc6ab6c3ca918f7d55896e1f1edd64adc0e615cde27ebf1", // Math
        "0x9d856cdba9618289f3262e2ede47d9bb49f0f98b007a5e24f66f46e85b1b9f5a", // Tick
        "0xe0917b74a5912e4ad186ac634e29c922ab83903f71af7500969f9411706f9b9a", // upgrade_service
        "0xecf47609d7da919ea98e7fd04f6e0648a0a79b337aaad373fa37aac8febf19c8", // treasury
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
}

pub async fn kriya_clmm_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {
    let mut res = vec![];
    let parent_id = pool.pool;
    // trading_enabled ID
    {
        let type_tag = TypeTag::Vector(Box::new(TypeTag::U8));
        let key_value = "trading_enabled".to_string();
        let key_bytes = bcs::to_bytes(&key_value)?;
        let child_id = derive_dynamic_field_id(parent_id, &type_tag, &key_bytes)?;
        res.push(child_id.to_string());
    };

    let parsed_pool = {
        let pool_obj = simulator
            .get_object(&pool.pool)
            .await
            .ok_or_else(|| eyre!("KriyaClmm pool not found: {}", pool.pool))?;

        let layout = pool_layout(pool.pool, simulator);

        let move_obj = pool_obj.data.try_as_move().ok_or_eyre("Not a Move object")?;
        MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
    };

    let sui_client = SuiClientBuilder::default()
    .build(SUI_RPC_NODE)
    .await
    .unwrap();

    // tick ID
    {
        let ticks = extract_struct_from_move_struct(&parsed_pool, "ticks")?;
        let ticks_id = {
            let id = extract_struct_from_move_struct(&ticks, "id")?;
            let id = extract_struct_from_move_struct(&id, "id")?;
            extract_object_id_from_move_struct(&id, "bytes")?
        };

        let mut next_cursor = None;
        let mut tick_vec = Vec::new();

        loop {
            let ret = sui_client.read_api().get_dynamic_fields(ticks_id, next_cursor, None).await?;
            next_cursor = ret.next_cursor;
            tick_vec.extend(ret.data); 
            if !ret.has_next_page {
                break;
            }
        }
        let tick_vec: Vec<String> = tick_vec.iter().map(|field_info| {
            field_info.object_id.to_string()
        }).collect();
        res.extend(tick_vec);
    }


    // tick_bitmap ID
    {
    

        let tick_bitmap_id = {
            let tick_bitmap = extract_struct_from_move_struct(&parsed_pool, "tick_bitmap")?;
            let id = extract_struct_from_move_struct(&tick_bitmap, "id")?;
            let id = extract_struct_from_move_struct(&id, "id")?;
            extract_object_id_from_move_struct(&id, "bytes")?
        };

        let mut next_cursor = None;
        let mut tick_vec = Vec::new();
    
        loop {
            let ret = sui_client.read_api().get_dynamic_fields(tick_bitmap_id, next_cursor, None).await?;
            next_cursor = ret.next_cursor;
            tick_vec.extend(ret.data); 
            if !ret.has_next_page {
                break;
            }
        }
        let tick_vec: Vec<String> = tick_vec.iter().map(|field_info| {
            field_info.object_id.to_string()
        }).collect();
        res.extend(tick_vec);
    }

    Ok(res)


}

fn pool_layout(pool_id: ObjectID, simulator: Arc<dyn Simulator>) -> MoveStructLayout {
    KRIYA_CLMM_POOL_LAYOUT
        .get_or_init(|| {
            simulator
                .get_object_layout(&pool_id)
                .expect("Failed to get KriyaClmm pool layout")
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use mev_logger::LevelFilter;
    use simulator::DBSimulator;
    use simulator::HttpSimulator;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_swap_event_http() {
        let provider = HttpSimulator::new(SUI_RPC_NODE, &None).await;

        let swap_event = KriyaClmmSwapEvent {
            pool: ObjectID::from_str("0x4ab1017f5a10d122fdfc6656f6c2f7cc641edc1e2d12680cd9d98cf59d4e7e7b").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x549e8b69270defbfafd4f94e17ec44cdbdd99820b33bda2278dea3b9a32d3f55::cert::CERT";
        let expected_b = "0xa99b8952d4f7d947ea77fe0ecdcc9e5fc0bcab2841d6e2a5aa00c3044e5544b5::navx::NAVX";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_swap_event_db() {
        let provider = DBSimulator::new_default_slow().await;

        let swap_event = KriyaClmmSwapEvent {
            pool: ObjectID::from_str("0x4ab1017f5a10d122fdfc6656f6c2f7cc641edc1e2d12680cd9d98cf59d4e7e7b").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x549e8b69270defbfafd4f94e17ec44cdbdd99820b33bda2278dea3b9a32d3f55::cert::CERT";
        let expected_b = "0xa99b8952d4f7d947ea77fe0ecdcc9e5fc0bcab2841d6e2a5aa00c3044e5544b5::navx::NAVX";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_kriya_clmm_pool_children_ids() {
        mev_logger::init_console_logger(Some(LevelFilter::INFO));

        let pool = Pool {
            protocol: Protocol::KriyaClmm,
            pool: ObjectID::from_str("0x367e02acb99632e18db69c3e93d89d21eb721e1d1fcebc0f6853667337450acc").unwrap(),
            tokens: vec![
                Token::new("0x2::sui::SUI", 9),
                Token::new(
                    "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
                    9,
                ),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);
        let start = Instant::now();
        let children_ids = kriya_clmm_pool_children_ids(&pool, simulator).await.unwrap();
        println!("Took ==============> : {} ms", start.elapsed().as_millis());
        println!("{:?}", children_ids);
    }
}
