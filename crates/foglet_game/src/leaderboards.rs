//! `leaderboards` — shared-world named score tables (SPEC_v2 §Task 8).
//!
//! Task 8a shipped the `leaderboard_scores` migration. Subsequent
//! sub-tasks layer behavior on top of the schema introduced here:
//!
//! - 8b (this iteration) adds [`WorldDb::set_score`] for upserting one
//!   `(board, player_id)` row to an absolute value.
//! - 8c will add `increment_score` for delta updates.
//! - 8d will add `top_scores(name, n)` for the leaderboard render.
//! - 8e will add `player_rank(name, player_id)` for "you are #N".
//!
//! Splitting the schema commit from the helper commits keeps the
//! bisect signal sharp: a regression that drops a column or an index
//! flunks the schema test in this module rather than appearing as a
//! mysterious query failure inside a higher-level helper test.
//!
//! The migration is exported as a `pub const` so the runtime startup
//! path (Task 10) and game-author code can apply one canonical
//! definition without redeclaring the schema and drifting from it —
//! same pattern as [`crate::players::PLAYERS_MIGRATION`],
//! [`crate::turns::TURN_LEDGER_MIGRATION`], and
//! [`crate::events::WORLD_EVENTS_MIGRATION`].
//!
//! # Why a dedicated table per board, keyed by board name
//!
//! SPEC_v2 §4.8 mandates four behaviors for any leaderboard:
//!
//! 1. Set a player's score on a named board.
//! 2. Increment a player's score on a named board.
//! 3. Query the top N entries on a named board.
//! 4. Query one player's rank on a named board.
//!
//! All four are "by name" operations. A naive design might create one
//! SQL table per declared `[[leaderboards]]` entry, but the board set
//! is game-authored config (SPEC §5) and we don't want to issue
//! `CREATE TABLE` at runtime every time a game adds a board. Folding
//! every board into one table keyed by `(board, player_id)` keeps the
//! migration story stable: adding a board is a config edit, not a
//! schema change, and the kit's helpers query one table by `board`
//! parameter.
//!
//! # Why `(board, player_id)` is the natural primary key
//!
//! A player has at most one score on any given board. SPEC §4.8's
//! "set score" and "increment score" both target one row per
//! `(board, player_id)` pair, and a primary key over that pair gives
//! us:
//!
//! - O(1) upsert via `INSERT … ON CONFLICT(board, player_id) DO
//!   UPDATE` for `set_score` (8b) and `increment_score` (8c).
//! - O(1) `player_rank` lookup of one specific row before the rank
//!   computation runs.
//! - A single covering index for the `top_scores` query when paired
//!   with the secondary index below.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the shared-world leaderboard table — SPEC_v2 §4.8 /
/// §Task 8a.
///
/// One row per `(board, player_id)` pair. Rows are mutated in place by
/// `set_score` (8b) and `increment_score` (8c); unlike `world_events`
/// this table is not append-only.
///
/// # Column shape
///
/// - `board` — `TEXT NOT NULL`. Matches the `name` field of a
///   `[[leaderboards]]` config entry (e.g. `"investigators"`). Stored
///   as text so an operator inspecting the file with `sqlite3` reads
///   the same identifier the game-author wrote in `game.toml`. The
///   kit treats unknown board names as game-author bugs surfaced
///   through Task 10's runtime validation, not a SQL-layer constraint
///   — there is no enum table because the board set is owned by config.
/// - `player_id` — `INTEGER NOT NULL REFERENCES players(id)`. Same
///   foreign-key story as the turn ledger and event log: phantom ids
///   should never land here. SQLite enforces FKs only when
///   `PRAGMA foreign_keys = ON`, which the runtime layer (Task 10) is
///   responsible for; until then the constraint is documentation but
///   the column shape is already correct.
/// - `score` — `INTEGER NOT NULL DEFAULT 0`. Score values are
///   integral by SPEC §4.8 (no fractional scores in v2). The default
///   matters for `increment_score` (8c) when it lands a new row via
///   upsert: the conflict-target path updates `score = score + ?`,
///   while the insert path needs a sane initial value before the
///   increment is applied.
/// - `updated_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp stored as ISO text — same encoding as `world_events`
///   for the same reasons (operator-inspectable in `sqlite3`, sorts
///   lexically the same way it sorts chronologically). `set_score`
///   and `increment_score` will write this column explicitly so the
///   default only fires for migrations that backfill existing rows.
///
/// # Primary key
///
/// `PRIMARY KEY (board, player_id)`. Composite primary key rather
/// than a synthetic `id` column because:
///
/// - SQLite's automatic `WITHOUT ROWID` optimization is **not**
///   applied here (we keep the rowid for FK targeting symmetry with
///   the rest of the schema), but the composite key still gives us a
///   `UNIQUE` index for free, which is exactly what the `INSERT … ON
///   CONFLICT(board, player_id) DO UPDATE` upserts in 8b/8c need.
/// - The natural identity of a row *is* the `(board, player_id)`
///   pair. Adding a synthetic id would create a second uniqueness
///   constraint we'd have to maintain separately.
///
/// # Indexes
///
/// One secondary index ships with the migration so the Task 8d
/// `top_scores` query is seek-bound from day one:
///
/// - `idx_leaderboard_scores_board_score` over
///   `(board, score, player_id)`. Matches the §4.8 contract: filter
///   by `board`, sort by `score` (descending for `desc` boards,
///   ascending for `asc` boards), and tie-break by `player_id` for
///   determinism. Including `player_id` in the index makes it
///   covering for the `top_scores` query — SQLite can answer
///   `SELECT player_id, score FROM leaderboard_scores WHERE board = ?
///   ORDER BY score DESC, player_id LIMIT ?` without touching the
///   table heap.
///
/// SPEC §4.8 also requires `player_rank`, which is a count of rows
/// with a "better" score on the same board. The same index covers
/// that query without a separate index — the planner can range-scan
/// `(board, score)` and return a count.
///
/// # Why no `updated_at` index
///
/// `updated_at` is a write-time stamp. No SPEC §4.8 query orders by
/// it, and adding an index would be dead weight on every write. If a
/// future task needs "recently active leaderboards" it can land a
/// follow-up migration; we don't speculatively pay for the index now.
///
/// # Version
///
/// `version = 5`. Versions 1–4 are reserved for prior kit migrations
/// (1 reserved, 2 = players, 3 = turn_ledger, 4 = world_events).
/// Game-authored migrations (Murder Motel's `motel_world_state` from
/// Task 12a) start from a higher band so they don't collide with kit
/// migrations the runtime applies on every open.
pub const LEADERBOARD_SCORES_MIGRATION: WorldMigration = WorldMigration {
    version: 5,
    name: "create_leaderboard_scores",
    sql: "\
CREATE TABLE IF NOT EXISTS leaderboard_scores (\n\
    board       TEXT NOT NULL,\n\
    player_id   INTEGER NOT NULL REFERENCES players(id),\n\
    score       INTEGER NOT NULL DEFAULT 0,\n\
    updated_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    PRIMARY KEY (board, player_id)\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_leaderboard_scores_board_score\n\
    ON leaderboard_scores(board, score, player_id);\n\
",
};

