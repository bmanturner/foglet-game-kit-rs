//! `turns` — daily turn ledger schema (SPEC_v2 §Task 6).
//!
//! Task 6a (this iteration) ships the `turn_ledger` migration only.
//! Subsequent sub-tasks layer behavior on top of the schema introduced
//! here:
//!
//! - 6b adds an injectable date provider so deterministic tests can
//!   simulate "tomorrow" without sleeping for 24 hours.
//! - 6c creates today's row from the configured `daily_allowance` the
//!   first time a player asks for their balance.
//! - 6d implements the atomic spend path.
//! - 6e rejects insufficient-turn spends without mutating the row.
//! - 6f handles new-day reset, including carryover capped by
//!   `[turns].carryover_max`.
//!
//! Splitting the migration into its own commit keeps the bisect signal
//! sharp: a regression that drops a column flunks the schema test in
//! this module rather than a higher-level spend assertion that's harder
//! to attribute. The migration is exported as a `pub const` so the
//! runtime startup path (Task 10) and game-author code can reference one
//! canonical definition without redeclaring the schema and drifting from
//! it — same pattern as [`crate::players::PLAYERS_MIGRATION`].
//!
//! # Why a per-(player, date) row instead of a single rolling balance
//!
//! SPEC_v2 §4.6 mandates four behaviors that all assume a notion of
//! "today's allowance":
//!
//! 1. Initialize a player with today's allowance.
//! 2. Spend turns atomically (today's balance shrinks).
//! 3. Reset on a new day (yesterday's row is no longer the active one).
//! 4. Carry over up to `carryover_max` to the new day.
//!
//! A single `players.balance` column would force the runtime to detect
//! "is today a new day?" *and* mutate balance in the same round-trip,
//! racing every other writer. Keeping one row per (`player_id`,
//! `local_date`) makes the active row a pure lookup (`WHERE player_id =
//! ? AND local_date = ?`) and reduces the carryover step (Task 6f) to
//! "read yesterday's balance, write today's row" — both single-row
//! operations the SPEC §9 transaction wrapper can compose without
//! touching unrelated rows.
//!
//! It also gives operators a queryable history: a sysop poking at
//! `sqlite3` can see *when* a player burned their turns rather than
//! just the current count. That's not a SPEC requirement, but it's a
//! free side-effect of the keying choice and worth not throwing away.

use crate::world_db::WorldMigration;

