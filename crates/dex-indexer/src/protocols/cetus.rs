use std::{
    collections::HashSet,
    str::FromStr,
    sync::{Arc, OnceLock},
};

use eyre::{ensure, eyre, OptionExt, Result};
use move_core_types::annotated_value::{MoveStruct, MoveStructLayout};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use shio::ShioEvent;
use simulator::{SimulateCtx, Simulator};
use sui_sdk::{
    rpc_types::{EventFilter, SuiData, SuiEvent, SuiObjectDataOptions},
    types::{base_types::ObjectID, TypeTag},
    SuiClient, SuiClientBuilder,
};
use sui_types::{
    base_types::SuiAddress, dynamic_field::derive_dynamic_field_id, object::Object, programmable_transaction_builder::ProgrammableTransactionBuilder, transaction::{Command, TransactionData}, Identifier
};
use utils::object::*;

use super::{get_coin_decimals, get_pool_coins_type, SUI_RPC_NODE};
use crate::{
    get_coin_in_out_v2,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const CETUS_POOL_CREATED: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::factory::CreatePoolEvent";

pub const CETUS_SWAP_EVENT: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::SwapEvent";

const CETUS_PACKAGE_ID: &str = "0x3a5aa90ffa33d09100d7b6941ea1c0ffe6ab66e77062ddd26320c1b073aabb10";
const TICK_BOUND: i64 = 443636;

static CETUS_POOL_LAYOUT: OnceLock<MoveStructLayout> = OnceLock::new();

pub fn cetus_event_filter() -> EventFilter {
    EventFilter::MoveEventType(CETUS_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct CetusPoolCreated {
    pub pool: ObjectID,
    pub token0: String,
    pub token1: String,
} 

impl TryFrom<&SuiEvent> for CetusPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool_id"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;
        let token0 = parsed_json["coin_type_a"]
            .as_str()
            .ok_or_else(|| eyre!("Missing coin_type_a"))?;
        let token0 = format!("0x{token0}");
        let token1 = parsed_json["coin_type_b"]
            .as_str()
            .ok_or_else(|| eyre!("Missing coin_type_b"))?;
        let token1 = format!("0x{token1}");

        Ok(Self { pool, token0, token1 })
    }
}

impl CetusPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let token0_decimals = get_coin_decimals(sui, &self.token0).await?;
        let token1_decimals = get_coin_decimals(sui, &self.token1).await?;

        let opts = SuiObjectDataOptions::default().with_content();

        let pool_obj = sui
            .read_api()
            .get_object_with_options(self.pool, opts)
            .await?
            .data
            .ok_or_else(|| eyre!("Pool not found"))?;

        let fee_rate: u64 = pool_obj
            .content
            .ok_or_else(|| eyre!("Pool has no content"))?
            .try_into_move()
            .ok_or_else(|| eyre!("Pool content is not Move"))?
            .fields
            .field_value("fee_rate")
            .ok_or_else(|| eyre!("Missing fee_rate"))?
            .to_string()
            .parse()?;

        let tokens = vec![
            Token::new(&self.token0, token0_decimals),
            Token::new(&self.token1, token1_decimals),
        ];
        let extra = PoolExtra::Cetus { fee_rate };

        Ok(Pool {
            protocol: Protocol::Cetus,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CetusSwapEvent {
    pub pool: ObjectID,
    pub amount_in: u64,
    pub amount_out: u64,
    pub a2b: bool,
}

impl TryFrom<&SuiEvent> for CetusSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(event.type_.to_string() == CETUS_SWAP_EVENT, "Not a CetusSwapEvent");

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for CetusSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == CETUS_SWAP_EVENT, "Not a CetusSwapEvent");

        event
            .parsed_json
            .as_ref()
            .ok_or(eyre!("Missing parsed_json"))?
            .try_into()
    }
}

