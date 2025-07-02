use async_channel::Sender;
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

pub struct ShioExecutor {
    keypair: SuiKeyPair,
    bid_sender: Sender<Value>,
}

impl ShioExecutor {
    pub async fn new(keypair: SuiKeyPair, bid_sender: Sender<Value>) -> Self {
        Self { keypair, bid_sender }
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
            "oppTxDigest": opp_tx_digest.base58_encode(),
            "bidAmount": bid_amount,
            "txData": tx_b64,
            "sig": sig,
        }))
    }
}

#[async_trait]
impl Executor<(TransactionData, u64, TransactionDigest)> for ShioExecutor {
    fn name(&self) -> &str {
        "ShioExecutor"
    }

    async fn execute(
        &self,
        (tx_data, bid_amount, opp_tx_digest): (TransactionData, u64, TransactionDigest),
    ) -> Result<()> {
        let bid = self.encode_bid(tx_data, bid_amount, opp_tx_digest).await?;
        self.bid_sender.send(bid).await?;
        Ok(())
    }
}
