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
    /// [`WorldDb::leave_faction`] was called for a `(player_id,
    /// faction_id)` pair that has no row in `faction_memberships` —
    /// neither active nor historical. The player has never joined
    /// this faction, so there is nothing to leave. Distinct from
    /// "already left" (which the helper handles idempotently and
    /// returns `Ok` for); the typed variant lets the agency-roster
    /// UI render "you aren't a member" rather than swallowing it as
    /// a generic SQL failure.
    #[error("player {player_id} has no membership row in faction {faction_id}")]
    NotMember {
        /// Player id that was supplied to the leave call.
        player_id: i64,
        /// Faction id that was supplied to the leave call.
        faction_id: i64,
    },
    /// [`WorldDb::create_shared_goal`] was called with a
    /// `target_amount` of zero or negative. The schema enforces
    /// `CHECK (target_amount > 0)`; pre-validating in Rust lets the
    /// helper short-circuit before opening a transaction and surface
    /// a typed variant the agency-config UI can render as "target
    /// must be positive" without parsing a generic SQLite CHECK
    /// failure. Same shape as
    /// [`crate::market::MarketError::NonPositiveBuyQuantity`].
    #[error("shared-goal target_amount must be > 0 (got {actual})")]
    InvalidTargetAmount {
        /// The non-positive value the caller supplied.
        actual: i64,
    },
    /// [`WorldDb::contribute_to_goal`] was called with a
    /// non-positive `amount`. SPEC §4.4 frames a contribution as
    /// "current_amount += amount"; an `amount` of zero is a no-op
    /// that would still appear to "succeed" to UI callers, and a
    /// negative amount would silently rewind progress for every
    /// other contributor — both worth refusing at the typed-error
    /// boundary so a stuck "0" or a sign-flip bug in a contribution
    /// keypad surfaces here rather than as a confusing world-state
    /// regression. Same defensive shape as
    /// [`crate::market::MarketError::NonPositiveBuyQuantity`].
    #[error("shared-goal contribution amount must be > 0 (got {actual})")]
    NonPositiveContribution {
        /// The non-positive value the caller supplied. Echoed so the
        /// contribution UI can re-render the offending input.
        actual: i64,
    },
    /// [`WorldDb::contribute_to_goal`] referenced a `goal_id` that
    /// has no row in `shared_goals`. Distinct from
    /// [`Self::GoalAlreadyCompleted`] so the agency UI can offer a
    /// targeted "this goal has been removed, refresh your view"
    /// recovery rather than a generic "couldn't contribute" toast.
    /// Same shape as [`crate::market::MarketError::NotFound`] and
    /// [`crate::challenges::ChallengeError::NotFound`].
    #[error("shared goal id {id} was not found")]
    GoalNotFound {
        /// Goal id the caller passed.
        id: i64,
    },
    /// [`WorldDb::contribute_to_goal`] was called against a goal
    /// whose `state` is already `'completed'`. SPEC §4.4 pins the
    /// state machine at `active -> completed`; once flipped, further
    /// contributions are rejected so the bulletin can render a stable
    /// "this goal is solved" terminal state without a late-arriving
    /// contribution silently re-opening it. Distinct from
    /// [`Self::GoalNotFound`] so the agency UI can render "this clue
    /// board is already solved — well done!" rather than the generic
    /// "not found" recovery flow.
    #[error("shared goal {id} is already completed; contributions are closed")]
    GoalAlreadyCompleted {
        /// Goal id the caller passed.
        id: i64,
    },
    /// The contributor callback inside [`WorldDb::contribute_to_goal`]
    /// returned `Err`. The wrapping transaction has already rolled
    /// back, so the goal's `current_amount` is unchanged and any side
    /// effects the callback attempted (currency debit, inventory
    /// burn, evidence row insert) are undone — that's the SPEC §4.4
    /// "Contributions MUST be transactional" contract. Distinct from
    /// [`Self::Sqlite`] so the agency UI can surface a contributor-
    /// side reason ("you don't have enough clue points") separately
    /// from a SQLite-layer failure. Same shape as
    /// [`crate::market::MarketError::BuyerCallback`].
    #[error(
        "contributor callback inside contribute_to_goal failed; goal current_amount rolled back: {source}"
    )]
    ContributorCallback {
        /// Underlying `rusqlite::Error` returned by the callback. The
        /// callback is expected to map any non-SQLite failure into a
        /// [`rusqlite::Error::SqliteFailure`] with an explanatory
        /// message before returning, mirroring the convention on
        /// [`crate::market::MarketError::BuyerCallback`].
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

    /// Add `player_id` to `faction_id` as an active member —
    /// SPEC_v3 §4.4 / §Task 6c.
    ///
    /// The contract is "after this call returns, there is exactly
    /// one row in `faction_memberships` with `(player_id,
    /// faction_id, left_at IS NULL)`, and the returned struct
    /// describes it". Calling again with the same arguments while
    /// the membership is still active is a documented no-op: the
    /// helper returns the *existing* row, not a duplicate. This
    /// matches the kit's broader async-multiplayer convention —
    /// idempotent helpers free the game UI from defensive "do they
    /// already belong?" queries before offering a join button.
    ///
    /// # Idempotency primitive
    ///
    /// `INSERT … SELECT … WHERE NOT EXISTS (… active row …)
    /// RETURNING …` is a single statement that either inserts a
    /// fresh active row (and `RETURNING` echoes it) or inserts
    /// nothing (and `RETURNING` produces zero rows). Folding the
    /// existence check into the same statement as the insert is
    /// what makes the path race-free under concurrent door opens
    /// for the same player+faction: a "SELECT then INSERT"
    /// handshake would let two near-simultaneous calls each see
    /// "no active row", each insert, and leave the agency roster
    /// rendering the player twice. The schema deliberately does
    /// not carry a `UNIQUE (player_id, faction_id)` constraint
    /// (Task 6d soft-deletes leave the historical row in place,
    /// which would clash with such a unique key) — that's why the
    /// idempotency MUST live in the helper SQL, not the schema.
    ///
    /// On `QueryReturnedNoRows` (an existing active row blocked
    /// the insert) the helper performs one follow-up `SELECT` for
    /// the same active membership and returns it. The SELECT
    /// shares the column order of `RETURNING` so both arms decode
    /// through the same crate-private `row_to_membership` helper.
    ///
    /// # `role`
    ///
    /// `role: Option<&str>` — `None` resolves to the schema-side
    /// `DEFAULT 'member'` via `COALESCE(?3, 'member')` in the
    /// VALUES clause. SPEC §4.4 leaves the role lexicon to the
    /// game (rookies/sergeants vs. drivers/fences); games that
    /// don't model roles pass `None` and never see the column.
    ///
    /// When the player is already an active member, the supplied
    /// `role` is **ignored** — the existing row's role wins. Re-
    /// joining an already-joined faction does not promote or demote
    /// the player; that's a separate concern that would need a
    /// dedicated helper, and conflating "join" with "set role"
    /// would surprise game code that calls `join_faction` defensively
    /// from a join-screen submit handler.
    ///
    /// # Multi-faction membership
    ///
    /// SPEC §4.4 states "One player MAY belong to multiple
    /// factions unless the game config restricts it" — the schema
    /// and this helper enforce no per-player limit. Cross-faction
    /// exclusivity (the noir convention "you're either Blue Desk
    /// or Red Room, not both") lives in the game's join-screen
    /// logic, not the storage layer.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `INSERT … RETURNING` plus an
    /// optional follow-up `SELECT` under the configured busy
    /// timeout. Same borrow shape as
    /// [`WorldDb::create_challenge`] and [`WorldDb::accept_challenge`];
    /// no explicit `BEGIN`/`COMMIT` is needed because the
    /// idempotency check folds into the single INSERT statement.
    pub fn join_faction(
        &self,
        player_id: i64,
        faction_id: i64,
        role: Option<&str>,
    ) -> Result<FactionMembership, FactionError> {
        // The INSERT only fires when no active membership row
        // exists for this (player, faction) pair — `WHERE NOT
        // EXISTS` runs in the same statement as the insert, so the
        // existence check and the write happen under one lock and
        // cannot race with a concurrent door open. `COALESCE` lets
        // a `None` role bind as NULL and resolve to the schema-
        // side default `'member'` without branching the SQL.
        // `RETURNING` echoes the canonical row (autoincrement id +
        // SQL-side `joined_at`) for the freshly-inserted case.
        const INSERT_SQL: &str = "\
INSERT INTO faction_memberships (player_id, faction_id, role) \
SELECT ?1, ?2, COALESCE(?3, 'member') \
WHERE NOT EXISTS (\
    SELECT 1 FROM faction_memberships \
    WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL\
) \
RETURNING id, player_id, faction_id, role, joined_at, left_at";

        // When the INSERT inserts nothing (the player is already
        // an active member), look up the existing active row and
        // return it. Backed by `idx_faction_memberships_active`
        // — a partial index on `(player_id, faction_id) WHERE
        // left_at IS NULL` from FACTIONS_MIGRATION — so the lookup
        // is seek-bound regardless of how many historical "left"
        // rows the audit trail accumulates.
        const SELECT_EXISTING_SQL: &str = "\
SELECT id, player_id, faction_id, role, joined_at, left_at \
FROM faction_memberships \
WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL";

        match self.connection().query_row(
            INSERT_SQL,
            rusqlite::params![player_id, faction_id, role],
            row_to_membership,
        ) {
            Ok(membership) => Ok(membership),
            Err(rusqlite::Error::QueryReturnedNoRows) => self
                .connection()
                .query_row(
                    SELECT_EXISTING_SQL,
                    rusqlite::params![player_id, faction_id],
                    row_to_membership,
                )
                .map_err(|source| FactionError::Sqlite { source }),
            Err(source) => Err(FactionError::Sqlite { source }),
        }
    }

    /// Soft-delete the player's active membership in `faction_id` —
    /// SPEC_v3 §4.4 / §Task 6d.
    ///
    /// The contract is "after this call returns successfully, the
    /// `(player_id, faction_id)` pair has no active membership row
    /// (`left_at IS NULL`), and the returned `FactionMembership`
    /// describes the row whose departure was recorded". Calling
    /// again on an already-left player returns the same row
    /// unchanged — `left_at` is preserved across repeat calls so
    /// downstream UI surfaces (audit trail, "left at 02:14") render
    /// a stable timestamp rather than moving forward on every
    /// defensive UI submit.
    ///
    /// # Soft-delete, not row removal
    ///
    /// Per [`FACTIONS_MIGRATION`]'s doc on `faction_memberships`,
    /// the v3 schema reserves `left_at` for the leave timestamp and
    /// retains the historical row. This mirrors `notices.archived_at`
    /// — the row stays for audit and replay (Task 6c notes that
    /// "a player who leaves and rejoins" creates a new active row
    /// while the old one is preserved). A `DELETE`-based variant
    /// would also clash with bounty/notice references that captured
    /// the historical `faction_memberships.id` for attribution.
    ///
    /// # Two-arm idempotency
    ///
    /// 1. `UPDATE … SET left_at = CURRENT_TIMESTAMP WHERE … AND
    ///    left_at IS NULL RETURNING …` — fires only when there is
    ///    an active row. Backed by `idx_faction_memberships_active`
    ///    so the targeting filter is seek-bound regardless of how
    ///    many historical rows have accumulated for this player /
    ///    faction.
    /// 2. On `QueryReturnedNoRows` (no active row): one follow-up
    ///    `SELECT … ORDER BY id DESC LIMIT 1` returns the most
    ///    recent historical row. If even *that* is empty the player
    ///    has never joined the faction, and the helper raises
    ///    [`FactionError::NotMember`].
    ///
    /// Selecting the *most recent* historical row (highest `id`)
    /// rather than any-historical row matters for join → leave →
    /// rejoin → leave cycles: a stale UI that calls
    /// `leave_faction` after the second leave should observe the
    /// timestamp of the *second* leave, not the first. `id DESC`
    /// is the deterministic tiebreaker that pins this — `joined_at`
    /// alone could collide on the same second.
    ///
    /// # Why `WHERE left_at IS NULL` on the UPDATE
    ///
    /// Without the `IS NULL` guard the UPDATE would also fire on
    /// historical rows, refreshing their `left_at` to the current
    /// time and violating the "stable timestamp" invariant the
    /// idempotency test pins. The guard is structurally necessary,
    /// not stylistic.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an
    /// optional follow-up `SELECT` under the configured busy
    /// timeout. Same borrow shape as
    /// [`WorldDb::join_faction`] — no explicit `BEGIN`/`COMMIT` is
    /// needed because the targeted UPDATE is a single statement and
    /// the fallback SELECT is read-only.
    pub fn leave_faction(
        &self,
        player_id: i64,
        faction_id: i64,
    ) -> Result<FactionMembership, FactionError> {
        // Targets only the active row. The `left_at IS NULL`
        // predicate is what makes the helper idempotent: a second
        // call finds nothing to UPDATE, falls through to the
        // SELECT arm, and returns the historical row with its
        // original `left_at` preserved.
        const UPDATE_SQL: &str = "\
UPDATE faction_memberships \
SET left_at = CURRENT_TIMESTAMP \
WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL \
RETURNING id, player_id, faction_id, role, joined_at, left_at";

        // Fallback for the idempotent / never-joined branch. The
        // `ORDER BY id DESC LIMIT 1` returns the most recent row,
        // which under join/leave/rejoin/leave cycles is the row the
        // caller most likely intends to observe. `id` (autoincrement)
        // monotonically increases, so it doubles as a tiebreaker
        // when multiple rows share a `left_at` second.
        const SELECT_LATEST_SQL: &str = "\
SELECT id, player_id, faction_id, role, joined_at, left_at \
FROM faction_memberships \
WHERE player_id = ?1 AND faction_id = ?2 \
ORDER BY id DESC LIMIT 1";

        match self.connection().query_row(
            UPDATE_SQL,
            rusqlite::params![player_id, faction_id],
            row_to_membership,
        ) {
            Ok(membership) => Ok(membership),
            Err(rusqlite::Error::QueryReturnedNoRows) => match self.connection().query_row(
                SELECT_LATEST_SQL,
                rusqlite::params![player_id, faction_id],
                row_to_membership,
            ) {
                Ok(membership) => Ok(membership),
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(FactionError::NotMember {
                    player_id,
                    faction_id,
                }),
                Err(source) => Err(FactionError::Sqlite { source }),
            },
            Err(source) => Err(FactionError::Sqlite { source }),
        }
    }

    /// Idempotently create a shared goal for `(faction_id, key)` —
    /// SPEC_v3 §4.4 / §Task 6e.
    ///
    /// The contract is "after this call returns successfully there
    /// is exactly one `shared_goals` row matching the supplied
    /// `(faction_id, key)`, and the returned struct describes it".
    /// Calling again with the same `(faction_id, key)` is a no-op:
    /// the helper returns the *existing* row — same id, same
    /// `target_amount`, same `current_amount`, same `created_at` —
    /// rather than creating a duplicate or overwriting progress.
    /// This matches the v3 helper convention of leaning on the
    /// underlying schema's uniqueness so a defensive double-call
    /// from the agency-bootstrap path never accidentally resets a
    /// goal that already has contributions on it.
    ///
    /// # Idempotency primitive
    ///
    /// `INSERT … SELECT … WHERE NOT EXISTS (…) RETURNING …` —
    /// single statement that either inserts a fresh `active` row
    /// (and `RETURNING` echoes it) or inserts nothing (and
    /// `RETURNING` produces zero rows). The same shape used by
    /// [`WorldDb::join_faction`]; chosen here because the schema's
    /// `UNIQUE (faction_id, key)` index treats NULL as distinct
    /// (SQLite default semantics). A bare
    /// `INSERT … ON CONFLICT(faction_id, key) DO NOTHING` would
    /// silently allow duplicate world-wide goals (`faction_id IS
    /// NULL`) on every call, which is exactly the regression the
    /// idempotency contract above forbids. The helper-level
    /// `WHERE NOT EXISTS` clause uses SQLite's `IS` operator
    /// (`faction_id IS ?1`) so NULL matches NULL — making
    /// idempotency hold uniformly for faction-scoped *and* world-
    /// wide goals.
    ///
    /// On `QueryReturnedNoRows` (an existing row blocked the
    /// insert) the helper performs one follow-up `SELECT` for the
    /// same `(faction_id, key)` and returns it. The SELECT shares
    /// the column order of `RETURNING` so both arms decode through
    /// the same crate-private `row_to_shared_goal` helper.
    ///
    /// # `target_amount` is validated up front
    ///
    /// The schema enforces `CHECK (target_amount > 0)` (a goal with
    /// target 0 would be instantly complete on creation, surfacing
    /// "solved!" to the UI before any work is done). Pre-checking
    /// in Rust lets the helper return [`FactionError::InvalidTargetAmount`]
    /// without paying a SQLite round-trip and without forcing the
    /// caller to parse a generic CHECK-violation `SqliteFailure` to
    /// distinguish it from "FK missing" or "DB locked". On the
    /// idempotent re-create path the supplied `target_amount` is
    /// **ignored** — the existing row's target wins, mirroring
    /// `join_faction`'s "supplied role is ignored on re-join"
    /// stance. A game author who needs to change a goal's target
    /// mid-run picks a new `key` (or, in a future helper, calls a
    /// dedicated "retarget" path that lands its own world event);
    /// silently overwriting target_amount on every defensive UI
    /// submit would let a stray 1-character config edit erase the
    /// progress proportions every contributor has been seeing.
    ///
    /// # `faction_id` is `Option<i64>`
    ///
    /// SPEC §4.4 explicitly lists "faction id optional" — a goal
    /// MAY be world-wide (e.g. "the city solves 100 cases") rather
    /// than scoped to one faction. `None` lands as SQL `NULL` and
    /// is treated as a distinct goal-scope from any specific
    /// faction.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `INSERT … RETURNING` plus an
    /// optional follow-up `SELECT` under the configured busy
    /// timeout. Same borrow shape as [`WorldDb::join_faction`] —
    /// no explicit `BEGIN`/`COMMIT` is needed because the
    /// idempotency check folds into the single INSERT statement.
    pub fn create_shared_goal(
        &self,
        faction_id: Option<i64>,
        key: &str,
        target_amount: i64,
    ) -> Result<SharedGoal, FactionError> {
        // Pre-validate `target_amount` so the schema CHECK is the
        // safety net for raw-SQL bypass paths, not the primary
        // failure surface for game code. Surfacing a typed variant
        // here lets the agency-config UI render "target must be
        // positive" without `match`-ing on a `SqliteFailure`'s
        // string body.
        if target_amount <= 0 {
            return Err(FactionError::InvalidTargetAmount {
                actual: target_amount,
            });
        }

        // Fold the existence check into the INSERT itself so a
        // concurrent door open hitting the same (faction_id, key)
        // pair cannot race past a "SELECT then INSERT" handshake
        // and land a duplicate row. `IS` (rather than `=`) is
        // critical for the `faction_id IS NULL` case — `=` returns
        // NULL when either operand is NULL, which would let two
        // world-wide goals with the same key both pass the
        // existence check and both land in the table.
        const INSERT_SQL: &str = "\
INSERT INTO shared_goals (faction_id, key, target_amount) \
SELECT ?1, ?2, ?3 \
WHERE NOT EXISTS (\
    SELECT 1 FROM shared_goals \
    WHERE faction_id IS ?1 AND key = ?2\
) \
RETURNING id, faction_id, key, target_amount, current_amount, state, created_at, completed_at";

        // Fallback for the idempotent re-create branch. Same
        // `IS`-rather-than-`=` semantics so NULL faction_id matches
        // its own row. Backed by `idx_shared_goals_faction_key`
        // (UNIQUE on `(faction_id, key)`).
        const SELECT_EXISTING_SQL: &str = "\
SELECT id, faction_id, key, target_amount, current_amount, state, created_at, completed_at \
FROM shared_goals \
WHERE faction_id IS ?1 AND key = ?2";

        match self.connection().query_row(
            INSERT_SQL,
            rusqlite::params![faction_id, key, target_amount],
            row_to_shared_goal,
        ) {
            Ok(goal) => Ok(goal),
            Err(rusqlite::Error::QueryReturnedNoRows) => self
                .connection()
                .query_row(
                    SELECT_EXISTING_SQL,
                    rusqlite::params![faction_id, key],
                    row_to_shared_goal,
                )
                .map_err(|source| FactionError::Sqlite { source }),
            Err(source) => Err(FactionError::Sqlite { source }),
        }
    }

    /// Atomically increment a shared goal's `current_amount` and run
    /// a contributor-side callback inside the same transaction —
    /// SPEC_v3 §4.4 / §Task 6f.
    ///
    /// The contract is "either everything happens (the goal's
    /// running total moves up by `amount` *and* every write the
    /// callback performed lands) or nothing does". This is how the
    /// kit honours SPEC §4.4's "Contributions MUST be transactional"
    /// rule: a contributor who can't afford to spend the resource,
    /// or a callback that fails partway through inventory mutation,
    /// leaves the goal's `current_amount` exactly where it was.
    /// Without that guarantee a flaky network drop mid-contribution
    /// could double-debit a player or, worse, leave the agency goal
    /// believing it received clue points the player never paid for.
    ///
    /// # Why a callback (not a separate write path)
    ///
    /// The kit owns the `shared_goals` row but knows nothing about
    /// the resource the contribution costs the player — clue points,
    /// inventory items, currency, time tokens, all game-defined.
    /// SPEC §1 / §17 keep the kit out of the game's resource model;
    /// the contributor closure is the seam where game code spends
    /// whatever it needs to spend, inside the same `rusqlite::
    /// Transaction` the kit opened, so a failure rolls *both* the
    /// debit and the increment back together. Mirrors the buyer-
    /// callback shape on [`WorldDb::buy_listing`].
    ///
    /// # State-machine guarantees
    ///
    /// - The increment only fires when `state = 'active'`. A goal
    ///   that has already been flipped to `'completed'` (Task 6g
    ///   handles that flip + the world event) rejects further
    ///   contributions with [`FactionError::GoalAlreadyCompleted`]
    ///   — the bulletin renders a stable terminal state, and a late
    ///   contribute call from a stale UI cannot silently re-open it.
    /// - This helper does *not* itself flip the state when the
    ///   target is reached; that is Task 6g's job (state flip +
    ///   `shared_goal.completed` world event). 6f's contract is
    ///   strictly "`current_amount` increments transactionally".
    ///   Splitting the responsibilities keeps each helper testable
    ///   in isolation.
    /// - There is no schema-side `CHECK` constraint capping
    ///   `current_amount` at `target_amount`, so a final contribute
    ///   that arrives "after" the target is met still succeeds and
    ///   pushes `current_amount` past `target_amount`. Task 6g will
    ///   read the post-update row and decide whether to flip — that
    ///   model is simpler and race-safer than trying to clamp inside
    ///   the UPDATE here.
    ///
    /// # Errors surfaced
    ///
    /// - [`FactionError::NonPositiveContribution`] — `amount <= 0`,
    ///   raised before opening the transaction so a broken UI keypad
    ///   doesn't pay for a `BEGIN` round-trip. A negative `amount`
    ///   would silently rewind progress; a zero is a no-op that
    ///   still appears to "succeed". Both worth refusing.
    /// - [`FactionError::GoalNotFound`] — `goal_id` is not in
    ///   `shared_goals` at all. Diagnosed by a follow-up SELECT
    ///   inside the same transaction so the read sees the same
    ///   snapshot the UPDATE saw.
    /// - [`FactionError::GoalAlreadyCompleted`] — the row exists but
    ///   its `state` is `'completed'` (the only other vocabulary
    ///   point per the schema CHECK).
    /// - [`FactionError::ContributorCallback`] — the closure
    ///   returned `Err`. The transaction is dropped without commit
    ///   so every write the callback attempted, plus the kit's own
    ///   increment, rolls back as a unit.
    /// - [`FactionError::Sqlite`] — any other `rusqlite` failure
    ///   (DB lock timeout, schema corruption, FK enforcement
    ///   surprise).
    ///
    /// # Concurrency
    ///
    /// Takes `&mut self` so we can open a `rusqlite::Transaction`
    /// via the crate-internal `connection_mut` accessor. Same shape
    /// as [`WorldDb::buy_listing`]; a `&self` variant would force
    /// the helper to leak SQL across multiple statements without
    /// transactional grouping, defeating the all-or-nothing
    /// guarantee under a concurrent door open hitting the same
    /// world DB file. The conditional UPDATE
    /// (`WHERE id = ?1 AND state = 'active'`) plus SQLite's per-
    /// statement atomicity guarantees no concurrent contribute can
    /// interleave between the predicate check and the increment.
    pub fn contribute_to_goal<F>(
        &mut self,
        goal_id: i64,
        amount: i64,
        contributor: F,
    ) -> Result<SharedGoal, FactionError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &SharedGoal) -> Result<(), rusqlite::Error>,
    {
        // Reject non-positive contributions before opening the
        // transaction — same defensive ordering as `buy_listing`.
        // A zero would be a no-op that still appears to "succeed";
        // a negative would silently rewind progress for every other
        // contributor.
        if amount <= 0 {
            return Err(FactionError::NonPositiveContribution { actual: amount });
        }

        // Conditional UPDATE: only increments when the row is still
        // active. `RETURNING` echoes the post-increment row so the
        // contributor callback can inspect the new running total
        // (handy for "you tipped the goal over the target" UX) and
        // the helper can return the canonical record to the caller.
        // Column order matches `row_to_shared_goal` so the decoder
        // and the SQL stay locked together.
        const UPDATE_SQL: &str = "\
UPDATE shared_goals \
SET current_amount = current_amount + ?2 \
WHERE id = ?1 AND state = 'active' \
RETURNING id, faction_id, key, target_amount, current_amount, state, created_at, completed_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| FactionError::Sqlite { source })?;

        // SQLite's per-statement atomicity plus the wrapping
        // transaction guarantees no concurrent contribute can
        // interleave between the predicate check and the write.
        let goal = match tx.query_row(
            UPDATE_SQL,
            rusqlite::params![goal_id, amount],
            row_to_shared_goal,
        ) {
            Ok(goal) => goal,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Diagnose why the conditional update affected zero
                // rows: either the goal doesn't exist (GoalNotFound)
                // or its state is already 'completed'
                // (GoalAlreadyCompleted). A second read inside the
                // same transaction sees a consistent snapshot (the
                // UPDATE didn't change anything) and lets us surface
                // a typed error rather than a generic "no rows".
                use rusqlite::OptionalExtension;
                let state: Option<String> = tx
                    .query_row(
                        "SELECT state FROM shared_goals WHERE id = ?1",
                        rusqlite::params![goal_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|source| FactionError::Sqlite { source })?;
                // Drop the transaction without commit — rolls back
                // automatically. The diagnostic SELECT didn't write
                // anything, so the rollback is observably a no-op,
                // but the explicit drop here documents the intent.
                drop(tx);
                return Err(match state {
                    None => FactionError::GoalNotFound { id: goal_id },
                    Some(_) => FactionError::GoalAlreadyCompleted { id: goal_id },
                });
            }
            Err(source) => {
                drop(tx);
                return Err(FactionError::Sqlite { source });
            }
        };

        // Run the contributor callback inside the same transaction.
        // A callback `Err` propagates as `ContributorCallback` and
        // the transaction drops without commit — every write the
        // callback attempted, plus the kit's own increment, rolls
        // back as a unit. SPEC §4.4 "Contributions MUST be
        // transactional".
        if let Err(source) = contributor(&tx, &goal) {
            return Err(FactionError::ContributorCallback { source });
        }

        tx.commit()
            .map_err(|source| FactionError::Sqlite { source })?;

        Ok(goal)
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

/// Read model for one row of the `faction_memberships` table —
/// SPEC_v3 §4.4.
///
/// Returned by [`WorldDb::join_faction`] (and the upcoming Task 6d
/// `leave_faction` helper) so callers receive the canonical row
/// SQLite produced — autoincrement `id`, SQL-side `joined_at`,
/// resolved `role` (the input default `'member'` when no override
/// was supplied) — rather than echoing back the input arguments.
///
/// `left_at` is `Option<String>`: while the membership is active
/// the column is `NULL` and decodes to `None`; Task 6d soft-deletes
/// by stamping it with `CURRENT_TIMESTAMP`. Game UI surfaces filter
/// to active membership by checking `left_at.is_none()` (or by
/// using one of the partial indexes on the schema, which already
/// scope to `WHERE left_at IS NULL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactionMembership {
    /// Autoincrement primary key. Stable handle the kit references
    /// memberships by; lets a player who leaves and rejoins the
    /// same faction be addressed unambiguously by their *current*
    /// active membership row even though the historical row is
    /// retained as audit trail.
    pub id: i64,
    /// Foreign key into `players(id)`. The player who belongs to
    /// the faction.
    pub player_id: i64,
    /// Foreign key into `factions(id)`. The faction the player is
    /// currently (`left_at IS NULL`) a member of.
    pub faction_id: i64,
    /// Game-defined role label. Defaults to `'member'` when the
    /// caller passes `None` to [`WorldDb::join_faction`]. The
    /// schema does not constrain the vocabulary (no `CHECK`) — see
    /// [`FACTIONS_MIGRATION`]'s doc on `faction_memberships.role`.
    pub role: String,
    /// SQLite-assigned UTC ISO timestamp (`CURRENT_TIMESTAMP`) of
    /// the join — the moment this membership row was first
    /// inserted. Stable for the life of the row.
    pub joined_at: String,
    /// `None` while the membership is active. Stamped with the
    /// `CURRENT_TIMESTAMP` of the leave event by Task 6d's
    /// `leave_faction` helper. Always `None` immediately after a
    /// successful [`WorldDb::join_faction`] return.
    pub left_at: Option<String>,
}

