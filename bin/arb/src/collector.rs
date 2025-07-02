use burberry::{async_trait, Collector, CollectorStream};
use eyre::Result;
use fastcrypto::encoding::{Base64, Encoding};
use futures::stream::StreamExt;
use interprocess::local_socket::{
    tokio::{prelude::*, Stream},
    GenericNamespaced,
};
use serde::Deserialize;
use sui_json_rpc_types::{SuiEvent, SuiTransactionBlockEffects};
use sui_types::{effects::TransactionEffects, transaction::TransactionData};
use tokio::{io::AsyncReadExt, pin, time};
use tracing::{debug, error};

use crate::types::Event;

pub struct PublicTxCollector {
    path: String,
}

impl PublicTxCollector {
    pub fn new(path: &str) -> Self {
        Self { path: path.to_string() }
    }

    async fn connect(&self) -> Result<Stream> {
        let name = self.path.as_str().to_ns_name::<GenericNamespaced>()?;
        let conn = Stream::connect(name).await?;
        Ok(conn)
    }
}

#[async_trait]
impl Collector<Event> for PublicTxCollector {
    fn name(&self) -> &str {
        "PublicTxCollector"
    }

    async fn get_event_stream(&self) -> Result<CollectorStream<'_, Event>> {
        let mut conn = self.connect().await?;
        let mut effects_len_buf = [0u8; 4];
        let mut events_len_buf = [0u8; 4];

        let stream = async_stream::stream! {
            loop {
                tokio::select! {
                    result = conn.read_exact(&mut effects_len_buf) => {
                        if result.is_err() {
                            debug!("Failed to read effects length");
                            conn = self.connect().await.expect("Failed to reconnect to tx socket");
                            continue;
                        }

                        let effects_len = u32::from_be_bytes(effects_len_buf);
                        let mut effects_buf = vec![0u8; effects_len as usize];
                        if conn.read_exact(&mut effects_buf).await.is_err() {
                            debug!("Failed to read effects");
                            conn = self.connect().await.expect("Failed to reconnect to tx socket");
                            continue;
                        }

                        if conn.read_exact(&mut events_len_buf).await.is_err() {
                            debug!("Failed to read events length");
                            conn = self.connect().await.expect("Failed to reconnect to tx socket");
                            continue;
                        }

                        let events_len = u32::from_be_bytes(events_len_buf);
                        let mut events_buf = vec![0u8; events_len as usize];
                        if conn.read_exact(&mut events_buf).await.is_err() {
                            debug!("Failed to read events");
                            conn = self.connect().await.expect("Failed to reconnect to tx socket");
                            continue;
                        }

                        let tx_effects: TransactionEffects = match bincode::deserialize(&effects_buf) {
                            Ok(tx_effects) => tx_effects,
                            Err(e) => {
                                error!("Invalid tx_effects: {:?}", e);
                                continue;
                            }
                        };

                        let events: Vec<SuiEvent> = if events_len == 0 {
                            vec![]
                        } else {
                            match serde_json::from_slice(&events_buf) {
                                Ok(events) => events,
                                Err(e) => {
                                    error!("Invalid events: {:?}", e);
                                    continue;
                                }
                            }
                        };

                        if let Ok(tx_effects) = SuiTransactionBlockEffects::try_from(tx_effects) {
                            yield Event::PublicTx(tx_effects, events);
                        }

                    }
                    else => {
                        time::sleep(time::Duration::from_millis(10)).await;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TxMessage {
    tx_bytes: String,
}

impl TryFrom<TxMessage> for TransactionData {
    type Error = eyre::Error;

    fn try_from(tx_message: TxMessage) -> Result<Self> {
        let tx_bytes = Base64::decode(&tx_message.tx_bytes)?;
        let tx_data: TransactionData = bcs::from_bytes(&tx_bytes)?;
        Ok(tx_data)
    }
}

pub struct PrivateTxCollector {
    ws_url: String,
}

impl PrivateTxCollector {
    pub fn new(ws_url: &str) -> Self {
        Self {
            ws_url: ws_url.to_string(),
        }
    }
}

#[async_trait]
impl Collector<Event> for PrivateTxCollector {
    fn name(&self) -> &str {
        "PrivateTxCollector"
    }

    async fn get_event_stream(&self) -> Result<CollectorStream<'_, Event>> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(&self.ws_url)
            .await
            .expect("Failed to connect to relay server");

        let (_, read) = ws_stream.split();

        let stream = async_stream::stream! {
            pin!(read);
            while let Some(message) = read.next().await {
                let message = match message {
                    Ok(msg) => msg,
                    Err(e) => {
                        error!("Relay websocket error: {:?}", e);
                        continue;
                    }
                };

                let tx_message: TxMessage = serde_json::from_str(message.to_text().unwrap()).unwrap();
                let tx_data = match TransactionData::try_from(tx_message) {
                    Ok(tx_data) => tx_data,
                    Err(e) => {
                        error!("Invalid tx_message: {:?}", e);
                        continue;
                    }
                };

                yield Event::PrivateTx(tx_data);
            }
        };

        Ok(Box::pin(stream))
    }
}
