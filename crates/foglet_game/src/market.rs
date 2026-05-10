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
}
