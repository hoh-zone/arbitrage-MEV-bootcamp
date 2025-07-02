use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ::utils::heartbeat;
use burberry::{executor::telegram_message::TelegramMessageDispatcher, map_collector, map_executor, Engine};
use clap::Parser;
use eyre::Result;
use object_pool::ObjectPool;
use shio::{new_shio_collector_and_executor, ShioRPCExecutor};
use simulator::{DBSimulator, HttpSimulator, ReplaySimulator, Simulator};
use sui_types::{base_types::SuiAddress, crypto::SuiKeyPair};
use tracing::{info, warn};

use crate::{
    collector::{PrivateTxCollector, PublicTxCollector},
    executor::PublicTxExecutor,
    strategy::ArbStrategy,
    types::{Action, Event},
    HttpConfig,
};

#[derive(Clone, Debug, Parser)]
pub struct Args {
    #[arg(long, env = "SUI_PRIVATE_KEY")]
    pub private_key: String,

    #[arg(long, help = "shio executor uses RPC to submit bid")]
    pub shio_use_rpc: bool,

    #[command(flatten)]
    pub http_config: HttpConfig,

    #[command(flatten)]
    collector_config: CollectorConfig,

    #[command(flatten)]
    db_sim_config: DbSimConfig,

    #[command(flatten)]
    worker_config: WorkerConfig,
}

#[derive(Clone, Debug, Parser)]
struct CollectorConfig {
    /// relay tx collector (should be mutually exclusive with public tx collector)
    #[arg(long)]
    pub relay_ws_url: Option<String>,

    /// shio collector
    #[arg(long)]
    pub shio_ws_url: Option<String>,

    /// public tx collector
    #[arg(long, env = "SUI_TX_SOCKET_PATH", default_value = "/tmp/sui_tx.sock")]
    pub tx_socket_path: String,
}

#[derive(Clone, Debug, Parser)]
struct DbSimConfig {
    /// needed for db simulator
    #[arg(long, env = "SUI_DB_PATH", default_value = "/home/ubuntu/sui/db/live/store")]
    pub db_path: String,

    /// needed for db simulator
    #[arg(long, env = "SUI_CONFIG_PATH", default_value = "/home/ubuntu/sui/fullnode.yaml")]
    pub config_path: String,

    /// db simulator listens to this socket
    /// sui node will update object changes
    #[arg(long, env = "SUI_UPDATE_CACHE_SOCKET", default_value = "/tmp/sui_cache_updates.sock")]
    pub update_cache_socket: String,

    /// pool related objects path
    #[arg(
        long,
        env = "SUI_PRELOAD_PATH",
        default_value = "/home/ubuntu/suiflow-relay/pool_related_ids.txt"
    )]
    pub preload_path: String,

    /// use db simulator or not
    #[arg(long, default_value_t = false)]
    pub use_db_simulator: bool,

    /// catchup interval in seconds
    #[arg(long, default_value_t = 60)]
    pub catchup_interval: u64,
}

#[derive(Clone, Debug, Parser)]
struct WorkerConfig {
    /// Number of workers to process events (public tx, private tx, shio)
    /// 8 is usually enough
    #[arg(long, default_value_t = 8)]
    pub workers: usize,

    /// Number of simulator in simulator pool.
    #[arg(long, default_value_t = 32)]
    pub num_simulators: usize,

    /// If a new coin comes in and it has been processed within the last `max_recent_arbs` times,
    /// it will be ignored.
    #[arg(long, default_value_t = 20)]
    pub max_recent_arbs: usize,

    /// short and long interval for dedicated simulator (in milliseconds)
    /// short: 50ms
    #[arg(long, default_value_t = 50)]
    pub dedicated_short_interval: u64,

    /// long: 200ms
    #[arg(long, default_value_t = 200)]
    pub dedicated_long_interval: u64,
}

