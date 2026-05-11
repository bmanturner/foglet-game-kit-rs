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

use crate::inventory::{InventoryError, InventorySlot};
use crate::world_db::WorldDb;

/// Game-supplied capacity policy used by inventory helpers.
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

/// Errors produced by capacity validation and transfers.
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
    /// The underlying inventory primitive failed.
    #[error("inventory capacity operation failed: {source}")]
    InventoryError {
        /// Wrapped inventory error.
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
    /// exists, because inventory treats incoming quantity as additive
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

    /// Transfer inventory while validating destination capacity in the
    /// same transaction as the debit and credit.
    pub fn transfer_with_capacity<F>(
        &mut self,
        source: (&str, i64),
        destination: (&str, i64),
        item_key: &str,
        quantity: i64,
        policy: &impl CapacityPolicy,
        on_commit: Option<F>,
    ) -> Result<(InventorySlot, InventorySlot), CapacityError>
    where
        F: FnOnce(
            &rusqlite::Transaction<'_>,
            &InventorySlot,
            &InventorySlot,
        ) -> Result<(), rusqlite::Error>,
    {
        if quantity <= 0 {
            return Err(InventoryError::InvalidTransferQuantity { quantity }.into());
        }

        let (source_owner_kind, source_owner_id) = source;
        let (destination_owner_kind, destination_owner_id) = destination;
        let transition = transfer_transition(
            source_owner_kind,
            source_owner_id,
            destination_owner_kind,
            destination_owner_id,
        );
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|source| InventoryError::TransferFailed {
                transition: transition.clone(),
                item_key: item_key.to_string(),
                source,
            })?;

        let source_slot =
            match find_slot_for_owner(&tx, source_owner_kind, source_owner_id, item_key).map_err(
                |source| InventoryError::TransferFailed {
                    transition: transition.clone(),
                    item_key: item_key.to_string(),
                    source,
                },
            )? {
                Some(slot) => slot,
                None => {
                    return Err(InventoryError::MissingSourceSlot {
                        owner_kind: source_owner_kind.to_string(),
                        owner_id: source_owner_id,
                        item_key: item_key.to_string(),
                    }
                    .into());
                }
            };

        if source_slot.quantity < quantity {
            return Err(InventoryError::InsufficientStock {
                owner_kind: source_owner_kind.to_string(),
                owner_id: source_owner_id,
                item_key: item_key.to_string(),
                requested: quantity,
                available: source_slot.quantity,
            }
            .into());
        }

        validate_incoming_in_tx(
            &tx,
            destination_owner_kind,
            destination_owner_id,
            item_key,
            quantity,
            policy,
        )?;

        tx.execute(
            "UPDATE inventory_slots\nSET quantity = quantity - ?2,\nupdated_at = CURRENT_TIMESTAMP\nWHERE id = ?1",
            rusqlite::params![source_slot.id, quantity],
        )
        .and_then(|updated| {
            if updated == 0 {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(updated)
            }
        })
        .map_err(|source| InventoryError::TransferFailed {
            transition: transition.clone(),
            item_key: item_key.to_string(),
            source,
        })?;

        let destination_slot =
            match find_slot_for_owner(&tx, destination_owner_kind, destination_owner_id, item_key)
                .map_err(|source| InventoryError::TransferFailed {
                    transition: transition.clone(),
                    item_key: item_key.to_string(),
                    source,
                })? {
                Some(slot) => {
                    tx.execute(
                        "UPDATE inventory_slots\nSET quantity = quantity + ?2,\nupdated_at = CURRENT_TIMESTAMP\nWHERE id = ?1",
                        rusqlite::params![slot.id, quantity],
                    )
                    .and_then(|updated| {
                        if updated == 0 {
                            Err(rusqlite::Error::QueryReturnedNoRows)
                        } else {
                            Ok(updated)
                        }
                    })
                    .map_err(|source| InventoryError::TransferFailed {
                        transition: transition.clone(),
                        item_key: item_key.to_string(),
                        source,
                    })?;
                    slot
                }
                None => tx
                    .query_row(
                        "INSERT INTO inventory_slots (owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json)\n\
                         VALUES (?1, ?2, ?3, ?4, NULL, NULL)\n\
                         RETURNING id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json",
                        rusqlite::params![
                            destination_owner_kind,
                            destination_owner_id,
                            item_key,
                            quantity,
                        ],
                        row_to_inventory_slot,
                    )
                    .map_err(|source| InventoryError::TransferFailed {
                        transition: transition.clone(),
                        item_key: item_key.to_string(),
                        source,
                    })?,
            };

        let post_source = load_slot_by_id(&tx, source_slot.id).map_err(|source| {
            InventoryError::TransferFailed {
                transition: transition.clone(),
                item_key: item_key.to_string(),
                source,
            }
        })?;
        let post_destination = load_slot_by_id(&tx, destination_slot.id).map_err(|source| {
            InventoryError::TransferFailed {
                transition: transition.clone(),
                item_key: item_key.to_string(),
                source,
            }
        })?;

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &post_source, &post_destination).map_err(|source| {
                InventoryError::TransferRejected {
                    transition: transition.clone(),
                    item_key: item_key.to_string(),
                    source,
                }
            })?;
        }

        tx.commit()
            .map_err(|source| InventoryError::TransferFailed {
                transition,
                item_key: item_key.to_string(),
                source,
            })?;

        Ok((post_source, post_destination))
    }
}

