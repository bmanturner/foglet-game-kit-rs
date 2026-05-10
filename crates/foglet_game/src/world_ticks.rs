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

use crate::world_db::{WorldDb, WorldMigration, WorldTickCallback};

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
    /// SQLite query failed while reading due tasks in `run_due_ticks`.
    #[error("failed to query due world ticks: {source}")]
    DueTasksQueryFailed {
        /// Underlying SQLite failure.
        #[source]
        source: rusqlite::Error,
    },
    /// A tick key was registered without an in-memory callback this
    /// runtime process can dispatch.
    #[error("no callback registered in memory for tick `{key}`")]
    MissingCallbackForTick {
        /// Task key with missing callback.
        key: String,
    },
    /// A tick callback rejected because its callback closure failed.
    #[error("tick callback rejected for `{key}`: {source}")]
    CallbackRejected {
        /// Task key whose callback failed.
        key: String,
        /// Callback or SQL failure surfaced from callback context.
        #[source]
        source: rusqlite::Error,
    },
    /// A tick row could not be marked as run within its transaction.
    #[error("failed to mark tick `{key}` as run: {source}")]
    RunTickFailed {
        /// Task key that failed to update.
        key: String,
        /// Underlying SQL failure.
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
    /// The callback argument is stored on this `WorldDb` handle during
    /// startup so later `run_due_ticks` calls can dispatch it in-process.
    /// Registration keeps a single durable row in SQLite and one callback
    /// binding per task key.
    pub fn register_tick<F>(
        &mut self,
        key: &str,
        interval_seconds: i64,
        on_commit: F,
    ) -> Result<WorldTickTask, WorldTickError>
    where
        F: FnMut(&rusqlite::Transaction<'_>) -> rusqlite::Result<()> + 'static,
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

        self.tick_callbacks
            .borrow_mut()
            .insert(key.to_string(), Box::new(on_commit) as WorldTickCallback);

        Ok(task)
    }

    /// Run all due tick callbacks at the supplied `now` timestamp.
    ///
    /// A task is due when `last_run_at` is `NULL` (never run) or the
    /// cadence window has elapsed (`datetime(last_run_at, interval) <= now`).
    ///
    /// Why this method updates `last_run_at` inside the same transaction
    /// as the callback:
    ///
    /// - callback failures roll back the row update, so callers retry the
    ///   missed work later;
    /// - successful callbacks are durable markers of work completed during
    ///   this invocation.
    ///
    /// The return value is the count of callbacks that reached `COMMIT`.
    ///
    /// In a **space exploration** game, this powers periodic station
    /// replenishment without busy looping after long absences.
    /// In a **dungeon crawler**, trap resets can catch up one
    /// tick-at-a-time when the player returns after disconnection.
    /// `max_catchup_per_call` caps how many due tasks run in one
    /// invocation so long downtime doesn't execute an unbounded backlog.
    pub fn run_due_ticks(
        &mut self,
        now: &str,
        max_catchup_per_call: u32,
    ) -> Result<usize, WorldTickError> {
        if max_catchup_per_call == 0 {
            return Ok(0);
        }

        use rusqlite::params;
        use rusqlite::OptionalExtension;

        let mut ran = 0_usize;
        while ran < max_catchup_per_call as usize {
            let Some(task) = self
                .connection()
                .query_row(
                    "SELECT key, last_run_at, interval_seconds, metadata_json\n\
                     FROM world_tick_tasks\n\
                     WHERE last_run_at IS NULL\n\
                        OR datetime(last_run_at, '+' || interval_seconds || ' seconds') <= ?1\n\
                     ORDER BY key\n\
                     LIMIT 1",
                    params![now],
                    row_to_world_tick_task,
                )
                .optional()
                .map_err(|source| WorldTickError::DueTasksQueryFailed { source })?
            else {
                break;
            };

            let mut callback = {
                let mut callbacks = self.tick_callbacks.borrow_mut();
                callbacks.remove(&task.key).ok_or_else(|| {
                    WorldTickError::MissingCallbackForTick {
                        key: task.key.clone(),
                    }
                })?
            };

            let tx = self.connection_mut().transaction().map_err(|source| {
                WorldTickError::RunTickFailed {
                    key: task.key.clone(),
                    source,
                }
            })?;

            let updated = tx
                .execute(
                    "UPDATE world_tick_tasks\n\
                     SET last_run_at = ?2\n\
                     WHERE key = ?1\n\
                        AND (\n\
                             last_run_at IS NULL\n\
                             OR datetime(last_run_at, '+' || interval_seconds || ' seconds') <= ?2\n\
                        )",
                    params![task.key, now],
                )
                .map_err(|source| WorldTickError::RunTickFailed {
                    key: task.key.clone(),
                    source,
                })?;

            if updated == 0 {
                drop(tx);
                self.tick_callbacks
                    .borrow_mut()
                    .insert(task.key.clone(), callback);
                continue;
            }

            if let Err(source) = callback(&tx) {
                drop(tx);
                self.tick_callbacks
                    .borrow_mut()
                    .insert(task.key.clone(), callback);
                return Err(WorldTickError::CallbackRejected {
                    key: task.key.clone(),
                    source,
                });
            }

            tx.commit()
                .map_err(|source| WorldTickError::RunTickFailed {
                    key: task.key.clone(),
                    source,
                })?;

            self.tick_callbacks
                .borrow_mut()
                .insert(task.key.clone(), callback);

            ran += 1;
        }

        Ok(ran)
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
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::thread;
    use std::time::Duration;

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

    /// Task 9c runs only ticks whose cadence is due at `now` and skips
    /// rows that are still waiting for their next window.
    #[test]
    fn run_due_ticks_executes_only_due_tasks() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let calls = Rc::new(RefCell::new(Vec::new()));

        world
            .register_tick("null_key_is_due", 600, {
                let calls = Rc::clone(&calls);
                move |_tx| {
                    calls.borrow_mut().push("null_key_is_due".to_string());
                    Ok(())
                }
            })
            .expect("register_tick persists row");

        world
            .register_tick("not_due", 3600, {
                let calls = Rc::clone(&calls);
                move |_tx| {
                    calls.borrow_mut().push("not_due".to_string());
                    Ok(())
                }
            })
            .expect("register_tick persists row");

        world
            .connection()
            .execute(
                "UPDATE world_tick_tasks\n SET last_run_at = ?1\n WHERE key = ?2",
                params!["2026-01-01 10:30:00", "not_due"],
            )
            .expect("seed not_due last_run_at");

        let ran = world
            .run_due_ticks("2026-01-01 10:45:00", 100)
            .expect("run due ticks executes");

        assert_eq!(ran, 1, "only one task should have advanced at 10:45");
        let observed = calls.borrow().clone();
        assert_eq!(
            observed,
            vec!["null_key_is_due".to_string()],
            "only due callback should run"
        );

        let null_last: String = world
            .connection()
            .query_row(
                "SELECT last_run_at FROM world_tick_tasks WHERE key = ?1",
                params!["null_key_is_due"],
                |row| row.get(0),
            )
            .expect("query updated due row");
        assert_eq!(
            null_last, "2026-01-01 10:45:00",
            "due row should receive new last_run_at"
        );

        let not_due_last: String = world
            .connection()
            .query_row(
                "SELECT last_run_at FROM world_tick_tasks WHERE key = ?1",
                params!["not_due"],
                |row| row.get(0),
            )
            .expect("query unchanged skipped row");
        assert_eq!(
            not_due_last, "2026-01-01 10:30:00",
            "skip target must leave last_run_at untouched"
        );
    }

    /// Task 9d requires failed callbacks to execute atomically with their row
    /// updates and be retriable on the next `run_due_ticks` invocation.
    #[test]
    fn run_due_ticks_retries_failed_callback_without_advancing_last_run_at() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        world
            .connection()
            .execute(
                "CREATE TABLE tick_side_effects (\n                task_key TEXT PRIMARY KEY,\n                attempts INTEGER NOT NULL\n             )",
                [],
            )
            .expect("side-effect fixture table exists");

        let attempts = Rc::new(RefCell::new(0_u32));

        world
            .register_tick("retry_tick", 300, {
                let attempts = Rc::clone(&attempts);
                move |tx| {
                    let attempt = {
                        let mut attempts = attempts.borrow_mut();
                        *attempts += 1;
                        *attempts
                    };

                    tx.execute(
                        "INSERT INTO tick_side_effects (task_key, attempts)\n                     VALUES (?1, ?2)",
                        rusqlite::params!["retry_tick", attempt],
                    )?;

                    if attempt == 1 {
                        return Err(rusqlite::Error::QueryReturnedNoRows);
                    }

                    Ok(())
                }
            })
            .expect("register_tick persists row");

        let first = world.run_due_ticks("2026-01-01 10:45:00", 100);
        assert!(matches!(
            first,
            Err(WorldTickError::CallbackRejected { key, .. }) if key == "retry_tick"
        ));

        let last_run_after_failure: Option<String> = world
            .connection()
            .query_row(
                "SELECT last_run_at FROM world_tick_tasks WHERE key = ?1",
                params!["retry_tick"],
                |row| row.get(0),
            )
            .expect("query persisted row");
        assert!(
            last_run_after_failure.is_none(),
            "failed tick should not advance last_run_at"
        );

        let side_effect_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM tick_side_effects WHERE task_key = ?1",
                params!["retry_tick"],
                |row| row.get(0),
            )
            .expect("query failed inserts from rolled-back attempt");
        assert_eq!(
            side_effect_count, 0,
            "failed callback side-effects must rollback with transaction"
        );

        let second = world
            .run_due_ticks("2026-01-01 10:45:00", 100)
            .expect("retry should now succeed");
        assert_eq!(second, 1, "one retrying callback should now succeed");

        let last_run_after_retry: Option<String> = world
            .connection()
            .query_row(
                "SELECT last_run_at FROM world_tick_tasks WHERE key = ?1",
                params!["retry_tick"],
                |row| row.get(0),
            )
            .expect("query persisted row");
        assert_eq!(
            last_run_after_retry.as_deref(),
            Some("2026-01-01 10:45:00"),
            "successful retry should now stamp last_run_at"
        );

        let stored_attempts: i64 = world
            .connection()
            .query_row(
                "SELECT attempts FROM tick_side_effects WHERE task_key = ?1",
                params!["retry_tick"],
                |row| row.get(0),
            )
            .expect("query committed retry attempt");
        assert_eq!(
            stored_attempts, 2,
            "only successful attempt should remain committed"
        );
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

    /// Task 9e enforces the due-task ceiling on each call so a
    /// large backlog is chunked across invocations.
    #[test]
    fn run_due_ticks_limits_due_tasks_to_max_catchup_per_call() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let calls = Rc::new(RefCell::new(Vec::new()));

        for key in ["first_wave", "second_wave"] {
            world
                .register_tick(key, 120, {
                    let calls = Rc::clone(&calls);
                    move |_tx| {
                        calls.borrow_mut().push(key.to_string());
                        Ok(())
                    }
                })
                .expect("register_tick persists row");
        }

        let first = world
            .run_due_ticks("2026-01-01 11:00:00", 1)
            .expect("run with catch-up cap");
        assert_eq!(
            first, 1,
            "only one task should run when max_catchup_per_call is one"
        );
        assert_eq!(calls.borrow().len(), 1, "one callback should have executed");

        let second = world
            .run_due_ticks("2026-01-01 11:00:00", 1)
            .expect("follow-up run with same cap");
        assert_eq!(
            second, 1,
            "remaining due task should run on the second pass"
        );
        assert_eq!(
            calls.borrow().len(),
            2,
            "both callbacks should eventually run"
        );
    }

    /// Task 9f requires concurrent runners to claim and run disjoint
    /// due-task sets so no task is executed twice.
    #[test]
    fn run_due_ticks_concurrent_runners_do_not_double_invoke_tasks() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let db_path = db_path.into_os_string();

        let alpha_count = Arc::new(AtomicUsize::new(0));
        let beta_count = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(2));

        let run_a = {
            let db_path = db_path.clone();
            let alpha_count = Arc::clone(&alpha_count);
            let beta_count = Arc::clone(&beta_count);
            let start_a = Arc::clone(&start);
            thread::spawn(move || {
                let mut world = WorldDb::open(db_path.as_os_str()).expect("open succeeds");

                world
                    .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
                    .expect("world_tick_tasks migration applies");

                world
                    .register_tick("alpha_watch", 120, move |_tx| {
                        alpha_count.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(75));
                        Ok(())
                    })
                    .expect("register tick alpha");

                world
                    .register_tick("beta_bay", 120, move |_tx| {
                        beta_count.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(75));
                        Ok(())
                    })
                    .expect("register tick beta");

                start_a.wait();
                world
                    .run_due_ticks("2026-01-01 11:30:00", 1)
                    .expect("first runner succeeds")
            })
        };

        let run_b = {
            let db_path = db_path.clone();
            let alpha_count = Arc::clone(&alpha_count);
            let beta_count = Arc::clone(&beta_count);
            let start_b = Arc::clone(&start);
            thread::spawn(move || {
                let mut world = WorldDb::open(db_path.as_os_str()).expect("open succeeds");

                world
                    .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
                    .expect("world_tick_tasks migration applies");

                world
                    .register_tick("alpha_watch", 120, move |_tx| {
                        alpha_count.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(75));
                        Ok(())
                    })
                    .expect("register tick alpha");

                world
                    .register_tick("beta_bay", 120, move |_tx| {
                        beta_count.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(75));
                        Ok(())
                    })
                    .expect("register tick beta");

                start_b.wait();
                world
                    .run_due_ticks("2026-01-01 11:30:00", 1)
                    .expect("second runner succeeds")
            })
        };

        let run_a_count = run_a.join().expect("thread join");
        let run_b_count = run_b.join().expect("thread join");

        assert_eq!(
            run_a_count + run_b_count,
            2,
            "all due tasks should run once"
        );
        assert_eq!(
            alpha_count.load(Ordering::SeqCst),
            1,
            "alpha task should run once"
        );
        assert_eq!(
            beta_count.load(Ordering::SeqCst),
            1,
            "beta task should run once"
        );
    }
}
