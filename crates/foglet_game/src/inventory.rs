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

#[cfg(test)]
mod tests {
    use super::{InventoryError, INVENTORY_SLOTS_MIGRATION};
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
}
