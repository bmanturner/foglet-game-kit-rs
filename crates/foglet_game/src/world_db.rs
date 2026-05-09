//! `world_db` — shared-world SQLite handle (SPEC_v2 §Task 3).
//!
//! This module owns the lifetime of the per-door SQLite database that
//! powers v2's shared-world features (player registry, daily turn
//! ledger, append-only event log, leaderboards, and Murder Motel
//! Room 7 fixture). Every authoring concern that needs cross-player
//! persistence flows through a [`WorldDb`] handle so the rest of the
//! runtime never speaks `rusqlite::Connection` directly.
//!
//! # What lands here, and when
//!
//! Task 4a (this commit) extends the open path to bootstrap the
//! `world_migrations` bookkeeping table. The table itself is empty —
//! Task 4b records the first row and Task 4c hardens idempotency around
//! repeat applications.
//!
//! Keeping each behavior in its own iteration means the test that
//! ships with this commit covers exactly one promise ("the file opens
//! under a temp dir") and a future bisect across the world DB layer
//! lands on the iteration that introduced the regression rather than
//! a 400-line "stand up the world DB" mega-commit.
//!
//! # Why a wrapper instead of exposing `Connection` directly
//!
//! Three reasons, in priority order:
//!
//! 1. **Architecture tenet (PROMPT.md): the runtime contract is
//!    centralised.** Authoring code shouldn't reach into raw
//!    `rusqlite` any more than it reaches into `crossterm` raw mode.
//!    Wrapping the connection lets later tasks (3c–3d) tighten the
//!    open path without touching every call site.
//! 2. **Error funnelling.** A `thiserror`-derived [`WorldDbError`] at
//!    the boundary lets the runtime layer (Task 10) decide whether a
//!    DB-open failure is a clean-error abort or a fatal panic without
//!    every caller pattern-matching `rusqlite::Error` variants.
//! 3. **Testability.** The Task 4+ migration helpers and Task 9
//!    transaction wrapper hang off this type. Putting the constructor
//!    behind `WorldDb::open` means tests in those tasks build on the
//!    same surface authors use in production.

use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use thiserror::Error;

/// Tunables applied to the SQLite connection at open time.
///
/// Authors normally build this from the
/// [`crate::config::WorldSection`] via [`From`] (below) so a `[world]`
/// TOML edit propagates without code changes.
///
/// `Default` matches the SPEC v2 §5 documented defaults — 5 seconds of
/// retry and WAL journaling — so unit tests and ad-hoc callers
/// (Task 4 migration tests, for example) can spell
/// `WorldDbOptions::default()` without rediscovering the SPEC's
/// numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldDbOptions {
    /// Milliseconds SQLite spends retrying a locked database before
    /// returning `SQLITE_BUSY`. Translates to
    /// `sqlite3_busy_timeout(ms)` via [`Connection::busy_timeout`].
    ///
    /// `0` disables the busy handler entirely (immediate `BUSY`
    /// errors). v2 ships a non-zero default because contention is
    /// expected: a local-dev session running two Murder Motel
    /// instances against the same SQLite file would otherwise see
    /// spurious lock errors on the second writer.
    pub busy_timeout_ms: u64,
    /// SQLite journal mode applied via `PRAGMA journal_mode = X`.
    ///
    /// Stored as a `String` (rather than a closed enum) so a config
    /// authored against a future SQLite that grows new journal modes
    /// continues to round-trip. The set of values accepted at open
    /// time is constrained — see `ALLOWED_JOURNAL_MODES` — so an
    /// arbitrary string can never reach the SQL pragma.
    ///
    /// The default is `wal` because v2's shared-world workload (two
    /// local-dev sessions + a Foglet host) benefits materially from
    /// WAL's reader/writer concurrency. Networked filesystems that
    /// reject WAL fall back automatically — see [`WorldDb::journal_mode`].
    pub journal_mode: String,
}