fn validate_incoming_in_tx(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    owner_id: i64,
    item_key: &str,
    quantity: i64,
    policy: &impl CapacityPolicy,
) -> Result<(), CapacityError> {
    let Some(capacity) = policy.owner_capacity(owner_kind, owner_id)? else {
        return Ok(());
    };
    let used = used_capacity_in_tx(conn, owner_kind, owner_id, policy)?;
    let metadata = find_slot_for_owner(conn, owner_kind, owner_id, item_key)
        .map_err(|source| CapacityError::InventoryError {
            source: Box::new(InventoryError::GetFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                item_key: item_key.to_string(),
                source,
            }),
        })?
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

fn used_capacity_in_tx(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    owner_id: i64,
    policy: &impl CapacityPolicy,
) -> Result<i64, CapacityError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
             FROM inventory_slots\n\
             WHERE owner_kind = ?1 AND owner_id = ?2\n\
             ORDER BY item_key ASC, id ASC",
        )
        .map_err(|source| CapacityError::InventoryError {
            source: Box::new(InventoryError::ListFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                source,
            }),
        })?;
    let slots = stmt
        .query_map(
            rusqlite::params![owner_kind, owner_id],
            row_to_inventory_slot,
        )
        .map_err(|source| CapacityError::InventoryError {
            source: Box::new(InventoryError::ListFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                source,
            }),
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| CapacityError::InventoryError {
            source: Box::new(InventoryError::ListFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                source,
            }),
        })?;

    let mut used = 0;
    for slot in slots {
        let metadata = slot
            .metadata_json
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()
            .map_err(|source| CapacityError::PolicyError(source.to_string()))?
            .unwrap_or(Value::Null);
        used += policy.item_volume(&slot.item_key, &metadata)? * slot.quantity;
    }
    Ok(used)
}

