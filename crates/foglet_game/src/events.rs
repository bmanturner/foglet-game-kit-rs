//! `events` — shared-world append-only event log schema (SPEC_v2 §Task 7).
//!
//! Task 7a shipped the `world_events` migration. Subsequent sub-tasks
//! layer behavior on top of the schema introduced here:
//!
//! - 7b added `WorldDb::append_event` for inserting one row.
//! - 7c added `WorldDb::recent_events(limit)` for the lobby bulletin
//!   (SPEC §3.1 / §13).
//! - 7d added `WorldDb::player_events(player_id, limit)` for per-player
//!   history. Mirrors 7c's contract but constrains the result to one
//!   player via the `idx_world_events_player_recent` partial index.
//! - 7e (this iteration) adds the message validation guard
//!   (`validate_event_message`) so `append_event` rejects empty and
//!   overlong messages before they reach the SQL round-trip. The cap
//!   lives in [`MAX_EVENT_MESSAGE_LEN`] so game authors and the kit
//!   share one definition.
//!
//! Splitting the migration into its own commit keeps the bisect signal
//! sharp: a regression that drops a column flunks the schema test in
//! this module rather than a higher-level append/query assertion that's
//! harder to attribute. The migration is exported as a `pub const` so
//! the runtime startup path (Task 10) and game-author code can reference
//! one canonical definition without redeclaring the schema and drifting
//! from it — same pattern as [`crate::players::PLAYERS_MIGRATION`] and
//! [`crate::turns::TURN_LEDGER_MIGRATION`].
//!
//! # Why a dedicated table instead of folding events onto another row
//!
//! SPEC_v2 §4.7 mandates four behaviors that all assume an immutable
//! sequence of records:
//!
//! 1. Append a new entry.
//! 2. Query the most recent entries globally (lobby bulletin).
//! 3. Query the most recent entries for one player (per-player history).
//! 4. Never lose an entry once written.
//!
//! Folding events onto `players` (e.g. as a JSON column of "recent
//! actions") would force every read to deserialize the entire blob and
//! every write to rewrite it — a write-amplification problem that gets
//! worse as the history grows. A dedicated append-only table makes
//! "newest 20 globally" an `ORDER BY` over a covering index and keeps
//! the per-player query an index seek on `(player_id, created_at)`.
//!
//! It also gives operators a queryable audit trail: `sqlite3
//! world.sqlite 'SELECT created_at, kind FROM world_events ORDER BY id
//! DESC LIMIT 20'` is the cold-debug story for "what just happened in
//! this door". That's not a SPEC requirement, but it's a free
//! side-effect of the keying choice and worth not throwing away.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the shared-world event log — SPEC_v2 §4.7 / §Task 7a.
///
/// One row per emitted event. Rows are append-only by convention: no
/// authoring API in v2 will expose an `UPDATE` or `DELETE`. SQLite does
/// not enforce this at the storage layer (the table is a regular
/// `INTEGER PRIMARY KEY` table, not WAL-frozen) so the contract lives in
/// the helpers Tasks 7b–7e expose. An operator with a `sqlite3` shell
/// can still rewrite history; the kit's contract is "if you only go
/// through the public API, the log is append-only".
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid. Used as
///   the deterministic tiebreaker for `recent_events` (Task 7c) when two
///   events share the same `created_at` text — the SPEC's "newest first
///   with deterministic tie ordering" rule resolves to `ORDER BY
///   created_at DESC, id DESC`.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. Stored as ISO text so
///   it's human-readable in the `sqlite3` CLI and sorts lexically the
///   same way it sorts chronologically. Defaulted at the SQL layer so
///   `append_event` (Task 7b) doesn't have to thread a clock.
/// - `kind` — `TEXT NOT NULL`. Short machine-readable label
///   (e.g. `"room_7_opened"`, `"clue_found"`). Game authors pick the
///   namespace; the kit's only rule is "it must round-trip as text".
/// - `player_id` — `INTEGER REFERENCES players(id)`, **nullable**. SPEC
///   §4.7 explicitly types this column as optional so the log can carry
///   "system" events that aren't attributable to one player (e.g. a
///   future "midnight reset" tick the runtime might emit). Foreign-keyed
///   for the same reason as the turn ledger: a phantom id should never
///   land here. SQLite enforces FKs only when `PRAGMA foreign_keys = ON`,
///   which the runtime layer (Task 10) is responsible for; until then the
///   constraint is documentation but the column shape is already correct.
/// - `message` — `TEXT NOT NULL`. Game-authored display string —
///   "@alice opened Room 7". §4.7 calls out that messages are
///   game-authored, **not raw terminal transcripts**. Task 7e installs a
///   length/empty guard so the kit can't be tricked into storing
///   pathological values.
/// - `metadata` — `TEXT`, nullable. Optional JSON object (per the
///   SPEC's `metadata_json` field). Stored as text rather than `BLOB`
///   so an operator inspecting the file with `sqlite3 -json` can pretty-
///   print it without a hex dump. The kit treats this column as opaque;
///   serialization happens in the Task 7b helper.
///
/// # Indexes
///
/// Two covering indexes are created up-front so the Task 7c/7d query
/// patterns are seek-bound from the moment they land. Adding them later
/// would require a follow-up migration and a backfill window where the
/// query path scans the table; we avoid that by paying the index cost
/// at the same migration that creates the table:
///
/// - `idx_world_events_recent` covers `ORDER BY created_at DESC, id DESC`
///   for `recent_events`. SQLite's BTREE indexes are bidirectional, so
///   declaring the columns in ascending order is sufficient — the
///   planner walks the index backwards for the descending sort. Naming
///   the columns in the order the query uses them is a defensive habit
///   that keeps the index intent legible.
/// - `idx_world_events_player_recent` is a partial index over
///   `(player_id, created_at, id)` `WHERE player_id IS NOT NULL`. The
///   partial predicate keeps the index small (it skips system events
///   with `NULL` player) and matches the Task 7d query exactly:
///   `WHERE player_id = ? ORDER BY created_at DESC, id DESC`.
///
/// # Version
///
/// `version = 4`. Versions 1–3 are reserved for prior kit migrations
/// (1 reserved, 2 = players, 3 = turn_ledger). Game-authored migrations
/// (Murder Motel's `motel_world_state` from Task 12a) start from a higher
/// band so they don't collide with kit migrations the runtime applies on
/// every open.
pub const WORLD_EVENTS_MIGRATION: WorldMigration = WorldMigration {
    version: 4,
    name: "create_world_events",
    sql: "\
CREATE TABLE IF NOT EXISTS world_events (\n\
    id          INTEGER PRIMARY KEY,\n\
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    kind        TEXT NOT NULL,\n\
    player_id   INTEGER REFERENCES players(id),\n\
    message     TEXT NOT NULL,\n\
    metadata    TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_world_events_recent\n\
    ON world_events(created_at, id);\n\
CREATE INDEX IF NOT EXISTS idx_world_events_player_recent\n\
    ON world_events(player_id, created_at, id) WHERE player_id IS NOT NULL;\n\
",
};

