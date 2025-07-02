use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use sui_core::authority::authority_per_epoch_store::AuthorityPerEpochStore;
use sui_core::authority::SuiLockResult;
use sui_core::execution_cache::{ObjectCacheRead, WritebackCache};
use sui_types::base_types::{ObjectID, ObjectRef, SequenceNumber, VersionNumber};
use sui_types::bridge::Bridge;
use sui_types::clock::Clock;
use sui_types::committee::EpochId;
use sui_types::digests::TransactionDigest;
use sui_types::error::{SuiError, SuiResult, UserInputError};
use sui_types::id::{ID, UID};
use sui_types::messages_checkpoint::CheckpointSequenceNumber;
use sui_types::object::{Data, MoveObject, Object, ObjectInner, Owner, OBJECT_START_VERSION};
use sui_types::storage::{MarkerValue, ObjectKey, ObjectOrTombstone, PackageObject};
use sui_types::sui_system_state::SuiSystemState;
use sui_types::transaction::ObjectReadResultKind;
use sui_types::SUI_CLOCK_OBJECT_ID;
use sui_types::{
    storage::{BackingPackageStore, ChildObjectResolver, ObjectStore, ParentSync},
    transaction::ObjectReadResult,
};
use tracing::warn;

macro_rules! ret_latest_clock_obj {
    () => {{
        let object = ObjectInner {
            owner: Owner::Shared {
                initial_shared_version: OBJECT_START_VERSION,
            },
            data: Data::Move(MoveObject {
                type_: Clock::type_().into(),
                has_public_transfer: false,
                version: OBJECT_START_VERSION, // safe?
                contents: bcs::to_bytes(&Clock {
                    id: UID {
                        id: ID {
                            bytes: SUI_CLOCK_OBJECT_ID,
                        },
                    },
                    timestamp_ms: { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 },
                })
                .unwrap(),
            }),
            previous_transaction: TransactionDigest::genesis_marker(),
            storage_rebate: 0,
        }
        .into();
        object
    }};
}

pub struct OverrideCache {
    pub fallback: Option<Arc<WritebackCache>>,
    pub overrides: Vec<ObjectReadResult>, // usually it's small so vec is fine

    // when we reach fallback, we record the versioned object here
    pub versioned_cache: RwLock<BTreeMap<(ObjectID, SequenceNumber), Object>>,
}

