//! `factions` — shared-world faction, membership, and shared-goal
//! schema (SPEC_v3 §4.4 / §Task 6a).
//!
//! v3 introduces durable async player factions: detective agencies
//! and equivalent in-game allegiances, the players who belong to
//! them, and the shared goals (clue-board totals, bounty pools,
//! contribution drives) the membership chips away at together. The
//! whole feature sits on top of three tightly coupled tables —
//! `factions`, `faction_memberships`, `shared_goals` — whose shape
//! is pinned by [`FACTIONS_MIGRATION`]. This module exists only to
//! declare that schema and prove it applies; the `Faction` /
//! `FactionMembership` / `SharedGoal` Rust types and the
//! `seed_factions` / `join_faction` / `leave_faction` /
//! `create_shared_goal` / `contribute_to_goal` helpers land in
//! subsequent §Task 6 sub-items (6b–6g). Splitting the migration
//! into its own commit keeps the bisect signal sharp — a column
//! rename, a relaxed `CHECK`, or a missing index flunks the schema
//! test in this module rather than a higher-level transactional
//! test that's harder to attribute.
//!
//! # Why one migration for three tables
//!
//! Notices, challenges, and market listings are each one table per
//! migration because each is a single conceptual primitive whose
//! row is self-contained. Factions are different: a `faction` row
//! is meaningless without its `faction_memberships` (who belongs)
//! and `shared_goals` (what the membership is working toward), and
//! the two child tables foreign-key to `factions(id)`. Bundling all
//! three into one migration means:
//!
//! 1. A door that comes up with `factions` enabled gets the full
//!    feature in one atomic version bump — no intermediate state
//!    where memberships exist but goals do not (or vice versa).
//! 2. The three tables get one shared rationale block right here,
//!    rather than three near-duplicate doc comments across three
//!    files.
//! 3. The single bisect signal for "the v3 faction schema is
//!    wrong" stays one test (`migration_creates_faction_tables`),
//!    not three that all flunk in lockstep.
//!
//! Splitting across three migrations would also burn three
//! versions on a feature that ships as one atomic primitive, which
//! complicates the kit version → SPEC band mapping documented in
//! `docs/shared-world.md` §8.1.
//!
//! # Why `version = 9`
//!
//! v2 occupies migration versions 1–5 (see `docs/shared-world.md`
//! §8.1). v3 claims `6` and above, dense and grouped per primitive.
//! Notices took 6, challenges took 7, market listings took 8.
//! Factions are the fourth v3 primitive to land, so they take 9.
//! The remaining v3 migration (bounties, §Task 7a) takes 10.

use thiserror::Error;