/// Decoded `world_events` row — SPEC_v2 §4.7 read model.
///
/// Mirrors the column shape pinned by [`WORLD_EVENTS_MIGRATION`]. The
/// runtime layer (Task 10) and Murder Motel screens (Task 13) consume
/// this struct rather than reaching into raw `rusqlite::Row`s — that
/// keeps the schema-to-Rust mapping in one place and turns a column
/// rename into a single compile error instead of a fan-out of runtime
/// decode failures.
///
/// `created_at` and `metadata` stay as raw SQLite text. The kit stores
/// the timestamp as ISO text precisely so the operator-facing
/// `sqlite3` story (see the module docs) reads the same value the
/// runtime sees; parsing it into a richer type would be a one-way trip
/// that hides corrupt data instead of surfacing it. `metadata` is
/// likewise opaque text — Task 7b does not own JSON serialization, and
/// the kit treats the column as "whatever the caller put there".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    /// Autoincrement primary key. Doubles as the deterministic
    /// tiebreaker for `recent_events` (Task 7c) when two rows share a
    /// `created_at` value at second resolution.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text — see struct docs for
    /// rationale.
    pub created_at: String,
    /// Game-authored kind label (e.g. `"room_7_opened"`). Round-tripped
    /// verbatim; the kit imposes no namespace.
    pub kind: String,
    /// Player attribution. `None` for "system" events the runtime emits
    /// without a player on whose behalf they acted (SPEC §4.7 calls
    /// this case out explicitly).
    pub player_id: Option<i64>,
    /// Display string the lobby bulletin / per-player history will
    /// render. Game-authored — never a raw transcript.
    pub message: String,
    /// Optional opaque metadata blob (typically a JSON object). Stored
    /// as text so `sqlite3 -json` can pretty-print it; the kit does
    /// not parse it.
    pub metadata: Option<String>,
}

/// Maximum allowed character length for an event message — SPEC_v2
/// §Task 7e cap.
///
/// Counted as Unicode scalar values via [`str::chars`] rather than
/// bytes, because the lobby bulletin renders by visible characters and
/// a byte cap would arbitrarily punish non-ASCII handles
/// (e.g. "@玲" costs three bytes per character). The number itself —
/// 500 — is chosen to comfortably exceed a few wrapped lines on an
/// 80-column terminal (the SPEC §13.1 minimum) while still rejecting
/// pathological multi-megabyte inputs that could DOS the bulletin
/// query path or eat operator disk in seconds. SPEC_v2 §4.7 deliberately
/// leaves the cap to the kit; pinning it here lets game authors call
/// `MAX_EVENT_MESSAGE_LEN` rather than re-derive it from a magic number
/// in this module.
pub const MAX_EVENT_MESSAGE_LEN: usize = 500;

