use dex_indexer::{
    types::{Pool, Protocol},
    DexIndexer,
};
use eyre::{bail, ensure, OptionExt, Result};
use object_pool::ObjectPool;
use simulator::Simulator;
use std::sync::Arc;
use sui_sdk::SUI_COIN_TYPE;
use sui_types::base_types::ObjectID;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;

use super::{
    aftermath::Aftermath, cetus::Cetus, deepbook_v2::DeepbookV2, flowx_clmm::FlowxClmm, turbos::Turbos, Dex,
    DexSearcher, Path,
};
use crate::defi::{blue_move::BlueMove, kriya_amm::KriyaAmm, kriya_clmm::KriyaClmm};

static INDEXER: OnceCell<Arc<DexIndexer>> = OnceCell::const_new();

#[derive(Clone)]
pub struct IndexerDexSearcher {
    simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>,
    indexer: Arc<DexIndexer>,
}

impl IndexerDexSearcher {
    pub async fn new(http_url: &str, simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>) -> Result<Self> {
        let indexer = INDEXER
            .get_or_init(|| async {
                let indexer = DexIndexer::new(http_url).await.unwrap();
                Arc::new(indexer)
            })
            .await
            .clone();

        Ok(Self {
            simulator_pool,
            indexer,
        })
    }
}

async fn new_dexes(
    simulator: Arc<Box<dyn Simulator>>,
    pool: &Pool,
    token_in_type: &str,
    token_out_type: Option<String>,
) -> Result<Vec<Box<dyn Dex>>> {
    let dexes = match pool.protocol {
        Protocol::Turbos => {
            let dex = Turbos::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::Cetus => {
            let dex = Cetus::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::Aftermath => Aftermath::new(simulator, pool, token_in_type, token_out_type)
            .await?
            .into_iter()
            .map(|dex| Box::new(dex) as Box<dyn Dex>)
            .collect(),
        Protocol::FlowxClmm => {
            let dex = FlowxClmm::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::KriyaAmm => {
            let dex = KriyaAmm::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::KriyaClmm => {
            let dex = KriyaClmm::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::DeepbookV2 => {
            let dex = DeepbookV2::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        Protocol::BlueMove => {
            let dex = BlueMove::new(simulator, pool, token_in_type).await?;
            vec![Box::new(dex) as Box<dyn Dex>]
        }

        _ => bail!("unsupported protocol: {:?}", pool.protocol),
    };

    Ok(dexes)
}

#[async_trait::async_trait]
impl DexSearcher for IndexerDexSearcher {
    async fn find_dexes(&self, token_in_type: &str, token_out_type: Option<String>) -> Result<Vec<Box<dyn Dex>>> {
        let pools = if let Some(token_out_type) = token_out_type.as_ref() {
            self.indexer.get_pools_by_token01(token_in_type, token_out_type)
        } else {
            self.indexer.get_pools_by_token(token_in_type)
        };
        ensure!(
            pools.is_some(),
            "pools not found, coin_in: {}, coin_out: {:?}",
            token_in_type,
            token_out_type
        );

        let mut join_set = JoinSet::new();
        for pool in pools.unwrap() {
            let simulator = self.simulator_pool.get();
            let token_in_type = token_in_type.to_string();
            let token_out_type = token_out_type.clone();
            join_set.spawn(async move { new_dexes(simulator, &pool, &token_in_type, token_out_type).await });
        }

        let mut res = Vec::new();
        while let Some(Ok(result)) = join_set.join_next().await {
            match result {
                Ok(dexes) => res.extend(dexes),
                Err(_error) => {
                    // trace!(?error, "invalid pool");
                }
            }
        }

        Ok(res)
    }

    async fn find_test_path(&self, path: &[ObjectID]) -> Result<Path> {
        let mut dexes = vec![];
        let mut coin_in = SUI_COIN_TYPE.to_string();

        for pool_id in path {
            let simulator = self.simulator_pool.get();
            let pool = self.indexer.get_pool_by_id(pool_id).ok_or_eyre("pool not found")?;
            let dex = new_dexes(simulator, &pool, &coin_in, None).await?.pop().unwrap();
            coin_in = dex.coin_out_type();
            dexes.push(dex);
        }

        Ok(Path { path: dexes })
    }
}