/// Schema for the daily turn ledger — SPEC_v2 §4.6 / §Task 6a.
///
/// One row per (`player_id`, `local_date`) pair. The "active" row for a
/// player on a given day is the one matching today's local date in the
/// configured timezone (Task 6b will introduce the date provider that
/// makes "today" testable). Old rows are retained on purpose so the
/// carryover step (6f) can read yesterday's balance directly and so
/// operators have a queryable history.
///
/// # Column shape
///
/// - `player_id` — `INTEGER NOT NULL REFERENCES players(id)`. Foreign
///   keyed to the player registry from Task 5 so the ledger can never
///   refer to a phantom identity. SQLite enforces foreign keys only
///   when `PRAGMA foreign_keys = ON`; the runtime layer (Task 10) is
///   responsible for enabling it at open time. Until then the constraint
///   is documentation, but the column shape is already correct so
///   enabling FK enforcement later is a one-line change rather than a
///   migration.
/// - `local_date` — `TEXT NOT NULL`. Stored as `YYYY-MM-DD` in the
///   server's local timezone (the only `[turns].reset` value v2 ships
///   is `local_midnight`). Text rather than `INTEGER` because the
///   sortable ISO format is human-readable in the `sqlite3` CLI and
///   round-trips cleanly through `chrono::NaiveDate` once Task 6b lands.
/// - `balance` — `INTEGER NOT NULL`. Remaining turns for that day.
///   Allowed to be zero (a player who burned every turn) but never
///   negative — Task 6e's insufficient-turn rejection lives in code
///   rather than as a `CHECK` constraint so the failure surfaces with
///   a typed error instead of `SQLITE_CONSTRAINT`. We could add the
///   `CHECK` belt-and-braces later; for now the simpler schema wins.
/// - `daily_allowance` — `INTEGER NOT NULL`. Snapshot of the
///   `[turns].daily_allowance` config value at the moment this row was
///   created. Stored (rather than recomputed from config) so an
///   operator who lowers the allowance mid-day doesn't retroactively
///   shrink yesterday's balances, and so the carryover step (6f) can
///   compute "unspent turns today" as `daily_allowance - balance` —
///   wait, that's only true when balance hasn't been touched by 6d
///   below. We store the original allowance to keep the row
///   self-describing for operators reading it cold.
/// - `created_at` / `updated_at` — UTC timestamps for audit. Defaulted
///   to `CURRENT_TIMESTAMP` so 6c can `INSERT` without threading a
///   clock; 6d updates `updated_at` on every spend.
///
/// # Primary key choice
///
/// `(player_id, local_date)` is the natural key — the SPEC's "keyed by
/// player and local date" wording in CHECKLIST_v2 §6a maps directly
/// onto a composite primary key. SQLite implements this as a
/// non-rowid covering index, so today-row lookups (`WHERE player_id =
/// ? AND local_date = ?`) are an index seek and the carryover lookup
/// (`WHERE player_id = ? AND local_date < ? ORDER BY local_date DESC
/// LIMIT 1`) is also seek-bound thanks to the left-prefix on
/// `player_id`.
///
/// Declaring it `WITHOUT ROWID` would shave a row of overhead per
/// entry; we don't, because (a) the table is small (one row per player
/// per day), (b) `WITHOUT ROWID` rules out future `RETURNING rowid`
/// patterns, and (c) the SPEC doesn't ask for it.
///
/// # Version
///
/// `version = 3`. Versions 1 and 2 are reserved for future kit-level
/// migrations and the players table respectively. Game-authored
/// migrations (Murder Motel's `motel_world_state` from Task 12a) start
/// from a higher band so they don't collide with kit migrations the
/// runtime applies on every open.
pub const TURN_LEDGER_MIGRATION: WorldMigration = WorldMigration {
    version: 3,
    name: "create_turn_ledger",
    sql: "\
CREATE TABLE IF NOT EXISTS turn_ledger (\n\
    player_id        INTEGER NOT NULL REFERENCES players(id),\n\
    local_date       TEXT NOT NULL,\n\
    balance          INTEGER NOT NULL,\n\
    daily_allowance  INTEGER NOT NULL,\n\
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    PRIMARY KEY (player_id, local_date)\n\
);\n\
",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v2 §Task 6a acceptance: applying [`TURN_LEDGER_MIGRATION`]
    /// creates the documented `turn_ledger` table with the column shape
    /// later sub-tasks (6c–6f) depend on. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC contract (so a later
    ///    edit that renames or reorders a column flunks here rather
    ///    than buried in a 6d spend test).
    ///
    /// We apply the players migration first because `turn_ledger`
    /// references it via `FOREIGN KEY`. With FK enforcement off (the
    /// SQLite default until Task 10 turns it on) the migration would
    /// succeed even without the parent table, but exercising the real
    /// dependency order here mirrors how the runtime startup path will
    /// drive migrations on a real door open.
    #[test]
    fn migration_creates_turn_ledger_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'turn_ledger'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "turn_ledger table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('turn_ledger') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "player_id".to_string(),
                "local_date".to_string(),
                "balance".to_string(),
                "daily_allowance".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
            ],
            "turn_ledger column shape must match the documented contract"
        );
    }

    /// The composite primary key `(player_id, local_date)` is what
    /// makes "today's row" a single index lookup and lets the Task 6f
    /// carryover step `SELECT … ORDER BY local_date DESC LIMIT 1`
    /// without a sequential scan. A regression that downgrades it to a
    /// simple `INTEGER PRIMARY KEY` (or drops the composite altogether)
    /// would silently degrade the ledger and let two rows for the same
    /// (player, day) pair coexist.
    #[test]
    fn turn_ledger_primary_key_is_player_id_and_local_date() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        // `pragma_table_info` reports the position of each column
        // within the primary key in its `pk` field (1-based, 0 for
        // non-PK columns). Querying it directly is more robust than
        // parsing `sqlite_master.sql`, which formats the key in
        // implementation-defined whitespace.
        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, pk FROM pragma_table_info('turn_ledger') \
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
            vec![("player_id".to_string(), 1), ("local_date".to_string(), 2),],
            "primary key must be (player_id, local_date) in that order"
        );
    }

    /// Inserting two rows that share `(player_id, local_date)` must
    /// raise a uniqueness error — the contract Task 6c–6f relies on
    /// when it reads "today's row" without first locking the table.
    /// Without this guarantee a race between two writers could leave
    /// the ledger with two contradictory balance rows for the same
    /// (player, day) pair and SPEC §4.6's "spend turns atomically"
    /// promise would be unenforceable.
    #[test]
    fn turn_ledger_rejects_duplicate_player_day_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn_ledger migration applies");

        // Insert a parent player so the FK column has something to
        // reference. We don't enable `PRAGMA foreign_keys` (Task 10's
        // job) so the parent isn't strictly required, but writing a
        // realistic row keeps the test true to how the runtime will
        // drive the table.
        world
            .connection()
            .execute("INSERT INTO players (id, handle) VALUES (1, 'alice')", [])
            .expect("seed player row");

        world
            .connection()
            .execute(
                "INSERT INTO turn_ledger (player_id, local_date, balance, daily_allowance) \
                 VALUES (1, '2026-05-08', 30, 30)",
                [],
            )
            .expect("first ledger row inserts");

        let err = world
            .connection()
            .execute(
                "INSERT INTO turn_ledger (player_id, local_date, balance, daily_allowance) \
                 VALUES (1, '2026-05-08', 29, 30)",
                [],
            )
            .expect_err("duplicate (player_id, local_date) must be rejected");

        // Don't pin the exact `rusqlite::Error` variant — SQLite's
        // wording around constraint violations evolves between
        // versions. Asserting the message names the table is enough
        // for a regression to point at the right spot.
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("turn_ledger") || msg.to_lowercase().contains("unique"),
            "duplicate insert error should mention turn_ledger or uniqueness, got: {msg}"
        );
    }
}
