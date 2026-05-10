//! `world_ticks` — durable scheduled-task schema (SPEC_v4 Task 9a).
//!
//! The table declared here defines when periodic game logic should run.
//! The module intentionally remains schema-only in this task so later
//! iterations can layer runtime APIs (`register_tick`, `run_due_ticks`) on a
//! stable shape. `WorldDb` remains the durability boundary; the callback
//! registration API here only ensures durable registration and deduplicates
//! duplicate definitions by key.
//!
//! Genre-neutral framing:
//!
//! - In a **space exploration** game, tasks can be used to refresh
//!   docking manifests, rebalance station supply lines, or trigger
//!   station events.
//! - In a **dungeon crawler**, tasks can restock room hazards,
//!   reset timed puzzle state, or run patrol-wave churn.

use thiserror::Error;

use crate::world_db::WorldDb;
use crate::world_db::WorldMigration;

/// Durable timer task persisted in `world_tick_tasks`.
///
/// The row stores the cadence contract for one recurring callback:
///
/// - `key` is the task id and owner-controlled namespace.
/// - `last_run_at` captures the last successful invocation for catch-up math.
/// - `interval_seconds` is a positive cadence in whole seconds.
/// - `metadata_json` is optional game-authored JSON.
///
/// In a **space exploration** game, this lets a station-owner task keep
/// cargo replenishment aligned to ship arrivals. In a **dungeon crawler**, the
/// same shape can represent trap reset cadence or timed wave spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldTickTask {
    /// Stable task key used by both persistence and callback registration.
    pub key: String,
    /// Last successful run timestamp (`NULL` before first execution).
    pub last_run_at: Option<String>,
    /// Positive recurrence in seconds.
    pub interval_seconds: i64,
    /// Optional game-authored metadata payload.
    pub metadata_json: Option<String>,
}

/// Errors for task registration and lookup.
///
/// Registration is intentionally strict here to keep bad callbacks and bad
/// intervals from silently creating unrecoverable scheduling state.
#[derive(Debug, Error)]
pub enum WorldTickError {
    /// Registration failed due to malformed/invalid interval inputs.
    #[error("invalid world tick interval for `{key}`: {interval_seconds}; interval must be greater than zero")]
    InvalidInterval {
        /// Task key that failed registration.
        key: String,
        /// Interval supplied by the caller.
        interval_seconds: i64,
    },
    /// SQLite failed while inserting or updating the durable task row.
    #[error("failed to register world tick `{key}`: {source}")]
    RegisterFailed {
        /// Task key that failed.
        key: String,
        /// Underlying SQLite failure.
        #[source]
        source: rusqlite::Error,
    },
    /// SQLite failed while reading the registered row back for return.
    #[error("failed to read world tick `{key}` after registration: {source}")]
    ReadAfterRegisterFailed {
        /// Task key that failed reading.
        key: String,
        /// Underlying SQLite failure.
        #[source]
        source: rusqlite::Error,
    },
}

impl WorldDb {
    /// Register a recurring world tick definition by key.
    ///
    /// The method is idempotent with respect to `(key, interval_seconds)`:
    ///
    /// - on first call, the row is inserted;
    /// - on repeated registration with the same key and interval, the existing
    ///   row is left as-is;
    /// - on repeated registration with the same key and a different interval,
    ///   the cadence is updated so one source of truth remains.
    ///
    /// The callback argument is intentionally accepted now so task startup code
    /// can register durable callbacks consistently across all v4 features.
    /// This iteration only persists and normalizes metadata; callback storage and
    /// invocation wiring lands in Task 9c.
    pub fn register_tick<F>(
        &mut self,
        key: &str,
        interval_seconds: i64,
        _on_commit: F,
    ) -> Result<WorldTickTask, WorldTickError>
    where
        F: FnMut(&rusqlite::Transaction<'_>) -> rusqlite::Result<()>,
    {
        if interval_seconds <= 0 {
            return Err(WorldTickError::InvalidInterval {
                key: key.to_string(),
                interval_seconds,
            });
        }

        let tx = self.connection_mut().transaction().map_err(|source| {
            WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            }
        })?;

        tx.execute(
            "INSERT INTO world_tick_tasks (key, interval_seconds, metadata_json)\n\
             VALUES (?1, ?2, NULL)\n\
             ON CONFLICT(key) DO UPDATE\n\
             SET interval_seconds = excluded.interval_seconds",
            rusqlite::params![key, interval_seconds],
        )
        .map_err(|source| WorldTickError::RegisterFailed {
            key: key.to_string(),
            source,
        })?;

        let task = tx
            .query_row(
                "SELECT key, last_run_at, interval_seconds, metadata_json\n\
                 FROM world_tick_tasks\n\
                 WHERE key = ?1",
                rusqlite::params![key],
                row_to_world_tick_task,
            )
            .map_err(|source| WorldTickError::ReadAfterRegisterFailed {
                key: key.to_string(),
                source,
            })?;

        tx.commit()
            .map_err(|source| WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            })?;

