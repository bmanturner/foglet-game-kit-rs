//! `bounties` — shared-world bounty/job-board schema ( /
//! ).
//!
//!  introduces a durable async bounty board: a poster (a player or
//! the system) writes up a job with a reward payload, claimants pick
//! it up, and one of them eventually completes it with game-defined
//! evidence. The whole lifecycle sits on top of a single `bounties`
//! table whose shape is pinned by [`BOUNTIES_MIGRATION`]. This module
//! exists only to declare that schema and prove it applies; the
//! `Bounty` Rust type and the `post_bounty` `claim_bounty` /
//! `complete_bounty` `expire_bounties` helpers land in subsequent
//!  sub-items (7b–7f). Splitting the migration into its own
//! commit keeps the bisect signal sharp — a column rename, a relaxed
//! `CHECK`, or a dropped partial index flunks the schema test in this
//! module rather than a higher-level state-machine test that's harder
//! to attribute. Same convention as [`crate::challenges`]
//! and [`crate::market`].
//!
//! # Why a dedicated table
//!
//!  lists bounties alongside notices, challenges, market
//! listings, and factions as separate primitives. We follow the same
//! v2/convention: one table, one migration, one named index
//! family. Folding bounties onto `world_events` would conflate the
//! append-only event stream with mutable lifecycle state (`state`.
//! `claimed_by_player_id`, `claimed_at`, `completed_at`)
//! fundamentally different write patterns. Folding bounties onto
//! `challenges` would force one state machine to model two domains
//! (rival challenges are private and 1:1; bounties are public and
//! 1:N → 1) and would burn an extra migration to add the columns
//! later.
//!
//! # Why `version = 10`
//!
//!  occupies migration versions 1–5 (see `docs/shared-world.md`
//! ). claims `6` and above, dense and grouped per primitive:
//! notices=6, challenges=7, market_listings=8, factions=9. Bounties
//! are the fifth and final primitive to land, so they take 10.
//! Game-authored migrations live in their own higher band and are
//! not affected.

use crate::world_db::{WorldDb, WorldMigration};
use thiserror::Error;

/// Schema for the bounty/job-board table —.
///
/// One row per bounty. Bounties are mutable in the narrow sense that
/// `state`, `claimed_by_player_id`, `claimed_at`, and `completed_at`
/// are flipped by the typed helpers landing in ; the
/// addressing, `title`, `description`, `reward`, and creation
/// timestamp are write-once. The kit's contract is "if you only go
/// through the public API, the only state changes are the documented
/// transitions, and every transition runs inside a SQLite
/// transaction". An operator with `sqlite3` can
/// of course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`].
/// [`crate::notices::NOTICES_MIGRATION`].
/// [`crate::challenges::CHALLENGES_MIGRATION`].
/// [`crate::market::MARKET_LISTINGS_MIGRATION`], and
/// [`crate::factions::FACTIONS_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid.
///   Doubles as the deterministic tiebreaker for queries that order
///   by `created_at` and need a stable secondary sort, matching the
///   convention on every other v2/ primitive.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. ISO text so it
///   sorts lexically the same way it sorts chronologically and
///   reads cleanly under the `sqlite3` CLI — the same contract as
///   every other v2/timestamp column.
/// - `posted_by_player_id` — `INTEGER REFERENCES players(id)`.
///   nullable. explicitly lists "posted_by player id
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
///   lives at the helper layer — the schema enforces
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
///   kit owns the lifecycle. Same contract as `challenges.stake`.
///   `notices.metadata`, and `world_events.metadata`. Marked
///   `NOT NULL` because lists `reward` without an
///   "optional" modifier (unlike `posted_by`) — every bounty
///   advertises a payout, even if the JSON encodes "0 credits"
///   for narrative-only bounties.
/// - `state` — `TEXT NOT NULL DEFAULT 'open'` with a `CHECK`
///   constraint pinning `state IN
///   ('open','claimed','completed','expired')`. The state-machine
///   vocabulary lives in the schema so a regression that introduced
///   a new state in code without a matching migration would fail at
///   INSERT/UPDATE time, not silently in production.
///   documents the legal transitions through the
///   acceptance criteria: `open -> claimed -> completed`.
///   `open -> expired`, `claimed -> expired`. Default `'open'`
///   matches 's "starts open" acceptance. Same shape as the
///   `state` column on `challenges`.
/// - `claimed_by_player_id` — `INTEGER REFERENCES players(id)`.
///   nullable. The investigator who claimed the bounty. Stays
///   `NULL` for `open` and `expired`-without-claim rows; populated
///   on the `open -> claimed` transition and preserved
///   through `claimed -> completed` so the completion
///   credit stays attributable. lists this as
///   "claimed_by optional player id".
/// - `claimed_at` — `TEXT`, nullable. ISO timestamp of the
///   `open -> claimed` transition. Same audit-view rationale as
///   `challenges.accepted_at` — lets the UI show "claimed
///   2026-05-09 17:42 UTC" without a separate event lookup.
/// - `completed_at` — `TEXT`, nullable. ISO timestamp of the
///   `claimed -> completed` transition. Stays `NULL` for bounties
///   that never completed (still open, still claimed, expired).
/// - `expires_at` — `TEXT`, nullable. ISO timestamp after which the
///   sweeper may flip an `open` or `claimed` bounty to
///   `expired`. Nullable so a bounty can be open-ended (no
///   deadline) without reserving a sentinel value; the partial
///   index below filters on `expires_at IS NOT NULL` so the sweeper
///   only walks bounties that *can* expire. Same shape as
///   `challenges.expires_at`.
///
/// # Indexes
///
/// Three partial indexes are created up-front so the lookup patterns
///  rely on are seek-bound from the moment they land.
/// Adding them later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the
/// index cost at the same migration that creates the table — the
/// same rationale as the partial indexes on `notices`, `challenges`.
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
///   expires_at IS NOT NULL`. The sweeper walks this in
///   `expires_at` order and stops at the first row where
///   `expires_at > now`, so the cost of "expire all due bounties"
///   stays proportional to the number of bounties that actually
///   need expiring — not to the total bounty count. Both `open`
///   and `claimed` bounties are eligible for expiry
///   (a claimant who never completes their work shouldn't pin the
///   bounty open forever); the index covers both states with one
///   partial predicate.
///
/// # Version
///
/// `version = 10`. uses 1–5; uses 6+ (notices=6, challenges=7.
/// market_listings=8, factions=9). Bounties are the fifth and final
///  primitive to land, so they take 10. v3.1+ migrations pick up
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
///  ships only `max_notice_body_chars` as configurable;
/// bounty titles follow the same convention as
/// [`crate::notices::NOTICE_SUBJECT_MAX_CHARS`] and
/// [`crate::market::MARKET_DISPLAY_NAME_MAX_CHARS`] — bounded by the
/// kit, not by game authors, so every consuming game presents a
/// uniform "short title" UI on the bounty board. 120 chars matches
/// the notice-subject and market-display-name caps and fits one
/// 80-column line with room for a reward suffix on board list views.
///
/// Counted in Unicode scalar values (`str::chars.count`), not
/// bytes — talk in *characters*, and a byte cap would
/// let a single emoji eat four "chars" of budget. Same rule as the
/// notice-subject and market-display-name caps.
pub const BOUNTY_TITLE_MAX_CHARS: usize = 120;

/// Kit-internal cap for a bounty's player- or game-authored
/// `description`.
///
///  frames `description` as the longer body copy explaining
/// the work and the evidence required. The kit caps it at 1000 chars
/// — the same default the notice-body field ships with via
/// `MultiplayerSection::max_notice_body_chars` — but bounty
/// descriptions are not configurable per game in ( only
/// lists the notice body knob). 1000 chars is a few short paragraphs:
/// enough to set up a clue chain, list evidence requirements, and
/// add some flavour, without inviting a wall of text the bounty board
/// can't render in its detail panel.
///
/// Counted in Unicode scalar values, the same rule as
/// [`BOUNTY_TITLE_MAX_CHARS`] and every other player-authored cap.
pub const BOUNTY_DESCRIPTION_MAX_CHARS: usize = 1000;

