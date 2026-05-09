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

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

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

/// Decoded `notices` row — SPEC_v3 §4.1 read model.
///
/// Mirrors the column shape pinned by [`NOTICES_MIGRATION`] one-for-one,
/// in the same order, so the SQL `RETURNING` clause and the `query_map`
/// row decoder share a single column list. Authoring code consumes
/// this struct rather than reaching into raw `rusqlite::Row`s — that
/// keeps the schema-to-Rust mapping in one place and turns a column
/// rename into a single compile error instead of a fan-out of decode
/// failures.
///
/// All timestamps stay as raw SQLite ISO text, the same contract as
/// [`crate::events::EventRecord`]: parsing into a richer type would be
/// a one-way trip that hides corrupt data and forces a chrono / time
/// dependency on every consumer. `metadata` is likewise opaque text —
/// game code that wants structured metadata serialises JSON before
/// handing it to [`WorldDb::send_notice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Autoincrement primary key. Doubles as the deterministic
    /// tiebreaker for the inbox query (Task 3d) when two notices share
    /// a `created_at` value at second resolution.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text — see struct docs.
    pub created_at: String,
    /// Sender's `players.id`, or `None` for system notices. SPEC §4.1
    /// explicitly allows "System notices MAY have no sender".
    pub sender_player_id: Option<i64>,
    /// Recipient's `players.id`. Required by the schema — every notice
    /// has exactly one addressee.
    pub recipient_player_id: i64,
    /// Game-authored kind label (e.g. `"guestbook_note"`,
    /// `"challenge_offer"`). Round-tripped verbatim; the kit imposes
    /// no namespace.
    pub kind: String,
    /// Player- or system-authored short title. Length bounds are
    /// enforced by Task 3c, not at the storage layer.
    pub subject: String,
    /// Player- or system-authored body. Length bounds are enforced by
    /// Task 3c, not at the storage layer.
    pub body: String,
    /// ISO timestamp the recipient first marked the notice as read, or
    /// `None` while it is still unread. Flipped to a timestamp by the
    /// idempotent Task 3e helper.
    pub read_at: Option<String>,
    /// ISO timestamp the recipient archived the notice, or `None` if
    /// it is still in the active inbox. Flipped to a timestamp by the
    /// Task 3f helper.
    pub archived_at: Option<String>,
    /// Optional ISO timestamp after which the notice is considered
    /// expired. The kit does not auto-purge in v3 (see module docs);
    /// game-authored or future kit code may filter on this.
    pub expires_at: Option<String>,
    /// Optional opaque metadata blob (typically a JSON object). Stored
    /// as text so `sqlite3 -json` can pretty-print it; the kit does
    /// not parse it.
    pub metadata: Option<String>,
}

/// Failure modes for [`WorldDb::send_notice`].
///
/// Library-internal `thiserror` shape — the runtime wraps these with
/// `anyhow` at the process boundary. Mirrors [`crate::events::EventError`]
/// so all world-DB write paths surface errors with the same shape.
///
/// Task 3b only emits [`NoticeError::Sqlite`]; Task 3c will add
/// length-validation variants (`EmptySubject`, `SubjectTooLong`,
/// `BodyTooLong`, …) without disturbing the call signature — they slot
/// in as additional `#[error]` arms before the `Sqlite` round-trip
/// runs. Carving out the error type now means 3c is a non-breaking
/// change to consumers.
#[derive(Debug, Error)]
pub enum NoticeError {
    /// The `INSERT … RETURNING` round-trip failed. Wrapping
    /// `rusqlite::Error` keeps the call site readable (one error type,
    /// one mapping) while preserving the underlying cause for
    /// `tracing` and operator-facing messages.
    #[error("failed to write notice to world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the insert statement.
        #[source]
        source: rusqlite::Error,
    },
}

