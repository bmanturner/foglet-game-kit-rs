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

use crate::world_db::{WorldDb, WorldMigration};
use thiserror::Error;

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

/// Kit-internal cap for a bounty's player- or game-authored `title`.
///
/// SPEC_v3 §5.2 ships only `max_notice_body_chars` as configurable;
/// bounty titles follow the same convention as
/// [`crate::notices::NOTICE_SUBJECT_MAX_CHARS`] and
/// [`crate::market::MARKET_DISPLAY_NAME_MAX_CHARS`] — bounded by the
/// kit, not by game authors, so every consuming game presents a
/// uniform "short title" UI on the bounty board. 120 chars matches
/// the notice-subject and market-display-name caps and fits one
/// 80-column line with room for a reward suffix on board list views.
///
/// Counted in Unicode scalar values (`str::chars().count()`), not
/// bytes — SPEC §4.5 / §7 talk in *characters*, and a byte cap would
/// let a single emoji eat four "chars" of budget. Same rule as the
/// notice-subject and market-display-name caps.
pub const BOUNTY_TITLE_MAX_CHARS: usize = 120;

/// Kit-internal cap for a bounty's player- or game-authored
/// `description`.
///
/// SPEC §4.5 frames `description` as the longer body copy explaining
/// the work and the evidence required. The kit caps it at 1000 chars
/// — the same default the notice-body field ships with via
/// `MultiplayerSection::max_notice_body_chars` — but bounty
/// descriptions are not configurable per game in v3 (SPEC §5.2 only
/// lists the notice body knob). 1000 chars is a few short paragraphs:
/// enough to set up a clue chain, list evidence requirements, and
/// add some flavour, without inviting a wall of text the bounty board
/// can't render in its detail panel.
///
/// Counted in Unicode scalar values, the same rule as
/// [`BOUNTY_TITLE_MAX_CHARS`] and every other v3 player-authored cap.
pub const BOUNTY_DESCRIPTION_MAX_CHARS: usize = 1000;

/// Lifecycle state for a [`Bounty`] — SPEC_v3 §4.5 vocabulary.
///
/// The kit's helpers ([`WorldDb::post_bounty`] and the upcoming
/// 7c–7e transitions) use this enum at their boundaries so call sites
/// get exhaustive matches and a typed transition target rather than
/// stringly-typed magic. The wire/storage representation stays as
/// `TEXT` (see [`BOUNTIES_MIGRATION`]); [`Self::as_str`] is the one
/// place the mapping lives so a future state addition is a single
/// edit (schema migration plus enum variant). Same shape as
/// [`crate::challenges::ChallengeState`].
///
/// Variants are listed in the natural lifecycle order — `Open` first,
/// terminal states last — so `Debug` output reads naturally in
/// failure messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BountyState {
    /// Freshly posted and awaiting a claimant. SPEC §4.5 default;
    /// matches the schema-level `DEFAULT 'open'`.
    Open,
    /// A claimant has picked up the bounty and is working on it. Set
    /// by Task 7c on the `open -> claimed` transition.
    Claimed,
    /// The claimant submitted evidence and game code accepted it.
    /// Terminal state set by Task 7d on `claimed -> completed`.
    Completed,
    /// The bounty's `expires_at` deadline lapsed before completion.
    /// Terminal state set by Task 7e's sweeper on `open -> expired`
    /// or `claimed -> expired`.
    Expired,
}

impl BountyState {
    /// Short text encoding used in the `state` column and the
    /// schema-level `CHECK` constraint. The kit never persists a
    /// state via any other path, so this method is the single
    /// source-of-truth for how the enum hits SQLite. Same shape as
    /// [`crate::challenges::ChallengeState::as_str`].
    pub fn as_str(self) -> &'static str {
        match self {
            BountyState::Open => "open",
            BountyState::Claimed => "claimed",
            BountyState::Completed => "completed",
            BountyState::Expired => "expired",
        }
    }
}

