use cached::proc_macro::cached;
use eyre::Result;
use sui_sdk::{
    rpc_types::{SuiObjectData, SuiObjectDataOptions},
    SuiClient,
};
use sui_types::base_types::ObjectID;

#[cached(key = "String", convert = r##"{ obj_id.to_string() }"##, result = true)]
pub async fn get_object_cache(sui: &SuiClient, obj_id: &str) -> Result<SuiObjectData> {
    get_object(sui, obj_id).await
}

pub async fn get_object(sui: &SuiClient, obj_id: &str) -> Result<SuiObjectData> {
    let obj_id = ObjectID::from_hex_literal(obj_id)?;
    let opts = SuiObjectDataOptions::full_content();
    let obj = sui
        .read_api()
        .get_object_with_options(obj_id, opts)
        .await?
        .into_object()?;

    Ok(obj)
}
