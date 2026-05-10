//! `market` — shared-world market-listing schema (SPEC_v3 §4.3 /
//! §Task 5a).
//!
//! v3 introduces durable async player-to-player marketplaces: a
//! seller posts a listing (price, quantity, item key), a buyer
//! atomically decrements the quantity, the game's inventory and
//! balance callbacks settle the transaction. The whole feature sits
//! on top of one `market_listings` table whose shape is pinned by
//! [`MARKET_LISTINGS_MIGRATION`]. This module exists only to declare
//! that schema and prove it applies; the `MarketListing` Rust type
//! and the `create_listing` / `active_listings` / `buy_listing`
//! helpers land in subsequent §Task 5 sub-items (5b–5f). Splitting
//! the migration into its own commit keeps the bisect signal sharp —
//! a column rename, a relaxed `CHECK`, or a dropped partial index
//! flunks the schema test in this module rather than a higher-level
//! transactional test that's harder to attribute.
//!
//! # Why a dedicated table
//!
//! SPEC_v3 §3 lists the market alongside notices, challenges,
//! factions, and bounties as separate primitives. We follow the same
//! v2/v3 convention as [`crate::notices`] and [`crate::challenges`]:
//! one table, one migration, one named index family. Folding
//! listings onto the `world_events` log would conflate the
//! append-only event stream with mutable inventory state (`quantity`
//! decrements with every buy) — fundamentally different write
//! patterns.
//!
//! # Why `version = 8`
//!
//! v2 occupies migration versions 1–5 (see `docs/shared-world.md`
//! §8.1). v3 claims `6` and above, dense and grouped per primitive.
//! Notices took version 6, challenges took 7. Market listings are the
//! third v3 primitive to land, so they take 8. Subsequent v3
//! migrations (factions, bounties) MUST pick the next available kit
//! version — game-authored migrations live in their own higher band
//! and are not affected.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the shared marketplace table — SPEC_v3 §4.3 / §Task 5a.
///
/// One row per listing. Listings are mutable in the narrow sense that
/// `quantity` is decremented by the typed helpers landing in Tasks
/// 5d/5e (atomic buy with rollback); the addressing, `item_key`,
/// `display_name`, `price`, and creation timestamp are write-once.
/// The kit's contract is "if you only go through the public API, the
/// only state changes are the documented quantity decrements, and
/// every decrement runs inside a SQLite transaction with the buyer
/// callback" (SPEC_v3 §4.3 / §7). An operator with `sqlite3` can of
/// course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`] and
/// [`crate::challenges::CHALLENGES_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid.
///   Doubles as the deterministic tiebreaker for the
///   `active_listings` query (Task 5c) when two listings share a
///   `created_at` value at second resolution. Same role as `id` on
///   notices and challenges.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. ISO text so it
///   sorts lexically the same way it sorts chronologically and reads
///   cleanly in the `sqlite3` CLI — the same contract as every other
///   v2/v3 timestamp column.
/// - `seller_player_id` — `INTEGER REFERENCES players(id)`,
///   **nullable**. SPEC §4.3 explicitly allows NPC/system listings
///   ("seller player id optional for NPC/system listings"). Foreign-
///   keyed for the same reason as the turn ledger and notices: a
///   phantom id should never land here. SQLite enforces FKs only
///   when `PRAGMA foreign_keys = ON`, which the runtime is
///   responsible for; until then the constraint is documentation but
///   the column shape is correct.
/// - `item_key` — `TEXT NOT NULL`. Game-authored stable identifier
///   for the item being sold (e.g. `"clue.fingerprint"`,
///   `"item.lockpick"`). Game code owns the namespace; the kit's
///   only rule is "round-trips as text". Used by `active_listings`
///   to filter "show me only X listings" and by buy callbacks to
///   route the inventory grant.
/// - `display_name` — `TEXT NOT NULL`. Player- or game-authored
///   short title rendered in the marketplace UI. Length bounds for
///   player-authored display names are enforced by Task 5b's helper,
///   not at the schema layer, because the cap lives alongside other
///   `[multiplayer]` config — the same rationale as
///   `notices.subject` and `notices.body`.
/// - `price` — `INTEGER NOT NULL CHECK (price >= 0)`. Game-defined
///   currency unit, expressed as a non-negative integer. SPEC §4.3
///   requires "The kit MUST reject negative prices"; the schema-
///   level `CHECK` is the safety net so a regression that bypassed
///   the Task 5b validator (e.g. raw SQL in a test fixture, or a
///   future helper that forgot to validate) fails at INSERT time
///   rather than landing a corrupt row that crashes the marketplace
///   UI on read. Stored as `INTEGER` rather than `REAL` because
///   floating-point currency is a known footgun (`0.1 + 0.2 != 0.3`
///   in IEEE 754); games that need fractional pricing scale into
///   minor units (cents, mils) at the boundary.
/// - `quantity` — `INTEGER NOT NULL CHECK (quantity >= 0)`. Number
///   of units still available in the listing. Decrements atomically
///   on `buy_listing` (Task 5d). SPEC §4.3 requires "The kit MUST
///   reject negative quantities"; the `CHECK` is the schema-side
///   safety net for the same reason as `price`. A listing whose
///   quantity reaches `0` stays in the table (audit trail; future
///   reactivation by the seller) but falls out of the partial
///   `idx_market_listings_active` index below.
/// - `expires_at` — `TEXT`, nullable. ISO timestamp after which an
///   active listing should fall out of the marketplace UI. SPEC §4.3
///   lists `expires_at` in the field set but does not specify
///   automatic expiry semantics; the kit follows the same policy
///   as `notices.expires_at` — store the column, let the
///   `active_listings` query (Task 5c) filter on it, do not run a
///   sweeper. Nullable so a listing can be open-ended (no deadline)
///   without reserving a sentinel value.
/// - `metadata` — `TEXT`, nullable. Optional opaque JSON. Stored as
///   text rather than `BLOB` so an operator can pretty-print it
///   with `sqlite3 -json`; the kit treats this column as opaque,
///   the same contract as `notices.metadata` and `challenges.stake`.
///   Game code that wants structured metadata (item rarity, lore
///   blurb, etched serial number) serialises JSON before handing it
///   to the kit.
///
/// # Indexes
///
/// One partial index is created up-front so the `active_listings`
/// query pattern Task 5c relies on is seek-bound from the moment it
/// lands. Adding it later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the
/// index cost at the same migration that creates the table — the
/// same rationale as `idx_notices_inbox` and the partial indexes on
/// `challenges`.
///
/// - `idx_market_listings_active` is a partial index over
///   `(created_at, id)` `WHERE quantity > 0`. The partial predicate
///   keeps the index small — exhausted listings (quantity = 0) fall
///   out automatically the moment the buy transaction commits, so
///   the active-marketplace view never has to filter them out at
///   query time. The leading `created_at` column matches Task 5c's
///   expected default ordering ("newest first" / "oldest first" —
///   either direction is a one-line query change against this
///   index). The `id` tiebreaker keeps the order deterministic when
///   two listings post in the same second, mirroring the
///   `notices` / `challenges` partial-index strategy.
///
/// # Why no `kind` column
///
/// Notices have `kind` because the inbox UI filters by category
/// (`"guestbook_note"`, `"challenge_offer"`). Listings have
/// `item_key` instead, which serves the same role — a stable
/// machine-readable handle that game code dispatches on. Adding a
/// separate `kind` column would duplicate the namespace and tempt
/// game authors to use one or the other inconsistently.
///
/// # Version
///
/// `version = 8`. Notices claim 6, challenges claim 7 (see
/// [`crate::notices::NOTICES_MIGRATION`] and
/// [`crate::challenges::CHALLENGES_MIGRATION`]); market listings are
/// the third v3 primitive to land, so they take 8. Subsequent v3
/// migrations (factions, bounties) take 9 and onward.
pub const MARKET_LISTINGS_MIGRATION: WorldMigration = WorldMigration {
    version: 8,
    name: "create_market_listings",
    sql: "\
CREATE TABLE IF NOT EXISTS market_listings (\n\
    id               INTEGER PRIMARY KEY,\n\
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    seller_player_id INTEGER REFERENCES players(id),\n\
    item_key         TEXT NOT NULL,\n\
    display_name     TEXT NOT NULL,\n\
    price            INTEGER NOT NULL CHECK (price >= 0),\n\
    quantity         INTEGER NOT NULL CHECK (quantity >= 0),\n\
    expires_at       TEXT,\n\
    metadata         TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_market_listings_active\n\
    ON market_listings(created_at, id) WHERE quantity > 0;\n\
",
};

