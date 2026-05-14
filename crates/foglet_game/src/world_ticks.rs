//! `world_ticks` — durable scheduled-task schema.
//!
//! The table declared here defines when periodic game logic should run.
//! The module intentionally remains schema-only in this task so later
//! iterations can layer runtime APIs (`register_tick`, `run_due_ticks`) on a
//! stable shape. `WorldDb` remains the durability boundary; the callback
//! registration API here only ensures durable registration and deduplicates
//! duplicate definitions by key.
//!
//! Tick callbacks are designed for a lock-safe, multi-runner world where no
//! process can assume it has exclusive write ownership of the database. Callers
//! must therefore treat callback SQL as potentially concurrent with another
//! `run_due_ticks` invocation and prefer idempotent, conflict-tolerant
//! statements.
//!
//! Genre-neutral framing:
//!
//! - In a **space exploration** game, tasks can be used to refresh
//!   docking manifests, rebalance station supply lines, or trigger
//!   station events.
//! - In a **dungeon crawler**, tasks can restock room hazards.
//!   reset timed puzzle state, or run patrol-wave churn.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration, WorldTickCallback};

struct IntervalTickCallback<F>(F);

impl<F> WorldTickCallback for IntervalTickCallback<F>
where
    F: FnMut(&rusqlite::Transaction<'_>) -> rusqlite::Result<()> + 'static,
{
    fn call(
        &mut self,
        tx: &rusqlite::Transaction<'_>,
        _context: &WorldTickContext,
    ) -> rusqlite::Result<()> {
        (self.0)(tx)
    }
}

struct ContextTickCallback<F>(F);

impl<F> WorldTickCallback for ContextTickCallback<F>
where
    F: FnMut(&rusqlite::Transaction<'_>, &WorldTickContext) -> rusqlite::Result<()> + 'static,
{
    fn call(
        &mut self,
        tx: &rusqlite::Transaction<'_>,
        context: &WorldTickContext,
    ) -> rusqlite::Result<()> {
        (self.0)(tx, context)
    }
}

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
    /// Durable schedule kind for this task.
    pub schedule_kind: WorldTickScheduleKind,
    /// Fixed daily slots for daily-slot tasks.
    pub daily_slots: Vec<DailySlot>,
    /// Last completed scheduled slot for daily-slot tasks.
    pub last_completed_slot_at: Option<String>,
}

/// Schedule kind persisted for a world tick task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldTickScheduleKind {
    /// Legacy interval task using `last_run_at + interval_seconds`.
    Interval,
    /// Fixed clock slots that do not drift when execution is late.
    DailySlots,
}

/// A validated wall-clock daily slot in 24-hour `HH:MM` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DailySlot {
    hour: u8,
    minute: u8,
}

impl DailySlot {
    /// Create a validated daily slot from hour and minute components.
    pub fn new(hour: u8, minute: u8) -> Result<Self, WorldTickError> {
        if hour > 23 || minute > 59 {
            return Err(WorldTickError::InvalidDailySlot {
                slot: format!("{hour:02}:{minute:02}"),
            });
        }
        Ok(Self { hour, minute })
    }

    /// Parse a validated daily slot from `HH:MM`.
    pub fn parse(raw: &str) -> Result<Self, WorldTickError> {
        let Some((hour, minute)) = raw.split_once(':') else {
            return Err(WorldTickError::InvalidDailySlot {
                slot: raw.to_string(),
            });
        };
        if hour.len() != 2 || minute.len() != 2 {
            return Err(WorldTickError::InvalidDailySlot {
                slot: raw.to_string(),
            });
        }
        let hour = hour
            .parse::<u8>()
            .map_err(|_| WorldTickError::InvalidDailySlot {
                slot: raw.to_string(),
            })?;
        let minute = minute
            .parse::<u8>()
            .map_err(|_| WorldTickError::InvalidDailySlot {
                slot: raw.to_string(),
            })?;
        Self::new(hour, minute).map_err(|_| WorldTickError::InvalidDailySlot {
            slot: raw.to_string(),
        })
    }

