use std::sync::Arc;

use burberry::{async_trait, ActionSubmitter, Strategy};
use eyre::Result;
use sui_sdk::{types::event::EventID, SuiClient};
use tokio::task::JoinSet;
use tracing::{debug, error, info};

use crate::{
    supported_protocols, token01_key,
    types::{Event, NoAction, PoolCache, Protocol},
    DB,
};

#[derive(Clone)]
pub struct PoolCreatedStrategy {
    pool_cache: PoolCache,

    db: Arc<dyn DB>,
    sui: SuiClient,
}

impl PoolCreatedStrategy {
    pub fn new(db: Arc<dyn DB>, sui: SuiClient, pool_cache: PoolCache) -> Result<Self> {
        Ok(Self { pool_cache, db, sui })
    }

    pub async fn backfill_pools(&self) -> Result<()> {
        let mut joinset = JoinSet::new();
        let cursors = self.db.get_processed_cursors()?;
        for protocol in supported_protocols() {
            let (sui, db) = (self.sui.clone(), self.db.clone());
            let pool_cache = self.pool_cache.clone();
            let cursor = cursors.get(&protocol).cloned().flatten();

            joinset.spawn(async move { backfill_pools_for_protocol(sui, db, protocol, cursor, pool_cache).await });
        }

        while let Some(res) = joinset.join_next().await {
            if let Err(e) = res {
                error!("backfill_pools error: {:?}", e);
            }
        }

        info!("backfill_pools done");
        Ok(())
    }
}

#[async_trait]
impl Strategy<Event, NoAction> for PoolCreatedStrategy {
    fn name(&self) -> &str {
        "PoolCreatedStrategy"
    }

    async fn sync_state(&mut self, _submitter: Arc<dyn ActionSubmitter<NoAction>>) -> Result<()> {
        self.backfill_pools().await
    }

    async fn process_event(&mut self, _event: Event, _: Arc<dyn ActionSubmitter<NoAction>>) {
        if let Err(error) = self.backfill_pools().await {
            error!("backfill_pools error: {:?}", error);
        }
    }
}

async fn backfill_pools_for_protocol(
    sui: SuiClient,
    db: Arc<dyn DB>,
    protocol: Protocol,
    cursor: Option<EventID>,
    pool_cache: PoolCache,
) -> Result<()> {
    let filter = protocol.event_filter();
    let mut cursor = cursor;

    debug!(%protocol, ?filter, ?cursor, "querying events");
    let mut page = sui
        .event_api()
        .query_events(filter.clone(), cursor, None, false)
        .await?;
    debug!(%protocol, ?page, "events queried");

    let PoolCache {
        token_pools,
        token01_pools,
        pool_map,
    } = pool_cache;

    while !page.data.is_empty() {
        let mut pools = vec![];
        for event in &page.data {
            match protocol.sui_event_to_pool(event, &sui).await {
                Ok(pool) => {
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
                    pool_map.insert(pool.pool, pool.clone());

                    pools.push(pool)
                }
                Err(e) => {
                    error!("invalid {:?}: {:?}", event, e);
                }
            }
        }
        debug!("{}: {} pools found at cursor {:?}", protocol, pools.len(), cursor);
        cursor = if page.has_next_page {
            page.next_cursor
        } else {
            page.data.last().map(|e| e.id)
        };
        db.flush(&protocol, &pools, cursor)?;

        // thread::sleep(Duration::from_secs(1));
        page = sui
            .event_api()
            .query_events(filter.clone(), cursor, None, false)
            .await?;
    }

    info!(
        "{}: backfill complete, pool_count = {}",
        protocol,
        db.pool_count(&protocol)?
    );

    Ok(())
}
