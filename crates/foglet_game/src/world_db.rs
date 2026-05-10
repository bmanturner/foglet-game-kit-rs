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

    /// Borrow the underlying SQLite [`Connection`] for game-authored
    /// reads and writes against tables the *game* owns (i.e. tables
    /// declared by a game-authored [`WorldMigration`] like Murder
    /// Motel's `motel_world_state`). SPEC_v2 §Task 12 requires games
    /// to query their own schema to back features like "did anyone
    /// open Room 7 yet?", and the existing curated helpers
    /// (`upsert_player`, `append_event`, leaderboards, turns) only
    /// cover *kit-owned* tables. This method is the documented escape
    /// hatch for the game half.
    ///
    /// **Use only on tables your own migration created.** Touching
    /// kit-owned tables (`players`, `turn_ledger`, `world_events`,
    /// `leaderboard_scores`, `world_migrations`) through this borrow
    /// is unsupported — kit invariants (e.g. partial unique indexes on
    /// `players`, append-only ordering on `world_events`) live inside
    /// the curated helpers, and bypassing them turns silent data
    /// drift into the most likely failure mode. Use the curated
    /// helpers for kit tables and reserve this entry point for
    /// game-authored ones.
    ///
    /// Returns a shared (`&Connection`) borrow on purpose: it matches
    /// the runtime contract where [`crate::screen::GameContext::world_db`]
    /// hands screens an `Option<&WorldDb>`, so a game-authored read or
    /// single-statement write inside `tick`/`handle_input` is reachable
    /// without forcing the runtime to upgrade to a unique borrow.
    /// Multi-statement transactions still go through
    /// [`WorldDb::transaction`] and require `&mut self` — game code
    /// that needs that should run it from a context that owns the
    /// `WorldDb`, not from inside a screen callback.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Crate-internal mutable accessor to the underlying [`Connection`].
    ///
    /// Exposed so v3 multiplayer modules whose transactional helpers
    /// need to surface their own typed errors (e.g.
    /// [`crate::market::WorldDb::buy_listing`]) can open their own
    /// `rusqlite::Transaction` without funneling failures through
    /// [`WorldDbError`]. Deliberately `pub(crate)` rather than `pub` —
    /// an external caller that grabs `&mut Connection` could re-apply
    /// migrations, smuggle in schema changes, or roll back the
    /// idempotency bookkeeping. In-tree modules already speak the kit
    /// invariants and are reviewed alongside this accessor.
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Apply a single [`WorldMigration`], recording its version in
    /// `world_migrations` on success.
    ///
    /// Task 4b implemented the happy path; Task 4c (this iteration) layers
    /// **idempotency** on top: if the migration's `version` is already
    /// recorded in `world_migrations`, the call is a no-op — the SQL body
    /// is *not* re-run and no second row is written. This matches the
    /// `external_pty` relaunch dance in SPEC §7: every door re-exec walks
    /// the same migration list, so any other contract would either
    /// duplicate rows (PK violation today) or re-run mutating SQL on every
    /// startup (data corruption tomorrow).
    ///
    /// Idempotency is keyed on `version` alone, *not* `(version, name)`.
    /// SPEC §4.3 makes `version` the unique identity of a migration; the
    /// `name` is operator-facing prose. Authors are free to rename a
    /// migration ("init" → "v1_init") between releases without the runtime
    /// thinking the renamed migration is a new one to apply.
    ///
    /// Task 4d (this iteration) pins the **"failed migration leaves no
    /// row"** guarantee with an explicit test. The mechanism that delivers
    /// it is the wrapping `Connection::transaction`: if `execute_batch` or
    /// the bookkeeping `INSERT` returns an error, the early return drops
    /// `tx` without `commit()`, and `rusqlite::Transaction`'s `Drop` impl
    /// rolls the transaction back. The bookkeeping row is therefore never
    /// observable to the next `apply_migration` call — the relaunch path
    /// will retry the migration cleanly rather than silently skip a half-
    /// applied version.
    ///
    /// `&mut self` is required because [`Connection::transaction`] needs
    /// a unique borrow. The runtime layer (Task 10) wraps the world DB
    /// in `Option<WorldDb>` on `GameContext`, and authoring code reaches
    /// it through a `&mut` borrow scoped to a single screen tick — so
    /// the mutable signature here matches the call shape that's coming.
    pub fn apply_migration(&mut self, migration: &WorldMigration) -> Result<(), WorldDbError> {
        let WorldMigration { version, name, sql } = migration;

        // Idempotency check: if the version is already recorded, the
        // migration ran in a previous process and re-running the SQL
        // body would either duplicate a CREATE (without IF NOT EXISTS)
        // or, worse, replay a data-mutating step. Querying
        // `world_migrations` rather than (say) `sqlite_master` keeps
        // the contract centred on the bookkeeping table — a future
        // migration that *only* writes data would still be tracked.
        if migration_recorded(&self.conn, *version)? {
            return Ok(());
        }

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

    /// Run `f` inside a SQLite transaction, committing on `Ok` and
    /// rolling back on `Err`.
    ///
    /// This is the SPEC §3.1 "transaction helper for shared-world
    /// mutations" exposed for sibling modules (Task 9c will compose it
    /// for the spend-turn + mutate + append-event flow). The closure
    /// receives a borrowed [`rusqlite::Transaction`] so it can issue
    /// any number of statements that all observe-or-don't as a unit.
    ///
    /// # Why a closure rather than handing back a `Transaction`
    ///
    /// `rusqlite::Transaction` rolls back on `Drop` *unless* `commit()`
    /// has been called. Owning it from a higher-level module makes it
    /// far too easy to forget the commit and silently drop writes —
    /// the kind of bug that only shows up after a player notices their
    /// turn-spend "didn't take". By inverting the control flow we
    /// guarantee both branches: a clean `Ok` always commits, an `Err`
    /// always rolls back. There is no path that returns the closure's
    /// success without also flushing the transaction.
    ///
    /// # Error type
    ///
    /// The closure's error channel is the library-wide
    /// [`WorldDbError`]. Sibling modules that already speak `rusqlite`
    /// errors at the boundary (Task 4 migrations, Task 9c spend-turn
    /// helper) can map their own errors into a `WorldDbError` variant;
    /// callers that need to propagate a non-`WorldDbError` failure can
    /// stash it in [`WorldDbError::Transaction`] via the `source`
    /// field, but the common case is a closure that simply returns
    /// the same error type the rest of the world DB layer uses.
    ///
    /// # Borrow shape
    ///
    /// `&mut self` is required because [`Connection::transaction`]
    /// needs a unique borrow; this matches the call shape Task 10
    /// already plans for, where `GameContext` holds the world DB
    /// behind a `&mut` borrow scoped to a single screen tick.
    pub fn transaction<T, F>(&mut self, f: F) -> Result<T, WorldDbError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, WorldDbError>,
    {
        let tx = self
            .conn
            .transaction()
            .map_err(|source| WorldDbError::Transaction { source })?;

        // Run the caller's body. On `Err` we return early; `tx` drops
        // without `commit()`, and `rusqlite::Transaction`'s `Drop` impl
        // rolls the SQLite transaction back. This is the only path
        // that exists for closure failures — there is no "commit on
        // error" branch, intentionally.
        let value = f(&tx)?;

        tx.commit()
            .map_err(|source| WorldDbError::Transaction { source })?;

        Ok(value)
    }

    /// Spend a turn, run a caller-supplied world mutation, and append
    /// an event — all inside a single SQLite transaction. SPEC_v2
    /// §Task 9c.
    ///
    /// This is the composed primitive Murder Motel (Task 13) and
    /// future game code reach for when an action consumes a turn,
    /// touches game-specific state, and should be visible in the
    /// lobby bulletin. Pulling the three steps into one helper means
    /// authoring code never has to remember the begin/commit dance,
    /// and — critically — the *atomicity* contract is that all three
    /// steps land or none do:
    ///
    /// - **Insufficient turns.** The internal spend phase
    ///   short-circuits with [`crate::turns::TurnError::InsufficientTurns`] before
    ///   the closure or the event insert runs. The transaction rolls
    ///   back on the way out so the lazy `ensure_today_turns_on`
    ///   row-materialisation is the only work that touched SQLite —
    ///   which is exactly the desired post-condition (a player who
    ///   tried to spend with an empty balance sees no event, no
    ///   game-state change, and no balance change beyond the
    ///   already-zero today's row).
    /// - **Mutation closure failed.** The caller's closure returned
    ///   `Err`. The spend is rolled back too — the player did not
    ///   "lose" a turn for an action that didn't take effect. The
    ///   error surfaces as [`SpendAndEmitError::Mutation`].
    /// - **Event insert failed.** A SQL error or a malformed message
    ///   surfaces as [`SpendAndEmitError::Event`]. Both the spend
    ///   and the closure mutation roll back. (Message validation
    ///   actually runs *before* the transaction begins, so a bad
    ///   message never even opens one.)
    ///
    /// # Closure shape
    ///
    /// The closure receives a borrowed [`rusqlite::Transaction`] and
    /// must return `Result<(), rusqlite::Error>`. Game code typically
    /// runs one or more `tx.execute(...)` calls writing to its own
    /// game-state tables. Returning `rusqlite::Error` (rather than a
    /// game-specific error) keeps the helper minimal — game code
    /// that needs richer errors can catch them outside this call by
    /// pre-validating, or by mapping inside the closure into
    /// [`rusqlite::Error::SqliteFailure`] with an explanatory message.
    /// The Task 13 call sites do not need richer errors today; if
    /// that changes, the closure error type is the obvious knob to
    /// generalise.
    ///
    /// # Argument grouping
    ///
    /// The argument list is long because the operation is genuinely
    /// the cross-product of two existing primitives. Grouping the
    /// turn-spend args and the event args in their declaration order
    /// keeps the call site readable; if a future call site finds
    /// itself building these from a struct, that struct can be added
    /// without breaking the function signature.
    ///
    /// # Why message validation runs *outside* the transaction
    ///
    /// `validate_event_message` is a pure-Rust check. Running it
    /// before [`Connection::transaction`] means a malformed message
    /// fails without opening a SQLite transaction at all — no busy
    /// timeout impact, no cleanup branch to test. The transactional
    /// guarantee covers SQL-side failures; client-side validation
    /// stays where it is most efficient.
    #[allow(clippy::too_many_arguments)] // The argument list is the cross-product of two existing primitives (turn-spend + event-append) plus the closure; grouping into a struct would obscure the call site without adding type safety.
    pub fn spend_turn_and_emit<P, F>(
        &mut self,
        player_id: i64,
        amount: u32,
        daily_allowance: u32,
        carryover_max: u32,
        date_provider: &P,
        event_kind: &str,
        event_message: &str,
        event_metadata: Option<&str>,
        mutate: F,
    ) -> Result<SpendAndEmitOutcome, SpendAndEmitError>
    where
        P: crate::turns::DateProvider,
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<(), rusqlite::Error>,
    {
        // Pre-validate the message before we touch SQLite. A caller
        // bug here would otherwise pay for a `BEGIN` round-trip
        // before failing — and would also need a rollback path that
        // the simple "no transaction yet" branch sidesteps.
        crate::events::validate_event_message(event_message).map_err(SpendAndEmitError::Event)?;

        // Capture today once. The provider is asked exactly once so a
        // misbehaving implementation that returns different values on
        // successive calls can't desync the spend (which keys the row
        // by today) from any future "stamp the event with today"
        // logic that might be added.
        let today = date_provider.today();

        let tx = self
            .conn
            .transaction()
            .map_err(|source| SpendAndEmitError::Transaction { source })?;

        // Spend first so an `InsufficientTurns` short-circuit avoids
        // running the closure or appending the event. The early
        // return drops `tx` without `commit()`, and rusqlite's `Drop`
        // impl rolls back — which is the SPEC §Task 9c "insufficient
        // turns rolls back" guarantee.
        let ledger = crate::turns::spend_turns_on(
            &tx,
            player_id,
            amount,
            daily_allowance,
            carryover_max,
            &today,
        )
        .map_err(SpendAndEmitError::Turn)?;

        // Run the caller's world-mutation. A `rusqlite::Error` here
        // also rolls the spend back via the same drop-without-commit
        // path: the player isn't billed a turn for an action that
        // didn't land.
        mutate(&tx).map_err(|source| SpendAndEmitError::Mutation { source })?;

        // Append the event last. By this point we know the spend
        // succeeded and the closure committed its writes against the
        // same transaction; failing here rolls everything back as a
        // unit, preserving the bulletin/ledger invariant ("every
        // event corresponds to a real, persisted action").
        let event = crate::events::append_event_on(
            &tx,
            event_kind,
            Some(player_id),
            event_message,
            event_metadata,
        )
        .map_err(SpendAndEmitError::Event)?;

        tx.commit()
            .map_err(|source| SpendAndEmitError::Transaction { source })?;

        Ok(SpendAndEmitOutcome { ledger, event })
    }
}

