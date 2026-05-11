//! `fgk tick` — one-shot world-tick runner for hosted games.
//!
//! This module is intentionally small and side-effect explicit:
//!
//! - It loads the project config at `assets/game.toml`.
//! - It opens the configured world database.
//! - It registers no-op callbacks for all registered `world_tick_tasks`.
//! - It invokes `run_due_ticks` once with the configured catch-up bound.
//!
//! The intent is not to create a daemon or background scheduler:
//! `fgk tick` is a manual/operator-invoked catch-up path for when the
//! door process is already idle. That keeps schedule control explicit in
//! deployment scripts while preserving the spec-level contract of no
//! long-lived worker.
//!
//! Genre-neutral examples:
//!
//! - In a **space exploration** door, this command can be run from cron after
//!   a station goes offline to repulse timed replenishment tasks.
//! - In a **dungeon crawler**, operators can invoke it while no players are
//!   connected to refresh trap tables and patrol state before reopening the
//!   room cycle.

use std::path::Path;
use std::thread;
use std::time::Duration;

use foglet_game::{
    ConfigError, GameConfig, WorldDb, WorldDbError, WorldTickError, WORLD_TICK_TASKS_MIGRATION,
};
use thiserror::Error;

use crate::emit_manifest::GAME_TOML_RELATIVE;

/// Error variants for a `fgk tick` invocation.
#[derive(Debug, Error)]
pub enum TickCommandError {
    /// The game project did not include a parseable `assets/game.toml`.
    #[error("failed to load game config from `{path}`: {source}")]
    Config {
        /// Path we attempted to read.
        path: String,
        /// Why config parsing or schema validation failed.
        #[source]
        source: ConfigError,
    },

    /// `world_ticks` must be enabled for the command to do anything.
    ///
    /// Disabled means there is no runtime contract for callbacks.
    #[error(
        "`[world_ticks]` is disabled; enable it in assets/game.toml before running `fgk tick`"
    )]
    WorldTicksDisabled,

    /// Failed to open the configured SQLite path (permissions, corruption,
    /// missing parents, etc.).
    #[error("failed to open world db at `{path}`: {source}")]
    OpenWorldDb {
        /// Configured database path.
        path: String,
        /// Database open/mount failure.
        #[source]
        source: WorldDbError,
    },

    /// The configured world database does not exist yet.
    ///
    /// `fgk tick` is an operator-facing maintenance command; we do not
    /// auto-create this file because that would blur the distinction
    /// between "no work to do" and "runtime/world state has not been
    /// provisioned yet."
    #[error("world db is missing at `{path}`")]
    MissingWorldDb {
        /// Configured path that must already exist.
        path: String,
    },

    /// The migration install for `world_tick_tasks` failed.
    #[error("failed to ensure `world_tick_tasks` table: {source}")]
    EnsureTickTable {
        /// Why SQLite rejected the migration apply.
        #[source]
        source: WorldDbError,
    },

    /// We could not load row metadata for callback registration.
    #[error("failed to read tasks while preparing tick callbacks: {details}")]
    ReadTasks {
        /// SQL read failure.
        details: String,
    },

    /// Callback registration must happen in-memory in the CLI process, and
    /// that registration failed for one of the rows.
    #[error("failed to register in-memory tick callback for `{key}`: {source}")]
    RegisterCallback {
        /// Tick key that could not be registered.
        key: String,
        /// Why registration failed.
        #[source]
        source: WorldTickError,
    },

    /// Running due ticks failed during execution.
    #[error("run_due_ticks failed: {source}")]
    RunDueTicks {
        /// Run failure from `foglet_game`.
        #[source]
        source: WorldTickError,
    },
}

/// Summary for a single `fgk tick` run.
#[derive(Debug, Clone, Copy)]
pub struct TickRunSummary {
    /// Number of callbacks that reached COMMIT in this run.
    pub tasks_run: usize,
    /// Number of due tasks deferred to a later call by the catch-up bound.
    pub tasks_skipped: usize,
}

/// Execute one `fgk tick` call against a project.
///
/// This is the library half of the CLI subcommand. It is intentionally
/// deterministic and side-effect explicit:
///
/// 1. Load config and resolve `world.path`.
/// 2. Open the DB and ensure `world_tick_tasks` migration exists.
/// 3. Register a no-op callback for each durable task key so execution is
///    always in-process.
/// 4. Compute `now` and run due tasks once.
///
/// A return value of `tasks_skipped = 0` means every due task ran in this
/// invocation. A larger skip count means due work is still pending.
pub fn run_tick(project_dir: &Path) -> Result<TickRunSummary, TickCommandError> {
    let config_path = project_dir.join(GAME_TOML_RELATIVE);
    let config = GameConfig::load(&config_path).map_err(|source| TickCommandError::Config {
        path: config_path.display().to_string(),
        source,
    })?;

    if !config.world_ticks.enabled {
        return Err(TickCommandError::WorldTicksDisabled);
    }

    let world_path = project_dir.join(&config.world.path);
    if !world_path.exists() {
        return Err(TickCommandError::MissingWorldDb {
            path: world_path.display().to_string(),
        });
    }

    let mut world =
        WorldDb::open(world_path.clone()).map_err(|source| TickCommandError::OpenWorldDb {
            path: world_path.display().to_string(),
            source,
        })?;

    let callback_delay_ms = std::env::var("FGK_TICK_TEST_CALLBACK_DELAY_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|delay_ms| *delay_ms > 0);

    world
        .apply_migration(&WORLD_TICK_TASKS_MIGRATION)
        .map_err(|source| TickCommandError::EnsureTickTable { source })?;

    let tasks: Vec<(String, i64)> = {
        let mut stmt = world
            .connection()
            .prepare("SELECT key, interval_seconds FROM world_tick_tasks ORDER BY key")
            .map_err(|source| TickCommandError::ReadTasks {
                details: source.to_string(),
            })?;
        let rows = stmt
            .query_map([], |row| Ok::<(String, i64), _>((row.get(0)?, row.get(1)?)))
            .map_err(|source| TickCommandError::ReadTasks {
                details: source.to_string(),
            })?;

        let mut out = Vec::<(String, i64)>::new();
        for row in rows {
            out.push(row.map_err(|source| TickCommandError::ReadTasks {
                details: source.to_string(),
            })?);
        }
        out
    };

    for (key, interval_seconds) in tasks {
        let delay_ms = callback_delay_ms;
        world
            .register_tick(&key, interval_seconds, move |_tx| {
                if let Some(delay_ms) = delay_ms {
                    thread::sleep(Duration::from_millis(delay_ms));
                }
                Ok(())
            })
            .map_err(|source| TickCommandError::RegisterCallback { key, source })?;
    }

    let now: String = world
        .connection()
        .query_row("SELECT datetime('now')", [], |row| row.get(0))
        .map_err(|source| TickCommandError::ReadTasks {
            details: source.to_string(),
        })?;

    let due_count: usize = world
        .connection()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM world_tick_tasks \
             WHERE last_run_at IS NULL \
             OR datetime(last_run_at, '+' || interval_seconds || ' seconds') <= '{}'",
                now
            ),
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| TickCommandError::ReadTasks {
            details: source.to_string(),
        })? as usize;

    let tasks_run = world
        .run_due_ticks(&now, config.world_ticks.max_catchup_per_call)
        .map_err(|source| TickCommandError::RunDueTicks { source })?;

    Ok(TickRunSummary {
        tasks_run,
        tasks_skipped: due_count.saturating_sub(tasks_run),
    })
}