impl WorldDb {
    /// Insert one row into `notices` and return the canonical
    /// [`Notice`] SQLite produced (SPEC_v3 §4.1 / §Task 3b).
    ///
    /// The contract is "the notice I asked you to send is now durably
    /// in the recipient's mailbox, with the id and `created_at` SQLite
    /// assigned, and `read_at` / `archived_at` both still `NULL`". A
    /// freshly sent notice MUST be unread — the test
    /// `send_notice_stores_unread_notice` pins both fields explicitly
    /// so a future schema or default-value change can't silently flip
    /// the lifecycle.
    ///
    /// Length validation is intentionally **not** performed here —
    /// SPEC_v3 §Task 3c owns the empty-subject / overlong-body guard
    /// and lands in the next iteration. The schema's `NOT NULL`
    /// constraints on `subject` and `body` are still in force, so a
    /// caller who somehow passes a literal `""` will land a row with
    /// an empty string (legal at the storage layer); 3c will reject
    /// that case at the kit boundary before the SQL round-trip.
    ///
    /// `kind`, `subject`, and `body` are required by the schema.
    /// `sender_player_id` is `Option<i64>` because system notices have
    /// no attributable sender (§4.1). `expires_at` is the only optional
    /// timestamp on the *write* path: `read_at` and `archived_at` are
    /// always `NULL` at send time and get filled by their dedicated
    /// helpers. `metadata` is opaque text, same contract as
    /// [`crate::events::EventRecord::metadata`].
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) to read the
    /// canonical row — `id`, the SQL-side `created_at`, plus every
    /// other column — without a second round-trip, the same pattern
    /// as [`Self::append_event`] and [`Self::upsert_player`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single insert statement under the configured
    /// busy timeout. `&mut self` would fight the runtime layer where
    /// `GameContext` borrows the world DB once per tick.
    #[allow(clippy::too_many_arguments)] // Matches the SPEC §4.1 column shape one-for-one (sender, recipient, kind, subject, body, expires_at, metadata); bundling into a struct would force every call site through a builder dance without adding type safety, since each parameter is already strongly typed.
    pub fn send_notice(
        &self,
        sender_player_id: Option<i64>,
        recipient_player_id: i64,
        kind: &str,
        subject: &str,
        body: &str,
        expires_at: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<Notice, NoticeError> {
        // `RETURNING` echoes the full row back — including the SQL-side
        // `CURRENT_TIMESTAMP` default for `created_at` and the `NULL`
        // values for `read_at` / `archived_at`. The column order here
        // matches `row_to_notice` and the inbox query in Task 3d so all
        // three share one decoder.
        const SQL: &str = "\
INSERT INTO notices \
    (sender_player_id, recipient_player_id, kind, subject, body, expires_at, metadata) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
RETURNING id, created_at, sender_player_id, recipient_player_id, \
          kind, subject, body, read_at, archived_at, expires_at, metadata";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    sender_player_id,
                    recipient_player_id,
                    kind,
                    subject,
                    body,
                    expires_at,
                    metadata,
                ],
                row_to_notice,
            )
            .map_err(|source| NoticeError::Sqlite { source })
    }
}

