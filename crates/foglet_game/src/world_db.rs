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
//! Task 3b (this commit) extends the open path so missing parent
//! directories are created up-front. It still deliberately does not:
//!
//! - apply the `[world].busy_timeout_ms` config — Task 3c;
//! - apply the `[world].journal_mode` config — Task 3d;
//! - run any migrations — Task 4.
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

use rusqlite::Connection;
use thiserror::Error;

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
    /// (3c busy timeout, 3d journal mode, 9 transactions) can layer
    /// behavior on top without breaking callers that grabbed `&mut
    /// conn` directly.
    conn: Connection,
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
    /// # Out of scope for Task 3b
    ///
    /// This constructor still does not apply busy timeout (3c) or
    /// journal mode (3d). Those follow in their own iterations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorldDbError> {
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
        Ok(Self { conn })
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