/// Failure modes for [`WorldDb::append_event`].
///
/// Library-internal `thiserror` shape — Task 10 will wrap these with
/// `anyhow` at the process boundary so the operator-facing message
/// stays a single sentence. Mirrors [`crate::players::PlayerError`]
/// and [`crate::turns::TurnError`] so all world-DB write paths surface
/// errors with the same shape.
#[derive(Debug, Error)]
pub enum EventError {
    /// The supplied message was empty (or whitespace-only). SPEC §4.7
    /// describes messages as game-authored display strings; a blank row
    /// has no useful UI rendering and almost certainly indicates a
    /// caller bug (forgot to substitute a template variable, etc.).
    /// Failing fast at the kit boundary keeps the bug visible instead
    /// of silently filling the bulletin with empty entries.
    #[error("event message must not be empty or whitespace-only")]
    EmptyMessage,
    /// The supplied message exceeded [`MAX_EVENT_MESSAGE_LEN`] characters.
    /// Carrying both the offending length and the cap in the variant
    /// gives operator-facing logs ("got 4096, max 500") without forcing
    /// the caller to recompute either value.
    #[error("event message too long: {len} chars exceeds max of {max}")]
    MessageTooLong {
        /// The character count of the rejected message — measured as
        /// Unicode scalar values (`chars().count()`), the same unit the
        /// cap is expressed in.
        len: usize,
        /// The cap the message exceeded. Mirrors
        /// [`MAX_EVENT_MESSAGE_LEN`] at the time of the rejection so
        /// the error survives a future config knob without rewriting
        /// the message.
        max: usize,
    },
    /// The `INSERT … RETURNING` round-trip failed. Wrapping
    /// `rusqlite::Error` keeps the call site readable (one error type,
    /// one mapping) while preserving the underlying cause for
    /// `tracing` and operator-facing messages.
    #[error("failed to append event to world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the insert statement.
        #[source]
        source: rusqlite::Error,
    },
}

/// Validate `message` against the SPEC_v2 §Task 7e guard rails — empty
/// rejection and the [`MAX_EVENT_MESSAGE_LEN`] cap.
///
/// Pulled out of [`WorldDb::append_event`] so future paths that emit
/// events through a different surface (e.g. the Task 9c spend-turn +
/// append-event transaction helper) can share one validator instead of
/// reimplementing the rule and drifting. The function is `pub(crate)`
/// because callers outside the world-DB module shouldn't be inventing
/// their own validation — they should go through `append_event`.
///
/// "Empty" is interpreted as `trim().is_empty()`: a message of `" "`
/// or `"\n"` would render as a blank line in the bulletin, which is
/// indistinguishable from a missing event and almost always a caller
/// bug. Failing both literal empty and whitespace-only with the same
/// error keeps the failure mode legible for operators.
pub(crate) fn validate_event_message(message: &str) -> Result<(), EventError> {
    if message.trim().is_empty() {
        return Err(EventError::EmptyMessage);
    }
    // Counted as `chars()` rather than `len()` so the cap is in
    // user-visible characters, not UTF-8 bytes. See the const docs for
    // why that matters for non-ASCII handles.
    let len = message.chars().count();
    if len > MAX_EVENT_MESSAGE_LEN {
        return Err(EventError::MessageTooLong {
            len,
            max: MAX_EVENT_MESSAGE_LEN,
        });
    }
    Ok(())
}

impl WorldDb {
    /// Append one row to `world_events` and return the canonical
    /// [`EventRecord`] SQLite produced (SPEC_v2 §4.7 / §Task 7b).
    ///
    /// The contract is "the row I asked you to insert is now durably
    /// in the log, with the id and timestamp the database assigned".
    /// We use SQLite's `RETURNING` clause (≥ 3.35) so the caller gets
    /// the autoincrement `id` and the SQL-side `CURRENT_TIMESTAMP`
    /// without a second round-trip — the same pattern as
    /// [`Self::upsert_player`].
    ///
    /// `kind` and `message` are required by the schema; `player_id`
    /// and `metadata` are optional. `message` is validated before the
    /// SQL round-trip — empty, whitespace-only, or longer than
    /// [`MAX_EVENT_MESSAGE_LEN`] inputs fail fast with
    /// [`EventError::EmptyMessage`] or [`EventError::MessageTooLong`]
    /// (SPEC_v2 §Task 7e). `kind` and
    /// `metadata` are intentionally not validated: `kind` is a
    /// game-authored namespace and `metadata` is opaque text whose
    /// shape the kit doesn't own.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the insert is a single statement, so the busy
    /// timeout configured at open time is the only contention story
    /// we need. `&mut self` would fight the runtime layer (Task 10)
    /// where `GameContext` borrows the world DB once per tick.
    pub fn append_event(
        &self,
        kind: &str,
        player_id: Option<i64>,
        message: &str,
        metadata: Option<&str>,
    ) -> Result<EventRecord, EventError> {
        append_event_on(self.connection(), kind, player_id, message, metadata)
    }

