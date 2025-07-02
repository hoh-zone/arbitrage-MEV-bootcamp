mod override_cache;
mod replay_simulator;

pub use replay_simulator::ReplaySimulator;

use std::{
    collections::HashSet,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use async_trait::async_trait;
use eyre::Result;
use move_core_types::annotated_value::{MoveDatatypeLayout, MoveStructLayout};
use prometheus::Registry;
use sui_config::{NodeConfig, PersistedConfig};
use sui_core::{
    authority::{authority_store_tables::AuthorityPerpetualTables, backpressure::BackpressureManager, AuthorityStore},
    execution_cache::{metrics::ExecutionCacheMetrics, ExecutionCacheWrite, ObjectCacheRead, WritebackCache},
};
use sui_execution::Executor;
use sui_indexer::errors::IndexerError;
use sui_json_rpc::{get_balance_changes_from_effect, ObjectProvider};
use sui_json_rpc_types::{BalanceChange, SuiTransactionBlockEffects, SuiTransactionBlockEvents};
use sui_sdk::SUI_COIN_TYPE;
use sui_types::{
    base_types::{ObjectID, SequenceNumber},
    committee::{EpochId, ProtocolVersion},
    digests::TransactionDigest,
    effects::TransactionEffects,
    error::SuiError,
    gas::SuiGasStatus,
    inner_temporary_store::InnerTemporaryStore,
    metrics::LimitsMetrics,
    object::{MoveObject, Object, Owner, OBJECT_START_VERSION},
    storage::{BackingPackageStore, ObjectKey, ObjectStore},
    supported_protocol_versions::{Chain, ProtocolConfig},
    transaction::{
        CheckedInputObjects, InputObjectKind, InputObjects, ObjectReadResult, ObjectReadResultKind, TransactionData,
        TransactionDataAPI,
    },
    TypeTag,
};
use tokio::{io::AsyncReadExt, net::UnixStream};
use tracing::{debug, error, info};

use super::{SimulateCtx, SimulateResult, Simulator};
use override_cache::OverrideCache;

pub struct DBSimulator {
    pub store: Arc<WritebackCache>,
    executor: Arc<dyn Executor + Send + Sync>,
    protocol_config: ProtocolConfig,
    metrics: Arc<LimitsMetrics>,
    writeback_metrics: Arc<ExecutionCacheMetrics>,
    with_fallback: bool,
}

impl DBSimulator {
    pub async fn new_authority_store(store_path: &str, config_path: &str) -> Arc<AuthorityStore> {
        let config: NodeConfig = PersistedConfig::read(&PathBuf::from(config_path))
            .map_err(|err| err.context(format!("Cannot open Sui Node Config file at {:?}", config_path)))
            .unwrap();

        let genesis = config.genesis().unwrap().clone();

        let perpetual_tables = Arc::new(AuthorityPerpetualTables::open_readonly_as_rw(Path::new(store_path)));

        AuthorityStore::open(perpetual_tables, &genesis, &config, &Registry::new())
            .await
            .unwrap()
    }

    pub async fn new_slow(
        store_path: &str,
        config_path: &str,
        update_socket: Option<&str>,
        preload_path: Option<&str>,
    ) -> Self {
        let authority_store = Self::new_authority_store(store_path, config_path).await;
        Self::new(
            authority_store,
            update_socket.map(PathBuf::from),
            preload_path.map(PathBuf::from),
            true,
        )
        .await
    }

    pub async fn new_default_slow() -> Self {
        Self::new_slow(
            "/home/ubuntu/sui/db/live/store",
            "/home/ubuntu/sui/fullnode.yaml",
            None,
            Some("/home/ubuntu/suiflow-relay/pool_related_ids.txt"),
        )
        .await
    }

    pub async fn new_test(fallback: bool) -> Self {
        let authority_store =
            Self::new_authority_store("/home/ubuntu/sui/db/live/store", "/home/ubuntu/sui/fullnode.yaml").await;

        Self::new(authority_store, None, None, fallback).await
    }

    pub async fn new(
        authority_store: Arc<AuthorityStore>,
        update_socket: Option<PathBuf>,
        preload_path: Option<PathBuf>,
        with_fallback: bool,
    ) -> Self {
        let metrics = Arc::new(ExecutionCacheMetrics::new(&Registry::new()));
        let backpressure_manager = BackpressureManager::new_for_tests();

        let writeback_cache = Arc::new(WritebackCache::new(
            &Default::default(),
            authority_store.clone(),
            metrics.clone(),
            backpressure_manager,
        ));

        let preload_ids = if let Some(preload_path) = preload_path {
            let objects_ids = std::fs::read_to_string(preload_path).unwrap();
            objects_ids
                .trim()
                .split("\n")
                .map(|s| ObjectID::from_str(s).unwrap())
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        let preload_ids = preload_ids.iter().cloned().collect::<Vec<_>>();

        // preload objects
        let _ = writeback_cache.multi_get_objects(&preload_ids);

        if let Some(update_socket) = update_socket {
            let execution_cache_writer = writeback_cache.clone();
            let preload_ids = preload_ids.clone();
            std::thread::Builder::new()
                .name("update-thread".to_string())
                .spawn(move || spawn_update_thread(update_socket, preload_ids, execution_cache_writer))
                .unwrap();
        }

        let mut protocol_config = ProtocolConfig::get_for_version(ProtocolVersion::MAX, Chain::Mainnet);

        protocol_config.object_runtime_max_num_cached_objects = Some(1000000);
        protocol_config.object_runtime_max_num_cached_objects_system_tx = Some(1000000);
        protocol_config.object_runtime_max_num_store_entries = Some(1000000);
        protocol_config.object_runtime_max_num_store_entries_system_tx = Some(1000000);

        let executor =
            sui_execution::executor(&protocol_config, true, None).expect("Creating an executor should not fail here");

        Self {
            store: writeback_cache,
            executor,
            protocol_config,
            metrics: Arc::new(LimitsMetrics::new(&Registry::new())),
            writeback_metrics: metrics,
            with_fallback,
        }
    }

    pub fn get_input_objects(
        &self,
        input_object_kinds: &[InputObjectKind],
        epoch_id: EpochId,
    ) -> Result<InputObjects, SuiError> {
        let mut input_results = vec![None; input_object_kinds.len()];
        let mut object_refs = Vec::with_capacity(input_object_kinds.len());
        let mut fetch_indices = Vec::with_capacity(input_object_kinds.len());

        for (i, kind) in input_object_kinds.iter().enumerate() {
            match kind {
                // Packages are loaded one at a time via the cache
                InputObjectKind::MovePackage(id) => {
                    let Some(package) = self.store.get_package_object(id)?.map(|o| o.into()) else {
                        return Err(SuiError::from(kind.object_not_found_error()));
                    };
                    input_results[i] = Some(ObjectReadResult {
                        input_object_kind: *kind,
                        object: ObjectReadResultKind::Object(package),
                    });
                }
                InputObjectKind::SharedMoveObject { id, .. } => match self.store.get_object(id) {
                    Some(object) => input_results[i] = Some(ObjectReadResult::new(*kind, object.into())),
                    None => {
                        if let Some((version, digest)) = self.store.get_last_shared_object_deletion_info(id, epoch_id) {
                            input_results[i] = Some(ObjectReadResult {
                                input_object_kind: *kind,
                                object: ObjectReadResultKind::DeletedSharedObject(version, digest),
                            });
                        } else {
                            return Err(SuiError::from(kind.object_not_found_error()));
                        }
                    }
                },
                InputObjectKind::ImmOrOwnedMoveObject(objref) => {
                    object_refs.push(*objref);
                    fetch_indices.push(i);
                }
            }
        }

        let objects = self
            .store
            .multi_get_objects_by_key(&object_refs.iter().map(ObjectKey::from).collect::<Vec<_>>());
        assert_eq!(objects.len(), object_refs.len());
        for (index, object) in fetch_indices.into_iter().zip(objects.into_iter()) {
            // ignore mock objects
            if let Some(object) = object {
                input_results[index] = Some(ObjectReadResult {
                    input_object_kind: input_object_kinds[index],
                    object: ObjectReadResultKind::Object(object),
                });
            }
        }

        Ok(input_results.into_iter().flatten().collect::<Vec<_>>().into())
    }

    fn get_mutated_objects(
        &self,
        effects: &TransactionEffects,
        store: &InnerTemporaryStore,
    ) -> eyre::Result<Vec<ObjectReadResult>> {
        let mut object_changes = vec![];
        for (obj_ref, owner) in effects.mutated_excluding_gas() {
            if let Some(obj) = store.written.get(&obj_ref.0) {
                let object = ObjectReadResultKind::Object(obj.clone());

                let kind = match owner {
                    Owner::Shared { initial_shared_version } => InputObjectKind::SharedMoveObject {
                        id: obj_ref.0,
                        initial_shared_version,
                        mutable: true,
                    },
                    _ => InputObjectKind::ImmOrOwnedMoveObject(obj_ref),
                };

                object_changes.push(ObjectReadResult::new(kind, object));
            }
        }

        Ok(object_changes)
    }
}

#[async_trait]
impl Simulator for DBSimulator {
    async fn simulate(&self, tx: TransactionData, ctx: SimulateCtx) -> eyre::Result<SimulateResult> {
        let cache_misses_before = self.writeback_metrics.cache_misses_count();

        let SimulateCtx {
            epoch,
            mut override_objects,
            borrowed_coin,
        } = ctx;

        let mut input_objects = self.get_input_objects(&tx.input_objects()?, epoch.epoch_id)?;

        let sender = tx.sender();
        let original_gas = tx.gas().to_vec();

        let mock_gas_id =
            ObjectID::from_str("0x0000000000000000000000000000000000000000000000000000000000001337").unwrap();
        let use_mock_gas = original_gas.is_empty();
        let (gas_ref, gas_obj) = if use_mock_gas {
            let sender = tx.sender();
            // use a 1B sui coin
            const MIST_TO_SUI: u64 = 1_000_000_000;
            const DRY_RUN_SUI: u64 = 1_000_000_000;

            let max_coin_value = MIST_TO_SUI * DRY_RUN_SUI;
            let gas_object = Object::new_move(
                MoveObject::new_gas_coin(OBJECT_START_VERSION, mock_gas_id, max_coin_value),
                Owner::AddressOwner(sender),
                TransactionDigest::genesis_marker(),
            );
            let gas_object_ref = gas_object.compute_object_reference();
            (vec![gas_object_ref], Some(gas_object))
        } else {
            (original_gas, None)
        };

        let gas_status = match SuiGasStatus::new(tx.gas_budget(), tx.gas_price(), tx.gas_price(), &self.protocol_config)
            .map_err(|e| eyre::eyre!(e))
        {
            Ok(gas_status) => gas_status,
            Err(e) => {
                info!("simulate error: {:?}", e);
                return Err(e);
            }
        };

        // extend override objects with mocked gas and borrowed coin
        if use_mock_gas {
            let gas_obj = gas_obj.unwrap();
            let object_read_result = ObjectReadResult {
                input_object_kind: InputObjectKind::ImmOrOwnedMoveObject(gas_obj.compute_object_reference()),
                object: ObjectReadResultKind::Object(gas_obj),
            };

            input_objects.objects.push(object_read_result.clone());
            override_objects.push(object_read_result);
        }

        if let Some((borrowed_coin, _borrowed_amount)) = &borrowed_coin {
            let object_read_result = ObjectReadResult {
                input_object_kind: InputObjectKind::ImmOrOwnedMoveObject(borrowed_coin.compute_object_reference()),
                object: ObjectReadResultKind::Object(borrowed_coin.clone()),
            };

            input_objects.objects.push(object_read_result.clone());
            override_objects.push(object_read_result);
        }

        // create override cache
        let override_cache = if self.with_fallback {
            OverrideCache::new(Some(self.store.clone()), override_objects)
        } else {
            OverrideCache::new(None, override_objects)
        };

        // update input objects again with override cache
        for object_read_result in input_objects.objects.iter_mut() {
            if let ObjectReadResultKind::Object(object) = &object_read_result.object {
                if let Some(object) = (&override_cache as &dyn ObjectCacheRead).get_object(&object.id()) {
                    object_read_result.object = ObjectReadResultKind::Object(object);
                }
            }
        }

        let digest = tx.digest();
        let kind = tx.into_kind();
        let input_object_kinds = input_objects.object_kinds().cloned().collect::<Vec<_>>();

        let simulate_start = std::time::Instant::now();

        let (inner_temporary_store, effects) = catch_unwind(AssertUnwindSafe(|| {
            let (inner_temporary_store, _, effects, _) = self.executor.execute_transaction_to_effects(
                &override_cache,
                &self.protocol_config,
                self.metrics.clone(),
                false,
                &HashSet::new(),
                &epoch.epoch_id,
                epoch.epoch_start_timestamp,
                CheckedInputObjects::new_with_checked_transaction_inputs(input_objects),
                gas_ref,
                gas_status,
                kind,
                sender,
                digest,
            );
            (inner_temporary_store, effects)
        }))
        .map_err(|e| eyre::eyre!("failed to simulate: {e:?}"))?;

        debug!("simulate tx_data elapsed: {:?}", simulate_start.elapsed());

        let object_changes = self.get_mutated_objects(&effects, &inner_temporary_store)?;

        let executed_db = ExecutedDB {
            db: &override_cache,
            temp_store: &inner_temporary_store,
        };

        // don't let sui calc balance change. we will do it manually
        let mut balance_changes = if !use_mock_gas {
            // ignore borrowed coin
            get_balance_changes_from_effect(
                &executed_db,
                &effects,
                input_object_kinds,
                borrowed_coin.clone().map(|(obj, _)| vec![obj.id()]),
            )
            .await?
        } else {
            let mut ignore_ids = vec![mock_gas_id];
            if let Some((borrowed_coin_obj, _)) = &borrowed_coin {
                ignore_ids.push(borrowed_coin_obj.id());
            }
            get_balance_changes_from_effect(&executed_db, &effects, input_object_kinds, Some(ignore_ids)).await?
        };

        // Subtract how much we borrowed
        // TODO: we should borrow any coin other than just sui
        if let Some((_borrowed_coin_obj, borrowed_amount)) = &borrowed_coin {
            let mut found = false;
            if let Some(bc) = balance_changes
                .iter_mut()
                .find(|bc| bc.owner == Owner::AddressOwner(sender) && bc.coin_type.to_string() == SUI_COIN_TYPE)
            {
                found = true;
                bc.amount -= *borrowed_amount as i128;
            }

            if !found {
                balance_changes.push(BalanceChange {
                    owner: Owner::AddressOwner(sender),
                    coin_type: TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
                        address: move_core_types::account_address::AccountAddress::TWO,
                        module: sui_types::Identifier::new("sui").unwrap(),
                        name: sui_types::Identifier::new("SUI").unwrap(),
                        type_params: vec![],
                    })),
                    amount: -(*borrowed_amount as i128),
                });
            }
        }

        if use_mock_gas {
            let mut found = false;

            let init_amount = 1_000_000_000u64 * 1_000_000_000u64;
            let final_amount = inner_temporary_store
                .written
                .get(&mock_gas_id)
                .unwrap()
                .as_coin_maybe()
                .unwrap()
                .value();

            for bc in balance_changes.iter_mut() {
                if bc.owner == Owner::AddressOwner(sender) && bc.coin_type.to_string() == SUI_COIN_TYPE {
                    bc.amount -= (init_amount - final_amount) as i128;
                    found = true;
                }
            }

            if !found {
                // we manually add a balance change for the mock gas
                balance_changes.push(BalanceChange {
                    owner: Owner::AddressOwner(sender),
                    coin_type: TypeTag::Struct(Box::new(move_core_types::language_storage::StructTag {
                        address: move_core_types::account_address::AccountAddress::TWO,
                        module: sui_types::Identifier::new("sui").unwrap(),
                        name: sui_types::Identifier::new("SUI").unwrap(),
                        type_params: vec![],
                    })),
                    amount: final_amount as i128 - init_amount as i128,
                });
            }
        }

        let mut layout_resolver = self.executor.type_layout_resolver(Box::new(&self.store));
        let events =
            SuiTransactionBlockEvents::try_from(inner_temporary_store.events, digest, None, layout_resolver.as_mut())?;

        let cache_misses = self
            .writeback_metrics
            .cache_misses_count()
            .saturating_sub(cache_misses_before);

        Ok(SimulateResult {
            effects: SuiTransactionBlockEffects::try_from(effects)?,
            events,
            object_changes,
            balance_changes,
            cache_misses,
        })
    }

    fn name(&self) -> &str {
        "DBSimulator"
    }

    async fn get_object(&self, obj_id: &ObjectID) -> Option<Object> {
        self.store.get_object(obj_id)
    }

    fn get_object_layout(&self, obj_id: &ObjectID) -> Option<MoveStructLayout> {
        let object = self.store.get_object(obj_id)?;
        let obj_type = object.type_().cloned()?;

        let layout = self
            .executor
            .type_layout_resolver(Box::new(&self.store))
            .get_annotated_layout(&obj_type.into());

        match layout {
            Ok(layout) => match layout {
                MoveDatatypeLayout::Struct(layout) => Some(*layout),
                _ => None,
            },
            Err(_) => {
                error!("failed to get layout for object: {:?}", obj_id);
                None
            }
        }
    }
}