impl OverrideCache {
    pub fn new(fallback: Option<Arc<WritebackCache>>, overrides: Vec<ObjectReadResult>) -> Self {
        Self {
            fallback,
            overrides,
            versioned_cache: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn get_override(&self, object_id: &ObjectID) -> Option<ObjectReadResult> {
        if object_id == &SUI_CLOCK_OBJECT_ID {
            return Some(ObjectReadResult {
                input_object_kind: sui_types::transaction::InputObjectKind::SharedMoveObject {
                    id: SUI_CLOCK_OBJECT_ID,
                    initial_shared_version: OBJECT_START_VERSION,
                    mutable: true,
                },
                object: ObjectReadResultKind::Object(ret_latest_clock_obj!()),
            });
        }

        self.overrides.iter().find(|o| o.id() == *object_id).cloned()
    }

    pub fn get_override_object(&self, object_id: &ObjectID) -> Option<Object> {
        match self.get_override(object_id) {
            Some(r) => match r.object {
                ObjectReadResultKind::Object(object) => Some(object),
                ObjectReadResultKind::DeletedSharedObject(_, _) => None,
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => unreachable!(),
            },
            _ => None,
        }
    }

    pub fn get_versioned_object_for_comparison(&self, object_id: &ObjectID, version: SequenceNumber) -> Option<Object> {
        self.versioned_cache
            .read()
            .unwrap()
            .get(&(*object_id, version))
            .cloned()
    }
}

impl BackingPackageStore for OverrideCache {
    fn get_package_object(&self, package_id: &ObjectID) -> SuiResult<Option<PackageObject>> {
        ObjectCacheRead::get_package_object(self, package_id)
    }
}

impl ChildObjectResolver for OverrideCache {
    fn read_child_object(
        &self,
        parent: &ObjectID,
        child: &ObjectID,
        child_version_upper_bound: SequenceNumber,
    ) -> SuiResult<Option<sui_types::object::Object>> {
        let Some(child_object) = self.find_object_lt_or_eq_version(*child, child_version_upper_bound) else {
            return Ok(None);
        };

        let parent = *parent;
        if child_object.owner != Owner::ObjectOwner(parent.into()) {
            let sui_error = SuiError::InvalidChildObjectAccess {
                object: *child,
                given_parent: parent,
                actual_owner: child_object.owner.clone(),
            };
            return Err(sui_error);
        }
        Ok(Some(child_object))
    }

    fn get_object_received_at_version(
        &self,
        owner: &ObjectID,
        receiving_object_id: &ObjectID,
        receive_object_at_version: SequenceNumber,
        epoch_id: EpochId,
    ) -> SuiResult<Option<sui_types::object::Object>> {
        let Some(recv_object) =
            ObjectCacheRead::get_object_by_key(self, receiving_object_id, receive_object_at_version)
        else {
            return Ok(None);
        };

        // Check for:
        // * Invalid access -- treat as the object does not exist. Or;
        // * If we've already received the object at the version -- then treat it as though it doesn't exist.
        // These two cases must remain indisguishable to the caller otherwise we risk forks in
        // transaction replay due to possible reordering of transactions during replay.
        if recv_object.owner != Owner::AddressOwner((*owner).into())
            || self.have_received_object_at_version(receiving_object_id, receive_object_at_version, epoch_id)
        {
            return Ok(None);
        }

        Ok(Some(recv_object))
    }
}
impl ObjectStore for OverrideCache {
    fn get_object(&self, object_id: &ObjectID) -> Option<Object> {
        ObjectCacheRead::get_object(self, object_id)
    }

    fn get_object_by_key(&self, object_id: &ObjectID, version: VersionNumber) -> Option<Object> {
        ObjectCacheRead::get_object_by_key(self, object_id, version)
    }
}
impl ParentSync for OverrideCache {
    fn get_latest_parent_entry_ref_deprecated(&self, _: ObjectID) -> Option<ObjectRef> {
        panic!("won't be called in new version")
    }
}

impl ObjectCacheRead for OverrideCache {
    fn get_package_object(&self, id: &ObjectID) -> SuiResult<Option<PackageObject>> {
        // first check if the object is in the overrides
        if let Some(override_object) = self.get_override(id) {
            match override_object.object {
                ObjectReadResultKind::Object(object) => return Ok(Some(PackageObject::new(object))),
                ObjectReadResultKind::DeletedSharedObject(_, _) => return Ok(None),
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => {
                    unreachable!("override object is in cancelled transaction")
                }
            }
        }

        warn!("❗️ [get_package_object] override missing: {:?}", id);
        if let Some(ref fallback) = self.fallback {
            fallback.get_package_object(id)
        } else {
            Ok(None)
        }
    }

    fn force_reload_system_packages(&self, system_package_ids: &[ObjectID]) {
        if let Some(ref fallback) = self.fallback {
            fallback.force_reload_system_packages(system_package_ids)
        }
    }

    fn get_object(&self, id: &ObjectID) -> Option<Object> {
        // first check if the object is in the overrides
        if let Some(override_object) = self.get_override(id) {
            match override_object.object {
                ObjectReadResultKind::Object(object) => return Some(object),
                ObjectReadResultKind::DeletedSharedObject(_, _) => return None,
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => {
                    unreachable!("override object is in cancelled transaction")
                }
            }
        }

        warn!("❗️ [get_object] override missing: {:?}", id);
        if let Some(ref fallback) = self.fallback {
            // if not, check the fallback
            let obj = fallback.get_object(id);
            if let Some(obj) = obj.clone() {
                self.versioned_cache.write().unwrap().insert((*id, obj.version()), obj);
            }
            obj
        } else {
            None
        }
    }

    fn get_latest_object_ref_or_tombstone(&self, object_id: ObjectID) -> Option<ObjectRef> {
        if let Some(override_object) = self.get_override_object(&object_id) {
            return Some(override_object.compute_object_reference());
        }

        warn!(
            "❗️ [get_latest_object_ref_or_tombstone] override missing: {:?}",
            object_id
        );
        // if it's not found, we lookup in fallback
        // if it's deleted, also lookup in fallback because it's not deleted in fallback
        // (we don't have object digest for deleted object in override)
        if let Some(ref fallback) = self.fallback {
            fallback.get_latest_object_ref_or_tombstone(object_id)
        } else {
            None
        }
    }

    fn get_latest_object_or_tombstone(&self, object_id: ObjectID) -> Option<(ObjectKey, ObjectOrTombstone)> {
        if let Some(override_object) = self.get_override(&object_id) {
            match override_object.object {
                ObjectReadResultKind::Object(object) => {
                    return Some((
                        ObjectKey::from(object.compute_object_reference()),
                        ObjectOrTombstone::Object(object),
                    ))
                }
                ObjectReadResultKind::DeletedSharedObject(_, _) => {
                    let undeleted_object = match self.fallback.as_ref()?.get_object(&object_id) {
                        Some(object) => object,
                        // is it possible?
                        None => return None,
                    };

                    let object_ref = undeleted_object.compute_object_reference();
                    return Some((ObjectKey::from(&object_ref), ObjectOrTombstone::Tombstone(object_ref)));
                }
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => {
                    unreachable!("override object is in cancelled transaction")
                }
            }
        }

        warn!("❗️ [get_latest_object_or_tombstone] override missing: {:?}", object_id);
        if let Some(ref fallback) = self.fallback {
            fallback.get_latest_object_or_tombstone(object_id)
        } else {
            None
        }
    }

    fn get_object_by_key(&self, object_id: &ObjectID, version: SequenceNumber) -> Option<Object> {
        if let Some(override_object) = self.get_override(object_id) {
            match override_object.object {
                ObjectReadResultKind::Object(object) => {
                    if object.version() == version {
                        return Some(object);
                    }
                }
                ObjectReadResultKind::DeletedSharedObject(_, _) => return None,
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => {
                    unreachable!("override object is in cancelled transaction")
                }
            }
        }

        warn!(
            "❗️ [get_object_by_key] override missing: {:?}, version: {:?}",
            object_id, version
        );
        if let Some(ref fallback) = self.fallback {
            fallback.get_object_by_key(object_id, version)
        } else {
            None
        }
    }

    fn multi_get_objects_by_key(&self, object_keys: &[ObjectKey]) -> Vec<Option<Object>> {
        let mut result = vec![];
        for object_key in object_keys {
            result.push((self as &dyn ObjectCacheRead).get_object_by_key(&object_key.0, object_key.1));
        }
        result
    }

    fn object_exists_by_key(&self, object_id: &ObjectID, version: SequenceNumber) -> bool {
        let Some(_) = (self as &dyn ObjectCacheRead).get_object_by_key(object_id, version) else {
            return false;
        };
        true
    }

    fn multi_object_exists_by_key(&self, object_keys: &[ObjectKey]) -> Vec<bool> {
        let mut result = vec![];
        for object_key in object_keys {
            result.push((self as &dyn ObjectCacheRead).object_exists_by_key(&object_key.0, object_key.1));
        }
        result
    }

    fn find_object_lt_or_eq_version(&self, object_id: ObjectID, version: SequenceNumber) -> Option<Object> {
        let object = (self as &dyn ObjectCacheRead).get_object(&object_id)?;
        if object.version() <= version {
            return Some(object);
        }
        None
    }

    fn get_lock(&self, obj_ref: ObjectRef, epoch_store: &AuthorityPerEpochStore) -> SuiLockResult {
        self.fallback
            .as_ref()
            .ok_or_else(|| SuiError::Unknown("No fallback".to_string()))?
            .get_lock(obj_ref, epoch_store)
    }

    fn _get_live_objref(&self, object_id: ObjectID) -> SuiResult<ObjectRef> {
        // if deleted, return Err
        if let Some(override_object) = self.get_override(&object_id) {
            match override_object.object {
                ObjectReadResultKind::Object(object) => {
                    return Ok(object.compute_object_reference());
                }
                ObjectReadResultKind::DeletedSharedObject(_, _) => {
                    return Err(SuiError::UserInputError {
                        error: UserInputError::ObjectNotFound {
                            object_id,
                            version: None,
                        },
                    })
                }
                ObjectReadResultKind::CancelledTransactionSharedObject(_) => {
                    unreachable!("override object is in cancelled transaction")
                }
            }
        }

        warn!("❗️ [_get_live_objref] override missing: {:?}", object_id);
        if let Some(ref fallback) = self.fallback {
            fallback._get_live_objref(object_id)
        } else {
            Err(SuiError::Unknown("No fallback".to_string()))
        }
    }

    fn check_owned_objects_are_live(&self, owned_object_refs: &[ObjectRef]) -> SuiResult {
        for owned_object_ref in owned_object_refs {
            if (self as &dyn ObjectCacheRead)
                .get_object_by_key(&owned_object_ref.0, owned_object_ref.1)
                .is_none()
            {
                return Err(UserInputError::ObjectVersionUnavailableForConsumption {
                    provided_obj_ref: *owned_object_ref,
                    current_version: owned_object_ref.1,
                }
                .into());
            }
        }

        Ok(())
    }

    fn get_sui_system_state_object_unsafe(&self) -> SuiResult<SuiSystemState> {
        self.fallback
            .as_ref()
            .ok_or_else(|| SuiError::Unknown("No fallback".to_string()))?
            .get_sui_system_state_object_unsafe()
    }

    fn get_bridge_object_unsafe(&self) -> SuiResult<Bridge> {
        self.fallback
            .as_ref()
            .ok_or_else(|| SuiError::Unknown("No fallback".to_string()))?
            .get_bridge_object_unsafe()
    }

    fn get_marker_value(
        &self,
        object_id: &ObjectID,
        version: SequenceNumber,
        epoch_id: EpochId,
    ) -> Option<MarkerValue> {
        // TODO: implement
        self.fallback.as_ref()?.get_marker_value(object_id, version, epoch_id)
    }

    fn get_latest_marker(&self, object_id: &ObjectID, epoch_id: EpochId) -> Option<(SequenceNumber, MarkerValue)> {
        // TODO: implement
        self.fallback.as_ref()?.get_latest_marker(object_id, epoch_id)
    }

    fn get_highest_pruned_checkpoint(&self) -> CheckpointSequenceNumber {
        if let Some(ref fallback) = self.fallback {
            fallback.get_highest_pruned_checkpoint()
        } else {
            CheckpointSequenceNumber::default()
        }
    }
}