/// Successful return value from [`WorldDb::spend_turn_and_emit`].
///
/// Bundles both observable side-effects so callers don't need a
/// follow-up read. The `ledger` row reflects the post-spend balance
/// (handy for "you have N turns left" UI) and the `event` carries the
/// canonical `id`/`created_at` SQLite assigned (handy for tests and
/// for future "scroll to the latest event" UI hooks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendAndEmitOutcome {
    /// Post-spend turn ledger row for the player. The `balance` field
    /// is `prior_balance - amount` (or freshly seeded from the daily
    /// allowance if today's row was being materialised for the first
    /// time inside this transaction).
    pub ledger: crate::turns::TurnLedgerRow,
    /// Newly-appended event row, including the autoincrement `id` and
    /// `created_at` SQLite assigned at insert time.
    pub event: crate::events::EventRecord,
}

/// Failure modes for [`WorldDb::spend_turn_and_emit`].
///
/// Wraps the three underlying error types — [`crate::turns::TurnError`],
/// [`crate::events::EventError`], and a `rusqlite::Error` from the
/// caller's mutation closure — plus a fourth variant for begin/commit
/// failures from the SQLite layer itself. Callers that only care
/// "did the action go through" can `.is_err()`; callers that want to
/// surface "you only have N turns left" specifically can match on
/// [`SpendAndEmitError::Turn`] and then on
/// [`crate::turns::TurnError::InsufficientTurns`].
#[derive(Debug, Error)]
pub enum SpendAndEmitError {
    /// Turn-ledger phase failed. The most common variant carried here
    /// is [`crate::turns::TurnError::InsufficientTurns`]; SQL-level
    /// failures (the row couldn't be read or written) surface as
    /// [`crate::turns::TurnError::Sqlite`]. The wrapping transaction
    /// has already rolled back by the time this variant is observed.
    #[error("turn-ledger phase of spend_turn_and_emit failed: {0}")]
    Turn(#[source] crate::turns::TurnError),

    /// The caller's mutation closure returned `Err`. The wrapping
    /// transaction has rolled back, so the spend itself is also
    /// undone — the player is not billed for an action whose
    /// closure failed.
    #[error("world-mutation closure inside spend_turn_and_emit failed: {source}")]
    Mutation {
        /// Underlying `rusqlite` error from the closure. The closure
        /// is expected to map any non-`rusqlite` error into a
        /// `SqliteFailure` with an explanatory message before
        /// returning, so this variant always carries a SQLite-shaped
        /// error.
        #[source]
        source: rusqlite::Error,
    },

    /// Event-append phase failed — either message validation
    /// (`EmptyMessage`/`MessageTooLong`) or the SQL `INSERT
    /// ... RETURNING`. Validation failures fail before the
    /// transaction even begins, so for those the rollback is a
    /// no-op; SQL failures roll back both the spend and the
    /// closure mutation.
    #[error("event-append phase of spend_turn_and_emit failed: {0}")]
    Event(#[source] crate::events::EventError),

    /// `Connection::transaction` or `Transaction::commit` returned an
    /// error. Distinct from [`Self::Turn`]/[`Self::Event`] so an
    /// operator-facing message can name the SQLite-layer failure
    /// rather than implying that one of the inner phases is the cause.
    #[error("spend_turn_and_emit transaction begin/commit failed: {source}")]
    Transaction {
        /// Underlying `rusqlite` error from `BEGIN` or `COMMIT`.
        #[source]
        source: rusqlite::Error,
    },
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

/// Returns `true` when a migration with `version` is already recorded
/// in the `world_migrations` bookkeeping table.
///
/// Pulled out of [`WorldDb::apply_migration`] so the idempotency check
/// (Task 4c) is named and individually testable. Lookup by primary key
/// is an index seek — cheap enough to do unconditionally on every
/// `apply_migration` call, which is what the relaunch path requires.
///
/// Errors are mapped to [`WorldDbError::ApplyMigration`] so a failure
/// here surfaces with the same operator-facing wording as the rest of
/// the migration apply path; the caller doesn't need to special-case
/// "the pre-check itself blew up".
fn migration_recorded(conn: &Connection, version: i64) -> Result<bool, WorldDbError> {
    // `SELECT 1 ... LIMIT 1` over the PK column is the canonical
    // existence probe in SQLite. We use `query_row` + `optional()` to
    // distinguish "row exists" (`Some`) from "row missing" (`None`)
    // without paying for an extra round-trip.
    use rusqlite::OptionalExtension;
    let row: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM world_migrations WHERE version = ?1",
            rusqlite::params![version],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| WorldDbError::ApplyMigration {
            version,
            name: String::new(),
            source,
        })?;
    Ok(row.is_some())
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

    /// `Connection::transaction` or `Transaction::commit` returned an
    /// error from inside [`WorldDb::transaction`]. Distinct from
    /// [`Self::ApplyMigration`] so the operator-facing error names
    /// "transaction" rather than implying a migration is in flight —
    /// Task 9c's spend-turn helper, for example, surfaces here when
    /// the SQLite layer itself rejects begin/commit.
    #[error("world database transaction failed: {source}")]
    Transaction {
        /// Underlying `rusqlite` error from begin or commit. Closure-
        /// originated failures don't land here — they propagate
        /// whatever variant the closure returned.
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

    /// SPEC_v2 §Task 4c acceptance: applying the same migration twice
    /// records exactly one row and leaves the schema valid.
    ///
    /// The second call deliberately uses a SQL body that *would* fail
    /// if it were re-executed — `CREATE TABLE demo (...)` without
    /// `IF NOT EXISTS` against an already-present table — so this test
    /// also proves the idempotency check short-circuits *before* the SQL
    /// body runs. A regression that records the row but still re-executes
    /// the body (or vice versa) would flunk one or the other assertion.
    #[test]
    fn apply_migration_is_idempotent_across_repeat_calls() {
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
            .expect("first application succeeds");
        world
            .apply_migration(&migration)
            .expect("second application is a no-op, not an error");

        // Exactly one bookkeeping row for this version.
        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE version = ?1",
                rusqlite::params![1_i64],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(
            row_count, 1,
            "repeat application must not duplicate the bookkeeping row"
        );

        // Schema is still valid: the `demo` table exists and is usable.
        // Inserting a row exercises the CHECK that the table wasn't
        // dropped/recreated by an accidental re-run of the SQL body.
        world
            .connection()
            .execute(
                "INSERT INTO demo (label) VALUES (?1)",
                rusqlite::params!["smoke"],
            )
            .expect("demo table is intact and writable after repeat apply");
    }

    /// Idempotency keys on `version`, not `(version, name)`. Renaming a
    /// migration between releases (e.g. `init` → `v1_init`) is a
    /// documentation tweak, not a new migration to apply — the runtime
    /// must treat the second call as a no-op rather than re-running the
    /// body. Without this guard, an author tidying up names would silently
    /// re-execute every migration on the next `external_pty` relaunch.
    #[test]
    fn apply_migration_idempotency_keys_on_version_not_name() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&WorldMigration {
                version: 1,
                name: "init",
                sql: "CREATE TABLE demo (id INTEGER PRIMARY KEY);",
            })
            .expect("first application succeeds");

        // Same version, new name, same body — should be a no-op even
        // though the name disagrees with the recorded row.
        world
            .apply_migration(&WorldMigration {
                version: 1,
                name: "v1_init",
                sql: "CREATE TABLE demo (id INTEGER PRIMARY KEY);",
            })
            .expect("rename of an applied migration is a no-op");

        // The recorded name reflects the *first* application — we did not
        // overwrite bookkeeping on the rename. This is the conservative
        // choice; if a future requirement needs to update names in place
        // it can do so explicitly rather than as a side effect of apply.
        let recorded_name: String = world
            .connection()
            .query_row(
                "SELECT name FROM world_migrations WHERE version = ?1",
                rusqlite::params![1_i64],
                |row| row.get(0),
            )
            .expect("recorded name is queryable");
        assert_eq!(recorded_name, "init");
    }

    /// SPEC_v2 §Task 4d acceptance: a migration whose SQL body is
    /// rejected by SQLite surfaces as a [`WorldDbError::ApplyMigration`]
    /// **and** leaves no row in `world_migrations`. Without this guard
    /// the relaunch path (SPEC §7 `external_pty` re-exec) could record a
    /// version that never actually ran, then silently skip it on the
    /// next startup — a half-applied migration that no operator tooling
    /// would ever notice.
    ///
    /// The body is invalid SQL (`NOT_A_KEYWORD ...`) so the parser fails
    /// before any side effects; pairing the error assertion with a
    /// `world_migrations` count keeps both halves of the guarantee
    /// (loud failure, clean state) in one place.
    #[test]
    fn apply_migration_failure_does_not_record_row() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        let bad = WorldMigration {
            version: 7,
            name: "broken_migration",
            sql: "NOT_A_KEYWORD totally_invalid_sql;",
        };

        let err = world
            .apply_migration(&bad)
            .expect_err("invalid SQL must surface as an error");

        match err {
            WorldDbError::ApplyMigration { version, name, .. } => {
                assert_eq!(version, 7, "error must echo the offending version");
                assert_eq!(
                    name, "broken_migration",
                    "error must echo the offending name"
                );
            }
            other => panic!("expected ApplyMigration, got {other:?}"),
        }

        // No row recorded — the wrapping transaction must have rolled back.
        // Counting the whole table (rather than `WHERE version = 7`) also
        // catches a regression that recorded the row under a different
        // version key.
        let row_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_migrations", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(
            row_count, 0,
            "failed migration must leave the bookkeeping table empty"
        );

