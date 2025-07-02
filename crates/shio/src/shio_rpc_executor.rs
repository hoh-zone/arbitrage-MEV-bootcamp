use burberry::{async_trait, Executor};
use eyre::Result;
use fastcrypto::{encoding::Base64, hash::HashFunction};
use serde_json::{json, Value};
use shared_crypto::intent::{Intent, IntentMessage};
use sui_types::{
    crypto::{Signer, SuiKeyPair},
    digests::TransactionDigest,
    transaction::TransactionData,
};

use crate::SHIO_JSON_RPC_URL;

pub struct ShioRPCExecutor {
    keypair: SuiKeyPair,
    rpc_client: reqwest::Client,
}

impl ShioRPCExecutor {
    pub fn new(keypair: SuiKeyPair) -> Self {
        let rpc_client = reqwest::Client::new();
        Self { keypair, rpc_client }
    }

    pub async fn encode_bid(
        &self,
        tx_data: TransactionData,
        bid_amount: u64,
        opp_tx_digest: TransactionDigest,
    ) -> Result<Value> {
        let tx_bytes = bcs::to_bytes(&tx_data)?;
        let tx_b64 = Base64::from_bytes(&tx_bytes).encoded();

        let sig = {
            let intent_msg = IntentMessage::new(Intent::sui_transaction(), tx_data);
            let raw_tx = bcs::to_bytes(&intent_msg)?;
            let digest = {
                let mut hasher = sui_types::crypto::DefaultHash::default();
                hasher.update(raw_tx.clone());
                hasher.finalize().digest
            };

            self.keypair.sign(&digest)
        };

        Ok(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "shio_submitBid",
            "params": [
                opp_tx_digest.base58_encode(),
                bid_amount,
                tx_b64,
                sig,
            ]
        }))
    }
}

#[async_trait]
impl Executor<(TransactionData, u64, TransactionDigest)> for ShioRPCExecutor {
    fn name(&self) -> &str {
        "ShioRPCExecutor"
    }

    async fn execute(
        &self,
        (tx_data, bid_amount, opp_tx_digest): (TransactionData, u64, TransactionDigest),
    ) -> Result<()> {
        let bid = self.encode_bid(tx_data, bid_amount, opp_tx_digest).await?;
        tracing::warn!("🧀>> {}", bid);
        let resp = self.rpc_client.post(SHIO_JSON_RPC_URL).json(&bid).send().await?;
        let status = resp.status();
        let response = resp.text().await?;
        tracing::warn!("🧀<< {:?} {:?}", status, response);

        Ok(())
    }
}