    /// Return the `limit` most recently appended events, newest first
    /// (SPEC_v2 §4.7 / §Task 7c).
    ///
    /// Powers the Murder Motel lobby bulletin (Task 13d): "what's
    /// happened recently across this door". The ordering contract is
    /// `ORDER BY created_at DESC, id DESC` — newest timestamp wins,
    /// and within one timestamp the higher (later) `id` wins. The
    /// `id` tiebreaker matters because `created_at` is stored at
    /// `CURRENT_TIMESTAMP` second resolution; two events appended in
    /// the same second would otherwise sort non-deterministically.
    /// SPEC §Task 7c specifically requires deterministic tie ordering
    /// so the bulletin renders the same sequence on every refresh.
    ///
    /// The query is index-bound: `idx_world_events_recent` covers
    /// `(created_at, id)` ascending, and SQLite walks the BTREE
    /// backwards to satisfy the `DESC, DESC` sort without a temp
    /// b-tree sort. Even on a long-running door with hundreds of
    /// thousands of events, the bulletin read stays seek-bound.
    ///
    /// # Parameters
    ///
    /// `limit` is `u32`: large enough for any plausible bulletin size
    /// (the Murder Motel UI shows ~20 entries) and small enough that
    /// a lossless cast to SQLite's `i64` is trivial. A `usize`-typed
    /// parameter would invite confusion on 32-bit targets and a
    /// signed `i64` would force callers to think about negatives we
    /// don't accept. `0` is legal and returns an empty vec — it lets
    /// callers wire UI plumbing before the bulletin is sized.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single statement under the configured busy
    /// timeout, same as [`Self::append_event`]. The runtime layer
    /// (Task 10) will call this from the lobby render path; keeping
    /// the borrow shared lets `GameContext` share one world-DB
    /// reference across screens without a `RefCell` dance.
    pub fn recent_events(&self, limit: u32) -> Result<Vec<EventRecord>, EventError> {
        const SQL: &str = "\
SELECT id, created_at, kind, player_id, message, metadata \
FROM world_events \
ORDER BY created_at DESC, id DESC \
LIMIT ?1";

        let mut stmt = self
            .connection()
            .prepare(SQL)
            .map_err(|source| EventError::Sqlite { source })?;
        let rows = stmt
            .query_map(rusqlite::params![i64::from(limit)], row_to_event_record)
            .map_err(|source| EventError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| EventError::Sqlite { source })
    }

    /// Return the `limit` most recent events attributed to one player,
    /// newest first (SPEC_v2 §4.7 / §Task 7d).
    ///
    /// Powers per-player history surfaces — Murder Motel will use this
    /// to render "your last N actions" alongside the global lobby
    /// bulletin (Task 13d). The ordering contract matches
    /// [`Self::recent_events`]: `ORDER BY created_at DESC, id DESC` so
    /// two events appended in the same SQLite-second sort
    /// deterministically by their autoincrement id.
    ///
    /// "System" events with `NULL` `player_id` are deliberately
    /// excluded: they aren't attributable to anyone, so a player's
    /// per-player view should never surface them. The
    /// `idx_world_events_player_recent` partial index covers exactly
    /// this case (`WHERE player_id IS NOT NULL`), so even on a long-
    /// running door this query stays seek-bound.
    ///
    /// # Parameters
    ///
    /// `player_id` is the canonical id from [`crate::players::PlayerRecord`]
    /// — the same id `append_event` stores. Passing an unknown id is
    /// not an error: it simply returns an empty vec, which is the
    /// correct UI behavior for "this player has no events yet".
    ///
    /// `limit` mirrors [`Self::recent_events`]: `u32`, `0` is legal and
    /// returns an empty vec.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single statement under the configured busy
    /// timeout, same as [`Self::append_event`] and [`Self::recent_events`].
    pub fn player_events(
        &self,
        player_id: i64,
        limit: u32,
    ) -> Result<Vec<EventRecord>, EventError> {
        // Querying `WHERE player_id = ?` filters out the `NULL`
        // system-event rows automatically (SQL `=` with `NULL` is
        // `UNKNOWN`, which the `WHERE` treats as false). That matches
        // the partial-index predicate exactly so the planner can use
        // `idx_world_events_player_recent` without a residual filter.
        const SQL: &str = "\
SELECT id, created_at, kind, player_id, message, metadata \
FROM world_events \
WHERE player_id = ?1 \
ORDER BY created_at DESC, id DESC \
LIMIT ?2";

        let mut stmt = self
            .connection()
            .prepare(SQL)
            .map_err(|source| EventError::Sqlite { source })?;
        let rows = stmt
            .query_map(
                rusqlite::params![player_id, i64::from(limit)],
                row_to_event_record,
            )
            .map_err(|source| EventError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| EventError::Sqlite { source })
    }
}