use crate::config::FactionSeed;
use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the faction, faction_membership, and shared_goal
/// tables — SPEC_v3 §4.4 / §Task 6a.
///
/// One migration, three tables, four indexes. The tables form a
/// single conceptual primitive; see the module-level docs for why
/// they are bundled rather than split into three migrations.
///
/// # Table: `factions`
///
/// One row per game-defined faction (detective agency, guild,
/// crew). SPEC §4.4 lists the fields: `faction id`, `slug`,
/// `display_name`, `description`, `created_at`. The kit's contract
/// is "factions are seeded from `[[factions.seed]]` in
/// `game.toml`, idempotently, by Task 6b — once seeded a faction
/// row is write-once".
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid.
///   Stable handle the membership and shared-goal rows reference.
/// - `slug` — `TEXT NOT NULL UNIQUE`. The game-author-controlled
///   stable identifier (e.g. `"blue-desk"`). The `UNIQUE`
///   constraint is what makes Task 6b's idempotent seed possible:
///   `INSERT … ON CONFLICT(slug) DO NOTHING` round-trips a single
///   row regardless of how many times the door starts. The slug
///   travels in `[[factions.seed]]` config blocks (SPEC §5.2),
///   not the autoincrement id, because config-file ids would be
///   fragile across reseeded worlds.
/// - `display_name` — `TEXT NOT NULL`. Human-facing label rendered
///   in the agency-selection screen and goal headers. Game
///   authors own the namespace; player-authored display names are
///   not allowed at the schema layer (factions are content, not
///   user-generated like notices).
/// - `description` — `TEXT NOT NULL`. Short flavour blurb for the
///   selection screen. Required at the schema layer because every
///   faction MUST render a meaningful card in the agency picker;
///   a NULL here would produce a UI gap that's indistinguishable
///   from a rendering bug.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`.
///   UTC ISO timestamp. Same convention as every other v2/v3
///   timestamp column: text sort = chronological sort, reads
///   cleanly under `sqlite3` CLI.
///
/// # Table: `faction_memberships`
///
/// One row per (player, faction) pair. SPEC §4.4 explicitly says
/// "One player MAY belong to multiple factions unless the game
/// config restricts it" — the schema therefore does not impose a
/// "one faction per player" constraint; that policy lives in the
/// `[multiplayer]` config block and is enforced (or not) by the
/// game's join screen, not the storage layer.
///
/// - `id` — `INTEGER PRIMARY KEY`. Stable membership handle. Lets
///   Task 6c/6d return a struct that can be addressed unambiguously
///   even if the same player rejoins the same faction after
///   leaving (a new row, a new id; the old row is preserved as
///   audit trail).
/// - `player_id` — `INTEGER NOT NULL REFERENCES players(id)`. FK
///   to the v2 players table; same enforcement caveat as every
///   other v2/v3 FK column (active only with `PRAGMA
///   foreign_keys = ON`).
/// - `faction_id` — `INTEGER NOT NULL REFERENCES factions(id)`.
///   The faction the player belongs to.
/// - `role` — `TEXT NOT NULL DEFAULT 'member'`. SPEC §4.4 lists
///   `role` in the membership shape. v3 ships with no enforced
///   vocabulary (no `CHECK` constraint) because the role lexicon
///   is game-defined: a noir detective agency might use
///   `'rookie'/'sergeant'`, a heist crew `'driver'/'fence'`. The
///   default `'member'` is the kit's neutral fallback so
///   `join_faction` (Task 6c) can stay a one-argument call for
///   games that don't model roles at all.
/// - `joined_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. ISO
///   timestamp the player joined.
/// - `left_at` — `TEXT`, nullable. ISO timestamp the player left,
///   or `NULL` while the membership is active. Task 6d stamps
///   this column rather than `DELETE`-ing the row so departures
///   are audit-trail visible — the same soft-delete convention as
///   `notices.archived_at`. The "active membership" view is
///   anchored by the `idx_faction_memberships_active` partial
///   index below.
///
/// A `UNIQUE (player_id, faction_id, joined_at)` index would be
/// over-tight (a player who leaves and rejoins on the same second
/// would clash). Instead, "one *active* membership per (player,
/// faction)" is enforced by Task 6c's helper inside its
/// transaction, and the partial index below guarantees the read
/// path stays seek-bound.
///
/// # Table: `shared_goals`
///
/// One row per shared goal (clue-board total, contribution drive,
/// shared bounty pool). SPEC §4.4 fields: `goal id`, `faction_id`
/// (nullable — a goal may be world-wide rather than scoped to one
/// faction), `key`, `target_amount`, `current_amount`, `state`.
///
/// - `id` — `INTEGER PRIMARY KEY`.
/// - `faction_id` — `INTEGER REFERENCES factions(id)`, nullable.
///   SPEC §4.4 explicitly lists "faction id optional"; world-wide
///   goals (e.g. "the city solves 100 cases") have `NULL` here.
/// - `key` — `TEXT NOT NULL`. Game-authored stable identifier
///   (e.g. `"blue-desk.clue-board"`). Combined with `faction_id`
///   it forms a logical lookup key that game code uses to find
///   "the active goal for this faction"; the unique-key index
///   below makes that lookup seek-bound and prevents accidental
///   duplicate goal rows.
/// - `target_amount` — `INTEGER NOT NULL CHECK (target_amount > 0)`.
///   The amount the contribution drive needs to reach. A goal
///   with target 0 is meaningless (instantly complete on creation),
///   so the `CHECK` rejects it at the schema layer rather than
///   leaving the trap open for Task 6e.
/// - `current_amount` — `INTEGER NOT NULL DEFAULT 0
///   CHECK (current_amount >= 0)`. Running total. Decrements are
///   not part of the v3 contract — `contribute_to_goal` only adds
///   — so the lower-bound `CHECK` is the safety net for any path
///   that bypasses the helper.
/// - `state` — `TEXT NOT NULL DEFAULT 'active'
///   CHECK (state IN ('active','completed'))`. Two-state machine.
///   `active` while `current_amount < target_amount`; transitions
///   to `completed` exactly once when the helper detects the
///   target was reached (Task 6g). The `CHECK` constraint pins the
///   vocabulary so a typo in a future helper (`'compelted'`)
///   flunks at INSERT time rather than landing a row the UI
///   cannot interpret.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`.
/// - `completed_at` — `TEXT`, nullable. Stamped at the moment
///   `state` flips to `'completed'`. Lets the UI render "solved
///   at 02:14" without re-deriving the timestamp from the world
///   event log.
///
/// # Indexes
///
/// Four named indexes are created up-front so the read paths
/// Task 6b–6g rely on are seek-bound from the moment they land.
/// Same rationale as the partial indexes on `notices`,
/// `challenges`, and `market_listings`: pay the index cost at the
/// same migration that creates the table, never in a follow-up.
///
/// - `idx_factions_slug` — implicit via the `UNIQUE` constraint
///   on `factions.slug`. Backs the idempotent-seed lookup
///   (Task 6b) and the `slug → faction` resolver future screens
///   will use. Materialised by SQLite, not by an explicit
///   `CREATE INDEX`.
/// - `idx_faction_memberships_active` — partial index over
///   `(player_id, faction_id)` `WHERE left_at IS NULL`. Backs
///   "what factions does this player currently belong to" and
///   "is this player a member of this faction" queries. Partial
///   predicate keeps the index small (left memberships are
///   excluded) and matches Task 6c/6d's expected query shape
///   exactly.
/// - `idx_faction_memberships_by_faction` — partial index over
///   `(faction_id, joined_at, id)` `WHERE left_at IS NULL`. Backs
///   "list active members of this faction" — the agency roster
///   screen. The `(joined_at, id)` tail orders members by tenure
///   with a deterministic id tiebreaker, mirroring the
///   `(created_at, id)` convention on the other v3 tables.
/// - `idx_shared_goals_faction_key` — `UNIQUE (faction_id, key)`.
///   Pins "one goal per (faction, key)". Required for Task 6e's
///   create-or-find helper to be safely idempotent under the same
///   `INSERT … ON CONFLICT DO NOTHING` shape as faction seeding.
///   `faction_id` participates in the uniqueness so two factions
///   may share the same `key` (e.g. each agency has its own
///   `"clue-board"` goal). NULL `faction_id` participates per
///   SQLite's default semantics: NULLs are not equal, so multiple
///   world-wide goals with the same key are permitted — game code
///   that wants to forbid that resolves it at the helper layer.
/// - `idx_shared_goals_active` — partial index over `(faction_id,
///   key)` `WHERE state = 'active'`. Backs "find the open goal
///   matching this (faction, key)" without scanning completed
///   rows.
///
/// # Version
///
/// `version = 9`. v2 uses 1–5; v3 uses 6+ (notices=6,
/// challenges=7, market_listings=8). Factions are the fourth v3
/// primitive to land, so they take 9. Bounties (§Task 7a) take 10.
pub const FACTIONS_MIGRATION: WorldMigration = WorldMigration {
    version: 9,
    name: "create_factions",
    sql: "\
CREATE TABLE IF NOT EXISTS factions (\n\
    id           INTEGER PRIMARY KEY,\n\
    slug         TEXT NOT NULL UNIQUE,\n\
    display_name TEXT NOT NULL,\n\
    description  TEXT NOT NULL,\n\
    created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n\
);\n\
CREATE TABLE IF NOT EXISTS faction_memberships (\n\
    id         INTEGER PRIMARY KEY,\n\
    player_id  INTEGER NOT NULL REFERENCES players(id),\n\
    faction_id INTEGER NOT NULL REFERENCES factions(id),\n\
    role       TEXT NOT NULL DEFAULT 'member',\n\
    joined_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    left_at    TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_faction_memberships_active\n\
    ON faction_memberships(player_id, faction_id) WHERE left_at IS NULL;\n\
CREATE INDEX IF NOT EXISTS idx_faction_memberships_by_faction\n\
    ON faction_memberships(faction_id, joined_at, id) WHERE left_at IS NULL;\n\
CREATE TABLE IF NOT EXISTS shared_goals (\n\
    id             INTEGER PRIMARY KEY,\n\
    faction_id     INTEGER REFERENCES factions(id),\n\
    key            TEXT NOT NULL,\n\
    target_amount  INTEGER NOT NULL CHECK (target_amount > 0),\n\
    current_amount INTEGER NOT NULL DEFAULT 0 CHECK (current_amount >= 0),\n\
    state          TEXT NOT NULL DEFAULT 'active'\n\
        CHECK (state IN ('active','completed')),\n\
    created_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    completed_at   TEXT\n\
);\n\
CREATE UNIQUE INDEX IF NOT EXISTS idx_shared_goals_faction_key\n\
    ON shared_goals(faction_id, key);\n\
CREATE INDEX IF NOT EXISTS idx_shared_goals_active\n\
    ON shared_goals(faction_id, key) WHERE state = 'active';\n\
",
};

