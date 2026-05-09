//! `players` — shared-world player registry (SPEC_v2 §Task 5).
//!
//! Task 5a shipped the `players` table migration. Task 5b layered the
//! typed [`PlayerRecord`] read model and the [`WorldDb::upsert_player`]
//! write path on top, scoped to the Foglet-user-id case. Task 5c (this
//! iteration) extends the upsert path to the local-dev fallback so a
//! [`FogletContext`] with no `user_id` lands on a stable
//! `local_dev_key`-keyed row instead of erroring. Subsequent sub-tasks
//! fill in the rest:
//!
//! - 5d — `last_seen_at` refresh on repeat upsert.
//! - 5e — `FogletRole` parsing and `security_level` mapping.
//! - 5f — Persist normalized role/security metadata at upsert time.
//!
//! Splitting the migration into its own iteration keeps every commit
//! small enough to bisect cleanly: a regression that drops the
//! `local_dev_key` column will flunk the schema test in this module
//! rather than a higher-level upsert assertion that's harder to
//! attribute. The migration itself is a `pub const` so other modules
//! (the runtime startup path in Task 10, future Murder Motel
//! migrations in Task 12) can reference one canonical definition
//! instead of redeclaring the schema and drifting from it.

use thiserror::Error;

use crate::foglet::FogletContext;
use crate::world_db::{WorldDb, WorldMigration};

/// Default handle written when a [`FogletContext`] arrives without a
/// `username` / `handle` field.
///
/// SPEC §4.4 makes `handle` `NOT NULL` because every player needs
/// *something* to render in the lobby/leaderboard UI. Foglet normally
/// supplies one, but the field is documented as optional in §5.1, so
/// the upsert path needs a fallback. `"guest"` is intentionally
/// unremarkable — the on-screen affordance is "we couldn't find a
/// handle for this session" and the operator can fix it upstream.
const DEFAULT_HANDLE: &str = "guest";

/// Prefix on every synthesised `local_dev_key` value (Task 5c).
///
/// SPEC §4.4 demands that the local-dev key "does not collide with
/// real Foglet users". Foglet-issued user ids are opaque tokens that
/// the loader writes into the dedicated `foglet_user_id` column, so
/// the two namespaces are already separated at the schema layer (one
/// partial unique index per column). The prefix is belt-and-braces:
/// even if a future migration ever merges the columns, every
/// kit-synthesised value starts with `local-dev:` and is recognisable
/// at a glance in the `sqlite3` CLI. It also makes the `local-dev`
/// origin auditable from a raw row dump without consulting the rest
/// of the schema.
const LOCAL_DEV_KEY_PREFIX: &str = "local-dev:";

/// Synthesise the `local_dev_key` for a [`FogletContext`] that has no
/// `user_id`.
///
/// SPEC §4.4 rule: "If `user_id` is absent, the runtime MUST
/// synthesize a local key that does not collide with real Foglet
/// users." The key has to be **stable** for a given local-dev session
/// (so a second launch of the same dev user lands on the same
/// `players.id`) and **distinct** between different local-dev users
/// (so two-player Murder Motel smoke tests don't fork into one row).
///
/// We key on `ctx.username`, normalised by [`str::trim`] and falling
/// back to [`DEFAULT_HANDLE`] when missing or whitespace-only. Username
/// is the only identity bit a `--local-dev-user alice` invocation can
/// influence (Task 9d's CLI flag will set this), so it's the natural
/// pivot. `door_id` is also stable per launch but identical across the
/// two-player smoke test (both sessions run the same door binary), so
/// it cannot disambiguate by itself.
///
/// Two callers with the same trimmed handle deliberately collapse onto
/// the same row — that's the price of supporting an "I forgot to set a
/// handle" workflow without inventing fake entropy. Operators who want
/// independent rows pass distinct handles.
fn synthesize_local_dev_key(handle: &str) -> String {
    let trimmed = handle.trim();
    let key_body = if trimmed.is_empty() {
        DEFAULT_HANDLE
    } else {
        trimmed
    };
    format!("{LOCAL_DEV_KEY_PREFIX}{key_body}")
}

