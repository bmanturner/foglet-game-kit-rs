//! `notices` — shared-world player mail/notice schema (SPEC_v3 §4.1 /
//! §Task 3a).
//!
//! v3 introduces durable async player-to-player mail: leave a note for
//! another investigator, drop a system advisory in a player's inbox,
//! mark it read, archive it. The whole feature sits on top of one
//! `notices` table whose shape is pinned by [`NOTICES_MIGRATION`]. This
//! module exists only to declare that schema and prove it applies; the
//! `Notice` Rust type and the `send_notice` / `inbox` / `mark_read` /
//! `archive_notice` helpers land in subsequent §Task 3 sub-items
//! (3b–3f). Splitting the migration into its own commit keeps the
//! bisect signal sharp — a column rename or dropped index flunks the
//! schema test in this module rather than a higher-level behavioural
//! test that's harder to attribute.
//!
//! # Why a dedicated table
//!
//! SPEC_v3 §3 lists notices alongside challenges, market listings,
//! factions, and bounties as separate primitives. We follow the v2
//! convention of "one migration per top-level concept, named
//! `create_<table>`" (see [`crate::events`] for the canonical
//! example): one table, one migration, one named index family. Folding
//! notices onto the `world_events` log would conflate the
//! append-only, never-mutated event stream with mailbox state that
//! mutates (`read_at`, `archived_at`) — a fundamentally different
//! lifecycle.
//!
//! # Why `version = 6`
//!
//! v2 occupies migration versions 1–5 (see
//! `docs/shared-world.md` §8.1). v3 claims `6` and above, dense and
//! grouped per primitive. Notices are the first v3 primitive to land,
//! so they take version 6. Subsequent v3 migrations (challenges,
//! market listings, factions, bounties) MUST pick the next available
//! kit version — game-authored migrations live in their own higher
//! band and are not affected.

use crate::world_db::WorldMigration;

