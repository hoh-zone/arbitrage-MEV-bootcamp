use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use dashmap::DashMap;
use eyre::{eyre, Result};
use sui_sdk::types::event::EventID;
use tracing::{debug, error};

use crate::{
    token01_key,
    types::{PoolCache, Token01Pools, TokenPools},
    Pool, Protocol, DB,
};

#[derive(Debug, Clone)]
pub struct FileDB {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Clone)]
struct Inner {
    pools_paths: HashMap<Protocol, PathBuf>,
    cursors_path: PathBuf,
    processed_cursors: HashMap<Protocol, Option<EventID>>,
}

impl FileDB {
    pub fn new(base_path: impl Into<PathBuf>, protocols: &[Protocol]) -> Result<Self> {
        let base_path = base_path.into();
        let pools_paths = protocols
            .iter()
            .map(|protocol| {
                let path = base_path.join(format!("{}_pools.txt", protocol));
                (protocol.clone(), path)
            })
            .collect();

        let cursors_path = base_path.join("processed_cursors.json");
        let mut processed_cursors = HashMap::new();
        if cursors_path.exists() {
            let cursors_file = File::open(&cursors_path)?;
            let reader = BufReader::new(cursors_file);
            processed_cursors = serde_json::from_reader(reader)?;
        }

        Ok(Self {
            inner: Arc::new(Mutex::new(Inner {
                pools_paths,
                cursors_path,
                processed_cursors,
            })),
        })
    }
}

impl DB for FileDB {
    fn flush(&self, protocol: &Protocol, pools: &[Pool], cursor: Option<EventID>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();

        let pool_path = inner
            .pools_paths
            .get(protocol)
            .ok_or_else(|| eyre!("Protocol not supported: {:?}", protocol))?;
        let mut pool_file = OpenOptions::new().create(true).append(true).open(pool_path)?;
        for pool in pools {
            writeln!(pool_file, "{}", pool)?;
        }

        inner.processed_cursors.insert(protocol.clone(), cursor);
        let cursors_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&inner.cursors_path)?;
        serde_json::to_writer(cursors_file, &inner.processed_cursors)?;

        Ok(())
    }

    fn load_token_pools(&self, protocols: &[Protocol]) -> Result<PoolCache> {
        let inner = self.inner.lock().map_err(|_| eyre!("Mutex poisoned"))?;
        let token_pools = TokenPools::new();
        let token01_pools = Token01Pools::new();
        let pool_map = DashMap::new();

        for protocol in protocols {
            debug!(?protocol, "loading token pools");
            let pool_path = inner
                .pools_paths
                .get(protocol)
                .ok_or_else(|| eyre!("Protocol not supported: {:?}", protocol))?;
            let pool_file = match File::open(pool_path) {
                Ok(file) => file,
                Err(e) => {
                    debug!(?protocol, ?e, "loading token pools");
                    continue;
                }
            };
            let reader = BufReader::new(pool_file);

            let mut count = 0;
            for line in reader.lines() {
                count += 1;
                if line.is_err() {
                    error!("Error reading line: {:?}", line);
                    continue;
                }

                let pool = Pool::try_from(line?.as_str())?;
                // token_pools
                for token in &pool.tokens {
                    let key = token.token_type.clone();
                    token_pools.entry(key).or_default().insert(pool.clone());
                }
                // token01_pools
                for (token0_type, token1_type) in pool.token01_pairs() {
                    let key = token01_key(&token0_type, &token1_type);
                    token01_pools.entry(key).or_default().insert(pool.clone());
                }
                // pool_map
                pool_map.insert(pool.pool, pool);
            }
            debug!(?protocol, pools_count = %count, "token pools loaded");
        }

        Ok(PoolCache::new(token_pools, token01_pools, pool_map))
    }

    fn get_processed_cursors(&self) -> Result<HashMap<Protocol, Option<EventID>>> {
        let inner = self.inner.lock().map_err(|_| eyre!("Mutex poisoned"))?;
        Ok(inner.processed_cursors.clone())
    }

    fn pool_count(&self, protocol: &Protocol) -> Result<usize> {
        let inner = self.inner.lock().map_err(|_| eyre!("Mutex poisoned"))?;
        let pool_path = inner
            .pools_paths
            .get(protocol)
            .ok_or_else(|| eyre!("Protocol not supported: {:?}", protocol))?;
        let pool_file = File::open(pool_path)?;
        let reader = BufReader::new(pool_file);
        Ok(reader.lines().count())
    }

    fn get_all_pools(&self, protocol: &Protocol) -> Result<Vec<Pool>> {
        let inner = self.inner.lock().map_err(|_| eyre!("Mutex poisoned"))?;
        let pool_path = inner
            .pools_paths
            .get(protocol)
            .ok_or_else(|| eyre!("Protocol not supported: {:?}", protocol))?;
        let pool_file = File::open(pool_path)?;
        let reader = BufReader::new(pool_file);

        let mut pools = vec![];
        for line in reader.lines() {
            pools.push(Pool::try_from(line?.as_str())?);
        }

        Ok(pools)
    }
}