/// Read model for one row of the `factions` table — SPEC_v3 §4.4.
///
/// Returned by [`WorldDb::seed_factions`] so callers receive the
/// canonical row SQLite produced (autoincrement `id`, SQL-side
/// `created_at`) rather than echoing back the input config. Future
/// v3 helpers (`join_faction`, agency selector queries) will read
/// the same shape.
///
/// `display_name` and `description` are mirrored from the seed row
/// because v3 treats faction rows as write-once after first seed —
/// see [`WorldDb::seed_factions`] for the rationale. If a future
/// version ever wants editable faction metadata, the change lands
/// in a new helper, not by mutating this struct's contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Faction {
    /// Autoincrement primary key. Stable for the life of the world
    /// DB; the kit references factions by id internally and by slug
    /// at the config boundary.
    pub id: i64,
    /// Game-author-controlled stable identifier (e.g. `"blue-desk"`).
    /// Round-trips verbatim from the `[[factions.seed]]` config
    /// entry; validated for shape at config-load time.
    pub slug: String,
    /// Human-facing label. Pinned at first seed and not updated by
    /// subsequent seed calls (write-once contract).
    pub display_name: String,
    /// One-sentence flavour text. Same write-once contract as
    /// `display_name`.
    pub description: String,
    /// SQLite-assigned UTC ISO timestamp (`CURRENT_TIMESTAMP`) of
    /// the first seed call that materialised this row. Stable
    /// across subsequent idempotent seed calls.
    pub created_at: String,
}