/// Failure modes for [`WorldDb::post_bounty`] and the upcoming
/// bounty-lifecycle helpers.
///
/// Library-internal `thiserror` shape — the runtime wraps these with
/// `anyhow` at the process boundary. Mirrors
/// [`crate::market::MarketError`] and
/// [`crate::challenges::ChallengeError`] so all v3 multiplayer write
/// paths surface errors with the same shape. (A future code review
/// pass could fold these into a single multiplayer-error trait once
/// every v3 primitive lands; SPEC tenet "no premature abstraction"
/// keeps them separate today — there is no shared consumer yet.)
///
/// Task 7b needs: `EmptyTitle`, `EmptyDescription`, `EmptyReward`,
/// `TitleTooLong`, `DescriptionTooLong`, and `Sqlite`. `NotFound`
/// and the transition-time variants land with Tasks 7c–7e.
#[derive(Debug, Error)]
pub enum BountyError {
    /// `title` was empty. SPEC §4.5 lists `title` as required; the
    /// kit additionally rejects the empty string here so the bounty
    /// board never renders a row with a blank headline that the
    /// browser can't tell apart from a rendering bug. Same rationale
    /// as [`crate::notices::NoticeError::EmptySubject`] and
    /// [`crate::market::MarketError::EmptyDisplayName`].
    #[error("bounty title must not be empty")]
    EmptyTitle,
    /// `description` was empty. Same rationale as
    /// [`Self::EmptyTitle`]: schema-level `NOT NULL` accepts `""`,
    /// but a bounty whose detail screen is blank is indistinguishable
    /// from a UI glitch. Player-authored "see attached" or "ask the
    /// poster" bounties belong in a flavour line in the description,
    /// not in a literally-empty body.
    #[error("bounty description must not be empty")]
    EmptyDescription,
    /// `reward` was empty. SPEC §4.5 lists `reward JSON` without an
    /// "optional" modifier — every bounty advertises a payout. The
    /// kit additionally rejects the empty string at the boundary so
    /// a regression that dropped the reward mid-call (an
    /// `unwrap_or_default()` pattern, say) surfaces here as a typed
    /// error rather than as an indistinguishable-from-narrative `""`
    /// payload in the audit view. Game code that genuinely has no
    /// structured reward MUST still pass an explicit JSON value
    /// (e.g. `"{}"` or `r#"{"credits":0}"#`) so the "no payout" case
    /// is intentional, not accidental. Same defensive shape as
    /// [`crate::challenges::ChallengeError::EmptyResult`].
    #[error("bounty reward must not be empty")]
    EmptyReward,
    /// `title` exceeded [`BOUNTY_TITLE_MAX_CHARS`]. Surfacing both
    /// the limit and the actual length lets the authoring screen
    /// show "120 / 137 characters" without re-counting. Same shape
    /// as [`crate::notices::NoticeError::SubjectTooLong`].
    #[error("bounty title exceeds {max}-character limit (got {actual})")]
    TitleTooLong {
        /// Cap that was breached — currently always
        /// [`BOUNTY_TITLE_MAX_CHARS`], named so future per-game caps
        /// (if ever introduced) don't break the error shape.
        max: usize,
        /// Actual `chars().count()` of the rejected title, in scalar
        /// values.
        actual: usize,
    },
    /// `description` exceeded [`BOUNTY_DESCRIPTION_MAX_CHARS`]. Same
    /// field shape as [`Self::TitleTooLong`] so authoring screens
    /// can render both failures with one helper.
    #[error("bounty description exceeds {max}-character limit (got {actual})")]
    DescriptionTooLong {
        /// Cap that was breached — currently always
        /// [`BOUNTY_DESCRIPTION_MAX_CHARS`].
        max: usize,
        /// Actual `chars().count()` of the rejected description, in
        /// scalar values.
        actual: usize,
    },
    /// The `INSERT … RETURNING` round-trip (or a future `UPDATE` on
    /// a transition) failed. Wrapping `rusqlite::Error` keeps the
    /// call site readable (one error type, one mapping) while
    /// preserving the underlying cause for `tracing` and operator-
    /// facing messages. Same shape as
    /// [`crate::market::MarketError::Sqlite`].
    #[error("failed to write bounty to world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the statement.
        #[source]
        source: rusqlite::Error,
    },
}