/// Decoded `market_listings` row — SPEC_v3 §4.3 read model.
///
/// Mirrors the column shape pinned by [`MARKET_LISTINGS_MIGRATION`]
/// one-for-one, in the same order, so the SQL `RETURNING` clause and
/// the upcoming `active_listings` row decoder share a single column
/// list. Authoring code consumes this struct rather than reaching into
/// raw `rusqlite::Row`s — that keeps the schema-to-Rust mapping in
/// one place and turns a column rename into a single compile error
/// instead of a fan-out of decode failures. Same shape as
/// [`crate::notices::Notice`] and [`crate::challenges::Challenge`].
///
/// All timestamps stay as raw SQLite ISO text, the same contract as
/// every other v2/v3 read model: parsing into a richer type would be
/// a one-way trip that hides corrupt data and forces a chrono / time
/// dependency on every consumer. `metadata` is likewise opaque text —
/// game code that wants structured metadata serialises JSON before
/// handing it to [`WorldDb::create_listing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketListing {
    /// Autoincrement primary key. Doubles as the deterministic
    /// tiebreaker for the `active_listings` query (Task 5c) when two
    /// listings share a `created_at` value at second resolution. Same
    /// role as `notices.id` and `challenges.id`.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text — see struct docs.
    pub created_at: String,
    /// Seller's `players.id`, or `None` for NPC/system listings. SPEC
    /// §4.3 explicitly allows "seller player id optional for
    /// NPC/system listings".
    pub seller_player_id: Option<i64>,
    /// Game-authored stable identifier for the item being sold
    /// (e.g. `"clue.fingerprint"`). Round-tripped verbatim; the kit
    /// imposes no namespace.
    pub item_key: String,
    /// Player- or game-authored short title rendered in the
    /// marketplace UI. Length bounds are enforced by the
    /// [`WorldDb::create_listing`] helper, not at the storage layer.
    pub display_name: String,
    /// Price in the game's chosen currency unit (game decides whether
    /// that is a major or minor unit). Always non-negative — the
    /// schema-level `CHECK (price >= 0)` is the safety net behind the
    /// Rust-side validator on the write path.
    pub price: i64,
    /// Units still available in the listing. Decrements atomically on
    /// `buy_listing` (Task 5d). Always non-negative — the schema-level
    /// `CHECK (quantity >= 0)` is the safety net behind the Rust-side
    /// validator on the write path. A listing whose quantity reaches
    /// zero stays in the table for audit but falls out of the
    /// `idx_market_listings_active` partial index.
    pub quantity: i64,
    /// Optional ISO timestamp after which an active listing should
    /// fall out of the marketplace UI. The kit does not auto-expire
    /// listings in v3 (see module docs).
    pub expires_at: Option<String>,
    /// Optional opaque metadata blob (typically a JSON object). Stored
    /// as text so `sqlite3 -json` can pretty-print it; the kit does
    /// not parse it.
    pub metadata: Option<String>,
}

/// Kit-internal cap for a listing's player- or game-authored
/// `display_name`.
///
/// SPEC_v3 §5.2 ships only `max_notice_body_chars` as configurable;
/// listing display names follow the same convention as the notice
/// subject line ([`crate::notices::NOTICE_SUBJECT_MAX_CHARS`]) — bound
/// by the kit, not by game authors, so every consuming game presents
/// a uniform "short title" UI in the marketplace. 120 chars matches
/// the notice-subject cap and fits one 80-column line with room for a
/// price/quantity suffix in marketplace list views.
///
/// Counted in Unicode scalar values (`str::chars().count()`), not
/// bytes — SPEC §4.3 / §7 talk in *characters*, and a byte cap would
/// let a single emoji eat four "chars" of budget. Same rule as the
/// notice-subject and (eventually) bounty-title caps.
pub const MARKET_DISPLAY_NAME_MAX_CHARS: usize = 120;

