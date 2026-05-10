//! `challenges` — shared-world rival-challenge schema (SPEC_v3 §4.2 /
//! §Task 4a).
//!
//! v3 introduces durable async player-to-player challenges: one
//! investigator dares another to solve a clue chain or beat a score
//! within a deadline, the target accepts/declines, and (eventually) one
//! side resolves the challenge with a game-defined outcome. The whole
//! lifecycle sits on top of one `challenges` table whose shape is pinned
//! by [`CHALLENGES_MIGRATION`]. This module exists only to declare that
//! schema and prove it applies; the `Challenge` Rust type and the
//! `create_challenge` / `accept_challenge` / `decline_challenge` /
//! `resolve_challenge` / `expire_open_challenges` helpers land in
//! subsequent §Task 4 sub-items (4b–4g). Splitting the migration into
//! its own commit keeps the bisect signal sharp — a column rename, a
//! relaxed `CHECK`, or a dropped partial index flunks the schema test in
//! this module rather than a higher-level state-machine test that's
//! harder to attribute.
//!
//! # Why a dedicated table
//!
//! SPEC_v3 §3 lists challenges alongside notices, market listings,
//! factions, and bounties as separate primitives. We follow the same
//! v2/v3 convention as [`crate::notices`]: one table, one migration, one
//! named index family. Folding challenges onto the `world_events` log
//! would conflate the append-only event stream with mutable lifecycle
//! state (`state`, `accepted_at`, `resolved_at`) — fundamentally
//! different write patterns, the same reason notices got their own
//! table.
//!
//! # Why `version = 7`
//!
//! v2 occupies migration versions 1–5 (see `docs/shared-world.md`
//! §8.1). v3 claims `6` and above, dense and grouped per primitive.
//! Notices took version 6 (the first v3 primitive to land). Challenges
//! take version 7. Subsequent v3 migrations (market listings, factions,
//! bounties) MUST pick the next available kit version — game-authored
//! migrations live in their own higher band and are not affected.

use crate::world_db::WorldMigration;