/// Schema for the shared-world player registry — SPEC_v2 §4.4.
///
/// One row per stable player identity (Foglet user OR local-dev
/// fallback). The shape mirrors §4.4 exactly:
///
/// - `id` — internal autoincrement primary key. Foreign-keyed by
///   later tables (`turn_ledger` in Task 6a, `world_events` in 7a,
///   `leaderboard_scores` in 8a) so per-player joins stay numeric and
///   cheap. `INTEGER PRIMARY KEY` is SQLite's idiom for a stable
///   `rowid` alias.
/// - `foglet_user_id` — nullable string. Populated when
///   `FogletContext.user_id` is present; left null on local-dev
///   sessions that haven't been issued a Foglet identity yet.
/// - `handle` — display string. SPEC §4.4 calls out that this is a
///   **display value, not an authorization key** — kept `NOT NULL`
///   because every player needs *something* to render, even a
///   default like "guest".
/// - `role` — normalized `FogletRole` text (`"sysop"`, `"mod"`,
///   `"user"`). Stored as text rather than an integer so an operator
///   inspecting the SQLite file with the `sqlite3` CLI can read it
///   without consulting source. Defaults to `"user"` at the SQL layer
///   so 5b (upsert) doesn't have to special-case missing roles.
/// - `security_level` — integer derived from role unless Foglet
///   supplies an explicit value (Task 5e mapping: sysop=100, mod=90,
///   user=50). Stored as an integer for ordering/comparison; the
///   mapping itself lives in code so the rules stay in one place.
/// - `first_seen_at` — UTC timestamp of the player's first upsert.
///   `DEFAULT CURRENT_TIMESTAMP` so 5b can `INSERT` without threading
///   a clock; 5d preserves this on repeat upserts (only `last_seen_at`
///   moves).
/// - `last_seen_at` — UTC timestamp updated on every upsert per 5d.
///   Defaulted the same way as `first_seen_at` so a freshly-inserted
///   row already has a sensible value.
/// - `local_dev_key` — nullable string identifying local-dev
///   identities (Task 5c). Distinct from `foglet_user_id` so the two
///   identity namespaces never collide: a Foglet user "alice" and a
///   local-dev "alice" land on separate rows, both queryable.
///
/// # Uniqueness
///
/// The migration creates two **partial unique indexes** rather than
/// declaring `UNIQUE` constraints inline:
///
/// - `idx_players_foglet_user_id` — unique over `foglet_user_id`
///   when not null. SQLite treats `NULL` as distinct under a plain
///   `UNIQUE` constraint, which would silently allow multiple
///   local-dev rows; the partial index (`WHERE foglet_user_id IS NOT
///   NULL`) makes the intent explicit and matches the SPEC §4.4 rule
///   that `user_id` "is the stable key" *when present*.
/// - `idx_players_local_dev_key` — unique over `local_dev_key`
///   when not null, for the same reason on the local-dev side.
///
/// Future tasks (5b upsert, 5c local-dev key) rely on these indexes
/// for `INSERT … ON CONFLICT` upserts; introducing them now keeps
/// the schema and the future write path in lockstep.
pub const PLAYERS_MIGRATION: WorldMigration = WorldMigration {
    version: 2,
    name: "create_players",
    sql: "\
CREATE TABLE IF NOT EXISTS players (\n\
    id              INTEGER PRIMARY KEY,\n\
    foglet_user_id  TEXT,\n\
    handle          TEXT NOT NULL,\n\
    role            TEXT NOT NULL DEFAULT 'user',\n\
    security_level  INTEGER NOT NULL DEFAULT 50,\n\
    first_seen_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    last_seen_at    TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    local_dev_key   TEXT\n\
);\n\
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_foglet_user_id\n\
    ON players(foglet_user_id) WHERE foglet_user_id IS NOT NULL;\n\
CREATE UNIQUE INDEX IF NOT EXISTS idx_players_local_dev_key\n\
    ON players(local_dev_key) WHERE local_dev_key IS NOT NULL;\n\
",
};

