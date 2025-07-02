use async_trait::async_trait;
use fastcrypto::encoding::Base64;
use futures::SinkExt;
use futures_util::stream::StreamExt;
use serde::Serialize;
use sui_network::api::{Validator, ValidatorServer};
use sui_types::{
    crypto::ToFromBytes,
    messages_checkpoint::{CheckpointRequest, CheckpointRequestV2, CheckpointResponse, CheckpointResponseV2},
    messages_grpc::{
        HandleCertificateRequestV3, HandleCertificateResponseV2, HandleCertificateResponseV3,
        HandleSoftBundleCertificatesRequestV3, HandleSoftBundleCertificatesResponseV3, HandleTransactionRequestV2,
        HandleTransactionResponse, HandleTransactionResponseV2, ObjectInfoRequest, ObjectInfoResponse,
        SubmitCertificateResponse, SystemStateRequest, TransactionInfoRequest, TransactionInfoResponse,
    },
    sui_system_state::SuiSystemState,
    transaction::{CertifiedTransaction, Transaction},
};
use tokio::{net::TcpListener, sync::watch};
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message};
use tracing::{debug, error, info};

const RELAY_SERVER_URL: &str = "/ip4/0.0.0.0/tcp/9000/http";
const WS_SERVER_URL: &str = "0.0.0.0:9001";

#[derive(Debug, Clone, Serialize, Default)]
pub struct TxMessage {
    tx_bytes: String,
    signatures: Vec<String>,
}

pub struct Relay {
    tx_sender: watch::Sender<TxMessage>,
}

impl Relay {
    pub fn new(tx_sender: watch::Sender<TxMessage>) -> Self {
        Relay { tx_sender }
    }

    async fn start_websocket_server(tx_sender: watch::Sender<TxMessage>) {
        info!("WebSocket Server running on {}", WS_SERVER_URL);
        let listener = TcpListener::bind(WS_SERVER_URL).await.unwrap();
        while let Ok((stream, _)) = listener.accept().await {
            let tx_sender = tx_sender.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                let (mut write, _) = ws_stream.split();
                let mut tx_receiver = tx_sender.subscribe();
                while tx_receiver.changed().await.is_ok() {
                    let tx_message = tx_receiver.borrow().clone();
                    let msg = Message::Text(serde_json::to_string(&tx_message).unwrap());
                    info!("🔥 Relay send {:?}", msg);
                    write.send(msg).await.unwrap();
                }
            });
        }
    }
}

#[async_trait]
impl Validator for Relay {
    async fn transaction(
        &self,
        request: tonic::Request<Transaction>,
    ) -> Result<tonic::Response<HandleTransactionResponse>, tonic::Status> {
        info!("🧀 Relay receive {:?}", request);

        let tx = request.into_inner();
        let tx_bytes = Base64::from_bytes(&bcs::to_bytes(tx.data().transaction_data()).unwrap()).encoded();
        let signatures: Vec<String> = tx
            .data()
            .tx_signatures()
            .iter()
            .map(|s| Base64::from_bytes(s.as_bytes()).encoded())
            .collect();

        let tx_message = TxMessage { tx_bytes, signatures };

        if self.tx_sender.send(tx_message).is_err() {
            debug!("💤 No subscriber");
        }

        Err(tonic::Status::internal("Not implemented"))
    }

    async fn transaction_v2(
        &self,
        _request: tonic::Request<HandleTransactionRequestV2>,
    ) -> Result<tonic::Response<HandleTransactionResponseV2>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn submit_certificate(
        &self,
        _request: tonic::Request<CertifiedTransaction>,
    ) -> Result<tonic::Response<SubmitCertificateResponse>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn handle_certificate_v2(
        &self,
        _request: tonic::Request<CertifiedTransaction>,
    ) -> Result<tonic::Response<HandleCertificateResponseV2>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn handle_certificate_v3(
        &self,
        _request: tonic::Request<HandleCertificateRequestV3>,
    ) -> Result<tonic::Response<HandleCertificateResponseV3>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn handle_soft_bundle_certificates_v3(
        &self,
        _request: tonic::Request<HandleSoftBundleCertificatesRequestV3>,
    ) -> Result<tonic::Response<HandleSoftBundleCertificatesResponseV3>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn object_info(
        &self,
        _request: tonic::Request<ObjectInfoRequest>,
    ) -> Result<tonic::Response<ObjectInfoResponse>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn transaction_info(
        &self,
        _request: tonic::Request<TransactionInfoRequest>,
    ) -> Result<tonic::Response<TransactionInfoResponse>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn checkpoint(
        &self,
        _request: tonic::Request<CheckpointRequest>,
    ) -> Result<tonic::Response<CheckpointResponse>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn checkpoint_v2(
        &self,
        _request: tonic::Request<CheckpointRequestV2>,
    ) -> Result<tonic::Response<CheckpointResponseV2>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }

    async fn get_system_state_object(
        &self,
        _request: tonic::Request<SystemStateRequest>,
    ) -> Result<tonic::Response<SuiSystemState>, tonic::Status> {
        Err(tonic::Status::internal("Not implemented"))
    }
}

#[tokio::main]
async fn main() {
    mev_logger::init_console_logger_with_directives(None, &["relay=debug"]);

    let (sender, _) = watch::channel(TxMessage::default());
    let relay = Relay::new(sender.clone());

    tokio::spawn(async move {
        Relay::start_websocket_server(sender).await;
    });

    // test code
    // tokio::spawn(async move {
    //     subscribe_websocket_messages().await;
    // });

    let server = mysten_network::config::Config::new()
        .server_builder()
        .add_service(ValidatorServer::new(relay))
        .bind(&RELAY_SERVER_URL.parse().unwrap(), None)
        .await
        .unwrap();

    info!("Server running on {}", server.local_addr());
    server.serve().await.unwrap();
}

#[allow(dead_code)]
async fn subscribe_websocket_messages() {
    let ws_addr = "ws://localhost:9001";
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_addr)
        .await
        .expect("Failed to connect");
    let (_, mut read) = ws_stream.split();
    while let Some(message) = read.next().await {
        match message {
            Ok(msg) => info!("✅ Subscriber receive {:?}", msg),
            Err(e) => error!("WebSocket error: {:?}", e),
        }
    }
}