/// Decode one `faction_memberships` row in the column order shared
/// by [`WorldDb::join_faction`] and (forthcoming) `leave_faction`.
/// Centralised so a column rename touches one place.
fn row_to_membership(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactionMembership> {
    Ok(FactionMembership {
        id: row.get(0)?,
        player_id: row.get(1)?,
        faction_id: row.get(2)?,
        role: row.get(3)?,
        joined_at: row.get(4)?,
        left_at: row.get(5)?,
    })
}

/// Read model for one row of the `shared_goals` table —
/// SPEC_v3 §4.4.
///
/// Returned by [`WorldDb::create_shared_goal`] (and the upcoming
/// Task 6f `contribute_to_goal` / Task 6g completion helpers) so
/// callers receive the canonical row SQLite produced —
/// autoincrement `id`, SQL-side `created_at`, schema-default
/// `current_amount = 0` and `state = 'active'` — rather than
/// echoing back the input arguments.
///
/// `faction_id` is `Option<i64>` because SPEC §4.4 explicitly
/// allows world-wide goals (`NULL` faction). `completed_at` is
/// `Option<String>` and stays `None` until Task 6g flips the
/// state to `'completed'` and stamps `CURRENT_TIMESTAMP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedGoal {
    /// Autoincrement primary key. Stable handle the kit and game
    /// code reference goals by; survives idempotent re-create
    /// calls (same id every time for a given `(faction_id, key)`).
    pub id: i64,
    /// Foreign key into `factions(id)`, or `None` for a world-wide
    /// goal (SPEC §4.4 "faction id optional").
    pub faction_id: Option<i64>,
    /// Game-authored stable identifier (e.g. `"blue-desk.clue-board"`).
    /// Combined with `faction_id` it forms the logical lookup key.
    pub key: String,
    /// The amount contributions need to accumulate before the goal
    /// flips to `completed`. Pinned at first creation; the
    /// idempotent re-create path returns the *existing* target,
    /// not the freshly-supplied one.
    pub target_amount: i64,
    /// Running total of contributions. Starts at `0`; Task 6f's
    /// `contribute_to_goal` increments it; Task 6g flips state to
    /// `completed` once the target is reached.
    pub current_amount: i64,
    /// Either `"active"` or `"completed"` — the schema CHECK on
    /// `shared_goals.state` pins the vocabulary. Newly-created
    /// goals start `"active"`.
    pub state: String,
    /// SQLite-assigned UTC ISO timestamp (`CURRENT_TIMESTAMP`) of
    /// the create call that materialised this row. Stable across
    /// idempotent re-create calls.
    pub created_at: String,
    /// `None` while the goal is active. Stamped with the
    /// `CURRENT_TIMESTAMP` of the completion event by Task 6g.
    pub completed_at: Option<String>,
}

