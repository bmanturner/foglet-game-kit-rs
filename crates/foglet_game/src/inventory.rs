//! `inventory` — owner-keyed stockpile schema (SPEC_v4 Task 7a).
//!
//! This module owns the durable table used to persist durable quantities for
//! arbitrary owners and item keys. It intentionally stays neutral:
//!
//! - In a **space exploration** game, a location (ship hold, cargo bay, or
//!   orbital depot) can own the same item key with different metadata.
//! - In a **dungeon crawler**, a container, room, or player can own
//!   the same key under distinct slot metadata.
//!
//! The contract in this task is migration-only: it declares the durable
//! schema so later movement/transfer tasks can rely on a stable shape.
//!
//! All inventory operations are intentionally owner-agnostic. `owner_kind`
//! is a caller-defined bucket (`"ship"`, `"player"`, `"chest"`, etc.),
//! and `owner_id` is that bucket's numeric identity.

use thiserror::Error;

use crate::world_db::WorldDb;
use crate::world_db::WorldMigration;

/// One mutable inventory slot in the shared world DB.
///
/// The shape is deliberately generic and does not encode stack limits,
/// unit conversions, or ownership semantics. Games model those rules in
/// callbacks and game logic.
///
/// # Genre-neutral usage
///
/// - In a **space exploration** game, a `"cargo-bay"` owner can hold
///   `"ore-container"` with metadata describing cargo inspection
///   state.
/// - In a **dungeon crawler** game, a `"chest"` owner can hold
///   `"health-potion"` while a `"character"` owner holds the same item
///   key for carried inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventorySlot {
    /// Stable numeric row id from SQLite.
    pub id: i64,
    /// Game-defined owner bucket label.
    pub owner_kind: String,
    /// Game-defined owner row id for `owner_kind`.
    pub owner_id: i64,
    /// Stable key for this resource in the game domain.
    pub item_key: String,
    /// Current stored quantity, non-negative by schema constraint.
    pub quantity: i64,
    /// Optional game-advisory target balance.
    pub equilibrium: Option<i64>,
    /// Optional opaque JSON metadata.
    pub metadata_json: Option<String>,
}