struct ExecutedDB<'a> {
    db: &'a OverrideCache,
    temp_store: &'a InnerTemporaryStore,
}

#[async_trait]
impl<'a> ObjectProvider for ExecutedDB<'a> {
    type Error = IndexerError;

    async fn get_object(&self, id: &ObjectID, version: &SequenceNumber) -> Result<Object, Self::Error> {
        if let Some(obj) = self.db.get_versioned_object_for_comparison(id, *version) {
            return Ok(obj);
        }

        if let Some(obj) = self.temp_store.input_objects.get(id) {
            if obj.version() == *version {
                return Ok(obj.clone());
            }
        }

        if let Some(obj) = self.temp_store.written.get(id) {
            if obj.version() == *version {
                return Ok(obj.clone());
            }
        }

        if let Some(obj) = (self.db as &dyn ObjectCacheRead).get_object_by_key(id, *version) {
            return Ok(obj);
        }

        Err(IndexerError::GenericError(format!(
            "Object not found: {:?}, {}",
            id, version
        )))
    }

    async fn find_object_lt_or_eq_version(
        &self,
        id: &ObjectID,
        version: &SequenceNumber,
    ) -> Result<Option<Object>, Self::Error> {
        if let Some(obj) = self.temp_store.written.get(id) {
            if obj.version() <= *version {
                return Ok(Some(obj.clone()));
            }
        }

        Ok(self.db.find_object_lt_or_eq_version(*id, *version))
    }
}

