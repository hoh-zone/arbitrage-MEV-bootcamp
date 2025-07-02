mod arb_cache;
mod worker;

use std::{
    collections::{HashSet, VecDeque},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use arb_cache::{ArbCache, ArbItem};
use async_channel::Sender;
use burberry::ActionSubmitter;
use dex_indexer::types::Protocol;
use eyre::{ensure, eyre, Result};
use fastcrypto::encoding::{Base64, Encoding};
use object_pool::ObjectPool;
use rayon::prelude::*;
use shio::{ShioItem, ShioObject};
use simulator::{ReplaySimulator, SimEpoch, SimulateCtx, Simulator};
use sui_json_rpc_types::{SuiEvent, SuiTransactionBlockEffects, SuiTransactionBlockEffectsAPI};
use sui_sdk::{SuiClient, SuiClientBuilder};
use sui_types::{
    base_types::{MoveObjectType, ObjectID, SuiAddress},
    committee::ProtocolVersion,
    digests::TransactionDigest,
    object::{MoveObject, Object, Owner, OBJECT_START_VERSION},
    supported_protocol_versions::{Chain, ProtocolConfig},
    transaction::{InputObjectKind, ObjectReadResult, TransactionData},
};
use tokio::{
    runtime::{Builder, Handle, RuntimeFlavor},
    task::JoinSet,
};
use tracing::{debug, error, info, instrument, warn};
use worker::Worker;

use crate::{
    arb::Arb,
    common::get_latest_epoch,
    types::{Action, Event, Source},
};

pub struct ArbStrategy {
    sender: SuiAddress,
    arb_item_sender: Option<Sender<ArbItem>>,
    arb_cache: ArbCache,

    recent_arbs: VecDeque<String>,
    max_recent_arbs: usize,

    simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>,
    own_simulator: Arc<dyn Simulator>, // only for execution of pending txs
    rpc_url: String,
    workers: usize,
    sui: SuiClient,
    epoch: Option<SimEpoch>,
    dedicated_simulator: Option<Arc<ReplaySimulator>>,
}

impl ArbStrategy {
    pub async fn new(
        attacker: SuiAddress,
        simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>,
        own_simulator: Arc<dyn Simulator>,
        recent_arbs: usize,
        rpc_url: &str,
        workers: usize,
        dedicated_simulator: Option<Arc<ReplaySimulator>>,
    ) -> Self {
        let sui = SuiClientBuilder::default().build(&rpc_url).await.unwrap();
        let epoch = get_latest_epoch(&sui).await.unwrap();

        Self {
            sender: attacker,
            arb_item_sender: None,
            arb_cache: ArbCache::new(Duration::from_secs(5)),
            recent_arbs: VecDeque::with_capacity(recent_arbs),
            max_recent_arbs: recent_arbs,
            simulator_pool,
            own_simulator,
            rpc_url: rpc_url.to_string(),
            workers,
            sui,
            epoch: Some(epoch),
            dedicated_simulator,
        }
    }

    #[instrument(name = "on-new-tx", skip_all, fields(tx = %tx.digest()))]
    async fn on_new_tx(&self, tx: TransactionData) -> Result<()> {
        // 1. simulate
        // 2. parse tx_effects/simulate_result
        // 3. enqueue arb_item
        Ok(())
    }

    #[instrument(name = "on-new-tx-effects", skip_all, fields(tx = %tx_effects.transaction_digest()))]
    async fn on_new_tx_effects(&mut self, tx_effects: SuiTransactionBlockEffects, events: Vec<SuiEvent>) -> Result<()> {
        let coin_pools = self.parse_involved_coin_pools(events).await;
        if coin_pools.is_empty() {
            return Ok(());
        }

        let tx_digest = tx_effects.transaction_digest();
        let epoch = self.get_latest_epoch().await?;
        let sim_ctx = SimulateCtx::new(epoch, vec![]);

        for (coin, pool_id) in coin_pools {
            self.arb_cache
                .insert(coin, pool_id, *tx_digest, sim_ctx.clone(), Source::Public);
        }

        Ok(())
    }

    #[instrument(name = "on-new-shio-item", skip_all, fields(tx = %shio_item.tx_digest()))]
    async fn on_new_shio_item(&mut self, shio_item: ShioItem) -> Result<()> {
        let (coin_pools, override_objects) = match self.get_potential_opportunity(&shio_item).await {
            Some(potential_opportunity) => potential_opportunity,
            None => return Ok(()),
        };

        let tx_digest = TransactionDigest::from_str(shio_item.tx_digest()).map_err(|e| eyre!(e))?;
        let epoch = self.get_latest_epoch().await?;
        let mut sim_ctx = SimulateCtx::new(epoch, override_objects);
        // A bid must has the exact gas_price as the opportunity transaction's.
        sim_ctx.with_gas_price(shio_item.gas_price());

        let source = Source::Shio {
            opp_tx_digest: tx_digest,
            bid_amount: 0,
            start: utils::current_time_ms(),
            // move the deadline up slightly to allow time for the final dry_run and network latency
            deadline: shio_item.deadline_timestamp_ms() - 20,
            arb_found: 0,
        };

        for (coin, pool_id) in coin_pools {
            self.arb_cache.insert(coin, pool_id, tx_digest, sim_ctx.clone(), source);
        }

        Ok(())
    }

    async fn parse_involved_coin_pools(&self, events: Vec<SuiEvent>) -> HashSet<(String, Option<ObjectID>)> {
        let mut join_set = JoinSet::new();

        for event in events {
            let own_simulator = self.own_simulator.clone();
            join_set.spawn(async move {
                if let Ok(protocol) = Protocol::try_from(&event) {
                    if let Ok(swap_event) = protocol.sui_event_to_swap_event(&event, own_simulator).await {
                        return Some((swap_event.involved_coin_one_side(), swap_event.pool_id()));
                    }
                }
                None
            });
        }

        let mut coin_pools = HashSet::new();
        while let Some(result) = join_set.join_next().await {
            if let Ok(Some((coin, pool_id))) = result {
                coin_pools.insert((coin, pool_id));
            }
        }

        coin_pools
    }

    // returns (involved_coin_pools, override_objects) if there are swap events.
    async fn get_potential_opportunity(
        &self,
        shio_item: &ShioItem,
    ) -> Option<(HashSet<(String, Option<ObjectID>)>, Vec<ObjectReadResult>)> {
        // parse involved coins from swap events
        let events = shio_item.events();
        if events.is_empty() {
            return None;
        }

        let mut join_set = JoinSet::new();
        for event in events {
            let own_simulator = self.own_simulator.clone();
            join_set.spawn(async move {
                if let Ok(protocol) = Protocol::try_from(&event) {
                    if let Ok(swap_event) = protocol.shio_event_to_swap_event(&event, own_simulator).await {
                        return Some((swap_event.involved_coin_one_side(), swap_event.pool_id()));
                    }
                }
                None
            });
        }

        let mut involved_coin_pools = HashSet::new();
        while let Some(result) = join_set.join_next().await {
            if let Ok(Some((coin, pool_id))) = result {
                involved_coin_pools.insert((coin, pool_id));
            }
        }

        if involved_coin_pools.is_empty() {
            return None;
        }

        // parse override_objects from created/mutated objects
        let tx_digest = TransactionDigest::from_str(shio_item.tx_digest()).ok()?;
        let override_objects: Vec<ObjectReadResult> = shio_item
            .created_mutated_objects()
            .par_iter()
            .filter_map(|shio_obj| new_object_read_result(tx_digest, shio_obj).ok())
            .collect();

        Some((involved_coin_pools, override_objects))
    }

    async fn get_latest_epoch(&mut self) -> Result<SimEpoch> {
        if let Some(epoch) = self.epoch {
            if !epoch.is_stale() {
                return Ok(epoch);
            } else {
                self.epoch = None;
            }
        }

        let epoch = get_latest_epoch(&self.sui).await?;
        self.epoch = Some(epoch);
        Ok(epoch)
    }
}

fn new_object_read_result(tx_digest: TransactionDigest, shio_obj: &ShioObject) -> Result<ObjectReadResult> {
    ensure!(
        shio_obj.data_type() == "moveObject",
        "invalid data type: {}",
        shio_obj.data_type()
    );

    let id = ObjectID::from_hex_literal(&shio_obj.id)?;

    let move_obj = {
        let type_: MoveObjectType = serde_json::from_str(&shio_obj.object_type)?;
        let has_public_transfer = shio_obj.has_public_transfer();
        let version = OBJECT_START_VERSION;
        let contents = Base64::decode(&shio_obj.object_bcs)?;
        let protocol_config = ProtocolConfig::get_for_version(ProtocolVersion::MAX, Chain::Mainnet);
        unsafe { MoveObject::new_from_execution(type_, has_public_transfer, version, contents, &protocol_config)? }
    };

    let owner = serde_json::from_value::<Owner>(shio_obj.owner.clone())?;
    let previous_transaction = tx_digest;
    let object = Object::new_move(move_obj, owner.clone(), previous_transaction);

    let input_object_kind = match owner {
        Owner::Shared { initial_shared_version } => InputObjectKind::SharedMoveObject {
            id,
            initial_shared_version,
            mutable: true,
        },
        _ => InputObjectKind::ImmOrOwnedMoveObject(object.compute_object_reference()),
    };

    Ok(ObjectReadResult::new(input_object_kind, object.into()))
}

#[macro_export]
macro_rules! run_in_tokio {
    ($code:expr) => {
        match Handle::try_current() {
            Ok(handle) => match handle.runtime_flavor() {
                RuntimeFlavor::CurrentThread => std::thread::scope(move |s| {
                    s.spawn(move || {
                        Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap()
                            .block_on(async move { $code.await })
                    })
                    .join()
                    .unwrap()
                }),
                _ => tokio::task::block_in_place(move || handle.block_on(async move { $code.await })),
            },
            Err(_) => Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move { $code.await }),
        }
    };
}