/// Decode a `notices` row into [`Notice`].
///
/// Pulled out so the Task 3b write path and the upcoming Task 3d
/// inbox query can share one decoder. Column order matches the
/// `RETURNING` clause in [`WorldDb::send_notice`] *and* the inbox
/// `SELECT` (when it lands); a regression that reorders columns will
/// surface here as a type error rather than as a silent field swap.
fn row_to_notice(row: &rusqlite::Row<'_>) -> rusqlite::Result<Notice> {
    Ok(Notice {
        id: row.get(0)?,
        created_at: row.get(1)?,
        sender_player_id: row.get(2)?,
        recipient_player_id: row.get(3)?,
        kind: row.get(4)?,
        subject: row.get(5)?,
        body: row.get(6)?,
        read_at: row.get(7)?,
        archived_at: row.get(8)?,
        expires_at: row.get(9)?,
        metadata: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::players::PLAYERS_MIGRATION;
    use tempfile::tempdir;

    /// Helper: build a [`FogletContext`] just complete enough for
    /// `upsert_player` to land a row. Tests don't care about
    /// `terminal_*` or `session_id`, so this keeps test bodies focused
    /// on the notice behavior under test.
    fn ctx(user_id: &str, username: &str) -> FogletContext {
        FogletContext {
            door_id: "test-door".to_string(),
            user_id: Some(user_id.to_string()),
            username: Some(username.to_string()),
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::ContextFile,
        }
    }

    /// Helper: open a fresh world DB with players + notices migrations
    /// applied. Both Task 3b tests need this setup; pulling it out
    /// keeps each test body focused on the assertion under test.
    fn world_with_notices() -> (tempfile::TempDir, WorldDb) {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&NOTICES_MIGRATION)
            .expect("notices migration applies");
        (dir, world)
    }

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

    /// SPEC_v3 §Task 3b acceptance: a freshly sent notice is durably
    /// stored AND is unread. The "stored" half asserts the round-tripped
    /// `Notice` matches what was sent (id assigned, fields preserved);
    /// the "unread" half pins `read_at` and `archived_at` both to
    /// `None`. SPEC §4.1 implicitly requires this — a notice that was
    /// born "read" or "archived" would never surface in any inbox query
    /// and the lifecycle would be broken from the start.
    ///
    /// We also verify the row is visible by primary key directly, so a
    /// regression that returned a `Notice` from `RETURNING` without
    /// actually persisting (e.g. a future change that wrapped the
    /// insert in a transaction and forgot to commit) would flunk here
    /// rather than only at the Task 3d inbox query.
    #[test]
    fn send_notice_stores_unread_notice() {
        let (_dir, world) = world_with_notices();
        let alice = world
            .upsert_player(&ctx("u-alice", "alice"))
            .expect("alice upsert succeeds");
        let bob = world
            .upsert_player(&ctx("u-bob", "bob"))
            .expect("bob upsert succeeds");

        let sent = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Welcome to the motel",
                "Stop by Room 7. There's a clue under the rug.",
                None,
                None,
            )
            .expect("send_notice succeeds");

        // Returned record reflects the inputs and SQL-side defaults.
        assert!(sent.id > 0, "RETURNING must echo an autoincrement id");
        assert!(
            !sent.created_at.is_empty(),
            "RETURNING must echo CURRENT_TIMESTAMP"
        );
        assert_eq!(sent.sender_player_id, Some(alice.id));
        assert_eq!(sent.recipient_player_id, bob.id);
        assert_eq!(sent.kind, "guestbook_note");
        assert_eq!(sent.subject, "Welcome to the motel");
        assert_eq!(sent.body, "Stop by Room 7. There's a clue under the rug.");

        // The unread / unarchived contract — the central Task 3b claim.
        assert!(
            sent.read_at.is_none(),
            "freshly sent notice must be unread (read_at IS NULL)"
        );
        assert!(
            sent.archived_at.is_none(),
            "freshly sent notice must not be archived (archived_at IS NULL)"
        );
        assert!(sent.expires_at.is_none());
        assert!(sent.metadata.is_none());

        // Round-trip through a direct primary-key SELECT to prove the
        // row really landed in the table — guards against a future
        // refactor that returns the `Notice` from `RETURNING` without
        // actually committing the insert.
        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, sender_player_id, recipient_player_id, \
                        kind, subject, body, read_at, archived_at, expires_at, metadata \
                 FROM notices WHERE id = ?1",
                rusqlite::params![sent.id],
                row_to_notice,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, sent, "stored row must equal RETURNING row");
    }

    /// SPEC §4.1: "System notices MAY have no sender." A `None`
    /// `sender_player_id` MUST round-trip as `NULL` and produce a
    /// notice that is otherwise indistinguishable from a player-sent
    /// one — same unread/unarchived contract. Pinning this here keeps
    /// the system-advisory pathway honest; without an explicit test, a
    /// future signature change that "helpfully" defaulted the sender to
    /// some sentinel id would silently break §4.1.
    ///
    /// Also exercises the optional `expires_at` and `metadata`
    /// parameters — the Task 3b signature accepts them, so a smoke
    /// that they round-trip belongs here, not in a later 3d/3e test.
    #[test]
    fn send_notice_supports_system_sender_and_optional_fields() {
        let (_dir, world) = world_with_notices();
        let recipient = world
            .upsert_player(&ctx("u-recipient", "recipient"))
            .expect("recipient upsert succeeds");

        let sent = world
            .send_notice(
                None,
                recipient.id,
                "system_advisory",
                "Door reset reminder",
                "The motel resets at midnight UTC.",
                Some("2026-12-31T23:59:59Z"),
                Some(r#"{"severity":"info"}"#),
            )
            .expect("system send_notice succeeds");

        assert_eq!(
            sent.sender_player_id, None,
            "system notices must store NULL sender per SPEC §4.1"
        );
        assert_eq!(sent.recipient_player_id, recipient.id);
        assert_eq!(sent.kind, "system_advisory");
        assert_eq!(sent.expires_at.as_deref(), Some("2026-12-31T23:59:59Z"));
        assert_eq!(sent.metadata.as_deref(), Some(r#"{"severity":"info"}"#));
        assert!(
            sent.read_at.is_none() && sent.archived_at.is_none(),
            "system notices must also start unread/unarchived"
        );
    }
}