/// In-memory mirror of a `bounties` row — SPEC_v3 §4.5.
///
/// Returned by [`WorldDb::post_bounty`] and the upcoming Task 7c–7e
/// transition helpers. The struct shape matches the table column
/// shape one-for-one and in declaration order so `row_to_bounty` is
/// a positional decode and a future column reorder surfaces as a
/// type error rather than as a silent field swap. Same convention as
/// [`crate::market::MarketListing`] and
/// [`crate::challenges::Challenge`].
///
/// `state` is kept as a `String` (not [`BountyState`]) on purpose:
/// a future schema state added without a matching enum variant still
/// surfaces here verbatim instead of crashing on decode. The typed
/// enum is for *write* boundaries (transition helpers); the read
/// surface is forgiving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounty {
    /// Autoincrement primary key. Doubles as the deterministic
    /// tiebreaker for queries that order by `created_at` and need a
    /// stable secondary sort, the same role as `notices.id`,
    /// `challenges.id`, and `market_listings.id`.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text so it sorts lexically
    /// the same way it sorts chronologically. See module docs.
    pub created_at: String,
    /// Poster's `players.id`, or `None` for system / NPC-organisation
    /// bounties. SPEC §4.5 explicitly lists "posted_by player id
    /// optional".
    pub posted_by_player_id: Option<i64>,
    /// Player- or game-authored short headline rendered on the bounty
    /// board listing. Length bounds are enforced by
    /// [`WorldDb::post_bounty`] at [`BOUNTY_TITLE_MAX_CHARS`], not at
    /// the storage layer.
    pub title: String,
    /// Player- or game-authored body copy explaining the work and
    /// the evidence required. Length bounds are enforced by
    /// [`WorldDb::post_bounty`] at [`BOUNTY_DESCRIPTION_MAX_CHARS`].
    pub description: String,
    /// Opaque reward payload (typically JSON describing currency,
    /// items, faction reputation). Round-tripped verbatim; the kit
    /// imposes no schema. Required — see [`BountyError::EmptyReward`].
    pub reward: String,
    /// Lifecycle state — one of `open`, `claimed`, `completed`,
    /// `expired`. The schema-level `CHECK` constraint pins the
    /// vocabulary; see [`BOUNTIES_MIGRATION`].
    pub state: String,
    /// Claimant's `players.id`, or `None` while the bounty is still
    /// open or expired-without-claim. Populated on the
    /// `open -> claimed` transition (Task 7c) and preserved through
    /// `claimed -> completed` (Task 7d).
    pub claimed_by_player_id: Option<i64>,
    /// ISO timestamp of the `open -> claimed` transition, or `None`
    /// while the bounty has not been claimed.
    pub claimed_at: Option<String>,
    /// ISO timestamp of the `claimed -> completed` transition, or
    /// `None` while the bounty has not been completed.
    pub completed_at: Option<String>,
    /// Optional ISO deadline. After this time, the Task 7e sweeper
    /// may transition an `open` or `claimed` bounty to `expired`.
    /// `None` means open-ended (no deadline).
    pub expires_at: Option<String>,
}

