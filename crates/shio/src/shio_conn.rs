use async_channel::{Receiver, Sender};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use tracing::error;

use crate::ShioItem;

pub async fn new_shio_conn(wss_url: String, num_retries: u32) -> (Sender<Value>, Receiver<ShioItem>) {
    let (bid_sender, bid_receiver) = async_channel::unbounded();
    let (shio_item_sender, shio_item_receiver) = async_channel::unbounded();

    tokio::spawn(async move {
        let bid_receiver: Receiver<Value> = bid_receiver;
        let shio_item_sender: Sender<ShioItem> = shio_item_sender;
        let wss_url = wss_url;

        let mut retry_count = 0;

        loop {
            let (mut wss_stream, _) = match tokio_tungstenite::connect_async(&wss_url).await {
                Ok(r) => {
                    retry_count = 0;
                    r
                }
                Err(e) => {
                    error!("fail to connect to ws server: {e:#}");
                    if retry_count == num_retries {
                        panic!("fail to connect to ws server after {} retries", num_retries);
                    }

                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    retry_count += 1;
                    continue;
                }
            };

            'connected: loop {
                // either receive from bid_receiver or wss_stream
                tokio::select! {
                    Ok(bid) = bid_receiver.recv() => {
                        let msg = Message::Text(bid.to_string());
                        if let Err(e) = wss_stream.send(msg).await {
                            error!("fail to send message to ws server: {e:#}");
                            break 'connected;
                        }
                    }
                    Some(msg) = wss_stream.next() => {
                        match msg {
                            Ok(Message::Text(text)) => {
                                let value = match serde_json::from_str::<Value>(&text) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        error!("error parsing json: {}", e);
                                        continue;
                                    }
                                };
                                shio_item_sender.send(ShioItem::from(value)).await.unwrap();
                            }
                            Ok(Message::Ping(val)) => {
                                if let Err(e) = wss_stream.send(Message::Pong(val)).await {
                                    error!("Failed to send pong: {}", e);
                                    break 'connected;
                                }
                            }
                            Ok(Message::Close(_)) | Ok(Message::Frame(_)) | Ok(Message::Pong(_)) | Ok(Message::Binary(_)) => {
                                panic!("unexpected websocket message: {:?}", msg);
                            }
                            Err(e) => {
                                error!("error receiving websocket message: {:?}", e);
                                break 'connected;
                            }
                        }
                    }
                }
            }
        }
    });

    (bid_sender, shio_item_receiver)
}