/// Decoded `leaderboard_scores` row — SPEC_v2 §4.8 read model.
///
/// Mirrors the column shape pinned by [`LEADERBOARD_SCORES_MIGRATION`].
/// The Task 8b/8c upsert helpers, the Task 8d `top_scores` query, and
/// the Task 13e/13f Murder Motel screens all consume this struct rather
/// than reaching into raw `rusqlite::Row`s — that keeps the
/// schema-to-Rust mapping in one place and turns a column rename into a
/// single compile error instead of a fan-out of runtime decode failures.
///
/// `updated_at` stays as raw SQLite text. The schema stores the
/// timestamp as ISO text precisely so the operator-facing `sqlite3`
/// story (see the module docs) reads the same value the runtime sees;
/// parsing it into a richer type would be a one-way trip that hides
/// corrupt data instead of surfacing it. Mirrors
/// [`crate::events::EventRecord`]'s handling of `created_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreRecord {
    /// Board name — same identifier the game-author wrote in
    /// `[[leaderboards]]` config (e.g. `"investigators"`). Round-tripped
    /// verbatim; the kit imposes no canonicalization on the column.
    pub board: String,
    /// Player attribution — the canonical id from
    /// [`crate::players::PlayerRecord`]. Always present; unlike
    /// [`crate::events::EventRecord`] there is no "system" rank.
    pub player_id: i64,
    /// The score value as last written by `set_score` (8b) or
    /// `increment_score` (8c). Stored as `i64` to match the SQLite
    /// `INTEGER` column without truncation on extreme values.
    pub score: i64,
    /// UTC timestamp written at the most recent upsert. Kept as ISO
    /// text — see struct docs for rationale.
    pub updated_at: String,
}