pub async fn run(args: Args) -> Result<()> {
    utils::set_panic_hook();
    mev_logger::init_with_whitelisted_modules(
        "mainnet",
        "sui-arb".to_string(),
        &["arb", "utils", "shio", "cache_metrics=debug"],
    );

    let keypair = SuiKeyPair::decode(&args.private_key)?;
    let pubkey = keypair.public();
    let attacker = SuiAddress::from(&pubkey);

    info!(
        "start_bot with attacker: {}, http_config: {:#?}, collector_config: {:#?}, db_sim_config: {:#?}, worker_config: {:#?}",
        attacker, args.http_config, args.collector_config, args.db_sim_config, args.worker_config
    );

    let rpc_url = args.http_config.rpc_url;
    let db_path = args.db_sim_config.db_path;
    let tx_socket_path = args.collector_config.tx_socket_path;
    let config_path = args.db_sim_config.config_path;
    let update_cache_socket = args.db_sim_config.update_cache_socket;
    let preload_path = args.db_sim_config.preload_path;
    let mut engine = Engine::default();

    if let Some(ref ws_url) = args.collector_config.shio_ws_url {
        let (shio_collector, shio_executor) =
            new_shio_collector_and_executor(keypair, Some(ws_url.clone()), None).await;
        engine.add_collector(map_collector!(shio_collector, Event::Shio));

        if args.shio_use_rpc {
            let shio_rpc_executor = ShioRPCExecutor::new(SuiKeyPair::decode(&args.private_key)?);
            engine.add_executor(map_executor!(shio_rpc_executor, Action::ShioSubmitBid));
        } else {
            engine.add_executor(map_executor!(shio_executor, Action::ShioSubmitBid));
        }
    } else {
        let public_tx_collector = PublicTxCollector::new(&tx_socket_path);
        engine.add_collector(Box::new(public_tx_collector));
    }

    engine.add_executor(map_executor!(
        PublicTxExecutor::new(&rpc_url, SuiKeyPair::decode(&args.private_key)?).await?,
        Action::ExecutePublicTx
    ));

    if let Some(ref relay_ws_url) = args.collector_config.relay_ws_url {
        let private_tx_collector = PrivateTxCollector::new(relay_ws_url);
        engine.add_collector(Box::new(private_tx_collector));
    }

    let simulator_pool: ObjectPool<Box<dyn Simulator>> = match args.db_sim_config.use_db_simulator {
        true => {
            let db_path = db_path.to_string();
            let config_path = config_path.to_string();
            let update_cache_socket = update_cache_socket.to_string();
            let preload_path = preload_path.to_string();
            ObjectPool::new(args.worker_config.num_simulators, move || {
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    let start = Instant::now();
                    let simulator = Box::new(
                        DBSimulator::new_slow(&db_path, &config_path, Some(&update_cache_socket), Some(&preload_path))
                            .await,
                    ) as Box<dyn Simulator>;
                    info!(elapsed = ?start.elapsed(), "DBSimulator initialized");
                    simulator
                })
            })
        }
        false => {
            warn!("http simulator is deprecated. use only for testing");

            let rpc_url = rpc_url.to_string();
            let ipc_path = args.http_config.ipc_path.clone();

            ObjectPool::new(args.worker_config.num_simulators, move || {
                let rpc_url = rpc_url.clone();
                let ipc_path = ipc_path.clone();

                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async { Box::new(HttpSimulator::new(&rpc_url, &ipc_path).await) as Box<dyn Simulator> })
            })
        }
    };

    // TODO: when we have relay (tons of un-executed txs), maybe we should use a simulator pool
    let own_simulator = if args.db_sim_config.use_db_simulator {
        Arc::new(DBSimulator::new_slow(&db_path, &config_path, Some(&update_cache_socket), Some(&preload_path)).await)
            as Arc<dyn Simulator>
    } else {
        warn!("http simulator is deprecated. use only for testing");
        let ipc_path = args.http_config.ipc_path;
        Arc::new(HttpSimulator::new(&rpc_url, &ipc_path).await) as Arc<dyn Simulator>
    };

    let dedicated_simulator = if args.db_sim_config.use_db_simulator {
        Some(Arc::new(
            ReplaySimulator::new_slow(
                &db_path,
                &config_path,
                Duration::from_millis(args.worker_config.dedicated_long_interval),
                Duration::from_millis(args.worker_config.dedicated_short_interval),
            )
            .await,
        ))
    } else {
        None
    };

    info!("simulator_pool initialized: {:?}", simulator_pool);

    let arb_strategy = ArbStrategy::new(
        attacker,
        Arc::new(simulator_pool),
        own_simulator,
        args.worker_config.max_recent_arbs,
        &rpc_url,
        args.worker_config.workers,
        dedicated_simulator,
    )
    .await;
    engine.add_strategy(Box::new(arb_strategy));

    engine.add_executor(map_executor!(
        TelegramMessageDispatcher::new_without_error_report(),
        Action::NotifyViaTelegram
    ));

    heartbeat::start("sui-arb", Duration::from_secs(30));

    engine.run_and_join().await.unwrap();

    Ok(())
}