/// Failure modes for [`WorldDb::create_listing`] and the upcoming
/// market-lifecycle helpers.
///
/// Library-internal `thiserror` shape — the runtime wraps these with
/// `anyhow` at the process boundary. Mirrors
/// [`crate::notices::NoticeError`] and
/// [`crate::challenges::ChallengeError`] so all v3 multiplayer write
/// paths surface errors with the same shape (a future code review
/// pass can fold these into a single multiplayer-error trait if a
/// fourth primitive needs the same variants, but three primitives
/// still doesn't justify the abstraction yet — SPEC tenet "no
/// premature abstraction").
///
/// Task 5b needs: `EmptyItemKey`, `EmptyDisplayName`,
/// `DisplayNameTooLong`, `NegativePrice`, `NegativeQuantity`, and
/// `Sqlite`. `NotFound` and the buy-time rollback variants land with
/// Tasks 5d–5f.
#[derive(Debug, Error)]
pub enum MarketError {
    /// `item_key` was empty. SPEC §4.3 lists `item_key` as required;
    /// the kit additionally rejects the empty string here so a listing
    /// can always be filtered/dispatched by item. A regression that
    /// silently accepted `""` would surface as a phantom row in every
    /// `WHERE item_key = ?` query.
    #[error("market listing item_key must not be empty")]
    EmptyItemKey,
    /// `display_name` was empty. SPEC §4.3 lists `display_name` as
    /// required; the kit additionally rejects the empty string so the
    /// marketplace UI never renders a row with a blank title that the
    /// browser can't tell apart from a rendering bug. Same rationale
    /// as [`crate::notices::NoticeError::EmptySubject`].
    #[error("market listing display_name must not be empty")]
    EmptyDisplayName,
    /// `display_name` exceeded [`MARKET_DISPLAY_NAME_MAX_CHARS`].
    /// Surfacing both the limit and the actual length lets the
    /// authoring screen show "120 / 137 characters" without
    /// re-counting. Same shape as
    /// [`crate::notices::NoticeError::SubjectTooLong`].
    #[error("market listing display_name exceeds {max}-character limit (got {actual})")]
    DisplayNameTooLong {
        /// Cap that was breached — currently always
        /// [`MARKET_DISPLAY_NAME_MAX_CHARS`], named so future per-game
        /// caps (if ever introduced) don't break the error shape.
        max: usize,
        /// Actual `chars().count()` of the rejected display name, in
        /// scalar values.
        actual: usize,
    },
    /// `price` was negative. SPEC §4.3 mandates "The kit MUST reject
    /// negative prices". The schema-level `CHECK (price >= 0)` is the
    /// safety net; this variant is the typed surface so authoring
    /// screens can render "price must be zero or higher" without
    /// parsing a generic SQL constraint error.
    #[error("market listing price must be non-negative (got {actual})")]
    NegativePrice {
        /// Actual price the caller passed. Echoed so the authoring
        /// screen can re-render the offending input.
        actual: i64,
    },
    /// `quantity` was negative. SPEC §4.3 mandates "The kit MUST
    /// reject negative quantities". Same rationale as
    /// [`Self::NegativePrice`]: the schema CHECK is the safety net,
    /// this variant is the typed UI surface.
    #[error("market listing quantity must be non-negative (got {actual})")]
    NegativeQuantity {
        /// Actual quantity the caller passed. Echoed so the authoring
        /// screen can re-render the offending input.
        actual: i64,
    },
    /// `buy_listing` was called with a non-positive quantity. SPEC §4.3
    /// frames the buy as "decrement quantity by N"; N must be strictly
    /// positive — buying zero or a negative number is meaningless and
    /// would leave the listing unchanged while still appearing to
    /// "succeed" to UI callers. Pinning the typed error means a UI
    /// regression that fed a stuck "0" through the buy keypad surfaces
    /// here rather than as a silent no-op. Same defensive shape as
    /// [`Self::NegativeQuantity`].
    #[error("market listing buy quantity must be positive (got {actual})")]
    NonPositiveBuyQuantity {
        /// Actual quantity the caller requested. Echoed so the buy
        /// screen can re-render the offending input.
        actual: i64,
    },
    /// `buy_listing` referenced a listing id that does not exist in
    /// `market_listings`. Distinct from
    /// [`Self::InsufficientQuantity`] so the marketplace UI can offer a
    /// targeted "this listing has been removed, refresh your view"
    /// recovery rather than a generic "couldn't buy" toast. Same shape
    /// as [`crate::notices::NoticeError::NotFound`] and
    /// [`crate::challenges::ChallengeError::NotFound`].
    #[error("market listing id {id} was not found")]
    NotFound {
        /// Listing id the caller passed.
        id: i64,
    },
    /// `buy_listing` requested more units than the listing has
    /// available (race-safe against concurrent buys: the conditional
    /// `UPDATE` only succeeds when `quantity >= requested`, so a
    /// listing that was sufficient at the read but exhausted by the
    /// time the write commits surfaces here, not as a silent partial
    /// fill). Echoes both the requested and the actual on-row quantity
    /// so the marketplace UI can render "only 2 left, you asked for 5"
    /// without re-querying.
    #[error(
        "market listing {id} has only {available} available; cannot fulfil buy of {requested}"
    )]
    InsufficientQuantity {
        /// Listing id the caller passed.
        id: i64,
        /// Quantity the caller requested.
        requested: i64,
        /// Quantity actually on the listing row at the moment the
        /// conditional update ran.
        available: i64,
    },
    /// The buyer callback inside [`crate::WorldDb::buy_listing`]
    /// returned `Err`. The wrapping transaction has already rolled
    /// back, so the listing's `quantity` is unchanged and any side
    /// effects the callback attempted (inventory grant, balance
    /// debit, seller credit) are undone — that's the SPEC §4.3
    /// "Buying MUST be atomic" contract. Distinct from
    /// [`Self::Sqlite`] so the marketplace UI can surface a buyer-
    /// side reason ("you can't afford this") separately from a
    /// SQLite-layer failure.
    #[error("buyer callback inside buy_listing failed; listing quantity rolled back: {source}")]
    BuyerCallback {
        /// Underlying `rusqlite::Error` returned by the callback. The
        /// callback is expected to map any non-SQLite failure into a
        /// [`rusqlite::Error::SqliteFailure`] with an explanatory
        /// message before returning, the same convention as
        /// [`crate::world_db::SpendAndEmitError::Mutation`].
        #[source]
        source: rusqlite::Error,
    },
    /// The `INSERT … RETURNING` round-trip (or a future `UPDATE` on
    /// the buy path) failed. Wrapping `rusqlite::Error` keeps the call
    /// site readable (one error type, one mapping) while preserving
    /// the underlying cause for `tracing` and operator-facing
    /// messages.
    #[error("failed to write market listing to world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the statement.
        #[source]
        source: rusqlite::Error,
    },
}

