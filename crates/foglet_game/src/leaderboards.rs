//! `leaderboards` — shared-world named score tables.
//!
//!  shipped the `leaderboard_scores` migration. Subsequent
//! sub-tasks layer behavior on top of the schema introduced here:
//!
//! - 8b adds [`WorldDb::set_score`] for upserting one
//!   `(board, player_id)` row to an absolute value.
//! - 8c adds [`WorldDb::increment_score`] for delta updates that don't
//!   require the caller to know the prior score.
//! - 8d adds [`WorldDb::top_scores`] for the leaderboard render
//!   `Desc`/`Asc` sort with deterministic `(updated_at, player_id)`
//!   tie ordering.
//! - 8e (this iteration) adds [`WorldDb::player_rank`] for "you are
//!   #N", reusing the same tie ordering as [`WorldDb::top_scores`] so
//!   a player's rank line agrees with their row in the leaderboard.
//!
//! Splitting the schema commit from the helper commits keeps the
//! bisect signal sharp: a regression that drops a column or an index
//! flunks the schema test in this module rather than appearing as a
//! mysterious query failure inside a higher-level helper test.
//!
//! The migration is exported as a `pub const` so the runtime startup
//! path and game-author code can apply one canonical
//! definition without redeclaring the schema and drifting from it
//! same pattern as [`crate::players::PLAYERS_MIGRATION`].
//! [`crate::turns::TURN_LEDGER_MIGRATION`], and
//! [`crate::events::WORLD_EVENTS_MIGRATION`].
//!
//! # Why a dedicated table per board, keyed by board name
//!
//!  mandates four behaviors for any leaderboard:
//!
//! 1. Set a player's score on a named board.
//! 2. Increment a player's score on a named board.
//! 3. Query the top N entries on a named board.
//! 4. Query one player's rank on a named board.
//!
//! All four are "by name" operations. A naive design might create one
//! SQL table per declared `[[leaderboards]]` entry, but the board set
//! is game-authored config and we don't want to issue
//! `CREATE TABLE` at runtime every time a game adds a board. Folding
//! every board into one table keyed by `(board, player_id)` keeps the
//! migration story stable: adding a board is a config edit, not a
//! schema change, and the kit's helpers query one table by `board`
//! parameter.
//!
//! # Why `(board, player_id)` is the natural primary key
//!
//! A player has at most one score on any given board. 's
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

use crate::config::LeaderboardSort;
use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the shared-world leaderboard table — /
/// .
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
///   through 's runtime validation, not a SQL-layer constraint
///   — there is no enum table because the board set is owned by config.
/// - `player_id` — `INTEGER NOT NULL REFERENCES players(id)`. Same
///   foreign-key story as the turn ledger and event log: phantom ids
///   should never land here. SQLite enforces FKs only when
///   `PRAGMA foreign_keys = ON`, which the runtime layer is
///   responsible for; until then the constraint is documentation but
///   the column shape is already correct.
/// - `score` — `INTEGER NOT NULL DEFAULT 0`. Score values are
///   integral by (no fractional scores in v2). The default
///   matters for `increment_score` (8c) when it lands a new row via
///   upsert: the conflict-target path updates `score = score + ?`.
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
/// One secondary index ships with the migration so the
/// `top_scores` query is seek-bound from day one:
///
/// - `idx_leaderboard_scores_board_score` over
///   `(board, score, player_id)`. Matches the contract: filter
///   by `board`, sort by `score` (descending for `desc` boards.
///   ascending for `asc` boards), and tie-break by `player_id` for
///   determinism. Including `player_id` in the index makes it
///   covering for the `top_scores` query — SQLite can answer
///   `SELECT player_id, score FROM leaderboard_scores WHERE board = ?
///   ORDER BY score DESC, player_id LIMIT ?` without touching the
///   table heap.
///
///  also requires `player_rank`, which is a count of rows
/// with a "better" score on the same board. The same index covers
/// that query without a separate index — the planner can range-scan
/// `(board, score)` and return a count.
///
/// # Why no `updated_at` index
///
/// `updated_at` is a write-time stamp. No query orders by
/// it, and adding an index would be dead weight on every write. If a
/// future task needs "recently active leaderboards" it can land a
/// follow-up migration; we don't speculatively pay for the index now.
///
/// # Version
///
/// `version = 5`. Versions 1–4 are reserved for prior kit migrations
/// (1 reserved, 2 = players, 3 = turn_ledger, 4 = world_events).
/// Game-authored migrations (Murder Motel's `motel_world_state` from
/// ) start from a higher band so they don't collide with kit
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