/// Journal modes the world-DB open path will pass through to SQLite.
///
/// Limited to the documented `PRAGMA journal_mode` values. Restricting
/// the set up front is also a defence-in-depth measure: the value gets
/// interpolated into a `PRAGMA journal_mode = X` statement (SQLite
/// pragmas don't accept bind parameters), so funnelling it through an
/// allowlist keeps a malformed `[world].journal_mode` from turning into
/// a SQL injection vector.
pub(crate) const ALLOWED_JOURNAL_MODES: &[&str] =
    &["delete", "truncate", "persist", "memory", "wal", "off"];

impl Default for WorldDbOptions {
    fn default() -> Self {
        // Defaults intentionally mirror `default_world_*` in `config.rs`
        // so this struct works in tests that don't touch `GameConfig`
        // parsing. Keeping them in lockstep is checked by the
        // `options_from_world_section_*` regression tests below.
        Self {
            busy_timeout_ms: 5_000,
            journal_mode: "wal".to_string(),
        }
    }
}

impl From<&crate::config::WorldSection> for WorldDbOptions {
    /// Bridge `[world]` TOML config to the open-time tunables. Lives
    /// here (not on `WorldSection`) so the config module stays free of
    /// `rusqlite` knowledge — `WorldSection` is also serialised back
    /// out by `fgk new`, and we want it to stay a pure data type.
    fn from(section: &crate::config::WorldSection) -> Self {
        Self {
            busy_timeout_ms: section.busy_timeout_ms,
            journal_mode: section.journal_mode.clone(),
        }
    }
}

/// Handle to a shared-world SQLite database.
///
/// One handle per running door process. Authoring code receives this
/// (eventually wrapped in `Option`) on [`crate::screen::GameContext`]
/// in Task 10; until then it stands alone so the open/bootstrap path
/// can be exercised in isolation.
///
/// The handle is **not** `Clone`: SQLite connections are not safe to
/// share across threads, and v2 explicitly defers real-time
/// multiplayer (SPEC §3.2). A single owner per door process is the
/// shape every later task assumes.
#[derive(Debug)]
pub struct WorldDb {
    /// Underlying `rusqlite` connection. Kept private so future tasks
    /// (Task 9 transactions, Task 4 migrations) can layer behavior on
    /// top without breaking callers that grabbed `&mut conn` directly.
    conn: Connection,
    /// Journal mode SQLite reported as active after the open-time
    /// `PRAGMA journal_mode = X` round-trip. Stored so callers (and
    /// the Task 14 docs) can distinguish "WAL applied" from "WAL
    /// requested but the host downgraded to delete" — SQLite signals a
    /// downgrade by returning the old mode rather than raising an
    /// error, so the only way to know is to read what came back.
    journal_mode: String,
}