impl WorldDb {
    /// Insert one row into `bounties` and return the canonical
    /// [`Bounty`] SQLite produced (SPEC_v3 §4.5 / §Task 7b).
    ///
    /// The contract is "the bounty I asked you to post is now
    /// durably on the board, in state `open`, with the id and
    /// `created_at` SQLite assigned, and `claimed_by_player_id` /
    /// `claimed_at` / `completed_at` all still `NULL`". The state
    /// is intentionally not a parameter — SPEC §4.5 mandates `open`
    /// as the entry state per Task 7b's "starts open" acceptance,
    /// and the kit owns that invariant. The schema-level
    /// `DEFAULT 'open'` plus this helper's `RETURNING` round-trip
    /// keeps the lifecycle honest: even an operator who tampered
    /// with the helper signature can't smuggle a row in at
    /// `claimed` without also dropping the migration's CHECK.
    ///
    /// `posted_by_player_id` is `Option<i64>` because SPEC §4.5
    /// explicitly allows "posted_by player id optional" — detective
    /// agencies, the city, or other in-game NPC organisations may
    /// post bounties without a real Foglet user behind them.
    /// `title`, `description`, and `reward` are required text;
    /// `expires_at` is optional (`None` means open-ended, no
    /// deadline). The lifecycle columns (`claimed_by_player_id`,
    /// `claimed_at`, `completed_at`) are intentionally not exposed
    /// on this path — a brand-new bounty has no claimant or
    /// completion, and exposing them as parameters would invite a
    /// regression where game code populated them before the
    /// `open -> claimed -> completed` transitions.
    ///
    /// # Validation order
    ///
    /// All checks run **before** the SQL round-trip so a rejected
    /// bounty never produces a row, an autoincrement gap, or a
    /// future event-log entry. Order — emptiness first (cheapest),
    /// then length — mirrors [`Self::send_notice`] and
    /// [`Self::create_listing`] so a UI that re-renders the
    /// authoring screen on failure shows a consistent surface
    /// across primitives. Title is checked before description
    /// because a blank title makes the board listing un-renderable
    /// regardless of how detailed the description is; reward is
    /// checked last because it's the most likely game-code-supplied
    /// (rather than player-typed) field and a regression there
    /// usually indicates a code bug rather than a UX problem.
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) to read the
    /// canonical row — `id`, the SQL-side `created_at`, the
    /// `'open'` default for `state`, plus every other column —
    /// without a second round-trip, the same pattern as
    /// [`Self::send_notice`], [`Self::create_listing`], and
    /// [`Self::create_challenge`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single insert statement under the
    /// configured busy timeout. SPEC §4.5 requires
    /// "Claim/complete transitions MUST be transactional"; the
    /// *posting* path is a single `INSERT` and thus already
    /// atomic, so no explicit transactional wrapper is needed
    /// here. The 7c–7e transition helpers will need explicit
    /// transactions because they read the current state, validate,
    /// and then write.
    pub fn post_bounty(
        &self,
        posted_by_player_id: Option<i64>,
        title: &str,
        description: &str,
        reward: &str,
        expires_at: Option<&str>,
    ) -> Result<Bounty, BountyError> {
        // Validation runs before the SQL round-trip so a rejected
        // bounty never produces a row. Emptiness first (cheapest),
        // then length — same ordering as the notice and market
        // paths, so a UI that re-renders the authoring screen on
        // failure shows a consistent surface across primitives.
        if title.is_empty() {
            return Err(BountyError::EmptyTitle);
        }
        if description.is_empty() {
            return Err(BountyError::EmptyDescription);
        }
        if reward.is_empty() {
            return Err(BountyError::EmptyReward);
        }
        // Count Unicode scalar values, not bytes — SPEC §4.5 / §7
        // talk in characters, and a byte cap would penalise non-
        // ASCII titles and descriptions.
        let title_chars = title.chars().count();
        if title_chars > BOUNTY_TITLE_MAX_CHARS {
            return Err(BountyError::TitleTooLong {
                max: BOUNTY_TITLE_MAX_CHARS,
                actual: title_chars,
            });
        }
        let description_chars = description.chars().count();
        if description_chars > BOUNTY_DESCRIPTION_MAX_CHARS {
            return Err(BountyError::DescriptionTooLong {
                max: BOUNTY_DESCRIPTION_MAX_CHARS,
                actual: description_chars,
            });
        }

        // `RETURNING` echoes the full row back — including the
        // SQL-side `CURRENT_TIMESTAMP` default for `created_at` and
        // the `'open'` default for `state`. The column order here
        // matches `row_to_bounty` so all read paths share one
        // decoder.
        const SQL: &str = "\
INSERT INTO bounties \
    (posted_by_player_id, title, description, reward, expires_at) \
VALUES (?1, ?2, ?3, ?4, ?5) \
RETURNING id, created_at, posted_by_player_id, title, description, \
          reward, state, claimed_by_player_id, claimed_at, \
          completed_at, expires_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![posted_by_player_id, title, description, reward, expires_at,],
                row_to_bounty,
            )
            .map_err(|source| BountyError::Sqlite { source })
    }
}

