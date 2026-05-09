//! `leaderboards` — shared-world named score tables (SPEC_v2 §Task 8).
//!
//! Task 8a (this iteration) ships only the `leaderboard_scores`
//! migration. The behavioral helpers — `set_score` (8b),
//! `increment_score` (8c), `top_scores` (8d), and `player_rank` (8e) —
//! land in follow-up commits, each in its own module-internal block.
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

use crate::world_db::WorldMigration;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

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
}
