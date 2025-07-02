//! Dex indexer
//! Usage: see unit tests `test_get_pools`.

mod blockberry;
mod collector;
mod file_db;
mod protocols;
mod strategy;
pub mod types;

use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    sync::Arc,
    time::Instant,
};

use burberry::Engine;
use collector::QueryEventCollector;
use eyre::Result;
use strategy::PoolCreatedStrategy;
use sui_sdk::{
    types::{base_types::ObjectID, event::EventID},
    SuiClientBuilder, SUI_COIN_TYPE,
};
use tokio::task::JoinSet;
use tracing::info;
use types::{DummyExecutor, Event, NoAction, Pool, PoolCache, Protocol};

pub const FILE_DB_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/data");

pub fn supported_protocols() -> Vec<Protocol> {
    vec![
        Protocol::Cetus,
        Protocol::Turbos,
        Protocol::Aftermath,
        Protocol::KriyaAmm,
        Protocol::KriyaClmm,
        Protocol::FlowxClmm,
        Protocol::DeepbookV2,
        Protocol::BlueMove,
    ]
}

#[derive(Clone)]
pub struct DexIndexer {
    pool_cache: PoolCache,

    db: Arc<dyn DB>,
    _live_indexer_tasks: Arc<JoinSet<()>>,
}

impl DexIndexer {
    pub async fn new(http_url: &str) -> Result<Self> {
        let sui = SuiClientBuilder::default().build(http_url).await?;
        let db = Arc::new(file_db::FileDB::new(FILE_DB_DIR, &supported_protocols())?);

        let timer = Instant::now();
        info!("loading token pools...");
        let pool_cache = db.load_token_pools(&supported_protocols())?;
        info!(elapsed = ?timer.elapsed(), token_pools_count = %pool_cache.token_pools.len(), token01_pools_count = %pool_cache.token01_pools.len(), "token pools loaded");

        let strategy = PoolCreatedStrategy::new(db.clone(), sui.clone(), pool_cache.clone())?;
        strategy.backfill_pools().await?;

        // Build the bubbery engine
        let mut engine = Engine::<Event, NoAction>::new();
        let collector = QueryEventCollector::new();
        engine.add_collector(Box::new(collector));
        engine.add_strategy(Box::new(strategy));
        engine.add_executor(Box::new(DummyExecutor));

        let join_set = engine.run().await.expect("Burberry engine run failed");

        Ok(Self {
            pool_cache,
            db,
            _live_indexer_tasks: Arc::new(join_set),
        })
    }

    /// Get the pools by the given token type.
    pub fn get_pools_by_token(&self, token_type: &str) -> Option<HashSet<Pool>> {
        self.pool_cache.token_pools.get(token_type).map(|p| p.clone())
    }

    /// Get the pools by the given token01 types.
    pub fn get_pools_by_token01(&self, token0_type: &str, token1_type: &str) -> Option<HashSet<Pool>> {
        let key = token01_key(token0_type, token1_type);
        self.pool_cache.token01_pools.get(&key).map(|p| p.clone())
    }

    /// Get the pool by the given pool id.
    pub fn get_pool_by_id(&self, pool_id: &ObjectID) -> Option<Pool> {
        self.pool_cache.pool_map.get(pool_id).map(|p| p.clone())
    }

    /// Get the pools count by the given protocol.
    pub fn pool_count(&self, protocol: &Protocol) -> usize {
        self.db.pool_count(protocol).unwrap_or_default()
    }

    /// Get all pools by the given protocol.
    pub fn get_all_pools(&self, protocol: &Protocol) -> Result<Vec<Pool>> {
        self.db.get_all_pools(protocol)
    }
}

#[inline]
pub fn token01_key(token0_type: &str, token1_type: &str) -> (String, String) {
    if token0_type < token1_type {
        (token0_type.to_string(), token1_type.to_string())
    } else {
        (token1_type.to_string(), token0_type.to_string())
    }
}

#[inline]
pub fn normalize_coin_type(coin_type: &str) -> String {
    if coin_type == "0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI" {
        SUI_COIN_TYPE.to_string()
    } else {
        coin_type.to_string()
    }
}

pub trait DB: Debug + Send + Sync {
    fn flush(&self, protocol: &Protocol, pools: &[Pool], cursor: Option<EventID>) -> Result<()>;
    fn load_token_pools(&self, protocols: &[Protocol]) -> Result<PoolCache>;
    fn get_processed_cursors(&self) -> Result<HashMap<Protocol, Option<EventID>>>;
    fn pool_count(&self, protocol: &Protocol) -> Result<usize>;
    fn get_all_pools(&self, protocol: &Protocol) -> Result<Vec<Pool>>;
}

#[cfg(test)]
mod tests {

    use super::*;

    pub const TEST_HTTP_URL: &str = "";
    const TOKEN0_TYPE: &str = "";
    const TOKEN1_TYPE: &str = "";

    #[tokio::test]
    async fn test_get_pools() {
        // `DexIndexer::new` will backfill pools first.
        let indexer = DexIndexer::new(TEST_HTTP_URL).await.unwrap();

        // get pools by token
        let pools = indexer.get_pools_by_token(TOKEN0_TYPE).unwrap();
        println!("pools_len: {}", pools.len());
        println!("first pool: {:?}", pools.iter().next());

        // get pools by token01
        let pools = indexer.get_pools_by_token01(TOKEN0_TYPE, TOKEN1_TYPE).unwrap();
        println!("pools_len: {}", pools.len());
        println!("first pool: {:?}", pools.iter().next());
    }

    #[test]
    fn test_normalize_token_type() {
        assert_eq!(
            normalize_coin_type("0x0000000000000000000000000000000000000000000000000000000000000002::sui::SUI"),
            TOKEN0_TYPE.to_string()
        );

        assert_eq!(normalize_coin_type(TOKEN1_TYPE), TOKEN1_TYPE.to_string());
    }

    #[tokio::test]
    async fn test_pools_count() {
        let indexer = DexIndexer::new(TEST_HTTP_URL).await.unwrap();

        for protocol in supported_protocols() {
            let count = indexer.pool_count(&protocol);
            println!("{}: {}", protocol, count);
        }
    }
}