impl WorldDb {
    /// Open the SQLite database at `path`, creating the file if it
    /// does not yet exist.
    ///
    /// Errors are mapped onto [`WorldDbError`] so the runtime layer
    /// can surface a SPEC §13.1 clean-error message ("could not open
    /// world database at /srv/foglet/doors/.../world/world.sqlite")
    /// without callers having to match on `rusqlite::Error` directly.
    ///
    /// # Parent directory handling (Task 3b)
    ///
    /// If the immediate or any ancestor parent directory of `path`
    /// does not yet exist, [`open`](Self::open) creates the chain via
    /// `fs::create_dir_all` *before* asking SQLite to open the file.
    /// This matches SPEC §10.4's packaging contract: a fresh install
    /// includes only `world/.keep` (later tasks), and authors should
    /// be able to point at `world/world.sqlite` without an explicit
    /// `mkdir -p` step in `run.sh`.
    ///
    /// Failures to create the parent chain surface as
    /// [`WorldDbError::CreateParents`] so the operator can distinguish
    /// "filesystem rejected `mkdir`" (permission/disk-full) from
    /// "SQLite rejected the open" (corrupt file, locked DB).
    ///
    /// Equivalent to [`Self::open_with_options`] using
    /// [`WorldDbOptions::default`]. Kept as a convenience because the
    /// majority of unit-test call sites (and the test fixtures that
    /// land in Task 4+) don't care about the open-time knobs.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorldDbError> {
        Self::open_with_options(path, WorldDbOptions::default())
    }

    /// Open the SQLite database and apply the supplied tunables.
    ///
    /// Honours both [`WorldDbOptions::busy_timeout_ms`] and
    /// [`WorldDbOptions::journal_mode`] so the runtime layer (Task 10)
    /// only ever calls a single constructor.
    ///
    /// Tunables are applied *after* [`Connection::open`] so a failure
    /// to set a pragma surfaces with the responsible knob named
    /// (e.g. [`WorldDbError::ApplyBusyTimeout`],
    /// [`WorldDbError::ApplyJournalMode`]) rather than masquerading as a
    /// generic open error — operationally these are very different
    /// conditions (the file is fine, the connection just couldn't be
    /// configured).
    pub fn open_with_options(
        path: impl AsRef<Path>,
        options: WorldDbOptions,
    ) -> Result<Self, WorldDbError> {
        let path_ref = path.as_ref();

        // Create the parent chain when one is named. `Path::parent`
        // returns `Some("")` for bare relative filenames like
        // `world.sqlite`; `create_dir_all("")` is a no-op on Unix but
        // skipping it keeps behavior portable and avoids surfacing
        // confusing errors on platforms that disagree.
        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| WorldDbError::CreateParents {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
        }

        let conn = Connection::open(path_ref).map_err(|source| WorldDbError::Open {
            path: path_ref.display().to_string(),
            source,
        })?;

        // `busy_timeout` calls `sqlite3_busy_timeout`, which installs a
        // retry handler so contended writes (two local-dev sessions on
        // the same file) wait instead of erroring. We apply it
        // unconditionally — including for `0` — so callers who
        // explicitly want the no-retry behavior get it without having
        // to know that "skip the call" and "pass 0" coincide today.
        conn.busy_timeout(Duration::from_millis(options.busy_timeout_ms))
            .map_err(|source| WorldDbError::ApplyBusyTimeout {
                busy_timeout_ms: options.busy_timeout_ms,
                source,
            })?;

        let journal_mode = apply_journal_mode(&conn, &options.journal_mode)?;

        bootstrap_migrations_table(&conn)?;

        Ok(Self { conn, journal_mode })
    }

    /// Journal mode that SQLite reported as active after open.
    ///
    /// Usually equals the requested value (`"wal"` for the default
    /// config). May differ when SQLite refuses the requested mode —
    /// the canonical case is asking for `wal` on a network filesystem
    /// that doesn't support shared-memory mapping; SQLite silently
    /// keeps the prior mode and returns it from the pragma. Callers
    /// (operator docs, future Task 14 health checks) inspect this to
    /// surface the downgrade rather than assume WAL took effect.
    pub fn journal_mode(&self) -> &str {
        &self.journal_mode
    }

    /// Borrow the underlying connection for crate-internal use.
    ///
    /// Crate-private on purpose: only sibling modules (Task 4
    /// migrations onward) should reach in. External authors get the
    /// curated helpers that ship with later tasks.
    #[allow(dead_code)] // Used by Task 4+ once they land.
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Apply a single [`WorldMigration`], recording its version in
    /// `world_migrations` on success.
    ///
    /// Task 4b implements the *happy path* only: one migration is
    /// executed and one row is recorded. Idempotency (re-applying the
    /// same version is a no-op) lands in Task 4c, and the
    /// "failed-migration leaves no row" guarantee lands in Task 4d. The
    /// SQL body and the bookkeeping insert run inside a single SQLite
    /// transaction so 4d can keep the failure semantics tight without
    /// having to revisit this code path.
    ///
    /// `&mut self` is required because [`Connection::transaction`] needs
    /// a unique borrow. The runtime layer (Task 10) wraps the world DB
    /// in `Option<WorldDb>` on `GameContext`, and authoring code reaches
    /// it through a `&mut` borrow scoped to a single screen tick — so
    /// the mutable signature here matches the call shape that's coming.
    pub fn apply_migration(&mut self, migration: &WorldMigration) -> Result<(), WorldDbError> {
        let WorldMigration { version, name, sql } = migration;

        let tx = self
            .conn
            .transaction()
            .map_err(|source| WorldDbError::ApplyMigration {
                version: *version,
                name: name.to_string(),
                source,
            })?;

        // The migration body is author-supplied SQL — almost always a
        // multi-statement `CREATE TABLE`/`CREATE INDEX` batch — so use
        // `execute_batch` rather than `execute` to permit semicolons.
        tx.execute_batch(sql)
            .map_err(|source| WorldDbError::ApplyMigration {
                version: *version,
                name: name.to_string(),
                source,
            })?;

        tx.execute(
            "INSERT INTO world_migrations (version, name) VALUES (?1, ?2)",
            rusqlite::params![*version, name],
        )
        .map_err(|source| WorldDbError::ApplyMigration {
            version: *version,
            name: name.to_string(),
            source,
        })?;

        tx.commit().map_err(|source| WorldDbError::ApplyMigration {
            version: *version,
            name: name.to_string(),
            source,
        })?;

        Ok(())
    }
}