/// Decoded `leaderboard_scores` row — read model.
///
/// Mirrors the column shape pinned by [`LEADERBOARD_SCORES_MIGRATION`].
/// The upsert helpers, the `top_scores` query, and
/// the Murder Motel screens all consume this struct rather
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
/// Library-internal `thiserror` shape — will wrap these with
/// `anyhow` at the process boundary so the operator-facing message
/// stays a single sentence. Mirrors [`crate::events::EventError`] and
/// [`crate::turns::TurnError`] so all world-DB write paths surface
/// errors with the same shape.
#[derive(Debug, Error)]
pub enum LeaderboardError {
    /// The supplied board name was empty (or whitespace-only).
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
/// "Empty" is interpreted as `trim.is_empty`: a board of `" "` or
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
    /// return the canonical [`ScoreRecord`] SQLite produced (
    ///  ).
    ///
    /// The contract is "after this call returns, `(board, player_id)`
    /// has exactly the score I asked for, with the timestamp the
    /// database assigned". This is an *upsert* — a missing
    /// `(board, player_id)` row is created, an existing row is
    /// overwritten. That matches 's "set score" verb (as
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
    /// caught at the FK layer once enables `PRAGMA
    /// foreign_keys = ON`, and `score` ranges (including negatives) are
    /// the game-author's domain.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the upsert is a single statement, so the busy
    /// timeout configured at open time is the only contention story we
    /// need. Same borrow shape as [`Self::append_event`] so the runtime
    /// layer can hold one shared world-DB handle across
    /// screens without a `RefCell` dance.
    pub fn set_score(
        &self,
        board: &str,
        player_id: i64,
        score: i64,
    ) -> Result<ScoreRecord, LeaderboardError> {
        // Validate before reaching SQL. A bad board is a caller bug.
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

    /// Add `delta` to one player's score on a named board, creating the
    /// row at `delta` if it does not yet exist, and return the canonical
    /// [`ScoreRecord`] SQLite produced.
    ///
    /// The contract is "after this call returns, `(board, player_id)`
    /// has its previous score plus `delta` — and if there was no
    /// previous score, the row exists with `delta` as its score". This
    /// is 's "increment score" verb: callers who want to
    /// register +1 for a clue inspection (Murder Motel ), or
    /// any other event-driven score change, shouldn't have to do a
    /// read-modify-write dance from Rust — that would race with another
    /// process touching the same row.
    ///
    /// `delta` is `i64` rather than `u64` so callers can decrement a
    /// score (e.g. a penalty) with the same helper. The score column is
    /// `INTEGER NOT NULL` and imposes no non-negative
    /// constraint, so a negative result is a valid game-author choice
    /// rather than a kit-level error. Overflow is not guarded:
    ///  scores are integral and SQLite stores them as 64-bit, which
    /// is enough headroom for any door game's lifetime; pretending to
    /// guard a 64-bit counter would be theatre.
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) so the caller gets
    /// the canonical row — including the post-increment score and the
    /// SQL-side `CURRENT_TIMESTAMP` — without a second round-trip.
    /// mirroring [`Self::set_score`] and [`Self::append_event`]. The
    /// upsert resolves on the existing `(board, player_id)` primary key
    /// the migration ships with, so no extra index is needed.
    ///
    /// `board` is validated before the SQL round-trip — empty or
    /// whitespace-only inputs fail fast with
    /// [`LeaderboardError::EmptyBoardName`], same as [`Self::set_score`]
    /// — so the validator is the single source of truth for the rule.
    /// `player_id` and `delta` are intentionally not validated: a
    /// non-existent `player_id` is caught at the FK layer once
    /// enables `PRAGMA foreign_keys = ON`, and `delta` ranges
    /// (including zero, which is a no-op that still bumps `updated_at`)
    /// are the game-author's domain.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the upsert is a single atomic statement, so two
    /// concurrent `increment_score` calls against the same row will
    /// serialize on the busy-timeout path rather than racing through a
    /// read-modify-write window. That is the whole reason this helper
    /// exists alongside [`Self::set_score`] — a Rust-side
    /// `set_score(get_score + 1)` would silently lose increments
    /// under contention.
    pub fn increment_score(
        &self,
        board: &str,
        player_id: i64,
        delta: i64,
    ) -> Result<ScoreRecord, LeaderboardError> {
        // Same validation contract as `set_score` — a blank board is a
        // caller bug regardless of which verb the caller invoked.
        validate_board_name(board)?;

        // Insert path seeds the row with `delta` as the starting score
        // (matching the schema's `DEFAULT 0` plus a single increment;
        // we encode the addition explicitly rather than relying on the
        // default + a follow-up update so the "first write" case lands
        // in one statement). Conflict path adds `excluded.score`
        // which is the proposed insert value, i.e. `delta` — to the
        // existing `score` column. The same `?3` parameter is used in
        // both code paths via `excluded`, keeping the "by how much"
        // value bound exactly once.
        //
        // `updated_at = CURRENT_TIMESTAMP` overrides the column default
        // on the update path: even a no-op `+0` increment touches the
        // row, so an operator inspecting the table can see that the
        // increment ran. Mirrors `set_score`'s behavior so both verbs
        // leave a consistent audit trail.
        const SQL: &str = "\
INSERT INTO leaderboard_scores (board, player_id, score, updated_at) \
VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) \
ON CONFLICT(board, player_id) DO UPDATE SET \
    score = score + excluded.score, \
    updated_at = CURRENT_TIMESTAMP \
RETURNING board, player_id, score, updated_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![board, player_id, delta],
                row_to_score_record,
            )
            .map_err(|source| LeaderboardError::Sqlite { source })
    }

    /// Return the top `limit` rows on a named board in the configured
    /// sort direction.
    ///
    /// "Top" means *best first* per the board's [`LeaderboardSort`]:
    /// `Desc` boards (the typical case — investigators, kills, points)
    /// rank highest score first; `Asc` boards (time trials, golf-style
    /// scoring) rank lowest score first. Callers that own a
    /// [`crate::config::LeaderboardSection`] pass its `sort` field
    /// straight through; the helper will not invent a default because
    /// "top by score" is ambiguous without a direction.
    ///
    /// # Tie ordering — deterministic by
    ///
    ///  mandates that tie ordering is deterministic. We resolve
    /// ties in two stages so the order is total even when scores collide:
    ///
    /// 1. `updated_at ASC` — the *earlier* writer wins a score tie. This
    ///    matches operator intuition for any "first to N" board: if two
    ///    investigators each found 10 clues, the one who got there first
    ///    is ahead. Stored as ISO text precisely so lexical ordering
    ///    matches chronological ordering (see [`ScoreRecord`] docs).
    /// 2. `player_id ASC` — final tiebreaker for the rare case where
    ///    two rows share both a score and an `updated_at` (within the
    ///    same SQLite-second). `player_id` is monotone per
    ///    [`crate::players::PlayerRecord`], so this stage is total.
    ///
    /// The same secondary sort applies regardless of `sort`: only the
    /// primary `score` direction flips. That keeps `Asc` and `Desc`
    /// boards consistent — "earlier wins, lower id wins" reads the same
    /// way to operators inspecting either kind of board.
    ///
    /// # Parameters
    ///
    /// `board` is validated by the shared `validate_board_name` guard.
    /// same contract as [`Self::set_score`] [`Self::increment_score`]:
    /// empty or whitespace-only inputs fail fast with
    /// [`LeaderboardError::EmptyBoardName`].
    ///
    /// `limit` is `u32` — same shape as [`Self::recent_events`] /
    /// [`Self::player_events`]. `0` is legal and returns an empty vec; a
    /// signed `i64` would force callers to think about negatives we
    /// don't accept. SQLite's `LIMIT` parameter is `INTEGER` so we widen
    /// to `i64` at the bind site.
    ///
    /// Querying a board that has no rows is *not* an error: the helper
    /// returns an empty vec, which is the correct UI state for "no
    /// scores yet" on a freshly-launched door.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single read statement under the configured busy
    /// timeout, same as [`Self::recent_events`]. The runtime layer
    ///  will call this from the leaderboard render path
    /// ; keeping the borrow shared lets `GameContext` share
    /// one world-DB reference across screens without a `RefCell` dance.
    pub fn top_scores(
        &self,
        board: &str,
        sort: LeaderboardSort,
        limit: u32,
    ) -> Result<Vec<ScoreRecord>, LeaderboardError> {
        // Same blank-board contract as the write helpers — failing fast
        // here keeps the SQL round-trip from running on input that was
        // always going to return empty rows.
        validate_board_name(board)?;

        // Two SQL strings instead of interpolating a direction into one:
        // SQLite parameters can bind values, not the `ASC`/`DESC` token
        // itself, and string-formatting SQL would be both unsafe and
        // pointless given the closed [`LeaderboardSort`] enum. The two
        // strings differ only in the `score` direction; the secondary
        // tiebreakers (`updated_at ASC, player_id ASC`) are identical so
        // the deterministic-ordering contract stays the same regardless
        // of sort. The `(board, score, player_id)` index from the
        // migration covers the `WHERE board = ?` filter and the score
        // sort; SQLite will read the small page of `updated_at` from the
        // table heap to apply the secondary sort, which is fine for the
        // small `limit` the leaderboard UI requests (typically ≤ 10).
        const SQL_DESC: &str = "\
SELECT board, player_id, score, updated_at \
FROM leaderboard_scores \
WHERE board = ?1 \
ORDER BY score DESC, updated_at ASC, player_id ASC \
LIMIT ?2";
        const SQL_ASC: &str = "\
SELECT board, player_id, score, updated_at \
FROM leaderboard_scores \
WHERE board = ?1 \
ORDER BY score ASC, updated_at ASC, player_id ASC \
LIMIT ?2";
        let sql = match sort {
            LeaderboardSort::Desc => SQL_DESC,
            LeaderboardSort::Asc => SQL_ASC,
        };

        let mut stmt = self
            .connection()
            .prepare(sql)
            .map_err(|source| LeaderboardError::Sqlite { source })?;
        let rows = stmt
            .query_map(
                rusqlite::params![board, i64::from(limit)],
                row_to_score_record,
            )
            .map_err(|source| LeaderboardError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| LeaderboardError::Sqlite { source })
    }

    /// Return one player's 1-based rank on a named board, or `None` if
    /// the player has no score on that board.
    ///
    /// The contract is "given the same `sort` direction
    /// [`Self::top_scores`] would use, what position would this
    /// player's row occupy?". Rank `1` is the head of the leaderboard.
    /// rank `N` is the tail (where `N` is the total row count on the
    /// board). A player with no row on the board returns `Ok(None)`
    /// rather than an error: an unranked player is a normal UI state
    /// (the leaderboard render shows "—" or "unranked"), not a caller
    /// bug. Distinguishing "no row" from "rank 0" with `Option`
    /// prevents callers from rendering a misleading "rank 0" line.
    ///
    /// # Tie ordering — same chain as [`Self::top_scores`]
    ///
    /// Rank counts how many rows would appear *ahead* of the player in
    /// `top_scores`'s ordering, plus one. The tiebreaker chain is
    /// identical so the two helpers can never disagree:
    ///
    /// 1. Primary: `score` in the requested direction (`Desc` → higher
    ///    is better; `Asc` → lower is better).
    /// 2. Secondary: `updated_at ASC` — earlier writer wins a score
    ///    tie.
    /// 3. Tertiary: `player_id ASC` — final monotone tiebreaker.
    ///
    /// A player who is the only row at their score still gets a sane
    /// rank because the count of "rows ahead" is well-defined for any
    /// total order.
    ///
    /// # Why one query, not two
    ///
    /// A naive implementation would `SELECT score, updated_at FROM
    /// leaderboard_scores WHERE board = ? AND player_id = ?` and then
    /// `SELECT COUNT(*) FROM leaderboard_scores WHERE …`. That's two
    /// round-trips and a TOCTOU window where another writer could
    /// shift the rank between calls. We fold both into one statement
    /// using a correlated subquery on `me`, which gives a consistent
    /// snapshot and halves the wire cost. The `(board, score.
    /// player_id)` index from the migration covers the inner count.
    ///
    /// # Parameters
    ///
    /// `board` is validated by the shared `validate_board_name`
    /// guard, same contract as the other leaderboard verbs: empty or
    /// whitespace-only inputs fail fast with
    /// [`LeaderboardError::EmptyBoardName`].
    ///
    /// `sort` must match the board's configured direction — passing
    /// the wrong direction silently returns the rank in the *other*
    /// ordering. The kit cannot infer it from the board name alone
    /// because the leaderboard config is owned by the game-author;
    /// callers that hold a [`crate::config::LeaderboardSection`]
    /// should pass its `sort` field straight through, mirroring
    /// [`Self::top_scores`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single read statement under the configured
    /// busy timeout, same as [`Self::top_scores`]. The runtime layer
    ///  will call this from the leaderboard render path
    ///  to render "you are #N" alongside the top-scores
    /// table.
    pub fn player_rank(
        &self,
        board: &str,
        sort: LeaderboardSort,
        player_id: i64,
    ) -> Result<Option<u64>, LeaderboardError> {
        // Same blank-board contract as the other leaderboard verbs.
        // Failing fast keeps an unreachable-board query from running.
        validate_board_name(board)?;

        // Two SQL strings — one per sort direction — for the same
        // reason as `top_scores`: the comparison operator is part of
        // the SQL grammar, not a bindable parameter, and the closed
        // [`LeaderboardSort`] enum makes string-formatting needless.
        //
        // The correlated subquery counts rows on the same board that
        // would appear *ahead* of `me` in the `top_scores` ordering.
        // For `Desc`, "ahead" means strictly higher score, *or* tied
        // score with an earlier `updated_at`, *or* tied
        // (score, updated_at) with a smaller `player_id`. Adding one
        // converts that count to a 1-based rank.
        //
        // `me` self-joins via the `WHERE` clause (`me.board = ?1 AND
        // me.player_id = ?2`). If no such row exists, the outer query
        // returns zero rows, which we surface as `Ok(None)` — the
        // "unranked" state.
        const SQL_DESC: &str = "\
SELECT 1 + (\
    SELECT COUNT(*) FROM leaderboard_scores other \
    WHERE other.board = me.board \
      AND ( \
        other.score > me.score \
        OR (other.score = me.score AND other.updated_at < me.updated_at) \
        OR (other.score = me.score AND other.updated_at = me.updated_at \
            AND other.player_id < me.player_id) \
      ) \
) AS rank \
FROM leaderboard_scores me \
WHERE me.board = ?1 AND me.player_id = ?2";
        const SQL_ASC: &str = "\
SELECT 1 + (\
    SELECT COUNT(*) FROM leaderboard_scores other \
    WHERE other.board = me.board \
      AND ( \
        other.score < me.score \
        OR (other.score = me.score AND other.updated_at < me.updated_at) \
        OR (other.score = me.score AND other.updated_at = me.updated_at \
            AND other.player_id < me.player_id) \
      ) \
) AS rank \
FROM leaderboard_scores me \
WHERE me.board = ?1 AND me.player_id = ?2";
        let sql = match sort {
            LeaderboardSort::Desc => SQL_DESC,
            LeaderboardSort::Asc => SQL_ASC,
        };

        // `query_row` returns `QueryReturnedNoRows` when the player
        // has no row on this board. That is the "unranked" state, not
        // an error — translate it to `Ok(None)` so callers can render
        // it as a normal UI affordance. Any other rusqlite error is a
        // genuine SQL failure and gets the standard mapping.
        match self
            .connection()
            .query_row(sql, rusqlite::params![board, player_id], |row| {
                row.get::<_, i64>(0)
            }) {
            Ok(rank) => {
                // SQLite returns `INTEGER` as `i64`; the rank is
                // logically a count + 1, so it cannot be negative
                // unless something has gone deeply wrong with the
                // query. Cast to `u64` via `try_into` so a negative
                // value would surface as a SQL error rather than a
                // panic, but in practice this branch is unreachable.
                let rank: u64 = rank.try_into().map_err(|_| LeaderboardError::Sqlite {
                    source: rusqlite::Error::IntegralValueOutOfRange(0, rank),
                })?;
                Ok(Some(rank))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(LeaderboardError::Sqlite { source }),
        }
    }
}