/// Schema for the player-mail table — SPEC_v3 §4.1 / §Task 3a.
///
/// One row per notice. Notices are mutable in the narrow sense that
/// `read_at` and `archived_at` are flipped from `NULL` to a timestamp
/// once, by an idempotent helper (Task 3e/3f); the body, subject, and
/// addressing fields are write-once. The kit's contract is "if you only
/// go through the public API, the only mutations are `mark_read` and
/// `archive_notice`, both idempotent". An operator with `sqlite3` can
/// of course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid. Doubles
///   as the deterministic tiebreaker for the inbox query (Task 3d) when
///   two notices share the same `created_at` second.
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. Stored as ISO text so
///   it sorts lexically the same way it sorts chronologically and
///   reads cleanly in the `sqlite3` CLI.
/// - `sender_player_id` — `INTEGER REFERENCES players(id)`,
///   **nullable**. SPEC §4.1 explicitly allows system notices with no
///   sender ("System notices MAY have no sender"). Foreign-keyed for
///   the same reason as the turn ledger and event log: a phantom id
///   should never land here. SQLite enforces FKs only when `PRAGMA
///   foreign_keys = ON`, which the runtime is responsible for; until
///   then the constraint is documentation but the column shape is
///   correct.
/// - `recipient_player_id` — `INTEGER NOT NULL REFERENCES players(id)`.
///   Every notice has exactly one addressee. Group/broadcast notices
///   are intentionally not modelled at the schema layer: a "send to
///   N players" helper would loop and insert N rows, keeping the read
///   path (inbox query) trivial.
/// - `kind` — `TEXT NOT NULL`. Short machine-readable label
///   (e.g. `"guestbook_note"`, `"challenge_offer"`). Game authors pick
///   the namespace; the kit's only rule is "round-trips as text". Used
///   by Task 3-consuming screens to filter inbox views by category.
/// - `subject` — `TEXT NOT NULL`. Player-authored short title. Length
///   bounds are enforced by Task 3c, not at the schema layer, because
///   the cap lives in `[multiplayer]` config (`max_notice_body_chars`
///   covers body; subject gets its own kit-internal cap). A schema-
///   level `CHECK` would be hostile to future config tuning.
/// - `body` — `TEXT NOT NULL`. Player-authored body. Same length-cap
///   story as `subject`. Both fields are required at the schema layer
///   so the kit can render any inbox row without a fallback string;
///   "empty body" failures surface at write time (Task 3c) rather than
///   silently producing a blank cell in the UI.
/// - `read_at` — `TEXT`, nullable. ISO timestamp the player marked the
///   notice as read, or `NULL` if it's still unread. SPEC §4.1 requires
///   "Reading a notice MUST be idempotent"; storing the first-read
///   timestamp lets the helper detect "already read" without a
///   separate boolean.
/// - `archived_at` — `TEXT`, nullable. ISO timestamp the notice was
///   archived. The default inbox query (Task 3d) will exclude rows
///   where this is non-null; the partial index below makes that
///   exclusion seek-bound.
/// - `expires_at` — `TEXT`, nullable. SPEC §4.1 lists optional expiry.
///   The kit does not auto-purge expired notices in v3; expiry is a
///   filter the inbox query may apply (a future Task 3 sub-item, or
///   game-author code). Storing it now keeps the schema stable.
/// - `metadata` — `TEXT`, nullable. Optional opaque JSON. Stored as
///   text rather than `BLOB` so an operator can pretty-print it with
///   `sqlite3 -json`; the kit treats this column as opaque, the same
///   contract as [`crate::events::WORLD_EVENTS_MIGRATION`]'s
///   `metadata` column.
///
/// # Indexes
///
/// Two covering indexes are created up-front so the inbox query
/// patterns Task 3d–3f rely on are seek-bound from the moment they
/// land. Adding them later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the index
/// cost at the same migration that creates the table.
///
/// - `idx_notices_inbox` is a partial index over
///   `(recipient_player_id, created_at, id)` `WHERE archived_at IS
///   NULL`. The partial predicate keeps the index small (archived
///   notices are excluded) and matches the default inbox query
///   exactly: `WHERE recipient_player_id = ? AND archived_at IS NULL
///   ORDER BY created_at DESC, id DESC`. Same shape and rationale as
///   `idx_world_events_player_recent` from
///   [`crate::events::WORLD_EVENTS_MIGRATION`].
/// - `idx_notices_recipient_all` covers
///   `(recipient_player_id, created_at, id)` without the partial
///   predicate, for the "show me everything in my mailbox including
///   archived" view a future screen might surface (and for operator
///   audits via `sqlite3`). Two indexes is cheap on the low-cardinality
///   data v3 expects, and the partial-vs-full split keeps the default
///   path's index as small as possible.
///
/// # Version
///
/// `version = 6`. v2 occupies 1–5 (see `docs/shared-world.md` §8.1);
/// v3 claims 6+. Notices are the first v3 primitive, so they take 6.
pub const NOTICES_MIGRATION: WorldMigration = WorldMigration {
    version: 6,
    name: "create_notices",
    sql: "\
CREATE TABLE IF NOT EXISTS notices (\n\
    id                  INTEGER PRIMARY KEY,\n\
    created_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    sender_player_id    INTEGER REFERENCES players(id),\n\
    recipient_player_id INTEGER NOT NULL REFERENCES players(id),\n\
    kind                TEXT NOT NULL,\n\
    subject             TEXT NOT NULL,\n\
    body                TEXT NOT NULL,\n\
    read_at             TEXT,\n\
    archived_at         TEXT,\n\
    expires_at          TEXT,\n\
    metadata            TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_notices_inbox\n\
    ON notices(recipient_player_id, created_at, id) WHERE archived_at IS NULL;\n\
CREATE INDEX IF NOT EXISTS idx_notices_recipient_all\n\
    ON notices(recipient_player_id, created_at, id);\n\
",
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v3 §Task 3a acceptance: applying [`NOTICES_MIGRATION`]
    /// creates the documented `notices` table with the column shape
    /// SPEC §4.1 pins. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.1 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 3b/3d behavioural test).
    ///
    /// The players migration is applied first because `notices`
    /// references `players(id)` via two foreign keys. With FK
    /// enforcement off (the SQLite default until the runtime turns it
    /// on) the migration would succeed even without the parent table,
    /// but exercising the real dependency order here mirrors how the
    /// runtime startup path drives migrations on a real door open.
    #[test]
    fn migration_creates_notices_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("notices migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'notices'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(count, 1, "notices table must exist after migration applies");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('notices') ORDER BY cid")
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
                "sender_player_id".to_string(),
                "recipient_player_id".to_string(),
                "kind".to_string(),
                "subject".to_string(),
                "body".to_string(),
                "read_at".to_string(),
                "archived_at".to_string(),
                "expires_at".to_string(),
                "metadata".to_string(),
            ],
            "notices column shape must match the SPEC_v3 §4.1 contract"
        );
    }

    /// SPEC §4.1 makes `sender_player_id` optional ("System notices
    /// MAY have no sender") and `recipient_player_id` required. A
    /// regression that flipped either nullability would be a silent
    /// behavioural break — system advisories suddenly need a fake
    /// sender, or a notice with no addressee would be silently storable
    /// and never visible in any inbox. Pin both nullabilities
    /// explicitly here.
    #[test]
    fn notices_addressing_nullability_matches_spec() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("notices migration applies");

        // `pragma_table_info`'s `notnull` column is `1` for `NOT NULL`
        // columns and `0` otherwise. Querying it directly is more
        // robust than parsing the `sqlite_master.sql` text, whose
        // whitespace is implementation-defined.
        let sender_notnull: i64 = world
            .connection()
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('notices') \
                 WHERE name = 'sender_player_id'",
                [],
                |row| row.get(0),
            )
            .expect("sender notnull query runs");
        assert_eq!(
            sender_notnull, 0,
            "sender_player_id must be nullable so system notices can have no sender"
        );

        let recipient_notnull: i64 = world
            .connection()
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('notices') \
                 WHERE name = 'recipient_player_id'",
                [],
                |row| row.get(0),
            )
            .expect("recipient notnull query runs");
        assert_eq!(
            recipient_notnull, 1,
            "recipient_player_id must be NOT NULL so every notice has an addressee"
        );
    }

    /// The default inbox query (Task 3d) walks
    /// `idx_notices_inbox` — a partial index over
    /// `(recipient_player_id, created_at, id) WHERE archived_at IS
    /// NULL`. If the migration ever stops creating this index, the
    /// inbox read path silently becomes a table scan; pin its existence
    /// (and the partial predicate) here so a regression flunks at
    /// `cargo test` rather than in production under load.
    #[test]
    fn notices_inbox_partial_index_is_present() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("notices migration applies");

        // `sqlite_master.sql` for an index includes the partial
        // predicate text verbatim, so we can pin both the index name
        // and the `WHERE archived_at IS NULL` clause in one query.
        let sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_notices_inbox'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_notices_inbox");
        assert!(
            sql.contains("archived_at IS NULL"),
            "idx_notices_inbox must be a partial index filtering archived rows; got: {sql}"
        );
        assert!(
            sql.contains("recipient_player_id"),
            "idx_notices_inbox must lead with recipient_player_id; got: {sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the same
    /// migration list every open; v3 inherits that contract. A second
    /// `apply_migration(&NOTICES_MIGRATION)` MUST be a no-op (the
    /// version is already in `world_migrations`), not an error from
    /// `CREATE TABLE` on an existing table.
    #[test]
    fn notices_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("first notices migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("second notices migration applies (idempotent)");
    }
}