/// Lifecycle state for a [`Bounty`] — vocabulary.
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
/// Variants are listed in the natural lifecycle order — `Open` first.
/// terminal states last — so `Debug` output reads naturally in
/// failure messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BountyState {
    /// Freshly posted and awaiting a claimant. default;
    /// matches the schema-level `DEFAULT 'open'`.
    Open,
    /// A claimant has picked up the bounty and is working on it. Set
    /// by on the `open -> claimed` transition.
    Claimed,
    /// The claimant submitted evidence and game code accepted it.
    /// Terminal state set by on `claimed -> completed`.
    Completed,
    /// The bounty's `expires_at` deadline lapsed before completion.
    /// Terminal state set by 's sweeper on `open -> expired`
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
/// [`crate::challenges::ChallengeError`] so all multiplayer write
/// paths surface errors with the same shape. (A future code review
/// pass could fold these into a single multiplayer-error trait once
/// every primitive lands; tenet "no premature abstraction"
/// keeps them separate today — there is no shared consumer yet.)
///
///  needs: `EmptyTitle`, `EmptyDescription`, `EmptyReward`.
/// `TitleTooLong`, `DescriptionTooLong`, and `Sqlite`. `NotFound`
/// and the transition-time variants land with.
#[derive(Debug, Error)]
pub enum BountyError {
    /// `title` was empty. lists `title` as required; the
    /// kit additionally rejects the empty string here so the bounty
    /// board never renders a row with a blank headline that the
    /// browser can't tell apart from a rendering bug. Same rationale
    /// as [`crate::notices::NoticeError::EmptySubject`] and
    /// [`crate::market::MarketError::EmptyDisplayName`].
    #[error("bounty title must not be empty")]
    EmptyTitle,
    /// `description` was empty. Same rationale as
    /// [`Self::EmptyTitle`]: schema-level `NOT NULL` accepts `""`.
    /// but a bounty whose detail screen is blank is indistinguishable
    /// from a UI glitch. Player-authored "see attached" or "ask the
    /// poster" bounties belong in a flavour line in the description.
    /// not in a literally-empty body.
    #[error("bounty description must not be empty")]
    EmptyDescription,
    /// `reward` was empty. lists `reward JSON` without an
    /// "optional" modifier — every bounty advertises a payout. The
    /// kit additionally rejects the empty string at the boundary so
    /// a regression that dropped the reward mid-call (an
    /// `unwrap_or_default` pattern, say) surfaces here as a typed
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
    /// show "120 137 characters" without re-counting. Same shape
    /// as [`crate::notices::NoticeError::SubjectTooLong`].
    #[error("bounty title exceeds {max}-character limit (got {actual})")]
    TitleTooLong {
        /// Cap that was breached — currently always
        /// [`BOUNTY_TITLE_MAX_CHARS`], named so future per-game caps
        /// (if ever introduced) don't break the error shape.
        max: usize,
        /// Actual `chars.count` of the rejected title, in scalar
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
        /// Actual `chars.count` of the rejected description, in
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
    /// No `bounties` row exists with the given id. Surfaced by the
    /// transition helpers ([`WorldDb::claim_bounty`] and the upcoming
    /// `complete_bounty` `expire_bounties` paths) when the
    /// caller's id is stale or the row was removed by an operator.
    /// Same shape as [`crate::challenges::ChallengeError::NotFound`].
    #[error("bounty {id} does not exist")]
    NotFound {
        /// The id the caller looked up. Echoed so log lines and
        /// operator-facing errors can name the missing row.
        id: i64,
    },
    /// The transition is not legal from the bounty's current state.
    ///  mandates exactly three transitions: `open ->
    /// claimed`, `claimed -> completed`, and `{open,claimed} ->
    /// expired`. Every other (from, to) pair fails with this
    /// variant. Carrying both `from` (the actual state the row was
    /// found in) and the desired `to` lets the UI render a precise
    /// message — "this bounty is already claimed" vs "this bounty
    /// has been completed". Same shape as
    /// [`crate::challenges::ChallengeError::InvalidTransition`].
    #[error("cannot transition bounty {id} from {from:?} to {to:?}")]
    InvalidTransition {
        /// Bounty id the caller targeted. Echoed so log lines can
        /// name the offending row.
        id: i64,
        /// The state the row was in when the helper read it. Kept
        /// as a `String` (rather than [`BountyState`]) so a future
        /// schema state added without a matching enum variant still
        /// surfaces here verbatim instead of crashing on decode.
        from: String,
        /// The state the helper attempted to set.
        to: BountyState,
    },
    /// The bounty was still in `open` but its `expires_at` deadline
    /// has already passed. makes deadlined bounties
    /// uncl­aimable past their deadline — the kit refuses to flip
    /// `open -> claimed` even before the sweeper runs, so
    /// a slow sweeper can't widen the window in which a stale
    /// bounty looks claimable. A separate variant from
    /// [`Self::InvalidTransition`] lets the UI distinguish "this
    /// bounty's deadline lapsed before you got here" from "this
    /// bounty is in some other terminal state". Same shape as
    /// [`crate::challenges::ChallengeError::Expired`].
    #[error("bounty {id} expired before it could be claimed")]
    Expired {
        /// Bounty id the caller targeted.
        id: i64,
    },
}

/// In-memory mirror of a `bounties` row
///
/// Returned by [`WorldDb::post_bounty`] and the upcoming
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
    /// stable secondary sort, the same role as `notices.id`.
    /// `challenges.id`, and `market_listings.id`.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text so it sorts lexically
    /// the same way it sorts chronologically. See module docs.
    pub created_at: String,
    /// Poster's `players.id`, or `None` for system NPC-organisation
    /// bounties. explicitly lists "posted_by player id
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
    /// Opaque reward payload (typically JSON describing currency.
    /// items, faction reputation). Round-tripped verbatim; the kit
    /// imposes no schema. Required — see [`BountyError::EmptyReward`].
    pub reward: String,
    /// Lifecycle state — one of `open`, `claimed`, `completed`.
    /// `expired`. The schema-level `CHECK` constraint pins the
    /// vocabulary; see [`BOUNTIES_MIGRATION`].
    pub state: String,
    /// Claimant's `players.id`, or `None` while the bounty is still
    /// open or expired-without-claim. Populated on the
    /// `open -> claimed` transition and preserved through
    /// `claimed -> completed`.
    pub claimed_by_player_id: Option<i64>,
    /// ISO timestamp of the `open -> claimed` transition, or `None`
    /// while the bounty has not been claimed.
    pub claimed_at: Option<String>,
    /// ISO timestamp of the `claimed -> completed` transition, or
    /// `None` while the bounty has not been completed.
    pub completed_at: Option<String>,
    /// Optional ISO deadline. After this time, the sweeper
    /// may transition an `open` or `claimed` bounty to `expired`.
    /// `None` means open-ended (no deadline).
    pub expires_at: Option<String>,
}

impl WorldDb {
    /// Insert one row into `bounties` and return the canonical
    /// [`Bounty`] SQLite produced.
    ///
    /// The contract is "the bounty I asked you to post is now
    /// durably on the board, in state `open`, with the id and
    /// `created_at` SQLite assigned, and `claimed_by_player_id` /
    /// `claimed_at` `completed_at` all still `NULL`". The state
    /// is intentionally not a parameter — mandates `open`
    /// as the entry state per 's "starts open" acceptance.
    /// and the kit owns that invariant. The schema-level
    /// `DEFAULT 'open'` plus this helper's `RETURNING` round-trip
    /// keeps the lifecycle honest: even an operator who tampered
    /// with the helper signature can't smuggle a row in at
    /// `claimed` without also dropping the migration's CHECK.
    ///
    /// `posted_by_player_id` is `Option<i64>` because
    /// explicitly allows "posted_by player id optional" — detective
    /// agencies, the city, or other in-game NPC organisations may
    /// post bounties without a real Foglet user behind them.
    /// `title`, `description`, and `reward` are required text;
    /// `expires_at` is optional (`None` means open-ended, no
    /// deadline). The lifecycle columns (`claimed_by_player_id`.
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
    /// future event-log entry. Order — emptiness first (cheapest).
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
    /// `'open'` default for `state`, plus every other column
    /// without a second round-trip, the same pattern as
    /// [`Self::send_notice`], [`Self::create_listing`], and
    /// [`Self::create_challenge`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single insert statement under the
    /// configured busy timeout. requires
    /// "Claim/complete transitions MUST be transactional"; the
    /// *posting* path is a single `INSERT` and thus already
    /// atomic, so no explicit transactional wrapper is needed
    /// here. The 7c–7e transition helpers will need explicit
    /// transactions because they read the current state, validate.
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
        // bounty never produces a row. Emptiness first (cheapest).
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
        // Count Unicode scalar values, not bytes
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