impl WorldDb {
    /// Insert one row into `market_listings` and return the canonical
    /// [`MarketListing`] SQLite produced (SPEC_v3 §4.3 / §Task 5b).
    ///
    /// The contract is "the listing I asked you to create is now
    /// durably in the marketplace, with the id and `created_at`
    /// SQLite assigned, and the price/quantity/display_name/item_key
    /// I supplied". `seller_player_id` is `Option<i64>` because SPEC
    /// §4.3 explicitly allows NPC/system listings with no
    /// attributable seller. `expires_at` and `metadata` are likewise
    /// optional — `None` means "open-ended" / "no metadata".
    ///
    /// `price` and `quantity` are `i64` so this helper can validate
    /// negatives at the boundary and return a typed
    /// [`MarketError::NegativePrice`] / [`MarketError::NegativeQuantity`]
    /// rather than letting the caller hit the schema `CHECK` and
    /// surface a stringly-typed SQL error. The schema CHECKs remain
    /// the safety net for any path that bypasses this helper.
    ///
    /// # Validation order
    ///
    /// All checks run **before** the SQL round-trip so a rejected
    /// listing never produces a row, an autoincrement gap, or an
    /// event-log entry. Order — emptiness first (cheapest), then
    /// length, then signedness — mirrors
    /// [`Self::send_notice`] for symmetry across multiplayer write
    /// paths. `item_key` is checked before `display_name` because a
    /// blank `item_key` makes the listing un-routable at the buy step
    /// regardless of how friendly its display name is.
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) to read the
    /// canonical row — `id`, the SQL-side `created_at`, plus every
    /// other column — without a second round-trip, the same pattern
    /// as [`Self::send_notice`], [`Self::create_challenge`], and
    /// [`Self::append_event`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single insert statement under the configured
    /// busy timeout. The buy path (Task 5d) will need an explicit
    /// transaction because it reads quantity, decrements, and
    /// dispatches buyer/seller callbacks; *creating* a listing is one
    /// `INSERT` and is already atomic.
    #[allow(clippy::too_many_arguments)] // Matches the SPEC §4.3 column shape one-for-one (seller, item_key, display_name, price, quantity, expires_at, metadata); bundling into a struct would force every call site through a builder dance without adding type safety, since each parameter is already strongly typed. Same rationale as `WorldDb::send_notice`.
    pub fn create_listing(
        &self,
        seller_player_id: Option<i64>,
        item_key: &str,
        display_name: &str,
        price: i64,
        quantity: i64,
        expires_at: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<MarketListing, MarketError> {
        // Validation runs before the SQL round-trip so a rejected
        // listing never produces a row. Emptiness first (cheapest),
        // then length, then signedness — same ordering as the notice
        // path, so a UI that re-renders the authoring screen on
        // failure shows a consistent surface across primitives.
        if item_key.is_empty() {
            return Err(MarketError::EmptyItemKey);
        }
        if display_name.is_empty() {
            return Err(MarketError::EmptyDisplayName);
        }
        // Count Unicode scalar values, not bytes — SPEC §4.3/§7 talk
        // in characters, and a byte cap would penalise non-ASCII
        // listing names.
        let display_chars = display_name.chars().count();
        if display_chars > MARKET_DISPLAY_NAME_MAX_CHARS {
            return Err(MarketError::DisplayNameTooLong {
                max: MARKET_DISPLAY_NAME_MAX_CHARS,
                actual: display_chars,
            });
        }
        if price < 0 {
            return Err(MarketError::NegativePrice { actual: price });
        }
        if quantity < 0 {
            return Err(MarketError::NegativeQuantity { actual: quantity });
        }

        // `RETURNING` echoes the full row back — including the
        // SQL-side `CURRENT_TIMESTAMP` default for `created_at`. The
        // column order here matches `row_to_listing` so all read
        // paths share one decoder.
        const SQL: &str = "\
INSERT INTO market_listings \
    (seller_player_id, item_key, display_name, price, quantity, expires_at, metadata) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
RETURNING id, created_at, seller_player_id, item_key, display_name, \
          price, quantity, expires_at, metadata";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    seller_player_id,
                    item_key,
                    display_name,
                    price,
                    quantity,
                    expires_at,
                    metadata,
                ],
                row_to_listing,
            )
            .map_err(|source| MarketError::Sqlite { source })
    }

    /// Return every currently-active market listing, sorted
    /// deterministically (SPEC_v3 §4.3 / §Task 5c).
    ///
    /// "Active" means two things:
    ///
    /// 1. `quantity > 0` — exhausted listings stay in the table for
    ///    audit (a future "listing history" view consumes them) but
    ///    fall out of the marketplace UI the moment a buy commits. The
    ///    partial `idx_market_listings_active` index landed in 5a
    ///    encodes that predicate, so the planner can satisfy this
    ///    query with an index walk and no residual scan.
    /// 2. `expires_at` is either NULL (open-ended listing) or strictly
    ///    in the future. The kit deliberately does **not** run a
    ///    sweeper on listings — same policy as `notices.expires_at`,
    ///    documented on [`MARKET_LISTINGS_MIGRATION`]. Filtering at
    ///    read time keeps the schema simple (no background jobs, no
    ///    "expired" state column to maintain) at the cost of a tiny
    ///    `datetime()` comparison per row in the active set. The
    ///    `datetime()` wrapping handles both ISO forms the kit accepts
    ///    (`'YYYY-MM-DDTHH:MM:SSZ'` from callers,
    ///    `'YYYY-MM-DD HH:MM:SS'` from `CURRENT_TIMESTAMP`) so the
    ///    comparison is chronological rather than lexicographic — same
    ///    rationale as [`Self::accept_challenge`] and
    ///    [`Self::expire_open_challenges`].
    ///
    /// # Ordering
    ///
    /// `ORDER BY created_at DESC, id DESC` — newest first, with the
    /// autoincrement `id` as the deterministic tiebreaker when two
    /// listings post in the same SQLite second. Matches the partial
    /// index column order so the planner satisfies the sort with a
    /// reverse index walk. Same shape as [`Self::inbox`] and
    /// [`Self::recent_events`] so the marketplace UI feels consistent
    /// with the rest of the v3 mailbox surface.
    ///
    /// # Now-clock
    ///
    /// Uses SQL-side `datetime('now')` rather than a `now: &str`
    /// parameter. This is a read query called from interactive UI
    /// paths; the `expire_open_challenges` sweeper takes a `now`
    /// parameter because it *writes* and tests need a deterministic
    /// cutoff on the boundary, but `active_listings` is observation-
    /// only and the SQL clock is what production callers want anyway.
    /// Tests express expiry windows relative to "now" using
    /// `datetime('now', '-1 hour')` / `'+1 hour'` modifiers.
    ///
    /// # No filters
    ///
    /// No `item_key` filter, no `seller_player_id` filter, no
    /// pagination. SPEC §4.3 doesn't ask for any, and the v3
    /// marketplace is expected to stay small enough (per-game world,
    /// short-lived listings) that the UI can filter client-side. A
    /// future paged or item-filtered variant can be added without
    /// breaking this signature.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single read statement under the configured
    /// busy timeout, same as [`Self::inbox`].
    pub fn active_listings(&self) -> Result<Vec<MarketListing>, MarketError> {
        // Column order matches `row_to_listing` and the `RETURNING`
        // clause in `create_listing` — one decoder, one column list,
        // surfaced as a type error if a future schema edit diverges
        // them.
        const SQL: &str = "\
SELECT id, created_at, seller_player_id, item_key, display_name, \
       price, quantity, expires_at, metadata \
FROM market_listings \
WHERE quantity > 0 \
  AND (expires_at IS NULL OR datetime(expires_at) > datetime('now')) \
ORDER BY created_at DESC, id DESC";

        let mut stmt = self
            .connection()
            .prepare(SQL)
            .map_err(|source| MarketError::Sqlite { source })?;
        let rows = stmt
            .query_map([], row_to_listing)
            .map_err(|source| MarketError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| MarketError::Sqlite { source })
    }

    /// Atomically decrement a listing's `quantity` and run a buyer
    /// callback inside the same SQLite transaction (SPEC_v3 §4.3 /
    /// §Task 5d).
    ///
    /// The contract is the SPEC §4.3 invariant verbatim: "Buying MUST
    /// be atomic: quantity decrement, buyer inventory/balance callback,
    /// seller credit callback if used, event append." Tasks 5d–5f land
    /// the three observable phases incrementally — 5d here lays down
    /// the kit-owned quantity decrement plus the buyer callback hook;
    /// 5e adds the rollback test when the callback fails (no signature
    /// change); 5f layers the world-event append after the callback
    /// inside the same transaction. Game code that already wires a
    /// callback today gets the event-append for free when 5f lands.
    ///
    /// # Atomicity model
    ///
    /// The decrement is a single conditional `UPDATE … RETURNING` whose
    /// `WHERE` clause folds the existence + sufficiency check:
    /// `id = ?1 AND quantity >= ?2`. Combining the two predicates means
    /// the decrement is race-safe under concurrent buys — a listing
    /// that was sufficient at read time but exhausted by the time this
    /// statement commits returns `QueryReturnedNoRows`, not a silent
    /// partial fill. The whole flow runs inside an explicit
    /// `Connection::transaction` so the callback's writes (inventory
    /// grant, balance debit, future event append) commit atomically
    /// with the decrement; if any of them returns `Err`, dropping the
    /// transaction without `commit()` rolls every write back, which is
    /// the SPEC §7 "A failed transaction MUST NOT partially debit
    /// turns, consume inventory, or change challenge state" guarantee
    /// — extended here to listings.
    ///
    /// # Buyer callback shape
    ///
    /// Receives `(tx, post_decrement_listing)`. The tx borrow lets the
    /// callback issue its own writes against the same transaction
    /// (game-defined inventory and balance tables); the listing argument
    /// is the post-decrement row, so the callback knows exactly which
    /// item to grant, at what price, on whose behalf. Returning
    /// `rusqlite::Error` keeps the helper minimal — game code with
    /// richer error needs maps non-SQLite failures into
    /// `Error::SqliteFailure` with an explanatory message, the same
    /// convention as
    /// [`crate::world_db::WorldDb::spend_turn_and_emit`]. Callbacks that
    /// don't need to do anything (system listing, "kit decides
    /// inventory") can pass a no-op closure.
    ///
    /// # Error mapping
    ///
    /// - [`MarketError::NonPositiveBuyQuantity`] — caller asked to buy
    ///   `<= 0` units. Caught **before** opening the transaction so a
    ///   broken UI doesn't pay for a `BEGIN` round-trip.
    /// - [`MarketError::NotFound`] — listing id does not exist. Surfaced
    ///   via a follow-up SELECT after `QueryReturnedNoRows`, so the UI
    ///   can render "this listing has been removed".
    /// - [`MarketError::InsufficientQuantity`] — listing exists but
    ///   has fewer units available than requested. Carries both the
    ///   requested and the actual on-row quantity so the buy screen
    ///   can render "only 2 left" without a second query.
    /// - [`MarketError::BuyerCallback`] — callback returned `Err`. The
    ///   transaction has already rolled back; this is the only error
    ///   variant the rollback path produces.
    /// - [`MarketError::Sqlite`] — SQLite-layer failure on the
    ///   transaction begin/commit, the `UPDATE … RETURNING`, or the
    ///   diagnostic `SELECT`.
    ///
    /// # Concurrency
    ///
    /// Takes `&mut self`: the wrapping transaction needs a unique
    /// borrow on the connection, same as
    /// [`Self::spend_turn_and_emit`] and [`Self::transaction`]. Game
    /// code that drives buy from a screen tick does so via a
    /// `&mut WorldDb` borrow scoped to the tick, mirroring the existing
    /// `spend_turn_and_emit` call sites in `murder_motel`.
    pub fn buy_listing<F>(
        &mut self,
        listing_id: i64,
        quantity_to_buy: i64,
        buyer: F,
    ) -> Result<MarketListing, MarketError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &MarketListing) -> Result<(), rusqlite::Error>,
    {
        // Reject non-positive buys before opening the transaction so a
        // broken UI doesn't pay for a `BEGIN` round-trip and the
        // rollback branch doesn't need a "but the txn was empty" case.
        if quantity_to_buy <= 0 {
            return Err(MarketError::NonPositiveBuyQuantity {
                actual: quantity_to_buy,
            });
        }

        // `RETURNING` echoes the post-decrement row back so the buyer
        // callback can inspect it and the helper can return it to the
        // caller. Column order matches `row_to_listing` so the decoder
        // and the SQL stay locked together.
        const UPDATE_SQL: &str = "\
UPDATE market_listings \
SET quantity = quantity - ?2 \
WHERE id = ?1 AND quantity >= ?2 \
RETURNING id, created_at, seller_player_id, item_key, display_name, \
          price, quantity, expires_at, metadata";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| MarketError::Sqlite { source })?;

        // Conditional decrement. SQLite's per-statement atomicity plus
        // the wrapping transaction guarantees no concurrent buy can
        // interleave between the predicate check and the write.
        let listing = match tx.query_row(
            UPDATE_SQL,
            rusqlite::params![listing_id, quantity_to_buy],
            row_to_listing,
        ) {
            Ok(listing) => listing,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Diagnose why the conditional update affected zero
                // rows: either the listing doesn't exist (NotFound) or
                // it has fewer units than requested (InsufficientQuantity).
                // A second read inside the same transaction sees a
                // consistent snapshot (the UPDATE didn't change
                // anything) and lets us surface a typed error rather
                // than a generic "no rows".
                use rusqlite::OptionalExtension;
                let available: Option<i64> = tx
                    .query_row(
                        "SELECT quantity FROM market_listings WHERE id = ?1",
                        rusqlite::params![listing_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|source| MarketError::Sqlite { source })?;
                // Drop the transaction without commit — rolls back
                // automatically. The diagnostic SELECT didn't write
                // anything, so the rollback is observably a no-op, but
                // the explicit drop here documents the intent.
                drop(tx);
                return Err(match available {
                    None => MarketError::NotFound { id: listing_id },
                    Some(available) => MarketError::InsufficientQuantity {
                        id: listing_id,
                        requested: quantity_to_buy,
                        available,
                    },
                });
            }
            Err(source) => {
                drop(tx);
                return Err(MarketError::Sqlite { source });
            }
        };

        // Run the buyer callback inside the same transaction. A
        // callback `Err` propagates as `BuyerCallback` and the
        // transaction drops without commit — every write the callback
        // attempted, plus the kit's own decrement, rolls back as a
        // unit. SPEC §7's "no partial debit" guarantee.
        if let Err(source) = buyer(&tx, &listing) {
            return Err(MarketError::BuyerCallback { source });
        }

        tx.commit()
            .map_err(|source| MarketError::Sqlite { source })?;

        Ok(listing)
    }
}