#[burberry::async_trait]
impl burberry::Strategy<Event, Action> for ArbStrategy {
    fn name(&self) -> &str {
        "ArbStrategy"
    }

    async fn sync_state(&mut self, submitter: Arc<dyn ActionSubmitter<Action>>) -> Result<()> {
        if self.arb_item_sender.is_some() {
            panic!("already synced!");
        }

        let (arb_item_sender, arb_item_receiver) = async_channel::unbounded();
        self.arb_item_sender = Some(arb_item_sender);

        let sender = self.sender;
        let rpc_url = self.rpc_url.clone();

        let workers_to_spawn = self.workers;
        info!("spawning {} workers to process messages", workers_to_spawn);

        let (init_tx, mut init_rx) = tokio::sync::mpsc::channel(workers_to_spawn);

        for id in 0..workers_to_spawn {
            debug!(worker.id = id, "spawning worker...");

            let arb_item_receiver = arb_item_receiver.clone();
            let submitter = submitter.clone();

            let sui = SuiClientBuilder::default().build(&rpc_url).await?;
            let rpc_url = rpc_url.clone();
            let init_tx = init_tx.clone();
            let simulator_pool_arb = self.simulator_pool.clone();
            let simulator_pool_worker = self.simulator_pool.clone();
            let simulator_name = simulator_pool_arb.get().name().to_string();
            let dedicated_simulator = self.dedicated_simulator.clone();

            let _ = std::thread::Builder::new()
                .stack_size(128 * 1024 * 1024) // 128 MB
                .name(format!("worker-{id}"))
                .spawn(move || {
                    let arb = Arc::new(run_in_tokio!({ Arb::new(&rpc_url, simulator_pool_arb) }).unwrap());

                    // Signal that this worker is initialized
                    run_in_tokio!(init_tx.send(())).unwrap();

                    let worker = Worker {
                        _id: id,
                        sender,
                        arb_item_receiver,
                        simulator_pool: simulator_pool_worker,
                        simulator_name,
                        submitter,
                        sui,
                        arb,
                        dedicated_simulator,
                    };
                    worker.run().unwrap_or_else(|e| panic!("worker {id} panicked: {e:?}"));
                });
        }

        // Wait for all workers to initialize
        for _ in 0..workers_to_spawn {
            init_rx.recv().await.expect("worker initialization failed");
        }

        info!("workers all spawned!");
        Ok(())
    }