/// A schema/bootstrap step authored by a game.
///
/// Mirrors SPEC_v2 §4.3: a monotonically increasing `version`, a
/// human-readable `name`, and a SQL `sql` body. The `checksum` field
/// the SPEC mentions ("if practical") is intentionally deferred —
/// Task 4b ships embedded SQL migrations only; file-backed migrations
/// (and the checksum that goes with them) are a v2-future extension
/// and would land alongside the public API surface that loads them.
///
/// Borrowed string fields keep the type allocation-free at the call
/// site: a game module can declare `const MIGRATIONS: &[WorldMigration]
/// = &[WorldMigration { version: 1, name: "init", sql: "..." }]`
/// without any heap traffic. Owned-string variants can be added later
/// if file-backed migrations need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldMigration {
    /// Monotonically increasing version. SPEC §4.3 requires uniqueness;
    /// Task 4c (idempotency) and Task 4d (failure handling) layer the
    /// "applied exactly once" rule on top of this.
    pub version: i64,
    /// Short human-readable label echoed back in errors and operator
    /// tooling. Kept distinct from `version` so two migrations with
    /// adjacent versions stay diagnosable in logs.
    pub name: &'static str,
    /// SQL body executed via `execute_batch`. Multi-statement bodies
    /// are supported on purpose: a single `CREATE TABLE` migration
    /// commonly ships its supporting indexes in the same step.
    pub sql: &'static str,
}

/// Apply `PRAGMA journal_mode = X` and return SQLite's reported active
/// mode.
///
/// Pulled out of [`WorldDb::open_with_options`] to keep the open path
/// readable and to make the validation rule (allowlist + case folding)
/// easy to find. The function trims and lower-cases the requested mode
/// so `"WAL"`, `" wal"`, and `"wal"` all behave identically — config
/// authors shouldn't be punished for a stray space or capital letter.
fn apply_journal_mode(conn: &Connection, requested: &str) -> Result<String, WorldDbError> {
    let normalized = requested.trim().to_ascii_lowercase();
    if !ALLOWED_JOURNAL_MODES.contains(&normalized.as_str()) {
        return Err(WorldDbError::InvalidJournalMode {
            requested: requested.to_string(),
        });
    }

    // SQLite pragmas don't accept bind parameters, so the value is
    // interpolated. Safety comes from the allowlist above: `normalized`
    // is provably one of a handful of literal ASCII keywords. The query
    // returns one row whose single column is the active mode (string).
    let active: String = conn
        .query_row(&format!("PRAGMA journal_mode = {normalized}"), [], |row| {
            row.get(0)
        })
        .map_err(|source| WorldDbError::ApplyJournalMode {
            requested: normalized,
            source,
        })?;

    Ok(active)
}

