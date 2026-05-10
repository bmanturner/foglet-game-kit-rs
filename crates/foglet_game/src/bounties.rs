//! `bounties` — shared-world bounty/job-board schema (SPEC_v3 §4.5 /
//! §Task 7a).
//!
//! v3 introduces a durable async bounty board: a poster (a player or
//! the system) writes up a job with a reward payload, claimants pick
//! it up, and one of them eventually completes it with game-defined
//! evidence. The whole lifecycle sits on top of a single `bounties`
//! table whose shape is pinned by [`BOUNTIES_MIGRATION`]. This module
//! exists only to declare that schema and prove it applies; the
//! `Bounty` Rust type and the `post_bounty` / `claim_bounty` /
//! `complete_bounty` / `expire_bounties` helpers land in subsequent
//! §Task 7 sub-items (7b–7f). Splitting the migration into its own
//! commit keeps the bisect signal sharp — a column rename, a relaxed
//! `CHECK`, or a dropped partial index flunks the schema test in this
//! module rather than a higher-level state-machine test that's harder
//! to attribute. Same convention as [`crate::challenges`] §Task 4a
//! and [`crate::market`] §Task 5a.
//!
//! # Why a dedicated table
//!
//! SPEC_v3 §3 lists bounties alongside notices, challenges, market
//! listings, and factions as separate primitives. We follow the same
//! v2/v3 convention: one table, one migration, one named index
//! family. Folding bounties onto `world_events` would conflate the
//! append-only event stream with mutable lifecycle state (`state`,
//! `claimed_by_player_id`, `claimed_at`, `completed_at`) —
//! fundamentally different write patterns. Folding bounties onto
//! `challenges` would force one state machine to model two domains
//! (rival challenges are private and 1:1; bounties are public and
//! 1:N → 1) and would burn an extra migration to add the columns
//! later.
//!
//! # Why `version = 10`
//!
//! v2 occupies migration versions 1–5 (see `docs/shared-world.md`
//! §8.1). v3 claims `6` and above, dense and grouped per primitive:
//! notices=6, challenges=7, market_listings=8, factions=9. Bounties
//! are the fifth and final v3 primitive to land, so they take 10.
//! Game-authored migrations live in their own higher band and are
//! not affected.

use crate::world_db::WorldMigration;