/// Errors raised while seeding or otherwise mutating faction state.
///
/// Library-internal `thiserror` per the v3 convention shared with
/// [`crate::notices::NoticeError`] and [`crate::market::MarketError`].
/// Today only the SQL boundary error is needed — config-side
/// validation (slug shape, non-empty fields, duplicate slugs) is
/// already enforced by `crate::config::GameConfig::validate` at
/// load time, so by the time a `&[FactionSeed]` reaches
/// [`WorldDb::seed_factions`] every entry is structurally sound.
/// New variants slot in here as later §Task 6 sub-items (membership,
/// shared goals) ship their own helpers.
#[derive(Debug, Error)]
pub enum FactionError {
    /// A SQL statement in the seed transaction failed. Wrapping
    /// `rusqlite::Error` keeps the call site readable (one error
    /// type, one mapping) while preserving the underlying cause for
    /// `tracing` and operator-facing messages.
    #[error("failed to seed factions: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the failing statement.
        #[source]
        source: rusqlite::Error,
    },
}

impl WorldDb {
    /// Idempotently upsert configured factions into the world DB —
    /// SPEC_v3 §4.4 / §Task 6b.
    ///
    /// The contract is "every slug in `seeds` corresponds to a
    /// `factions` row after this call returns, and calling again
    /// with the same input is a no-op". Concretely:
    ///
    /// 1. For each seed, run `INSERT … ON CONFLICT(slug) DO NOTHING`
    ///    against `factions`. The `UNIQUE(slug)` constraint from
    ///    [`FACTIONS_MIGRATION`] is what makes the conflict clause
    ///    safe — a second run sees the existing row and skips the
    ///    insert without raising.
    /// 2. SELECT the canonical row by slug inside the same
    ///    transaction so the helper returns the same id, display_name,
    ///    description, and created_at every call regardless of which
    ///    invocation actually inserted the row.
    /// 3. Commit. If any statement fails the entire batch rolls back
    ///    so the `factions` table never lands in a "half-seeded"
    ///    state that the agency picker would render with gaps.
    ///
    /// # Write-once contract
    ///
    /// `display_name` and `description` are deliberately *not*
    /// updated when a slug already exists. SPEC_v3 §4.4 + the
    /// schema doc on [`FACTIONS_MIGRATION`] state that "once seeded
    /// a faction row is write-once". A game author who wants to
    /// rename a detective agency mid-run picks a new slug; an
    /// operator who wants to rewrite history edits the row through
    /// `sqlite3` directly. This avoids the surprise of a config
    /// edit silently rewriting historical bounty/membership
    /// references that game UIs may have rendered with the old
    /// name.
    ///
    /// # Empty input is a valid no-op
    ///
    /// `seeds.is_empty()` returns an empty `Vec` without opening a
    /// transaction. Doors that disable factions in `[multiplayer]`
    /// (or simply ship none in `[[factions.seed]]`) call this
    /// helper at startup with an empty slice; paying for a `BEGIN`
    /// round-trip in that path would be wasted I/O.
    ///
    /// # Concurrency
    ///
    /// Takes `&mut self` so we can open a `rusqlite::Transaction`
    /// via the crate-internal `connection_mut` accessor. Same shape as
    /// [`WorldDb::buy_listing`]; a `&self` variant would force the
    /// helper to leak SQL across multiple statements without
    /// transactional grouping, defeating the all-or-nothing seeding
    /// guarantee under a concurrent door open hitting the same
    /// world DB file.
    pub fn seed_factions(&mut self, seeds: &[FactionSeed]) -> Result<Vec<Faction>, FactionError> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        // Open a single transaction across all seeds: any failure
        // (a DB lock timeout, a corrupt schema) rolls every insert
        // back so the `factions` table is either fully seeded or
        // unchanged — never half-seeded with the agency picker
        // missing entries the runtime expects. Mirrors the
        // transactional shape of `buy_listing`.
        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| FactionError::Sqlite { source })?;

        // `INSERT … ON CONFLICT(slug) DO NOTHING` is the
        // idempotency primitive. The conflict target uses the
        // schema-level `UNIQUE(slug)` constraint declared in
        // FACTIONS_MIGRATION; relying on `UNIQUE` rather than a
        // bespoke "SELECT then INSERT" handshake makes the path
        // race-free without needing a SAVEPOINT or busy-loop.
        const INSERT_SQL: &str = "\
INSERT INTO factions (slug, display_name, description) \
VALUES (?1, ?2, ?3) \
ON CONFLICT(slug) DO NOTHING";

        // Read the canonical row back after the upsert so the
        // returned `Faction` reflects the values actually in the
        // table — important for the write-once contract: if a
        // previous seed call established `display_name = "Original"`
        // and this call ships `"Updated"`, the caller MUST observe
        // the persisted `"Original"`, not the input it just
        // submitted. This SELECT runs inside the same transaction
        // so the read sees a consistent post-insert snapshot.
        const SELECT_SQL: &str = "\
SELECT id, slug, display_name, description, created_at \
FROM factions WHERE slug = ?1";

        let mut out = Vec::with_capacity(seeds.len());
        for seed in seeds {
            tx.execute(
                INSERT_SQL,
                rusqlite::params![&seed.slug, &seed.display_name, &seed.description],
            )
            .map_err(|source| FactionError::Sqlite { source })?;

            let faction = tx
                .query_row(SELECT_SQL, rusqlite::params![&seed.slug], row_to_faction)
                .map_err(|source| FactionError::Sqlite { source })?;
            out.push(faction);
        }

        tx.commit()
            .map_err(|source| FactionError::Sqlite { source })?;
        Ok(out)
    }
}