    /// Return the canonical `HH:MM` representation.
    pub fn as_hh_mm(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    fn seconds_after_midnight(self) -> i64 {
        i64::from(self.hour) * 3600 + i64::from(self.minute) * 60
    }
}

/// Context supplied to world tick callbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldTickContext {
    /// Stable task key.
    pub task_key: String,
    /// Scheduled slot datetime being applied.
    pub scheduled_at: String,
    /// Actual runner datetime supplied to `run_due_ticks`.
    pub actual_run_at: String,
    /// Schedule kind that produced this callback.
    pub schedule_kind: WorldTickScheduleKind,
    /// Zero-based ordering within this `run_due_ticks` invocation.
    pub catchup_index: usize,
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
    /// Daily slot list was empty.
    #[error("daily slot tick `{key}` must include at least one slot")]
    EmptyDailySlots {
        /// Task key that failed validation.
        key: String,
    },
    /// A daily slot was malformed or outside 00:00 through 23:59.
    #[error("invalid daily slot `{slot}`; expected unique HH:MM between 00:00 and 23:59")]
    InvalidDailySlot {
        /// Malformed slot value.
        slot: String,
    },
    /// A daily slot list repeated a slot.
    #[error("duplicate daily slot `{slot}` for `{key}`")]
    DuplicateDailySlot {
        /// Task key that failed validation.
        key: String,
        /// Duplicate slot.
        slot: String,
    },
    /// Persisted schedule metadata could not be understood.
    #[error("ambiguous world tick schedule metadata for `{key}`: {details}")]
    AmbiguousScheduleMetadata {
        /// Task key with bad durable metadata.
        key: String,
        /// Explanation of the metadata problem.
        details: String,
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
    /// - on repeated registration with the same key and a different interval.
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
        self.ensure_world_tick_schedule_columns(key)?;

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
            "INSERT INTO world_tick_tasks (key, interval_seconds, metadata_json, schedule_kind)\n\
             VALUES (?1, ?2, NULL, 'interval')\n\
             ON CONFLICT(key) DO UPDATE\n\
             SET interval_seconds = excluded.interval_seconds,\n\
                 schedule_kind = 'interval',\n\
                 daily_slots_json = NULL,\n\
                 last_completed_slot_at = NULL",
            rusqlite::params![key, interval_seconds],
        )
        .map_err(|source| WorldTickError::RegisterFailed {
            key: key.to_string(),
            source,
        })?;

        let task = tx
            .query_row(
                &format!("{SELECT_TASK_SQL} FROM world_tick_tasks WHERE key = ?1"),
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
            .insert(key.to_string(), Box::new(IntervalTickCallback(on_commit)));

        Ok(task)
    }

    fn next_due_tick(&self, now: &str) -> Result<Option<(WorldTickTask, String)>, WorldTickError> {
        use rusqlite::OptionalExtension;

        let interval = self
            .connection()
            .query_row(
                &format!(
                    "{SELECT_TASK_SQL} FROM world_tick_tasks\n\
                     WHERE schedule_kind = 'interval'\n\
                       AND (last_run_at IS NULL\n\
                            OR datetime(last_run_at, '+' || interval_seconds || ' seconds') <= ?1)\n\
                     ORDER BY key LIMIT 1"
                ),
                rusqlite::params![now],
                row_to_world_tick_task,
            )
            .optional()
            .map_err(|source| WorldTickError::DueTasksQueryFailed { source })?
            .map(|task| (task, now.to_string()));

        let mut stmt = self
            .connection()
            .prepare(&format!(
                "{SELECT_TASK_SQL} FROM world_tick_tasks\n\
                 WHERE schedule_kind = 'daily_slots'\n\
                 ORDER BY key"
            ))
            .map_err(|source| WorldTickError::DueTasksQueryFailed { source })?;
        let daily_rows = stmt
            .query_map([], row_to_world_tick_task)
            .map_err(|source| WorldTickError::DueTasksQueryFailed { source })?;

        let mut best_daily: Option<(WorldTickTask, String)> = None;
        for row in daily_rows {
            let task = row.map_err(|source| WorldTickError::DueTasksQueryFailed { source })?;
            let scheduled_at = next_due_daily_slot(&task, now)?;
            if let Some(scheduled_at) = scheduled_at {
                let replace = best_daily
                    .as_ref()
                    .map(|(best_task, best_scheduled_at)| {
                        scheduled_at < *best_scheduled_at
                            || (scheduled_at == *best_scheduled_at && task.key < best_task.key)
                    })
                    .unwrap_or(true);
                if replace {
                    best_daily = Some((task, scheduled_at));
                }
            }
        }

        Ok(match (interval, best_daily) {
            (Some(interval), Some(daily)) => {
                if daily.1 <= interval.1 {
                    Some(daily)
                } else {
                    Some(interval)
                }
            }
            (Some(interval), None) => Some(interval),
            (None, Some(daily)) => Some(daily),
            (None, None) => None,
        })
    }