pub trait CacheWriter: ExecutionCacheWrite + ObjectStore {}

impl CacheWriter for WritebackCache {}

#[inline]
pub fn preload_objects(cache_writer: Arc<dyn CacheWriter>, preload_ids: &[ObjectID]) {
    let preload_objects = cache_writer
        .multi_get_objects(&preload_ids)
        .into_iter()
        .filter_map(|obj| match obj {
            Some(obj) => Some((obj.id(), obj)),
            None => None,
        })
        .collect::<Vec<_>>();

    cache_writer.reload_objects(preload_objects);
}

#[tokio::main]
async fn spawn_update_thread(socket_path: PathBuf, preload_ids: Vec<ObjectID>, cache_writer: Arc<dyn CacheWriter>) {
    let mut socket = UnixStream::connect(socket_path)
        .await
        .expect("failed to connect to update socket");

    let mut buf = [0u8; 4];
    let mut last_catch_up_time = std::time::Instant::now();
    loop {
        // Read length prefix
        if let Err(e) = socket.read_exact(&mut buf).await {
            println!("Error reading length prefix: {}", e);
            break;
        }
        let len = u32::from_le_bytes(buf) as usize;

        // Read payload
        let mut payload = vec![0u8; len];
        if let Err(e) = socket.read_exact(&mut payload).await {
            println!("Error reading payload: {}", e);
            break;
        }

        match bcs::from_bytes::<Vec<(ObjectID, Object)>>(&payload) {
            Ok(objects) => {
                cache_writer.reload_objects(objects);
            }
            Err(e) => {
                println!("Error deserializing cache update: {}", e);
            }
        }

        // every day, update perpetual tables and clear cache
        if last_catch_up_time.elapsed() > std::time::Duration::from_secs(3600 * 24) {
            cache_writer.update_underlying(true);
            preload_objects(cache_writer.clone(), &preload_ids);

            last_catch_up_time = std::time::Instant::now();
        }
    }
}