/// Free-function form of [`WorldDb::append_event`] that operates on
/// any `&Connection` — including the `&Transaction` handed to a
/// closure inside [`WorldDb::transaction`] (since `rusqlite::Transaction`
/// derefs to `Connection`).
///
/// Pulled out so the SPEC_v2 §Task 9c spend-turn + mutate + append-event
/// helper can compose the validated event insert into a single
/// transaction with the turn spend without re-borrowing the
/// [`WorldDb`]. Validation runs first so a malformed message is rejected
/// before any SQL round-trip — and, when called from the 9c helper,
/// before the wrapping transaction has done any work.
pub(crate) fn append_event_on(
    conn: &rusqlite::Connection,
    kind: &str,
    player_id: Option<i64>,
    message: &str,
    metadata: Option<&str>,
) -> Result<EventRecord, EventError> {
    validate_event_message(message)?;

    const SQL: &str = "\
INSERT INTO world_events (kind, player_id, message, metadata) \
VALUES (?1, ?2, ?3, ?4) \
RETURNING id, created_at, kind, player_id, message, metadata";

    conn.query_row(
        SQL,
        rusqlite::params![kind, player_id, message, metadata],
        row_to_event_record,
    )
    .map_err(|source| EventError::Sqlite { source })
}

/// Decode a `world_events` row into [`EventRecord`].
///
/// Pulled out of the append call site so Task 7c/7d read helpers can
/// share one decoder. Column order matches the `RETURNING` clause in
/// [`WorldDb::append_event`] and the SPEC §4.7 schema; a regression
/// that reorders columns in the migration will surface here as a
/// `rusqlite` type error rather than a runtime panic in production.
fn row_to_event_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        id: row.get(0)?,
        created_at: row.get(1)?,
        kind: row.get(2)?,
        player_id: row.get(3)?,
        message: row.get(4)?,
        metadata: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v2 §Task 7a acceptance: applying [`WORLD_EVENTS_MIGRATION`]
    /// creates the documented `world_events` table with the column shape
    /// later sub-tasks (7b–7e) depend on. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.7 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 7b append-event test).
    ///
    /// We apply the players migration first because `world_events`
    /// references it via `FOREIGN KEY`. With FK enforcement off (the
    /// SQLite default until Task 10 turns it on) the migration would
    /// succeed even without the parent table, but exercising the real
    /// dependency order here mirrors how the runtime startup path will
    /// drive migrations on a real door open.
    #[test]
    fn migration_creates_world_events_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world_events migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'world_events'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "world_events table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('world_events') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "id".to_string(),
                "created_at".to_string(),
                "kind".to_string(),
                "player_id".to_string(),
                "message".to_string(),
                "metadata".to_string(),
            ],
            "world_events column shape must match the SPEC §4.7 contract"
        );
    }

    /// SPEC §4.7 calls out that `player_id` is optional so the log can
    /// carry "system" events. A regression that flipped the column to
    /// `NOT NULL` would silently force every future caller to invent a
    /// fake player id; pin the nullability explicitly.
    #[test]
    fn world_events_player_id_is_nullable() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world_events migration applies");

        // `pragma_table_info`'s `notnull` column is `1` for `NOT NULL`
        // columns and `0` otherwise. Querying it directly is more
        // robust than parsing the `sqlite_master.sql` text, whose
        // whitespace is implementation-defined.
        let notnull: i64 = world
            .connection()
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('world_events') \
                 WHERE name = 'player_id'",
                [],
                |row| row.get(0),
            )
            .expect("pragma_table_info reports player_id");
        assert_eq!(
            notnull, 0,
            "player_id must be nullable so system events can omit it"
        );

        // The same query for `kind` and `message` should report
        // `NOT NULL` — flunking here would mean we accidentally relaxed
        // the wrong constraint while fixing a different one.
        let kind_notnull: i64 = world
            .connection()
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('world_events') \
                 WHERE name = 'kind'",
                [],
                |row| row.get(0),
            )
            .expect("pragma_table_info reports kind");
        assert_eq!(kind_notnull, 1, "kind must remain NOT NULL");
    }

    /// The Task 7c/7d query plans rely on the indexes shipped with this
    /// migration. Asserting the indexes exist by name pins the contract
    /// without coupling the test to the SQL planner's choice of access
    /// path (which is implementation-defined and changes across SQLite
    /// versions). A regression that drops or renames an index flunks
    /// here, where the cause is obvious, instead of as a slow query in
    /// production.
    #[test]
    fn world_events_recent_and_player_indexes_exist() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world_events migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND tbl_name = 'world_events' \
                 ORDER BY name",
            )
            .expect("sqlite_master query preparable");
        let raw: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows decode");
        // Filter out the autoindex SQLite creates for the integer
        // primary key — its name is implementation-defined and not part
        // of the migration contract we want to pin.
        let indexes: Vec<String> = raw
            .into_iter()
            .filter(|name| !name.starts_with("sqlite_autoindex_"))
            .collect();

        assert_eq!(
            indexes,
            vec![
                "idx_world_events_player_recent".to_string(),
                "idx_world_events_recent".to_string(),
            ],
            "world_events migration must ship the recent + per-player indexes"
        );
    }

    /// Helper: build a minimally-populated [`FogletContext`] with just
    /// the identity bits the upsert path reads. Mirrors the same
    /// helper in `players::tests` so the per-module test bodies stay
    /// focused on the assertion under test rather than restating the
    /// full struct literal.
    fn ctx_with(user_id: Option<&str>, username: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "test-door".to_string(),
            user_id: user_id.map(str::to_string),
            username: username.map(str::to_string),
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::ContextFile,
        }
    }

    /// Helper: open a temp world DB and apply the migrations
    /// `world_events` depends on (players → world_events). Returns the
    /// open DB and the tempdir guard so callers can drop both in one
    /// `let _guard` move at the end of a test.
    fn open_world_with_events(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("world_events migration applies");
        world
    }

    /// SPEC_v2 §Task 7b acceptance: `append_event` durably stores the
    /// row with the supplied `kind` and `player_id`, and the returned
    /// [`EventRecord`] echoes the same values plus a SQLite-assigned
    /// `id` and `created_at`.
    ///
    /// Round-trip via a `SELECT` rather than trusting the `RETURNING`
    /// row alone — a regression where `append_event` accidentally
    /// `INSERT`ed nothing but synthesized a fake record from its
    /// arguments would still pass a "the returned struct matches my
    /// inputs" assertion. Reading the row back proves durability.
    #[test]
    fn append_event_stores_player_id_and_kind() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        let event = world
            .append_event(
                "room_7_opened",
                Some(alice.id),
                "@alice opened Room 7",
                None,
            )
            .expect("append_event succeeds");

        assert_eq!(event.kind, "room_7_opened");
        assert_eq!(event.player_id, Some(alice.id));
        assert_eq!(event.message, "@alice opened Room 7");
        assert_eq!(event.metadata, None);
        // SQLite assigns `id` from the INTEGER PRIMARY KEY; the first
        // row in an empty table is `1`. Pinning the value (rather than
        // just `> 0`) catches a regression that wires the wrong
        // `RETURNING` column into the decoder.
        assert_eq!(event.id, 1);
        // `created_at` defaults to `CURRENT_TIMESTAMP`. We don't pin
        // the exact value (it's wall-clock-dependent) but it must be
        // non-empty, which proves the schema default fired and the
        // decoder read the right column.
        assert!(!event.created_at.is_empty(), "created_at must be set");

        // Read it back through a fresh query — proves the row is
        // actually in the table, not just synthesized in-memory.
        let stored: (i64, String, Option<i64>, String) = world
            .connection()
            .query_row(
                "SELECT id, kind, player_id, message FROM world_events WHERE id = ?1",
                rusqlite::params![event.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("event row readable");
        assert_eq!(
            stored,
            (event.id, event.kind, event.player_id, event.message)
        );
    }

    /// SPEC §4.7 explicitly types `player_id` as optional so the log
    /// can carry "system" events with no player attribution. Pin that
    /// the append path accepts `None` and stores it as SQL `NULL` —
    /// otherwise a future caller emitting a midnight-reset tick would
    /// be forced to invent a fake player id.
    #[test]
    fn append_event_allows_null_player_id_for_system_events() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        let event = world
            .append_event("midnight_reset", None, "daily turns rolled over", None)
            .expect("append_event succeeds with no player");

        assert_eq!(event.player_id, None);

        // Confirm the column is stored as SQL NULL, not as the string
        // "None" or the integer `0` — both would silently pass the
        // `Option<i64>` decode in `EventRecord` if the column were a
        // legitimate row but corrupt the per-player query in Task 7d.
        let raw_is_null: bool = world
            .connection()
            .query_row(
                "SELECT player_id IS NULL FROM world_events WHERE id = ?1",
                rusqlite::params![event.id],
                |row| row.get(0),
            )
            .expect("null-check query runs");
        assert!(raw_is_null, "system event must store player_id as SQL NULL");
    }

    /// SPEC_v2 §Task 7c acceptance (empty case): `recent_events` on a
    /// fresh table returns an empty vec rather than erroring or
    /// returning a sentinel row. The lobby bulletin renders this case
    /// as "no events yet" and assumes a clean `Vec::is_empty()`.
    #[test]
    fn recent_events_empty_returns_empty_vec() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        let events = world.recent_events(10).expect("recent_events runs");
        assert!(events.is_empty(), "no rows yet → empty vec");

        // `limit = 0` is also legal and equally empty.
        let none = world.recent_events(0).expect("recent_events(0) runs");
        assert!(none.is_empty(), "limit=0 short-circuits to empty");
    }

    /// SPEC_v2 §Task 7c acceptance (ordering + tiebreak): newest events
    /// come first, and when two events share `created_at` (stored at
    /// SQLite's `CURRENT_TIMESTAMP` second resolution) the one with
    /// the higher `id` wins. Also pins that `limit` truncates the
    /// result.
    ///
    /// Seeding via direct SQL with explicit `created_at` is the only
    /// way to deterministically force a tie — the public
    /// `append_event` path uses the `CURRENT_TIMESTAMP` default and
    /// the test would otherwise depend on wall-clock granularity to
    /// produce two same-second rows.
    #[test]
    fn recent_events_returns_newest_first_with_id_tiebreak() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        // Two pairs of ties at two distinct timestamps. After the
        // inserts, ids 1..=4 map to ("first", "second", "third",
        // "fourth") in the order shown. Newest-first ordering should
        // surface them as fourth → third (newer ts pair, id desc),
        // then second → first (older ts pair, id desc).
        world
            .connection()
            .execute_batch(
                "INSERT INTO world_events (created_at, kind, message) VALUES \
                 ('2026-05-08 10:00:00', 'first',  'first');\n\
                 INSERT INTO world_events (created_at, kind, message) VALUES \
                 ('2026-05-08 10:00:00', 'second', 'second');\n\
                 INSERT INTO world_events (created_at, kind, message) VALUES \
                 ('2026-05-08 10:00:01', 'third',  'third');\n\
                 INSERT INTO world_events (created_at, kind, message) VALUES \
                 ('2026-05-08 10:00:01', 'fourth', 'fourth');",
            )
            .expect("seed events");

        let events = world.recent_events(10).expect("recent_events runs");
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["fourth", "third", "second", "first"],
            "newer created_at first; within a tie, higher id first"
        );

        // `limit` truncates from the newest end, not from a random
        // slice. A regression that ordered ascending and then
        // reversed in Rust (instead of letting SQLite do the sort)
        // would still pass the full-list assertion above but flunk
        // here because the truncated head would be the oldest two.
        let head = world.recent_events(2).expect("recent_events(2) runs");
        let head_kinds: Vec<&str> = head.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            head_kinds,
            vec!["fourth", "third"],
            "limit takes the newest N"
        );
    }

    /// SPEC_v2 §Task 7d acceptance: `player_events` filters by
    /// `player_id`, returns newest-first with the same id-tiebreak as
    /// `recent_events`, excludes `NULL`-player system events, and
    /// returns an empty vec for an unknown id.
    ///
    /// Seeds rows with explicit `created_at` so tie-ordering is
    /// deterministic — the public `append_event` path uses
    /// `CURRENT_TIMESTAMP` and the test would otherwise depend on
    /// wall-clock granularity to produce a same-second tie.
    #[test]
    fn player_events_filters_to_player_with_id_tiebreak() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");

        // Mix of alice events, a bob event, and a system (NULL) event.
        // Two of alice's rows share `created_at` so we can prove the
        // id-tiebreak on the per-player path matches `recent_events`.
        world
            .connection()
            .execute_batch(&format!(
                "INSERT INTO world_events (created_at, kind, player_id, message) VALUES \
                 ('2026-05-08 09:00:00', 'a1', {alice}, 'a1');\n\
                 INSERT INTO world_events (created_at, kind, player_id, message) VALUES \
                 ('2026-05-08 09:00:01', 'b1', {bob}, 'b1');\n\
                 INSERT INTO world_events (created_at, kind, player_id, message) VALUES \
                 ('2026-05-08 09:00:02', 'a2', {alice}, 'a2');\n\
                 INSERT INTO world_events (created_at, kind, player_id, message) VALUES \
                 ('2026-05-08 09:00:02', 'a3', {alice}, 'a3');\n\
                 INSERT INTO world_events (created_at, kind, message) VALUES \
                 ('2026-05-08 09:00:03', 'sys', 'midnight reset');",
                alice = alice.id,
                bob = bob.id,
            ))
            .expect("seed events");

        let alice_events = world
            .player_events(alice.id, 10)
            .expect("player_events runs");
        let kinds: Vec<&str> = alice_events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["a3", "a2", "a1"],
            "alice's events newest-first; same-second pair sorts by id desc"
        );
        // Every returned row is attributed to alice — proves the WHERE
        // clause is filtering and the system event was excluded.
        assert!(
            alice_events.iter().all(|e| e.player_id == Some(alice.id)),
            "all rows must be alice's"
        );

        // Bob sees only his single row — not alice's, not the system
        // event.
        let bob_events = world.player_events(bob.id, 10).expect("bob query runs");
        let bob_kinds: Vec<&str> = bob_events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(bob_kinds, vec!["b1"]);

        // `limit` truncates from the newest end, same as recent_events.
        let head = world.player_events(alice.id, 2).expect("limit query runs");
        let head_kinds: Vec<&str> = head.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(head_kinds, vec!["a3", "a2"]);

        // Unknown player id is not an error — it simply has no rows.
        // 9_999 is well above any id `upsert_player` assigned above.
        let none = world
            .player_events(9_999, 10)
            .expect("unknown id query runs");
        assert!(none.is_empty(), "unknown player id → empty vec");
    }

    /// SPEC_v2 §Task 7e acceptance (empty rejection): `append_event`
    /// rejects a literal empty message with [`EventError::EmptyMessage`]
    /// and writes nothing to the table. Pinning that the row count
    /// stays at zero proves the validator runs *before* the SQL
    /// round-trip — a regression that validated post-insert would still
    /// surface the error but leave a dangling row.
    #[test]
    fn append_event_rejects_empty_message() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);
        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");

        let err = world
            .append_event("clue_found", Some(alice.id), "", None)
            .expect_err("empty message must be rejected");
        assert!(
            matches!(err, EventError::EmptyMessage),
            "expected EmptyMessage, got {err:?}"
        );

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM world_events", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(count, 0, "rejected empty message must not persist a row");
    }

    /// SPEC §Task 7e acceptance (whitespace-only): a message of just
    /// spaces / tabs / newlines is treated as empty. The SPEC §4.7
    /// contract is "game-authored display string"; a blank-rendering
    /// row is indistinguishable from a missing event in the lobby
    /// bulletin and almost always a caller bug (forgot to substitute a
    /// template variable). Reject it the same way as a literal empty.
    #[test]
    fn append_event_rejects_whitespace_only_message() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        for blank in ["   ", "\t", "\n", " \t\n "] {
            let err = world
                .append_event("clue_found", None, blank, None)
                .expect_err("whitespace-only message must be rejected");
            assert!(
                matches!(err, EventError::EmptyMessage),
                "expected EmptyMessage for {blank:?}, got {err:?}",
            );
        }
    }

    /// SPEC §Task 7e acceptance (overlong rejection): a message longer
    /// than [`MAX_EVENT_MESSAGE_LEN`] characters is rejected with
    /// [`EventError::MessageTooLong`] carrying both the offending
    /// length and the cap. A message of exactly the cap is accepted —
    /// the boundary is `len > MAX`, not `>= MAX`, so authors can paste
    /// a known-good template right at the limit without surprise
    /// failures.
    #[test]
    fn append_event_rejects_overlong_message_and_accepts_cap_exactly() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        // One character over the cap → rejected. Build the string from
        // ASCII so the char-count and byte-count happen to match,
        // making the assertion's failure mode obvious if it triggers.
        let too_long: String = "a".repeat(MAX_EVENT_MESSAGE_LEN + 1);
        let err = world
            .append_event("clue_found", None, &too_long, None)
            .expect_err("overlong message must be rejected");
        match err {
            EventError::MessageTooLong { len, max } => {
                assert_eq!(len, MAX_EVENT_MESSAGE_LEN + 1);
                assert_eq!(max, MAX_EVENT_MESSAGE_LEN);
            }
            other => panic!("expected MessageTooLong, got {other:?}"),
        }

        // Exactly at the cap → accepted. Proves the boundary is `>` not
        // `>=` and pins it against an off-by-one regression.
        let at_cap: String = "a".repeat(MAX_EVENT_MESSAGE_LEN);
        world
            .append_event("clue_found", None, &at_cap, None)
            .expect("message at exactly the cap must be accepted");
    }

    /// SPEC §Task 7e acceptance (Unicode counting): the cap is
    /// expressed in characters (Unicode scalar values), not bytes. A
    /// non-ASCII message whose `len()` (bytes) exceeds the cap but
    /// whose `chars().count()` does not must be accepted — otherwise
    /// the kit silently penalizes non-ASCII handles.
    ///
    /// Pick a 3-byte-per-char glyph ("玲") and emit exactly
    /// `MAX_EVENT_MESSAGE_LEN` of them. Bytes = 3 × cap (well over
    /// any byte-cap we'd plausibly choose), chars = cap exactly →
    /// accepted.
    #[test]
    fn append_event_caps_by_chars_not_bytes() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_events(&dir);

        let glyph = "玲";
        assert_eq!(glyph.len(), 3, "test prerequisite: glyph is 3 bytes");
        let multi_byte: String = glyph.repeat(MAX_EVENT_MESSAGE_LEN);
        assert!(
            multi_byte.len() > MAX_EVENT_MESSAGE_LEN,
            "test prerequisite: byte length exceeds the char cap",
        );

        world
            .append_event("clue_found", None, &multi_byte, None)
            .expect("Unicode message at the char cap must be accepted");
    }
}