/// Decode a `leaderboard_scores` row into [`ScoreRecord`].
///
/// Pulled out of the upsert call site so the upcoming 8c/8d/8e read
/// helpers can share one decoder. Column order matches the `RETURNING`
/// clause in [`WorldDb::set_score`] and the schema; a
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

    ///   acceptance: applying
    /// [`LEADERBOARD_SCORES_MIGRATION`] creates the documented
    /// `leaderboard_scores` table with the column shape later sub-tasks
    /// (8b–8e) depend on. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in an 8b set-score test).
    ///
    /// We apply the players migration first because
    /// `leaderboard_scores` references it via `FOREIGN KEY`. With FK
    /// enforcement off (the SQLite default until turns it on)
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
            "leaderboard_scores column shape must match the contract"
        );
    }

    /// 's "set score" "increment score" upserts will rely
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

    /// The `top_scores` and 8e `player_rank` query plans rely
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

    ///  requires that `score` and `board` always carry a
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

    ///   acceptance: the first `set_score` for a fresh
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
    /// the composite primary key. Pinning this here protects the
    ///  "set score" verb — callers who hold an authoritative score
    /// and write it twice in a row should see the second value win.
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

    ///  keys every leaderboard query by name; a blank board
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

    ///   acceptance: incrementing an existing score
    /// updates the row in place by adding `delta` to the prior value.
    /// A regression that turned the upsert into a `set` (overwriting
    /// rather than adding) would flunk here — the post-increment score
    /// would equal `delta` instead of `prior + delta`.
    #[test]
    fn increment_score_increments_existing_row() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        world
            .set_score("investigators", alice.id, 10)
            .expect("seed set_score succeeds");

        let after = world
            .increment_score("investigators", alice.id, 3)
            .expect("increment_score succeeds");

        assert_eq!(
            after.score, 13,
            "increment must add delta to the prior score"
        );

        // Round-trip through SELECT proves durability — a regression
        // that synthesized a fake record from arguments without
        // actually touching the row would still pass the returned-
        // record assertion above, but flunk this one.
        let stored: i64 = world
            .connection()
            .query_row(
                "SELECT score FROM leaderboard_scores \
                 WHERE board = ?1 AND player_id = ?2",
                rusqlite::params!["investigators", alice.id],
                |row| row.get(0),
            )
            .expect("row reads back");
        assert_eq!(stored, 13, "stored row must reflect the increment");

        // And exactly one row exists — a regression that turned the
        // upsert into a plain INSERT (creating a duplicate row instead
        // of updating in place) would flunk here.
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
            "increment must update the existing row, not insert a duplicate"
        );
    }

    ///   acceptance: incrementing a `(board, player_id)`
    /// pair that has no prior row creates the row with `delta` as the
    /// starting score. A regression that required a prior row (e.g.
    /// dropping the `INSERT … ON CONFLICT` upsert in favor of a plain
    /// `UPDATE`) would flunk here — the call would silently no-op and
    /// the leaderboard would never register the player.
    #[test]
    fn increment_score_creates_missing_row() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        let after = world
            .increment_score("investigators", alice.id, 5)
            .expect("increment_score succeeds on fresh row");

        assert_eq!(after.board, "investigators");
        assert_eq!(after.player_id, alice.id);
        assert_eq!(after.score, 5, "first increment must seed the row at delta");
        assert!(!after.updated_at.is_empty(), "updated_at must be set");

        let stored: (i64, String) = world
            .connection()
            .query_row(
                "SELECT score, board FROM leaderboard_scores \
                 WHERE board = ?1 AND player_id = ?2",
                rusqlite::params!["investigators", alice.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("row reads back");
        assert_eq!(
            stored,
            (5, "investigators".to_string()),
            "missing-row increment must durably create the row"
        );
    }

    /// `delta` is signed: negative deltas decrement, and the result
    /// can legally be negative or zero. imposes no
    /// non-negative constraint and the kit shouldn't invent one.
    /// Pinning this here protects callers (e.g. Murder Motel
    /// penalties) who rely on the signed contract.
    #[test]
    fn increment_score_supports_negative_delta() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        world
            .set_score("investigators", alice.id, 2)
            .expect("seed set_score succeeds");
        let after = world
            .increment_score("investigators", alice.id, -5)
            .expect("negative increment succeeds");

        assert_eq!(
            after.score, -3,
            "negative delta must subtract from the prior score, including past zero"
        );
    }

    ///   acceptance: `top_scores` on a `Desc` board
    /// returns rows with the highest score first, capped at `limit`.
    /// A regression that flipped the sort direction would flunk here
    /// the lowest score would appear at the head of the vec.
    #[test]
    fn top_scores_desc_returns_highest_first() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert succeeds");
        let carol = world
            .upsert_player(&ctx_with(Some("u-carol"), Some("carol")))
            .expect("carol upsert succeeds");

        world
            .set_score("investigators", alice.id, 5)
            .expect("alice set_score");
        world
            .set_score("investigators", bob.id, 12)
            .expect("bob set_score");
        world
            .set_score("investigators", carol.id, 8)
            .expect("carol set_score");

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores succeeds");

        let ranked: Vec<(i64, i64)> = top.iter().map(|r| (r.player_id, r.score)).collect();
        assert_eq!(
            ranked,
            vec![(bob.id, 12), (carol.id, 8), (alice.id, 5)],
            "Desc board must rank highest score first"
        );
    }

    /// `Asc` boards (time-trial, golf-style) rank lowest score first.
    /// Pinning this here protects the `Asc` direction so a
    /// regression that hard-coded `DESC` in the SQL would flunk.
    #[test]
    fn top_scores_asc_returns_lowest_first() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert succeeds");

        world
            .set_score("speedrun", alice.id, 90)
            .expect("alice set_score");
        world
            .set_score("speedrun", bob.id, 45)
            .expect("bob set_score");

        let top = world
            .top_scores("speedrun", LeaderboardSort::Asc, 10)
            .expect("top_scores succeeds");

        let ranked: Vec<(i64, i64)> = top.iter().map(|r| (r.player_id, r.score)).collect();
        assert_eq!(
            ranked,
            vec![(bob.id, 45), (alice.id, 90)],
            "Asc board must rank lowest score first"
        );
    }

    /// `limit` caps the returned row count. A regression that ignored
    /// the parameter would return every row and flunk here.
    #[test]
    fn top_scores_respects_limit() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        for i in 0..5 {
            let user = format!("u-{i}");
            let name = format!("p{i}");
            let player = world
                .upsert_player(&ctx_with(Some(&user), Some(&name)))
                .expect("upsert succeeds");
            world
                .set_score("investigators", player.id, i64::from(i) * 10)
                .expect("set_score");
        }

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 3)
            .expect("top_scores succeeds");
        assert_eq!(top.len(), 3, "limit must cap the returned row count");
        let scores: Vec<i64> = top.iter().map(|r| r.score).collect();
        assert_eq!(
            scores,
            vec![40, 30, 20],
            "limit must keep the best rows by sort direction"
        );
    }

    ///  mandates deterministic tie ordering. When two players
    /// share a score *and* the same `updated_at` timestamp (the common
    /// case for back-to-back writes within one SQLite-second), the
    /// helper must break the tie by `player_id ASC`. A regression that
    /// fell through to SQLite's natural rowid ordering — which happens
    /// to coincide here but is not contractually guaranteed — would
    /// silently lose this property when the index changes.
    #[test]
    fn top_scores_ties_break_by_player_id() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(Some("u-carol"), Some("carol")))
            .expect("carol upsert");

        // Identical scores. Force identical `updated_at` so the tie-
        // breaker chain falls through to `player_id`. CURRENT_TIMESTAMP
        // already has one-second granularity, so back-to-back inserts
        // usually share an `updated_at`, but pinning the value via UPDATE
        // makes the test deterministic regardless of clock granularity
        // on the host running cargo test.
        world
            .set_score("investigators", carol.id, 7)
            .expect("carol set_score");
        world
            .set_score("investigators", alice.id, 7)
            .expect("alice set_score");
        world
            .set_score("investigators", bob.id, 7)
            .expect("bob set_score");
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-05-08T12:00:00' \
                 WHERE board = 'investigators'",
                [],
            )
            .expect("pin updated_at");

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores succeeds");

        // Player ids are issued monotonically by the players migration's
        // INTEGER PRIMARY KEY, so alice < bob < carol here. The expected
        // order is `player_id ASC` regardless of insertion order.
        let ids: Vec<i64> = top.iter().map(|r| r.player_id).collect();
        assert_eq!(
            ids,
            vec![alice.id, bob.id, carol.id],
            "tie on (score, updated_at) must break by player_id ASC"
        );
    }

    /// When two players share a score but have distinct `updated_at`
    /// stamps, the *earlier* writer wins the tie — "first to N" intuition
    /// for any leaderboard. This is the primary tiebreaker, ahead of
    /// `player_id`. A regression that swapped the two tiebreakers (e.g.
    /// `player_id` first) would flunk here: the later-by-time writer
    /// with a smaller player_id would jump ahead of the earlier writer.
    #[test]
    fn top_scores_ties_break_by_updated_at_before_player_id() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");

        world
            .set_score("investigators", alice.id, 7)
            .expect("alice set_score");
        world
            .set_score("investigators", bob.id, 7)
            .expect("bob set_score");

        // Force bob's row to be older than alice's. With `player_id`-
        // first tiebreaking, alice (smaller id) would still win — so an
        // assertion that bob appears first proves `updated_at` runs
        // ahead of `player_id` in the tie chain.
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-01-01T00:00:00' \
                 WHERE board = 'investigators' AND player_id = ?1",
                rusqlite::params![bob.id],
            )
            .expect("backdate bob");
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-05-08T12:00:00' \
                 WHERE board = 'investigators' AND player_id = ?1",
                rusqlite::params![alice.id],
            )
            .expect("future-date alice");

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores succeeds");
        let ids: Vec<i64> = top.iter().map(|r| r.player_id).collect();
        assert_eq!(
            ids,
            vec![bob.id, alice.id],
            "earlier updated_at must beat later updated_at on a score tie"
        );
    }

    /// Querying a board that has never been written to is not an error
    /// — it returns an empty vec. Pinning this here protects the UI
    /// path: a freshly-launched door must be able to render an empty
    /// leaderboard without surfacing a SQL error.
    #[test]
    fn top_scores_empty_board_returns_empty_vec() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores succeeds on empty board");
        assert!(
            top.is_empty(),
            "no rows means an empty result, not an error"
        );
    }

    /// `limit = 0` is legal and returns an empty vec — same shape as
    /// [`WorldDb::recent_events`]. Lets callers wire up UI plumbing
    /// before the leaderboard pane is sized.
    #[test]
    fn top_scores_zero_limit_returns_empty_vec() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        world
            .set_score("investigators", alice.id, 1)
            .expect("alice set_score");

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 0)
            .expect("top_scores succeeds with zero limit");
        assert!(top.is_empty(), "limit=0 must return no rows");
    }

    /// `top_scores` only sees rows on the requested board; another
    /// board's writes must not leak into the result. A regression that
    /// dropped the `WHERE board = ?` filter would flunk here.
    #[test]
    fn top_scores_filters_by_board() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        world
            .set_score("investigators", alice.id, 99)
            .expect("set on investigators");
        world
            .set_score("speedrun", alice.id, 1)
            .expect("set on speedrun");

        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores succeeds");
        assert_eq!(top.len(), 1, "must only see rows for the requested board");
        assert_eq!(top[0].board, "investigators");
        assert_eq!(top[0].score, 99);
    }

    /// 's blank-board rule applies to every leaderboard verb.
    /// including reads. The shared [`validate_board_name`] guard is the
    /// single source of truth; this test pins that `top_scores` uses it.
    #[test]
    fn top_scores_rejects_empty_board_name() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        for blank in ["", "   ", "\n\t"] {
            let err = world
                .top_scores(blank, LeaderboardSort::Desc, 10)
                .expect_err("blank board name must be rejected");
            assert!(
                matches!(err, LeaderboardError::EmptyBoardName),
                "expected EmptyBoardName for {blank:?}, got {err:?}"
            );
        }
    }

    /// 's blank-board rule applies to every leaderboard verb.
    /// not just `set_score`. The shared [`validate_board_name`] guard
    /// is the single source of truth for the rule; this test pins that
    /// `increment_score` uses it.
    #[test]
    fn increment_score_rejects_empty_board_name() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");

        for blank in ["", "   ", "\n\t"] {
            let err = world
                .increment_score(blank, alice.id, 1)
                .expect_err("blank board name must be rejected");
            assert!(
                matches!(err, LeaderboardError::EmptyBoardName),
                "expected EmptyBoardName for {blank:?}, got {err:?}"
            );
        }

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM leaderboard_scores", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(
            count, 0,
            "rejected increment_score calls must not write any rows"
        );
    }

    ///   acceptance: `player_rank` reports the 1-based
    /// position the player would occupy in [`WorldDb::top_scores`] on
    /// a `Desc` board. The single best score is rank 1; the second
    /// best is rank 2; the worst is rank N. A regression that started
    /// counting from 0, or that returned the count of rows behind
    /// instead of ahead, would flunk here.
    #[test]
    fn player_rank_desc_basic_ordering() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(Some("u-carol"), Some("carol")))
            .expect("carol upsert");

        world
            .set_score("investigators", alice.id, 5)
            .expect("alice score");
        world
            .set_score("investigators", bob.id, 12)
            .expect("bob score");
        world
            .set_score("investigators", carol.id, 8)
            .expect("carol score");

        // Bob is best (12), Carol middle (8), Alice last (5) — same
        // order as `top_scores` would return.
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, bob.id)
                .expect("rank query"),
            Some(1)
        );
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, carol.id)
                .expect("rank query"),
            Some(2)
        );
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, alice.id)
                .expect("rank query"),
            Some(3)
        );
    }

    /// `Asc` boards rank lowest score first. A regression that
    /// hard-coded `Desc` semantics would put alice (highest score) at
    /// rank 1 instead of last.
    #[test]
    fn player_rank_asc_basic_ordering() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");

        world.set_score("speedrun", alice.id, 90).expect("alice");
        world.set_score("speedrun", bob.id, 45).expect("bob");

        assert_eq!(
            world
                .player_rank("speedrun", LeaderboardSort::Asc, bob.id)
                .expect("rank query"),
            Some(1),
            "lower score wins on Asc"
        );
        assert_eq!(
            world
                .player_rank("speedrun", LeaderboardSort::Asc, alice.id)
                .expect("rank query"),
            Some(2)
        );
    }

    ///  mandates that `player_rank`'s tie ordering matches
    /// `top_scores`. When two players share a score *and* an
    /// `updated_at`, the smaller `player_id` ranks ahead — pinning
    /// this invariant prevents the two helpers from drifting apart
    /// such that a player's rank line would disagree with their row
    /// in the top-scores table.
    #[test]
    fn player_rank_ties_match_top_scores_ordering() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(Some("u-carol"), Some("carol")))
            .expect("carol upsert");

        // All three identical scores; pin updated_at so the chain
        // falls through to player_id (matching top_scores' tie test).
        world
            .set_score("investigators", carol.id, 7)
            .expect("carol");
        world
            .set_score("investigators", alice.id, 7)
            .expect("alice");
        world.set_score("investigators", bob.id, 7).expect("bob");
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-05-08T12:00:00' \
                 WHERE board = 'investigators'",
                [],
            )
            .expect("pin updated_at");

        // Cross-check: the order returned by `top_scores` is the same
        // sequence of player_ids as the ranks 1..=3 here.
        let top = world
            .top_scores("investigators", LeaderboardSort::Desc, 10)
            .expect("top_scores");
        let top_ids: Vec<i64> = top.iter().map(|r| r.player_id).collect();
        assert_eq!(
            top_ids,
            vec![alice.id, bob.id, carol.id],
            "sanity: top_scores ordering"
        );

        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, alice.id)
                .expect("rank"),
            Some(1)
        );
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, bob.id)
                .expect("rank"),
            Some(2)
        );
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, carol.id)
                .expect("rank"),
            Some(3)
        );
    }

    /// Earlier `updated_at` beats later `updated_at` on a score tie
    /// same primary tiebreaker as `top_scores`. A regression that
    /// swapped the tiebreaker order in the rank query (e.g. running
    /// `player_id` ahead of `updated_at`) would flunk here.
    #[test]
    fn player_rank_updated_at_beats_player_id_on_tie() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");

        world
            .set_score("investigators", alice.id, 7)
            .expect("alice");
        world.set_score("investigators", bob.id, 7).expect("bob");

        // Backdate bob so he's the earlier writer despite having the
        // larger player_id. With player_id-first tiebreaking he'd be
        // rank 2; with updated_at-first tiebreaking he's rank 1.
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-01-01T00:00:00' \
                 WHERE board = 'investigators' AND player_id = ?1",
                rusqlite::params![bob.id],
            )
            .expect("backdate bob");
        world
            .connection()
            .execute(
                "UPDATE leaderboard_scores SET updated_at = '2026-05-08T12:00:00' \
                 WHERE board = 'investigators' AND player_id = ?1",
                rusqlite::params![alice.id],
            )
            .expect("future-date alice");

        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, bob.id)
                .expect("rank"),
            Some(1),
            "earlier updated_at must rank ahead of later"
        );
        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, alice.id)
                .expect("rank"),
            Some(2)
        );
    }

    /// A player with no row on the board is unranked: `Ok(None)`. This
    /// is the normal UI state for a new arrival before they post a
    /// score; surfacing it as `Option::None` (rather than `Some(0)` or
    /// an error) lets the leaderboard render distinguish "no entry
    /// yet" from "rank 0", which would be nonsense.
    #[test]
    fn player_rank_unranked_player_returns_none() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");
        // Only alice scores.
        world
            .set_score("investigators", alice.id, 1)
            .expect("alice");

        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, bob.id)
                .expect("rank query"),
            None,
            "player with no row must return Ok(None)"
        );
    }

    /// `player_rank` only counts rows on the requested board. Another
    /// board's writes must not influence the rank — a regression that
    /// dropped the `WHERE board = ?` filter on either the outer query
    /// or the correlated subquery would flunk here.
    #[test]
    fn player_rank_filters_by_board() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert");

        // On `investigators`, alice is alone with a low score → rank 1.
        // On `speedrun`, bob has a much better Desc score; if the
        // filter leaks, alice's investigators rank could shift.
        world.set_score("investigators", alice.id, 1).expect("a-i");
        world.set_score("speedrun", bob.id, 999).expect("b-s");

        assert_eq!(
            world
                .player_rank("investigators", LeaderboardSort::Desc, alice.id)
                .expect("rank"),
            Some(1),
            "other-board writes must not influence rank"
        );
    }

    /// 's blank-board rule applies to every leaderboard verb.
    /// Pin that `player_rank` runs through [`validate_board_name`].
    #[test]
    fn player_rank_rejects_empty_board_name() {
        let dir = tempdir().expect("tempdir creates");
        let world = open_world_with_leaderboards(&dir);

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert");

        for blank in ["", "   ", "\n\t"] {
            let err = world
                .player_rank(blank, LeaderboardSort::Desc, alice.id)
                .expect_err("blank board name must be rejected");
            assert!(
                matches!(err, LeaderboardError::EmptyBoardName),
                "expected EmptyBoardName for {blank:?}, got {err:?}"
            );
        }
    }
}
