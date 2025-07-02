use std::str::FromStr;

use eyre::{eyre, Result};
use sui_sdk::{rpc_types::Coin, SuiClient, SUI_COIN_TYPE};
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    object::Object,
};

pub async fn get_gas_coin_refs(
    sui: &SuiClient,
    owner: SuiAddress,
    exclude: Option<ObjectID>,
) -> Result<Vec<ObjectRef>> {
    let coins = sui.coin_read_api().get_coins(owner, None, None, None).await?;

    let object_refs = coins
        .data
        .into_iter()
        .filter_map(|c| {
            if let Some(exclude) = exclude {
                if c.coin_object_id == exclude {
                    return None;
                }
            }
            Some(c.object_ref())
        })
        .collect();

    Ok(object_refs)
}

pub async fn get_coins(sui: &SuiClient, owner: SuiAddress, coin_type: &str, min_balance: u64) -> Result<Vec<Coin>> {
    let coins = sui
        .coin_read_api()
        .get_coins(owner, Some(coin_type.to_string()), None, None)
        .await?;

    let coins = coins
        .data
        .into_iter()
        .filter(|coin| coin.balance >= min_balance)
        .collect();

    Ok(coins)
}

pub async fn get_coin(sui: &SuiClient, owner: SuiAddress, coin_type: &str, min_balance: u64) -> Result<Coin> {
    let coins = get_coins(sui, owner, coin_type, min_balance).await?;

    coins
        .into_iter()
        .next()
        .ok_or_else(|| eyre!("No coins with balance >= {}", min_balance))
}

pub fn mocked_sui(owner: SuiAddress, amount: u64) -> Object {
    Object::with_id_owner_gas_for_testing(
        ObjectID::from_str("0x0000000000000000000000000000000000000000000000000000000000001338").unwrap(),
        owner,
        amount,
    )
}

pub fn is_native_coin(coin_type: &str) -> bool {
    coin_type == SUI_COIN_TYPE
}

pub fn format_sui_with_symbol(value: u64) -> String {
    let one_sui = 1_000_000_000.0;
    let value = value as f64 / one_sui;

    format!("{} SUI", value)
}