/// Decode one `factions` row in the column order shared by
/// [`WorldDb::seed_factions`]'s SELECT and any future faction
/// query helper. Centralised so a column rename touches one place.
fn row_to_faction(row: &rusqlite::Row<'_>) -> rusqlite::Result<Faction> {
    Ok(Faction {
        id: row.get(0)?,
        slug: row.get(1)?,
        display_name: row.get(2)?,
        description: row.get(3)?,
        created_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v3 §Task 6a acceptance: applying [`FACTIONS_MIGRATION`]
    /// creates the three documented tables (`factions`,
    /// `faction_memberships`, `shared_goals`) with the column shape
    /// SPEC §4.4 pins. Asserts both:
    ///
    /// 1. Each table exists in `sqlite_master` (so a regression
    ///    that silently dropped a `CREATE TABLE` from the migration
    ///    body would flunk).
    /// 2. The columns and order of each table match the SPEC §4.4
    ///    contract (so a later edit that renames or reorders a
    ///    column flunks here rather than buried in a 6b–6g
    ///    behavioural test).
    ///
    /// The players migration is applied first because both
    /// `faction_memberships` and `shared_goals` reach players via
    /// `factions(id)` (transitively) and `faction_memberships`
    /// references `players(id)` directly. With FK enforcement off
    /// (the SQLite default until the runtime turns it on) the
    /// migration would succeed even without the parent table, but
    /// exercising the real dependency order here mirrors how the
    /// runtime startup path drives migrations on a real door open.
    #[test]
    fn migration_creates_faction_tables() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&FACTIONS_MIGRATION)
            .expect("factions migration applies");

        for table in ["factions", "faction_memberships", "shared_goals"] {
            let count: i64 = world
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("sqlite_master query runs");
            assert_eq!(
                count, 1,
                "{table} table must exist after FACTIONS_MIGRATION applies"
            );
        }

        let factions_columns = pragma_columns(&world, "factions");
        assert_eq!(
            factions_columns,
            vec![
                "id".to_string(),
                "slug".to_string(),
                "display_name".to_string(),
                "description".to_string(),
                "created_at".to_string(),
            ],
            "factions column shape must match the SPEC_v3 §4.4 contract"
        );

        let membership_columns = pragma_columns(&world, "faction_memberships");
        assert_eq!(
            membership_columns,
            vec![
                "id".to_string(),
                "player_id".to_string(),
                "faction_id".to_string(),
                "role".to_string(),
                "joined_at".to_string(),
                "left_at".to_string(),
            ],
            "faction_memberships column shape must match the SPEC_v3 §4.4 contract"
        );

        let goal_columns = pragma_columns(&world, "shared_goals");
        assert_eq!(
            goal_columns,
            vec![
                "id".to_string(),
                "faction_id".to_string(),
                "key".to_string(),
                "target_amount".to_string(),
                "current_amount".to_string(),
                "state".to_string(),
                "created_at".to_string(),
                "completed_at".to_string(),
            ],
            "shared_goals column shape must match the SPEC_v3 §4.4 contract"
        );
    }

    /// SPEC §4.4 implies the shared-goal state machine is
    /// `active -> completed` (Task 6g flips it). The migration
    /// encodes that vocabulary via a `CHECK` constraint so a code
    /// path that ever tried to write an out-of-vocabulary state
    /// (e.g. `'closed'`, `'cancelled'`, a typo like `'compelted'`)
    /// fails at INSERT/UPDATE time rather than silently landing a
    /// corrupt row. Same shape as
    /// `challenges_state_check_constraint_locks_vocabulary`.
    #[test]
    fn shared_goals_state_check_constraint_locks_vocabulary() {
        let (_dir, world) = world_with_factions();

        for state in ["active", "completed"] {
            world
                .connection()
                .execute(
                    "INSERT INTO shared_goals (key, target_amount, state) \
                     VALUES (?1, 100, ?2)",
                    rusqlite::params![format!("k-{state}"), state],
                )
                .unwrap_or_else(|err| panic!("state {state:?} must be accepted by CHECK: {err}"));
        }

        let bogus = world.connection().execute(
            "INSERT INTO shared_goals (key, target_amount, state) \
             VALUES ('k-bogus', 100, 'cancelled')",
            [],
        );
        assert!(
            bogus.is_err(),
            "CHECK constraint must reject states outside the SPEC §4.4 vocabulary"
        );
    }

    /// SPEC §4.4 says contributions accumulate toward a target.
    /// The schema enforces `target_amount > 0` (a goal with
    /// target 0 would be instantly complete on creation, which
    /// every UI surface would render as "solved" before any work
    /// is done — meaningless) and `current_amount >= 0` (the
    /// helper only adds, but a raw-SQL bypass shouldn't be able to
    /// store a negative running total). Pin both halves so a
    /// regression that relaxed either `CHECK` flunks here.
    #[test]
    fn shared_goals_amount_check_constraints_reject_invalid_values() {
        let (_dir, world) = world_with_factions();

        let bad_target = world.connection().execute(
            "INSERT INTO shared_goals (key, target_amount) VALUES ('k1', 0)",
            [],
        );
        assert!(
            bad_target.is_err(),
            "CHECK constraint must reject target_amount = 0"
        );

        let bad_target_neg = world.connection().execute(
            "INSERT INTO shared_goals (key, target_amount) VALUES ('k2', -5)",
            [],
        );
        assert!(
            bad_target_neg.is_err(),
            "CHECK constraint must reject negative target_amount"
        );

        let bad_current = world.connection().execute(
            "INSERT INTO shared_goals (key, target_amount, current_amount) \
             VALUES ('k3', 10, -1)",
            [],
        );
        assert!(
            bad_current.is_err(),
            "CHECK constraint must reject negative current_amount"
        );
    }

    /// The Task 6b idempotent-seed helper relies on the `UNIQUE`
    /// constraint on `factions.slug` to upsert a faction without
    /// duplicating rows. Pin the constraint at the schema layer
    /// here so a regression that dropped `UNIQUE` flunks at
    /// `cargo test` rather than silently allowing duplicate
    /// `('blue-desk', …)` rows on every door restart.
    #[test]
    fn factions_slug_is_unique() {
        let (_dir, world) = world_with_factions();

        world
            .connection()
            .execute(
                "INSERT INTO factions (slug, display_name, description) \
                 VALUES ('blue-desk', 'Blue Desk Agency', 'first')",
                [],
            )
            .expect("first insert succeeds");

        let dup = world.connection().execute(
            "INSERT INTO factions (slug, display_name, description) \
             VALUES ('blue-desk', 'Blue Desk Agency', 'second')",
            [],
        );
        assert!(
            dup.is_err(),
            "UNIQUE constraint on factions.slug must reject duplicate slugs"
        );
    }

    /// The Task 6c–6d/6f helpers walk three partial indexes the
    /// migration creates up-front. If any of them ever stops being
    /// created, the read silently becomes a full table scan in
    /// production. Pin every index name plus its partial predicate
    /// so a regression flunks at `cargo test` rather than under
    /// load. Same rationale as
    /// `challenges_partial_indexes_are_present` and
    /// `market_listings_active_partial_index_is_present`.
    #[test]
    fn factions_partial_indexes_are_present() {
        let (_dir, world) = world_with_factions();

        let active_membership_sql = index_sql(&world, "idx_faction_memberships_active");
        assert!(
            active_membership_sql.contains("left_at IS NULL"),
            "idx_faction_memberships_active must filter to left_at IS NULL; got: {active_membership_sql}"
        );
        assert!(
            active_membership_sql.contains("player_id"),
            "idx_faction_memberships_active must lead with player_id; got: {active_membership_sql}"
        );

        let by_faction_sql = index_sql(&world, "idx_faction_memberships_by_faction");
        assert!(
            by_faction_sql.contains("left_at IS NULL"),
            "idx_faction_memberships_by_faction must filter to left_at IS NULL; got: {by_faction_sql}"
        );
        assert!(
            by_faction_sql.contains("faction_id"),
            "idx_faction_memberships_by_faction must lead with faction_id; got: {by_faction_sql}"
        );

        let goal_unique_sql = index_sql(&world, "idx_shared_goals_faction_key");
        assert!(
            goal_unique_sql.to_uppercase().contains("UNIQUE"),
            "idx_shared_goals_faction_key must be UNIQUE; got: {goal_unique_sql}"
        );

        let goal_active_sql = index_sql(&world, "idx_shared_goals_active");
        assert!(
            goal_active_sql.contains("state = 'active'"),
            "idx_shared_goals_active must filter to state = 'active'; got: {goal_active_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; v3 inherits that contract.
    /// A second `apply_migration(&FACTIONS_MIGRATION)` MUST be a
    /// no-op (the version is already in `world_migrations`), not
    /// an error from `CREATE TABLE` on an existing table. Same
    /// shape as `challenges_migration_is_idempotent` and
    /// `market_listings_migration_is_idempotent`.
    #[test]
    fn factions_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&FACTIONS_MIGRATION)
            .expect("first factions migration applies");
        world
            .apply_migration(&FACTIONS_MIGRATION)
            .expect("second factions migration applies (idempotent)");
    }

    /// SPEC_v3 §Task 6b acceptance: `seed_factions` materialises one
    /// row per input seed and returns the canonical `Faction` rows.
    /// Pins the happy path so a regression that swapped the SELECT
    /// to read by id (instead of slug) or that dropped the post-
    /// insert read entirely flunks here rather than in a
    /// downstream agency-picker test.
    #[test]
    fn seed_factions_inserts_all_seeds_and_returns_canonical_rows() {
        let (_dir, mut world) = world_with_factions();

        let seeds = vec![
            FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk Agency".to_string(),
                description: "Methodical, by-the-book investigators.".to_string(),
            },
            FactionSeed {
                slug: "red-room".to_string(),
                display_name: "Red Room Detectives".to_string(),
                description: "Hard-boiled, ask-questions-later types.".to_string(),
            },
        ];

        let seeded = world.seed_factions(&seeds).expect("seed succeeds");

        assert_eq!(seeded.len(), 2, "one Faction per input seed");
        assert_eq!(seeded[0].slug, "blue-desk");
        assert_eq!(seeded[0].display_name, "Blue Desk Agency");
        assert_eq!(
            seeded[0].description,
            "Methodical, by-the-book investigators."
        );
        assert!(
            seeded[0].id > 0,
            "id assigned by SQLite autoincrement, got {}",
            seeded[0].id
        );
        assert!(
            !seeded[0].created_at.is_empty(),
            "created_at populated by CURRENT_TIMESTAMP default"
        );
        assert_eq!(seeded[1].slug, "red-room");
        assert_ne!(
            seeded[0].id, seeded[1].id,
            "distinct slugs must produce distinct ids"
        );

        // Belt-and-braces: confirm rows are durably committed by
        // counting through a fresh statement. A regression that
        // forgot to call `commit()` would leave the rows visible
        // inside the helper's transaction but absent here.
        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM factions", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(count, 2, "both seeds committed to the table");
    }

    /// SPEC_v3 §Task 6b acceptance criterion: "second seed call does
    /// not duplicate". Doors restart on every player session, so
    /// `seed_factions` runs at every startup; without idempotency
    /// the table would grow a duplicate row every restart and the
    /// agency picker would render N×restart-count entries.
    ///
    /// Verifies three independent regression vectors with one test:
    ///
    /// 1. The second call returns the *same* ids as the first
    ///    (catches a regression that re-keyed factions on every run).
    /// 2. The second call returns the *same* `created_at` (catches
    ///    a regression that wrote a fresh timestamp on each call).
    /// 3. The total row count after two calls is still N (catches
    ///    the most direct regression — `INSERT OR IGNORE` removed
    ///    or the `ON CONFLICT` clause flipped to `DO UPDATE`).
    #[test]
    fn seed_factions_is_idempotent_under_repeated_calls() {
        let (_dir, mut world) = world_with_factions();

        let seeds = vec![
            FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk Agency".to_string(),
                description: "first description".to_string(),
            },
            FactionSeed {
                slug: "red-room".to_string(),
                display_name: "Red Room Detectives".to_string(),
                description: "first description".to_string(),
            },
        ];

        let first = world.seed_factions(&seeds).expect("first seed succeeds");
        let second = world.seed_factions(&seeds).expect("second seed succeeds");

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(
                a.id, b.id,
                "id stable across seed calls for slug {}",
                a.slug
            );
            assert_eq!(
                a.created_at, b.created_at,
                "created_at stable across seed calls for slug {}",
                a.slug
            );
        }

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM factions", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            count, 2,
            "second seed call MUST NOT duplicate rows (got {count})"
        );
    }

    /// Write-once contract: when a slug already exists in the
    /// `factions` table, a subsequent `seed_factions` call with the
    /// same slug but updated `display_name` / `description` MUST
    /// return the *persisted* values, not the new input. Pins the
    /// `ON CONFLICT(slug) DO NOTHING` clause against a regression
    /// that flipped it to `DO UPDATE` (which would silently rewrite
    /// historical references in bounty/membership UIs that captured
    /// the old name).
    #[test]
    fn seed_factions_does_not_update_existing_rows() {
        let (_dir, mut world) = world_with_factions();

        let original = vec![FactionSeed {
            slug: "blue-desk".to_string(),
            display_name: "Original Name".to_string(),
            description: "Original description.".to_string(),
        }];
        let first = world.seed_factions(&original).expect("first seed succeeds");

        let updated = vec![FactionSeed {
            slug: "blue-desk".to_string(),
            display_name: "Updated Name".to_string(),
            description: "Updated description.".to_string(),
        }];
        let second = world.seed_factions(&updated).expect("second seed succeeds");

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, first[0].id, "same slug returns same id");
        assert_eq!(
            second[0].display_name, "Original Name",
            "display_name is write-once: updated input MUST NOT overwrite"
        );
        assert_eq!(
            second[0].description, "Original description.",
            "description is write-once: updated input MUST NOT overwrite"
        );
    }

    /// Empty input is a documented no-op: doors that disable
    /// factions in `[multiplayer]` (or simply ship no seeds) call
    /// `seed_factions(&[])` at startup. The helper MUST short-
    /// circuit before opening a transaction so the cold-path I/O
    /// cost is zero.
    #[test]
    fn seed_factions_with_empty_input_is_a_no_op() {
        let (_dir, mut world) = world_with_factions();

        let seeded = world.seed_factions(&[]).expect("empty seed succeeds");
        assert!(seeded.is_empty());

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM factions", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(count, 0, "empty seed MUST NOT touch the table");
    }

    /// Mixed-state input: some slugs already exist, some are new.
    /// The single-call mix is what the runtime hits when a game
    /// author adds a new agency to `[[factions.seed]]` between
    /// player sessions. The new entry MUST land while the existing
    /// entries stay write-once.
    #[test]
    fn seed_factions_inserts_new_slugs_alongside_existing_ones() {
        let (_dir, mut world) = world_with_factions();

        let initial = vec![FactionSeed {
            slug: "blue-desk".to_string(),
            display_name: "Blue Desk".to_string(),
            description: "first".to_string(),
        }];
        let first = world
            .seed_factions(&initial)
            .expect("initial seed succeeds");

        let mixed = vec![
            FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Renamed Blue Desk".to_string(),
                description: "renamed".to_string(),
            },
            FactionSeed {
                slug: "red-room".to_string(),
                display_name: "Red Room".to_string(),
                description: "newly added".to_string(),
            },
        ];
        let second = world.seed_factions(&mixed).expect("mixed seed succeeds");

        assert_eq!(second.len(), 2);
        // Existing slug retains original name (write-once).
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(second[0].display_name, "Blue Desk");
        // New slug gets a fresh id and lands.
        assert_ne!(second[1].id, first[0].id);
        assert_eq!(second[1].display_name, "Red Room");

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM factions", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(count, 2, "one new row added, original retained");
    }

    /// Helper: open a fresh world DB with `players` and
    /// `factions` migrations applied. Returned tuple keeps the
    /// `TempDir` alive for the test's scope (dropping it would
    /// unlink the SQLite file mid-test). Same shape as
    /// `notices::tests::world_with_notices` and
    /// `market::tests::world_with_listings`.
    fn world_with_factions() -> (tempfile::TempDir, WorldDb) {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&FACTIONS_MIGRATION)
            .expect("factions migration applies");
        (dir, world)
    }

    /// Read column names from `pragma_table_info` in cid order —
    /// the storage-side column order, which is what the column-
    /// shape assertions pin.
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
    /// raw SQL includes the partial predicate so callers can
    /// assert on `WHERE …` clauses.
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
