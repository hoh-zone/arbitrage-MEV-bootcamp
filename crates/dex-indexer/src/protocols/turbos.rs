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
// use sui_types::{dynamic_field::derive_dynamic_field_id, TypeTag};

use utils::object::{
    extract_object_id_from_move_struct, extract_struct_from_move_struct,
};

use super::{get_coin_decimals, get_pool_coins_type, SUI_RPC_NODE};
use crate::{
    get_coin_in_out_v2,
    types::{Pool, PoolExtra, Protocol, SwapEvent, Token},
};

const TURBOS_POOL_CREATED: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool_factory::PoolCreatedEvent";

pub const TURBOS_SWAP_EVENT: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::SwapEvent";

static TURBOS_POOL_LAYOUT: OnceLock<MoveStructLayout> = OnceLock::new();

pub fn turbos_event_filter() -> EventFilter {
    EventFilter::MoveEventType(TURBOS_POOL_CREATED.parse().unwrap())
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurbosPoolCreated {
    pub pool: ObjectID,
    pub fee: u32,
}

impl TryFrom<&SuiEvent> for TurbosPoolCreated {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        let parsed_json = &event.parsed_json;
        let pool = parsed_json["pool"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;
        let fee = parsed_json["fee"].as_u64().ok_or_else(|| eyre!("Missing fee"))? as u32;

        Ok(Self { pool, fee })
    }
}

impl TurbosPoolCreated {
    pub async fn to_pool(&self, sui: &SuiClient) -> Result<Pool> {
        let (token0_type, token1_type) = get_pool_coins_type(sui, self.pool).await?;

        let token0_decimals = get_coin_decimals(sui, &token0_type).await?;
        let token1_decimals = get_coin_decimals(sui, &token1_type).await?;

        let tokens = vec![
            Token::new(&token0_type, token0_decimals),
            Token::new(&token1_type, token1_decimals),
        ];
        let extra = PoolExtra::Turbos { fee: self.fee };

        Ok(Pool {
            protocol: Protocol::Turbos,
            pool: self.pool,
            tokens,
            extra,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TurbosSwapEvent {
    pub pool: ObjectID,
    pub amount_in: u64,
    pub amount_out: u64,
    pub a2b: bool,
}

impl TryFrom<&SuiEvent> for TurbosSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        ensure!(event.type_.to_string() == TURBOS_SWAP_EVENT, "Not a TurbosSwapEvent");

        (&event.parsed_json).try_into()
    }
}

impl TryFrom<&ShioEvent> for TurbosSwapEvent {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        ensure!(event.event_type == TURBOS_SWAP_EVENT, "Not a TurbosSwapEvent");

        event.parsed_json.as_ref().ok_or_eyre("Missing parsed_json")?.try_into()
    }
}

impl TryFrom<&Value> for TurbosSwapEvent {
    type Error = eyre::Error;

    fn try_from(parsed_json: &Value) -> Result<Self> {
        let pool = parsed_json["pool"]
            .as_str()
            .ok_or_else(|| eyre!("Missing pool_id"))?
            .parse()?;

        let amount_a: u64 = parsed_json["amount_a"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_a"))?
            .parse()?;

        let amount_b: u64 = parsed_json["amount_b"]
            .as_str()
            .ok_or_else(|| eyre!("Missing amount_b"))?
            .parse()?;

        let a2b = parsed_json["a_to_b"].as_bool().ok_or_else(|| eyre!("Missing a_to_b"))?;

        let (amount_in, amount_out) = if a2b {
            (amount_a, amount_b)
        } else {
            (amount_b, amount_a)
        };

        Ok(Self {
            pool,
            amount_in,
            amount_out,
            a2b,
        })
    }
}

impl TurbosSwapEvent {
    #[allow(dead_code)]
    pub async fn to_swap_event_v1(&self, sui: &SuiClient) -> Result<SwapEvent> {
        let (coin_a, coin_b) = get_pool_coins_type(sui, self.pool).await?;
        let (coin_in, coin_out) = if self.a2b { (coin_a, coin_b) } else { (coin_b, coin_a) };

        Ok(SwapEvent {
            protocol: Protocol::Turbos,
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
            protocol: Protocol::Turbos,
            pool: Some(self.pool),
            coins_in: vec![coin_in],
            coins_out: vec![coin_out],
            amounts_in: vec![self.amount_in],
            amounts_out: vec![self.amount_out],
        })
    }
}

pub fn turbos_related_object_ids() -> Vec<String> {
    vec![
        "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1", // Turbos 1
        "0x1a3c42ded7b75cdf4ebc7c7b7da9d1e1db49f16fcdca934fac003f35f39ecad9", // Turbos 4
        "0xdc67d6de3f00051c505da10d8f6fbab3b3ec21ec65f0dc22a2f36c13fc102110", // Turbos 9
        "0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f", // Versioned
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

pub async fn turbos_pool_children_ids(pool: &Pool, simulator: Arc<dyn Simulator>) -> Result<Vec<String>> {

    let parsed_pool = {
        let pool_obj = simulator
            .get_object(&pool.pool)
            .await
            .ok_or_else(|| eyre!("Turbos pool not found: {}", pool.pool))?;

        let layout = pool_layout(pool.pool, simulator);

        let move_obj = pool_obj.data.try_as_move().ok_or_eyre("Not a Move object")?;
        MoveStruct::simple_deserialize(move_obj.contents(), &layout).map_err(|e| eyre!(e))?
    };

    let tick_map = extract_struct_from_move_struct(&parsed_pool, "tick_map")?;

    let tickmap_id = {
        let id = extract_struct_from_move_struct(&tick_map, "id")?;
        let id = extract_struct_from_move_struct(&id, "id")?;
        extract_object_id_from_move_struct(&id, "bytes")?
    };

    let sui_client = SuiClientBuilder::default()
    .build(SUI_RPC_NODE)
    .await
    .unwrap();

    let mut next_cursor = None;
    let mut tick_vec = Vec::new();

    loop {
        let ret = sui_client.read_api().get_dynamic_fields(tickmap_id, next_cursor, None).await?;
        next_cursor = ret.next_cursor;
        tick_vec.extend(ret.data); 
        if !ret.has_next_page {
            break;
        }
    }

    let tick_vec: Vec<String> = tick_vec.iter().map(|field_info| {
        field_info.object_id.to_string()
    }).collect();

    // println!("tick_vec ======> {:?}", tick_vec);

    Ok(tick_vec)

}


fn pool_layout(pool_id: ObjectID, simulator: Arc<dyn Simulator>) -> MoveStructLayout {
    TURBOS_POOL_LAYOUT
        .get_or_init(|| {
            simulator
                .get_object_layout(&pool_id)
                .expect("Failed to get Turbos pool layout")
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

    #[tokio::test]
    async fn test_swap_event_http() {
        let provider = HttpSimulator::new("", &None).await;

        let swap_event = TurbosSwapEvent {
            pool: ObjectID::from_str("0x77f786e7bbd5f93f7dc09edbcffd9ea073945564767b65cf605f388328449d50").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x2::sui::SUI";
        let expected_b = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_swap_event_db() {
        let provider = DBSimulator::new_default_slow().await;

        let swap_event = TurbosSwapEvent {
            pool: ObjectID::from_str("0x77f786e7bbd5f93f7dc09edbcffd9ea073945564767b65cf605f388328449d50").unwrap(),
            amount_in: 0x1337,
            amount_out: 0x1338,
            a2b: true,
        };

        let swap_event = swap_event.to_swap_event_v2(Arc::new(provider)).await.unwrap();
        let expected_a = "0x2::sui::SUI";
        let expected_b = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";

        assert_eq!(swap_event.coins_in[0], expected_a);
        assert_eq!(swap_event.coins_out[0], expected_b);
    }

    #[tokio::test]
    async fn test_turbos_pool_children_ids() {
        mev_logger::init_console_logger(Some(LevelFilter::INFO));

        let pool = Pool {
            protocol: Protocol::Turbos,
            pool: ObjectID::from_str("0x0df4f02d0e210169cb6d5aabd03c3058328c06f2c4dbb0804faa041159c78443").unwrap(),
            tokens: vec![
                Token::new("0x2::sui::SUI", 9),
                Token::new(
                    "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
                    9,
                ),
            ],
            extra: PoolExtra::None,
        };

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);

        let children_ids = turbos_pool_children_ids(&pool, simulator).await.unwrap();
        println!("{:?}", children_ids);
    }

    // #[tokio::test]
    // async fn test_turbos_pool_children_ids2() {
    //     mev_logger::init_console_logger(Some(LevelFilter::INFO));

    //     let pool = Pool {
    //         protocol: Protocol::Turbos,
    //         pool: ObjectID::from_str("0x0df4f02d0e210169cb6d5aabd03c3058328c06f2c4dbb0804faa041159c78443").unwrap(),
    //         tokens: vec![
    //             Token::new("0x2::sui::SUI", 9),
    //             Token::new(
    //                 "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
    //                 9,
    //             ),
    //         ],
    //         extra: PoolExtra::None,
    //     };

    //     let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);

    //     let children_ids = turbos_pool_children_ids2(&pool, simulator).await.unwrap();
    //     // println!("{:?}", children_ids);
    // }

}
