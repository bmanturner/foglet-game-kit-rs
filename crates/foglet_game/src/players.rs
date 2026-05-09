//! `players` — shared-world player registry (SPEC_v2 §Task 5).
//!
//! Task 5a (this commit) ships the `players` table migration only.
//! Subsequent sub-tasks build on it:
//!
//! - 5b — `PlayerRecord` plus `WorldDb::upsert_player(&FogletContext)`.
//! - 5c — Local-dev fallback identity for missing `user_id`.
//! - 5d — `last_seen_at` refresh on repeat upsert.
//! - 5e — `FogletRole` parsing and `security_level` mapping.
//! - 5f — Persist normalized role/security metadata at upsert time.
//!
//! Splitting the migration into its own iteration keeps every commit
//! small enough to bisect cleanly: a regression that drops the
//! `local_dev_key` column will flunk the schema test in this module
//! rather than a higher-level upsert assertion that's harder to
//! attribute. The migration itself is a `pub const` so other modules
//! (the runtime startup path in Task 10, future Murder Motel
//! migrations in Task 12) can reference one canonical definition
//! instead of redeclaring the schema and drifting from it.

use crate::world_db::WorldMigration;

/// Schema for the shared-world player registry — SPEC_v2 §4.4.
///
/// One row per stable player identity (Foglet user OR local-dev
/// fallback). The shape mirrors §4.4 exactly:
///
/// - `id` — internal autoincrement primary key. Foreign-keyed by
///   later tables (`turn_ledger` in Task 6a, `world_events` in 7a,
///   `leaderboard_scores` in 8a) so per-player joins stay numeric and
///   cheap. `INTEGER PRIMARY KEY` is SQLite's idiom for a stable
///   `rowid` alias.
/// - `foglet_user_id` — nullable string. Populated when
///   `FogletContext.user_id` is present; left null on local-dev
///   sessions that haven't been issued a Foglet identity yet.
/// - `handle` — display string. SPEC §4.4 calls out that this is a
///   **display value, not an authorization key** — kept `NOT NULL`
///   because every player needs *something* to render, even a
///   default like "guest".
/// - `role` — normalized `FogletRole` text (`"sysop"`, `"mod"`,
///   `"user"`). Stored as text rather than an integer so an operator
///   inspecting the SQLite file with the `sqlite3` CLI can read it
///   without consulting source. Defaults to `"user"` at the SQL layer
///   so 5b (upsert) doesn't have to special-case missing roles.
/// - `security_level` — integer derived from role unless Foglet
///   supplies an explicit value (Task 5e mapping: sysop=100, mod=90,
///   user=50). Stored as an integer for ordering/comparison; the
///   mapping itself lives in code so the rules stay in one place.
/// - `first_seen_at` — UTC timestamp of the player's first upsert.
///   `DEFAULT CURRENT_TIMESTAMP` so 5b can `INSERT` without threading
///   a clock; 5d preserves this on repeat upserts (only `last_seen_at`
///   moves).
/// - `last_seen_at` — UTC timestamp updated on every upsert per 5d.
///   Defaulted the same way as `first_seen_at` so a freshly-inserted
///   row already has a sensible value.
/// - `local_dev_key` — nullable string identifying local-dev
///   identities (Task 5c). Distinct from `foglet_user_id` so the two
///   identity namespaces never collide: a Foglet user "alice" and a
///   local-dev "alice" land on separate rows, both queryable.
///
/// # Uniqueness
///
/// The migration creates two **partial unique indexes** rather than
/// declaring `UNIQUE` constraints inline:
///
/// - `idx_players_foglet_user_id` — unique over `foglet_user_id`
///   when not null. SQLite treats `NULL` as distinct under a plain
///   `UNIQUE` constraint, which would silently allow multiple
///   local-dev rows; the partial index (`WHERE foglet_user_id IS NOT
///   NULL`) makes the intent explicit and matches the SPEC §4.4 rule
///   that `user_id` "is the stable key" *when present*.
/// - `idx_players_local_dev_key` — unique over `local_dev_key`
///   when not null, for the same reason on the local-dev side.
///
/// Future tasks (5b upsert, 5c local-dev key) rely on these indexes
/// for `INSERT … ON CONFLICT` upserts; introducing them now keeps
/// the schema and the future write path in lockstep.
pub const PLAYERS_MIGRATION: WorldMigration = WorldMigration {
    version: 2,
    name: "create_players",
    sql: "\
CREATE TABLE IF NOT EXISTS players (\n\
    id              INTEGER PRIMARY KEY,\n\
    foglet_user_id  TEXT,\n\
    handle          TEXT NOT NULL,\n\
    role            TEXT NOT NULL DEFAULT 'user',\n\
    security_level  INTEGER NOT NULL DEFAULT 50,\n\
    first_seen_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    last_seen_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    local_dev_key   TEXT\n\
);\n\
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_foglet_user_id\n\
    ON players(foglet_user_id) WHERE foglet_user_id IS NOT NULL;\n\
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_local_dev_key\n\
    ON players(local_dev_key) WHERE local_dev_key IS NOT NULL;\n\
",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v2 §Task 5a acceptance: applying [`PLAYERS_MIGRATION`]
    /// records the version *and* leaves the documented column shape
    /// behind. Pinning both halves in one test means a regression that
    /// renames a column (5d's `last_seen_at` is the most likely
    /// candidate) flunks here rather than in a 5b upsert assertion
    /// where the cause is harder to localise.
    #[test]
    fn applies_players_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies cleanly to a fresh DB");

        // `pragma_table_info` is the canonical "describe this table"
        // query in SQLite. Asserting on the ordered column-name list
        // (rather than just `COUNT(*) = 8`) catches a regression that
        // drops one column and adds another by accident.
        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('players') ORDER BY cid")
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
                "foglet_user_id".to_string(),
                "handle".to_string(),
                "role".to_string(),
                "security_level".to_string(),
                "first_seen_at".to_string(),
                "last_seen_at".to_string(),
                "local_dev_key".to_string(),
            ],
            "players schema must match SPEC_v2 §4.4 exactly"
        );

        // Bookkeeping row recorded at the migration's declared version
        // — proves the standard apply path was used (vs. a side-channel
        // `execute_batch`) so the relaunch idempotency test in Task 4c
        // continues to apply.
        let recorded: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params![PLAYERS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(recorded, PLAYERS_MIGRATION.version);
    }

    /// Re-applying [`PLAYERS_MIGRATION`] is a no-op (Task 4c
    /// idempotency carries forward to the kit's built-in migrations,
    /// not just author-supplied ones). Without this guard a relaunch
    /// against an already-bootstrapped DB would surface a `table
    /// already exists` error from the second `CREATE TABLE` in the
    /// batch — the `IF NOT EXISTS` clauses make the SQL re-runnable
    /// even if the idempotency check ever regressed.
    #[test]
    fn players_migration_is_idempotent_on_repeat_apply() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("first apply succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("second apply is a no-op, not an error");

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE version = ?1",
                rusqlite::params![PLAYERS_MIGRATION.version],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(row_count, 1, "repeat apply must not duplicate the row");
    }

    /// Partial unique index on `foglet_user_id` rejects duplicates
    /// while still allowing multiple local-dev rows where the column
    /// is null. This is the property Task 5b's upsert path relies on
    /// — without it, two concurrent Foglet sessions for the same user
    /// could create separate registry rows and split a player's
    /// turn ledger and leaderboard score across them.
    #[test]
    fn foglet_user_id_unique_when_not_null() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let conn = world.connection();
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle) VALUES (?1, ?2)",
            rusqlite::params!["u-alice", "alice"],
        )
        .expect("first insert succeeds");

        let dup = conn.execute(
            "INSERT INTO players (foglet_user_id, handle) VALUES (?1, ?2)",
            rusqlite::params!["u-alice", "alice-twin"],
        );
        assert!(
            dup.is_err(),
            "duplicate foglet_user_id must be rejected by the partial unique index"
        );

        // Two NULL `foglet_user_id` rows coexist — the partial index
        // intentionally excludes null so local-dev identities (which
        // have no Foglet user_id yet) aren't forced to share a row.
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle, local_dev_key) VALUES (NULL, ?1, ?2)",
            rusqlite::params!["dev1", "dev-key-1"],
        )
        .expect("first null-user_id row succeeds");
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle, local_dev_key) VALUES (NULL, ?1, ?2)",
            rusqlite::params!["dev2", "dev-key-2"],
        )
        .expect("second null-user_id row coexists");
    }

    /// Companion guard to [`foglet_user_id_unique_when_not_null`] for
    /// the local-dev key namespace. Task 5c will lean on this so two
    /// local-dev sessions with the same synthesized key resolve to a
    /// single registry row instead of forking the player's history.
    #[test]
    fn local_dev_key_unique_when_not_null() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let conn = world.connection();
        conn.execute(
            "INSERT INTO players (handle, local_dev_key) VALUES (?1, ?2)",
            rusqlite::params!["alice", "local:alice"],
        )
        .expect("first local-dev insert succeeds");

        let dup = conn.execute(
            "INSERT INTO players (handle, local_dev_key) VALUES (?1, ?2)",
            rusqlite::params!["alice-twin", "local:alice"],
        );
        assert!(
            dup.is_err(),
            "duplicate local_dev_key must be rejected by the partial unique index"
        );
    }

    /// Defaults — role and security_level fall back to `'user'` / `50`
    /// when the caller doesn't specify them. 5b's upsert path will
    /// rely on this for any context that arrives without role
    /// information, so the SQL-side default is the load-bearing piece.
    #[test]
    fn defaults_for_role_and_security_level_apply() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        world
            .connection()
            .execute(
                "INSERT INTO players (handle) VALUES (?1)",
                rusqlite::params!["someone"],
            )
            .expect("minimal insert succeeds");

        let (role, security_level): (String, i64) = world
            .connection()
            .query_row(
                "SELECT role, security_level FROM players WHERE handle = ?1",
                rusqlite::params!["someone"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query succeeds");
        assert_eq!(role, "user");
        assert_eq!(security_level, 50);
    }
}