    /// Register a recurring world tick at fixed daily `HH:MM` wall-clock slots.
    ///
    /// Slots are stored in sorted canonical order and the task remains keyed by
    /// `key`, so re-registering the same key updates the durable schedule
    /// instead of creating another task. Timestamps are interpreted in the same
    /// timezone as the `now` string supplied to [`WorldDb::run_due_ticks`].
    pub fn register_daily_slot_tick<F>(
        &mut self,
        key: &str,
        slots: &[&str],
        on_commit: F,
    ) -> Result<WorldTickTask, WorldTickError>
    where
        F: FnMut(&rusqlite::Transaction<'_>, &WorldTickContext) -> rusqlite::Result<()> + 'static,
    {
        self.ensure_world_tick_schedule_columns(key)?;

        let slots = validate_slots(key, slots)?;
        self.register_daily_slot_tick_slots(key, &slots, on_commit)
    }

    /// Register a fixed daily slot tick from already typed slots.
    pub fn register_daily_slot_tick_slots<F>(
        &mut self,
        key: &str,
        slots: &[DailySlot],
        on_commit: F,
    ) -> Result<WorldTickTask, WorldTickError>
    where
        F: FnMut(&rusqlite::Transaction<'_>, &WorldTickContext) -> rusqlite::Result<()> + 'static,
    {
        self.ensure_world_tick_schedule_columns(key)?;

        let slots = validate_typed_slots(key, slots)?;
        let slots_json =
            serde_json::to_string(&slots.iter().map(|slot| slot.as_hh_mm()).collect::<Vec<_>>())
                .expect("serializing daily slots cannot fail");

        let tx = self.connection_mut().transaction().map_err(|source| {
            WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            }
        })?;

        tx.execute(
            "INSERT INTO world_tick_tasks (key, interval_seconds, metadata_json, schedule_kind, daily_slots_json)\n\
             VALUES (?1, 1, NULL, 'daily_slots', ?2)\n\
             ON CONFLICT(key) DO UPDATE\n\
             SET interval_seconds = 1,\n\
                 schedule_kind = 'daily_slots',\n\
                 daily_slots_json = excluded.daily_slots_json",
            rusqlite::params![key, slots_json],
        )
        .map_err(|source| WorldTickError::RegisterFailed {
            key: key.to_string(),
            source,
        })?;

        let task = tx
            .query_row(
                &format!("{SELECT_TASK_SQL} FROM world_tick_tasks WHERE key = ?1"),
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
            .insert(key.to_string(), Box::new(ContextTickCallback(on_commit)));

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
    ///
    /// A callback must not assume this process has the only writer lock on the
    /// world database. `run_due_ticks` can be called by login flow and by
    /// `fgk tick`, and there can be more than one process invoking either
    /// path. Keep callback SQL idempotent (or protected by application-level
    /// version checks) so retries and concurrent invocations remain safe.
    ///
    /// **Call-site guidance:** do not invoke this from
    /// paint/render loops. Run it from login boundaries or explicit
    /// screen transitions so callback latency is paid at game-state
    /// boundaries instead of frame time.
    pub fn run_due_ticks(
        &mut self,
        now: &str,
        max_catchup_per_call: u32,
    ) -> Result<usize, WorldTickError> {
        self.ensure_world_tick_schedule_columns("run_due_ticks")?;

        if max_catchup_per_call == 0 {
            return Ok(0);
        }

        use rusqlite::params;
        let mut ran = 0_usize;
        while ran < max_catchup_per_call as usize {
            let Some((task, scheduled_at)) = self.next_due_tick(now)? else {
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

            let updated = if task.schedule_kind == WorldTickScheduleKind::DailySlots {
                tx.execute(
                    "UPDATE world_tick_tasks\n\
                     SET last_run_at = ?3,\n\
                         last_completed_slot_at = ?2\n\
                     WHERE key = ?1\n\
                       AND schedule_kind = 'daily_slots'\n\
                       AND (last_completed_slot_at IS NULL OR last_completed_slot_at < ?2)",
                    params![task.key, scheduled_at, now],
                )
            } else {
                tx.execute(
                    "UPDATE world_tick_tasks\n\
                     SET last_run_at = ?2\n\
                     WHERE key = ?1\n\
                        AND schedule_kind = 'interval'\n\
                        AND (\n\
                             last_run_at IS NULL\n\
                             OR datetime(last_run_at, '+' || interval_seconds || ' seconds') <= ?2\n\
                        )",
                    params![task.key, now],
                )
            }
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

            let context = WorldTickContext {
                task_key: task.key.clone(),
                scheduled_at,
                actual_run_at: now.to_string(),
                schedule_kind: task.schedule_kind,
                catchup_index: ran,
            };

            if let Err(source) = callback.call(&tx, &context) {
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

    /// Return whether any interval task or fixed daily slot is currently due.
    ///
    /// This is intended for operator summaries after a bounded
    /// [`WorldDb::run_due_ticks`] pass. It uses the same schedule discovery as
    /// the runner without claiming or mutating a task.
    pub fn has_due_ticks(&mut self, now: &str) -> Result<bool, WorldTickError> {
        self.ensure_world_tick_schedule_columns("has_due_ticks")?;
        self.next_due_tick(now).map(|due| due.is_some())
    }

    fn ensure_world_tick_schedule_columns(&mut self, key: &str) -> Result<(), WorldTickError> {
        let existing = self
            .connection()
            .prepare("SELECT name FROM pragma_table_info('world_tick_tasks')")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|source| WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            })?;

        let conn = self.connection_mut();
        if !existing.iter().any(|name| name == "schedule_kind") {
            conn.execute_batch(
                "ALTER TABLE world_tick_tasks ADD COLUMN schedule_kind TEXT NOT NULL DEFAULT 'interval' CHECK (schedule_kind IN ('interval', 'daily_slots'))",
            )
            .map_err(|source| WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            })?;
        }
        if !existing.iter().any(|name| name == "daily_slots_json") {
            conn.execute_batch("ALTER TABLE world_tick_tasks ADD COLUMN daily_slots_json TEXT")
                .map_err(|source| WorldTickError::RegisterFailed {
                    key: key.to_string(),
                    source,
                })?;
        }
        if !existing.iter().any(|name| name == "last_completed_slot_at") {
            conn.execute_batch(
                "ALTER TABLE world_tick_tasks ADD COLUMN last_completed_slot_at TEXT",
            )
            .map_err(|source| WorldTickError::RegisterFailed {
                key: key.to_string(),
                source,
            })?;
        }
        Ok(())
    }
}

const SELECT_TASK_SQL: &str = "\
SELECT key, last_run_at, interval_seconds, metadata_json,\n\
       COALESCE(schedule_kind, 'interval'), daily_slots_json, last_completed_slot_at";

fn row_to_world_tick_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorldTickTask> {
    use rusqlite::types::Type;

    let key: String = row.get(0)?;
    let schedule_kind_raw: String = row.get(4)?;
    let schedule_kind = match schedule_kind_raw.as_str() {
        "interval" => WorldTickScheduleKind::Interval,
        "daily_slots" => WorldTickScheduleKind::DailySlots,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                4,
                Type::Text,
                format!("unknown schedule_kind `{schedule_kind_raw}`").into(),
            ))
        }
    };
    let daily_slots_json: Option<String> = row.get(5)?;
    let daily_slots = match (schedule_kind, daily_slots_json) {
        (WorldTickScheduleKind::Interval, _) => Vec::new(),
        (WorldTickScheduleKind::DailySlots, Some(raw)) => {
            let raw_slots: Vec<String> = serde_json::from_str(&raw).map_err(|source| {
                rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(source))
            })?;
            let slot_refs = raw_slots.iter().map(String::as_str).collect::<Vec<_>>();
            validate_slots(&key, &slot_refs).map_err(|source| {
                rusqlite::Error::FromSqlConversionFailure(5, Type::Text, Box::new(source))
            })?
        }
        (WorldTickScheduleKind::DailySlots, None) => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                Type::Null,
                "daily slot schedule missing slots".into(),
            ))
        }
    };

    Ok(WorldTickTask {
        key,
        last_run_at: row.get(1)?,
        interval_seconds: row.get(2)?,
        metadata_json: row.get(3)?,
        schedule_kind,
        daily_slots,
        last_completed_slot_at: row.get(6)?,
    })
}