/// Decode one `bounties` row into a [`Bounty`].
///
/// Pulled out so the Task 7b write path and the upcoming Task
/// 7c–7e transition / query paths can share one decoder. Column
/// order matches the `RETURNING` clause in
/// [`WorldDb::post_bounty`] *and* future SELECTs (e.g. an
/// `open_bounties` listing); a regression that reorders columns
/// will surface here as a type error rather than as a silent
/// field swap. Same shape as [`crate::market::row_to_listing`]
/// and the challenges decoder.
fn row_to_bounty(row: &rusqlite::Row<'_>) -> rusqlite::Result<Bounty> {
    Ok(Bounty {
        id: row.get(0)?,
        created_at: row.get(1)?,
        posted_by_player_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        reward: row.get(5)?,
        state: row.get(6)?,
        claimed_by_player_id: row.get(7)?,
        claimed_at: row.get(8)?,
        completed_at: row.get(9)?,
        expires_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
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

    /// Variant of [`world_with_bounties`] that pre-creates one
    /// poster player and returns its `players.id`. Used by every
    /// `post_bounty` test that needs an attributable poster.
    /// `system`-poster tests (where `posted_by_player_id = None`)
    /// can still use the bare [`world_with_bounties`] helper.
    /// Same shape as `market::tests::world_with_listings`.
    fn world_with_poster() -> (tempfile::TempDir, WorldDb, i64) {
        let (dir, world) = world_with_bounties();
        let poster_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-poster', 'poster') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert poster");
        (dir, world, poster_id)
    }

    /// SPEC_v3 §Task 7b acceptance: a valid `post_bounty` round-
    /// trips a fully-populated [`Bounty`] back to the caller — the
    /// kit-assigned `id` is non-zero, `created_at` is the SQLite-
    /// stamped ISO timestamp, every input field is preserved
    /// verbatim, **`state` starts as `'open'`** (the explicit Task
    /// 7b acceptance), and the lifecycle columns
    /// (`claimed_by_player_id`, `claimed_at`, `completed_at`) are
    /// all `NULL`. Pinning the full struct here guards against a
    /// regression that silently dropped a column from the
    /// `RETURNING` clause or the row decoder.
    #[test]
    fn post_bounty_round_trips_full_row_in_open_state() {
        let (_dir, world, poster_id) = world_with_poster();

        let bounty = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Last seen on the 3rd-floor landing. Bring photographic evidence.",
                r#"{"credits":250}"#,
                Some("2099-01-01T00:00:00Z"),
            )
            .expect("post_bounty succeeds with valid inputs");

        assert!(bounty.id > 0, "kit-assigned id must be positive");
        assert!(
            !bounty.created_at.is_empty(),
            "SQLite must stamp created_at"
        );
        assert_eq!(bounty.posted_by_player_id, Some(poster_id));
        assert_eq!(bounty.title, "Find the missing pocketwatch");
        assert_eq!(
            bounty.description,
            "Last seen on the 3rd-floor landing. Bring photographic evidence."
        );
        assert_eq!(bounty.reward, r#"{"credits":250}"#);
        // The §Task 7b "starts open" acceptance criterion. Pinned
        // against both the schema-side default and the typed enum
        // encoding so a regression in either layer flunks here.
        assert_eq!(bounty.state, BountyState::Open.as_str());
        assert_eq!(bounty.state, "open");
        assert_eq!(
            bounty.claimed_by_player_id, None,
            "fresh bounty must have no claimant"
        );
        assert_eq!(
            bounty.claimed_at, None,
            "fresh bounty must have no claimed_at"
        );
        assert_eq!(
            bounty.completed_at, None,
            "fresh bounty must have no completed_at"
        );
        assert_eq!(
            bounty.expires_at.as_deref(),
            Some("2099-01-01T00:00:00Z"),
            "expires_at must round-trip verbatim"
        );

        // Belt-and-braces: a fresh SELECT must see exactly one row
        // with the same `state = 'open'`. Catches a regression
        // where `RETURNING` showed `'open'` but the actual INSERT
        // wrote a different state (e.g. via a future trigger that
        // overwrote the default).
        let durable_state: String = world
            .connection()
            .query_row(
                "SELECT state FROM bounties WHERE id = ?1",
                [bounty.id],
                |row| row.get(0),
            )
            .expect("durable state SELECT runs");
        assert_eq!(durable_state, "open");
    }

    /// SPEC §4.5 explicitly allows "posted_by player id optional"
    /// — system / NPC-organisation bounties (e.g. the city offering
    /// a clue bounty in `murder_motel`). A `None`
    /// `posted_by_player_id` MUST round-trip — pinned here so a
    /// future helper that defensively defaults to "must have a
    /// poster" flunks the test rather than silently breaking the
    /// system-bounty path.
    #[test]
    fn post_bounty_accepts_no_poster_for_system_bounties() {
        let (_dir, world) = world_with_bounties();

        let bounty = world
            .post_bounty(
                None,
                "City clue bounty",
                "Bring evidence of the locked-room mystery.",
                r#"{"credits":100}"#,
                None,
            )
            .expect("post_bounty succeeds with no poster");

        assert_eq!(bounty.posted_by_player_id, None);
        assert_eq!(bounty.state, "open");
        assert_eq!(
            bounty.expires_at, None,
            "open-ended bounty (no deadline) must round-trip None"
        );
    }

    /// SPEC §4.5 lists `title` as required. The kit additionally
    /// rejects `""` at the boundary so a blank-title regression
    /// surfaces as a typed [`BountyError::EmptyTitle`] before it
    /// touches `world.sqlite`. Mirrors
    /// `create_listing_rejects_empty_display_name` and
    /// `send_notice_rejects_empty_subject`.
    #[test]
    fn post_bounty_rejects_empty_title() {
        let (_dir, world, poster_id) = world_with_poster();

        let err = world
            .post_bounty(Some(poster_id), "", "non-empty body", r#"{"x":1}"#, None)
            .expect_err("empty title must be rejected");
        assert!(
            matches!(err, BountyError::EmptyTitle),
            "expected EmptyTitle, got {err:?}"
        );

        // The rejected bounty MUST NOT have produced a row — pin
        // that explicitly so a future "validate after the round-
        // trip" regression surfaces here. Same belt-and-braces as
        // the market and notice equivalents.
        let row_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM bounties", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            row_count, 0,
            "rejected bounty must not have produced a bounties row"
        );
    }

    /// SPEC §4.5 lists `description` as required. Empty
    /// descriptions are rejected at the boundary for the same
    /// reason as empty notice bodies — a bounty whose detail
    /// screen is blank is indistinguishable from a UI bug.
    #[test]
    fn post_bounty_rejects_empty_description() {
        let (_dir, world, poster_id) = world_with_poster();

        let err = world
            .post_bounty(Some(poster_id), "Title here", "", r#"{"x":1}"#, None)
            .expect_err("empty description must be rejected");
        assert!(
            matches!(err, BountyError::EmptyDescription),
            "expected EmptyDescription, got {err:?}"
        );
    }

    /// SPEC §4.5 lists `reward JSON` without an "optional" modifier
    /// — every bounty advertises a payout. The kit additionally
    /// rejects `""` at the boundary so a "regression dropped the
    /// reward" bug surfaces as a typed
    /// [`BountyError::EmptyReward`] rather than a phantom audit
    /// row whose payout looks identical to a no-payout narrative
    /// bounty. Game code that genuinely has no structured payout
    /// MUST still pass `"{}"` (or similar) explicitly.
    #[test]
    fn post_bounty_rejects_empty_reward() {
        let (_dir, world, poster_id) = world_with_poster();

        let err = world
            .post_bounty(Some(poster_id), "Title", "Description", "", None)
            .expect_err("empty reward must be rejected");
        assert!(
            matches!(err, BountyError::EmptyReward),
            "expected EmptyReward, got {err:?}"
        );
    }

    /// `title` exactly at [`BOUNTY_TITLE_MAX_CHARS`] MUST be
    /// accepted; one character over MUST be rejected. Counted in
    /// Unicode scalar values, not bytes — same rule as
    /// `NOTICE_SUBJECT_MAX_CHARS` and
    /// `MARKET_DISPLAY_NAME_MAX_CHARS`. Boundary tests prevent
    /// off-by-one regressions in either direction.
    #[test]
    fn post_bounty_enforces_title_length_at_boundary() {
        let (_dir, world, poster_id) = world_with_poster();

        let at_limit: String = "a".repeat(BOUNTY_TITLE_MAX_CHARS);
        let bounty = world
            .post_bounty(Some(poster_id), &at_limit, "body", r#"{"x":1}"#, None)
            .expect("title exactly at limit must be accepted");
        assert_eq!(bounty.title.chars().count(), BOUNTY_TITLE_MAX_CHARS);

        let over_limit: String = "a".repeat(BOUNTY_TITLE_MAX_CHARS + 1);
        let err = world
            .post_bounty(Some(poster_id), &over_limit, "body", r#"{"x":1}"#, None)
            .expect_err("title one over the limit must be rejected");
        match err {
            BountyError::TitleTooLong { max, actual } => {
                assert_eq!(max, BOUNTY_TITLE_MAX_CHARS);
                assert_eq!(actual, BOUNTY_TITLE_MAX_CHARS + 1);
            }
            other => panic!("expected TitleTooLong, got {other:?}"),
        }
    }

    /// `description` exactly at [`BOUNTY_DESCRIPTION_MAX_CHARS`]
    /// MUST be accepted; one character over MUST be rejected.
    /// Same boundary-test rationale as the title-length test above.
    #[test]
    fn post_bounty_enforces_description_length_at_boundary() {
        let (_dir, world, poster_id) = world_with_poster();

        let at_limit: String = "d".repeat(BOUNTY_DESCRIPTION_MAX_CHARS);
        let bounty = world
            .post_bounty(Some(poster_id), "title", &at_limit, r#"{"x":1}"#, None)
            .expect("description exactly at limit must be accepted");
        assert_eq!(
            bounty.description.chars().count(),
            BOUNTY_DESCRIPTION_MAX_CHARS
        );

        let over_limit: String = "d".repeat(BOUNTY_DESCRIPTION_MAX_CHARS + 1);
        let err = world
            .post_bounty(Some(poster_id), "title", &over_limit, r#"{"x":1}"#, None)
            .expect_err("description one over the limit must be rejected");
        match err {
            BountyError::DescriptionTooLong { max, actual } => {
                assert_eq!(max, BOUNTY_DESCRIPTION_MAX_CHARS);
                assert_eq!(actual, BOUNTY_DESCRIPTION_MAX_CHARS + 1);
            }
            other => panic!("expected DescriptionTooLong, got {other:?}"),
        }
    }

    /// Two posts back-to-back must each get a distinct
    /// autoincrement id. The schema relies on SQLite's rowid alias
    /// behaviour for `INTEGER PRIMARY KEY`, but a future migration
    /// edit (e.g. adding `WITHOUT ROWID` or a custom default) could
    /// break that silently — pin it here so the regression flunks
    /// at `cargo test`. Same shape as the corresponding market /
    /// notice round-trip tests.
    #[test]
    fn post_bounty_assigns_distinct_ids_for_back_to_back_posts() {
        let (_dir, world, poster_id) = world_with_poster();

        let first = world
            .post_bounty(Some(poster_id), "first", "body", r#"{"x":1}"#, None)
            .expect("first post succeeds");
        let second = world
            .post_bounty(Some(poster_id), "second", "body", r#"{"x":2}"#, None)
            .expect("second post succeeds");

        assert_ne!(
            first.id, second.id,
            "back-to-back bounties must get distinct ids"
        );
        assert!(
            second.id > first.id,
            "autoincrement id must move monotonically forward (got {} then {})",
            first.id,
            second.id
        );
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