        // And a subsequent successful apply at the same version works —
        // proving the rollback didn't leave SQLite in a state that blocks
        // retry. This is the property the relaunch path actually depends
        // on; without it, a transient SQL error would brick the door.
        let good = WorldMigration {
            version: 7,
            name: "broken_migration",
            sql: "CREATE TABLE recovered (id INTEGER PRIMARY KEY);",
        };
        world
            .apply_migration(&good)
            .expect("retry after a failed apply succeeds");

        let row_count_after: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_migrations", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(row_count_after, 1, "retry must record exactly one row");
    }

    /// SPEC_v2 §Task 9a acceptance: a successful closure inside
    /// [`WorldDb::transaction`] commits — the writes it issued are
    /// observable to the next read. Pairing the inside-the-closure
    /// `INSERT` with an outside-the-closure `SELECT` proves that the
    /// commit hand-off works: a regression that returned the closure's
    /// `Ok` without actually calling `tx.commit()` would leave the
    /// table empty (rusqlite rolls back on `Drop`) and flunk this test.
    #[test]
    fn transaction_commits_writes_on_ok() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        // Set up a target table outside the transaction so the test
        // covers the transaction wrapper itself, not table creation.
        world
            .connection()
            .execute_batch("CREATE TABLE demo (id INTEGER PRIMARY KEY, label TEXT NOT NULL);")
            .expect("create demo table");

        let returned: i64 = world
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO demo (label) VALUES (?1)",
                    rusqlite::params!["committed"],
                )
                .map_err(|source| WorldDbError::Transaction { source })?;
                Ok(42)
            })
            .expect("transaction body succeeds and commits");

        // The closure's `Ok` value is threaded back through the wrapper
        // unchanged — callers (Task 9c) rely on this to return the new
        // turn balance, the appended event id, etc.
        assert_eq!(returned, 42, "transaction must return the closure's value");

        // The write is visible after the transaction returns. This is
        // the load-bearing assertion for 9a: a wrapper that forgot to
        // call `tx.commit()` would see zero rows here.
        let row_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM demo", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            row_count, 1,
            "committed transaction must persist its writes"
        );

        let label: String = world
            .connection()
            .query_row("SELECT label FROM demo LIMIT 1", [], |row| row.get(0))
            .expect("label query runs");
        assert_eq!(label, "committed");
    }

    /// SPEC_v2 §Task 9b acceptance: a closure that returns `Err`
    /// rolls back. Any writes the closure issued before failing must
    /// not be observable after [`WorldDb::transaction`] returns, and
    /// the wrapper must propagate the original error verbatim.
    ///
    /// This is the load-bearing safety property of the helper: the
    /// Task 9c spend-turn flow assumes that a failed event-append
    /// undoes the turn deduction. A regression where the wrapper
    /// committed-on-error (or even left the transaction dangling so
    /// rusqlite's `Drop` rolled it back *but* the wrapper still
    /// returned `Ok`) would silently corrupt the ledger. We assert
    /// both the data side (zero rows after rollback) and the error
    /// side (the original error reaches the caller unchanged).
    #[test]
    fn transaction_rolls_back_on_closure_error() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .connection()
            .execute_batch("CREATE TABLE demo (id INTEGER PRIMARY KEY, label TEXT NOT NULL);")
            .expect("create demo table");

        // Construct a sentinel `WorldDbError` we can identify on the
        // way out. `ApplyMigration` is reused here purely as a tagged
        // carrier — the test asserts on the variant fields, not the
        // message — because it's a pre-existing variant with a
        // `String` payload and a numeric tag that survive the round
        // trip through the transaction wrapper unchanged.
        let result: Result<(), WorldDbError> = world.transaction(|tx| {
            // Issue a real write inside the transaction so the rollback
            // assertion below is meaningful — without this the test
            // would pass even if the wrapper committed-on-error.
            tx.execute(
                "INSERT INTO demo (label) VALUES (?1)",
                rusqlite::params!["should-not-persist"],
            )
            .map_err(|source| WorldDbError::Transaction { source })?;

            Err(WorldDbError::ApplyMigration {
                version: 9_001,
                name: "sentinel".to_string(),
                source: rusqlite::Error::InvalidQuery,
            })
        });

        match result {
            Err(WorldDbError::ApplyMigration { version, name, .. }) => {
                assert_eq!(version, 9_001, "original error version is preserved");
                assert_eq!(name, "sentinel", "original error name is preserved");
            }
            other => panic!("expected ApplyMigration sentinel error, got {other:?}"),
        }

        // The write inside the closure must be gone. This is the
        // assertion that would fail if the wrapper called `commit()`
        // on the error path.
        let row_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM demo", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            row_count, 0,
            "closure error must roll the transaction back; no rows should persist"
        );

        // And the connection is still usable afterwards — a botched
        // rollback would leave SQLite in a state where the next
        // transaction errors with `cannot start a transaction within
        // a transaction`.
        world
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO demo (label) VALUES (?1)",
                    rusqlite::params!["after-rollback"],
                )
                .map_err(|source| WorldDbError::Transaction { source })?;
                Ok(())
            })
            .expect("connection remains usable after a rolled-back transaction");

        let row_count_after: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM demo", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            row_count_after, 1,
            "follow-up transaction commits independently of the rolled-back one"
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

    /// SPEC_v2 §Task 9c acceptance: an `InsufficientTurns` rejection
    /// rolls back both the caller's world mutation and the would-be
    /// event append. The helper composes spend → mutate → append in a
    /// single transaction; if the very first step short-circuits with
    /// `InsufficientTurns`, neither of the later steps run, and any
    /// SQL the spend phase performed (the lazy ensure-row insert
    /// inside [`crate::turns::spend_turns_on`]) rolls back along with
    /// the rest of the transaction.
    ///
    /// We seed the player's today's row with `balance = 0` and
    /// `daily_allowance = 0` *outside* the helper, so the helper sees
    /// an existing row whose balance can't satisfy the spend guard
    /// and fails with `InsufficientTurns`. That setup also pins the
    /// rollback assertions to a stable starting state: any post-call
    /// `SELECT` should see exactly the row we seeded — no fresh
    /// allowance, no balance change, no event row, no mutation row.
    #[test]
    fn spend_turn_and_emit_rolls_back_on_insufficient_turns() {
        use crate::events::{EventError, WORLD_EVENTS_MIGRATION};
        use crate::players::PLAYERS_MIGRATION;
        use crate::turns::{FixedDateProvider, LocalDate, TurnError, TURN_LEDGER_MIGRATION};

        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        // Apply the kit migrations the helper depends on. Failing to
        // apply any of them would surface as a `Turn::Sqlite` error
        // rather than `InsufficientTurns`, which would mask the
        // assertion this test is making — apply them up front and
        // assert success so a regression in the migration pipeline
        // doesn't masquerade as a 9c regression.
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn ledger migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world events migration applies");

        // Game-side state table the closure will (try to) write to.
        // Using a separate table makes the rollback assertion
        // unambiguous: a non-zero count after the call would mean the
        // closure's INSERT survived the rollback.
        world
            .connection()
            .execute_batch(
                "CREATE TABLE motel_world_state \
                 (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("create motel_world_state");

        // Insert a player so the FK in turn_ledger / world_events
        // resolves. We hand-roll the insert rather than route through
        // `upsert_player` because all this test needs is a stable
        // `id` to key the ledger and event rows by.
        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'tester', 'user', 50, \
                 CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("insert test player");
        let player_id: i64 = world
            .connection()
            .query_row(
                "SELECT id FROM players WHERE handle = 'tester'",
                [],
                |row| row.get(0),
            )
            .expect("read player id");

        // Seed today's ledger row at balance=0 so the spend guard
        // (`WHERE balance >= ?`) cannot match. We could also rely on
        // the helper to materialise a fresh row at the configured
        // allowance and then drain it with a second call, but doing
        // it explicitly here keeps the test focused on 9c — it
        // doesn't transitively depend on Task 6f's seeding behavior
        // staying constant.
        let today = LocalDate::parse("2026-05-09").expect("valid local date");
        world
            .connection()
            .execute(
                "INSERT INTO turn_ledger \
                 (player_id, local_date, balance, daily_allowance) \
                 VALUES (?1, ?2, 0, 0)",
                rusqlite::params![player_id, today.as_str()],
            )
            .expect("seed empty turn_ledger row");

        let provider = FixedDateProvider::new(today.clone());

        // Track whether the closure ran so the assertion can
        // distinguish "closure ran and was rolled back" from "closure
        // never ran at all" — the SPEC_v2 contract is the *latter*
        // (insufficient short-circuits before the closure), but we
        // still assert the table is empty either way to guarantee
        // rollback even if a future refactor reorders the steps.
        let mut closure_ran = false;
        let closure_ran_ref = &mut closure_ran;

        let result = world.spend_turn_and_emit(
            player_id,
            1,
            // `daily_allowance = 0` and `carryover_max = 0` mean the
            // ensure-row path inside `spend_turns_on` is a true no-op
            // for our seeded row — it sees the existing row and
            // leaves it alone.
            0,
            0,
            &provider,
            "clue_inspected",
            "tester examined the bloody footprint",
            None,
            |tx| {
                *closure_ran_ref = true;
                tx.execute(
                    "INSERT INTO motel_world_state (key, value) \
                     VALUES ('room_7_opened_by', 'tester')",
                    [],
                )?;
                Ok(())
            },
        );

        // Helper must surface the SPEC_v2 §Task 6e variant unchanged.
        match result {
            Err(SpendAndEmitError::Turn(TurnError::InsufficientTurns {
                player_id: pid,
                balance,
                requested,
            })) => {
                assert_eq!(pid, player_id, "error names the offending player");
                assert_eq!(balance, 0, "balance is read back at zero");
                assert_eq!(requested, 1, "requested amount is echoed verbatim");
            }
            Err(other) => panic!("expected Turn(InsufficientTurns), got {other:?}"),
            Ok(outcome) => {
                panic!("expected InsufficientTurns rejection; got committed outcome {outcome:?}")
            }
        }

        // Closure should not have run — spend_turns_on short-circuits
        // before mutate is invoked. If a future refactor reorders the
        // pipeline, this assertion is the canary; the rollback
        // assertions below stay correct either way.
        assert!(
            !closure_ran,
            "insufficient-turn rejection must short-circuit before the closure"
        );

        // The seeded ledger row is unchanged. A regression where the
        // helper "ate" a turn even on rejection would change the
        // balance.
        let balance: i64 = world
            .connection()
            .query_row(
                "SELECT balance FROM turn_ledger \
                 WHERE player_id = ?1 AND local_date = ?2",
                rusqlite::params![player_id, today.as_str()],
                |row| row.get(0),
            )
            .expect("ledger row still present");
        assert_eq!(balance, 0, "rejected spend must not mutate balance");

        // The closure's would-be mutation never landed. This is the
        // load-bearing assertion of the test: even if a future
        // refactor reorders the steps so the closure runs first, the
        // surrounding transaction's rollback must still erase its
        // writes.
        let mutation_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM motel_world_state", [], |row| {
                row.get(0)
            })
            .expect("mutation count query runs");
        assert_eq!(
            mutation_count, 0,
            "rejected spend must roll back the closure's writes"
        );

        // No event row landed. SPEC §Task 7c contract: the bulletin
        // must not show events for actions that didn't happen.
        let event_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_events", [], |row| row.get(0))
            .expect("event count query runs");
        assert_eq!(
            event_count, 0,
            "rejected spend must not append an event row"
        );

        // Sanity: the connection is still usable. A botched rollback
        // would leave SQLite in a state where the next transaction
        // errors with "cannot start a transaction within a
        // transaction".
        let _ = EventError::EmptyMessage; // keep the import live for clarity even if unused
        world
            .transaction(|tx| {
                tx.execute("CREATE TABLE post_rollback_canary (id INTEGER)", [])
                    .map_err(|source| WorldDbError::Transaction { source })?;
                Ok(())
            })
            .expect("connection remains usable after a rolled-back spend_turn_and_emit");
    }

    /// Companion to the rollback test above: a successful call commits
    /// all three side-effects atomically. Documents the happy path so
    /// a regression that, say, dropped the `tx.commit()` (and rolled
    /// back successful spends) would surface here rather than in a
    /// higher-level Murder Motel integration test where the failure
    /// mode is harder to attribute.
    #[test]
    fn spend_turn_and_emit_commits_all_three_steps_on_success() {
        use crate::events::WORLD_EVENTS_MIGRATION;
        use crate::players::PLAYERS_MIGRATION;
        use crate::turns::{FixedDateProvider, LocalDate, TURN_LEDGER_MIGRATION};

        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn ledger migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world events migration applies");

        world
            .connection()
            .execute_batch(
                "CREATE TABLE motel_world_state \
                 (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("create motel_world_state");

        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'committer', 'user', 50, \
                 CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("insert test player");
        let player_id: i64 = world
            .connection()
            .query_row(
                "SELECT id FROM players WHERE handle = 'committer'",
                [],
                |row| row.get(0),
            )
            .expect("read player id");

        let today = LocalDate::parse("2026-05-09").expect("valid local date");
        let provider = FixedDateProvider::new(today.clone());

        let outcome = world
            .spend_turn_and_emit(
                player_id,
                1,
                3, // daily_allowance — the ensure-row path will seed
                0, // carryover_max
                &provider,
                "room_7_opened",
                "committer unlocked Room 7",
                Some(r#"{"room":7}"#),
                |tx| {
                    tx.execute(
                        "INSERT INTO motel_world_state (key, value) \
                         VALUES ('room_7_opened_by', 'committer')",
                        [],
                    )?;
                    Ok(())
                },
            )
            .expect("happy-path spend commits");

        // Ledger reflects the spend: seeded at 3, decremented by 1.
        assert_eq!(outcome.ledger.balance, 2, "post-spend balance");
        assert_eq!(outcome.ledger.player_id, player_id);
        assert_eq!(outcome.ledger.daily_allowance, 3);

        // Event carries the SQLite-assigned id and the verbatim
        // message we passed in.
        assert_eq!(outcome.event.kind, "room_7_opened");
        assert_eq!(outcome.event.player_id, Some(player_id));
        assert_eq!(outcome.event.message, "committer unlocked Room 7");
        assert_eq!(outcome.event.metadata.as_deref(), Some(r#"{"room":7}"#));

        // Closure mutation persisted.
        let mutation_value: String = world
            .connection()
            .query_row(
                "SELECT value FROM motel_world_state WHERE key = 'room_7_opened_by'",
                [],
                |row| row.get(0),
            )
            .expect("mutation row present after commit");
        assert_eq!(mutation_value, "committer");
    }

    /// Two more guardrails for the helper: the mutation closure's
    /// `Err` rolls everything back (including the spend), and a
    /// malformed event message fails before the transaction even
    /// begins. Combined with the rollback test above, these three
    /// cases pin the SPEC_v2 §Task 9c contract: the helper either
    /// commits all three side-effects, or none of them.
    #[test]
    fn spend_turn_and_emit_rolls_back_on_mutation_failure() {
        use crate::events::WORLD_EVENTS_MIGRATION;
        use crate::players::PLAYERS_MIGRATION;
        use crate::turns::{FixedDateProvider, LocalDate, TURN_LEDGER_MIGRATION};

        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn ledger migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world events migration applies");

        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'rollback', 'user', 50, \
                 CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("insert test player");
        let player_id: i64 = world
            .connection()
            .query_row(
                "SELECT id FROM players WHERE handle = 'rollback'",
                [],
                |row| row.get(0),
            )
            .expect("read player id");

        let today = LocalDate::parse("2026-05-09").expect("valid local date");
        let provider = FixedDateProvider::new(today.clone());

        let result = world.spend_turn_and_emit(
            player_id,
            1,
            5,
            0,
            &provider,
            "clue_inspected",
            "rollback inspected the lobby",
            None,
            // Closure fails by issuing intentionally malformed SQL.
            // The error is real `rusqlite::Error` so the helper's
            // mutation branch surfaces it as
            // `SpendAndEmitError::Mutation`.
            |tx| {
                tx.execute("THIS IS NOT VALID SQL", [])?;
                Ok(())
            },
        );

        match result {
            Err(SpendAndEmitError::Mutation { .. }) => {}
            other => panic!("expected Mutation error, got {other:?}"),
        }

        // Spend was rolled back: today's ledger row never persisted,
        // so the table is empty. (The lazy ensure-row insert that
        // ran inside the transaction is rolled back too.)
        let ledger_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM turn_ledger WHERE player_id = ?1",
                rusqlite::params![player_id],
                |row| row.get(0),
            )
            .expect("ledger count query runs");
        assert_eq!(
            ledger_count, 0,
            "mutation failure must roll back the lazy ensure-row insert too"
        );

        let event_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_events", [], |row| row.get(0))
            .expect("event count query runs");
        assert_eq!(event_count, 0, "no event should land on mutation failure");
    }
}