/// Failure modes for the leaderboard helpers ([`WorldDb::set_score`] in
/// this iteration; 8c–8e add their own surfaces over the same enum).
///
/// Library-internal `thiserror` shape — Task 10 will wrap these with
/// `anyhow` at the process boundary so the operator-facing message
/// stays a single sentence. Mirrors [`crate::events::EventError`] and
/// [`crate::turns::TurnError`] so all world-DB write paths surface
/// errors with the same shape.
#[derive(Debug, Error)]
pub enum LeaderboardError {
    /// The supplied board name was empty (or whitespace-only). SPEC §4.8
    /// keys every leaderboard query by name; a blank board would be
    /// indistinguishable from "any board" in the index and almost
    /// certainly indicates a caller bug (forgot to substitute a config
    /// lookup, etc.). Failing fast at the kit boundary keeps the bug
    /// visible instead of silently writing rows that no `top_scores`
    /// call can ever surface.
    #[error("leaderboard board name must not be empty or whitespace-only")]
    EmptyBoardName,
    /// The `INSERT … ON CONFLICT … RETURNING` round-trip failed.
    /// Wrapping `rusqlite::Error` keeps the call site readable (one
    /// error type, one mapping) while preserving the underlying cause
    /// for `tracing` and operator-facing messages.
    #[error("failed to write leaderboard score: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the upsert statement.
        #[source]
        source: rusqlite::Error,
    },
}

/// Validate `board` against the leaderboard guard rails — empty rejection.
///
/// Pulled out of [`WorldDb::set_score`] so the upcoming 8c/8d/8e helpers
/// can share one validator instead of reimplementing the rule and
/// drifting. Same shape as
/// [`crate::events::validate_event_message`]: `pub(crate)` because
/// callers outside the world-DB module shouldn't be inventing their own
/// validation — they should go through the typed helpers.
///
/// "Empty" is interpreted as `trim().is_empty()`: a board of `" "` or
/// `"\n"` would render as a blank header in the leaderboard UI, which
/// is indistinguishable from a missing board and almost always a caller
/// bug.
pub(crate) fn validate_board_name(board: &str) -> Result<(), LeaderboardError> {
    if board.trim().is_empty() {
        return Err(LeaderboardError::EmptyBoardName);
    }
    Ok(())
}

impl WorldDb {
    /// Set one player's score on a named board to an absolute value and
    /// return the canonical [`ScoreRecord`] SQLite produced (SPEC_v2
    /// §4.8 / §Task 8b).
    ///
    /// The contract is "after this call returns, `(board, player_id)`
    /// has exactly the score I asked for, with the timestamp the
    /// database assigned". This is an *upsert* — a missing
    /// `(board, player_id)` row is created, an existing row is
    /// overwritten. That matches SPEC §4.8's "set score" verb (as
    /// distinct from 8c's "increment score"): callers who hold an
    /// authoritative score (e.g. derived from a save snapshot, or a
    /// recomputed total) want to write it without a read-modify-write
    /// dance.
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) so the caller gets
    /// the canonical row — including the SQL-side `CURRENT_TIMESTAMP`
    /// — without a second round-trip, mirroring
    /// [`Self::append_event`] and [`Self::upsert_player`].
    ///
    /// `board` is validated before the SQL round-trip — empty or
    /// whitespace-only inputs fail fast with
    /// [`LeaderboardError::EmptyBoardName`]. `player_id` and `score`
    /// are intentionally not validated: a non-existent `player_id` is
    /// caught at the FK layer once Task 10 enables `PRAGMA
    /// foreign_keys = ON`, and `score` ranges (including negatives) are
    /// the game-author's domain.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the upsert is a single statement, so the busy
    /// timeout configured at open time is the only contention story we
    /// need. Same borrow shape as [`Self::append_event`] so the runtime
    /// layer (Task 10) can hold one shared world-DB handle across
    /// screens without a `RefCell` dance.
    pub fn set_score(
        &self,
        board: &str,
        player_id: i64,
        score: i64,
    ) -> Result<ScoreRecord, LeaderboardError> {
        // Validate before reaching SQL. A bad board is a caller bug,
        // not a database problem — surfacing it as `EmptyBoardName` is
        // more actionable than a silent write to an unreachable row, and
        // it avoids paying for a round-trip on input that was always
        // going to be wrong.
        validate_board_name(board)?;

        // `ON CONFLICT(board, player_id) DO UPDATE` matches the
        // composite primary key declared in the migration so SQLite
        // resolves the upsert via the existing index without a separate
        // uniqueness check. `excluded.score` is the value we proposed in
        // the `INSERT` — using it (rather than a second bound parameter)
        // keeps the "set to N" semantics encoded in one place. The
        // explicit `updated_at = CURRENT_TIMESTAMP` overrides the
        // column default on the update path: the default only fires for
        // a fresh insert, but a "set score that didn't change" still
        // bumps the timestamp so operators inspecting the table can tell
        // the row was touched.
        const SQL: &str = "\
INSERT INTO leaderboard_scores (board, player_id, score, updated_at) \
VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) \
ON CONFLICT(board, player_id) DO UPDATE SET \
    score = excluded.score, \
    updated_at = CURRENT_TIMESTAMP \
RETURNING board, player_id, score, updated_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![board, player_id, score],
                row_to_score_record,
            )
            .map_err(|source| LeaderboardError::Sqlite { source })
    }
}

