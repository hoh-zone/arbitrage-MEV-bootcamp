pub mod abex;
pub mod aftermath;
pub mod babyswap;
pub mod blue_move;
pub mod cetus;
pub mod deepbook_v2;
pub mod flowx_amm;
pub mod flowx_clmm;
pub mod interest;
pub mod kriya_amm;
pub mod kriya_clmm;
pub mod navi;
pub mod suiswap;
pub mod turbos;

use std::str::FromStr;

use cached::proc_macro::cached;
use eyre::{bail, ensure, eyre, Result};

use sui_sdk::{
    rpc_types::SuiObjectDataOptions,
    types::{base_types::ObjectID, TypeTag},
    SuiClient, SuiClientBuilder,
};

use crate::{blockberry, normalize_coin_type};

pub const SUI_RPC_NODE: &str = "";

#[cached(key = "String", convert = r##"{ coin_type.to_string() }"##, result = true)]
pub async fn get_coin_decimals(sui: &SuiClient, coin_type: &str) -> Result<u8> {
    let coin_meta = sui.coin_read_api().get_coin_metadata(coin_type.into()).await?;
    if let Some(meta) = coin_meta {
        return Ok(meta.decimals);
    }

    // fallback to blockberry
    match blockberry::get_coin_decimals(coin_type).await {
        Ok(decimals) => Ok(decimals),
        Err(e) => Err(e),
    }
}

#[cached(key = "String", convert = r##"{ pool_id.to_string() }"##, result = true)]
pub async fn get_pool_coins_type(sui: &SuiClient, pool_id: ObjectID) -> Result<(String, String)> {
    let opts = SuiObjectDataOptions::default().with_type();
    let obj = sui
        .read_api()
        .get_object_with_options(pool_id, opts)
        .await?
        .into_object()?;

    let pool_type = obj.object_type().map_err(|e| eyre!(e))?.to_string();
    let type_tag =
        TypeTag::from_str(&pool_type).map_err(|_| eyre!("invalid pool_type: {}, object_id: {}", pool_type, pool_id))?;
    let struct_tag = match type_tag {
        TypeTag::Struct(s) => s,
        _ => bail!("invalid pool_type: {}, object_id: {}", pool_type, pool_id),
    };

    ensure!(
        struct_tag.type_params.len() >= 2,
        "invalid pool_type: {}, object_id: {}",
        pool_type,
        pool_id
    );

    let coin_a = struct_tag.type_params[0].to_string();
    let coin_a = normalize_coin_type(&coin_a);
    let coin_b = struct_tag.type_params[1].to_string();
    let coin_b = normalize_coin_type(&coin_b);

    Ok((coin_a, coin_b))
}

#[macro_export]
macro_rules! get_coin_in_out_v2 {
    ($pool:expr, $provider:expr, $a2b:expr) => {{
        let obj_inner = $provider
            .get_object(&$pool)
            .await
            .ok_or_else(|| eyre!("object not found"))?
            .into_inner();

        let obj = obj_inner
            .data
            .try_as_move()
            .ok_or_else(|| eyre!("object is not a move object"))?;

        let type_params = obj.type_().type_params();
        let coin_a = match type_params.first() {
            Some(sui_sdk::types::TypeTag::Struct(t)) => {
                $crate::normalize_coin_type(&format!("0x{}::{}::{}", t.address, t.module, t.name))
            }
            _ => return Err(eyre!("missing type param")),
        };

        let coin_b = match type_params.get(1) {
            Some(sui_sdk::types::TypeTag::Struct(t)) => {
                $crate::normalize_coin_type(&format!("0x{}::{}::{}", t.address, t.module, t.name))
            }
            _ => return Err(eyre!("missing type param")),
        };

        if $a2b {
            (coin_a, coin_b)
        } else {
            (coin_b, coin_a)
        }
    }};
}

// For generating pool_related_ids.txt only, using HttpClient is acceptable.
pub async fn get_children_ids(id: ObjectID) -> Result<Vec<String>> {
    let sui_client = SuiClientBuilder::default().build(SUI_RPC_NODE).await.unwrap();
    let mut next_cursor = None;
    let mut children = vec![];

    loop {
        let ret = sui_client.read_api().get_dynamic_fields(id, next_cursor, None).await?;
        next_cursor = ret.next_cursor;
        let children_ids = ret.data.iter().map(|field_info| field_info.object_id.to_string());
        children.extend(children_ids);
        if !ret.has_next_page {
            break;
        }
    }

    Ok(children)
}

#[macro_export]
macro_rules! move_field_layout {
    ($name:literal, $layout:expr) => {
        MoveFieldLayout {
            name: Identifier::new($name).unwrap(),
            layout: $layout,
        }
    };
}

#[macro_export]
macro_rules! move_type_layout_struct {
    ($struct:expr) => {
        MoveTypeLayout::Struct(Box::new($struct))
    };
}

#[macro_export]
macro_rules! move_struct_layout {
    ($type_:expr, $fields:expr) => {
        MoveStructLayout {
            type_: $type_,
            fields: Box::new($fields),
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use simulator::{DBSimulator, Simulator};
    use sui_sdk::SuiClientBuilder;
    use sui_types::base_types::SuiAddress;

    use super::*;
    use crate::tests::TEST_HTTP_URL;

    #[tokio::test]
    async fn test_get_coin_decimals() {
        let sui = SuiClientBuilder::default().build(TEST_HTTP_URL).await.unwrap();
        let decimals = get_coin_decimals(
            &sui,
            "0x19bb4ac89056993bd6f76ddfcd4b152b41c0fda25d3f01b343e98af29756b150::cally::CALLY",
        )
        .await
        .unwrap();
        assert_eq!(decimals, 6);
    }

    #[tokio::test]
    async fn test_get_pool_coins_type() {
        let sui = SuiClientBuilder::default().build(TEST_HTTP_URL).await.unwrap();
        let pool_id: ObjectID = "0x863d838561f4e82b9dbf54a4634fbd7018ac118f5c64fb34aceb1fc0b5882b0a"
            .parse()
            .unwrap();

        let (coin_a, coin_b) = get_pool_coins_type(&sui, pool_id).await.unwrap();
        assert_eq!(
            coin_a,
            "0x92baf7a2dcb487f54a3f8f0f7ffee6dd07517f1b94b05e89355995a371b7df35::xec::XEC"
        );
        assert_eq!(coin_b, "0x2::sui::SUI");
    }

    // cargo test --package dex-indexer --lib -- protocols::tests::test_debug_object_info --exact --show-output
    #[tokio::test]
    async fn test_debug_object_info() {
        let id =
            ObjectID::from_hex_literal("0x0fea99ed9c65068638963a81587c3b8cafb71dc38c545319f008f7e9feb2b5f8").unwrap();

        let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);
        let object = simulator.get_object(&id).await.unwrap();
        println!("🔥 {:?}", object);
        let layout = simulator.get_object_layout(&id).unwrap();
        println!("🧀 {:?}", layout);
    }

    #[tokio::test]
    async fn test_debug_child_objects() {
        let sui = SuiClientBuilder::default().build(TEST_HTTP_URL).await.unwrap();
        let owner = SuiAddress::from_str("0x577f358f93a323a91766d98681acf0b60fc85415189860c0832872a2d8f18d19").unwrap();
        let resp = sui.read_api().get_owned_objects(owner, None, None, None).await.unwrap();
        println!("🧀 {:?}", resp);
    }
}