/// Typed read of a `players` row — SPEC_v2 §4.4 column-for-column.
///
/// Returned by [`WorldDb::upsert_player`] so authoring code never has
/// to spell out a `query_row` against the world database to learn its
/// own player id. Owning every column (rather than borrowing) matches
/// the rest of the v2 surface: the runtime hands these to screens by
/// value and the lifetime story stays simple.
///
/// # Field semantics
///
/// - `id` — internal autoincrement primary key. Stable across
///   relaunches; foreign-keyed by future tables (`turn_ledger` in
///   Task 6, `world_events` in Task 7, `leaderboard_scores` in Task 8)
///   so per-player joins stay numeric.
/// - `foglet_user_id` — `Some` when the upsert came from a Foglet
///   context with a user id; `None` for the local-dev path landing
///   in Task 5c.
/// - `handle` — display string. Refreshed on every upsert so a
///   user who changes their Foglet handle sees the new value the
///   next time they launch.
/// - `role`, `security_level` — set by SQL defaults today (`'user'`
///   / `50`). Task 5e/5f overwrite these from the live context.
/// - `first_seen_at` — UTC timestamp of the first upsert. Preserved
///   across repeat upserts (Task 5d's invariant).
/// - `last_seen_at` — UTC timestamp Task 5d will refresh on repeat
///   upsert. Today it equals `first_seen_at` until that task lands.
/// - `local_dev_key` — `Some` only on the Task 5c local-dev path;
///   `None` for Foglet-user-id rows like the ones 5b creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerRecord {
    /// Internal primary key — stable across relaunches.
    pub id: i64,
    /// Foglet-supplied stable user id, when present.
    pub foglet_user_id: Option<String>,
    /// Display handle (refreshed on every upsert).
    pub handle: String,
    /// Normalized role text (`'sysop'`, `'mod'`, `'user'`).
    pub role: String,
    /// Integer derived from `role` (sysop=100, mod=90, user=50).
    pub security_level: i64,
    /// UTC timestamp of the first upsert; preserved by Task 5d.
    pub first_seen_at: String,
    /// UTC timestamp Task 5d refreshes on every repeat upsert.
    pub last_seen_at: String,
    /// Synthesised local-dev key (Task 5c) — always `None` here.
    pub local_dev_key: Option<String>,
}

/// Failure modes for [`WorldDb::upsert_player`].
///
/// Library-internal `thiserror`: the runtime layer (Task 10) wraps
/// these with `anyhow` at the process boundary so the operator-facing
/// message stays a single sentence.
#[derive(Debug, Error)]
pub enum PlayerError {
    /// The `INSERT … ON CONFLICT … RETURNING` round-trip failed.
    /// Wrapping `rusqlite::Error` keeps the upsert call site readable
    /// (one error type, one mapping) while preserving the underlying
    /// cause for `tracing` and operator-facing messages.
    #[error("failed to upsert player into world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the upsert statement.
        #[source]
        source: rusqlite::Error,
    },
}

impl WorldDb {
    /// Upsert the player identified by `ctx` and return the
    /// resulting [`PlayerRecord`].
    ///
    /// Stable-key contract (SPEC_v2 §4.4): two calls with the same
    /// `FogletContext.user_id` resolve to the same `players.id`. The
    /// underlying mechanism is the partial unique index on
    /// `foglet_user_id` plus an `ON CONFLICT … DO UPDATE` clause —
    /// the conflict refreshes `handle` so a user who renames in
    /// Foglet sees the new label without us creating a duplicate row.
    ///
    /// Two identity paths are supported, both keyed by a partial
    /// unique index in [`PLAYERS_MIGRATION`]:
    ///
    /// - `ctx.user_id = Some(_)` → upsert keyed on `foglet_user_id`.
    /// - `ctx.user_id = None`    → upsert keyed on a synthesised
    ///   `local_dev_key` (Task 5c) derived from `ctx.username`.
    ///
    /// Task 5d will refresh `last_seen_at` on every call; Task 5e/5f
    /// will write normalized `role` and `security_level`. Until then
    /// the `players` row picks up SQL defaults for those columns.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the upsert is a single statement, so the busy
    /// timeout configured at open time is the only contention story
    /// we need. `&mut self` would fight the runtime layer (Task 10)
    /// where `GameContext` borrows the world DB once per tick.
    pub fn upsert_player(&self, ctx: &FogletContext) -> Result<PlayerRecord, PlayerError> {
        // `username` is optional on the wire (SPEC §5.1). Falling back
        // to `DEFAULT_HANDLE` keeps the `NOT NULL` `handle` constraint
        // satisfied without burying the choice in the SQL string.
        let handle = ctx.username.as_deref().unwrap_or(DEFAULT_HANDLE);

        match ctx.user_id.as_deref() {
            Some(user_id) => self.upsert_by_foglet_user_id(user_id, handle),
            None => {
                // SPEC §4.4 mandates a synthesised local key that does
                // not collide with real Foglet users. The dedicated
                // `local_dev_key` column + partial unique index gives
                // us that namespace separation at the schema layer;
                // [`synthesize_local_dev_key`] picks the value.
                let key = synthesize_local_dev_key(handle);
                self.upsert_by_local_dev_key(&key, handle)
            }
        }
    }