    async fn process_event(&mut self, event: Event, _submitter: Arc<dyn ActionSubmitter<Action>>) {
        let result = match event {
            Event::PublicTx(tx_effects, events) => self.on_new_tx_effects(tx_effects, events).await,
            Event::PrivateTx(tx_data) => self.on_new_tx(tx_data).await,
            Event::Shio(shio_item) => self.on_new_shio_item(shio_item).await,
        };
        if let Err(error) = result {
            error!(?error, "failed to process event");
            return;
        }

        // send arb_item to workers if channel is < 10
        let channel_len = self.arb_item_sender.as_ref().unwrap().len();
        if channel_len < 10 {
            let num_to_send = 10 - channel_len;
            for _ in 0..num_to_send {
                if let Some(item) = self.arb_cache.pop_one() {
                    if !self.recent_arbs.contains(&item.coin) || item.source.is_shio() {
                        let coin = item.coin.clone();
                        self.arb_item_sender.as_ref().unwrap().send(item).await.unwrap();

                        self.recent_arbs.push_back(coin);
                        if self.recent_arbs.len() > self.max_recent_arbs {
                            self.recent_arbs.pop_front();
                        }
                    }
                } else {
                    // no more arb_item to send
                    break;
                }
            }
        } else {
            warn!("arb_item channel stash {}", channel_len);
        }

        let expired_coins = self.arb_cache.remove_expired();
        for coin in expired_coins {
            if let Some(pos) = self.recent_arbs.iter().position(|x| x == &coin) {
                self.recent_arbs.remove(pos);
            }
        }
    }
}