/// Decode a `market_listings` row into [`MarketListing`].
///
/// Pulled out so the Task 5b write path and the upcoming Task 5c/5d
/// query and buy paths can share one decoder. Column order matches
/// the `RETURNING` clause in [`WorldDb::create_listing`] *and* the
/// future `active_listings` `SELECT`; a regression that reorders
/// columns will surface here as a type error rather than as a silent
/// field swap. Same shape as
/// [`crate::notices::row_to_notice`] and the challenges decoder.
fn row_to_listing(row: &rusqlite::Row<'_>) -> rusqlite::Result<MarketListing> {
    Ok(MarketListing {
        id: row.get(0)?,
        created_at: row.get(1)?,
        seller_player_id: row.get(2)?,
        item_key: row.get(3)?,
        display_name: row.get(4)?,
        price: row.get(5)?,
        quantity: row.get(6)?,
        expires_at: row.get(7)?,
        metadata: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use tempfile::tempdir;

    /// SPEC_v3 §Task 5a acceptance: applying
    /// [`MARKET_LISTINGS_MIGRATION`] creates the documented
    /// `market_listings` table with the column shape SPEC §4.3 pins.
    /// Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.3 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 5b–5f behavioural test).
    ///
    /// The players migration is applied first because
    /// `market_listings` references `players(id)` via the seller
    /// foreign key. With FK enforcement off (the SQLite default
    /// until the runtime turns it on) the migration would succeed
    /// even without the parent table, but exercising the real
    /// dependency order here mirrors how the runtime startup path
    /// drives migrations on a real door open.
    #[test]
    fn migration_creates_market_listings_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("market_listings migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'market_listings'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "market_listings table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('market_listings') ORDER BY cid")
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
                "seller_player_id".to_string(),
                "item_key".to_string(),
                "display_name".to_string(),
                "price".to_string(),
                "quantity".to_string(),
                "expires_at".to_string(),
                "metadata".to_string(),
            ],
            "market_listings column shape must match the SPEC_v3 §4.3 contract"
        );
    }

    /// SPEC §4.3 mandates "The kit MUST reject negative prices and
    /// negative quantities". The Rust-side validator landing in
    /// Task 5b is the primary enforcement; the schema-level `CHECK`
    /// constraints pinned here are the safety net for any path that
    /// bypasses the validator (raw SQL fixtures, future helpers
    /// that forget to validate). Pin both halves explicitly so a
    /// regression that relaxed either CHECK flunks here rather than
    /// in production.
    ///
    /// Same shape as `challenges_state_check_constraint_locks_vocabulary`:
    /// schema-level CHECK is the alphabet, Rust-side validation is
    /// the spelling.
    #[test]
    fn market_listings_check_constraints_reject_negative_values() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("market_listings migration applies");

        // A real seller row so the optional FK column references
        // something sensible; the FK itself isn't enforced without
        // `PRAGMA foreign_keys = ON`, but using a real id keeps the
        // test honest about what a production row looks like.
        let seller_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-alice', 'alice') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert seller");

        // Non-negative price/quantity must be accepted, including
        // the boundary value zero (a free or sold-out listing is
        // structurally valid; the active-listings index filters
        // quantity > 0 separately).
        for (price, quantity) in [(0_i64, 0_i64), (0, 1), (100, 0), (100, 5)] {
            world
                .connection()
                .execute(
                    "INSERT INTO market_listings \
                     (seller_player_id, item_key, display_name, price, quantity) \
                     VALUES (?1, 'unit_test', 'Unit Test Item', ?2, ?3)",
                    rusqlite::params![seller_id, price, quantity],
                )
                .unwrap_or_else(|err| {
                    panic!("(price={price}, quantity={quantity}) must be accepted by CHECK: {err}")
                });
        }

        // Negative price must be rejected — SPEC §4.3.
        let bad_price = world.connection().execute(
            "INSERT INTO market_listings \
             (seller_player_id, item_key, display_name, price, quantity) \
             VALUES (?1, 'unit_test', 'Unit Test Item', -1, 1)",
            rusqlite::params![seller_id],
        );
        assert!(
            bad_price.is_err(),
            "CHECK constraint must reject negative prices per SPEC §4.3"
        );

        // Negative quantity must be rejected — SPEC §4.3.
        let bad_quantity = world.connection().execute(
            "INSERT INTO market_listings \
             (seller_player_id, item_key, display_name, price, quantity) \
             VALUES (?1, 'unit_test', 'Unit Test Item', 1, -1)",
            rusqlite::params![seller_id],
        );
        assert!(
            bad_quantity.is_err(),
            "CHECK constraint must reject negative quantities per SPEC §4.3"
        );
    }

    /// The Task 5c `active_listings` helper will walk the partial
    /// `idx_market_listings_active` index; if that index ever stops
    /// being created, the read silently becomes a full table scan
    /// in production. Pin both the index name and its partial
    /// predicate (`WHERE quantity > 0`) so a regression flunks at
    /// `cargo test` rather than under load. Same rationale as
    /// `challenges_partial_indexes_are_present` and
    /// `notices_inbox_partial_index_is_present`.
    #[test]
    fn market_listings_active_partial_index_is_present() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("market_listings migration applies");

        let active_sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_market_listings_active'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_market_listings_active");
        assert!(
            active_sql.contains("quantity > 0"),
            "idx_market_listings_active must filter to quantity > 0; got: {active_sql}"
        );
        assert!(
            active_sql.contains("created_at"),
            "idx_market_listings_active must lead with created_at; got: {active_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; v3 inherits that contract. A
    /// second `apply_migration(&MARKET_LISTINGS_MIGRATION)` MUST be
    /// a no-op (the version is already in `world_migrations`), not
    /// an error from `CREATE TABLE` on an existing table. Same
    /// shape as `challenges_migration_is_idempotent` and
    /// `notices_migration_is_idempotent`.
    #[test]
    fn market_listings_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("first market_listings migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("second market_listings migration applies (idempotent)");
    }

    /// Helper: open a fresh world DB with players + market_listings
    /// migrations applied and a single seller row pre-seeded.
    /// Returned tuple keeps the `TempDir` alive for the test's scope
    /// (dropping it would unlink the SQLite file mid-test). Same
    /// shape as `notices::tests::world_with_notices`.
    fn world_with_listings() -> (tempfile::TempDir, WorldDb, i64) {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&MARKET_LISTINGS_MIGRATION)
            .expect("market_listings migration applies");
        let seller_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-alice', 'alice') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert seller");
        (dir, world, seller_id)
    }

    /// SPEC_v3 §Task 5b happy path: a valid `create_listing` round-
    /// trips a fully-populated [`MarketListing`] back to the caller —
    /// the kit-assigned `id` is non-zero, `created_at` is the SQLite-
    /// stamped ISO timestamp, and every input field is preserved
    /// verbatim. Pinning the full struct here guards against a
    /// regression that silently dropped a column from the `RETURNING`
    /// clause or the row decoder.
    #[test]
    fn create_listing_round_trips_full_row() {
        let (_dir, world, seller_id) = world_with_listings();

        let listing = world
            .create_listing(
                Some(seller_id),
                "clue.fingerprint",
                "Smudged Fingerprint Card",
                250,
                3,
                None,
                Some(r#"{"rarity":"uncommon"}"#),
            )
            .expect("create_listing succeeds with valid inputs");

        assert!(listing.id > 0, "kit-assigned id must be positive");
        assert!(
            !listing.created_at.is_empty(),
            "SQLite must stamp created_at"
        );
        assert_eq!(listing.seller_player_id, Some(seller_id));
        assert_eq!(listing.item_key, "clue.fingerprint");
        assert_eq!(listing.display_name, "Smudged Fingerprint Card");
        assert_eq!(listing.price, 250);
        assert_eq!(listing.quantity, 3);
        assert_eq!(listing.expires_at, None);
        assert_eq!(
            listing.metadata,
            Some(r#"{"rarity":"uncommon"}"#.to_string())
        );
    }

    /// SPEC_v3 §4.3 explicitly allows NPC/system listings with no
    /// seller. A `None` `seller_player_id` MUST round-trip — pinned
    /// here so a future helper that defensively defaults to "must
    /// have a seller" flunks the test rather than silently breaking
    /// system-shop listings in `murder_motel`.
    #[test]
    fn create_listing_accepts_no_seller_for_system_listings() {
        let (_dir, world, _seller_id) = world_with_listings();

        let listing = world
            .create_listing(
                None,
                "system.starter_kit",
                "Investigator's Starter Kit",
                0,
                10,
                None,
                None,
            )
            .expect("create_listing succeeds with no seller");

        assert_eq!(listing.seller_player_id, None);
        assert_eq!(listing.price, 0);
        assert_eq!(listing.quantity, 10);
    }

    /// SPEC §4.3 lists `item_key` as required. The kit additionally
    /// rejects `""` at the boundary so a blank-key regression
    /// surfaces as a typed [`MarketError::EmptyItemKey`] before it
    /// touches `world.sqlite`. Mirrors `send_notice_rejects_empty_subject`.
    #[test]
    fn create_listing_rejects_empty_item_key() {
        let (_dir, world, seller_id) = world_with_listings();

        let err = world
            .create_listing(Some(seller_id), "", "Anything", 1, 1, None, None)
            .expect_err("empty item_key must be rejected");
        assert!(
            matches!(err, MarketError::EmptyItemKey),
            "expected EmptyItemKey, got {err:?}"
        );

        // The rejected listing MUST NOT have produced a row — pin
        // that explicitly so a future regression that validated *and*
        // inserted (the classic "validate after the round-trip" bug)
        // surfaces here. Same belt-and-braces as
        // `send_notice_rejects_empty_subject`.
        let row_count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM market_listings", [], |row| row.get(0))
            .expect("count query runs");
        assert_eq!(
            row_count, 0,
            "rejected listing must not have produced a market_listings row"
        );
    }

    /// SPEC §4.3 lists `display_name` as required. Empty display
    /// names are rejected at the boundary for the same reason as
    /// empty subjects on notices (see
    /// [`crate::notices::NoticeError::EmptySubject`]) — a marketplace
    /// row with a blank title is indistinguishable from a UI bug.
    #[test]
    fn create_listing_rejects_empty_display_name() {
        let (_dir, world, seller_id) = world_with_listings();

        let err = world
            .create_listing(Some(seller_id), "item.lockpick", "", 5, 1, None, None)
            .expect_err("empty display_name must be rejected");
        assert!(
            matches!(err, MarketError::EmptyDisplayName),
            "expected EmptyDisplayName, got {err:?}"
        );
    }

    /// SPEC §4.3 mandates "The kit MUST reject negative prices". Pin
    /// the typed-error surface so authoring screens can render a
    /// targeted error message without parsing a generic SQL
    /// constraint failure. The schema-level CHECK is the safety net
    /// (covered by `market_listings_check_constraints_reject_negative_values`).
    #[test]
    fn create_listing_rejects_negative_price() {
        let (_dir, world, seller_id) = world_with_listings();

        let err = world
            .create_listing(
                Some(seller_id),
                "item.lockpick",
                "Lockpick",
                -1,
                1,
                None,
                None,
            )
            .expect_err("negative price must be rejected");
        match err {
            MarketError::NegativePrice { actual } => assert_eq!(actual, -1),
            other => panic!("expected NegativePrice, got {other:?}"),
        }
    }

    /// SPEC §4.3 mandates "The kit MUST reject negative quantities".
    /// Same rationale as the negative-price test: typed error for the
    /// UI, schema CHECK as the safety net.
    #[test]
    fn create_listing_rejects_negative_quantity() {
        let (_dir, world, seller_id) = world_with_listings();

        let err = world
            .create_listing(
                Some(seller_id),
                "item.lockpick",
                "Lockpick",
                5,
                -1,
                None,
                None,
            )
            .expect_err("negative quantity must be rejected");
        match err {
            MarketError::NegativeQuantity { actual } => assert_eq!(actual, -1),
            other => panic!("expected NegativeQuantity, got {other:?}"),
        }
    }

    /// A zero price (free listing) and a zero quantity (placeholder /
    /// pre-stocked entry) are both structurally valid — the schema
    /// CHECK is `>= 0`, not `> 0`. Pin both boundary values so a
    /// future "must be strictly positive" tightening lands as an
    /// explicit, reviewed change rather than a silent regression.
    #[test]
    fn create_listing_accepts_zero_price_and_zero_quantity() {
        let (_dir, world, seller_id) = world_with_listings();

        let free = world
            .create_listing(
                Some(seller_id),
                "promo.flyer",
                "Free Flyer",
                0,
                5,
                None,
                None,
            )
            .expect("zero price must be accepted");
        assert_eq!(free.price, 0);

        let sold_out = world
            .create_listing(
                Some(seller_id),
                "rare.coin",
                "Sold-Out Coin",
                99,
                0,
                None,
                None,
            )
            .expect("zero quantity must be accepted");
        assert_eq!(sold_out.quantity, 0);
    }

    /// `display_name` exactly at [`MARKET_DISPLAY_NAME_MAX_CHARS`]
    /// MUST be accepted; one character over MUST be rejected. Counted
    /// in Unicode scalar values, not bytes — same rule as
    /// `NOTICE_SUBJECT_MAX_CHARS`. Boundary tests prevent off-by-one
    /// regressions in either direction.
    #[test]
    fn create_listing_enforces_display_name_length_at_boundary() {
        let (_dir, world, seller_id) = world_with_listings();

        let at_limit: String = "a".repeat(MARKET_DISPLAY_NAME_MAX_CHARS);
        let listing = world
            .create_listing(Some(seller_id), "item.exact", &at_limit, 1, 1, None, None)
            .expect("display_name exactly at limit must be accepted");
        assert_eq!(
            listing.display_name.chars().count(),
            MARKET_DISPLAY_NAME_MAX_CHARS
        );

        let over_limit: String = "a".repeat(MARKET_DISPLAY_NAME_MAX_CHARS + 1);
        let err = world
            .create_listing(Some(seller_id), "item.over", &over_limit, 1, 1, None, None)
            .expect_err("display_name one over the limit must be rejected");
        match err {
            MarketError::DisplayNameTooLong { max, actual } => {
                assert_eq!(max, MARKET_DISPLAY_NAME_MAX_CHARS);
                assert_eq!(actual, MARKET_DISPLAY_NAME_MAX_CHARS + 1);
            }
            other => panic!("expected DisplayNameTooLong, got {other:?}"),
        }
    }

    /// SPEC_v3 §Task 5c: an empty marketplace returns an empty vec,
    /// not an error. Mirrors `inbox_returns_empty_vec_for_player_with_no_notices`
    /// — a fresh game world should render the marketplace screen
    /// without surfacing a "query failed" toast.
    #[test]
    fn active_listings_returns_empty_vec_when_no_listings() {
        let (_dir, world, _seller) = world_with_listings();
        let listings = world
            .active_listings()
            .expect("active_listings runs on empty table");
        assert!(
            listings.is_empty(),
            "fresh marketplace must yield zero listings, got {} rows",
            listings.len()
        );
    }

    /// Two listings posted in the same SQLite second MUST come back in
    /// a deterministic order — newest `id` first when `created_at`
    /// ties. Without an explicit tiebreaker, `ORDER BY created_at`
    /// alone is non-deterministic on rows that share a value, which
    /// would make the marketplace UI flicker between renders. Pinning
    /// the secondary `id DESC` ordering here guards against a
    /// regression that dropped the second sort key.
    ///
    /// Three rows are seeded in one rapid burst so they're nearly
    /// guaranteed to share a `created_at` second; the assertion is on
    /// `id` order alone, which is monotonic regardless of clock drift.
    #[test]
    fn active_listings_orders_newest_first_with_id_tiebreaker() {
        let (_dir, world, seller_id) = world_with_listings();

        // Three listings inserted back-to-back. We don't care about
        // their exact `created_at` strings; we care that the helper
        // returns them in `id DESC` order when their timestamps tie
        // (which is overwhelmingly likely at sub-second cadence).
        let first = world
            .create_listing(Some(seller_id), "item.a", "Alpha", 10, 1, None, None)
            .expect("first listing inserts");
        let second = world
            .create_listing(Some(seller_id), "item.b", "Bravo", 20, 1, None, None)
            .expect("second listing inserts");
        let third = world
            .create_listing(Some(seller_id), "item.c", "Charlie", 30, 1, None, None)
            .expect("third listing inserts");

        let listings = world
            .active_listings()
            .expect("active_listings runs after three inserts");

        assert_eq!(
            listings.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![third.id, second.id, first.id],
            "active_listings must return newest id first when created_at ties"
        );
    }

    /// A listing whose `quantity` has reached zero MUST fall out of
    /// `active_listings`. The partial `idx_market_listings_active`
    /// index encodes that predicate; this test pins the helper's
    /// behavioural contract end-to-end so a regression that wrote
    /// `WHERE quantity >= 0` (off-by-one) flunks here. Quantity goes
    /// to zero via raw `UPDATE` rather than the upcoming `buy_listing`
    /// helper because 5d hasn't landed yet — the contract under test
    /// is the read filter, not the write path.
    #[test]
    fn active_listings_excludes_zero_quantity_rows() {
        let (_dir, world, seller_id) = world_with_listings();

        let stocked = world
            .create_listing(Some(seller_id), "item.a", "Stocked", 10, 5, None, None)
            .expect("stocked listing inserts");
        let exhausted = world
            .create_listing(Some(seller_id), "item.b", "Exhausted", 10, 1, None, None)
            .expect("exhausted listing inserts");
        world
            .connection()
            .execute(
                "UPDATE market_listings SET quantity = 0 WHERE id = ?1",
                rusqlite::params![exhausted.id],
            )
            .expect("manual quantity zero out");

        let listings = world.active_listings().expect("active_listings runs");
        let ids: Vec<i64> = listings.iter().map(|l| l.id).collect();
        assert_eq!(
            ids,
            vec![stocked.id],
            "exhausted listings must not appear in active_listings; got {ids:?}"
        );
    }

    /// Listings with a lapsed `expires_at` MUST fall out of the
    /// marketplace view. SPEC §4.3 lists `expires_at` in the field
    /// set; the schema docs document the kit's policy of read-time
    /// filtering (no sweeper). Open-ended listings (`NULL`) and
    /// future-dated listings stay visible. Using `datetime('now',
    /// modifier)` SQL fixtures keeps the test deterministic without
    /// a stubbed clock.
    #[test]
    fn active_listings_filters_expired_and_keeps_open_ended() {
        let (_dir, world, seller_id) = world_with_listings();

        // Future deadline — must be visible.
        let future_iso: String = world
            .connection()
            .query_row("SELECT datetime('now', '+1 hour')", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("future timestamp computes");
        let future = world
            .create_listing(
                Some(seller_id),
                "item.future",
                "Future",
                10,
                1,
                Some(&future_iso),
                None,
            )
            .expect("future listing inserts");

        // Past deadline — must be hidden.
        let past_iso: String = world
            .connection()
            .query_row("SELECT datetime('now', '-1 hour')", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("past timestamp computes");
        let past = world
            .create_listing(
                Some(seller_id),
                "item.past",
                "Past",
                10,
                1,
                Some(&past_iso),
                None,
            )
            .expect("past listing inserts");

        // No deadline — must be visible.
        let open = world
            .create_listing(Some(seller_id), "item.open", "Open", 10, 1, None, None)
            .expect("open-ended listing inserts");

        let listings = world.active_listings().expect("active_listings runs");
        let visible_ids: std::collections::HashSet<i64> = listings.iter().map(|l| l.id).collect();
        assert!(
            visible_ids.contains(&future.id),
            "future-dated listing must be visible"
        );
        assert!(
            visible_ids.contains(&open.id),
            "open-ended listing must be visible"
        );
        assert!(
            !visible_ids.contains(&past.id),
            "lapsed listing must be filtered out"
        );
    }

    /// SPEC_v3 §Task 5d happy path: buying decrements the listing's
    /// `quantity` by exactly the requested amount, returns the post-
    /// decrement row, and the new value is durably visible to a
    /// follow-up read. Pinning a primary-key SELECT after the call
    /// means a regression that returned a stale view (e.g. read the
    /// row before the UPDATE) flunks here, not in production where the
    /// inconsistency would manifest as a buy that "didn't take" the
    /// next time the marketplace screen renders.
    #[test]
    fn buy_listing_decrements_quantity_and_returns_post_row() {
        let (_dir, mut world, seller_id) = world_with_listings();

        let listing = world
            .create_listing(
                Some(seller_id),
                "item.lockpick",
                "Lockpick",
                25,
                5,
                None,
                None,
            )
            .expect("create_listing succeeds");

        // Buy 2 of 5 — no callback work needed for the decrement test.
        let post = world
            .buy_listing(listing.id, 2, |_tx, _row| Ok(()))
            .expect("buy_listing succeeds with sufficient quantity");

        assert_eq!(post.id, listing.id, "returned row must be the same listing");
        assert_eq!(post.quantity, 3, "5 - 2 = 3");

        // Durability check: a follow-up SELECT MUST see the same value
        // the helper returned, otherwise the helper read pre-UPDATE.
        let durable: i64 = world
            .connection()
            .query_row(
                "SELECT quantity FROM market_listings WHERE id = ?1",
                rusqlite::params![listing.id],
                |row| row.get(0),
            )
            .expect("select quantity");
        assert_eq!(
            durable, 3,
            "post-buy quantity must persist; helper saw {} but DB has {}",
            post.quantity, durable
        );
    }

    /// Buying down to exactly zero MUST succeed (the schema CHECK is
    /// `quantity >= 0`, not `> 0`) and the now-exhausted listing MUST
    /// fall out of `active_listings` because the partial
    /// `idx_market_listings_active` index filters `quantity > 0`. Pins
    /// the boundary value: a regression that off-by-oned the gate
    /// (`quantity > N` instead of `>= N`) would flunk on the buy, and a
    /// regression that dropped the partial-index predicate would flunk
    /// on the post-buy `active_listings` assertion.
    #[test]
    fn buy_listing_to_zero_exhausts_listing_and_hides_it_from_active() {
        let (_dir, mut world, seller_id) = world_with_listings();

        let listing = world
            .create_listing(Some(seller_id), "item.last", "Last One", 10, 3, None, None)
            .expect("create_listing succeeds");

        let post = world
            .buy_listing(listing.id, 3, |_tx, _row| Ok(()))
            .expect("buy_listing of exact remaining quantity must succeed");
        assert_eq!(post.quantity, 0, "buying every unit leaves quantity at 0");

        let active = world.active_listings().expect("active_listings runs");
        assert!(
            !active.iter().any(|l| l.id == listing.id),
            "exhausted listing must drop out of active_listings"
        );
    }

    /// Non-positive buy quantities are typed errors at the boundary,
    /// caught **before** the transaction opens (the helper docs pin
    /// this as a deliberate design choice — a broken UI shouldn't pay
    /// for a `BEGIN`). Both `0` and a negative number must surface as
    /// [`MarketError::NonPositiveBuyQuantity`] with the offending
    /// value echoed back, and the listing's quantity must be unchanged.
    #[test]
    fn buy_listing_rejects_non_positive_quantity() {
        let (_dir, mut world, seller_id) = world_with_listings();

        let listing = world
            .create_listing(Some(seller_id), "item.x", "Item X", 1, 5, None, None)
            .expect("create_listing succeeds");

        for bad in [0_i64, -1, -100] {
            let err = world
                .buy_listing(listing.id, bad, |_tx, _row| Ok(()))
                .expect_err("non-positive buy quantity must be rejected");
            match err {
                MarketError::NonPositiveBuyQuantity { actual } => assert_eq!(actual, bad),
                other => panic!("expected NonPositiveBuyQuantity for {bad}, got {other:?}"),
            }
        }

        // The listing's quantity must be unchanged across all rejected
        // buys — otherwise a regression that "validated after BEGIN"
        // would have flunked silently against an absent assertion.
        let still: i64 = world
            .connection()
            .query_row(
                "SELECT quantity FROM market_listings WHERE id = ?1",
                rusqlite::params![listing.id],
                |row| row.get(0),
            )
            .expect("select quantity");
        assert_eq!(still, 5, "rejected buys must not touch the listing");
    }

    /// A buy targeting a listing id that doesn't exist surfaces as
    /// [`MarketError::NotFound`]. The diagnostic SELECT runs inside
    /// the wrapping transaction (see helper docs) so the discrimination
    /// between "missing" and "insufficient" is consistent even if a
    /// hostile concurrent writer were trying to race the listing in.
    #[test]
    fn buy_listing_rejects_missing_listing() {
        let (_dir, mut world, _seller) = world_with_listings();

        let err = world
            .buy_listing(9_999, 1, |_tx, _row| Ok(()))
            .expect_err("missing listing id must be rejected");
        match err {
            MarketError::NotFound { id } => assert_eq!(id, 9_999),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// Buying more units than the listing has available surfaces as
    /// [`MarketError::InsufficientQuantity`] with both the requested
    /// and the actual on-row quantity echoed. Pinning the row-unchanged
    /// invariant here means a regression that wrote *before* checking
    /// (e.g. unconditional `quantity = quantity - ?` then a compensating
    /// read) would observably flunk on the post-call SELECT.
    #[test]
    fn buy_listing_rejects_insufficient_quantity() {
        let (_dir, mut world, seller_id) = world_with_listings();

        let listing = world
            .create_listing(Some(seller_id), "item.rare", "Rare", 100, 3, None, None)
            .expect("create_listing succeeds");

        let err = world
            .buy_listing(listing.id, 10, |_tx, _row| {
                panic!("callback must not run when quantity is insufficient");
            })
            .expect_err("insufficient quantity must be rejected");
        match err {
            MarketError::InsufficientQuantity {
                id,
                requested,
                available,
            } => {
                assert_eq!(id, listing.id);
                assert_eq!(requested, 10);
                assert_eq!(available, 3);
            }
            other => panic!("expected InsufficientQuantity, got {other:?}"),
        }

        let still: i64 = world
            .connection()
            .query_row(
                "SELECT quantity FROM market_listings WHERE id = ?1",
                rusqlite::params![listing.id],
                |row| row.get(0),
            )
            .expect("select quantity");
        assert_eq!(
            still, 3,
            "rejected over-buy must leave the listing's quantity untouched"
        );
    }

    /// The buyer callback receives the **post-decrement** listing row,
    /// not the pre-decrement view. Game code that uses the listing's
    /// current quantity in the callback (e.g. "if this was the last
    /// one, also award the achievement") relies on this contract, so
    /// pin it explicitly — a regression that swapped to passing the
    /// pre-decrement row would flunk here. Also pins that the callback
    /// runs inside the same transaction by issuing a writes-itself
    /// statement and asserting it's durably committed after the call
    /// returns successfully.
    #[test]
    fn buy_listing_callback_runs_in_transaction_with_post_decrement_listing() {
        let (_dir, mut world, seller_id) = world_with_listings();
        let listing = world
            .create_listing(Some(seller_id), "item.k", "Item K", 50, 4, None, None)
            .expect("create_listing succeeds");

        // Game code's stand-in: a tiny scratch table the callback
        // writes to. If the callback ran outside the transaction or
        // didn't commit, the write would be missing after the call.
        world
            .connection()
            .execute(
                "CREATE TABLE buy_callback_log (\
                    listing_id     INTEGER NOT NULL,\
                    post_quantity  INTEGER NOT NULL\
                )",
                [],
            )
            .expect("scratch table created");

        let _post = world
            .buy_listing(listing.id, 1, |tx, row| {
                tx.execute(
                    "INSERT INTO buy_callback_log (listing_id, post_quantity) VALUES (?1, ?2)",
                    rusqlite::params![row.id, row.quantity],
                )?;
                Ok(())
            })
            .expect("buy_listing with successful callback");

        let (logged_id, logged_qty): (i64, i64) = world
            .connection()
            .query_row(
                "SELECT listing_id, post_quantity FROM buy_callback_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("callback row durable after commit");
        assert_eq!(logged_id, listing.id);
        assert_eq!(
            logged_qty, 3,
            "callback must observe the post-decrement quantity (4 - 1 = 3)"
        );
    }
}