    /// Transition a bounty from `open` to `claimed` and stamp
    /// `claimed_by_player_id` + `claimed_at` ( Task
    /// 7c).
    ///
    /// The contract is "if and only if the row was still `open` and
    /// not past its deadline, it is now `claimed` with the named
    /// claimant and a `claimed_at` timestamp; otherwise the row is
    /// unchanged and the helper returns a typed error explaining
    /// why". lists `open -> claimed` as the only legal
    /// entry into the `claimed` state — every other current state
    /// (already `claimed`, terminal `completed`/`expired`) surfaces
    /// as [`BountyError::InvalidTransition`], and that includes the
    /// "second claimant rejected" acceptance case from the
    /// brief: the loser of a claim race sees `from = "claimed"`.
    /// `to = BountyState::Claimed`, with the row already attributed
    /// to the winner.
    ///
    /// # Conditional UPDATE shape
    ///
    /// The `WHERE` clause folds three checks into one statement so
    /// the success path is a single round-trip:
    ///
    /// 1. `id = ?1` — addresses the row.
    /// 2. `state = 'open'` — only the `open -> claimed` transition
    ///    is legal ; any other current state must fall
    ///    through to the diagnostic SELECT and become an
    ///    [`BountyError::InvalidTransition`].
    /// 3. `expires_at IS NULL OR datetime(expires_at) > datetime('now')`
    ///    — open-ended bounties (`expires_at IS NULL`) are always
    ///    claimable; deadlined bounties are claimable only while
    ///    the deadline is still in the future. Wrapping both sides
    ///    in `datetime(…)` normalises the two ISO forms the kit
    ///    accepts (`'YYYY-MM-DDTHH:MM:SSZ'` from callers.
    ///    `'YYYY-MM-DD HH:MM:SS'` from `CURRENT_TIMESTAMP`) so the
    ///    comparison is chronological rather than lexicographic.
    ///    Same shape as [`Self::accept_challenge`].
    ///
    /// The bookkeeping fields are written in the same statement:
    /// `state = 'claimed'`, `claimed_by_player_id = ?2`, and
    /// `claimed_at = CURRENT_TIMESTAMP` (which `RETURNING` echoes
    /// back as the canonical row).
    ///
    /// # Diagnostic SELECT
    ///
    /// On `QueryReturnedNoRows` the helper performs one diagnostic
    /// SELECT to differentiate the failure modes — `NotFound` if no
    /// row exists, `Expired` if the row is still `open` but past
    /// its deadline, otherwise `InvalidTransition` carrying the
    /// row's actual state. The diagnostic is read-only and races
    /// only on the *error category* surfaced to the caller (a
    /// concurrent UPDATE could change the row between the failed
    /// UPDATE and the diagnostic SELECT); the helper never returns
    /// stale state because it only returns error categories on this
    /// path. Same convention as the diagnostic helper in
    /// [`crate::challenges`].
    ///
    /// # Race semantics — "second claimant rejected"
    ///
    /// Two callers racing on the same `open` bounty hit the same
    /// conditional `UPDATE … WHERE state = 'open'` in series under
    /// SQLite's write lock. Exactly one observes a non-zero row
    /// count and gets the `Ok(Bounty)` echo with their own id in
    /// `claimed_by_player_id`; the loser sees
    /// `QueryReturnedNoRows`, the diagnostic SELECT reads the
    /// just-flipped `'claimed'` state, and the loser surfaces
    /// `InvalidTransition { from: "claimed", to: Claimed }`. This
    /// is the "second claimant rejected" acceptance
    /// pinned by the `claim_bounty_rejects_second_claimant` test.
    ///
    /// # Failure
    ///
    /// - [`BountyError::NotFound`] — no row matches `id`.
    /// - [`BountyError::Expired`] — row is still `open` but its
    ///   `expires_at` deadline has passed.
    /// - [`BountyError::InvalidTransition`] — row is in any state
    ///   other than `open` (already `claimed`, `completed`, or
    ///   swept to `expired` by ).
    /// - [`BountyError::Sqlite`] — any other `rusqlite` error.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an
    /// optional diagnostic `SELECT` under the configured busy
    /// timeout. requires "Claim/complete transitions MUST
    /// be transactional"; a single conditional `UPDATE` is natively
    /// atomic in SQLite, so no explicit `BEGIN`/`COMMIT` wrapper is
    /// needed here. Same borrow shape as [`Self::post_bounty`].
    pub fn claim_bounty(
        &self,
        bounty_id: i64,
        claimant_player_id: i64,
    ) -> Result<Bounty, BountyError> {
        // Conditional UPDATE: only an `open + still-fresh` row gets
        // transitioned. transitions are exhaustive — any
        // non-matching row falls through to the diagnostic SELECT
        // below for typed-error mapping.
        const UPDATE_SQL: &str = "\
UPDATE bounties \
SET state = 'claimed', \
    claimed_by_player_id = ?2, \
    claimed_at = CURRENT_TIMESTAMP \
WHERE id = ?1 \
  AND state = 'open' \
  AND (expires_at IS NULL OR datetime(expires_at) > datetime('now')) \
RETURNING id, created_at, posted_by_player_id, title, description, \
          reward, state, claimed_by_player_id, claimed_at, \
          completed_at, expires_at";

        match self.connection().query_row(
            UPDATE_SQL,
            rusqlite::params![bounty_id, claimant_player_id],
            row_to_bounty,
        ) {
            Ok(bounty) => Ok(bounty),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(self.diagnose_failed_bounty_transition(bounty_id, BountyState::Claimed))
            }
            Err(source) => Err(BountyError::Sqlite { source }),
        }
    }

    /// Transition a bounty from `claimed` to `completed` and stamp
    /// `completed_at`.
    ///
    /// The contract is "if and only if the row was still `claimed`.
    /// it is now `completed` with a `completed_at` timestamp; the
    /// `reward`, `claimed_by_player_id`, and `claimed_at` columns
    /// are preserved verbatim so the audit view shows who claimed
    /// the bounty, when, and what payout they earned. Any other
    /// current state (still `open`, already `completed`, swept to
    /// `expired`) surfaces as [`BountyError::InvalidTransition`]".
    ///
    ///  says "Game code validates completion evidence"
    /// the kit's responsibility is the transactional state flip.
    /// not the evidence. Game code calls `complete_bounty` after
    /// it has validated whatever the player submitted (a photograph
    /// id, a captured suspect, a delivered item) against its own
    /// bounty schema. Mirrors the split between
    /// [`Self::resolve_challenge`] (kit owns the transition) and
    /// the game-side `result` payload (game owns the schema).
    ///
    /// # No claimant gate
    ///
    /// `complete_bounty` does not take a `completer_player_id` and
    /// does not gate on `claimed_by_player_id`. Game-design choices
    /// like "anyone can submit evidence" vs "only the claimant can
    /// complete" vs "the poster confirms completion" all live in
    /// the calling code's evidence-validation step. Pinning the
    /// claimant inside the kit would force one of those choices on
    /// every consuming game; deliberately leaves it open.
    /// Same convention as [`Self::resolve_challenge`], which doesn't
    /// gate on which side calls it either.
    ///
    /// # Conditional UPDATE shape
    ///
    /// One statement folds the two checks into the WHERE clause so
    /// the success path is a single round-trip:
    ///
    /// 1. `id = ?1` — addresses the row.
    /// 2. `state = 'claimed'` — only the `claimed -> completed`
    ///    transition is legal ; any other current state
    ///    falls through to the diagnostic SELECT for typed-error
    ///    mapping.
    ///
    /// The bookkeeping fields written in the same statement are
    /// `state = 'completed'` and `completed_at = CURRENT_TIMESTAMP`.
    /// `reward`, `claimed_by_player_id`, and `claimed_at` are
    /// deliberately not in the SET list — they were set on earlier
    /// transitions and must survive untouched so the
    /// "reward payload retained" acceptance criterion holds.
    ///
    /// Note this path does NOT gate on `expires_at`: a `claimed`
    /// row already passed the deadline gate at claim time
    /// ([`Self::claim_bounty`]), and the deadline is irrelevant
    /// once the bounty is in flight. The shared
    /// `diagnose_failed_bounty_transition` helper restricts
    /// [`BountyError::Expired`] to `attempted == Claimed` so this
    /// path can never accidentally surface it. (A separate sweeper
    /// path — — will flip lapsed `claimed` rows to
    /// `expired`, after which a future `complete_bounty` against
    /// the same id surfaces `InvalidTransition` from `expired`.)
    ///
    /// # Failure
    ///
    /// - [`BountyError::NotFound`] — no row matches `id`.
    /// - [`BountyError::InvalidTransition`] — row is in any state
    ///   other than `claimed` (still `open`, already `completed`.
    ///   or swept to `expired` by ).
    /// - [`BountyError::Sqlite`] — any other `rusqlite` error.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an
    /// optional diagnostic `SELECT` under the configured busy
    /// timeout. requires "Claim/complete transitions
    /// MUST be transactional"; a single conditional `UPDATE` is
    /// natively atomic in SQLite, so no explicit `BEGIN`/`COMMIT`
    /// wrapper is needed here. Same borrow shape as
    /// [`Self::claim_bounty`].
    pub fn complete_bounty(&self, bounty_id: i64) -> Result<Bounty, BountyError> {
        // Conditional UPDATE: only a `claimed` row gets transitioned.
        // The SET list intentionally excludes `reward`.
        // `claimed_by_player_id`, and `claimed_at` so the
        // "reward payload retained" acceptance and the audit-view
        // attribution survive the flip.
        const UPDATE_SQL: &str = "\
UPDATE bounties \
SET state = 'completed', \
    completed_at = CURRENT_TIMESTAMP \
WHERE id = ?1 \
  AND state = 'claimed' \
RETURNING id, created_at, posted_by_player_id, title, description, \
          reward, state, claimed_by_player_id, claimed_at, \
          completed_at, expires_at";

        match self
            .connection()
            .query_row(UPDATE_SQL, rusqlite::params![bounty_id], row_to_bounty)
        {
            Ok(bounty) => Ok(bounty),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(self.diagnose_failed_bounty_transition(bounty_id, BountyState::Completed))
            }
            Err(source) => Err(BountyError::Sqlite { source }),
        }
    }

    /// Sweep every `open` or `claimed` bounty whose `expires_at` is
    /// non-`NULL` and has lapsed at `now`, flipping it to the
    /// terminal `expired` state and returning the swept rows
    /// .
    ///
    ///  lifts a state machine where `expired` is a terminal
    /// state distinct from `completed`: a bounty whose deadline ran
    /// out without a successful completion is *not* the same as one
    /// that paid out — the audit view must keep them apart so an
    /// operator scanning the board can tell "noisy poster who keeps
    /// missing deadlines" from "successful payout history". The
    /// sweep is the only path that produces an `expired` row; no
    /// player-driven helper transitions to it.
    ///
    /// # Why both `open` and `claimed`
    ///
    /// Unlike [`Self::expire_open_challenges`] (which only sweeps
    /// `open` rows because an accepted challenge has passed the
    /// deadline gate and is in flight), bounty sweep covers BOTH
    /// `open` and `claimed`. The rationale is in
    /// [`BOUNTIES_MIGRATION`]'s `idx_bounties_expiring` doc and
    /// reflects: "a claimant who never completes their
    /// work shouldn't pin the bounty open forever". A bounty board
    /// with rows stuck in `claimed` because the claimant walked away
    /// is worse than one that re-opens to a fresh poster — and the
    /// audit view still preserves who *did* claim it via the
    /// surviving `claimed_by_player_id` `claimed_at` columns.
    /// which the SET list deliberately leaves untouched.
    ///
    /// `completed` and already-`expired` rows are out of scope: the
    /// former is a terminal success the kit must not re-touch (its
    /// `completed_at` audit row must survive verbatim), and the
    /// latter would be a no-op that wastes index walks.
    ///
    /// # `now` parameter
    ///
    /// Same shape as [`Self::expire_open_challenges`]: `now` is an
    /// ISO-8601 timestamp (typically `'YYYY-MM-DDTHH:MM:SSZ'`).
    /// Threading the cutoff through the helper keeps the SQL
    /// identical to the deadline-gate shape used in
    /// [`Self::claim_bounty`] (the same `datetime(expires_at) <=
    /// datetime(?1)` comparison that gate inverts), lets tests pin
    /// the cutoff to a specific instant, and matches 's
    /// call signature shape (`expire_bounties(now)`). Production
    /// callers pass an ISO timestamp synthesised from the runtime
    /// clock at the call site.
    ///
    /// # Conditional UPDATE shape
    ///
    /// The `WHERE` clause folds three checks into a single statement:
    ///
    /// 1. `state IN ('open','claimed')` — terminal states
    ///    (`completed`, `expired`) are never re-transitioned. The
    ///    pair-membership matches the partial-index predicate on
    ///    `idx_bounties_expiring` so the planner can serve the sweep
    ///    from the index.
    /// 2. `expires_at IS NOT NULL` — open-ended bounties with no
    ///    deadline must never be swept. A regression that dropped
    ///    this gate would silently expire every "ongoing" bounty.
    /// 3. `datetime(expires_at) <= datetime(?1)` — the deadline has
    ///    already passed at `now`. The `datetime` wrapping handles
    ///    both ISO forms the kit accepts (`'YYYY-MM-DDTHH:MM:SSZ'`
    ///    from callers, `'YYYY-MM-DD HH:MM:SS'` from
    ///    `CURRENT_TIMESTAMP` SQLite-native columns) so the
    ///    comparison is chronological, not lexicographic — same
    ///    normalisation 's claim gate uses, so a row that
    ///    fails the claim gate is the exact same row this sweep
    ///    catches on the next pass.
    ///
    /// The sweep walks the partial `idx_bounties_expiring` index
    /// landed in [`BOUNTIES_MIGRATION`] (`(expires_at, id) WHERE
    /// state IN ('open','claimed') AND expires_at IS NOT NULL`), so
    /// cost is proportional to the number of expiring open/claimed
    /// rows, not the total bounty count.
    ///
    /// # Preserved columns
    ///
    /// The SET list flips `state` to `'expired'` and *only that*.
    /// `claimed_by_player_id` and `claimed_at` are deliberately
    /// preserved on a sweep that catches a `claimed` row so the
    /// audit view can answer "who held this bounty when it
    /// lapsed" — the same don't-touch-write-once-columns calculus
    /// [`Self::complete_bounty`] uses for its own SET list.
    /// `completed_at` is by definition `NULL` on every swept row
    /// (no `completed` rows make it past the WHERE).
    ///
    /// # Return value
    ///
    /// Returns the swept rows in `RETURNING` order so the caller
    /// can log them, append world events (e.g. a "bounty expired"
    /// bulletin), or render an "expired since last visit"
    /// notification — all without a follow-up `SELECT`. Callers that
    /// only need a count call `.len` on the result. An empty
    /// `Vec` is the success case when nothing was due. Same return
    /// shape as [`Self::expire_open_challenges`].
    ///
    /// # Failure
    ///
    /// - [`BountyError::Sqlite`] — the `UPDATE … RETURNING` failed.
    ///
    /// `NotFound` `InvalidTransition` `Expired` are not in the
    /// failure set: a sweeper that finds nothing is a success, not
    /// an error.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` under the
    /// configured busy timeout. Naturally atomic as a single
    /// statement ( "Claim/complete transitions MUST be
    /// transactional"; the sweep's `open -> expired` /
    /// `claimed -> expired` flips inherit the same guarantee). Same
    /// borrow shape as [`Self::expire_open_challenges`] and the
    /// other transition helpers.
    pub fn expire_bounties(&self, now: &str) -> Result<Vec<Bounty>, BountyError> {
        // Conditional UPDATE: only `open` or `claimed` rows with a
        // non-NULL deadline that has passed at `now`. The three-clause
        // WHERE is documented in the helper rustdoc above; keep this
        // SQL and the doc-list aligned in any future edit.
        const SWEEP_SQL: &str = "\
UPDATE bounties \
SET state = 'expired' \
WHERE state IN ('open','claimed') \
  AND expires_at IS NOT NULL \
  AND datetime(expires_at) <= datetime(?1) \
RETURNING id, created_at, posted_by_player_id, title, description, \
          reward, state, claimed_by_player_id, claimed_at, \
          completed_at, expires_at";

        let mut stmt = self
            .connection()
            .prepare(SWEEP_SQL)
            .map_err(|source| BountyError::Sqlite { source })?;
        let rows = stmt
            .query_map(rusqlite::params![now], row_to_bounty)
            .map_err(|source| BountyError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| BountyError::Sqlite { source })
    }

    /// Map a no-rows response from a bounty-transition `UPDATE` onto
    /// the right typed [`BountyError`] by reading the row's current
    /// state.
    ///
    /// Pulled out of [`Self::claim_bounty`] so the upcoming
    /// `complete_bounty` and `expire_bounties` helpers (Tasks
    /// 7d/7e) can share the same diagnostic. The function does
    /// exactly one SELECT and returns the most specific error
    /// `NotFound` > `Expired` > `InvalidTransition` — without ever
    /// returning `Ok`. Internal-only; not part of the public API.
    /// Same shape as [`crate::challenges`]'s diagnostic helper.
    ///
    /// `Expired` is scoped to `attempted == Claimed` because that
    /// is the only transition today that gates on the deadline;
    /// `complete_bounty` (7d) operates on a row that already passed
    /// the gate at claim time, and `expire_bounties` (7e) is the
    /// thing that *creates* the expired state, not a transition
    /// that gets blocked by it. Pinning the scope here means a
    /// future caller can't accidentally surface `Expired` on a path
    /// where it would be misleading.
    fn diagnose_failed_bounty_transition(
        &self,
        bounty_id: i64,
        attempted: BountyState,
    ) -> BountyError {
        // Compute the deadline check in SQL so it uses the same
        // `datetime` normalisation as the UPDATE — a mismatch
        // here would let the diagnostic disagree with the gate that
        // produced the no-rows in the first place.
        const DIAG_SQL: &str = "\
SELECT state, \
       CASE \
           WHEN expires_at IS NOT NULL \
               AND datetime(expires_at) <= datetime('now') \
           THEN 1 ELSE 0 \
       END AS deadline_lapsed \
FROM bounties WHERE id = ?1";

        let row = self
            .connection()
            .query_row(DIAG_SQL, rusqlite::params![bounty_id], |row| {
                let state: String = row.get(0)?;
                let deadline_lapsed: i64 = row.get(1)?;
                Ok((state, deadline_lapsed != 0))
            });

        match row {
            Err(rusqlite::Error::QueryReturnedNoRows) => BountyError::NotFound { id: bounty_id },
            Err(source) => BountyError::Sqlite { source },
            // Row still open but deadline already lapsed — the
            // sweeper hasn't run, but the helper refused to claim.
            // Only meaningful when `attempted == Claimed`, the only
            // transition that gates on the deadline today.
            Ok((state, true))
                if state == BountyState::Open.as_str() && attempted == BountyState::Claimed =>
            {
                BountyError::Expired { id: bounty_id }
            }
            Ok((state, _)) => BountyError::InvalidTransition {
                id: bounty_id,
                from: state,
                to: attempted,
            },
        }
    }
}

