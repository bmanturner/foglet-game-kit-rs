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

/// Kit-internal cap for the subject line of a player-authored notice.
///
/// SPEC_v3 §5.2 ships only `max_notice_body_chars` as configurable; the
/// subject line is intentionally bounded by the kit instead of by game
/// authors so every consuming game presents a uniform "short title" UI.
/// 120 chars fits one 80-column line with room for a recipient prefix
/// and a "(unread)" annotation in inbox views, and sits well below the
/// 1 KB-ish soft limit at which SQLite text scanning stops being
/// instantaneous on a modern disk.
///
/// Counted in Unicode scalar values (`str::chars().count()`), not bytes
/// — SPEC §4.1 / §7 talk in *characters*, and a byte cap would let a
/// single emoji eat four "chars" of budget. Length checks for the body
/// (whose cap lives in `[multiplayer].max_notice_body_chars`) use the
/// same scalar-count rule for consistency.
pub const NOTICE_SUBJECT_MAX_CHARS: usize = 120;

/// Failure modes for [`WorldDb::send_notice`].
///
/// Library-internal `thiserror` shape — the runtime wraps these with
/// `anyhow` at the process boundary. Mirrors [`crate::events::EventError`]
/// so all world-DB write paths surface errors with the same shape.
///
/// Task 3c added the four length-validation variants. They run *before*
/// the SQL round-trip so a rejected notice never touches `world.sqlite`
/// — that keeps `world_events` and the inbox indexes from being
/// polluted by half-validated drafts and lets the caller re-render the
/// authoring screen with the original input intact.
#[derive(Debug, Error)]
pub enum NoticeError {
    /// Subject was empty (or whitespace-only collapsed to empty).
    /// SPEC §4.1 lists `subject` as `NOT NULL`; the kit additionally
    /// rejects an empty string here so the inbox never renders a row
    /// with a blank title that the recipient can't tell apart from a
    /// rendering bug.
    #[error("notice subject must not be empty")]
    EmptySubject,
    /// Body was empty. Same rationale as [`Self::EmptySubject`]:
    /// schema-level `NOT NULL` accepts `""`, but a notice with no body
    /// is indistinguishable from a UI glitch in the recipient's inbox.
    /// Player-authored "I just wanted to wave" notices belong in a
    /// `kind` choice, not in the body.
    #[error("notice body must not be empty")]
    EmptyBody,
    /// Subject exceeded [`NOTICE_SUBJECT_MAX_CHARS`]. Surfacing both the
    /// limit and the actual length lets the authoring screen show
    /// "120 / 137 characters" without re-counting.
    #[error("notice subject exceeds {max}-character limit (got {actual})")]
    SubjectTooLong {
        /// Cap that was breached — currently always
        /// [`NOTICE_SUBJECT_MAX_CHARS`], named so future per-game caps
        /// (if ever introduced) don't break the error shape.
        max: usize,
        /// Actual `chars().count()` of the rejected subject. Reported in
        /// scalar values, the same unit as `max`.
        actual: usize,
    },
    /// Body exceeded the configured cap from
    /// `[multiplayer].max_notice_body_chars`. Same field shape as
    /// [`Self::SubjectTooLong`] so authoring screens can render both
    /// failures with one helper.
    #[error("notice body exceeds {max}-character limit (got {actual})")]
    BodyTooLong {
        /// Cap that was breached — sourced from
        /// `MultiplayerSection::max_notice_body_chars` and threaded into
        /// [`WorldDb::send_notice`] by the caller.
        max: usize,
        /// Actual `chars().count()` of the rejected body, in scalar
        /// values.
        actual: usize,
    },
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
    /// `kind`, `subject`, and `body` are required by the schema.
    /// `sender_player_id` is `Option<i64>` because system notices have
    /// no attributable sender (§4.1). `expires_at` is the only optional
    /// timestamp on the *write* path: `read_at` and `archived_at` are
    /// always `NULL` at send time and get filled by their dedicated
    /// helpers. `metadata` is opaque text, same contract as
    /// [`crate::events::EventRecord::metadata`].
    ///
    /// `max_body_chars` comes from
    /// `MultiplayerSection::max_notice_body_chars` — the SPEC v3 §5.2
    /// configurable cap. It's threaded as an argument rather than
    /// pulled from a global so the same `WorldDb` can serve a multi-
    /// game door in the future without reaching for a singleton config.
    /// The subject cap is fixed at [`NOTICE_SUBJECT_MAX_CHARS`]; see
    /// that constant for why it's kit-internal.
    ///
    /// # Validation order
    ///
    /// Length checks (Task 3c) run **before** the SQL round-trip:
    /// emptiness first, then over-cap, subject before body. A rejected
    /// notice MUST NOT touch `world.sqlite` — that keeps `world_events`
    /// and the inbox indexes free of half-validated drafts and lets the
    /// authoring screen re-render the original text on failure.
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
    #[allow(clippy::too_many_arguments)] // Matches the SPEC §4.1 column shape one-for-one (sender, recipient, kind, subject, body, expires_at, metadata) plus the per-call body cap; bundling into a struct would force every call site through a builder dance without adding type safety, since each parameter is already strongly typed.
    pub fn send_notice(
        &self,
        sender_player_id: Option<i64>,
        recipient_player_id: i64,
        kind: &str,
        subject: &str,
        body: &str,
        expires_at: Option<&str>,
        metadata: Option<&str>,
        max_body_chars: u32,
    ) -> Result<Notice, NoticeError> {
        // Validation runs before the SQL round-trip so a rejected
        // notice never produces a row, an autoincrement gap, or an
        // event-log entry. Order: emptiness first (cheapest, catches
        // the "blank submit" path), then over-cap. Subject before body
        // so a notice that's both empty-subject and overlong-body
        // surfaces the authoring screen's first input as the failure
        // — that matches keyboard tab order in the planned UI.
        if subject.is_empty() {
            return Err(NoticeError::EmptySubject);
        }
        if body.is_empty() {
            return Err(NoticeError::EmptyBody);
        }
        // Count Unicode scalar values, not bytes — SPEC §4.1/§7 talk
        // in characters, and a byte cap would penalise non-ASCII text.
        let subject_chars = subject.chars().count();
        if subject_chars > NOTICE_SUBJECT_MAX_CHARS {
            return Err(NoticeError::SubjectTooLong {
                max: NOTICE_SUBJECT_MAX_CHARS,
                actual: subject_chars,
            });
        }
        let max_body = max_body_chars as usize;
        let body_chars = body.chars().count();
        if body_chars > max_body {
            return Err(NoticeError::BodyTooLong {
                max: max_body,
                actual: body_chars,
            });
        }
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

    /// Return the recipient's active inbox, newest first
    /// (SPEC_v3 §4.1 / §Task 3d).
    ///
    /// "Active" means `archived_at IS NULL` — archived notices are
    /// hidden by the default view. A future helper can surface the
    /// full history (Task 3f's archive UI may want a "show archived"
    /// toggle); the partial-index split documented on
    /// [`NOTICES_MIGRATION`] already provides the second covering
    /// index for that path. Reading vs. unread is *not* a filter here:
    /// inbox views typically render both, with unread rows styled
    /// differently. `read_at` is a column on the returned [`Notice`],
    /// so callers can filter or style without a second query.
    ///
    /// # Ordering
    ///
    /// `ORDER BY created_at DESC, id DESC` — newest first, with the
    /// autoincrement `id` as the deterministic tiebreaker when two
    /// notices land in the same SQLite second. Same shape as
    /// [`Self::recent_events`] / [`Self::player_events`] so the inbox
    /// and event log feel consistent in the UI.
    ///
    /// The `idx_notices_inbox` partial index covers
    /// `(recipient_player_id, created_at, id) WHERE archived_at IS
    /// NULL` exactly — the planner can satisfy this query with a
    /// reverse index walk and no residual filter.
    ///
    /// # Parameters
    ///
    /// `recipient_player_id` is the canonical id from
    /// [`crate::players::PlayerRecord`]. Passing an unknown id is not
    /// an error: it returns an empty vec, the correct UI behaviour for
    /// "this player has no notices yet". No limit parameter — SPEC §4.1
    /// scopes notices "to one game world DB" and the v3 mailbox is
    /// expected to stay small (per-recipient, archive on read); a
    /// future paged variant can be added without breaking this
    /// signature.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single statement under the configured busy
    /// timeout, same as [`Self::send_notice`].
    pub fn inbox(&self, recipient_player_id: i64) -> Result<Vec<Notice>, NoticeError> {
        // Column order matches `row_to_notice` and the `RETURNING`
        // clause in `send_notice` — one decoder, one column list,
        // surfaced as a type error if a future schema edit ever
        // diverges them.
        const SQL: &str = "\
SELECT id, created_at, sender_player_id, recipient_player_id, \
       kind, subject, body, read_at, archived_at, expires_at, metadata \
FROM notices \
WHERE recipient_player_id = ?1 AND archived_at IS NULL \
ORDER BY created_at DESC, id DESC";

        let mut stmt = self
            .connection()
            .prepare(SQL)
            .map_err(|source| NoticeError::Sqlite { source })?;
        let rows = stmt
            .query_map(rusqlite::params![recipient_player_id], row_to_notice)
            .map_err(|source| NoticeError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
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
                1_000,
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
                1_000,
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

    /// SPEC_v3 §Task 3c: an empty subject is rejected at the kit
    /// boundary, before the SQL round-trip. The schema's `NOT NULL`
    /// would accept `""`; the kit refuses so the inbox can never render
    /// a row with a blank title that a recipient can't tell apart from
    /// a render bug. Also pins that no row reaches `notices` on
    /// rejection — a regression that validated *after* the insert
    /// would flunk the count assertion.
    #[test]
    fn send_notice_rejects_empty_subject() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let err = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "",
                "body",
                None,
                None,
                1_000,
            )
            .expect_err("empty subject must be rejected");
        assert!(matches!(err, NoticeError::EmptySubject), "got {err:?}");

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM notices", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected notice must not produce a row");
    }