fn find_slot_for_owner(
    conn: &rusqlite::Connection,
    owner_kind: &str,
    owner_id: i64,
    item_key: &str,
) -> rusqlite::Result<Option<InventorySlot>> {
    const SQL: &str = "\
SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
FROM inventory_slots\n\
WHERE owner_kind = ?1 AND owner_id = ?2 AND item_key = ?3\n\
ORDER BY id ASC\n\
LIMIT 1";

    match conn.query_row(
        SQL,
        rusqlite::params![owner_kind, owner_id, item_key],
        row_to_inventory_slot,
    ) {
        Ok(slot) => Ok(Some(slot)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(source) => Err(source),
    }
}

fn load_slot_by_id(conn: &rusqlite::Connection, slot_id: i64) -> rusqlite::Result<InventorySlot> {
    conn.query_row(
        "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
         FROM inventory_slots\n\
         WHERE id = ?1",
        rusqlite::params![slot_id],
        row_to_inventory_slot,
    )
}

fn row_to_inventory_slot(row: &rusqlite::Row<'_>) -> rusqlite::Result<InventorySlot> {
    Ok(InventorySlot {
        id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_id: row.get(2)?,
        item_key: row.get(3)?,
        quantity: row.get(4)?,
        equilibrium: row.get(5)?,
        metadata_json: row.get(6)?,
    })
}

fn transfer_transition(
    source_owner_kind: &str,
    source_owner_id: i64,
    destination_owner_kind: &str,
    destination_owner_id: i64,
) -> String {
    format!(
        "{source_owner_kind}:{source_owner_id} -> {destination_owner_kind}:{destination_owner_id}"
    )
}

#[cfg(test)]
mod tests {
    use super::{CapacityError, CapacityPolicy};
    use crate::inventory::INVENTORY_SLOTS_MIGRATION;
    use crate::world_db::WorldDb;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;
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

    #[test]
    fn transfer_with_capacity_debits_source_and_credits_destination() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 1, "ration", 5, None, None)
            .expect("source slot inserts");
        world
            .create_slot("chest", 2, "ration", 1, None, None)
            .expect("destination slot inserts");

        struct CapacityTen;

        impl CapacityPolicy for CapacityTen {
            fn item_volume(
                &self,
                _item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(1)
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(10))
            }
        }

        let (source, destination) = world
            .transfer_with_capacity(
                ("player", 1),
                ("chest", 2),
                "ration",
                3,
                &CapacityTen,
                Option::<
                    fn(
                        &rusqlite::Transaction<'_>,
                        &crate::inventory::InventorySlot,
                        &crate::inventory::InventorySlot,
                    ) -> Result<(), rusqlite::Error>,
                >::None,
            )
            .expect("capacity transfer succeeds");

        assert_eq!(source.quantity, 2);
        assert_eq!(destination.quantity, 4);
        assert_eq!(
            world
                .get_slot("player", 1, "ration")
                .expect("source reads")
                .expect("source exists")
                .quantity,
            2
        );
        assert_eq!(
            world
                .get_slot("chest", 2, "ration")
                .expect("destination reads")
                .expect("destination exists")
                .quantity,
            4
        );
    }

    #[test]
    fn transfer_with_capacity_rolls_back_on_capacity_overflow() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 1, "ration", 5, None, None)
            .expect("source slot inserts");
        world
            .create_slot("chest", 2, "ration", 8, None, None)
            .expect("destination slot inserts");

        struct CapacityTen;

        impl CapacityPolicy for CapacityTen {
            fn item_volume(
                &self,
                _item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(1)
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(10))
            }
        }

        let rejected = world.transfer_with_capacity(
            ("player", 1),
            ("chest", 2),
            "ration",
            3,
            &CapacityTen,
            Option::<
                fn(
                    &rusqlite::Transaction<'_>,
                    &crate::inventory::InventorySlot,
                    &crate::inventory::InventorySlot,
                ) -> Result<(), rusqlite::Error>,
            >::None,
        );

        match rejected {
            Err(CapacityError::InsufficientCapacity {
                used,
                requested,
                capacity,
            }) => {
                assert_eq!(used, 8);
                assert_eq!(requested, 3);
                assert_eq!(capacity, 10);
            }
            other => panic!("expected capacity rejection, got {other:?}"),
        }
        assert_eq!(
            world
                .get_slot("player", 1, "ration")
                .expect("source reads")
                .expect("source exists")
                .quantity,
            5
        );
        assert_eq!(
            world
                .get_slot("chest", 2, "ration")
                .expect("destination reads")
                .expect("destination exists")
                .quantity,
            8
        );
    }

    #[test]
    fn transfer_with_capacity_allows_uncapped_destination() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 1, "anvil", 5, None, None)
            .expect("source slot inserts");

        struct UncappedPolicy;

        impl CapacityPolicy for UncappedPolicy {
            fn item_volume(
                &self,
                _item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(10_000)
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(None)
            }
        }

        let (source, destination) = world
            .transfer_with_capacity(
                ("player", 1),
                ("warehouse", 2),
                "anvil",
                5,
                &UncappedPolicy,
                Option::<
                    fn(
                        &rusqlite::Transaction<'_>,
                        &crate::inventory::InventorySlot,
                        &crate::inventory::InventorySlot,
                    ) -> Result<(), rusqlite::Error>,
                >::None,
            )
            .expect("uncapped transfer succeeds");

        assert_eq!(source.quantity, 0);
        assert_eq!(destination.quantity, 5);
    }

    #[test]
    fn transfer_with_capacity_propagates_policy_error_and_rolls_back() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 1, "ration", 5, None, None)
            .expect("source slot inserts");
        world
            .create_slot("chest", 2, "ration", 1, None, None)
            .expect("destination slot inserts");

        struct BrokenPolicy;

        impl CapacityPolicy for BrokenPolicy {
            fn item_volume(
                &self,
                item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Err(CapacityError::PolicyError(format!(
                    "cannot size {item_key}"
                )))
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(10))
            }
        }

        let rejected = world.transfer_with_capacity(
            ("player", 1),
            ("chest", 2),
            "ration",
            3,
            &BrokenPolicy,
            Option::<
                fn(
                    &rusqlite::Transaction<'_>,
                    &crate::inventory::InventorySlot,
                    &crate::inventory::InventorySlot,
                ) -> Result<(), rusqlite::Error>,
            >::None,
        );

        match rejected {
            Err(CapacityError::PolicyError(message)) => {
                assert_eq!(message, "cannot size ration");
            }
            other => panic!("expected policy error, got {other:?}"),
        }
        assert_eq!(
            world
                .get_slot("player", 1, "ration")
                .expect("source reads")
                .expect("source exists")
                .quantity,
            5
        );
        assert_eq!(
            world
                .get_slot("chest", 2, "ration")
                .expect("destination reads")
                .expect("destination exists")
                .quantity,
            1
        );
    }

    #[test]
    fn concurrent_capacity_transfers_cannot_collectively_overflow() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .create_slot("player", 1, "gem", 5, None, None)
            .expect("first source inserts");
        world
            .create_slot("player", 2, "gem", 5, None, None)
            .expect("second source inserts");
        drop(world);

        #[derive(Clone, Copy)]
        struct CapFive;

        impl CapacityPolicy for CapFive {
            fn item_volume(
                &self,
                _item_key: &str,
                _metadata: &serde_json::Value,
            ) -> Result<i64, CapacityError> {
                Ok(1)
            }

            fn owner_capacity(
                &self,
                _owner_kind: &str,
                _owner_id: i64,
            ) -> Result<Option<i64>, CapacityError> {
                Ok(Some(5))
            }
        }

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for player_id in [1, 2] {
            let db_path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let mut world = WorldDb::open(&db_path).expect("thread world opens");
                barrier.wait();
                world.transfer_with_capacity(
                    ("player", player_id),
                    ("chest", 99),
                    "gem",
                    5,
                    &CapFive,
                    Option::<
                        fn(
                            &rusqlite::Transaction<'_>,
                            &crate::inventory::InventorySlot,
                            &crate::inventory::InventorySlot,
                        ) -> Result<(), rusqlite::Error>,
                    >::None,
                )
            }));
        }

        let outcomes = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread joins"))
            .collect::<Vec<_>>();
        let success_count = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(
            success_count, 1,
            "exactly one full-capacity transfer can commit"
        );

        let world = WorldDb::open(&db_path).expect("world reopens");
        let destination = world
            .get_slot("chest", 99, "gem")
            .expect("destination reads")
            .expect("one destination slot exists");
        assert_eq!(destination.quantity, 5);
        assert!(
            outcomes
                .iter()
                .any(|outcome| matches!(outcome, Err(CapacityError::InsufficientCapacity { .. }))),
            "the losing transfer should re-check capacity after the winner commits"
        );
    }
}