        Ok(task)
    }
}

fn row_to_world_tick_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorldTickTask> {
    Ok(WorldTickTask {
        key: row.get(0)?,
        last_run_at: row.get(1)?,
        interval_seconds: row.get(2)?,
        metadata_json: row.get(3)?,
    })
}

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
    use super::{row_to_world_tick_task, WorldTickError, WORLD_TICK_TASKS_MIGRATION};
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use tempfile::tempdir;

    /// Task 9b registration helper accepts fresh definitions and rounds them
    /// through a typed row so callers can reuse the returned data for scheduling
    /// decisions.
    #[test]
    fn register_tick_persists_row_and_returns_committed_task() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let task = world
            .register_tick("station_restock", 600, |_tx| Ok(()))
            .expect("register_tick persists row");

        assert_eq!(task.key, "station_restock");
        assert_eq!(task.interval_seconds, 600);
        assert!(task.last_run_at.is_none());
        assert!(task.metadata_json.is_none());

        let persisted: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_tick_tasks WHERE key = ?1",
                params!["station_restock"],
                |row| row.get(0),
            )
            .expect("row count query runs");
        assert_eq!(persisted, 1, "one row should be persisted per key");
    }

    /// Task 9b requires idempotent registration when the same task key and
    /// interval are re-applied at startup.
    #[test]
    fn register_tick_is_idempotent_for_same_key_and_interval() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let first = world
            .register_tick("dungeon_patrol", 900, |_tx| Ok(()))
            .expect("first registration persists");
        let second = world
            .register_tick("dungeon_patrol", 900, |_tx| Ok(()))
            .expect("second registration is idempotent");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_tick_tasks WHERE key = ?1",
                params!["dungeon_patrol"],
                |row| row.get(0),
            )
            .expect("row count query runs");

        assert_eq!(count, 1, "re-registering must not create duplicates");
        assert_eq!(
            first.interval_seconds, second.interval_seconds,
            "existing interval is reused"
        );

        let persisted = world
            .connection()
            .query_row(
                "SELECT key, last_run_at, interval_seconds, metadata_json\n                 FROM world_tick_tasks\n                 WHERE key = ?1",
                params!["dungeon_patrol"],
                row_to_world_tick_task,
            )
            .expect("existing task is still readable");
        assert_eq!(persisted, first);
        assert_eq!(persisted, second);
    }

    /// Task 9b rejects invalid cadence values before touching SQLite and keeps
    /// the registration table unchanged.
    #[test]
    fn register_tick_rejects_non_positive_interval() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let result = world.register_tick("bad_interval", 0, |_tx| Ok(()));
        match result {
            Err(WorldTickError::InvalidInterval {
                key,
                interval_seconds,
            }) => {
                assert_eq!(key, "bad_interval");
                assert_eq!(interval_seconds, 0);
            }
            other => panic!("expected InvalidInterval, got {other:?}"),
        }

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_tick_tasks", [], |row| {
                row.get(0)
            })
            .expect("row count query runs");
        assert_eq!(count, 0, "invalid interval should not write rows");
    }

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