fn validate_slots(key: &str, slots: &[&str]) -> Result<Vec<DailySlot>, WorldTickError> {
    if slots.is_empty() {
        return Err(WorldTickError::EmptyDailySlots {
            key: key.to_string(),
        });
    }
    let parsed = slots
        .iter()
        .map(|slot| DailySlot::parse(slot))
        .collect::<Result<Vec<_>, _>>()?;
    validate_typed_slots(key, &parsed)
}

fn validate_typed_slots(key: &str, slots: &[DailySlot]) -> Result<Vec<DailySlot>, WorldTickError> {
    if slots.is_empty() {
        return Err(WorldTickError::EmptyDailySlots {
            key: key.to_string(),
        });
    }
    let mut slots = slots.to_vec();
    slots.sort();
    for pair in slots.windows(2) {
        if pair[0] == pair[1] {
            return Err(WorldTickError::DuplicateDailySlot {
                key: key.to_string(),
                slot: pair[0].as_hh_mm(),
            });
        }
    }
    Ok(slots)
}

fn next_due_daily_slot(task: &WorldTickTask, now: &str) -> Result<Option<String>, WorldTickError> {
    let now_parts =
        parse_datetime(now).ok_or_else(|| WorldTickError::AmbiguousScheduleMetadata {
            key: task.key.clone(),
            details: format!("runner now `{now}` is not YYYY-MM-DD HH:MM:SS"),
        })?;
    let last_parts = task
        .last_completed_slot_at
        .as_deref()
        .map(|last| {
            parse_datetime(last).ok_or_else(|| WorldTickError::AmbiguousScheduleMetadata {
                key: task.key.clone(),
                details: "last_completed_slot_at is not YYYY-MM-DD HH:MM:SS".to_string(),
            })
        })
        .transpose()?;

    let now_day = days_from_civil(now_parts.0, now_parts.1, now_parts.2);
    let now_second = now_parts.3;
    if task.last_completed_slot_at.is_none() {
        return Ok(task
            .daily_slots
            .iter()
            .rev()
            .find(|slot| slot.seconds_after_midnight() <= now_second)
            .map(|slot| {
                format!(
                    "{:04}-{:02}-{:02} {}:00",
                    now_parts.0,
                    now_parts.1,
                    now_parts.2,
                    slot.as_hh_mm()
                )
            }));
    }
    let start_day = last_parts
        .map(|(year, month, day, _)| days_from_civil(year, month, day))
        .unwrap_or(now_day);

    for day in start_day..=now_day {
        let (year, month, dom) = civil_from_days(day);
        for slot in &task.daily_slots {
            let scheduled_second = slot.seconds_after_midnight();
            if day == now_day && scheduled_second > now_second {
                continue;
            }
            let scheduled_at = format!("{year:04}-{month:02}-{dom:02} {}:00", slot.as_hh_mm());
            if task
                .last_completed_slot_at
                .as_deref()
                .map(|last| scheduled_at.as_str() <= last)
                .unwrap_or(false)
            {
                continue;
            }
            return Ok(Some(scheduled_at));
        }
    }

    Ok(None)
}