    /// Foglet-user-id branch of [`Self::upsert_player`]. Pulled out so
    /// the local-dev branch can mirror its structure without sharing
    /// SQL — the two `ON CONFLICT` targets reference different partial
    /// indexes, and a single statement that tried to handle both
    /// would have to special-case nullability in ways SQLite does not
    /// support cleanly.
    fn upsert_by_foglet_user_id(
        &self,
        user_id: &str,
        handle: &str,
    ) -> Result<PlayerRecord, PlayerError> {
        // Partial unique indexes require the `WHERE` clause to be
        // restated in the `ON CONFLICT` target; SQLite refuses to
        // match against a partial index otherwise. `excluded.handle`
        // names the row we tried to insert — this is the SQL idiom
        // for "use the new value during the conflict update".
        //
        // `RETURNING` (SQLite ≥ 3.35) lets us read the canonical row
        // back without a second `SELECT` — important because the
        // first-insert path needs the autoincrement `id` we don't
        // know yet, and the conflict path benefits from echoing the
        // stored timestamps so the caller doesn't get a stale view.
        const SQL: &str = "\
INSERT INTO players (foglet_user_id, handle) \
VALUES (?1, ?2) \
ON CONFLICT(foglet_user_id) WHERE foglet_user_id IS NOT NULL \
DO UPDATE SET handle = excluded.handle \
RETURNING id, foglet_user_id, handle, role, security_level, \
          first_seen_at, last_seen_at, local_dev_key";

        self.connection()
            .query_row(SQL, rusqlite::params![user_id, handle], row_to_record)
            .map_err(|source| PlayerError::Sqlite { source })
    }

    /// Local-dev branch of [`Self::upsert_player`] (Task 5c).
    ///
    /// Mirrors [`Self::upsert_by_foglet_user_id`] but conflicts on the
    /// `idx_players_local_dev_key` partial index. `foglet_user_id`
    /// stays NULL so the partial index over `foglet_user_id` does not
    /// engage — the two namespaces remain disjoint at the schema
    /// layer, which is the property SPEC §4.4 calls out.
    fn upsert_by_local_dev_key(
        &self,
        local_dev_key: &str,
        handle: &str,
    ) -> Result<PlayerRecord, PlayerError> {
        const SQL: &str = "\
INSERT INTO players (local_dev_key, handle) \
VALUES (?1, ?2) \
ON CONFLICT(local_dev_key) WHERE local_dev_key IS NOT NULL \
DO UPDATE SET handle = excluded.handle \
RETURNING id, foglet_user_id, handle, role, security_level, \
          first_seen_at, last_seen_at, local_dev_key";

        self.connection()
            .query_row(SQL, rusqlite::params![local_dev_key, handle], row_to_record)
            .map_err(|source| PlayerError::Sqlite { source })
    }
}

