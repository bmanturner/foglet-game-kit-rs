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

use crate::world_db::WorldMigration;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
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
}