fn parse_datetime(raw: &str) -> Option<(i32, u32, u32, i64)> {
    let date = raw.get(0..10)?;
    let time = raw.get(11..19)?;
    if raw.as_bytes().get(10) != Some(&b' ') {
        return None;
    }
    let year = date.get(0..4)?.parse::<i32>().ok()?;
    let month = date.get(5..7)?.parse::<u32>().ok()?;
    let day = date.get(8..10)?.parse::<u32>().ok()?;
    let hour = time.get(0..2)?.parse::<i64>().ok()?;
    let minute = time.get(3..5)?.parse::<i64>().ok()?;
    let second = time.get(6..8)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    Some((year, month, day, hour * 3600 + minute * 60 + second))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719468;
    let era = (if days >= 0 { days } else { days - 146096 }) / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (
        (year + i64::from(month <= 2)) as i32,
        month as u32,
        day as u32,
    )
}

/// Migration for the `world_tick_tasks` table.
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
    metadata_json   TEXT,
    schedule_kind   TEXT NOT NULL DEFAULT 'interval' CHECK (schedule_kind IN ('interval', 'daily_slots')),
    daily_slots_json TEXT,
    last_completed_slot_at TEXT
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

    ///  registration helper accepts fresh definitions and rounds them
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

    ///  requires idempotent registration when the same task key and
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
                &format!(
                    "{} FROM world_tick_tasks WHERE key = ?1",
                    super::SELECT_TASK_SQL
                ),
                params!["dungeon_patrol"],
                row_to_world_tick_task,
            )
            .expect("existing task is still readable");
        assert_eq!(persisted, first);
        assert_eq!(persisted, second);
    }

    ///  rejects invalid cadence values before touching SQLite and keeps
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

    ///  runs only ticks whose cadence is due at `now` and skips
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

    ///  requires failed callbacks to execute atomically with their row
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

    ///  accepts that the migration creates the documented schema.
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
                ("schedule_kind".to_string(), 1, 0, "TEXT".to_string()),
                ("daily_slots_json".to_string(), 0, 0, "TEXT".to_string()),
                (
                    "last_completed_slot_at".to_string(),
                    0,
                    0,
                    "TEXT".to_string(),
                ),
            ],
            "world_tick_tasks schema must match contract"
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

    ///  enforces the due-task ceiling on each call so a
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

    ///  requires concurrent runners to claim and run disjoint
    /// due-task sets so no task is executed twice.
    #[test]
    fn run_due_ticks_concurrent_runners_do_not_double_invoke_tasks() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let alpha_count = Arc::new(AtomicUsize::new(0));
        let beta_count = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(2));

        // Seed the DB once before the concurrent phase so this test isolates
        // the run_due_ticks claiming behavior instead of migration/DDL lock
        // timing during open/register setup.
        let mut seed_world = WorldDb::open(&db_path).expect("open succeeds");
        seed_world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");
        seed_world
            .register_tick("alpha_watch", 120, |_| Ok(()))
            .expect("register tick alpha");
        seed_world
            .register_tick("beta_bay", 120, |_| Ok(()))
            .expect("register tick beta");

        let run_a = {
            let db_path = db_path.clone();
            let alpha_count = Arc::clone(&alpha_count);
            let beta_count = Arc::clone(&beta_count);
            let start_a = Arc::clone(&start);
            thread::spawn(move || {
                let mut world = WorldDb::open(&db_path).expect("open succeeds");

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
                let mut world = WorldDb::open(&db_path).expect("open succeeds");

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

    #[test]
    fn daily_slot_tick_runs_late_without_drifting_next_slot() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let observed = Rc::new(RefCell::new(Vec::new()));
        world
            .register_daily_slot_tick("station_clock", &["00:00", "06:00", "12:00", "18:00"], {
                let observed = Rc::clone(&observed);
                move |_tx, context| {
                    observed.borrow_mut().push((
                        context.scheduled_at.clone(),
                        context.actual_run_at.clone(),
                        context.catchup_index,
                    ));
                    Ok(())
                }
            })
            .expect("register daily slot tick");

        let first = world
            .run_due_ticks("2026-01-01 06:37:00", 1)
            .expect("late 06:00 slot runs");
        assert_eq!(first, 1);
        assert_eq!(
            observed.borrow().as_slice(),
            &[(
                "2026-01-01 06:00:00".to_string(),
                "2026-01-01 06:37:00".to_string(),
                0
            )]
        );

        let before_noon = world
            .run_due_ticks("2026-01-01 11:59:00", 10)
            .expect("next fixed slot is not drifted");
        assert_eq!(before_noon, 0);

        let noon = world
            .run_due_ticks("2026-01-01 12:00:00", 10)
            .expect("fixed noon slot runs at noon");
        assert_eq!(noon, 1);
        assert_eq!(observed.borrow()[1].0, "2026-01-01 12:00:00");
    }

    #[test]
    fn daily_slot_tick_catches_up_across_midnight_with_cap() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        let observed = Rc::new(RefCell::new(Vec::new()));
        world
            .register_daily_slot_tick("station_clock", &["00:00", "06:00", "12:00", "18:00"], {
                let observed = Rc::clone(&observed);
                move |_tx, context| {
                    observed.borrow_mut().push(context.scheduled_at.clone());
                    Ok(())
                }
            })
            .expect("register daily slot tick");
        world
            .connection()
            .execute(
                "UPDATE world_tick_tasks SET last_completed_slot_at = ?1, last_run_at = ?1 WHERE key = ?2",
                params!["2026-01-01 18:00:00", "station_clock"],
            )
            .expect("seed previous completed slot");

        let ran = world
            .run_due_ticks("2026-01-02 12:30:00", 2)
            .expect("catches up with cap");
        assert_eq!(ran, 2);
        assert_eq!(
            observed.borrow().as_slice(),
            &[
                "2026-01-02 00:00:00".to_string(),
                "2026-01-02 06:00:00".to_string()
            ]
        );

        let remaining = world
            .run_due_ticks("2026-01-02 12:30:00", 10)
            .expect("remaining due slot runs");
        assert_eq!(remaining, 1);
        assert_eq!(observed.borrow()[2], "2026-01-02 12:00:00");
    }

    #[test]
    fn daily_slot_registration_rejects_invalid_slot_lists() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
            .expect("world_tick_tasks migration applies");

        assert!(matches!(
            world.register_daily_slot_tick("empty", &[], |_tx, _ctx| Ok(())),
            Err(WorldTickError::EmptyDailySlots { key }) if key == "empty"
        ));
        assert!(matches!(
            world.register_daily_slot_tick("bad", &["24:00"], |_tx, _ctx| Ok(())),
            Err(WorldTickError::InvalidDailySlot { slot }) if slot == "24:00"
        ));
        assert!(matches!(
            world.register_daily_slot_tick("dup", &["06:00", "06:00"], |_tx, _ctx| Ok(())),
            Err(WorldTickError::DuplicateDailySlot { key, slot }) if key == "dup" && slot == "06:00"
        ));
    }

    #[test]
    fn old_interval_only_row_is_upgraded_and_remains_runnable() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .connection()
            .execute_batch(
                "CREATE TABLE world_tick_tasks (
                    key TEXT PRIMARY KEY,
                    last_run_at TEXT,
                    interval_seconds INTEGER NOT NULL CHECK (interval_seconds > 0),
                    metadata_json TEXT
                );
                INSERT INTO world_tick_tasks (key, last_run_at, interval_seconds, metadata_json)
                VALUES ('legacy_interval', NULL, 300, NULL);",
            )
            .expect("seed old interval-only schema");

        let calls = Rc::new(RefCell::new(0));
        world
            .register_tick("legacy_interval", 300, {
                let calls = Rc::clone(&calls);
                move |_tx| {
                    *calls.borrow_mut() += 1;
                    Ok(())
                }
            })
            .expect("legacy row registers after additive upgrade");

        let ran = world
            .run_due_ticks("2026-01-01 10:00:00", 10)
            .expect("legacy interval row remains runnable");
        assert_eq!(ran, 1);
        assert_eq!(*calls.borrow(), 1);

        let schedule_kind: String = world
            .connection()
            .query_row(
                "SELECT schedule_kind FROM world_tick_tasks WHERE key = ?1",
                params!["legacy_interval"],
                |row| row.get(0),
            )
            .expect("new schedule column exists");
        assert_eq!(schedule_kind, "interval");
    }
}
