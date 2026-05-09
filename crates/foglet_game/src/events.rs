//! `events` — shared-world append-only event log schema (SPEC_v2 §Task 7).
//!
//! Task 7a (this iteration) ships the `world_events` migration only.
//! Subsequent sub-tasks layer behavior on top of the schema introduced
//! here:
//!
//! - 7b adds `WorldDb::append_event` for inserting one row.
//! - 7c adds `WorldDb::recent_events(limit)` for the lobby bulletin
//!   (SPEC §3.1 / §13).
//! - 7d adds `WorldDb::player_events(player_id, limit)` for per-player
//!   history.
//! - 7e adds the message validation guard (empty / overlong rejection).
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

use crate::world_db::WorldMigration;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
