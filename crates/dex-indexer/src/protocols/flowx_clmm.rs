use std::{
    str::FromStr,
    sync::{Arc, OnceLock},
};

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
    extract_u64_from_move_struct,
};

use super::{get_coin_decimals, get_pool_coins_type, SUI_RPC_NODE};
use crate::{
    get_coin_in_out_v2,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const FLOWX_CLMM_POOL_CREATED: &str =
    "0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d::pool_manager::PoolCreated";

pub const FLOWX_CLMM_SWAP_EVENT: &str =
    "0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d::pool::Swap";

static FLOWX_CLMM_POOL_LAYOUT: OnceLock<MoveStructLayout> = OnceLock::new();

pub fn flowx_clmm_event_filter() -> EventFilter {
    EventFilter::MoveEventType(FLOWX_CLMM_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowxClmmPoolCreated {
    pub pool: ObjectID,
    pub token0: String,
    pub token1: String,
    pub fee_rate: u64,
}

impl TryFrom<&SuiEvent> for FlowxClmmPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let coin_type_x = parsed_json["coin_type_x"]
            .as_object()
            .ok_or_else(|| eyre!("Missing coin_type_x"))?;
        let token0 = coin_type_x["name"]
            .as_str()
            .ok_or_else(|| eyre!("Missing coin_type_x"))?;

        let coin_type_y = parsed_json["coin_type_y"]
            .as_object()
            .ok_or_else(|| eyre!("Missing coin_type_y"))?;
        let token1 = coin_type_y["name"]
            .as_str()
            .ok_or_else(|| eyre!("Missing coin_type_y"))?;

        let fee_rate: u64 = parsed_json["fee_rate"]
            .as_str()
            .ok_or_else(|| eyre!("Missing fee_rate"))?
            .parse()?;

        Ok(Self {
            pool,
            token0: format!("0x{token0}"),
            token1: format!("0x{token1}"),
            fee_rate,
        })
    }
}

impl FlowxClmmPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let token0_decimals = get_coin_decimals(sui, &self.token0).await?;
        let token1_decimals = get_coin_decimals(sui, &self.token1).await?;

        let tokens = vec![
            Token::new(&self.token0, token0_decimals),
            Token::new(&self.token1, token1_decimals),
        ];
        let extra = PoolExtra::FlowxClmm {
            fee_rate: self.fee_rate,
        };

        Ok(Pool {
            protocol: Protocol::FlowxClmm,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlowxClmmSwapEvent {
    pub pool: ObjectID,
    pub amount_in: u64,
    pub amount_out: u64,
    pub a2b: bool,
}

impl TryFrom<&SuiEvent> for FlowxClmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(
            event.type_.to_string() == FLOWX_CLMM_SWAP_EVENT,
            "Not a FlowxClmmSwapEvent"
        );

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for FlowxClmmSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == FLOWX_CLMM_SWAP_EVENT, "Not a FlowxClmmSwapEvent");

        event
            .parsed_json
            .as_ref()
            .ok_or(eyre!("Missing parsed_json"))?
            .try_into()
    }
}

impl TryFrom<&Value> for FlowxClmmSwapEvent {
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

impl FlowxClmmSwapEvent {
    #[allow(dead_code)]
    pub async fn to_swap_event_v1(&self, sui: &SuiClient) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_pool_coins_type(sui, self.pool).await?;
        let (coin_in, coin_out) = if self.a2b { (coin_a, coin_b) } else { (coin_b, coin_a) };