/// Schema for the bounty/job-board table — SPEC_v3 §4.5 / §Task 7a.
///
/// One row per bounty. Bounties are mutable in the narrow sense that
/// `state`, `claimed_by_player_id`, `claimed_at`, and `completed_at`
/// are flipped by the typed helpers landing in Tasks 7b–7e; the
/// addressing, `title`, `description`, `reward`, and creation
/// timestamp are write-once. The kit's contract is "if you only go
/// through the public API, the only state changes are the documented
/// transitions, and every transition runs inside a SQLite
/// transaction" (SPEC_v3 §4.5 / §7). An operator with `sqlite3` can
/// of course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`],
/// [`crate::notices::NOTICES_MIGRATION`],
/// [`crate::challenges::CHALLENGES_MIGRATION`],
/// [`crate::market::MARKET_LISTINGS_MIGRATION`], and
/// [`crate::factions::FACTIONS_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid.
///   Doubles as the deterministic tiebreaker for queries that order
///   by `created_at` and need a stable secondary sort, matching the
///   convention on every other v2/v3 primitive.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. ISO text so it
///   sorts lexically the same way it sorts chronologically and
///   reads cleanly under the `sqlite3` CLI — the same contract as
///   every other v2/v3 timestamp column.
/// - `posted_by_player_id` — `INTEGER REFERENCES players(id)`,
///   nullable. SPEC §4.5 explicitly lists "posted_by player id
///   optional": detective agencies, the city, or other in-game NPC
///   organisations may post bounties without a real Foglet user
///   behind them. Mirrors [`crate::notices`]'s
///   `sender_player_id` convention. SQLite enforces FKs only when
///   `PRAGMA foreign_keys = ON`, which the runtime is responsible
///   for; until then the constraint is documentation but the column
///   shape is correct.
/// - `title` — `TEXT NOT NULL`. Short player-facing headline shown
///   in the bounty board listing. Required: a bounty without a
///   title can't be rendered as a board entry. Length bounding
///   lives at the helper layer (Task 7b) — the schema enforces
///   non-null but not max length, the same convention as
///   `notices.subject` and `market_listings.display_name`.
/// - `description` — `TEXT NOT NULL`. Longer body copy explaining
///   the work, the evidence required, and any flavour the poster
///   wants to attach. Required for the same reason: an empty
///   description on the detail screen is indistinguishable from a
///   render bug. Length bounding lives at the helper layer.
/// - `reward` — `TEXT NOT NULL`. Opaque JSON describing the payout
///   (in-game currency, items, XP, faction reputation). The kit
///   treats this column as opaque — the game owns the schema, the
///   kit owns the lifecycle. Same contract as `challenges.stake`,
///   `notices.metadata`, and `world_events.metadata`. Marked
///   `NOT NULL` because SPEC §4.5 lists `reward` without an
///   "optional" modifier (unlike `posted_by`) — every bounty
///   advertises a payout, even if the JSON encodes "0 credits"
///   for narrative-only bounties.
/// - `state` — `TEXT NOT NULL DEFAULT 'open'` with a `CHECK`
///   constraint pinning `state IN
///   ('open','claimed','completed','expired')`. The state-machine
///   vocabulary lives in the schema so a regression that introduced
///   a new state in code without a matching migration would fail at
///   INSERT/UPDATE time, not silently in production. SPEC §4.5
///   documents the legal transitions through the §Task 7b–7e
///   acceptance criteria: `open -> claimed -> completed`,
///   `open -> expired`, `claimed -> expired`. Default `'open'`
///   matches Task 7b's "starts open" acceptance. Same shape as the
///   `state` column on `challenges`.
/// - `claimed_by_player_id` — `INTEGER REFERENCES players(id)`,
///   nullable. The investigator who claimed the bounty. Stays
///   `NULL` for `open` and `expired`-without-claim rows; populated
///   on the `open -> claimed` transition (Task 7c) and preserved
///   through `claimed -> completed` (Task 7d) so the completion
///   credit stays attributable. SPEC §4.5 lists this as
///   "claimed_by optional player id".
/// - `claimed_at` — `TEXT`, nullable. ISO timestamp of the
///   `open -> claimed` transition. Same audit-view rationale as
///   `challenges.accepted_at` — lets the UI show "claimed
///   2026-05-09 17:42 UTC" without a separate event lookup.
/// - `completed_at` — `TEXT`, nullable. ISO timestamp of the
///   `claimed -> completed` transition. Stays `NULL` for bounties
///   that never completed (still open, still claimed, expired).
/// - `expires_at` — `TEXT`, nullable. ISO timestamp after which the
///   Task 7e sweeper may flip an `open` or `claimed` bounty to
///   `expired`. Nullable so a bounty can be open-ended (no
///   deadline) without reserving a sentinel value; the partial
///   index below filters on `expires_at IS NOT NULL` so the sweeper
///   only walks bounties that *can* expire. Same shape as
///   `challenges.expires_at`.
///
/// # Indexes
///
/// Three partial indexes are created up-front so the lookup patterns
/// Tasks 7b–7e rely on are seek-bound from the moment they land.
/// Adding them later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the
/// index cost at the same migration that creates the table — the
/// same rationale as the partial indexes on `notices`, `challenges`,
/// `market_listings`, and `factions`.
///
/// - `idx_bounties_open` is a partial index over
///   `(created_at, id)` `WHERE state = 'open'`. This covers the
///   bounty-board listing screen — "all open bounties, newest
///   first" — which is the dominant read path. The partial
///   predicate keeps the index small (claimed/completed/expired
///   bounties fall out of the index entirely once they leave
///   `open`) and matches the planned default query exactly. No
///   per-poster filter is included in the index because the board
///   is a single shared list; if a future "my open bounties" view
///   is added, it walks the same index and filters in-memory.
/// - `idx_bounties_claimant_active` is a partial index over
///   `(claimed_by_player_id, claimed_at, id)` `WHERE state =
///   'claimed' AND claimed_by_player_id IS NOT NULL`. Backs "what
///   bounties is this player working on right now" — the
///   personal-quest-list view. The partial predicate keeps
///   completed and expired claims out of the index. The
///   `IS NOT NULL` clause is belt-and-braces: a `'claimed'` row
///   without a claimant id would be a logic bug, but the index
///   declines to spend bytes on it either way.
/// - `idx_bounties_expiring` is a partial index over
///   `(expires_at, id)` `WHERE state IN ('open','claimed') AND
///   expires_at IS NOT NULL`. The Task 7e sweeper walks this in
///   `expires_at` order and stops at the first row where
///   `expires_at > now`, so the cost of "expire all due bounties"
///   stays proportional to the number of bounties that actually
///   need expiring — not to the total bounty count. Both `open`
///   and `claimed` bounties are eligible for expiry per SPEC §4.5
///   (a claimant who never completes their work shouldn't pin the
///   bounty open forever); the index covers both states with one
///   partial predicate.
///
/// # Version
///
/// `version = 10`. v2 uses 1–5; v3 uses 6+ (notices=6, challenges=7,
/// market_listings=8, factions=9). Bounties are the fifth and final
/// v3 primitive to land, so they take 10. v3.1+ migrations pick up
/// at 11.
pub const BOUNTIES_MIGRATION: WorldMigration = WorldMigration {
    version: 10,
    name: "create_bounties",
    sql: "\
CREATE TABLE IF NOT EXISTS bounties (\n\
    id                    INTEGER PRIMARY KEY,\n\
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    posted_by_player_id   INTEGER REFERENCES players(id),\n\
    title                 TEXT NOT NULL,\n\
    description           TEXT NOT NULL,\n\
    reward                TEXT NOT NULL,\n\
    state                 TEXT NOT NULL DEFAULT 'open'\n\
        CHECK (state IN ('open','claimed','completed','expired')),\n\
    claimed_by_player_id  INTEGER REFERENCES players(id),\n\
    claimed_at            TEXT,\n\
    completed_at          TEXT,\n\
    expires_at            TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_bounties_open\n\
    ON bounties(created_at, id) WHERE state = 'open';\n\
CREATE INDEX IF NOT EXISTS idx_bounties_claimant_active\n\
    ON bounties(claimed_by_player_id, claimed_at, id)\n\
    WHERE state = 'claimed' AND claimed_by_player_id IS NOT NULL;\n\
CREATE INDEX IF NOT EXISTS idx_bounties_expiring\n\
    ON bounties(expires_at, id)\n\
    WHERE state IN ('open','claimed') AND expires_at IS NOT NULL;\n\
",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v3 §Task 7a acceptance: applying [`BOUNTIES_MIGRATION`]
    /// creates the documented `bounties` table with the column shape
    /// SPEC §4.5 pins. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped `CREATE TABLE` from the migration body
    ///    would flunk).
    /// 2. The columns and order match the SPEC §4.5 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 7b–7e behavioural test).
    ///
    /// The players migration is applied first because `bounties`
    /// references `players(id)` for both `posted_by_player_id` and
    /// `claimed_by_player_id`. With FK enforcement off (the SQLite
    /// default until the runtime turns it on) the migration would
    /// succeed even without the parent table, but exercising the
    /// real dependency order here mirrors how the runtime startup
    /// path drives migrations on a real door open.
    #[test]
    fn migration_creates_bounties_table() {
        let (_dir, world) = world_with_bounties();

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'bounties'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "bounties table must exist after BOUNTIES_MIGRATION applies"
        );

        let columns = pragma_columns(&world, "bounties");
        assert_eq!(
            columns,
            vec![
                "id".to_string(),
                "created_at".to_string(),
                "posted_by_player_id".to_string(),
                "title".to_string(),
                "description".to_string(),
                "reward".to_string(),
                "state".to_string(),
                "claimed_by_player_id".to_string(),
                "claimed_at".to_string(),
                "completed_at".to_string(),
                "expires_at".to_string(),
            ],
            "bounties column shape must match the SPEC_v3 §4.5 contract"
        );
    }

    /// SPEC §4.5 implies the bounty state machine is
    /// `open -> claimed -> completed`, plus `open -> expired` and
    /// `claimed -> expired` (Tasks 7c–7e flip the columns). The
    /// migration encodes that vocabulary via a `CHECK` constraint
    /// so a code path that ever tried to write an out-of-vocabulary
    /// state (e.g. `'cancelled'`, `'paid'`, a typo like
    /// `'compelted'`) fails at INSERT/UPDATE time rather than
    /// silently landing a corrupt row. Same shape as
    /// `challenges_state_check_constraint_locks_vocabulary`.
    #[test]
    fn bounties_state_check_constraint_locks_vocabulary() {
        let (_dir, world) = world_with_bounties();

        // Every spec-listed state must be accepted by the CHECK.
        for state in ["open", "claimed", "completed", "expired"] {
            world
                .connection()
                .execute(
                    "INSERT INTO bounties \
                     (title, description, reward, state) \
                     VALUES (?1, 'desc', '{}', ?2)",
                    rusqlite::params![format!("t-{state}"), state],
                )
                .unwrap_or_else(|err| panic!("state {state:?} must be accepted by CHECK: {err}"));
        }

        // An out-of-vocabulary state must be rejected. Pick an
        // obvious typo — the kind a future refactor might
        // plausibly introduce.
        let bogus = world.connection().execute(
            "INSERT INTO bounties (title, description, reward, state) \
             VALUES ('t-bogus', 'desc', '{}', 'cancelled')",
            [],
        );
        assert!(
            bogus.is_err(),
            "CHECK constraint must reject states outside the SPEC §4.5 vocabulary"
        );
    }

    /// The Task 7b/7c/7d/7e helpers walk three partial indexes the
    /// migration creates up-front. If any of them ever stops being
    /// created, the read silently becomes a full table scan in
    /// production. Pin every index name plus its partial predicate
    /// so a regression flunks at `cargo test` rather than under
    /// load. Same rationale as
    /// `challenges_partial_indexes_are_present`,
    /// `market_listings_active_partial_index_is_present`, and
    /// `factions_partial_indexes_are_present`.
    #[test]
    fn bounties_partial_indexes_are_present() {
        let (_dir, world) = world_with_bounties();

        let open_sql = index_sql(&world, "idx_bounties_open");
        assert!(
            open_sql.contains("state = 'open'"),
            "idx_bounties_open must filter to state = 'open'; got: {open_sql}"
        );
        assert!(
            open_sql.contains("created_at"),
            "idx_bounties_open must lead with created_at; got: {open_sql}"
        );

        let claimant_sql = index_sql(&world, "idx_bounties_claimant_active");
        assert!(
            claimant_sql.contains("state = 'claimed'"),
            "idx_bounties_claimant_active must filter to state = 'claimed'; got: {claimant_sql}"
        );
        assert!(
            claimant_sql.contains("claimed_by_player_id IS NOT NULL"),
            "idx_bounties_claimant_active must filter claimed_by_player_id IS NOT NULL; got: {claimant_sql}"
        );

        let expiring_sql = index_sql(&world, "idx_bounties_expiring");
        assert!(
            expiring_sql.contains("expires_at IS NOT NULL"),
            "idx_bounties_expiring must filter expires_at IS NOT NULL; got: {expiring_sql}"
        );
        // Both 'open' and 'claimed' bounties must be eligible for
        // expiry per SPEC §4.5 — a claimant who never completes
        // their work shouldn't pin the bounty open forever. Pin
        // both states in the predicate so a regression that
        // narrowed the index to one state flunks here.
        assert!(
            expiring_sql.contains("'open'") && expiring_sql.contains("'claimed'"),
            "idx_bounties_expiring must cover both 'open' and 'claimed'; got: {expiring_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; v3 inherits that contract.
    /// A second `apply_migration(&BOUNTIES_MIGRATION)` MUST be a
    /// no-op (the version is already in `world_migrations`), not
    /// an error from `CREATE TABLE` on an existing table. Same
    /// shape as `challenges_migration_is_idempotent`,
    /// `market_listings_migration_is_idempotent`, and
    /// `factions_migration_is_idempotent`.
    #[test]
    fn bounties_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&BOUNTIES_MIGRATION)
            .expect("first bounties migration applies");
        world
            .apply_migration(&BOUNTIES_MIGRATION)
            .expect("second bounties migration applies (idempotent)");
    }

    /// Helper: open a fresh world DB with `players` and `bounties`
    /// migrations applied. Returned tuple keeps the `TempDir` alive
    /// for the test's scope (dropping it would unlink the SQLite
    /// file mid-test). Same shape as
    /// `notices::tests::world_with_notices`,
    /// `market::tests::world_with_listings`, and
    /// `factions::tests::world_with_factions`.
    fn world_with_bounties() -> (tempfile::TempDir, WorldDb) {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&BOUNTIES_MIGRATION)
            .expect("bounties migration applies");
        (dir, world)
    }

    /// Read column names from `pragma_table_info` in cid order —
    /// the storage-side column order, which is what the column-
    /// shape assertion pins.
    fn pragma_columns(world: &WorldDb, table: &str) -> Vec<String> {
        let mut stmt = world
            .connection()
            .prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
            ))
            .expect("pragma_table_info preparable");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode")
    }

    /// Read the `sqlite_master.sql` text for an index by name. The
    /// raw SQL includes the partial predicate so callers can assert
    /// on `WHERE …` clauses.
    fn index_sql(world: &WorldDb, name: &str) -> String {
        world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap_or_else(|err| panic!("sqlite_master lookup for {name} failed: {err}"))
    }
}
