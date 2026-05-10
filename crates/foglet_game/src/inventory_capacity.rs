//! `inventory_capacity` — game-defined capacity policy contracts for v5.
//!
//! Capacity in Foglet remains deliberately game-authored. The kit owns
//! the transactional pattern for checking capacity before inventory
//! movement, while games own the unit, volume rules, and owner limits.
//!
//! - In a **dungeon crawler**, a policy might interpret volume as
//!   backpack weight.
//! - In a **town simulation**, a policy might interpret capacity as
//!   warehouse shelf slots.

use serde_json::Value;
use thiserror::Error;

use crate::inventory::InventoryError;
use crate::world_db::WorldDb;

/// Game-supplied capacity policy used by v5 inventory helpers.
///
/// The trait intentionally exposes two scalar callbacks:
///
/// - [`CapacityPolicy::item_volume`] maps one item plus its opaque
///   metadata into the capacity units the game uses.
/// - [`CapacityPolicy::owner_capacity`] returns the maximum capacity
///   for one owner, or `None` for uncapped owners.
///
/// The kit does not define the unit. A game can treat `1` as one
/// pound, one crate, one pocket slot, or any other integer measure.
pub trait CapacityPolicy {
    /// Return the capacity units consumed by one item quantity.
    fn item_volume(&self, item_key: &str, metadata: &Value) -> Result<i64, CapacityError>;

    /// Return this owner's maximum capacity, or `None` for no cap.
    fn owner_capacity(&self, owner_kind: &str, owner_id: i64)
        -> Result<Option<i64>, CapacityError>;
}

/// Errors produced by v5 capacity validation and transfers.
#[derive(Debug, Error)]
pub enum CapacityError {
    /// Adding the requested capacity would exceed the owner's limit.
    #[error("insufficient capacity: used {used}, requested {requested}, capacity {capacity}")]
    InsufficientCapacity {
        /// Capacity already used by the owner.
        used: i64,
        /// Additional capacity requested by the attempted operation.
        requested: i64,
        /// Maximum capacity returned by the policy.
        capacity: i64,
    },
    /// A game-supplied policy callback failed.
    #[error("capacity policy failed: {0}")]
    PolicyError(String),
    /// The underlying v4 inventory primitive failed.
    #[error("inventory capacity operation failed: {source}")]
    InventoryError {
        /// Wrapped v4 inventory error.
        #[source]
        source: Box<InventoryError>,
    },
}

impl From<InventoryError> for CapacityError {
    fn from(source: InventoryError) -> Self {
        Self::InventoryError {
            source: Box::new(source),
        }
    }
}

impl WorldDb {
    /// Sum used capacity for every inventory slot owned by one owner.
    ///
    /// Each slot contributes `policy.item_volume(item_key, metadata) *
    /// quantity`. Missing metadata is passed to the policy as JSON
    /// `null`; malformed metadata is surfaced as [`CapacityError::PolicyError`]
    /// because the game-authored policy cannot reason about it safely.
    pub fn used_capacity(
        &self,
        owner_kind: &str,
        owner_id: i64,
        policy: &impl CapacityPolicy,
    ) -> Result<i64, CapacityError> {
        let mut used = 0;
        for slot in self.slots_for_owner(owner_kind, owner_id)? {
            let metadata = slot
                .metadata_json
                .as_deref()
                .map(serde_json::from_str::<Value>)
                .transpose()
                .map_err(|source| CapacityError::PolicyError(source.to_string()))?
                .unwrap_or(Value::Null);
            let volume = policy.item_volume(&slot.item_key, &metadata)?;
            used += volume * slot.quantity;
        }
        Ok(used)
    }