/// Decode a `players` row into [`PlayerRecord`].
///
/// Pulled out of the upsert call site so Task 5c's local-dev path and
/// any future read helpers (a `find_by_user_id` query, for instance)
/// can share one decoder. Column order matches the `RETURNING` clause
/// above and the SPEC §4.4 schema; a regression that reorders columns
/// in the migration will surface here as a type error rather than as
/// a runtime panic in production.
fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlayerRecord> {
    Ok(PlayerRecord {
        id: row.get(0)?,
        foglet_user_id: row.get(1)?,
        handle: row.get(2)?,
        role: row.get(3)?,
        security_level: row.get(4)?,
        first_seen_at: row.get(5)?,
        last_seen_at: row.get(6)?,
        local_dev_key: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::ContextSource;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// Helper: build a [`FogletContext`] with just the identity bits
    /// the upsert path reads. Tests don't care about `terminal_*` or
    /// `session_id`, but the struct fields are required, so this
    /// keeps test bodies focused on the behavior under test.
    fn ctx_with(user_id: Option<&str>, username: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "test-door".to_string(),
            user_id: user_id.map(str::to_string),
            username: username.map(str::to_string),
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::ContextFile,
        }
    }

    /// SPEC_v2 §Task 5b acceptance: a Foglet `user_id` is the stable
    /// key for a player row. Two upserts against the same user_id
    /// resolve to the same `players.id` and never duplicate the row,
    /// even if the display handle changes between calls.
    #[test]
    fn upsert_player_with_user_id_is_stable_across_repeats() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let first = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("first upsert succeeds");
        let second = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice-renamed")))
            .expect("repeat upsert succeeds");

        assert_eq!(first.id, second.id, "user_id must be the stable key");
        assert_eq!(first.foglet_user_id.as_deref(), Some("u-alice"));
        assert_eq!(second.handle, "alice-renamed");

        // And the table truly has one row — a regression that papered
        // over the partial-unique-index conflict by inserting twice
        // (and just hiding it from `RETURNING`) would flunk this.
        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM players WHERE foglet_user_id = ?1",
                rusqlite::params!["u-alice"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(count, 1, "stable key must not duplicate the row");
    }

    /// Two distinct user_ids land on two distinct rows. Pinning this
    /// alongside the stability test prevents a regression where the
    /// upsert path hard-codes the first-seen id and returns it for
    /// every caller.
    #[test]
    fn upsert_player_with_distinct_user_ids_creates_distinct_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("alice upsert succeeds");
        let bob = world
            .upsert_player(&ctx_with(Some("u-bob"), Some("bob")))
            .expect("bob upsert succeeds");

        assert_ne!(alice.id, bob.id);
        assert_eq!(alice.handle, "alice");
        assert_eq!(bob.handle, "bob");
    }

    /// Missing `username` falls back to the documented default rather
    /// than rejecting the upsert. Foglet's contract makes `username`
    /// optional (SPEC §5.1) — refusing to register a player on that
    /// path would lock anonymous-access doors out of the world.
    #[test]
    fn upsert_player_falls_back_to_default_handle_when_username_missing() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let record = world
            .upsert_player(&ctx_with(Some("u-anon"), None))
            .expect("anonymous-handle upsert succeeds");
        assert_eq!(record.handle, DEFAULT_HANDLE);
    }

    /// SQL defaults from the §4.4 schema cover `role` and
    /// `security_level` until Task 5e/5f land. Pinning the values
    /// here means a follow-up that flips a default (without touching
    /// the upsert) lands in this test, not a downstream Murder Motel
    /// assertion.
    #[test]
    fn upsert_player_uses_schema_defaults_for_role_and_security() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let record = world
            .upsert_player(&ctx_with(Some("u-defaults"), Some("d")))
            .expect("defaults upsert succeeds");
        assert_eq!(record.role, "user");
        assert_eq!(record.security_level, 50);
        assert!(record.local_dev_key.is_none());
    }

    /// SPEC_v2 §Task 5c acceptance: two local-dev sessions with
    /// distinct handles synthesise distinct `local_dev_key` values and
    /// land on distinct rows — they MUST NOT collide. Without this,
    /// the two-player Murder Motel smoke test (alice + bob) would
    /// share one player record and fork every per-player ledger,
    /// event, and leaderboard entry into a single shared identity.
    #[test]
    fn upsert_player_with_distinct_local_dev_handles_creates_distinct_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("alice local-dev upsert succeeds");
        let bob = world
            .upsert_player(&ctx_with(None, Some("bob")))
            .expect("bob local-dev upsert succeeds");

        assert_ne!(alice.id, bob.id, "local-dev handles must not collide");
        assert!(
            alice.foglet_user_id.is_none() && bob.foglet_user_id.is_none(),
            "local-dev rows leave foglet_user_id NULL"
        );
        assert_eq!(alice.local_dev_key.as_deref(), Some("local-dev:alice"));
        assert_eq!(bob.local_dev_key.as_deref(), Some("local-dev:bob"));
        assert_eq!(alice.handle, "alice");
        assert_eq!(bob.handle, "bob");
    }

    /// Repeat upserts with the same local-dev handle resolve to the
    /// same `players.id`. The mirror of the user-id stability test —
    /// without it, a dev relaunching `cargo run --example murder_motel`
    /// would keep forking new player rows on every invocation.
    #[test]
    fn upsert_player_with_same_local_dev_handle_is_stable_across_repeats() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let first = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("first local-dev upsert succeeds");
        let second = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("repeat local-dev upsert succeeds");
        assert_eq!(first.id, second.id);

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM players WHERE local_dev_key = ?1",
                rusqlite::params!["local-dev:alice"],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(count, 1, "stable local-dev key must not duplicate the row");
    }

    /// Local-dev and Foglet-user-id rows occupy disjoint namespaces —
    /// a Foglet user "alice" and a local-dev "alice" must end up on
    /// separate `players` rows. SPEC §4.4 calls this out explicitly:
    /// "synthesize a local key that does not collide with real Foglet
    /// users".
    #[test]
    fn local_dev_and_foglet_namespaces_do_not_collide() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let foglet_alice = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("foglet alice upsert succeeds");
        let local_alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("local-dev alice upsert succeeds");

        assert_ne!(foglet_alice.id, local_alice.id);
        assert_eq!(foglet_alice.foglet_user_id.as_deref(), Some("u-alice"));
        assert!(foglet_alice.local_dev_key.is_none());
        assert!(local_alice.foglet_user_id.is_none());
        assert_eq!(
            local_alice.local_dev_key.as_deref(),
            Some("local-dev:alice")
        );
    }

    /// Local-dev fallback also works when `username` is missing — a
    /// context with neither identity field shouldn't crash, it
    /// collapses onto the default-handle row. Documented as a soft
    /// collapse in [`synthesize_local_dev_key`]: operators who want
    /// independent rows pass a handle.
    #[test]
    fn upsert_player_with_no_identity_uses_default_local_dev_key() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let record = world
            .upsert_player(&ctx_with(None, None))
            .expect("no-identity upsert succeeds");
        assert_eq!(record.handle, DEFAULT_HANDLE);
        assert_eq!(
            record.local_dev_key.as_deref(),
            Some("local-dev:guest"),
            "missing username falls back to the default handle in the key"
        );
    }

    /// Whitespace-only handles trim down to the default. Without this
    /// guard, two `--local-dev-user "   "` invocations would each pin
    /// a distinct `local_dev_key` byte sequence (one with leading
    /// spaces, one without) and split a single sloppy operator's
    /// history across rows.
    #[test]
    fn synthesize_local_dev_key_trims_and_falls_back_on_whitespace() {
        assert_eq!(synthesize_local_dev_key("alice"), "local-dev:alice");
        assert_eq!(synthesize_local_dev_key("  alice  "), "local-dev:alice");
        assert_eq!(synthesize_local_dev_key(""), "local-dev:guest");
        assert_eq!(synthesize_local_dev_key("   "), "local-dev:guest");
    }

    /// SPEC_v2 §Task 5a acceptance: applying [`PLAYERS_MIGRATION`]
    /// records the version *and* leaves the documented column shape
    /// behind. Pinning both halves in one test means a regression that
    /// renames a column (5d's `last_seen_at` is the most likely
    /// candidate) flunks here rather than in a 5b upsert assertion
    /// where the cause is harder to localise.
    #[test]
    fn applies_players_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies cleanly to a fresh DB");

        // `pragma_table_info` is the canonical "describe this table"
        // query in SQLite. Asserting on the ordered column-name list
        // (rather than just `COUNT(*) = 8`) catches a regression that
        // drops one column and adds another by accident.
        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('players') ORDER BY cid")
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
                "foglet_user_id".to_string(),
                "handle".to_string(),
                "role".to_string(),
                "security_level".to_string(),
                "first_seen_at".to_string(),
                "last_seen_at".to_string(),
                "local_dev_key".to_string(),
            ],
            "players schema must match SPEC_v2 §4.4 exactly"
        );

        // Bookkeeping row recorded at the migration's declared version
        // — proves the standard apply path was used (vs. a side-channel
        // `execute_batch`) so the relaunch idempotency test in Task 4c
        // continues to apply.
        let recorded: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params![PLAYERS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(recorded, PLAYERS_MIGRATION.version);
    }

    /// Re-applying [`PLAYERS_MIGRATION`] is a no-op (Task 4c
    /// idempotency carries forward to the kit's built-in migrations,
    /// not just author-supplied ones). Without this guard a relaunch
    /// against an already-bootstrapped DB would surface a `table
    /// already exists` error from the second `CREATE TABLE` in the
    /// batch — the `IF NOT EXISTS` clauses make the SQL re-runnable
    /// even if the idempotency check ever regressed.
    #[test]
    fn players_migration_is_idempotent_on_repeat_apply() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("first apply succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("second apply is a no-op, not an error");

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE version = ?1",
                rusqlite::params![PLAYERS_MIGRATION.version],
                |row| row.get(0),
            )
            .expect("count query runs");
        assert_eq!(row_count, 1, "repeat apply must not duplicate the row");
    }

    /// Partial unique index on `foglet_user_id` rejects duplicates
    /// while still allowing multiple local-dev rows where the column
    /// is null. This is the property Task 5b's upsert path relies on
    /// — without it, two concurrent Foglet sessions for the same user
    /// could create separate registry rows and split a player's
    /// turn ledger and leaderboard score across them.
    #[test]
    fn foglet_user_id_unique_when_not_null() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let conn = world.connection();
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle) VALUES (?1, ?2)",
            rusqlite::params!["u-alice", "alice"],
        )
        .expect("first insert succeeds");

        let dup = conn.execute(
            "INSERT INTO players (foglet_user_id, handle) VALUES (?1, ?2)",
            rusqlite::params!["u-alice", "alice-twin"],
        );
        assert!(
            dup.is_err(),
            "duplicate foglet_user_id must be rejected by the partial unique index"
        );

        // Two NULL `foglet_user_id` rows coexist — the partial index
        // intentionally excludes null so local-dev identities (which
        // have no Foglet user_id yet) aren't forced to share a row.
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle, local_dev_key) VALUES (NULL, ?1, ?2)",
            rusqlite::params!["dev1", "dev-key-1"],
        )
        .expect("first null-user_id row succeeds");
        conn.execute(
            "INSERT INTO players (foglet_user_id, handle, local_dev_key) VALUES (NULL, ?1, ?2)",
            rusqlite::params!["dev2", "dev-key-2"],
        )
        .expect("second null-user_id row coexists");
    }

    /// Companion guard to [`foglet_user_id_unique_when_not_null`] for
    /// the local-dev key namespace. Task 5c will lean on this so two
    /// local-dev sessions with the same synthesized key resolve to a
    /// single registry row instead of forking the player's history.
    #[test]
    fn local_dev_key_unique_when_not_null() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let conn = world.connection();
        conn.execute(
            "INSERT INTO players (handle, local_dev_key) VALUES (?1, ?2)",
            rusqlite::params!["alice", "local:alice"],
        )
        .expect("first local-dev insert succeeds");

        let dup = conn.execute(
            "INSERT INTO players (handle, local_dev_key) VALUES (?1, ?2)",
            rusqlite::params!["alice-twin", "local:alice"],
        );
        assert!(
            dup.is_err(),
            "duplicate local_dev_key must be rejected by the partial unique index"
        );
    }

    /// Defaults — role and security_level fall back to `'user'` / `50`
    /// when the caller doesn't specify them. 5b's upsert path will
    /// rely on this for any context that arrives without role
    /// information, so the SQL-side default is the load-bearing piece.
    #[test]
    fn defaults_for_role_and_security_level_apply() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        world
            .connection()
            .execute(
                "INSERT INTO players (handle) VALUES (?1)",
                rusqlite::params!["someone"],
            )
            .expect("minimal insert succeeds");

        let (role, security_level): (String, i64) = world
            .connection()
            .query_row(
                "SELECT role, security_level FROM players WHERE handle = ?1",
                rusqlite::params!["someone"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query succeeds");
        assert_eq!(role, "user");
        assert_eq!(security_level, 50);
    }
}