/// Errors for inventory-slot read/write APIs.
///
/// Distinct variants keep task-level failures and SQL failures easy to
/// branch on while preserving root causes in structured logs.
#[derive(Debug, Error)]
pub enum InventoryError {
    /// Merge attempt was rejected because the two slots are not identity-compatible.
    #[error("cannot merge inventory slots `{slot_a_id}` and `{slot_b_id}`: {reason}")]
    MergeNotCompatible {
        /// Source slot id that could not be merged.
        slot_a_id: i64,
        /// Destination slot id that could not be merged.
        slot_b_id: i64,
        /// Human-readable reason for why merge preconditions failed.
        reason: &'static str,
    },
    /// SQLite failed while merging two inventory slot rows.
    #[error("failed to merge inventory slot `{slot_a_id}` into `{slot_b_id}`: {source}")]
    MergeFailed {
        /// Source slot id in the merge attempt.
        slot_a_id: i64,
        /// Destination slot id in the merge attempt.
        slot_b_id: i64,
        /// Underlying database error.
        #[source]
        source: rusqlite::Error,
    },
    /// SQL failed while moving stock between two slots.
    #[error("failed to transfer `{item_key}` for transition `{transition}`: {source}")]
    TransferFailed {
        /// Source→destination owner tuple summary.
        transition: String,
        /// Item key that was being moved.
        item_key: String,
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
    /// The source owner has no matching slot for this transfer key.
    #[error("missing source inventory slot for `{owner_kind}:{owner_id}` and item `{item_key}`")]
    MissingSourceSlot {
        /// Owner bucket that was missing an item row.
        owner_kind: String,
        /// Owner id that was missing an item row.
        owner_id: i64,
        /// Item key that should be moved.
        item_key: String,
    },
    /// The source stock is insufficient to cover the requested transfer.
    #[error(
        "insufficient stock for `{owner_kind}:{owner_id}` item `{item_key}`: requested {requested}, available {available}"
    )]
    InsufficientStock {
        /// Owner bucket that did not have enough stock.
        owner_kind: String,
        /// Owner id that did not have enough stock.
        owner_id: i64,
        /// Item key being moved.
        item_key: String,
        /// Number of units requested.
        requested: i64,
        /// Number of units currently stored.
        available: i64,
    },
    /// The transfer callback rejected the mutation.
    #[error(
        "transfer callback rejected movement of `{item_key}` for transition `{transition}`: {source}"
    )]
    TransferRejected {
        /// Source→destination owner tuple summary.
        transition: String,
        /// Item key that was being moved.
        item_key: String,
        /// Underlying callback error.
        #[source]
        source: rusqlite::Error,
    },
    /// Transfer quantity must be strictly positive.
    #[error("transfer quantity must be greater than zero; got {quantity}")]
    InvalidTransferQuantity {
        /// Requested transfer quantity.
        quantity: i64,
    },
    /// Slot write failed.
    #[error("failed to create inventory slot for owner `{owner_kind}:{owner_id}`, item `{item_key}`: {source}")]
    CreateFailed {
        /// Owner bucket for diagnostics.
        owner_kind: String,
        /// Owner row id for diagnostics.
        owner_id: i64,
        /// Item key for diagnostics.
        item_key: String,
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
    /// Slot lookup failed.
    #[error("failed to get inventory slot for owner `{owner_kind}:{owner_id}`, item `{item_key}`: {source}")]
    GetFailed {
        /// Owner bucket for diagnostics.
        owner_kind: String,
        /// Owner row id for diagnostics.
        owner_id: i64,
        /// Item key for diagnostics.
        item_key: String,
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
    /// A query failed while listing slots for one owner.
    #[error("failed to list slots for owner `{owner_kind}:{owner_id}`: {source}")]
    ListFailed {
        /// Owner bucket for diagnostics.
        owner_kind: String,
        /// Owner row id for diagnostics.
        owner_id: i64,
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
}

/// Migration for the `inventory_slots` table (SPEC_v4 Task 7a).
///
/// The migration uses integer row identity plus a compound owner model:
///
/// - `(owner_kind, owner_id)` is game-defined ownership metadata.
/// - `item_key` is a stable game key, not interpreted by the kit.
/// - `quantity` is stored as a non-negative integer.
/// - `equilibrium` is optional advisory target stock.
/// - `metadata_json` is opaque game-authored JSON payload.
/// - `created_at` / `updated_at` are SQLite timestamps.
///
/// This migration is intentionally minimal and game-agnostic:
///
/// - A **space exploration** game might map `owner_kind = "station"` for
///   warehouse inventory, while `item_key` models cargo pallets.
/// - A **dungeon crawler** might map `owner_kind = "chest"` for a room
///   container and keep encounter clues in `metadata_json`.
pub const INVENTORY_SLOTS_MIGRATION: WorldMigration = WorldMigration {
    version: 14,
    name: "create_inventory_slots",
    sql: "\
CREATE TABLE IF NOT EXISTS inventory_slots (\n\
    id             INTEGER PRIMARY KEY,\n\
    owner_kind     TEXT NOT NULL,\n\
    owner_id       INTEGER NOT NULL,\n\
    item_key       TEXT NOT NULL,\n\
    quantity       INTEGER NOT NULL DEFAULT 0 CHECK (quantity >= 0),\n\
    equilibrium    INTEGER,\n\
    metadata_json  TEXT,\n\
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    updated_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n\
);\n\
",
};

impl WorldDb {
    /// Create one inventory slot and return the committed row.
    ///
    /// This is the starting point for v4 stockpile state. The API is
    /// intentionally small:
    ///
    /// - It persists one owner/item pair.
    /// - It returns the committed values for immediate assertions.
    /// - It carries typed SQLite failure details for callers that must
    ///   branch on save-path outcomes.
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space exploration** game, add `"ore"` cargo to a
    ///   `"ship"` owner with equilibrium guidance for UI bars.
    /// - In a **dungeon crawler** game, add `"silver-key"` to a
    ///   `"player"` owner before presenting it in UI inventory.
    pub fn create_slot(
        &self,
        owner_kind: &str,
        owner_id: i64,
        item_key: &str,
        quantity: i64,
        equilibrium: Option<i64>,
        metadata_json: Option<&str>,
    ) -> Result<InventorySlot, InventoryError> {
        const SQL: &str = "\
INSERT INTO inventory_slots (owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json)\n\
VALUES (?1, ?2, ?3, ?4, ?5, ?6)\n\
RETURNING id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    owner_kind,
                    owner_id,
                    item_key,
                    quantity,
                    equilibrium,
                    metadata_json
                ],
                row_to_inventory_slot,
            )
            .map_err(|source| InventoryError::CreateFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                item_key: item_key.to_string(),
                source,
            })
    }

    /// Read one slot for owner/item.
    ///
    /// The method returns `Ok(None)` when the owner/item pair has no
    /// persisted row. When multiple physical rows match due to caller-level
    /// metadata differences, it deterministically returns the lowest
    /// `id` to keep behavior stable until metadata-aware merge helpers
    /// land in later tasks.
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space exploration** game, call this for a station's
    ///   cargo table to read the currently known "cargo-fuel" bucket.
    /// - In a **dungeon crawler** game, call this for a container's
    ///   `"healing-potion"` store before allowing transfer into a player
    ///   slot.
    pub fn get_slot(
        &self,
        owner_kind: &str,
        owner_id: i64,
        item_key: &str,
    ) -> Result<Option<InventorySlot>, InventoryError> {
        const SQL: &str = "\
SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
FROM inventory_slots\n\
WHERE owner_kind = ?1 AND owner_id = ?2 AND item_key = ?3\n\
ORDER BY id ASC\n\
LIMIT 1";

        match self.connection().query_row(
            SQL,
            rusqlite::params![owner_kind, owner_id, item_key],
            row_to_inventory_slot,
        ) {
            Ok(slot) => Ok(Some(slot)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(InventoryError::GetFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                item_key: item_key.to_string(),
                source,
            }),
        }
    }

    /// List all inventory slots for one owner in deterministic order.
    ///
    /// The ordering is stable and replay-safe:
    ///
    /// - primary sort: `item_key` ascending, so callers can render slots
    ///   in stable lexical order across restarts;
    /// - secondary sort: `id` ascending, which disambiguates duplicate keys
    ///   (for example, distinct metadata rows for the same owner/item) without
    ///   imposing any merge policy.
    ///
    /// Why this API exists:
    ///
    /// - In a **space exploration** game, a `"station"` owner can own a
    ///   mixed manifest of cargo and components across many bays, and the
    ///   rendering layer wants a deterministic list every refresh.
    /// - In a **dungeon crawler** game, a `"chest"` owner can expose
    ///   multiple stack rows for the same item in an intentionally
    ///   metadata-rich layout, while still presenting them in a reproducible
    ///   order.
    pub fn slots_for_owner(
        &self,
        owner_kind: &str,
        owner_id: i64,
    ) -> Result<Vec<InventorySlot>, InventoryError> {
        const SQL: &str = "\
SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
FROM inventory_slots\n\
WHERE owner_kind = ?1 AND owner_id = ?2\n\
ORDER BY item_key ASC, id ASC";

        let mut statement =
            self.connection()
                .prepare(SQL)
                .map_err(|source| InventoryError::ListFailed {
                    owner_kind: owner_kind.to_string(),
                    owner_id,
                    source,
                })?;

        let rows = statement
            .query_map(
                rusqlite::params![owner_kind, owner_id],
                row_to_inventory_slot,
            )
            .map_err(|source| InventoryError::ListFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                source,
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| InventoryError::ListFailed {
                owner_kind: owner_kind.to_string(),
                owner_id,
                source,
            })?;

        Ok(rows)
    }

    /// Merge one slot into another when metadata and ownership fully match.
    ///
    /// The helper does not invent merge policy; it applies the strict
    /// v4 contract:
    ///
    /// - `(owner_kind, owner_id, item_key, metadata_json)` must match
    ///   exactly across both slots.
    /// - The two row ids must be distinct.
    /// - Quantities are added in a single transaction.
    ///
    /// This API exists because v4 intentionally keeps stock behavior
    /// game-agnostic while still allowing explicit consolidation in
    /// shared logic (for example, combining partial loads in a ship cargo
    /// hold or combining duplicate stack rows in a dungeon chest without
    /// creating a game-specific stack policy inside the kit).
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space exploration** game, two `"ore"` entries recorded for
    ///   the same `"cargo-bay"` with identical scan metadata can be merged
    ///   into a single row for clean ship manifests.
    /// - In a **dungeon crawler** game, two `"health-potion"` rows under
    ///   a `"chest"` owner with identical loot metadata can be merged before
    ///   presenting room contents in UI.
    pub fn merge_slots(
        &mut self,
        destination: &InventorySlot,
        source: &InventorySlot,
    ) -> Result<InventorySlot, InventoryError> {
        if destination.id == source.id {
            return Err(InventoryError::MergeNotCompatible {
                slot_a_id: destination.id,
                slot_b_id: source.id,
                reason: "cannot merge a slot with itself",
            });
        }

        if destination.owner_kind != source.owner_kind
            || destination.owner_id != source.owner_id
            || destination.item_key != source.item_key
            || destination.metadata_json != source.metadata_json
        {
            return Err(InventoryError::MergeNotCompatible {
                slot_a_id: destination.id,
                slot_b_id: source.id,
                reason: "slots must match owner, item key, and metadata_json",
            });
        }

        let tx = self.connection_mut().transaction().map_err(|source_err| {
            InventoryError::MergeFailed {
                slot_a_id: destination.id,
                slot_b_id: source.id,
                source: source_err,
            }
        })?;

        tx.execute(
            "UPDATE inventory_slots\nSET quantity = quantity + (SELECT quantity FROM inventory_slots WHERE id = ?2),\nupdated_at = CURRENT_TIMESTAMP\nWHERE id = ?1",
            rusqlite::params![destination.id, source.id],
        )
        .and_then(|updated| {
            if updated == 0 {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(updated)
            }
        })
        .map_err(|source_err| InventoryError::MergeFailed {
            slot_a_id: destination.id,
            slot_b_id: source.id,
            source: source_err,
        })?;

        tx.execute(
            "DELETE FROM inventory_slots WHERE id = ?1",
            rusqlite::params![source.id],
        )
        .and_then(|deleted| {
            if deleted == 0 {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(deleted)
            }
        })
        .map_err(|source_err| InventoryError::MergeFailed {
            slot_a_id: destination.id,
            slot_b_id: source.id,
            source: source_err,
        })?;

        let merged = tx
            .query_row(
                "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
                 FROM inventory_slots\n\
                 WHERE id = ?1",
                rusqlite::params![destination.id],
                row_to_inventory_slot,
            )
            .map_err(|source_err| InventoryError::MergeFailed {
                slot_a_id: destination.id,
                slot_b_id: source.id,
                source: source_err,
            })?;

        tx.commit()
            .map_err(|source_err| InventoryError::MergeFailed {
                slot_a_id: destination.id,
                slot_b_id: source.id,
                source: source_err,
            })?;

        Ok(merged)
    }

    /// Transfer one item key atomically between two owners.
    ///
    /// The helper performs all writes inside one SQLite transaction so
    /// callers can rely on all-or-nothing semantics for:
    ///
    /// - debiting the source row,
    /// - crediting the destination row (or creating one if missing),
    /// - then invoking the optional `on_commit` callback.
    ///
    /// Why this shape exists:
    ///
    /// - In a **space exploration** game, credits can move between
    ///   a `"ship"` and `"station"` owner without partial writes even
    ///   if a docking callback rejects due to lockout.
    /// - In a **dungeon crawler** game, chest loot transfers from
    ///   `"player"` to `"chest"` stay atomic when a trap callback
    ///   wants to veto the move after the SQL write phase.
    ///
    /// Callback contract:
    ///
    /// - `on_commit` (when provided) runs after SQL mutations and
    ///   receives the post-mutation snapshots for both source and
    ///   destination rows.
    /// - If `on_commit` returns an error, the whole transfer rolls back,
    ///   including both quantity mutations.
    pub fn transfer<F>(
        &mut self,
        source: (&str, i64),
        destination: (&str, i64),
        item_key: &str,
        quantity: i64,
        on_commit: Option<F>,
    ) -> Result<(InventorySlot, InventorySlot), InventoryError>
    where
        F: FnOnce(
            &rusqlite::Transaction<'_>,
            &InventorySlot,
            &InventorySlot,
        ) -> Result<(), rusqlite::Error>,
    {
        if quantity <= 0 {
            return Err(InventoryError::InvalidTransferQuantity { quantity });
        }

        let (source_owner_kind, source_owner_id) = source;
        let (destination_owner_kind, destination_owner_id) = destination;
        let transition = transfer_transition(
            source_owner_kind,
            source_owner_id,
            destination_owner_kind,
            destination_owner_id,
        );

        let tx = self.connection_mut().transaction().map_err(|source| {
            InventoryError::TransferFailed {
                transition: transition.clone(),
                item_key: item_key.to_string(),
                source,
            }
        })?;

        let source_slot =
            match find_slot_for_owner_in_tx(&tx, source_owner_kind, source_owner_id, item_key)
                .map_err(|source| InventoryError::TransferFailed {
                    transition: transition.clone(),
                    item_key: item_key.to_string(),
                    source,
                })? {
                Some(slot) => slot,
                None => {
                    return Err(InventoryError::MissingSourceSlot {
                        owner_kind: source_owner_kind.to_string(),
                        owner_id: source_owner_id,
                        item_key: item_key.to_string(),
                    });
                }
            };

        if source_slot.quantity < quantity {
            return Err(InventoryError::InsufficientStock {
                owner_kind: source_owner_kind.to_string(),
                owner_id: source_owner_id,
                item_key: item_key.to_string(),
                requested: quantity,
                available: source_slot.quantity,
            });
        }

        let destination_slot = match find_slot_for_owner_in_tx(
            &tx,
            destination_owner_kind,
            destination_owner_id,
            item_key,
        )
        .map_err(|source| InventoryError::TransferFailed {
            transition: transition.clone(),
            item_key: item_key.to_string(),
            source,
        })? {
            Some(slot) => slot,
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

        if source_slot.id != destination_slot.id {
            tx.execute(
                "UPDATE inventory_slots\nSET quantity = quantity + ?2,\nupdated_at = CURRENT_TIMESTAMP\nWHERE id = ?1",
                rusqlite::params![destination_slot.id, quantity],
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
        }

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
                transition: transition.clone(),
                item_key: item_key.to_string(),
                source,
            })?;

        Ok((post_source, post_destination))
    }
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