/// Decode a `leaderboard_scores` row into [`ScoreRecord`].
///
/// Pulled out of the upsert call site so the upcoming 8c/8d/8e read
/// helpers can share one decoder. Column order matches the `RETURNING`
/// clause in [`WorldDb::set_score`] and the SPEC §4.8 schema; a
/// regression that reorders columns in the migration will surface here
/// as a `rusqlite` type error rather than a runtime panic in production.
fn row_to_score_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScoreRecord> {
    Ok(ScoreRecord {
        board: row.get(0)?,
        player_id: row.get(1)?,
        score: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// Helper: build a minimally-populated [`FogletContext`] with just
    /// the identity bits `upsert_player` reads. Mirrors the helper in
    /// `players::tests` and `events::tests` so each module's test
    /// bodies stay focused on the assertion under test rather than
    /// restating the full struct literal.
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

    /// Helper: open a temp world DB and apply every migration the
    /// leaderboard helpers depend on (players → leaderboard_scores).
    /// Mirrors `events::tests::open_world_with_events` so the per-test
    /// setup stays one line.
    fn open_world_with_leaderboards(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard_scores migration applies");
        world
    }

    /// SPEC_v2 §Task 8a acceptance: applying
    /// [`LEADERBOARD_SCORES_MIGRATION`] creates the documented
    /// `leaderboard_scores` table with the column shape later sub-tasks
    /// (8b–8e) depend on. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.8 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in an 8b set-score test).
    ///
    /// We apply the players migration first because
    /// `leaderboard_scores` references it via `FOREIGN KEY`. With FK
    /// enforcement off (the SQLite default until Task 10 turns it on)
    /// the migration would succeed even without the parent table, but
    /// exercising the real dependency order here mirrors how the
    /// runtime startup path will drive migrations on a real door open.
    #[test]
    fn migration_creates_leaderboard_scores_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard_scores migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'leaderboard_scores'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "leaderboard_scores table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('leaderboard_scores') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "board".to_string(),
                "player_id".to_string(),
                "score".to_string(),
                "updated_at".to_string(),
            ],
            "leaderboard_scores column shape must match the SPEC §4.8 contract"
        );
    }

    /// SPEC §4.8's "set score" / "increment score" upserts will rely
    /// on the composite primary key over `(board, player_id)`. A
    /// regression that flipped the schema to a synthetic `id` PK
    /// (or dropped the composite uniqueness) would silently allow
    /// duplicate `(board, player_id)` rows — a corruption mode that
    /// only manifests as wrong leaderboard rankings. Pin the PK
    /// shape explicitly here.
    #[test]
    fn leaderboard_scores_primary_key_is_board_and_player() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard_scores migration applies");

        // `pragma_table_info`'s `pk` column is `0` for non-PK columns
        // and `>= 1` for PK columns (giving the position within the
        // composite key). Querying it directly is more robust than
        // parsing the `sqlite_master.sql` text, whose whitespace is
        // implementation-defined.
        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, pk FROM pragma_table_info('leaderboard_scores') \
                 WHERE pk > 0 ORDER BY pk",
            )
            .expect("pragma_table_info preparable");
        let pk_columns: Vec<(String, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            pk_columns,
            vec![("board".to_string(), 1), ("player_id".to_string(), 2)],
            "primary key must be (board, player_id) in that order"
        );
    }

    /// The Task 8d `top_scores` and 8e `player_rank` query plans rely
    /// on the secondary index shipped with this migration. Asserting
    /// the index exists by name pins the contract without coupling the
    /// test to the SQL planner's choice of access path (which is
    /// implementation-defined and changes across SQLite versions). A
    /// regression that drops or renames the index flunks here, where
    /// the cause is obvious, instead of as a slow query in production.
    #[test]
    fn leaderboard_scores_board_score_index_exists() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard_scores migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND tbl_name = 'leaderboard_scores' \
                   AND name NOT LIKE 'sqlite_autoindex_%' \
                 ORDER BY name",
            )
            .expect("sqlite_master query preparable");
        let indexes: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            indexes,
            vec!["idx_leaderboard_scores_board_score".to_string()],
            "the (board, score, player_id) covering index must ship with the migration"
        );
    }

    /// SPEC §4.8 requires that `score` and `board` always carry a
    /// value — a leaderboard row with a NULL board is meaningless and
    /// a NULL score would corrupt every aggregate the kit returns.
    /// Pin both `NOT NULL` constraints explicitly so a future schema
    /// edit that relaxes either flunks here rather than as a runtime
    /// decode error.
    #[test]
    fn leaderboard_scores_board_and_score_are_not_null() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard_scores migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, \"notnull\" FROM pragma_table_info('leaderboard_scores') \
                 WHERE name IN ('board', 'score') ORDER BY name",
            )
            .expect("pragma_table_info preparable");
        let constraints: Vec<(String, i64)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            constraints,
            vec![("board".to_string(), 1), ("score".to_string(), 1)],
            "board and score must remain NOT NULL"
        );
    }

    /// SPEC_v2 §Task 8b acceptance: the first `set_score` for a fresh
    /// `(board, player_id)` pair creates a row with the supplied score
    /// and a SQLite-assigned `updated_at`, and the returned
    /// [`ScoreRecord`] echoes the values that landed in the table.
    ///
    /// Round-trip via a `SELECT` rather than trusting the `RETURNING`
    /// row alone — a regression where `set_score` accidentally
    /// `INSERT`ed nothing but synthesized a fake record from its
    /// arguments would still pass a "the returned struct matches my
    /// inputs" assertion. Reading the row back proves durability.
    #[test]
    fn set_score_first_write_creates_row() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        let record = world
            .set_score("investigators", alice.id, 42)
            .expect("set_score succeeds");

        assert_eq!(record.board, "investigators");
        assert_eq!(record.player_id, alice.id);
        assert_eq!(record.score, 42);
        // `updated_at` defaults to `CURRENT_TIMESTAMP`. We don't pin
        // the wall-clock value but it must be non-empty, which proves
        // the schema default fired and the decoder read the right
        // column.
        assert!(!record.updated_at.is_empty(), "updated_at must be set");

        let stored: (String, i64, i64) = world
            .connection()
            .query_row(
                "SELECT board, player_id, score FROM leaderboard_scores \
                 WHERE board = ?1 AND player_id = ?2",
                rusqlite::params!["investigators", alice.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row reads back");
        assert_eq!(
            stored,
            ("investigators".to_string(), alice.id, 42),
            "row must be durably stored, not just synthesized in-memory"
        );

        // And exactly one row exists — a regression that turned the
        // upsert into a plain INSERT (which would later collide with
        // the composite PK on the second call) would still create one
        // row on the first write, but a regression that double-inserted
        // somehow would flunk this.
        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM leaderboard_scores", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(count, 1, "first write must create exactly one row");
    }

    /// `set_score` is an *upsert*: a second call against the same
    /// `(board, player_id)` overwrites the score rather than failing on
    /// the composite primary key. Pinning this here protects the SPEC
    /// §4.8 "set score" verb — callers who hold an authoritative score
    /// and write it twice in a row should see the second value win,
    /// not a `UNIQUE constraint failed` error. A regression that
    /// dropped the `ON CONFLICT … DO UPDATE` clause would flunk here.
    #[test]
    fn set_score_overwrites_existing_row() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        world
            .set_score("investigators", alice.id, 10)
            .expect("first set_score succeeds");
        let updated = world
            .set_score("investigators", alice.id, 25)
            .expect("second set_score succeeds");

        assert_eq!(updated.score, 25, "second write must win");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM leaderboard_scores \
                 WHERE board = ?1 AND player_id = ?2",
                rusqlite::params!["investigators", alice.id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(
            count, 1,
            "upsert must keep one row per (board, player_id) pair"
        );
    }

    /// SPEC §4.8 keys every leaderboard query by name; a blank board
    /// would be indistinguishable from "any board" in the index. The
    /// guard rejects empty and whitespace-only board names at the kit
    /// boundary so unreachable rows can't enter the table. A regression
    /// that dropped the [`validate_board_name`] call would flunk here.
    #[test]
    fn set_score_rejects_empty_board_name() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        for blank in ["", "   ", "\n\t"] {
            let err = world
                .set_score(blank, alice.id, 1)
                .expect_err("blank board name must be rejected");
            assert!(
                matches!(err, LeaderboardError::EmptyBoardName),
                "expected EmptyBoardName for {blank:?}, got {err:?}"
            );
        }

        // And no row was written by the rejected calls — failing fast
        // means the SQL round-trip never ran.
        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM leaderboard_scores", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(count, 0, "rejected set_score calls must not write any rows");
    }
}