/// Decode one `bounties` row into a [`Bounty`].
///
/// Pulled out so the write path and the upcoming Task
/// 7c–7e transition query paths can share one decoder. Column
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

    ///   acceptance: applying [`BOUNTIES_MIGRATION`]
    /// creates the documented `bounties` table with the column shape
    ///  pins. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped `CREATE TABLE` from the migration body
    ///    would flunk).
    /// 2. The columns and order match the contract (so a
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
            "bounties column shape must match the contract"
        );
    }

    ///  implies the bounty state machine is
    /// `open -> claimed -> completed`, plus `open -> expired` and
    /// `claimed -> expired` ( flip the columns). The
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
            "CHECK constraint must reject states outside the vocabulary"
        );
    }

    /// The helpers walk three partial indexes the
    /// migration creates up-front. If any of them ever stops being
    /// created, the read silently becomes a full table scan in
    /// production. Pin every index name plus its partial predicate
    /// so a regression flunks at `cargo test` rather than under
    /// load. Same rationale as
    /// `challenges_partial_indexes_are_present`.
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
        // expiry — a claimant who never completes
        // their work shouldn't pin the bounty open forever. Pin
        // both states in the predicate so a regression that
        // narrowed the index to one state flunks here.
        assert!(
            expiring_sql.contains("'open'") && expiring_sql.contains("'claimed'"),
            "idx_bounties_expiring must cover both 'open' and 'claimed'; got: {expiring_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; inherits that contract.
    /// A second `apply_migration(&BOUNTIES_MIGRATION)` MUST be a
    /// no-op (the version is already in `world_migrations`), not
    /// an error from `CREATE TABLE` on an existing table. Same
    /// shape as `challenges_migration_is_idempotent`.
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
    /// `notices::tests::world_with_notices`.
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

    ///   acceptance: a valid `post_bounty` round-
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
        // The "starts open" acceptance criterion. Pinned
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

    ///  explicitly allows "posted_by player id optional"
    /// — system NPC-organisation bounties (e.g. the city offering
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

    ///  lists `title` as required. The kit additionally
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

    ///  lists `description` as required. Empty
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

    ///  lists `reward JSON` without an "optional" modifier
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

    /// Variant of [`world_with_bounties`] that pre-creates a poster
    /// and two distinct claimant players, returning their player ids.
    /// Used by the claim-bounty tests that need at least
    /// two attributable claimants (the second-claimant-rejected case).
    fn world_with_poster_and_two_claimants() -> (tempfile::TempDir, WorldDb, i64, i64, i64) {
        let (dir, world, poster_id) = world_with_poster();
        let alice_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-alice', 'alice') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert alice");
        let bob_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-bob', 'bob') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert bob");
        (dir, world, poster_id, alice_id, bob_id)
    }

    ///   acceptance, half 1: a fresh `open` bounty
    /// transitions to `claimed` on a successful claim. Pin the full
    /// post-conditions in one place so a regression in any of them
    /// flunks here:
    ///
    /// - `state` flips from `'open'` to `'claimed'`.
    /// - `claimed_by_player_id` is set to the caller's id (not
    ///   silently NULL or set to the poster).
    /// - `claimed_at` is stamped (not left NULL).
    /// - `completed_at` stays `NULL` — claim is a one-step
    ///   transition, not a "stamp everything" shortcut.
    /// - The post-claim row is *durable* — re-reading by primary key
    ///   matches the `RETURNING` echo, guarding against a regression
    ///   that returned a phantom row from `RETURNING` without
    ///   committing.
    #[test]
    fn claim_bounty_transitions_open_to_claimed() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Bring photographic evidence.",
                r#"{"credits":250}"#,
                None,
            )
            .expect("post_bounty succeeds");

        let claimed = world
            .claim_bounty(posted.id, alice_id)
            .expect("claim_bounty succeeds on a fresh open bounty");

        assert_eq!(
            claimed.state,
            BountyState::Claimed.as_str(),
            "open bounty must transition to 'claimed'"
        );
        assert_eq!(
            claimed.claimed_by_player_id,
            Some(alice_id),
            "claim_bounty must attribute the claimant"
        );
        assert!(
            claimed.claimed_at.is_some(),
            "claim_bounty must stamp claimed_at"
        );
        assert!(
            claimed.completed_at.is_none(),
            "claim_bounty must not stamp completed_at"
        );
        // Other fields must be preserved verbatim — the claim
        // transition only touches state, claimed_by, and
        // claimed_at; a regression that nulled out title/reward
        // mid-UPDATE would flunk here.
        assert_eq!(claimed.id, posted.id);
        assert_eq!(claimed.title, posted.title);
        assert_eq!(claimed.description, posted.description);
        assert_eq!(claimed.reward, posted.reward);
        assert_eq!(claimed.posted_by_player_id, posted.posted_by_player_id);
        assert_eq!(claimed.created_at, posted.created_at);

        // Durability: re-read by primary key and confirm the row
        // matches the `RETURNING` echo. Catches a regression where
        // `RETURNING` echoed an in-flight UPDATE that never
        // committed.
        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, posted_by_player_id, title, description, \
                        reward, state, claimed_by_player_id, claimed_at, \
                        completed_at, expires_at \
                 FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                row_to_bounty,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, claimed, "stored row must equal RETURNING row");
    }

    ///   acceptance, half 2: a second claimant must
    /// be rejected. The conditional UPDATE with `WHERE state =
    /// 'open'` ensures only the first caller wins the race; the
    /// loser surfaces as `InvalidTransition { from: "claimed".
    /// to: Claimed }`. Pin three things to lock the contract:
    ///
    /// - The typed error category (so a future regression that
    ///   silently swallowed the second claim's loss flunks here).
    /// - The `from` state shows the *winner's* state ("claimed").
    ///   not the loser's intent — proves the diagnostic SELECT
    ///   reads the post-flip row, not a stale snapshot.
    /// - The original claimant's id and `claimed_at` are unchanged
    ///   after the failed second claim — proves the UPDATE failed
    ///   atomically with no partial overwrite of the winner's
    ///   attribution.
    #[test]
    fn claim_bounty_rejects_second_claimant() {
        let (_dir, world, poster_id, alice_id, bob_id) = world_with_poster_and_two_claimants();
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Bring photographic evidence.",
                r#"{"credits":250}"#,
                None,
            )
            .expect("post_bounty succeeds");

        let first = world
            .claim_bounty(posted.id, alice_id)
            .expect("first claim succeeds");
        let err = world
            .claim_bounty(posted.id, bob_id)
            .expect_err("second claim must fail");
        assert!(
            matches!(
                &err,
                BountyError::InvalidTransition { id, from, to }
                    if *id == posted.id && from == "claimed" && *to == BountyState::Claimed
            ),
            "expected InvalidTransition from 'claimed' to Claimed, got {err:?}"
        );

        // The original claimant's attribution and timestamp must
        // be unchanged after the failed second claim. Catches a
        // regression where the conditional UPDATE wrote
        // `claimed_by_player_id = ?2` even though the WHERE clause
        // matched zero rows (a defensive misread of how RETURNING
        // interacts with no-rows UPDATEs).
        let (durable_claimant, durable_claimed_at): (Option<i64>, Option<String>) = world
            .connection()
            .query_row(
                "SELECT claimed_by_player_id, claimed_at \
                 FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("durable lookup");
        assert_eq!(
            durable_claimant,
            Some(alice_id),
            "rejected second claim must not overwrite winner's id"
        );
        assert_eq!(
            durable_claimed_at, first.claimed_at,
            "rejected second claim must not advance claimed_at"
        );
    }

    ///  makes deadlined bounties uncl­aimable past their
    /// `expires_at` — even before the sweeper runs. An
    /// `open` row whose deadline is already in the past must
    /// surface [`BountyError::Expired`] **and** leave the row
    /// untouched. We pin both halves: the typed error AND the row-
    /// unchanged invariant. Without the second half, a regression
    /// that flipped the order of the `WHERE` checks could let a
    /// stale bounty slip through with a "fail" return value while
    /// still mutating the row. Same shape as
    /// `accept_challenge_rejects_expired`.
    #[test]
    fn claim_bounty_rejects_expired_bounty() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        // A deadline well in the past so the `datetime` comparison
        // in claim_bounty rejects regardless of wall-clock skew.
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Stale bounty",
                "Already expired.",
                r#"{"credits":1}"#,
                Some("2000-01-01T00:00:00Z"),
            )
            .expect("post_bounty succeeds");

        let err = world
            .claim_bounty(posted.id, alice_id)
            .expect_err("claiming an expired bounty must fail");
        assert!(
            matches!(err, BountyError::Expired { id } if id == posted.id),
            "expected Expired {{ id: {} }}, got {err:?}",
            posted.id
        );

        // Row-unchanged invariant: state still 'open'.
        // claimed_by_player_id still NULL, claimed_at still NULL.
        // Pinning all three so a regression that wrote claimant
        // info before checking the deadline flunks here.
        let (state, claimed_by, claimed_at): (String, Option<i64>, Option<String>) = world
            .connection()
            .query_row(
                "SELECT state, claimed_by_player_id, claimed_at \
                 FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("durable lookup");
        assert_eq!(state, "open", "expired-but-unswept row must stay 'open'");
        assert!(
            claimed_by.is_none(),
            "rejected claim must leave claimed_by_player_id NULL"
        );
        assert!(
            claimed_at.is_none(),
            "rejected claim must leave claimed_at NULL"
        );
    }

    /// A stale id (bounty was deleted, or the caller fabricated
    /// one) must surface [`BountyError::NotFound`] rather than a
    /// generic SQL error or a misleading `InvalidTransition`. Same
    /// shape as `accept_challenge_missing_id_returns_not_found`.
    #[test]
    fn claim_bounty_missing_id_returns_not_found() {
        let (_dir, world, _poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let err = world
            .claim_bounty(424_242, alice_id)
            .expect_err("claim against a missing id must fail");
        assert!(
            matches!(err, BountyError::NotFound { id } if id == 424_242),
            "expected NotFound {{ id: 424242 }}, got {err:?}"
        );
    }

    /// An open-ended bounty (no `expires_at`) is always claimable
    /// while it remains in `open`. Pin this so a regression that
    /// inverted the deadline check ("reject when `expires_at IS
    /// NULL`") flunks at `cargo test` rather than as a confused
    /// production report.
    #[test]
    fn claim_bounty_succeeds_for_open_ended_bounty() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Open-ended bounty",
                "No deadline.",
                r#"{"credits":1}"#,
                None,
            )
            .expect("post_bounty succeeds");

        let claimed = world
            .claim_bounty(posted.id, alice_id)
            .expect("open-ended bounty must be claimable");
        assert_eq!(claimed.state, "claimed");
        assert_eq!(claimed.claimed_by_player_id, Some(alice_id));
        assert!(claimed.expires_at.is_none());
    }

    ///   acceptance, half 1: a `claimed` bounty
    /// transitions to `completed` and stamps `completed_at`. Pin
    /// the full post-conditions in one place so a regression in
    /// any of them flunks here:
    ///
    /// - `state` flips from `'claimed'` to `'completed'`.
    /// - `completed_at` is stamped (not left NULL).
    /// - The post-completion row is *durable* — re-reading by
    ///   primary key matches the `RETURNING` echo, guarding
    ///   against a regression that returned a phantom row from
    ///   `RETURNING` without committing.
    #[test]
    fn complete_bounty_transitions_claimed_to_completed() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Bring photographic evidence.",
                r#"{"credits":250}"#,
                None,
            )
            .expect("post_bounty succeeds");
        let claimed = world
            .claim_bounty(posted.id, alice_id)
            .expect("claim_bounty succeeds");

        let completed = world
            .complete_bounty(posted.id)
            .expect("complete_bounty succeeds on a claimed bounty");

        assert_eq!(
            completed.state,
            BountyState::Completed.as_str(),
            "claimed bounty must transition to 'completed'"
        );
        assert_eq!(completed.state, "completed");
        assert!(
            completed.completed_at.is_some(),
            "complete_bounty must stamp completed_at"
        );

        // Durability: re-read by primary key and confirm the row
        // matches the RETURNING echo. Catches a regression where
        // RETURNING echoed an in-flight UPDATE that never
        // committed.
        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, posted_by_player_id, title, description, \
                        reward, state, claimed_by_player_id, claimed_at, \
                        completed_at, expires_at \
                 FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                row_to_bounty,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, completed, "stored row must equal RETURNING row");
        // Sanity: the durable row's claim attribution must match
        // what `claim_bounty` originally wrote — pinning the audit
        // chain end-to-end.
        assert_eq!(stored.claimed_by_player_id, claimed.claimed_by_player_id);
        assert_eq!(stored.claimed_at, claimed.claimed_at);
    }

    ///   acceptance, half 2: the **reward payload
    /// is retained** through completion. Same applies to every
    /// other write-once column (`title`, `description`.
    /// `posted_by_player_id`, `created_at`, `expires_at`) and the
    /// claim-time bookkeeping (`claimed_by_player_id`.
    /// `claimed_at`). A regression that broadened the SET list
    /// would null out one of these mid-UPDATE; pin every survivor
    /// here so that flunks at `cargo test`. Without this pin the
    /// only test on the path is the state flip, and a "completed
    /// but reward NULL" row is the worst kind of corruption:
    /// payable but empty.
    #[test]
    fn complete_bounty_retains_reward_and_claim_attribution() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        // A realistic reward payload that exercises a few JSON
        // shapes — credits, items, faction reputation — so a
        // regression that re-serialised the column would corrupt
        // visibly.
        let reward_json = r#"{"credits":250,"items":["pocketwatch"],"rep":{"blue_desk":3}}"#;
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Bring photographic evidence.",
                reward_json,
                Some("2099-01-01T00:00:00Z"),
            )
            .expect("post_bounty succeeds");
        let claimed = world
            .claim_bounty(posted.id, alice_id)
            .expect("claim_bounty succeeds");

        let completed = world
            .complete_bounty(posted.id)
            .expect("complete_bounty succeeds");

        // The "reward payload retained" acceptance
        // verbatim, byte-for-byte. Pinning equality on the JSON
        // string (not a parsed shape) catches a regression that
        // re-encoded the value through a JSON round-trip and lost
        // key ordering or whitespace.
        assert_eq!(
            completed.reward, reward_json,
            "reward must be preserved verbatim through completion"
        );
        // Every other write-once claim-time column must survive.
        assert_eq!(completed.id, posted.id);
        assert_eq!(completed.created_at, posted.created_at);
        assert_eq!(completed.posted_by_player_id, posted.posted_by_player_id);
        assert_eq!(completed.title, posted.title);
        assert_eq!(completed.description, posted.description);
        assert_eq!(completed.expires_at, posted.expires_at);
        assert_eq!(
            completed.claimed_by_player_id,
            Some(alice_id),
            "claim attribution must survive completion"
        );
        assert_eq!(
            completed.claimed_at, claimed.claimed_at,
            "claimed_at timestamp must survive completion"
        );
    }

    /// Completing a bounty that is still `open` (never claimed)
    /// must surface `InvalidTransition { from: "open", to:
    /// Completed }` rather than silently flipping or surfacing a
    /// confusing `NotFound`. The lifecycle is
    /// `open -> claimed -> completed`; skipping the claim step is
    /// not a legal transition.
    #[test]
    fn complete_bounty_rejects_unclaimed_open_bounty() {
        let (_dir, world, poster_id) = world_with_poster();
        let posted = world
            .post_bounty(
                Some(poster_id),
                "Find the missing pocketwatch",
                "Bring photographic evidence.",
                r#"{"credits":250}"#,
                None,
            )
            .expect("post_bounty succeeds");

        let err = world
            .complete_bounty(posted.id)
            .expect_err("completing an unclaimed bounty must fail");
        assert!(
            matches!(
                &err,
                BountyError::InvalidTransition { id, from, to }
                    if *id == posted.id && from == "open" && *to == BountyState::Completed
            ),
            "expected InvalidTransition from 'open' to Completed, got {err:?}"
        );

        // Row-unchanged invariant: state still 'open'.
        // completed_at still NULL. Catches a regression that
        // wrote completed_at before checking state.
        let (state, completed_at): (String, Option<String>) = world
            .connection()
            .query_row(
                "SELECT state, completed_at FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("durable lookup");
        assert_eq!(state, "open");
        assert!(completed_at.is_none());
    }

    /// Completing a bounty that has already been completed must be
    /// rejected with `InvalidTransition { from: "completed", to:
    /// Completed }`. makes `completed` a terminal state.
    /// Pinning this guards against a regression that loosened the
    /// `state = 'claimed'` predicate to permit a completion
    /// re-stamp (which would also re-stamp `completed_at`.
    /// corrupting the audit timeline).
    #[test]
    fn complete_bounty_rejects_already_completed_bounty() {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let posted = world
            .post_bounty(Some(poster_id), "title", "desc", r#"{"credits":1}"#, None)
            .expect("post_bounty succeeds");
        world
            .claim_bounty(posted.id, alice_id)
            .expect("claim_bounty succeeds");
        let first = world
            .complete_bounty(posted.id)
            .expect("first complete_bounty succeeds");

        let err = world
            .complete_bounty(posted.id)
            .expect_err("second complete_bounty must fail");
        assert!(
            matches!(
                &err,
                BountyError::InvalidTransition { id, from, to }
                    if *id == posted.id && from == "completed" && *to == BountyState::Completed
            ),
            "expected InvalidTransition from 'completed' to Completed, got {err:?}"
        );

        // The original completed_at timestamp must NOT have been
        // overwritten by the rejected re-completion. Pin this
        // explicitly: a regression that stamped completed_at
        // before checking state would silently advance the audit
        // timeline on every duplicate call.
        let durable_completed_at: Option<String> = world
            .connection()
            .query_row(
                "SELECT completed_at FROM bounties WHERE id = ?1",
                rusqlite::params![posted.id],
                |row| row.get(0),
            )
            .expect("durable lookup");
        assert_eq!(
            durable_completed_at, first.completed_at,
            "rejected re-completion must not advance completed_at"
        );
    }

    /// A stale id (bounty was deleted, or the caller fabricated
    /// one) must surface [`BountyError::NotFound`] rather than a
    /// generic SQL error or a misleading `InvalidTransition`.
    /// Same shape as `claim_bounty_missing_id_returns_not_found`
    /// — the equivalent.
    #[test]
    fn complete_bounty_missing_id_returns_not_found() {
        let (_dir, world, _poster_id, _alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let err = world
            .complete_bounty(424_242)
            .expect_err("complete against a missing id must fail");
        assert!(
            matches!(err, BountyError::NotFound { id } if id == 424_242),
            "expected NotFound {{ id: 424242 }}, got {err:?}"
        );
    }

    ///   acceptance, central case: only `open` and
    /// `claimed` rows whose `expires_at` has lapsed at `now` are
    /// swept. Pin every cell of the policy matrix in one place so
    /// a regression in any of them flunks here:
    ///
    /// - open + past deadline → swept (the basic case).
    /// - claimed + past deadline → swept (the bounty-specific
    ///   contract, the documented "claimant who never completes
    ///   their work shouldn't pin the bounty open forever" rule).
    /// - open + future deadline → stays open.
    /// - claimed + future deadline → stays claimed.
    /// - open + no deadline → stays open (the `expires_at IS NOT
    ///   NULL` gate).
    /// - completed → must not be re-touched (terminal success).
    /// - already-expired → must not be re-touched (idempotency).
    ///
    /// A regression that dropped the `state IN ('open','claimed')`
    /// gate would still expire the right rows but additionally
    /// re-stamp completed/expired ones — caught here by the
    /// per-row durable-state assertion below. Same shape as
    /// `expire_open_challenges_only_sweeps_due_open_rows`.
    #[test]
    fn expire_bounties_only_sweeps_due_open_or_claimed_rows() {
        let (_dir, world, poster_id, alice_id, bob_id) = world_with_poster_and_two_claimants();

        // Case 1: open + past deadline → SHOULD be swept.
        let due_open = world
            .post_bounty(
                Some(poster_id),
                "Stale clue bounty",
                "Already lapsed when the sweep runs.",
                r#"{"credits":50}"#,
                Some("2000-01-01T00:00:00Z"),
            )
            .unwrap();
        // Case 2: claimed + past deadline → SHOULD be swept. Built
        // by posting with a future deadline (so `claim_bounty`'s
        // deadline gate accepts it), claiming, then back-dating the
        // deadline to a past instant via raw UPDATE. This is the
        // production analog of "bounty was claimed in time but the
        // claimant walked away before the deadline lapsed".
        let due_claimed = world
            .post_bounty(
                Some(poster_id),
                "Abandoned suspect tail",
                "Claimed in time, never completed.",
                r#"{"credits":75}"#,
                Some("2999-12-31T23:59:59Z"),
            )
            .unwrap();
        let due_claimed = world.claim_bounty(due_claimed.id, alice_id).unwrap();
        world
            .connection()
            .execute(
                "UPDATE bounties SET expires_at = ?1 WHERE id = ?2",
                rusqlite::params!["2000-01-02T00:00:00Z", due_claimed.id],
            )
            .unwrap();
        // Case 3: open + future deadline → MUST stay open.
        let future_open = world
            .post_bounty(
                Some(poster_id),
                "Future job",
                "Plenty of time left.",
                r#"{"credits":100}"#,
                Some("2999-12-31T23:59:59Z"),
            )
            .unwrap();
        // Case 4: claimed + future deadline → MUST stay claimed.
        let future_claimed = world
            .post_bounty(
                Some(poster_id),
                "Active investigation",
                "Bob is on it.",
                r#"{"credits":150}"#,
                Some("2999-12-31T23:59:59Z"),
            )
            .unwrap();
        let future_claimed = world.claim_bounty(future_claimed.id, bob_id).unwrap();
        // Case 5: open + no deadline → MUST stay open. The
        // `expires_at IS NOT NULL` gate exists for exactly this case.
        let openended = world
            .post_bounty(
                Some(poster_id),
                "Standing offer",
                "No deadline; ongoing reward.",
                r#"{"credits":25}"#,
                None,
            )
            .unwrap();
        // Case 6: completed → MUST stay completed. Built by post →
        // claim → complete; the row's `completed_at` audit field
        // must survive a sweep verbatim.
        let completed = world
            .post_bounty(
                Some(poster_id),
                "Already paid out",
                "Successfully closed earlier in the day.",
                r#"{"credits":200}"#,
                Some("2999-12-31T23:59:59Z"),
            )
            .unwrap();
        let completed = world.claim_bounty(completed.id, alice_id).unwrap();
        let completed = world.complete_bounty(completed.id).unwrap();
        // Case 7: already-expired → MUST stay expired (idempotency
        // against a pre-swept row). Built by raw INSERT because the
        // public API has no "post a directly-expired bounty" path.
        let already_expired_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO bounties \
                 (posted_by_player_id, title, description, reward, state, expires_at) \
                 VALUES (?1, 'Long gone', 'Swept on a prior pass.', \
                         '{\"credits\":10}', 'expired', '2000-01-03T00:00:00Z') \
                 RETURNING id",
                rusqlite::params![poster_id],
                |row| row.get(0),
            )
            .unwrap();

        // Sweep at a `now` after Cases 1/2's deadlines but well
        // before Cases 3/4's. Cases 5/6/7 are out of scope by state
        // or by the NULL deadline.
        let swept = world
            .expire_bounties("2026-01-01T00:00:00Z")
            .expect("sweep succeeds");

        // Only the two due rows (in expires_at order) should appear.
        let mut swept_ids: Vec<i64> = swept.iter().map(|b| b.id).collect();
        swept_ids.sort();
        let mut expected = vec![due_open.id, due_claimed.id];
        expected.sort();
        assert_eq!(swept_ids, expected, "only the two due rows must be swept");
        for row in &swept {
            assert_eq!(
                row.state,
                BountyState::Expired.as_str(),
                "every swept row's RETURNING state must be 'expired'"
            );
        }
        // The previously-claimed row's audit attribution MUST
        // survive the sweep — this is the contract the SET list
        // bakes in (state-only flip).
        let swept_claimed = swept.iter().find(|b| b.id == due_claimed.id).unwrap();
        assert_eq!(
            swept_claimed.claimed_by_player_id,
            Some(alice_id),
            "swept claimed row must preserve claimed_by_player_id"
        );
        assert!(
            swept_claimed.claimed_at.is_some(),
            "swept claimed row must preserve claimed_at"
        );

        // Per-row durable invariant: re-read by primary key and
        // confirm each row landed in the expected state.
        let state_of = |id: i64| -> String {
            world
                .connection()
                .query_row(
                    "SELECT state FROM bounties WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        assert_eq!(state_of(due_open.id), "expired");
        assert_eq!(state_of(due_claimed.id), "expired");
        assert_eq!(state_of(future_open.id), "open");
        assert_eq!(state_of(future_claimed.id), "claimed");
        assert_eq!(state_of(openended.id), "open");
        assert_eq!(state_of(completed.id), "completed");
        assert_eq!(state_of(already_expired_id), "expired");

        // The completed row's `completed_at` must not have been
        // wiped by an over-broad sweep — pins the SET-list contract
        // against a regression that started writing across all
        // lifecycle columns.
        let completed_at_after: Option<String> = world
            .connection()
            .query_row(
                "SELECT completed_at FROM bounties WHERE id = ?1",
                rusqlite::params![completed.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            completed_at_after, completed.completed_at,
            "completed_at on the untouched row must survive the sweep verbatim"
        );
    }

    /// A second sweep at the same `now` is a no-op: the previously
    /// swept rows are now `expired` and the
    /// `state IN ('open','claimed')` gate excludes them, while
    /// every still-fresh row's deadline is still in the future.
    /// Pin idempotency so a regression that (say) dropped the state
    /// gate and started re-stamping the row on every pass would
    /// observably flunk on `is_empty`. Same shape as
    /// `expire_open_challenges_is_idempotent`.
    #[test]
    fn expire_bounties_is_idempotent() {
        let (_dir, world, poster_id, _alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let due = world
            .post_bounty(
                Some(poster_id),
                "Stale",
                "Past its deadline.",
                r#"{"credits":1}"#,
                Some("2000-01-01T00:00:00Z"),
            )
            .unwrap();

        let first = world.expire_bounties("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(first.len(), 1, "first sweep expires the due row");
        assert_eq!(first[0].id, due.id);

        let second = world.expire_bounties("2026-01-01T00:00:00Z").unwrap();
        assert!(
            second.is_empty(),
            "second sweep at same now must be a no-op, got {second:?}"
        );
    }

    /// An empty `bounties` table (or a table whose only rows are
    /// not due) returns an empty `Vec`, not an error. The
    /// sweeper contract treats "nothing to do" as a success — a
    /// regression that surfaced this as `Sqlite { … }` would force
    /// every caller to special-case it. Same shape as
    /// `expire_open_challenges_empty_table_returns_empty_vec`.
    #[test]
    fn expire_bounties_empty_table_returns_empty_vec() {
        let (_dir, world) = world_with_bounties();
        let swept = world
            .expire_bounties("2026-01-01T00:00:00Z")
            .expect("sweep on empty table succeeds");
        assert!(
            swept.is_empty(),
            "sweep with no rows in scope must return Vec::new(), got {swept:?}"
        );
    }

    /// The cutoff is `<= now`, not `< now`: a deadline that exactly
    /// equals `now` MUST sweep. Without this assertion, a future
    /// regression that flipped the comparator to strict `<` would
    /// leave on-the-second deadlines stuck until the next sweep
    /// tick. Using identical text on both sides also pins that the
    /// `datetime` normalisation is the same on both sides of the
    /// comparison. Same shape as
    /// `expire_open_challenges_includes_exact_deadline`.
    #[test]
    fn expire_bounties_includes_exact_deadline() {
        let (_dir, world, poster_id) = world_with_poster();
        let exact = world
            .post_bounty(
                Some(poster_id),
                "Exact",
                "Deadline equals sweep now.",
                r#"{"credits":1}"#,
                Some("2026-01-01T00:00:00Z"),
            )
            .unwrap();

        let swept = world.expire_bounties("2026-01-01T00:00:00Z").unwrap();
        let swept_ids: Vec<i64> = swept.iter().map(|b| b.id).collect();
        assert_eq!(
            swept_ids,
            vec![exact.id],
            "an on-the-second deadline MUST be swept (<= comparator)"
        );
    }

    // -- — invalid-transition table tests --------------
    //
    // Standalone tests above already pin specific scenarios with extra
    // invariants (claim attribution, reward-payload preservation.
    // durability round-trips, deadline-equality sweep). The three
    // `rstest` tables below complete the matrix: one row per (helper.
    // starting-state) pair, asserting the *exact* outcome — `Ok` on
    // each legal edge, typed `InvalidTransition { from }`
    // everywhere else (and silent non-inclusion for the sweeper).
    // Without this exhaustive grid, a regression that (say) relaxed
    // `complete_bounty`'s gate to also accept an `open` row could pass
    // every existing test by coincidence; with the grid, that
    // regression is one failing case.
    //
    // The starting state is encoded as the schema-side `state` string
    // (not the `BountyState` enum) so a future state added in code
    // without a matching test row surfaces as a missing case rather
    // than as a silent compile-time mapping. The expected `from`
    // value is the same string, which mirrors how
    // `diagnose_failed_bounty_transition` actually populates the
    // error.
    //
    // Same shape as `challenges::tests::accept_challenge_table` etc.
    //
    // Notes on edges this table deliberately does *not* cover:
    //
    // * `claim_bounty` on an `open` row whose deadline has lapsed
    //   surfaces `Expired { id }`, not `InvalidTransition`. That edge
    //   is pinned by `claim_bounty_rejects_expired_bounty` above; the
    //   7f grid is concerned with state pairs, so every "open" row
    //   here is built without a deadline.
    // * `expire_bounties` is exercised in its own table below because
    //   it operates on a set of rows (no addressed `id`) and its
    //   "invalid transition" is a non-sweep, not a typed error.

    /// Seed a single `bounties` row already in the requested state
    /// using a raw `INSERT`. Building claimed/completed/expired rows
    /// by walking the public helpers chains the success paths under
    /// test (a claim → complete test would actually be testing *two*
    /// helpers), so the table tests use a raw insert to isolate one
    /// helper at a time. The schema-level `CHECK` constraint and FKs
    /// still apply.
    ///
    /// `claimant` is the `claimed_by_player_id` to attribute on
    /// `claimed`/`completed`/`expired` rows; ignored for `open`
    /// (which the schema disallows from carrying a claimant before
    /// the transition stamps it). Returns the new row id.
    fn seed_bounty_in_state(world: &WorldDb, poster: i64, claimant: i64, state: &str) -> i64 {
        // Each non-open state needs the audit fields the
        // lifecycle would have stamped; we synthesise plausible
        // values (real ISO timestamps, attributed claimant) so a
        // future test that primary-key-SELECTs the row also sees a
        // production-shaped record.
        let (claimed_by, claimed_at, completed_at, expires_at) = match state {
            "open" => (None, None, None, None),
            "claimed" => (Some(claimant), Some("2026-01-01T00:00:00Z"), None, None),
            "completed" => (
                Some(claimant),
                Some("2026-01-01T00:00:00Z"),
                Some("2026-01-01T00:01:00Z"),
                None,
            ),
            // An expired row was previously open or claimed with a
            // past deadline. The deadline string lets a regression
            // that re-checks `expires_at` on claim/complete still
            // exercise the lapse branch on the swept row.
            "expired" => (None, None, None, Some("2000-01-01T00:00:00Z")),
            other => panic!("unknown seed state {other:?}"),
        };
        world
            .connection()
            .query_row(
                "INSERT INTO bounties \
                 (posted_by_player_id, title, description, reward, state, \
                  claimed_by_player_id, claimed_at, completed_at, expires_at) \
                 VALUES (?1, 'Seed', 'Seeded row.', '{\"credits\":1}', ?2, \
                         ?3, ?4, ?5, ?6) \
                 RETURNING id",
                rusqlite::params![
                    poster,
                    state,
                    claimed_by,
                    claimed_at,
                    completed_at,
                    expires_at,
                ],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|err| panic!("seed {state:?} row: {err}"))
    }

    ///   acceptance — `claim_bounty` table.
    ///
    /// `open` is the only legal starting state; every other state
    /// MUST surface as `InvalidTransition { from: <state> }` with
    /// `to: BountyState::Claimed`. The `from` field's exact string
    /// is asserted so a regression that mapped the schema state
    /// through a lossy enum conversion would observably flunk.
    #[rstest::rstest]
    #[case::open_is_legal("open")]
    #[case::claimed_is_invalid("claimed")]
    #[case::completed_is_invalid("completed")]
    #[case::expired_is_invalid("expired")]
    fn claim_bounty_table(#[case] from: &str) {
        let (_dir, world, poster_id, alice_id, bob_id) = world_with_poster_and_two_claimants();
        let id = seed_bounty_in_state(&world, poster_id, alice_id, from);

        let outcome = world.claim_bounty(id, bob_id);
        if from == "open" {
            let bounty = outcome.expect("open -> claimed is legal");
            assert_eq!(
                bounty.state,
                BountyState::Claimed.as_str(),
                "open -> claimed must land state='claimed'"
            );
            assert_eq!(
                bounty.claimed_by_player_id,
                Some(bob_id),
                "claim must attribute the caller"
            );
            assert!(bounty.claimed_at.is_some(), "claim must stamp claimed_at");
        } else {
            match outcome.expect_err("non-open MUST NOT claim") {
                BountyError::InvalidTransition {
                    id: err_id,
                    from: err_from,
                    to,
                } => {
                    assert_eq!(err_id, id);
                    assert_eq!(
                        err_from, from,
                        "InvalidTransition.from must echo schema state"
                    );
                    assert_eq!(to, BountyState::Claimed);
                }
                other => panic!("expected InvalidTransition for from={from:?}, got {other:?}"),
            }
            // Row state must be unchanged — a regression that wrote
            // `claimed_by_player_id = ?2` before checking the gate
            // would observably flunk this assertion.
            let (stored_state, stored_claimant): (String, Option<i64>) = world
                .connection()
                .query_row(
                    "SELECT state, claimed_by_player_id FROM bounties WHERE id = ?1",
                    rusqlite::params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(stored_state, from, "rejected claim must not mutate state");
            // For seeded `claimed`/`completed` rows the original
            // claimant was `alice_id`; a regression that wrote
            // `bob_id` over the top would flunk here.
            if from == "claimed" || from == "completed" {
                assert_eq!(
                    stored_claimant,
                    Some(alice_id),
                    "rejected claim must not overwrite the existing claimant"
                );
            }
        }
    }

    ///   acceptance — `complete_bounty` table.
    ///
    /// `claimed` is the only legal starting state. The kit does not
    /// gate completion on a deadline (a `claimed` row already passed
    /// the deadline check at claim time, per `complete_bounty`'s
    /// docs), so unlike `claim` there is no `Expired` outcome to
    /// consider here.
    #[rstest::rstest]
    #[case::open_is_invalid("open")]
    #[case::claimed_is_legal("claimed")]
    #[case::completed_is_invalid("completed")]
    #[case::expired_is_invalid("expired")]
    fn complete_bounty_table(#[case] from: &str) {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();
        let id = seed_bounty_in_state(&world, poster_id, alice_id, from);

        let outcome = world.complete_bounty(id);
        if from == "claimed" {
            let bounty = outcome.expect("claimed -> completed is legal");
            assert_eq!(
                bounty.state,
                BountyState::Completed.as_str(),
                "claimed -> completed must land state='completed'"
            );
            assert!(
                bounty.completed_at.is_some(),
                "complete must stamp completed_at"
            );
            // The reward payload and claim attribution MUST survive
            // — pinned in detail by
            // `complete_bounty_retains_reward_and_claim_attribution`.
            // sanity-checked here so the table catches a regression
            // that broadened the SET list.
            assert_eq!(bounty.reward, r#"{"credits":1}"#);
            assert_eq!(bounty.claimed_by_player_id, Some(alice_id));
        } else {
            match outcome.expect_err("non-claimed MUST NOT complete") {
                BountyError::InvalidTransition {
                    id: err_id,
                    from: err_from,
                    to,
                } => {
                    assert_eq!(err_id, id);
                    assert_eq!(err_from, from);
                    assert_eq!(to, BountyState::Completed);
                }
                other => panic!("expected InvalidTransition for from={from:?}, got {other:?}"),
            }
            let (stored_state, stored_completed_at): (String, Option<String>) = world
                .connection()
                .query_row(
                    "SELECT state, completed_at FROM bounties WHERE id = ?1",
                    rusqlite::params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                stored_state, from,
                "rejected complete must not mutate state"
            );
            // For a seeded `completed` row the original
            // `completed_at` was `'2026-01-01T00:01:00Z'`; pinning
            // that it didn't advance catches a regression that
            // loosened the WHERE predicate to permit a re-stamp.
            if from == "completed" {
                assert_eq!(
                    stored_completed_at.as_deref(),
                    Some("2026-01-01T00:01:00Z"),
                    "rejected complete must not advance completed_at"
                );
            } else {
                assert_eq!(
                    stored_completed_at, None,
                    "rejected complete must not stamp completed_at on a non-completed row"
                );
            }
        }
    }

    ///   acceptance — `expire_bounties` table.
    ///
    /// The sweeper operates on a set, not a single id, and its
    /// "invalid transition" is silent non-inclusion (no typed
    /// error). The table covers every starting state plus deadline
    /// variants (no deadline future deadline past deadline) to
    /// pin the `expires_at IS NOT NULL` and `<= now` gates and the
    /// `state IN ('open','claimed')` membership. makes
    /// both `open` and `claimed` rows eligible — a claimant who
    /// never finishes their work shouldn't pin the bounty open
    /// forever.
    ///
    /// Legal sweeps are `(open, past) -> expired` and
    /// `(claimed, past) -> expired`. Every other case MUST stay in
    /// its starting state after the sweep.
    #[rstest::rstest]
    #[case::open_past_deadline_is_swept("open", Some("2000-01-01T00:00:00Z"), true, "expired")]
    #[case::open_no_deadline_is_not_swept("open", None, false, "open")]
    #[case::open_future_deadline_is_not_swept("open", Some("2999-12-31T23:59:59Z"), false, "open")]
    #[case::claimed_past_deadline_is_swept(
        "claimed",
        Some("2000-01-01T00:00:00Z"),
        true,
        "expired"
    )]
    #[case::claimed_no_deadline_is_not_swept("claimed", None, false, "claimed")]
    #[case::claimed_future_deadline_is_not_swept(
        "claimed",
        Some("2999-12-31T23:59:59Z"),
        false,
        "claimed"
    )]
    #[case::completed_is_not_swept("completed", None, false, "completed")]
    #[case::expired_is_not_swept("expired", None, false, "expired")]
    fn expire_bounties_table(
        #[case] start_state: &str,
        #[case] expires_at: Option<&str>,
        #[case] should_sweep: bool,
        #[case] end_state: &str,
    ) {
        let (_dir, world, poster_id, alice_id, _bob_id) = world_with_poster_and_two_claimants();

        // For "open" rows we drive `expires_at` from the case data
        // via the public `post_bounty` helper so the deadline gate
        // is exercised end-to-end. For non-open seed states, we
        // reach for `seed_bounty_in_state` and override its default
        // `expires_at` to match the case so a `claimed + future
        // deadline` row really does carry that future deadline.
        let id = if start_state == "open" {
            world
                .post_bounty(
                    Some(poster_id),
                    "Sweep target",
                    "Sweep policy under test.",
                    r#"{"credits":1}"#,
                    expires_at,
                )
                .unwrap()
                .id
        } else {
            let id = seed_bounty_in_state(&world, poster_id, alice_id, start_state);
            // `seed_bounty_in_state`'s default `expires_at` only
            // matches our case for `expired` (past deadline); for
            // `claimed` we override it to whatever the case wants.
            if start_state == "claimed" {
                world
                    .connection()
                    .execute(
                        "UPDATE bounties SET expires_at = ?2 WHERE id = ?1",
                        rusqlite::params![id, expires_at],
                    )
                    .unwrap();
            }
            id
        };

        let swept = world
            .expire_bounties("2026-01-01T00:00:00Z")
            .expect("sweep succeeds");
        let swept_ids: Vec<i64> = swept.iter().map(|b| b.id).collect();
        assert_eq!(
            swept_ids.contains(&id),
            should_sweep,
            "row id {id} sweep inclusion mismatch for ({start_state:?}, {expires_at:?})"
        );

        let stored: String = world
            .connection()
            .query_row(
                "SELECT state FROM bounties WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, end_state,
            "post-sweep state mismatch for ({start_state:?}, {expires_at:?})"
        );
    }

    /// Read column names from `pragma_table_info` in cid order
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