fn find_slot_for_owner_in_tx(
    connection: &rusqlite::Transaction<'_>,
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

    match connection.query_row(
        SQL,
        rusqlite::params![owner_kind, owner_id, item_key],
        row_to_inventory_slot,
    ) {
        Ok(slot) => Ok(Some(slot)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(source) => Err(source),
    }
}

fn load_slot_by_id(
    connection: &rusqlite::Transaction<'_>,
    slot_id: i64,
) -> rusqlite::Result<InventorySlot> {
    connection.query_row(
        "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
         FROM inventory_slots\n\
         WHERE id = ?1",
        rusqlite::params![slot_id],
        row_to_inventory_slot,
    )
}

#[cfg(test)]
mod tests {
    use super::{row_to_inventory_slot, InventoryError, InventorySlot, INVENTORY_SLOTS_MIGRATION};
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn applies_inventory_slots_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('inventory_slots') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "id".to_string(),
                "owner_kind".to_string(),
                "owner_id".to_string(),
                "item_key".to_string(),
                "quantity".to_string(),
                "equilibrium".to_string(),
                "metadata_json".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
            ],
            "inventory_slots schema must match SPEC_v4 Task 7a exactly"
        );

        let migration_version: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                params![INVENTORY_SLOTS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("world_migrations row exists");
        assert_eq!(
            migration_version, INVENTORY_SLOTS_MIGRATION.version,
            "migration should record the same version as the source constant"
        );

        let sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'inventory_slots'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master contains the create statement");
        assert!(
            sql.contains("CHECK (quantity >= 0)"),
            "quantity should be guarded at schema level"
        );
    }

    #[test]
    fn inventory_slots_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("first inventory_slots migration applies");
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("second inventory_slots migration applies");

        let recorded_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE name = ?1",
                params![INVENTORY_SLOTS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(recorded_count, 1, "migration record is idempotent");
    }

    /// Task 7b requires that slot creation and lookup round-trip
    /// matching values through the crate API.
    #[test]
    fn create_slot_and_get_slot_round_trip() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let created = world.create_slot(
            "ship",
            42,
            "oxygen-canister",
            7,
            Some(10),
            Some(r#"{"bay":"cargo","seal":"intact"}"#),
        )?;
        let loaded = world
            .get_slot("ship", 42, "oxygen-canister")?
            .expect("slot should exist after create");

        assert_eq!(created.id, loaded.id);
        assert_eq!(loaded.owner_kind, "ship".to_string());
        assert_eq!(loaded.owner_id, 42);
        assert_eq!(loaded.item_key, "oxygen-canister".to_string());
        assert_eq!(loaded.quantity, 7);
        assert_eq!(loaded.equilibrium, Some(10));
        assert_eq!(
            loaded.metadata_json,
            Some(r#"{"bay":"cargo","seal":"intact"}"#.to_string())
        );
        assert_eq!(created, loaded);

        let missing = world.get_slot("chest", 3, "iron-sword")?;
        assert!(missing.is_none(), "unknown owner/item should return None");

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM inventory_slots WHERE owner_kind = ?1 AND owner_id = ?2 AND item_key = ?3",
                params!["ship", 42, "oxygen-canister"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(row_count, 1);

        Ok(())
    }

    #[test]
    fn direct_negative_quantity_insert_is_rejected() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let direct_insert = world.connection().execute(
            "INSERT INTO inventory_slots (owner_kind, owner_id, item_key, quantity)\n\
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["station", 8, "fuel-cell", -2],
        );

        assert!(
            direct_insert.is_err(),
            "negative quantity must be rejected by the inventory constraint"
        );
    }

    #[test]
    fn equilibrium_is_advisory_only() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let created = world.create_slot(
            "market-stand",
            7,
            "plasma-core",
            4,
            Some(12),
            Some(r#"{"condition":"sealed"}"#),
        )?;
        assert_eq!(created.quantity, 4);
        assert_eq!(created.equilibrium, Some(12));
        assert_eq!(
            created.metadata_json,
            Some(r#"{"condition":"sealed"}"#.to_string())
        );

        let loaded_before = world
            .get_slot("market-stand", 7, "plasma-core")?
            .expect("slot exists");
        assert_eq!(loaded_before.quantity, 4);
        assert_eq!(loaded_before.equilibrium, Some(12));

        world
            .connection()
            .execute(
                "UPDATE inventory_slots SET equilibrium = ?1 WHERE id = ?2",
                rusqlite::params![88, created.id],
            )
            .expect("equilibrium can be updated independently");

        let loaded_after = world
            .get_slot("market-stand", 7, "plasma-core")?
            .expect("slot exists after equilibrium update");
        assert_eq!(
            loaded_after.equilibrium,
            Some(88),
            "advisory target can be moved independently"
        );
        assert_eq!(
            loaded_after.quantity, 4,
            "kit must not auto-drift quantity toward equilibrium"
        );

        Ok(())
    }

    /// Task 7e requires a deterministic listing for one owner.
    ///
    /// The test verifies:
    ///
    /// - filtering by exact owner tuple;
    /// - stable lexical ordering by `item_key`;
    /// - stable secondary ordering by `id` when keys tie.
    #[test]
    fn slots_for_owner_is_deterministic() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let zeta = world.create_slot("ship", 7, "zeta-parts", 12, None, None)?;
        let alpha_first =
            world.create_slot("ship", 7, "alpha-gel", 3, None, Some(r#"{"grade":"A"}"#))?;
        let alpha_second =
            world.create_slot("ship", 7, "alpha-gel", 5, None, Some(r#"{"grade":"B"}"#))?;
        let _other_owner = world.create_slot("station", 7, "alpha-gel", 99, None, None)?;

        let listed = world.slots_for_owner("ship", 7)?;

        assert_eq!(
            listed.len(),
            3,
            "two alpha slots plus one zeta slot for this owner"
        );
        assert_eq!(listed[0].id, alpha_first.id);
        assert_eq!(listed[1].id, alpha_second.id);
        assert_eq!(listed[2].id, zeta.id);
        assert_eq!(listed[0].owner_kind, "ship".to_string());
        assert_eq!(listed[0].owner_id, 7);
        assert_eq!(listed[0].item_key, "alpha-gel".to_string());
        assert_eq!(listed[1].item_key, "alpha-gel".to_string());
        assert_eq!(listed[2].item_key, "zeta-parts".to_string());

        Ok(())
    }

    #[test]
    fn merge_slots_adds_quantities_for_matching_metadata() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let first = world.create_slot(
            "station",
            7,
            "repair-drone",
            2,
            None,
            Some(r#"{"quality":"new"}"#),
        )?;
        let second = world.create_slot(
            "station",
            7,
            "repair-drone",
            5,
            None,
            Some(r#"{"quality":"new"}"#),
        )?;

        let merged = world.merge_slots(&first, &second)?;

        assert_eq!(merged.id, first.id);
        assert_eq!(merged.quantity, 7);
        assert_eq!(
            merged.metadata_json,
            Some(r#"{"quality":"new"}"#.to_string())
        );

        let owner_total: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM inventory_slots WHERE owner_kind = ?1 AND owner_id = ?2 AND item_key = ?3",
                rusqlite::params!["station", 7, "repair-drone"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(owner_total, 1);

        let list = world.slots_for_owner("station", 7)?;
        assert!(
            list.iter()
                .any(|slot| slot.id == merged.id && slot.quantity == 7),
            "merged quantity should live on one surviving row"
        );

        Ok(())
    }

    #[test]
    fn merge_slots_rejects_mismatched_metadata() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let left = world
            .create_slot("chest", 9, "coin", 10, None, Some(r#"{"origin":"minted"}"#))
            .expect("fixture slot exists");
        let right = world
            .create_slot(
                "chest",
                9,
                "coin",
                3,
                None,
                Some(r#"{"origin":"plundered"}"#),
            )
            .expect("fixture slot exists");

        let failed = world.merge_slots(&left, &right);
        match failed {
            Err(InventoryError::MergeNotCompatible {
                slot_a_id,
                slot_b_id,
                ..
            }) => {
                assert_eq!(slot_a_id, left.id);
                assert_eq!(slot_b_id, right.id);
            }
            other => panic!("expected merge compatibility failure, got {other:?}"),
        }

        let owner_total: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM inventory_slots WHERE owner_kind = ?1 AND owner_id = ?2 AND item_key = ?3",
                rusqlite::params!["chest", 9, "coin"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(
            owner_total, 2,
            "mismatched metadata must leave both rows untouched"
        );
    }

    #[test]
    fn transfer_moves_quantities_with_single_commit() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let captain_supply = world.create_slot(
            "ship",
            1,
            "ration-pack",
            12,
            Some(25),
            Some(r#"{"sealed":"yes"}"#),
        )?;
        let station_store = world.create_slot(
            "station",
            4,
            "ration-pack",
            3,
            Some(50),
            Some(r#"{"module":"coldchain"}"#),
        )?;

        let (after_source, after_destination) = world.transfer(
            ("ship", 1),
            ("station", 4),
            "ration-pack",
            5,
            None::<
                fn(
                    &rusqlite::Transaction<'_>,
                    &InventorySlot,
                    &InventorySlot,
                ) -> rusqlite::Result<()>,
            >,
        )?;

        assert_eq!(after_source.id, captain_supply.id);
        assert_eq!(after_source.quantity, 7);
        assert_eq!(after_destination.id, station_store.id);
        assert_eq!(after_destination.quantity, 8);

        let persisted_source: InventorySlot = world
            .connection()
            .query_row(
                "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
                 FROM inventory_slots\n\
                 WHERE id = ?1",
                rusqlite::params![captain_supply.id],
                row_to_inventory_slot,
            )
            .expect("source slot persists after transfer");
        let persisted_destination: InventorySlot = world
            .connection()
            .query_row(
                "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
                 FROM inventory_slots\n\
                 WHERE id = ?1",
                rusqlite::params![station_store.id],
                row_to_inventory_slot,
            )
            .expect("destination slot persists after transfer");

        assert_eq!(persisted_source.quantity, 7);
        assert_eq!(persisted_destination.quantity, 8);

        Ok(())
    }

    #[test]
    fn transfer_passes_post_mutation_snapshots_to_on_commit() -> Result<(), InventoryError> {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let source = world.create_slot(
            "ship",
            12,
            "water-canister",
            10,
            Some(25),
            Some(r#"{"sealed":"no"}"#),
        )?;
        let destination = world.create_slot(
            "outpost",
            33,
            "water-canister",
            2,
            None,
            Some(r#"{"seal":"hatch"}"#),
        )?;

        fn callback(
            _: &rusqlite::Transaction<'_>,
            source_slot: &InventorySlot,
            destination_slot: &InventorySlot,
        ) -> rusqlite::Result<()> {
            assert_eq!(source_slot.owner_kind, "ship");
            assert_eq!(source_slot.owner_id, 12);
            assert_eq!(source_slot.item_key, "water-canister");
            assert_eq!(destination_slot.owner_kind, "outpost");
            assert_eq!(destination_slot.owner_id, 33);
            assert_eq!(destination_slot.item_key, "water-canister");
            assert_eq!(source_slot.quantity, 6);
            assert_eq!(destination_slot.quantity, 6);
            Ok(())
        }

        let (after_source, after_destination) = world.transfer(
            ("ship", 12),
            ("outpost", 33),
            "water-canister",
            4,
            Some(callback),
        )?;

        assert_eq!(after_source.quantity, 6);
        assert_eq!(after_destination.quantity, 6);

        let persisted_source: InventorySlot = world
            .connection()
            .query_row(
                "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
                 FROM inventory_slots\n\
                 WHERE id = ?1",
                rusqlite::params![source.id],
                row_to_inventory_slot,
            )
            .expect("source slot persists after transfer");
        let persisted_destination: InventorySlot = world
            .connection()
            .query_row(
                "SELECT id, owner_kind, owner_id, item_key, quantity, equilibrium, metadata_json\n\
                 FROM inventory_slots\n\
                 WHERE id = ?1",
                rusqlite::params![destination.id],
                row_to_inventory_slot,
            )
            .expect("destination slot persists after transfer");

        assert_eq!(persisted_source.quantity, 6);
        assert_eq!(persisted_destination.quantity, 6);
        assert_eq!(persisted_source.owner_kind, "ship");
        assert_eq!(persisted_destination.owner_kind, "outpost");

        Ok(())
    }

    #[test]
    fn transfer_rejects_non_positive_quantity() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let source = world
            .create_slot("dock", 9, "fuel-cell", 10, None, None)
            .expect("source slot exists");

        for quantity in [0, -5] {
            let err = world
                .transfer(
                    ("dock", 9),
                    ("carrier", 3),
                    "fuel-cell",
                    quantity,
                    None::<
                        fn(
                            &rusqlite::Transaction<'_>,
                            &InventorySlot,
                            &InventorySlot,
                        ) -> rusqlite::Result<()>,
                    >,
                )
                .expect_err("non-positive transfer should be rejected");

            match err {
                InventoryError::InvalidTransferQuantity { quantity: got } => {
                    assert_eq!(got, quantity);
                }
                other => panic!("expected InvalidTransferQuantity, got {other:?}"),
            }
        }

        let after_source = world
            .get_slot("dock", 9, "fuel-cell")
            .expect("source still readable")
            .expect("source row still exists");
        assert_eq!(after_source.id, source.id);
        assert_eq!(after_source.quantity, 10);

        let missing_destination = world
            .get_slot("carrier", 3, "fuel-cell")
            .expect("destination read is okay");
        assert!(
            missing_destination.is_none(),
            "invalid quantity must not create destination rows"
        );
    }

    #[test]
    fn transfer_rejects_missing_stock_and_leaves_slots_unchanged() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory_slots migration applies");

        let source = world
            .create_slot("player", 1, "ration-pack", 2, None, None)
            .expect("source slot exists");

        let error = world
            .transfer(
                ("player", 1),
                ("merchant", 3),
                "ration-pack",
                5,
                None::<
                    fn(
                        &rusqlite::Transaction<'_>,
                        &InventorySlot,
                        &InventorySlot,
                    ) -> rusqlite::Result<()>,
                >,
            )
            .expect_err("transfer must reject insufficient stock");

        match error {
            InventoryError::InsufficientStock {
                owner_kind,
                owner_id,
                item_key,
                requested,
                available,
            } => {
                assert_eq!(owner_kind, "player");
                assert_eq!(owner_id, 1);
                assert_eq!(item_key, "ration-pack");
                assert_eq!(requested, 5);
                assert_eq!(available, 2);
            }
            other => panic!("expected InsufficientStock, got {other:?}"),
        }

        let after_source = world
            .get_slot("player", 1, "ration-pack")
            .expect("slot still readable")
            .expect("source row still exists");
        assert_eq!(after_source.id, source.id);
        assert_eq!(
            after_source.quantity, 2,
            "failed transfer must not debit source"
        );

        let missing_destination = world
            .get_slot("merchant", 3, "ration-pack")
            .expect("destination read is okay");
        assert!(
            missing_destination.is_none(),
            "insufficient source stock must not auto-create destination row"
        );
    }
}