        Ok(SwapEvent {
            protocol: Protocol::FlowxClmm,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }

    pub async fn to_swap_event_v2(&self, provider: Arc<dyn Simulator>) -> Result<SwapEvent> {
        let (coin_in, coin_out) = get_coin_in_out_v2!(self.pool, provider, self.a2b);

        Ok(SwapEvent {
            protocol: Protocol::FlowxClmm,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn flowx_clmm_related_object_ids() -> Vec<String> {
    vec![
        "0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d", // FlowxClmm
        "0x67624a1533b5aff5d0dfcf5e598684350efd38134d2d245f475524c03a64e656", // Versioned
        "0x27565d24a4cd51127ac90e4074a841bbe356cca7bf5759ddc14a975be1632abc", // PoolRegistry
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

pub async fn flowx_clmm_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {
    let mut res = vec![];

    let parsed_pool = {
        let pool_obj = simulator
            .get_object(&pool.pool)
            .await
            .ok_or_else(|| eyre!("FlowxClmm pool not found: {}", pool.pool))?;

        let layout = pool_layout(pool.pool, simulator);

        let move_obj = pool_obj.data.try_as_move().ok_or_eyre("Not a Move object")?;
        MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
    };
    let sui_client = SuiClientBuilder::default()
    .build(SUI_RPC_NODE)
    .await
    .unwrap();

    // get next init_tick using obejctID 
    {
        let parent_id =
        ObjectID::from_str("0xe746a19bf5e4ef0e5aa7993d9a36e49bd8f0928390723d43f2ebbbf87c416ef2")?;
        let key_tag =
        TypeTag::from_str("0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d::i32::I32").unwrap();
        let bit_map_len = 256u32;
        for i in 0..bit_map_len {
            let key_bytes = bcs::to_bytes(&i)?;
            let child_id = derive_dynamic_field_id(parent_id, &key_tag, &key_bytes)?;
            res.push(child_id.to_string());
        }
    }

    // (coin_a, coin_b, fee_rate) ID
    {
        let pool_registry_id =
            ObjectID::from_str("0x27565d24a4cd51127ac90e4074a841bbe356cca7bf5759ddc14a975be1632abc")?;

        let coin_a = format_coin_type_for_derive(&pool.token0_type());
        let coin_b = format_coin_type_for_derive(&pool.token1_type());
        let fee_rate = extract_u64_from_move_struct(&parsed_pool, "swap_fee_rate")?;

        let key_value = (coin_a, coin_b, fee_rate);
        let key_bytes = bcs::to_bytes(&key_value)?;
        let key_tag = TypeTag::from_str("0x02::dynamic_object_field::Wrapper<0x25929e7f29e0a30eb4e692952ba1b5b65a3a4d65ab5f2a32e1ba3edcb587f26d::pool_manager::PoolDfKey>").unwrap();
        let child_id = derive_dynamic_field_id(pool_registry_id, &key_tag, &key_bytes)?;
        res.push(child_id.to_string());
    } 

    // tick bitmap IDs
    {
        let tick_bitmap = extract_struct_from_move_struct(&parsed_pool, "tick_bitmap")?;

        let tick_bitmap_id = {
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


    // ticks
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

    Ok(res)
}

fn pool_layout(pool_id: ObjectID, simulator: Arc<dyn Simulator>) -> MoveStructLayout {
    FLOWX_CLMM_POOL_LAYOUT
        .get_or_init(|| {
            simulator
                .get_object_layout(&pool_id)
                .expect("Failed to get FlowxClmm pool layout")
        })
        .clone()
}

#[inline]
fn format_coin_type_for_derive(coin_type: &str) -> String {
    let coin_tag = TypeTag::from_str(coin_type).unwrap();
    format!("{}", coin_tag.to_canonical_display(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_logger::LevelFilter;
    use simulator::DBSimulator;
    use simulator::HttpSimulator;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_swap_event_http() {
        let provider = HttpSimulator::new("", &None).await;

        let swap_event = FlowxClmmSwapEvent {
            pool: ObjectID::from_str("0x2e88a6a61327ba517dcf1c57346ed1fdd25d98e78007e389f208658224baa72f").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
        let expected_b = "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_swap_event_db() {
        let provider = DBSimulator::new_default_slow().await;

        let swap_event = FlowxClmmSwapEvent {
            pool: ObjectID::from_str("0x2e88a6a61327ba517dcf1c57346ed1fdd25d98e78007e389f208658224baa72f").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
        let expected_b = "0xdeeb7a4662eec9f2f3def03fb937a663dddaa2e215b8078a284d026b7946c270::deep::DEEP";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_flowx_clmm_pool_children_ids() {
        mev_logger::init_console_logger(Some(LevelFilter::INFO));

        let pool = Pool {
            protocol: Protocol::FlowxClmm,
            pool: ObjectID::from_str("0x1903c1715a382457f04fb5c3c3ee718871f976a4b4a589eb899096b96f8d5eba").unwrap(),
            tokens: vec![
                Token::new("0x2::sui::SUI", 9),
                Token::new(
                    "0x3fb8bdeced0dc4bf830267652ef33fe8fb60b107b3d3b6e5e088dcc0067efa06::prh::PRH",
                    9,
                ),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);
        let start = Instant::now();
        let children_ids = flowx_clmm_pool_children_ids(&pool, simulator).await.unwrap();
        println!("{:?} ===================> {:?} ", children_ids, children_ids.len());
        println!("Took ==============> : {} ms", start.elapsed().as_millis());
    }

}
