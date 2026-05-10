//! `world_ticks` — durable scheduled-task schema (SPEC_v4 Task 9a).
//!
//! The table declared here defines when periodic game logic should run.
//! The module intentionally remains schema-only in this task so later
//! iterations can layer runtime APIs (`register_tick`, `run_due_ticks`) on
//! a stable shape.
//!
//! Genre-neutral framing:
//!
//! - In a **space exploration** game, tasks can be used to refresh
//!   docking manifests, rebalance station supply lines, or trigger
//!   station events.
//! - In a **dungeon crawler**, tasks can restock room hazards,
//!   reset timed puzzle state, or run patrol-wave churn.

use crate::world_db::WorldMigration;

/// Migration for the `world_tick_tasks` table (SPEC_v4 Task 9a).
///
/// The table stores durable, game-authored tick definitions:
///
/// - `key` identifies the task and is the primary key.
/// - `last_run_at` tracks the last successful run and can be NULL for
///   never-run tasks.
/// - `interval_seconds` controls cadence and must be a positive
///   integer.
/// - `metadata_json` is optional, game-authored payload.
pub const WORLD_TICK_TASKS_MIGRATION: WorldMigration = WorldMigration {
    version: 15,
    name: "create_world_tick_tasks",
    sql: "\
CREATE TABLE IF NOT EXISTS world_tick_tasks (
    key             TEXT PRIMARY KEY,
    last_run_at     TEXT,
    interval_seconds INTEGER NOT NULL CHECK (interval_seconds > 0),
    metadata_json   TEXT
);
",
};

#[cfg(test)]
mod tests {
    use super::WORLD_TICK_TASKS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// Task 9a accepts that the migration creates the documented schema.
    #[test]
    fn applies_world_tick_tasks_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, \"notnull\", pk, type\nFROM pragma_table_info('world_tick_tasks')\nORDER BY cid",
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
            .expect("columns query succeeds");

        assert_eq!(
            rows,
            vec![
                ("key".to_string(), 0, 1, "TEXT".to_string()),
                ("last_run_at".to_string(), 0, 0, "TEXT".to_string()),
                ("interval_seconds".to_string(), 1, 0, "INTEGER".to_string()),
                ("metadata_json".to_string(), 0, 0, "TEXT".to_string()),
            ],
            "world_tick_tasks schema must match Task 9a contract"
        );

        let sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'world_tick_tasks'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master row exists");
        assert!(
            sql.contains("CHECK (interval_seconds > 0)"),
            "interval_seconds must be constrained as positive"
        );
    }
}
