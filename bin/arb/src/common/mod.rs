pub mod notification;
pub mod search;

use eyre::Result;
use simulator::SimEpoch;
use sui_sdk::SuiClient;

pub async fn get_latest_epoch(sui: &SuiClient) -> Result<SimEpoch> {
    let sys_state = sui.governance_api().get_latest_sui_system_state().await?;
    Ok(SimEpoch::from(sys_state))
}
