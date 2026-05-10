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

use crate::world_db::WorldMigration;

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

#[cfg(test)]
mod tests {
    use super::INVENTORY_SLOTS_MIGRATION;
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
}
