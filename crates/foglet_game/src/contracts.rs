//! `contracts` — durable contract lifecycle schema (SPEC_v5 Task 3a).
//!
//! This module defines the shared-world table shape for contract-style
//! opportunities. The schema stays intentionally genre-neutral:
//!
//! - In a **space exploration** game, a row can represent a freight
//!   contract offered by a station authority.
//! - In a **dungeon crawler**, a row can represent a guild commission
//!   to recover an artifact from a crypt.
//!
//! Later v5 tasks layer CRUD and lifecycle transitions on top of this
//! durable shape.

use crate::world_db::WorldMigration;

/// Migration for the `contracts` table (SPEC_v5 Task 3a).
///
/// The schema captures one contract lifecycle row with optional
/// acceptance/completion timestamps and a state machine guard:
///
/// - `id` is the stable numeric row identity.
/// - `key` is an optional caller-defined stable identifier.
/// - `kind` and issuer owner fields are caller-defined taxonomy.
/// - `acceptor_player_id` is `NULL` until a player accepts.
/// - `state` is constrained to documented enum values.
/// - `objective_json`, `reward_json`, and `metadata_json` hold opaque
///   game-authored payloads.
/// - `created_at` is always present; other lifecycle timestamps are
///   nullable until each transition occurs.
///
/// This shape does not prescribe economics or story vocabulary. A
/// trading game can store delivery payloads while a fantasy game stores
/// escort objectives, both using the same table contract.
pub const CONTRACTS_MIGRATION: WorldMigration = WorldMigration {
    version: 16,
    name: "create_contracts",
    sql: "\
CREATE TABLE IF NOT EXISTS contracts (\n\
    id                  INTEGER PRIMARY KEY,\n\
    key                 TEXT UNIQUE,\n\
    kind                TEXT NOT NULL,\n\
    issuer_owner_kind   TEXT NOT NULL,\n\
    issuer_owner_id     INTEGER NOT NULL,\n\
    acceptor_player_id  INTEGER,\n\
    state               TEXT NOT NULL CHECK (state IN ('available', 'accepted', 'completed', 'failed', 'abandoned', 'expired')),\n\
    objective_json      TEXT NOT NULL,\n\
    reward_json         TEXT NOT NULL,\n\
    metadata_json       TEXT,\n\
    created_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    accepted_at         TEXT,\n\
    completed_at        TEXT,\n\
    expires_at          TEXT\n\
);\n\
",
};

#[cfg(test)]
mod tests {
    use super::CONTRACTS_MIGRATION;
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn applies_contracts_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, \"notnull\", pk, type\nFROM pragma_table_info('contracts')\nORDER BY cid",
            )
            .expect("pragma_table_info preparable");
        let rows: Vec<(String, i64, i64, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            rows,
            vec![
                ("id".to_string(), 0, 1, "INTEGER".to_string()),
                ("key".to_string(), 0, 0, "TEXT".to_string()),
                ("kind".to_string(), 1, 0, "TEXT".to_string()),
                ("issuer_owner_kind".to_string(), 1, 0, "TEXT".to_string()),
                ("issuer_owner_id".to_string(), 1, 0, "INTEGER".to_string()),
                (
                    "acceptor_player_id".to_string(),
                    0,
                    0,
                    "INTEGER".to_string()
                ),
                ("state".to_string(), 1, 0, "TEXT".to_string()),
                ("objective_json".to_string(), 1, 0, "TEXT".to_string()),
                ("reward_json".to_string(), 1, 0, "TEXT".to_string()),
                ("metadata_json".to_string(), 0, 0, "TEXT".to_string()),
                ("created_at".to_string(), 1, 0, "TEXT".to_string()),
                ("accepted_at".to_string(), 0, 0, "TEXT".to_string()),
                ("completed_at".to_string(), 0, 0, "TEXT".to_string()),
                ("expires_at".to_string(), 0, 0, "TEXT".to_string()),
            ],
            "contracts schema must match Task 3a contract"
        );

        let sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'contracts'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master contains contracts create statement");
        assert!(
            sql.contains(
                "CHECK (state IN ('available', 'accepted', 'completed', 'failed', 'abandoned', 'expired'))"
            ),
            "state should be constrained to the documented lifecycle enum"
        );
    }

    #[test]
    fn contracts_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("first contracts migration applies");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("second contracts migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE name = ?1",
                params![CONTRACTS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("world_migrations query succeeds");
        assert_eq!(count, 1, "contracts migration record should be idempotent");
    }
}
