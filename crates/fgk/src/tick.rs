//! `fgk tick` — one-shot world-tick runner for hosted v4 worlds.
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
/// The function returns the number of callbacks that reached `COMMIT`.
/// A return value of zero is common when no tasks are due; it is still
/// a successful run for cron operators.
pub fn run_tick(project_dir: &Path) -> Result<usize, TickCommandError> {
    let config_path = project_dir.join(GAME_TOML_RELATIVE);
    let config = GameConfig::load(&config_path).map_err(|source| TickCommandError::Config {
        path: config_path.display().to_string(),
        source,
    })?;

    if !config.world_ticks.enabled {
        return Err(TickCommandError::WorldTicksDisabled);
    }

    let world_path = project_dir.join(&config.world.path);
    let mut world =
        WorldDb::open(world_path.clone()).map_err(|source| TickCommandError::OpenWorldDb {
            path: world_path.display().to_string(),
            source,
        })?;

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
        world
            .register_tick(&key, interval_seconds, |_tx| Ok(()))
            .map_err(|source| TickCommandError::RegisterCallback { key, source })?;
    }

    let now: String = world
        .connection()
        .query_row("SELECT datetime('now')", [], |row| row.get(0))
        .map_err(|source| TickCommandError::ReadTasks {
            details: source.to_string(),
        })?;

    world
        .run_due_ticks(&now, config.world_ticks.max_catchup_per_call)
        .map_err(|source| TickCommandError::RunDueTicks { source })
}