/// Create the `world_migrations` bookkeeping table if it isn't already
/// present.
///
/// Called from every successful open path so a fresh database has the
/// table ready for Task 4b (recording an applied migration) and a
/// previously-bootstrapped database is unaffected — `IF NOT EXISTS`
/// keeps the call idempotent across the relaunches that happen on every
/// `external_pty` re-exec (SPEC §7).
///
/// The schema is intentionally minimal for Task 4a: later sub-tasks
/// (4b–4d) populate `name`/`checksum` and write rows. Columns:
///
/// - `version` — `INTEGER PRIMARY KEY` so rows are unique on the
///   monotonic version contract from SPEC §4.3 and queries that ask
///   "what's the highest applied version" are an index lookup.
/// - `name` — human-friendly label echoed back in errors and operator
///   tooling. `NOT NULL` because every migration ships with one.
/// - `checksum` — nullable, populated only for file-backed migrations
///   per SPEC §4.3 ("if practical").
/// - `applied_at` — UTC timestamp of when the migration was recorded,
///   defaulted to `CURRENT_TIMESTAMP` so callers don't have to thread
///   a clock through to bookkeeping inserts.
fn bootstrap_migrations_table(conn: &Connection) -> Result<(), WorldDbError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS world_migrations (\n\
            version    INTEGER PRIMARY KEY,\n\
            name       TEXT NOT NULL,\n\
            checksum   TEXT,\n\
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n\
         )",
    )
    .map_err(|source| WorldDbError::BootstrapMigrationsTable { source })
}

/// Errors raised while opening or operating on a [`WorldDb`].
///
/// Library-internal `thiserror` per the architecture tenets: callers
/// at the process boundary (the runtime, the CLI) wrap this with
/// `anyhow` so end-user output stays a single sentence.
#[derive(Debug, Error)]
pub enum WorldDbError {
    /// `rusqlite::Connection::open` rejected the path. With Task 3b
    /// the "parent directory does not exist" failure mode is gone, so
    /// hitting this variant typically means the file exists but is
    /// corrupt, locked by another process, or unreadable.
    #[error("failed to open world database at `{path}`: {source}")]
    Open {
        /// Path the caller asked us to open, echoed back so the
        /// operator-facing error names a concrete file.
        path: String,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },

    /// `Connection::busy_timeout` rejected the configured value. In
    /// practice this is rare — `sqlite3_busy_timeout` accepts any
    /// non-negative integer — but surfacing it as its own variant
    /// keeps "the file is fine, the knob isn't" diagnosable separately
    /// from a real open failure.
    #[error("failed to apply world database busy_timeout={busy_timeout_ms}ms: {source}")]
    ApplyBusyTimeout {
        /// Value the caller asked us to apply, echoed back so the
        /// operator-facing error names the offending knob.
        busy_timeout_ms: u64,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },

    /// The configured `[world].journal_mode` value isn't one of the
    /// modes the world DB layer is willing to pass through to SQLite
    /// (see `ALLOWED_JOURNAL_MODES`). Surfaced as a distinct variant
    /// so the operator-facing error tells them the config string is
    /// the problem — not the file, not the host SQLite build.
    #[error(
        "unsupported world database journal_mode `{requested}` \
         (expected one of: delete, truncate, persist, memory, wal, off)"
    )]
    InvalidJournalMode {
        /// Raw value as it appeared in config, echoed verbatim so the
        /// operator can spot the typo without consulting the source.
        requested: String,
    },

    /// `PRAGMA journal_mode = X` returned an error. Distinct from
    /// [`Self::InvalidJournalMode`] so a host-level rejection (e.g.
    /// SQLite built without WAL support) is diagnosable separately
    /// from a configuration typo.
    #[error("failed to apply world database journal_mode={requested}: {source}")]
    ApplyJournalMode {
        /// Normalized mode we attempted to apply.
        requested: String,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },

    /// `CREATE TABLE IF NOT EXISTS world_migrations` failed during the
    /// open-time bootstrap. Distinct variant so an operator-facing error
    /// can name "migration bookkeeping" specifically — a generic
    /// [`Self::Open`] would mislead, since by this point the file is
    /// open and the connection is configured.
    #[error("failed to bootstrap world_migrations table: {source}")]
    BootstrapMigrationsTable {
        /// Underlying `rusqlite` error from the `CREATE TABLE` batch.
        #[source]
        source: rusqlite::Error,
    },

    /// Applying a [`WorldMigration`] failed at any of its phases:
    /// opening the wrapping transaction, executing the SQL body, or
    /// recording the bookkeeping row in `world_migrations`. Task 4d
    /// hardens the "no row written on failure" guarantee around this
    /// variant; Task 4b only needs the variant to exist so the open
    /// path's error type can carry it.
    #[error("failed to apply world migration v{version} `{name}`: {source}")]
    ApplyMigration {
        /// Version we attempted to apply, echoed back so the
        /// operator-facing error names the offending migration.
        version: i64,
        /// Human-readable name of the migration, echoed back for the
        /// same reason.
        name: String,
        /// Underlying `rusqlite` error from the transaction, batch, or
        /// insert.
        #[source]
        source: rusqlite::Error,
    },

    /// `fs::create_dir_all` failed while ensuring the parent chain
    /// for the database file exists. Distinct from [`Self::Open`] so
    /// the operator can tell "filesystem won't let me make the
    /// directory" apart from "SQLite won't let me open the file".
    #[error("failed to create world database parent directory `{path}`: {source}")]
    CreateParents {
        /// Parent path we tried (and failed) to create.
        path: String,
        /// Underlying I/O error from `fs::create_dir_all`.
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// SPEC_v2 §Task 3a acceptance: opening a SQLite file under a
    /// temp dir succeeds and yields a usable handle.
    #[test]
    fn opens_sqlite_under_temp_dir() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let world = WorldDb::open(&db_path).expect("open succeeds under temp dir");

        // The file is created on disk so subsequent runs (and the
        // `external_pty` re-exec dance described in SPEC §7) reuse
        // the same database rather than starting fresh.
        assert!(
            db_path.exists(),
            "Connection::open must create the SQLite file"
        );

        // Smoke-test the handle by issuing the simplest possible
        // statement. We don't care about the result value — only that
        // the connection is live and the wrapper exposes it to
        // crate-internal callers (Task 4 migrations build on this).
        let conn = world.connection();
        let one: i64 = conn
            .query_row("SELECT 1", [], |row| row.get(0))
            .expect("trivial query runs against an open connection");
        assert_eq!(one, 1);
    }

    /// SPEC_v2 §Task 3c acceptance: the configured busy timeout is
    /// actually applied to the SQLite connection. We verify by
    /// querying `PRAGMA busy_timeout`, which echoes back the current
    /// `sqlite3_busy_timeout` value in milliseconds. Reading it
    /// directly (rather than trusting that `Connection::busy_timeout`
    /// returned `Ok`) guards against a future regression where the
    /// open path silently drops the option on the floor.
    #[test]
    fn open_with_options_applies_busy_timeout() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        // 7500ms is intentionally not the default (5000) so a
        // regression that ignored the option and left rusqlite's
        // built-in default in place would still flunk the assertion.
        let world = WorldDb::open_with_options(
            &db_path,
            WorldDbOptions {
                busy_timeout_ms: 7_500,
                ..WorldDbOptions::default()
            },
        )
        .expect("open succeeds with explicit busy timeout");

        let configured: i64 = world
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy_timeout pragma is queryable");
        assert_eq!(
            configured, 7_500,
            "PRAGMA busy_timeout must echo the value we applied at open time"
        );
    }

    /// SPEC_v2 §Task 3c follow-up: the convenience [`WorldDb::open`]
    /// must still produce a non-zero busy timeout matching
    /// [`WorldDbOptions::default`]. Without this the most common
    /// caller (Task 4 migration tests, Task 10 runtime startup) would
    /// inherit rusqlite's bare-`Connection::open` default of "no busy
    /// handler" — exactly the brittle behavior 3c exists to prevent.
    #[test]
    fn open_applies_default_busy_timeout() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("default open succeeds");

        let configured: i64 = world
            .connection()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy_timeout pragma is queryable");
        assert_eq!(
            configured,
            i64::from(WorldDbOptions::default().busy_timeout_ms as i32),
            "default open path must apply the documented default busy timeout"
        );
    }

    /// `WorldSection` -> `WorldDbOptions` is a thin bridge but it's
    /// worth a regression guard: a typo there would silently fall back
    /// to the `Default` value and we'd never notice in higher-level
    /// tests that don't exercise non-default config.
    #[test]
    fn options_from_world_section_carries_busy_timeout() {
        let section = crate::config::WorldSection {
            busy_timeout_ms: 12_345,
            ..crate::config::WorldSection::default()
        };
        let options: WorldDbOptions = (&section).into();
        assert_eq!(options.busy_timeout_ms, 12_345);
    }

    /// Companion guard for [`options_from_world_section_carries_busy_timeout`]:
    /// the journal_mode field also has to thread through the bridge.
    #[test]
    fn options_from_world_section_carries_journal_mode() {
        let section = crate::config::WorldSection {
            journal_mode: "delete".to_string(),
            ..crate::config::WorldSection::default()
        };
        let options: WorldDbOptions = (&section).into();
        assert_eq!(options.journal_mode, "delete");
    }

    /// SPEC_v2 §Task 3d acceptance: the configured journal mode is
    /// actually applied. Reading `PRAGMA journal_mode` after open
    /// confirms either the requested mode (the WAL happy path) or the
    /// documented fallback. Test files live on the local filesystem
    /// where WAL is supported, so the assertion is the strict form.
    #[test]
    fn open_with_options_applies_wal_journal_mode() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let world = WorldDb::open_with_options(&db_path, WorldDbOptions::default())
            .expect("open succeeds with default options");

        assert_eq!(
            world.journal_mode(),
            "wal",
            "default options must put the connection in WAL mode on a local fs"
        );

        // Round-trip the pragma directly, not just our cached field, to
        // catch a future regression where `journal_mode()` lies because
        // the open path forgot to issue the pragma.
        let active: String = world
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal_mode pragma is queryable");
        assert_eq!(active, "wal");
    }

    /// A non-WAL mode round-trips end-to-end. Picking `delete`
    /// specifically because it's SQLite's pre-WAL default — if the
    /// open path silently ignored our value, the assertion would still
    /// pass for `delete` and we'd ship a broken knob. The companion
    /// `truncate` assertion guards against that by exercising a value
    /// SQLite would never default to.
    #[test]
    fn open_with_options_applies_truncate_journal_mode() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let world = WorldDb::open_with_options(
            &db_path,
            WorldDbOptions {
                journal_mode: "truncate".to_string(),
                ..WorldDbOptions::default()
            },
        )
        .expect("open succeeds with truncate journal mode");

        assert_eq!(world.journal_mode(), "truncate");
    }

    /// Case folding and whitespace tolerance: `WAL ` should behave as
    /// `wal`. Authors who hand-edit `assets/game.toml` shouldn't get
    /// punished for a stray capital or trailing space.
    #[test]
    fn journal_mode_is_normalized_before_apply() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let world = WorldDb::open_with_options(
            &db_path,
            WorldDbOptions {
                journal_mode: " WAL ".to_string(),
                ..WorldDbOptions::default()
            },
        )
        .expect("open succeeds with whitespace-padded uppercase WAL");

        assert_eq!(world.journal_mode(), "wal");
    }

    /// Unknown journal modes are rejected before SQLite sees them. The
    /// allowlist is the only thing that keeps the value from being
    /// interpolated into a SQL pragma string, so a regression that
    /// removed the check would also be a quiet SQL-injection risk.
    #[test]
    fn invalid_journal_mode_is_rejected() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let err = WorldDb::open_with_options(
            &db_path,
            WorldDbOptions {
                journal_mode: "definitely-not-a-mode".to_string(),
                ..WorldDbOptions::default()
            },
        )
        .expect_err("nonsense journal modes must not silently fall through");

        match err {
            WorldDbError::InvalidJournalMode { requested } => {
                assert_eq!(requested, "definitely-not-a-mode");
            }
            other => panic!("expected InvalidJournalMode, got {other:?}"),
        }
    }

    /// SPEC_v2 §Task 4a acceptance: a freshly-opened world DB has the
    /// `world_migrations` bookkeeping table in place. Task 4b builds on
    /// this by recording rows; this test only verifies the table exists
    /// and matches the documented column shape so a future regression
    /// renaming a column flunks here rather than in a higher-level test
    /// where the failure mode is harder to attribute.
    #[test]
    fn open_creates_world_migrations_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        let world = WorldDb::open(&db_path).expect("open succeeds");

        // sqlite_master is the canonical "does this table exist" probe.
        // Asserting on the count rather than `Option<String>` keeps the
        // assertion trivial even if SQLite ever stores the table name in
        // mixed case.
        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'world_migrations'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(count, 1, "world_migrations table must exist after open");

        // Spot-check the columns so a future schema drift (e.g.
        // dropping `checksum` because Task 4b/4c forgot it was on the
        // contract) flunks here. `pragma_table_info` returns one row per
        // column with `name` in column index 1.
        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('world_migrations') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "version".to_string(),
                "name".to_string(),
                "checksum".to_string(),
                "applied_at".to_string(),
            ],
            "world_migrations schema must match the documented shape"
        );
    }

    /// Reopening the same DB file must not error or duplicate the
    /// migrations table — `IF NOT EXISTS` is the only thing standing
    /// between the relaunch path (SPEC §7 `external_pty` re-exec) and a
    /// loud `table already exists` failure on every restart.
    #[test]
    fn reopening_existing_db_keeps_migrations_table_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");

        // First open writes the table; drop the handle to release the
        // file so the second open mirrors a real process restart.
        drop(WorldDb::open(&db_path).expect("first open succeeds"));
        let world = WorldDb::open(&db_path).expect("reopen succeeds");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'world_migrations'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "reopen must leave exactly one world_migrations table"
        );
    }

    /// SPEC_v2 §Task 4b acceptance: applying a [`WorldMigration`]
    /// records its version in `world_migrations` and runs the SQL
    /// body. Two assertions, one test: the row is present *and* the
    /// migration's side effect (a created table) is observable. Either
    /// failing on its own would mean the apply path is half-broken,
    /// so the joint assertion catches both regressions in one place.
    #[test]
    fn apply_migration_records_version_and_runs_sql() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        let migration = WorldMigration {
            version: 1,
            name: "create_demo_table",
            sql: "CREATE TABLE demo (id INTEGER PRIMARY KEY, label TEXT NOT NULL);",
        };

        world
            .apply_migration(&migration)
            .expect("first application of a fresh migration succeeds");

        // Bookkeeping row is present with the version we asked for.
        let recorded: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params!["create_demo_table"],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(recorded, 1, "applied version must be recorded verbatim");

        // SQL body actually ran — the table it created is now visible
        // in sqlite_master. Without this leg, a regression that records
        // the row but skips `execute_batch` would still pass.
        let table_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'demo'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            table_count, 1,
            "migration SQL body must have executed against the connection"
        );
    }

    /// SPEC_v2 §Task 3b acceptance: a nested `world/world.sqlite`
    /// path whose parent directory does not yet exist is created on
    /// open rather than rejected. This is the SPEC §10.4 packaging
    /// shape — a fresh install ships `world/.keep` (later tasks) and
    /// authors should not need an extra `mkdir -p` step.
    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tempdir().expect("tempdir creates");
        // Two levels of missing parents to exercise `create_dir_all`,
        // not just a single `mkdir`.
        let nested = dir.path().join("world").join("nested").join("world.sqlite");

        assert!(
            !nested.parent().unwrap().exists(),
            "precondition: nested parent must not yet exist"
        );

        let world = WorldDb::open(&nested).expect("open creates parents and succeeds");
        assert!(nested.exists(), "SQLite file lands at the requested path");
        assert!(
            nested.parent().unwrap().is_dir(),
            "the full parent chain is created"
        );

        // Sanity: the connection is still usable after the parent
        // dance, so 3a's contract is preserved.
        let one: i64 = world
            .connection()
            .query_row("SELECT 1", [], |row| row.get(0))
            .expect("connection works after parent creation");
        assert_eq!(one, 1);
    }
}
