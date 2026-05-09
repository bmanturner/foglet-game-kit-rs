//! `events` — shared-world append-only event log schema (SPEC_v2 §Task 7).
//!
//! Task 7a shipped the `world_events` migration. Subsequent sub-tasks
//! layer behavior on top of the schema introduced here:
//!
//! - 7b added `WorldDb::append_event` for inserting one row.
//! - 7c (this iteration) adds `WorldDb::recent_events(limit)` for the
//!   lobby bulletin (SPEC §3.1 / §13).
//! - 7d will add `WorldDb::player_events(player_id, limit)` for
//!   per-player history.
//! - 7e will add the message validation guard (empty / overlong
//!   rejection).
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

/// Failure modes for [`WorldDb::append_event`].
///
/// Library-internal `thiserror` shape — Task 10 will wrap these with
/// `anyhow` at the process boundary so the operator-facing message
/// stays a single sentence. Mirrors [`crate::players::PlayerError`]
/// and [`crate::turns::TurnError`] so all world-DB write paths surface
/// errors with the same shape.
///
/// Task 7e will add a `Validation` variant for the empty/overlong
/// guard; today the only failure mode is the SQL round-trip itself.
#[derive(Debug, Error)]
pub enum EventError {
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
    /// and `metadata` are optional. The kit deliberately does *not*
    /// validate `message` length or content here — Task 7e adds the
    /// empty/overlong guard in its own commit so the bisect signal
    /// stays sharp. Until that lands, callers are trusted to pass
    /// game-authored strings (which is the SPEC §4.7 contract anyway).
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
        // `RETURNING` echoes every column in the same order the
        // migration declares them so [`row_to_event_record`] can be
        // shared with future read helpers (Tasks 7c, 7d) without each
        // one redeclaring the column list. A regression that reorders
        // the migration columns will flunk the schema test in this
        // module before this decoder even runs.
        const SQL: &str = "\
INSERT INTO world_events (kind, player_id, message, metadata) \
VALUES (?1, ?2, ?3, ?4) \
RETURNING id, created_at, kind, player_id, message, metadata";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![kind, player_id, message, metadata],
                row_to_event_record,
            )
            .map_err(|source| EventError::Sqlite { source })
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
}