impl TryFrom<&Value> for CetusSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let pool = parsed_json["pool"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool"))?
            .parse()?;

        let a2b = parsed_json["atob"].as_bool().ok_or_else(|| eyre!("Missing atob"))?;

        let amount_in: u64 = parsed_json["amount_in"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_in"))?
            .parse()?;

        let amount_out: u64 = parsed_json["amount_out"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_out"))?
            .parse()?;

        Ok(Self {
            pool,
            amount_in,
            amount_out,
            a2b,
        })
    }
}

impl CetusSwapEvent {
    #[allow(dead_code)]
    pub async fn to_swap_event_v1(&self, sui: &SuiClient) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_pool_coins_type(sui, self.pool).await?;
        let (coin_in, coin_out) = if self.a2b { (coin_a, coin_b) } else { (coin_b, coin_a) };

        Ok(SwapEvent {
            protocol: Protocol::Cetus,
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
            protocol: Protocol::Cetus,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn cetus_related_object_ids() -> Vec<String> {
    vec![
        "0xeffc8ae61f439bb34c9b905ff8f29ec56873dcedf81c7123ff2f1f67c45ec302", // CetusAggregator
        "0x11451575c775a3e633437b827ecbc1eb51a5964b0302210b28f5b89880be21a2", // CetusAggregator 2
        "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb", // Cetus 4
        "0x70968826ad1b4ba895753f634b0aea68d0672908ca1075a2abdf0fc9e0b2fc6a", // Cetus 19
        "0x3a5aa90ffa33d09100d7b6941ea1c0ffe6ab66e77062ddd26320c1b073aabb10", // Cetus 35
        "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f", // Config
        "0x639b5e433da31739e800cd085f356e64cae222966d0f1b11bd9dc76b322ff58b", // Partner
        "0x714a63a0dba6da4f017b42d5d0fb78867f18bcde904868e51d951a5a6f5b7f57", // PoolTickIndex
        "0xbe21a06129308e0495431d12286127897aff07a8ade3970495a4404d97f9eaaa", // PoolMath 1
        "0xe2b515f0052c0b3f83c23db045d49dbe1732818ccfc5d4596c9482f7f2e76a85", // PoolMath 2
        "0xe93247b408fe44ed0ee5b6ac508b36325b239d6333e44ffa240dcc0c1a69cdd8", // PoolMath 3
        "0x74bb5afd49dddf13007101238012c033a5138474e00338126b318b5e3e4603a9", // Frequent Unkown ID
        "0xbfda3feb64a496c8d7fbb39a152d632ec1d1cefb2010b349adc3460937a592fe"  // Frequent Unkown ID
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()
}

pub async fn cetus_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {
    let mut result = vec![];

    let pool_obj = simulator
        .get_object(&pool.pool)
        .await
        .ok_or_else(|| eyre!("Cetus pool not found: {}", pool.pool))?;

    let parsed_pool = {
        let layout = pool_layout(pool.pool, simulator.clone());

        let move_obj = pool_obj.data.try_as_move().ok_or_eyre("Not a Move object")?;
        MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
    };

    let tick_manager = extract_struct_from_move_struct(&parsed_pool, "tick_manager")?;

    // get position id
    let position_manager = extract_struct_from_move_struct(&parsed_pool, "position_manager")?;
    let positions = extract_struct_from_move_struct(&position_manager, "positions")?;
    let positions_id = {
        let id = extract_struct_from_move_struct(&positions, "id")?;
        let id = extract_struct_from_move_struct(&id, "id")?;
        let id = extract_object_id_from_move_struct(&id, "bytes")?;

        id
    };
    let sui_client = SuiClientBuilder::default()
    .build(SUI_RPC_NODE)
    .await
    .unwrap();

    let mut next_cursor = None;
    let mut tick_vec = Vec::new();

    loop {
        let ret = sui_client.read_api().get_dynamic_fields(positions_id, next_cursor, None).await?;
        next_cursor = ret.next_cursor;
        tick_vec.extend(ret.data); 
        if !ret.has_next_page {
            break;
        }
    }

    let tick_vec: Vec<String> = tick_vec.iter().map(|field_info| {
        field_info.object_id.to_string()
    }).collect();
   
    result.extend(tick_vec);

     // get tick id
    let key_tag = TypeTag::U64;
    let ticks = {
        let mut result = HashSet::new();
        // let key_tag = TypeTag::U64;

        let ticks = extract_struct_from_move_struct(&tick_manager, "ticks")?;
        let id = {
            let id = extract_struct_from_move_struct(&ticks, "id")?;
            let id = extract_struct_from_move_struct(&id, "id")?;
            let id = extract_object_id_from_move_struct(&id, "bytes")?;

            id
        };

        let mut next_cursor = None;
        let mut tick_vec = Vec::new();
    
        loop {
            let ret = sui_client.read_api().get_dynamic_fields(id, next_cursor, None).await?;
            next_cursor = ret.next_cursor;
            tick_vec.extend(ret.data); 
            if !ret.has_next_page {
                break;
            }
        }

        let tick_vec: Vec<String> = tick_vec.iter().map(|field_info| {
            field_info.object_id.to_string()
        }).collect();
    
        result.extend(tick_vec);

        for tick_score in get_tick_scores(pool, &pool_obj, simulator).await? {
            if tick_score == 0 {
                continue;
            }
            let key_bytes = bcs::to_bytes(&tick_score)?;
            let tick_id = derive_dynamic_field_id(id, &key_tag, &key_bytes)?;
            result.insert(tick_id.to_string());
        }

        result
    };

    result.extend(ticks);

    Ok(result)
}

fn parse_tick_scores(event: &SuiEvent) -> Result<Vec<u64>> {
    let parsed_json = &event.parsed_json;
    let ticks = parsed_json["ticks"].as_array().ok_or_eyre("Missing ticks")?;

    let result = ticks
        .par_iter()
        .filter_map(|tick| {
            let index = tick["index"].as_object().ok_or_eyre("Missing index").ok()?;
            let index: i32 = index["bits"].as_u64().ok_or_eyre("Missing bits").ok()? as i32;
            let tick_score = (index as i64 + TICK_BOUND) as u64;
            Some(tick_score)
        })
        .collect::<Vec<_>>();

    Ok(result)
}

async fn get_tick_scores(pool: &Pool, pool_obj: &Object, simulator: Arc<dyn Simulator>) -> Result<Vec<u64>> {
    let mut ptb = ProgrammableTransactionBuilder::new();

    let package = ObjectID::from_hex_literal(CETUS_PACKAGE_ID)?;
    let module = Identifier::new("fetcher_script").unwrap();
    let function = Identifier::new("fetch_ticks").unwrap();

    let type_args = vec![
        TypeTag::from_str(pool.token0_type().as_str()).unwrap(),
        TypeTag::from_str(pool.token1_type().as_str()).unwrap(),
    ];

    let args = {
        let pool_arg = ptb.obj(shared_obj_arg(pool_obj, true)).unwrap();
        let start: Vec<u32> = vec![];
        let start_arg = ptb.pure(start).unwrap();
        let limit_arg = ptb.pure(512u64).unwrap();

        vec![pool_arg, start_arg, limit_arg]
    };

    ptb.command(Command::move_call(package, module, function, type_args, args));

    let pt = ptb.finish();
    let sender = SuiAddress::random_for_testing_only();
    let tx_data = TransactionData::new_programmable(sender, vec![], pt, 1000000000, 10000);

    let ctx = SimulateCtx::default();
    let db_res = simulator.simulate(tx_data, ctx).await?;

    let tick_scores = db_res
        .events
        .data
        .into_iter()
        .filter_map(|sui_event| parse_tick_scores(&sui_event).ok())
        .flatten()
        .collect::<Vec<_>>();

    Ok(tick_scores)
}

fn pool_layout(pool_id: ObjectID, simulator: Arc<dyn Simulator>) -> MoveStructLayout {
    CETUS_POOL_LAYOUT
        .get_or_init(|| {
            simulator
                .get_object_layout(&pool_id)
                .expect("Failed to get Cetus pool layout")
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mev_logger::LevelFilter;
    use simulator::{DBSimulator, HttpSimulator};
    use tokio::time::Instant;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_swap_event_http() {
        let provider = HttpSimulator::new("", &None).await;

        let swap_event = CetusSwapEvent {
            pool: ObjectID::from_str("0xdb36a73be4abfad79dc57e986f59294cd33f3c43bdf7cf265376f624be60cb18").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0xa99b8952d4f7d947ea77fe0ecdcc9e5fc0bcab2841d6e2a5aa00c3044e5544b5::navx::NAVX";
        let expected_b = "0x549e8b69270defbfafd4f94e17ec44cdbdd99820b33bda2278dea3b9a32d3f55::cert::CERT";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_swap_event_db() {
        let provider = DBSimulator::new_default_slow().await;

        let swap_event = CetusSwapEvent {
            pool: ObjectID::from_str("0xdb36a73be4abfad79dc57e986f59294cd33f3c43bdf7cf265376f624be60cb18").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0xa99b8952d4f7d947ea77fe0ecdcc9e5fc0bcab2841d6e2a5aa00c3044e5544b5::navx::NAVX";
        let expected_b = "0x549e8b69270defbfafd4f94e17ec44cdbdd99820b33bda2278dea3b9a32d3f55::cert::CERT";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_cetus_pool_children_ids() {
        mev_logger::init_console_logger(Some(LevelFilter::INFO));

        let pool = Pool {
            protocol: Protocol::Cetus,
            pool: ObjectID::from_str("0x3c3dd05e348fba5d8bf6958369cc3b33c8e8be85c96e10b1ca6413ad1b2d7787").unwrap(),
            tokens: vec![
                Token::new(
                    "0xdb5162ae510a06dd9ce09016612e64328a27914e9570048bbb8e61b2cb5d6b3e::kw::KW",
                    9,
                ),
                Token::new("0x2::sui::SUI", 9),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);

        let start = Instant::now();
        let children_ids = cetus_pool_children_ids(&pool, simulator).await.unwrap();
        println!("Took==============> : {} ms", start.elapsed().as_millis());
        println!("{:?}", children_ids);
    } 

    #[tokio::test]
    async fn test_judge_cetus_pool_children_ids() {
        let pool = Pool {
            
            protocol: Protocol::Cetus,
            pool: ObjectID::from_str("0xefb30c2780bb10ffd4cf860049248dcc4b204927ca63c4c2e4d0ae5666a280d5").unwrap(),
            tokens: vec![
                Token::new(
                    "0xdb5162ae510a06dd9ce09016612e64328a27914e9570048bbb8e61b2cb5d6b3e::kw::KW",
                    9,
                ),
                Token::new("0x2::sui::SUI", 9),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);

        // let start = Instant::now();
        let children_ids = cetus_pool_children_ids(&pool, simulator).await.unwrap();
        if children_ids.contains(&"0x2dd0e8a1758121da7fc615a7d8923ffeaeb9ae5852882d2d4179193e3b9e7c1e".to_string()) || children_ids.contains(&"0x26e641e6c1734ed2733701e6f7708f0c8816c665c31b89a7cfd6fee3ffdcfb82".to_string()) {
            println!("==================> Success");
        } else {
            println!("==================> Failed");
        }
        // Get Position

    }
}