/// Schema for the rival-challenge table — SPEC_v3 §4.2 / §Task 4a.
///
/// One row per challenge. Challenges are mutable in the narrow sense
/// that `state`, `accepted_at`, `resolved_at`, and `result` are flipped
/// by the typed helpers landing in Tasks 4c–4f; the addressing,
/// `kind`, `stake`, and creation timestamp are write-once. The kit's
/// contract is "if you only go through the public API, the only state
/// changes are the documented transitions, and every transition runs
/// inside a SQLite transaction" (SPEC_v3 §4.2 / §7). An operator with
/// `sqlite3` can of course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`] and
/// [`crate::notices::NOTICES_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid. Doubles
///   as the deterministic tiebreaker for queries that order by
///   `created_at` and need a stable secondary sort (matching the same
///   pattern as `notices` / `world_events`).
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. ISO text so it sorts
///   lexically the same way it sorts chronologically and reads cleanly
///   in the `sqlite3` CLI — the same contract as every other v2/v3
///   timestamp column.
/// - `challenger_player_id` — `INTEGER NOT NULL REFERENCES players(id)`.
///   The investigator initiating the challenge. Required: a challenge
///   without an attributable challenger has no resolution path (who
///   gets credit?), unlike notices where system messages are a real
///   use case. Foreign-keyed for the same reason as the turn ledger
///   and notices: a phantom id should never land here. SQLite enforces
///   FKs only when `PRAGMA foreign_keys = ON`, which the runtime is
///   responsible for; until then the constraint is documentation but
///   the column shape is correct.
/// - `target_player_id` — `INTEGER NOT NULL REFERENCES players(id)`.
///   The investigator being challenged. Required: every challenge has
///   exactly one addressee, mirroring the inbox model. "Open
///   challenges" (no specific target, anyone can accept) are
///   intentionally not modelled here — a future v3.1 sub-task can add
///   a nullable target if the gameplay calls for it; v3 ships only
///   directed challenges to keep the state machine simple.
/// - `kind` — `TEXT NOT NULL`. Short machine-readable label
///   (e.g. `"clue_race"`, `"deduction_duel"`). Game authors pick the
///   namespace; the kit's only rule is "round-trips as text". Used by
///   challenge UIs to filter listings by category and by game code to
///   route resolution logic.
/// - `stake` — `TEXT`, nullable. Opaque JSON describing what's wagered
///   (XP, in-game currency, a clue token). The kit treats this column
///   as opaque — the game owns the schema, the kit owns the lifecycle.
///   Same contract as `metadata` on notices and world events. Nullable
///   because some challenges may carry no stake (a friendly duel for
///   bragging rights), and modelling "no stake" as the absence of the
///   field is cleaner than reserving a sentinel JSON value.
/// - `state` — `TEXT NOT NULL DEFAULT 'open'` with a `CHECK` constraint
///   pinning `state IN ('open','accepted','declined','resolved',
///   'expired')`. The state-machine vocabulary lives in the schema so
///   a regression that introduced a new state in code without a
///   matching migration would fail at INSERT/UPDATE time, not silently
///   in production. SPEC §4.2 documents the legal transitions:
///   `open -> accepted -> resolved`, `open -> declined`,
///   `open -> expired`. Default `'open'` matches Task 4b's "starts in
///   open" acceptance.
/// - `accepted_at` — `TEXT`, nullable. ISO timestamp the target
///   accepted the challenge. Stays `NULL` for challenges that never
///   moved past `open` (declined, expired, or still open). Storing the
///   first-accepted timestamp lets a future audit view show "accepted
///   2026-05-09 17:42 UTC" without a separate event lookup, the same
///   trick `notices.read_at` plays for the read lifecycle.
/// - `resolved_at` — `TEXT`, nullable. ISO timestamp the challenge was
///   resolved. Only populated on the `accepted -> resolved` transition;
///   declined and expired challenges leave it `NULL`. Same audit-view
///   rationale as `accepted_at`.
/// - `expires_at` — `TEXT`, nullable. ISO timestamp after which an
///   `open` challenge can be transitioned to `expired` by Task 4f's
///   sweeper. Nullable so a challenge can be open-ended (no deadline)
///   without reserving a sentinel value; the partial index below
///   filters on `expires_at IS NOT NULL` so the sweeper only walks
///   challenges that *can* expire. SPEC §4.2 requires "Expired
///   challenges cannot be accepted" — the helper enforcing that lives
///   in Task 4c, but the timestamp it consults lives here.
/// - `result` — `TEXT`, nullable. Opaque JSON describing the
///   resolution outcome (winner, payouts, narrative beats). Game code
///   computes and stores this on the `accepted -> resolved`
///   transition; the kit treats it as opaque, same contract as
///   `stake`. Nullable so a challenge that hasn't been resolved (or
///   that ended in `declined`/`expired`) is distinguishable from one
///   that resolved with an empty result object — a sentinel JSON
///   would conflate the two.
///
/// # Indexes
///
/// Two partial indexes are created up-front so the lookup patterns
/// Tasks 4c–4f rely on are seek-bound from the moment they land.
/// Adding them later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the
/// index cost at the same migration that creates the table — the same
/// rationale as `idx_notices_inbox` on [`crate::notices`].
///
/// - `idx_challenges_target_open` is a partial index over
///   `(target_player_id, created_at, id)` `WHERE state = 'open'`. This
///   covers "open challenges aimed at me" — the inbox-like screen the
///   target sees when deciding whether to accept or decline. The
///   partial predicate keeps the index small (resolved/declined/
///   expired challenges fall out of the index entirely once they leave
///   `open`) and matches the planned default query exactly.
/// - `idx_challenges_open_expiring` is a partial index over
///   `(expires_at, id)` `WHERE state = 'open' AND expires_at IS NOT
///   NULL`. The Task 4f sweeper walks this in `expires_at` order and
///   stops at the first row where `expires_at > now`, so the cost of
///   "expire all due challenges" stays proportional to the number of
///   challenges that actually need expiring — not to the total
///   challenge count. Same shape as the partial-index strategy on
///   notices.
///
/// # Version
///
/// `version = 7`. Notices claim version 6 (see
/// [`crate::notices::NOTICES_MIGRATION`]); challenges are the second
/// v3 primitive to land, so they take 7. Subsequent v3 migrations
/// (market listings, factions, bounties) take 8 and onward.
pub const CHALLENGES_MIGRATION: WorldMigration = WorldMigration {
    version: 7,
    name: "create_challenges",
    sql: "\
CREATE TABLE IF NOT EXISTS challenges (\n\
    id                    INTEGER PRIMARY KEY,\n\
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    challenger_player_id  INTEGER NOT NULL REFERENCES players(id),\n\
    target_player_id      INTEGER NOT NULL REFERENCES players(id),\n\
    kind                  TEXT NOT NULL,\n\
    stake                 TEXT,\n\
    state                 TEXT NOT NULL DEFAULT 'open'\n\
        CHECK (state IN ('open','accepted','declined','resolved','expired')),\n\
    accepted_at           TEXT,\n\
    resolved_at           TEXT,\n\
    expires_at            TEXT,\n\
    result                TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_challenges_target_open\n\
    ON challenges(target_player_id, created_at, id) WHERE state = 'open';\n\
CREATE INDEX IF NOT EXISTS idx_challenges_open_expiring\n\
    ON challenges(expires_at, id) WHERE state = 'open' AND expires_at IS NOT NULL;\n\
",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v3 §Task 4a acceptance: applying [`CHALLENGES_MIGRATION`]
    /// creates the documented `challenges` table with the column shape
    /// SPEC §4.2 pins. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.2 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 4b–4g behavioural test).
    ///
    /// The players migration is applied first because `challenges`
    /// references `players(id)` via two foreign keys (challenger and
    /// target). With FK enforcement off (the SQLite default until the
    /// runtime turns it on) the migration would succeed even without
    /// the parent table, but exercising the real dependency order here
    /// mirrors how the runtime startup path drives migrations on a
    /// real door open.
    #[test]
    fn migration_creates_challenges_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'challenges'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "challenges table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('challenges') ORDER BY cid")
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
                "challenger_player_id".to_string(),
                "target_player_id".to_string(),
                "kind".to_string(),
                "stake".to_string(),
                "state".to_string(),
                "accepted_at".to_string(),
                "resolved_at".to_string(),
                "expires_at".to_string(),
                "result".to_string(),
            ],
            "challenges column shape must match the SPEC_v3 §4.2 contract"
        );
    }

    /// SPEC §4.2 enumerates the legal states: `open`, `accepted`,
    /// `declined`, `resolved`, `expired`. The migration encodes that
    /// vocabulary via a `CHECK` constraint so a code-path that ever
    /// tried to write an out-of-vocabulary state (e.g. a typo like
    /// `'accpeted'`, or a future state added in code without a matching
    /// migration) fails at INSERT/UPDATE time rather than silently
    /// landing a corrupt row. Pin both halves here:
    ///
    /// 1. Every spec-listed state is accepted by the constraint.
    /// 2. An obviously-invalid state is rejected.
    ///
    /// This is the schema-level safety net for the typed state
    /// machine that lands in Tasks 4b–4f. The Rust helpers will
    /// additionally enforce *which* transitions are legal between
    /// states; this test only proves the alphabet itself is locked
    /// down.
    #[test]
    fn challenges_state_check_constraint_locks_vocabulary() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        // Two real player rows so the FK columns reference something
        // sensible; the FK itself isn't enforced without `PRAGMA
        // foreign_keys = ON`, but using real ids keeps the test honest
        // about what a production row looks like.
        let challenger_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-alice', 'alice') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert challenger");
        let target_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-bob', 'bob') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert target");

        // Every spec-listed state must be accepted by the CHECK.
        for state in ["open", "accepted", "declined", "resolved", "expired"] {
            world
                .connection()
                .execute(
                    "INSERT INTO challenges \
                     (challenger_player_id, target_player_id, kind, state) \
                     VALUES (?1, ?2, 'unit_test', ?3)",
                    rusqlite::params![challenger_id, target_id, state],
                )
                .unwrap_or_else(|err| panic!("state {state:?} must be accepted by CHECK: {err}"));
        }

        // An out-of-vocabulary state must be rejected. We pick an
        // obvious typo of "accepted" — the kind of mistake a future
        // refactor could plausibly introduce.
        let bogus = world.connection().execute(
            "INSERT INTO challenges \
             (challenger_player_id, target_player_id, kind, state) \
             VALUES (?1, ?2, 'unit_test', 'accpeted')",
            rusqlite::params![challenger_id, target_id],
        );
        assert!(
            bogus.is_err(),
            "CHECK constraint must reject states outside the SPEC §4.2 vocabulary"
        );
    }

    /// The Task 4c–4f helpers will read challenges via two query
    /// shapes: "open challenges aimed at this target" (the accept/
    /// decline screen) and "open challenges past their deadline" (the
    /// expiry sweeper). Both paths walk partial indexes the migration
    /// creates up-front; if either ever stops being created, the
    /// reads silently become full table scans. Pin both indexes
    /// (and their partial predicates) here so a regression flunks at
    /// `cargo test` rather than in production under load.
    ///
    /// Same rationale as `notices_inbox_partial_index_is_present` for
    /// `idx_notices_inbox` — one schema-level test per index family,
    /// asserting both its existence and the partial-predicate text.
    #[test]
    fn challenges_partial_indexes_are_present() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        // `sqlite_master.sql` for an index includes the partial
        // predicate text verbatim, so we can pin the index name and
        // its `WHERE` clause in one query.
        let target_open_sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_challenges_target_open'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_challenges_target_open");
        assert!(
            target_open_sql.contains("state = 'open'"),
            "idx_challenges_target_open must filter to state = 'open'; got: {target_open_sql}"
        );
        assert!(
            target_open_sql.contains("target_player_id"),
            "idx_challenges_target_open must lead with target_player_id; got: {target_open_sql}"
        );

        let expiring_sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_challenges_open_expiring'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_challenges_open_expiring");
        assert!(
            expiring_sql.contains("state = 'open'"),
            "idx_challenges_open_expiring must filter to state = 'open'; got: {expiring_sql}"
        );
        assert!(
            expiring_sql.contains("expires_at IS NOT NULL"),
            "idx_challenges_open_expiring must filter expires_at IS NOT NULL; got: {expiring_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; v3 inherits that contract. A
    /// second `apply_migration(&CHALLENGES_MIGRATION)` MUST be a
    /// no-op (the version is already in `world_migrations`), not an
    /// error from `CREATE TABLE` on an existing table. Same shape as
    /// `notices_migration_is_idempotent`.
    #[test]
    fn challenges_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("first challenges migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("second challenges migration applies (idempotent)");
    }
}