    /// Validate that adding `quantity` of `item_key` would fit.
    ///
    /// Existing owner/item metadata is reused when a slot already
    /// exists, because v4 inventory treats incoming quantity as additive
    /// to the first matching slot. If no slot exists, the policy sees
    /// JSON `null` metadata for the proposed item.
    pub fn validate_incoming(
        &self,
        owner_kind: &str,
        owner_id: i64,
        item_key: &str,
        quantity: i64,
        policy: &impl CapacityPolicy,
    ) -> Result<(), CapacityError> {
        let Some(capacity) = policy.owner_capacity(owner_kind, owner_id)? else {
            return Ok(());
        };

        let used = self.used_capacity(owner_kind, owner_id, policy)?;
        let metadata = self
            .get_slot(owner_kind, owner_id, item_key)?
            .and_then(|slot| slot.metadata_json)
            .map(|metadata| serde_json::from_str::<Value>(&metadata))
            .transpose()
            .map_err(|source| CapacityError::PolicyError(source.to_string()))?
            .unwrap_or(Value::Null);
        let requested = policy.item_volume(item_key, &metadata)? * quantity;

        if used + requested > capacity {
            return Err(CapacityError::InsufficientCapacity {
                used,
                requested,
                capacity,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CapacityError, CapacityPolicy};
    use crate::inventory::INVENTORY_SLOTS_MIGRATION;
    use crate::world_db::WorldDb;
    use serde_json::json;
    use tempfile::tempdir;

    struct FixedPolicy;

    impl CapacityPolicy for FixedPolicy {
        fn item_volume(
            &self,
            item_key: &str,
            metadata: &serde_json::Value,
        ) -> Result<i64, CapacityError> {
            if metadata.get("broken").and_then(|value| value.as_bool()) == Some(true) {
                return Err(CapacityError::PolicyError(item_key.to_string()));
            }
            Ok(2)
        }

        fn owner_capacity(
            &self,
            _owner_kind: &str,
            owner_id: i64,
        ) -> Result<Option<i64>, CapacityError> {
            Ok((owner_id > 0).then_some(10))
        }
    }

    #[test]
    fn capacity_policy_trait_and_error_variants_are_usable() {
        let policy = FixedPolicy;

        assert_eq!(
            policy
                .item_volume("ration", &json!({"fresh": true}))
                .expect("volume callback succeeds"),
            2
        );
        assert_eq!(
            policy
                .owner_capacity("player", 7)
                .expect("capacity callback succeeds"),
            Some(10)
        );
        assert_eq!(
            policy
                .owner_capacity("warehouse", 0)
                .expect("uncapped owner succeeds"),
            None
        );

        let errors = [
            CapacityError::InsufficientCapacity {
                used: 9,
                requested: 2,
                capacity: 10,
            }
            .to_string(),
            CapacityError::PolicyError("bad item".to_string()).to_string(),
        ];
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn used_capacity_sums_volume_times_quantity_for_owner_slots() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 7, "ration", 3, None, None)
            .expect("ration slot inserts");
        world
            .create_slot("player", 7, "lantern", 2, None, Some(r#"{"bulky":true}"#))
            .expect("lantern slot inserts");
        world
            .create_slot("player", 8, "lantern", 99, None, None)
            .expect("other owner slot inserts");

        struct VolumePolicy;

        impl CapacityPolicy for VolumePolicy {
            fn item_volume(
                &self,
                item_key: &str,
                metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(
                    match (
                        item_key,
                        metadata.get("bulky").and_then(|value| value.as_bool()),
                    ) {
                        ("ration", _) => 1,
                        ("lantern", Some(true)) => 5,
                        ("lantern", _) => 2,
                        _ => 0,
                    },
                )
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(100))
            }
        }

        let used = world
            .used_capacity("player", 7, &VolumePolicy)
            .expect("used capacity computes");

        assert_eq!(used, 13);
    }

    #[test]
    fn validate_incoming_rejects_when_proposed_quantity_exceeds_capacity() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 7, "ration", 4, None, None)
            .expect("ration slot inserts");

        struct CapacityTen;

        impl CapacityPolicy for CapacityTen {
            fn item_volume(
                &self,
                _item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(2)
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(10))
            }
        }

        world
            .validate_incoming("player", 7, "ration", 1, &CapacityTen)
            .expect("one more ration fits");
        let rejected = world.validate_incoming("player", 7, "ration", 2, &CapacityTen);

        match rejected {
            Err(CapacityError::InsufficientCapacity {
                used,
                requested,
                capacity,
            }) => {
                assert_eq!(used, 8);
                assert_eq!(requested, 4);
                assert_eq!(capacity, 10);
            }
            other => panic!("expected insufficient capacity, got {other:?}"),
        }
    }
}