    /// SPEC_v3 §Task 3c: an empty body is rejected for the same
    /// reason as an empty subject — silent blank rendering would
    /// be a UX hazard. Pinning this independently from
    /// `send_notice_rejects_empty_subject` keeps the two failure
    /// modes from masking each other.
    #[test]
    fn send_notice_rejects_empty_body() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let err = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Hi",
                "",
                None,
                None,
                1_000,
            )
            .expect_err("empty body must be rejected");
        assert!(matches!(err, NoticeError::EmptyBody), "got {err:?}");
    }

    /// Boundary: a subject of exactly [`NOTICE_SUBJECT_MAX_CHARS`]
    /// scalar values MUST succeed. Pins the off-by-one; flips to
    /// `<` instead of `<=` (or vice versa) would flunk this test
    /// or the over-cap test below, never both at once.
    #[test]
    fn send_notice_accepts_subject_at_limit() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let subject: String = "a".repeat(NOTICE_SUBJECT_MAX_CHARS);

        let sent = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                &subject,
                "body",
                None,
                None,
                1_000,
            )
            .expect("subject exactly at the cap must be accepted");
        assert_eq!(sent.subject.chars().count(), NOTICE_SUBJECT_MAX_CHARS);
    }

    /// Boundary: a subject of [`NOTICE_SUBJECT_MAX_CHARS`] + 1 scalar
    /// values MUST be rejected with [`NoticeError::SubjectTooLong`],
    /// and the error MUST report both the cap and the actual length so
    /// authoring screens can render "120 / 121 characters" without
    /// re-counting.
    #[test]
    fn send_notice_rejects_subject_over_limit() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let subject: String = "a".repeat(NOTICE_SUBJECT_MAX_CHARS + 1);

        let err = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                &subject,
                "body",
                None,
                None,
                1_000,
            )
            .expect_err("subject over the cap must be rejected");
        match err {
            NoticeError::SubjectTooLong { max, actual } => {
                assert_eq!(max, NOTICE_SUBJECT_MAX_CHARS);
                assert_eq!(actual, NOTICE_SUBJECT_MAX_CHARS + 1);
            }
            other => panic!("expected SubjectTooLong, got {other:?}"),
        }
    }

    /// Boundary: a body of exactly `max_body_chars` scalar values MUST
    /// succeed. Uses a small cap (8) to keep the test text readable;
    /// the production cap is 1000 but the off-by-one is the same.
    #[test]
    fn send_notice_accepts_body_at_limit() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let body: String = "x".repeat(8);
        let sent = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Hi",
                &body,
                None,
                None,
                8,
            )
            .expect("body exactly at the cap must be accepted");
        assert_eq!(sent.body.chars().count(), 8);
    }

    /// SPEC_v3 §Task 3c headline test: an overlong body fails clearly.
    /// Asserts the error names both the cap and the actual length, in
    /// scalar values (Unicode scalars, not bytes — see
    /// [`NOTICE_SUBJECT_MAX_CHARS`] doc comment for why). Uses a
    /// non-ASCII body to prove byte-vs-char correctness: 5 emoji =
    /// 5 chars (passes a cap of 5) but 20 bytes (would fail a byte
    /// cap). The intent here is the over-limit case; we assert
    /// rejection at cap+1 chars.
    #[test]
    fn send_notice_rejects_overlong_body() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let body: String = "x".repeat(9);
        let err = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Hi",
                &body,
                None,
                None,
                8,
            )
            .expect_err("body over the cap must be rejected");
        match err {
            NoticeError::BodyTooLong { max, actual } => {
                assert_eq!(max, 8);
                assert_eq!(actual, 9);
            }
            other => panic!("expected BodyTooLong, got {other:?}"),
        }

        // Sanity: the cap is in characters, not bytes. 4 emoji = 4
        // scalar values but 16 bytes; under a char-cap of 4 this
        // succeeds, proving a byte regression would be caught.
        let emoji = "🦀🦀🦀🦀";
        assert_eq!(emoji.chars().count(), 4);
        assert_eq!(emoji.len(), 16);
        world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Hi",
                emoji,
                None,
                None,
                4,
            )
            .expect("4-emoji body must pass a 4-char cap (chars, not bytes)");
    }

    /// SPEC_v3 §Task 3d: an empty inbox returns an empty vec, not an
    /// error. A new player with no notices is the dominant first-login
    /// case — surfacing an error there would force every UI consumer
    /// to special-case the empty path.
    #[test]
    fn inbox_returns_empty_vec_for_player_with_no_notices() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();

        let notices = world.inbox(alice.id).expect("inbox runs");
        assert!(notices.is_empty(), "fresh inbox must be empty");
    }

    /// SPEC_v3 §Task 3d headline: the inbox returns the recipient's
    /// notices newest first, with `id` breaking ties when two notices
    /// land in the same SQLite second. Mirrors the
    /// `recent_events_returns_newest_first_with_id_tiebreak` contract
    /// for the event log so the two feeds feel consistent in the UI.
    ///
    /// Also pins the recipient scope: a notice addressed to `bob`
    /// MUST NOT appear in `alice`'s inbox. A regression that dropped
    /// the `WHERE recipient_player_id = ?` clause would silently leak
    /// every player's mail to every player.
    #[test]
    fn inbox_returns_newest_first_scoped_to_recipient() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        // Three notices to bob, sent in order — created_at defaults
        // to CURRENT_TIMESTAMP (second resolution), so the id
        // tiebreaker MUST kick in to pin the order.
        let n1 = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "k",
                "first",
                "b1",
                None,
                None,
                1_000,
            )
            .unwrap();
        let n2 = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "k",
                "second",
                "b2",
                None,
                None,
                1_000,
            )
            .unwrap();
        let n3 = world
            .send_notice(None, bob.id, "k", "third (system)", "b3", None, None, 1_000)
            .unwrap();
        // One notice to alice — must not appear in bob's inbox.
        let _to_alice = world
            .send_notice(
                Some(bob.id),
                alice.id,
                "k",
                "for alice",
                "ba",
                None,
                None,
                1_000,
            )
            .unwrap();

        let inbox = world.inbox(bob.id).expect("inbox runs");
        assert_eq!(
            inbox.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![n3.id, n2.id, n1.id],
            "inbox must be newest-first with id tiebreak"
        );
        assert!(
            inbox.iter().all(|n| n.recipient_player_id == bob.id),
            "inbox must be scoped to the recipient"
        );

        // Alice's inbox sees only her one notice.
        let alice_inbox = world.inbox(alice.id).expect("alice inbox runs");
        assert_eq!(alice_inbox.len(), 1);
        assert_eq!(alice_inbox[0].subject, "for alice");
    }

    /// SPEC_v3 §Task 3d: the *default* inbox excludes archived
    /// notices — that's what makes Task 3f's archive button
    /// meaningful. Pinned here even though `archive_notice` itself
    /// lands in 3f: we exercise the filter by hand-flipping
    /// `archived_at` so the contract is locked before any helper that
    /// flips it lands.
    #[test]
    fn inbox_excludes_archived_notices() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let kept = world
            .send_notice(Some(alice.id), bob.id, "k", "kept", "b", None, None, 1_000)
            .unwrap();
        let archived = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "k",
                "archived",
                "b",
                None,
                None,
                1_000,
            )
            .unwrap();

        // Hand-flip archived_at; Task 3f's helper will replace this.
        world
            .connection()
            .execute(
                "UPDATE notices SET archived_at = CURRENT_TIMESTAMP WHERE id = ?1",
                rusqlite::params![archived.id],
            )
            .unwrap();

        let inbox = world.inbox(bob.id).expect("inbox runs");
        assert_eq!(inbox.len(), 1, "archived notices must be hidden");
        assert_eq!(inbox[0].id, kept.id);
    }

    /// SPEC_v3 §Task 3d: read notices stay in the default inbox view.
    /// Inbox UIs typically render unread *and* read mail with
    /// different styling; filtering on `read_at` here would force
    /// every consumer through a second query. Pin that an unread+read
    /// mix surfaces both rows.
    #[test]
    fn inbox_includes_read_notices() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let read = world
            .send_notice(Some(alice.id), bob.id, "k", "read", "b", None, None, 1_000)
            .unwrap();
        let _unread = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "k",
                "unread",
                "b",
                None,
                None,
                1_000,
            )
            .unwrap();
        world
            .connection()
            .execute(
                "UPDATE notices SET read_at = CURRENT_TIMESTAMP WHERE id = ?1",
                rusqlite::params![read.id],
            )
            .unwrap();

        let inbox = world.inbox(bob.id).expect("inbox runs");
        assert_eq!(
            inbox.len(),
            2,
            "read notices must remain in the default inbox"
        );
        assert!(
            inbox.iter().any(|n| n.id == read.id && n.read_at.is_some()),
            "read flag must round-trip"
        );
    }

    /// SPEC_v3 §Task 3c: rejected notices MUST NOT produce a row.
    /// Already covered for the empty-subject case in
    /// `send_notice_rejects_empty_subject`; pin the same invariant for
    /// the overlong-body path so a future change that "validated after
    /// insert" can't slip past in either direction.
    #[test]
    fn rejected_overlong_notice_does_not_persist() {
        let (_dir, world) = world_with_notices();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let _ = world
            .send_notice(
                Some(alice.id),
                bob.id,
                "guestbook_note",
                "Hi",
                &"x".repeat(50),
                None,
                None,
                8,
            )
            .expect_err("overlong body must be rejected");

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM notices", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected notice must not produce a row");
    }
}