/// Decode one `shared_goals` row in the column order shared by
/// [`WorldDb::create_shared_goal`]'s `RETURNING` clause and its
/// fallback SELECT. Centralised so a column rename touches one
/// place — same convention as `row_to_faction` and
/// `row_to_membership`.
fn row_to_shared_goal(row: &rusqlite::Row<'_>) -> rusqlite::Result<SharedGoal> {
    Ok(SharedGoal {
        id: row.get(0)?,
        faction_id: row.get(1)?,
        key: row.get(2)?,
        target_amount: row.get(3)?,
        current_amount: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
        completed_at: row.get(7)?,
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

    /// SPEC_v3 §Task 6c acceptance: `join_faction` inserts a fresh
    /// `faction_memberships` row for an unaffiliated player and
    /// returns the canonical record. Pins the happy path so a
    /// regression that swapped the `RETURNING` columns or dropped
    /// the autoincrement-id assignment flunks here rather than in
    /// a downstream agency-roster screen test.
    #[test]
    fn join_faction_creates_active_membership_row() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let seeds = vec![FactionSeed {
            slug: "blue-desk".to_string(),
            display_name: "Blue Desk Agency".to_string(),
            description: "by-the-book".to_string(),
        }];
        let factions = world.seed_factions(&seeds).expect("seed succeeds");
        let blue_desk = &factions[0];

        let membership = world
            .join_faction(alice.id, blue_desk.id, None)
            .expect("join succeeds");

        assert!(
            membership.id > 0,
            "id assigned by SQLite autoincrement, got {}",
            membership.id
        );
        assert_eq!(membership.player_id, alice.id);
        assert_eq!(membership.faction_id, blue_desk.id);
        assert_eq!(
            membership.role, "member",
            "None role resolves to the schema default 'member'"
        );
        assert!(
            !membership.joined_at.is_empty(),
            "joined_at populated by CURRENT_TIMESTAMP"
        );
        assert!(
            membership.left_at.is_none(),
            "freshly joined membership is active (left_at IS NULL)"
        );

        // Belt-and-braces: the row is durably visible to a fresh
        // SELECT — catches a regression that returned the
        // RETURNING row but failed to actually insert (e.g. a
        // typo'd WHERE clause in the WHERE NOT EXISTS guard that
        // always evaluated true).
        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM faction_memberships \
                 WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL",
                rusqlite::params![alice.id, blue_desk.id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(count, 1, "exactly one active membership row exists");
    }

    /// SPEC_v3 §Task 6c acceptance criterion: "membership row is
    /// created" — and, by the kit's broader idempotency convention,
    /// not duplicated on a second join. Doors restart on every
    /// player session and game UIs may call `join_faction`
    /// defensively from a "join" button without first checking
    /// membership; without idempotency the agency roster would
    /// render the same player N times.
    ///
    /// Verifies three independent regression vectors with one test:
    ///
    /// 1. The second call returns the *same* id as the first
    ///    (catches a regression that re-keyed memberships per
    ///    call).
    /// 2. The total active-membership row count stays at 1
    ///    (catches the most direct regression — the `WHERE NOT
    ///    EXISTS` guard removed or inverted).
    /// 3. The second call returns the *same* `joined_at` (catches
    ///    a regression that wrote a fresh timestamp on each call,
    ///    which would corrupt tenure-based UI surfaces).
    #[test]
    fn join_faction_is_idempotent_for_active_member() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();
        let blue_desk = &factions[0];

        let first = world.join_faction(alice.id, blue_desk.id, None).unwrap();
        let second = world.join_faction(alice.id, blue_desk.id, None).unwrap();

        assert_eq!(first.id, second.id, "id stable across join calls");
        assert_eq!(
            first.joined_at, second.joined_at,
            "joined_at stable across join calls"
        );

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM faction_memberships", [], |row| {
                row.get(0)
            })
            .expect("count query runs");
        assert_eq!(
            count, 1,
            "second join MUST NOT duplicate rows (got {count})"
        );
    }

    /// `role: Some("rookie")` MUST land on a fresh membership row.
    /// Pins that the `COALESCE(?3, 'member')` actually consults the
    /// bound parameter — a regression that always returned
    /// `'member'` would flunk here. Game-defined role vocabularies
    /// (rookies/sergeants, drivers/fences) are core to the
    /// async-multiplayer agency surface; without role plumbing
    /// every roster screen would read flat.
    #[test]
    fn join_faction_with_explicit_role_records_it() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let membership = world
            .join_faction(alice.id, factions[0].id, Some("sergeant"))
            .expect("join with role succeeds");

        assert_eq!(membership.role, "sergeant");
    }

    /// When a player is already an active member, a subsequent
    /// `join_faction` call with a different `role` MUST return the
    /// *existing* row unchanged — re-joining is not a promotion
    /// path. Pins that the helper ignores the supplied role on the
    /// idempotent path, which guards game-UI code that calls
    /// `join_faction` defensively (e.g. from a join-screen submit
    /// handler) against silently overwriting the player's
    /// established role.
    #[test]
    fn join_faction_does_not_change_role_for_existing_member() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let first = world
            .join_faction(alice.id, factions[0].id, Some("rookie"))
            .unwrap();
        let second = world
            .join_faction(alice.id, factions[0].id, Some("sergeant"))
            .unwrap();

        assert_eq!(first.id, second.id);
        assert_eq!(
            second.role, "rookie",
            "existing role MUST NOT be overwritten by a re-join"
        );
    }

    /// A player MAY belong to multiple factions (SPEC §4.4 — the
    /// schema enforces no per-player cap; cross-faction exclusivity
    /// is a game-config concern, not a storage one). Two separate
    /// joins for the same player into different factions MUST each
    /// succeed and produce a distinct active membership row.
    #[test]
    fn join_faction_supports_multiple_faction_membership() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[
                FactionSeed {
                    slug: "blue-desk".to_string(),
                    display_name: "Blue Desk".to_string(),
                    description: "x".to_string(),
                },
                FactionSeed {
                    slug: "red-room".to_string(),
                    display_name: "Red Room".to_string(),
                    description: "y".to_string(),
                },
            ])
            .unwrap();

        let blue = world.join_faction(alice.id, factions[0].id, None).unwrap();
        let red = world.join_faction(alice.id, factions[1].id, None).unwrap();

        assert_ne!(blue.id, red.id);
        assert_eq!(blue.faction_id, factions[0].id);
        assert_eq!(red.faction_id, factions[1].id);

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM faction_memberships \
                 WHERE player_id = ?1 AND left_at IS NULL",
                rusqlite::params![alice.id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(
            count, 2,
            "alice has two active memberships (one per faction)"
        );
    }

    /// SPEC_v3 §Task 6d acceptance: `leave_faction` soft-deletes the
    /// active membership by stamping `left_at` (per the chosen
    /// schema — see [`FACTIONS_MIGRATION`]'s doc on
    /// `faction_memberships.left_at`). After the call:
    ///
    /// 1. The returned [`FactionMembership`] has `left_at = Some(_)`.
    /// 2. The `idx_faction_memberships_active`-shaped query (the
    ///    one every membership-aware UI relies on) sees zero rows.
    /// 3. The historical row is still present in the table — pinned
    ///    here so a regression that switched from soft-delete to
    ///    `DELETE` flunks (audit trail and bounty/notice references
    ///    that captured the membership id depend on retention).
    #[test]
    fn leave_faction_marks_active_membership_inactive() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();
        let blue_desk = &factions[0];

        let joined = world.join_faction(alice.id, blue_desk.id, None).unwrap();
        let left = world
            .leave_faction(alice.id, blue_desk.id)
            .expect("leave succeeds");

        assert_eq!(left.id, joined.id, "same membership row, soft-deleted");
        assert!(
            left.left_at.is_some(),
            "left_at stamped by CURRENT_TIMESTAMP, got {:?}",
            left.left_at
        );

        let active: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM faction_memberships \
                 WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL",
                rusqlite::params![alice.id, blue_desk.id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(active, 0, "active-membership view sees zero rows");

        // Audit-trail invariant: the row itself is retained.
        let total: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM faction_memberships \
                 WHERE player_id = ?1 AND faction_id = ?2",
                rusqlite::params![alice.id, blue_desk.id],
                |row| row.get(0),
            )
            .expect("total count query runs");
        assert_eq!(
            total, 1,
            "soft-delete retains the historical row (got {total})"
        );
    }

    /// Idempotency: a second `leave_faction` call on an already-left
    /// player returns the same row with the same `left_at`. Pins
    /// three independent regression vectors:
    ///
    /// 1. The second call returns the same membership id (catches a
    ///    regression that returned a different historical row).
    /// 2. The second call returns the same `left_at` (catches a
    ///    regression that dropped the `WHERE left_at IS NULL` guard
    ///    on the UPDATE — that would refresh the timestamp on every
    ///    repeat call).
    /// 3. The second call still succeeds with `Ok` (catches a
    ///    regression that raised `NotMember` on the idempotent path,
    ///    which would force every game-UI leave button into a
    ///    defensive "are you still a member?" pre-check).
    #[test]
    fn leave_faction_is_idempotent_for_already_left_member() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        world.join_faction(alice.id, factions[0].id, None).unwrap();
        let first = world.leave_faction(alice.id, factions[0].id).unwrap();
        let second = world.leave_faction(alice.id, factions[0].id).unwrap();

        assert_eq!(first.id, second.id, "same membership id across calls");
        assert_eq!(
            first.left_at, second.left_at,
            "left_at stable across leave calls (UPDATE must not fire on already-left rows)"
        );
        assert!(second.left_at.is_some());
    }

    /// `leave_faction` for a player who has never joined the faction
    /// raises `NotMember`. Distinct from the already-left idempotent
    /// path so the agency-roster UI can render a meaningful "you
    /// aren't a member" message rather than swallowing the error or
    /// pretending success.
    #[test]
    fn leave_faction_returns_not_member_for_unaffiliated_player() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let err = world
            .leave_faction(alice.id, factions[0].id)
            .expect_err("leave on never-joined returns NotMember");

        match err {
            FactionError::NotMember {
                player_id,
                faction_id,
            } => {
                assert_eq!(player_id, alice.id);
                assert_eq!(faction_id, factions[0].id);
            }
            other => panic!("expected NotMember, got {other:?}"),
        }
    }

    /// Leaving one faction MUST NOT touch active memberships in
    /// other factions for the same player. SPEC §4.4 multi-faction
    /// guarantee: a player's two memberships are independent.
    #[test]
    fn leave_faction_does_not_affect_other_faction_memberships() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[
                FactionSeed {
                    slug: "blue-desk".to_string(),
                    display_name: "Blue Desk".to_string(),
                    description: "x".to_string(),
                },
                FactionSeed {
                    slug: "red-room".to_string(),
                    display_name: "Red Room".to_string(),
                    description: "y".to_string(),
                },
            ])
            .unwrap();

        world.join_faction(alice.id, factions[0].id, None).unwrap();
        world.join_faction(alice.id, factions[1].id, None).unwrap();

        let left = world.leave_faction(alice.id, factions[0].id).unwrap();
        assert_eq!(left.faction_id, factions[0].id);
        assert!(left.left_at.is_some());

        // Red Room membership must remain active.
        let active: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM faction_memberships \
                 WHERE player_id = ?1 AND faction_id = ?2 AND left_at IS NULL",
                rusqlite::params![alice.id, factions[1].id],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(active, 1, "leaving Blue Desk MUST NOT affect Red Room");
    }

    /// Join → leave → rejoin → leave: the latest leave call
    /// targets the *new* active row, and a subsequent idempotent
    /// leave returns *that* row (highest id), not the original
    /// historical one. Pins the `ORDER BY id DESC LIMIT 1` choice
    /// in the fallback SELECT against a regression that returned
    /// the first historical row instead of the most recent.
    #[test]
    fn leave_faction_targets_latest_membership_on_rejoin() {
        let (_dir, mut world) = world_with_factions();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let first_join = world.join_faction(alice.id, factions[0].id, None).unwrap();
        let first_leave = world.leave_faction(alice.id, factions[0].id).unwrap();
        assert_eq!(first_leave.id, first_join.id);

        // Rejoin lands a brand-new row (the historical one stays
        // soft-deleted; `WHERE NOT EXISTS (… active …)` on
        // join_faction sees no active row and inserts).
        let second_join = world.join_faction(alice.id, factions[0].id, None).unwrap();
        assert_ne!(second_join.id, first_join.id);

        let second_leave = world.leave_faction(alice.id, factions[0].id).unwrap();
        assert_eq!(second_leave.id, second_join.id);

        // Idempotent re-call surfaces the *latest* historical row.
        let idempotent = world.leave_faction(alice.id, factions[0].id).unwrap();
        assert_eq!(
            idempotent.id, second_join.id,
            "fallback SELECT must return the most recent historical row"
        );
        assert_eq!(idempotent.left_at, second_leave.left_at);
    }

    /// SPEC_v3 §Task 6e acceptance happy path: a fresh
    /// `create_shared_goal` call lands a row with the supplied
    /// target, schema defaults for `current_amount = 0` and
    /// `state = 'active'`, the SQL-side `created_at` populated, and
    /// `completed_at` still `None`. Doubles as proof that the
    /// `RETURNING` column order matches `row_to_shared_goal`'s
    /// decoder — a column-order swap would surface as a wrong
    /// `target_amount`/`current_amount` pair here, not in a 6f/6g
    /// behavioural test.
    #[test]
    fn create_shared_goal_inserts_active_row_with_schema_defaults() {
        let (_dir, mut world) = world_with_factions();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let goal = world
            .create_shared_goal(Some(factions[0].id), "clue-board", 100)
            .expect("create_shared_goal succeeds");

        assert!(goal.id > 0, "id is autoincrement-assigned");
        assert_eq!(goal.faction_id, Some(factions[0].id));
        assert_eq!(goal.key, "clue-board");
        assert_eq!(goal.target_amount, 100);
        assert_eq!(goal.current_amount, 0);
        assert_eq!(goal.state, "active");
        assert!(
            !goal.created_at.is_empty(),
            "SQL-side CURRENT_TIMESTAMP populated"
        );
        assert!(goal.completed_at.is_none());

        // Row was actually committed (a regression that opened a
        // transaction without committing would still succeed in the
        // RETURNING arm but fail this fresh-connection-style read).
        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM shared_goals \
                 WHERE faction_id = ?1 AND key = ?2",
                rusqlite::params![factions[0].id, "clue-board"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// SPEC_v3 §Task 6e acceptance — idempotency: a second call for
    /// the same `(faction_id, key)` returns the **existing** row
    /// (same id, same `target_amount`, same `current_amount`, same
    /// `created_at`) and does not insert a duplicate. Three
    /// regression vectors layered into one test:
    ///
    /// - id stability — pins the SELECT-existing fallback branch.
    /// - target/current/created_at stability — proves the supplied
    ///   `target_amount` is *ignored* on re-create (write-once
    ///   contract); proves a (theoretical) regression that overwrote
    ///   `current_amount` would flunk; proves `created_at` does not
    ///   refresh.
    /// - row count stays at 1 — proves no duplicate was inserted.
    #[test]
    fn create_shared_goal_is_idempotent_for_same_faction_key() {
        let (_dir, mut world) = world_with_factions();
        let factions = world
            .seed_factions(&[FactionSeed {
                slug: "blue-desk".to_string(),
                display_name: "Blue Desk".to_string(),
                description: "x".to_string(),
            }])
            .unwrap();

        let first = world
            .create_shared_goal(Some(factions[0].id), "clue-board", 100)
            .unwrap();

        // Simulate prior progress so a regression that reset
        // `current_amount` on the re-create path would surface here.
        world
            .connection()
            .execute(
                "UPDATE shared_goals SET current_amount = 25 WHERE id = ?1",
                rusqlite::params![first.id],
            )
            .unwrap();

        // Re-create with a *different* target_amount — the existing
        // row's target must win (write-once).
        let second = world
            .create_shared_goal(Some(factions[0].id), "clue-board", 9999)
            .unwrap();

        assert_eq!(second.id, first.id, "same id on re-create");
        assert_eq!(
            second.target_amount, 100,
            "supplied target_amount must be ignored on re-create"
        );
        assert_eq!(
            second.current_amount, 25,
            "current_amount progress must not be reset"
        );
        assert_eq!(
            second.created_at, first.created_at,
            "created_at must be stable across idempotent calls"
        );

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM shared_goals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "idempotent call must not insert a duplicate");
    }

    /// World-wide goals (`faction_id IS NULL`) are also idempotent
    /// per `(faction_id, key)`. SQLite treats NULLs as distinct in
    /// `=` comparisons, so a regression that used `=` instead of
    /// `IS` in the helper's `WHERE NOT EXISTS` clause would let the
    /// second call insert a duplicate world-wide row. This test
    /// pins the `IS` choice.
    #[test]
    fn create_shared_goal_is_idempotent_for_world_wide_goal() {
        let (_dir, world) = world_with_factions();

        let first = world
            .create_shared_goal(None, "city-cases-solved", 100)
            .unwrap();
        let second = world
            .create_shared_goal(None, "city-cases-solved", 100)
            .unwrap();

        assert_eq!(first.id, second.id);
        assert!(first.faction_id.is_none());

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM shared_goals WHERE faction_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// Two factions may each carry their own `"clue-board"` —
    /// `(faction_id, key)` is the uniqueness scope, not `key`
    /// alone. Pins the schema-side `UNIQUE (faction_id, key)`
    /// against a regression that narrowed the index to `key` only.
    #[test]
    fn create_shared_goal_allows_same_key_under_different_factions() {
        let (_dir, mut world) = world_with_factions();
        let factions = world
            .seed_factions(&[
                FactionSeed {
                    slug: "blue-desk".to_string(),
                    display_name: "Blue Desk".to_string(),
                    description: "x".to_string(),
                },
                FactionSeed {
                    slug: "red-room".to_string(),
                    display_name: "Red Room".to_string(),
                    description: "y".to_string(),
                },
            ])
            .unwrap();

        let blue = world
            .create_shared_goal(Some(factions[0].id), "clue-board", 100)
            .unwrap();
        let red = world
            .create_shared_goal(Some(factions[1].id), "clue-board", 50)
            .unwrap();

        assert_ne!(blue.id, red.id);
        assert_eq!(blue.target_amount, 100);
        assert_eq!(red.target_amount, 50);
    }

    /// SPEC §4.4 + the schema CHECK forbid `target_amount <= 0`.
    /// The helper short-circuits before opening a transaction so
    /// the agency-config UI sees a typed
    /// [`FactionError::InvalidTargetAmount`] with the offending
    /// value, not a generic `SqliteFailure` it has to string-parse.
    /// Covers zero and negative; also verifies the row-unchanged
    /// invariant after a rejected call.
    #[test]
    fn create_shared_goal_rejects_non_positive_target_amount() {
        let (_dir, world) = world_with_factions();

        for actual in [0_i64, -1, -100] {
            let err = world
                .create_shared_goal(None, "k", actual)
                .expect_err("non-positive target must be rejected");
            match err {
                FactionError::InvalidTargetAmount { actual: got } => assert_eq!(got, actual),
                other => panic!("expected InvalidTargetAmount, got {other:?}"),
            }
        }

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM shared_goals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected calls must not leak rows");
    }

    /// SPEC_v3 §Task 6f acceptance criterion: "current amount
    /// increments". Pins the happy path so a regression that
    /// reordered the UPDATE / decoder columns or dropped the
    /// `+= ?2` arithmetic flunks here rather than buried in a
    /// downstream agency-screen test. Asserts both the canonical
    /// returned record AND the durably-committed row (a follow-up
    /// fresh SELECT after the helper returns), the way the v3
    /// helper convention pins both the in-memory and on-disk
    /// outcomes against a regression that returned the right struct
    /// but failed to actually commit.
    #[test]
    fn contribute_to_goal_increments_current_amount_atomically() {
        let (_dir, mut world) = world_with_factions();
        let goal = world
            .create_shared_goal(None, "city-cases-solved", 100)
            .unwrap();

        let after = world
            .contribute_to_goal(goal.id, 25, |_, _| Ok(()))
            .expect("contribution succeeds");

        assert_eq!(after.id, goal.id, "id stable across the contribute call");
        assert_eq!(
            after.current_amount, 25,
            "current_amount moved from 0 to 25"
        );
        assert_eq!(
            after.target_amount, goal.target_amount,
            "target_amount untouched by a contribution"
        );
        assert_eq!(after.state, "active", "state stays active below target");

        // A second contribution accumulates rather than overwriting,
        // pinning the `+=` arithmetic against a regression to `=`.
        let after2 = world
            .contribute_to_goal(goal.id, 10, |_, _| Ok(()))
            .expect("second contribution succeeds");
        assert_eq!(after2.current_amount, 35);

        // Belt-and-braces: the row is durably visible to a fresh
        // SELECT — catches a regression that returned the
        // RETURNING row but failed to actually commit (e.g. a
        // typo'd commit() that dropped the transaction).
        let durable: i64 = world
            .connection()
            .query_row(
                "SELECT current_amount FROM shared_goals WHERE id = ?1",
                [goal.id],
                |row| row.get(0),
            )
            .expect("durable read");
        assert_eq!(durable, 35);
    }

    /// SPEC §4.4 "Contributions MUST be transactional" means the
    /// contributor callback MUST observe the same `rusqlite::
    /// Transaction` the kit's increment ran in — so a callback
    /// write (e.g. a currency debit) commits atomically with the
    /// increment, or rolls back atomically with it. Pin that the
    /// callback receives a live transaction handle and that its
    /// writes land on commit. Mirrors the buyer-callback shape on
    /// `buy_listing`.
    #[test]
    fn contribute_to_goal_runs_contributor_callback_inside_transaction() {
        let (_dir, mut world) = world_with_factions();
        let goal = world.create_shared_goal(None, "k", 100).unwrap();

        // Hand the callback a scratch table to write into. Using a
        // schema-less side table (rather than `players` or
        // `factions`) keeps the assertion narrowly scoped to "did
        // the callback's writes commit?".
        world
            .connection()
            .execute("CREATE TABLE contribute_audit (note TEXT NOT NULL)", [])
            .unwrap();

        let after = world
            .contribute_to_goal(goal.id, 7, |tx, observed_goal| {
                assert_eq!(observed_goal.id, goal.id);
                // Post-update goal: the callback can see the new
                // running total — useful for "you tipped it over"
                // UX. Pinning this against a regression that handed
                // the pre-update row to the callback.
                assert_eq!(observed_goal.current_amount, 7);
                tx.execute(
                    "INSERT INTO contribute_audit (note) VALUES (?1)",
                    rusqlite::params!["alice paid 7"],
                )?;
                Ok(())
            })
            .expect("callback succeeds");

        assert_eq!(after.current_amount, 7);

        let audit: String = world
            .connection()
            .query_row("SELECT note FROM contribute_audit", [], |row| row.get(0))
            .expect("audit row was committed alongside the increment");
        assert_eq!(audit, "alice paid 7");
    }

    /// SPEC §4.4 "Contributions MUST be transactional": a callback
    /// that returns `Err` MUST roll back BOTH the kit's increment
    /// AND every write the callback attempted before failing. Pin
    /// the rollback by:
    ///
    /// 1. Asserting the helper raised `ContributorCallback` (not
    ///    `Sqlite`, not `GoalNotFound`) so game UIs can branch on
    ///    the failure source.
    /// 2. Asserting `current_amount` is unchanged from before the
    ///    call (the increment rolled back).
    /// 3. Asserting any callback-side write (here, an audit row)
    ///    is absent (the callback writes rolled back).
    ///
    /// Without this guarantee a "you don't have enough clue points"
    /// rejection from the callback would still bump the goal's
    /// running total — rewarding the agency for a contribution that
    /// never actually happened.
    #[test]
    fn contribute_to_goal_rolls_back_on_callback_failure() {
        let (_dir, mut world) = world_with_factions();
        let goal = world.create_shared_goal(None, "k", 100).unwrap();
        // Pre-charge the goal so the rollback assertion has a non-
        // trivial baseline to compare against (a regression that
        // accidentally reset to 0 on rollback would still pass a
        // "current_amount == 0" check).
        world.contribute_to_goal(goal.id, 5, |_, _| Ok(())).unwrap();

        world
            .connection()
            .execute("CREATE TABLE contribute_audit (note TEXT NOT NULL)", [])
            .unwrap();

        let err = world
            .contribute_to_goal(goal.id, 50, |tx, _| {
                // Attempt a write that WOULD persist on commit, then
                // fail — this is the realistic shape: the callback
                // partially debits the contributor and *then*
                // discovers it can't satisfy the rest, returning
                // Err and trusting the kit to roll everything back.
                tx.execute(
                    "INSERT INTO contribute_audit (note) VALUES (?1)",
                    rusqlite::params!["partial debit"],
                )?;
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
                    Some("simulated callback failure".to_string()),
                ))
            })
            .expect_err("callback failure must surface");

        match err {
            FactionError::ContributorCallback { source: _ } => {}
            other => panic!("expected ContributorCallback, got {other:?}"),
        }

        let current: i64 = world
            .connection()
            .query_row(
                "SELECT current_amount FROM shared_goals WHERE id = ?1",
                [goal.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            current, 5,
            "current_amount must roll back to its pre-call value"
        );

        let audit_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM contribute_audit", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            audit_count, 0,
            "callback writes must roll back alongside the increment"
        );
    }

    /// `amount <= 0` is rejected before the transaction opens. Pin
    /// both `0` (no-op trap) and a negative value (would silently
    /// rewind progress for every other contributor). Same defensive
    /// shape as `create_shared_goal_rejects_non_positive_target_amount`
    /// and `buy_listing`'s non-positive-quantity guard.
    #[test]
    fn contribute_to_goal_rejects_non_positive_amount() {
        let (_dir, mut world) = world_with_factions();
        let goal = world.create_shared_goal(None, "k", 100).unwrap();

        for bad in [0_i64, -1, -100] {
            let err = world
                .contribute_to_goal(goal.id, bad, |_, _| Ok(()))
                .expect_err("non-positive amount must be rejected");
            match err {
                FactionError::NonPositiveContribution { actual } => assert_eq!(actual, bad),
                other => panic!("expected NonPositiveContribution for {bad}, got {other:?}"),
            }
        }

        // Row-unchanged invariant: rejected calls leave the goal as
        // it was — no half-opened transaction, no logged "attempt".
        let current: i64 = world
            .connection()
            .query_row(
                "SELECT current_amount FROM shared_goals WHERE id = ?1",
                [goal.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current, 0);
    }

    /// `goal_id` referencing a missing row surfaces
    /// `GoalNotFound` — distinct from `GoalAlreadyCompleted` so the
    /// agency UI can branch on the recovery hint. Pins the
    /// diagnostic SELECT path against a regression that swallowed
    /// the missing-row case as a generic `Sqlite`. Mirrors the shape
    /// of `buy_listing`'s `NotFound` path.
    #[test]
    fn contribute_to_goal_returns_not_found_for_missing_goal() {
        let (_dir, mut world) = world_with_factions();

        let err = world
            .contribute_to_goal(9_999, 5, |_, _| Ok(()))
            .expect_err("missing goal must be rejected");
        match err {
            FactionError::GoalNotFound { id } => assert_eq!(id, 9_999),
            other => panic!("expected GoalNotFound, got {other:?}"),
        }
    }

    /// SPEC §4.4 pins the state machine at `active -> completed`.
    /// A goal whose `state` is already `'completed'` rejects further
    /// contributions with `GoalAlreadyCompleted` — distinct from
    /// `GoalNotFound` so the agency UI can render a "this clue
    /// board is already solved" terminal state rather than the
    /// generic "not found" recovery flow. Manually flips the state
    /// (Task 6g will land the helper that does this in production)
    /// to exercise the rejection branch in isolation. Also pins the
    /// row-unchanged invariant so a rejected contribute leaves
    /// `current_amount` exactly where the completed-state flip put
    /// it.
    #[test]
    fn contribute_to_goal_rejects_completed_goal() {
        let (_dir, mut world) = world_with_factions();
        let goal = world.create_shared_goal(None, "k", 100).unwrap();
        world
            .connection()
            .execute(
                "UPDATE shared_goals SET state = 'completed', current_amount = 100 \
                 WHERE id = ?1",
                [goal.id],
            )
            .unwrap();

        let err = world
            .contribute_to_goal(goal.id, 5, |_, _| Ok(()))
            .expect_err("completed goal must be rejected");
        match err {
            FactionError::GoalAlreadyCompleted { id } => assert_eq!(id, goal.id),
            other => panic!("expected GoalAlreadyCompleted, got {other:?}"),
        }

        let current: i64 = world
            .connection()
            .query_row(
                "SELECT current_amount FROM shared_goals WHERE id = ?1",
                [goal.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            current, 100,
            "current_amount unchanged after a rejected contribute"
        );
    }

    /// Helper to build a `FogletContext` — same shape as
    /// `notices::tests::ctx` and `challenges::tests::ctx`.
    /// Deliberate duplication so the faction tests don't reach
    /// across modules.
    fn ctx(user_id: &str, username: &str) -> crate::foglet::FogletContext {
        crate::foglet::FogletContext {
            door_id: "test-door".to_string(),
            user_id: Some(user_id.to_string()),
            username: Some(username.to_string()),
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: crate::foglet::ContextSource::ContextFile,
        }
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
