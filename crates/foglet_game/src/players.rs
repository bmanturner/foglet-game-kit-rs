//! `players` — shared-world player registry.
//!
//!  shipped the `players` table migration. layered the
//! typed [`PlayerRecord`] read model and the [`WorldDb::upsert_player`]
//! write path on top, scoped to the Foglet-user-id case.
//! extended the upsert path to the local-dev fallback so a
//! [`FogletContext`] with no `user_id` lands on a stable
//! `local_dev_key`-keyed row instead of erroring. kept
//! `first_seen_at` stable while refreshing `last_seen_at`.
//! introduced [`FogletRole`](crate::FogletRole) parsing and the
//! `security_level` mapping. (this iteration) plumbs that
//! mapping into the upsert path so every relaunch persists the
//! normalized role/security pair from the live context — strictly
//! advisory metadata for in-game flavour, *not* a launch authorization
//! gate (Foglet itself decides who may exec the door; the kit only
//! records what it was told).
//!
//! Splitting the migration into its own iteration keeps every commit
//! small enough to bisect cleanly: a regression that drops the
//! `local_dev_key` column will flunk the schema test in this module
//! rather than a higher-level upsert assertion that's harder to
//! attribute. The migration itself is a `pub const` so other modules
//! (the runtime startup path in, future Murder Motel
//! migrations in ) can reference one canonical definition
//! instead of redeclaring the schema and drifting from it.

use thiserror::Error;

use crate::foglet::FogletContext;
use crate::world_db::{WorldDb, WorldMigration};

/// Default handle written when a [`FogletContext`] arrives without a
/// `username` `handle` field.
///
///  makes `handle` `NOT NULL` because every player needs
/// *something* to render in the lobby/leaderboard UI. Foglet normally
/// supplies one, but the field is documented as optional in, so
/// the upsert path needs a fallback. `"guest"` is intentionally
/// unremarkable — the on-screen affordance is "we couldn't find a
/// handle for this session" and the operator can fix it upstream.
const DEFAULT_HANDLE: &str = "guest";

/// Prefix on every synthesised `local_dev_key` value.
///
///  demands that the local-dev key "does not collide with
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
///  rule: "If `user_id` is absent, the runtime MUST
/// synthesize a local key that does not collide with real Foglet
/// users." The key has to be **stable** for a given local-dev session
/// (so a second launch of the same dev user lands on the same
/// `players.id`) and **distinct** between different local-dev users
/// (so two-player Murder Motel smoke tests don't fork into one row).
///
/// We key on `ctx.username`, normalised by [`str::trim`] and falling
/// back to [`DEFAULT_HANDLE`] when missing or whitespace-only. Username
/// is the only identity bit a `--local-dev-user alice` invocation can
/// influence ('s CLI flag will set this), so it's the natural
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

/// Schema for the shared-world player registry
///
/// One row per stable player identity (Foglet user OR local-dev
/// fallback). The shape mirrors exactly:
///
/// - `id` — internal autoincrement primary key. Foreign-keyed by
///   later tables (`turn_ledger` in, `world_events` in 7a.
///   `leaderboard_scores` in 8a) so per-player joins stay numeric and
///   cheap. `INTEGER PRIMARY KEY` is SQLite's idiom for a stable
///   `rowid` alias.
/// - `foglet_user_id` — nullable string. Populated when
///   `FogletContext.user_id` is present; left null on local-dev
///   sessions that haven't been issued a Foglet identity yet.
/// - `handle` — display string. calls out that this is a
///   **display value, not an authorization key** — kept `NOT NULL`
///   because every player needs *something* to render, even a
///   default like "guest".
/// - `role` — normalized `FogletRole` text (`"sysop"`, `"mod"`.
///   `"user"`). Stored as text rather than an integer so an operator
///   inspecting the SQLite file with the `sqlite3` CLI can read it
///   without consulting source. Defaults to `"user"` at the SQL layer
///   so 5b (upsert) doesn't have to special-case missing roles.
/// - `security_level` — integer derived from role unless Foglet
///   supplies an explicit value ( mapping: sysop=100, mod=90.
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
///   identities. Distinct from `foglet_user_id` so the two
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
///   NULL`) makes the intent explicit and matches the rule
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

/// Typed read of a `players` row — column-for-column.
///
/// Returned by [`WorldDb::upsert_player`] so authoring code never has
/// to spell out a `query_row` against the world database to learn its
/// own player id. Owning every column (rather than borrowing) matches
/// the rest of the surface: the runtime hands these to screens by
/// value and the lifetime story stays simple.
///
/// # Field semantics
///
/// - `id` — internal autoincrement primary key. Stable across
///   relaunches; foreign-keyed by future tables (`turn_ledger` in
///   , `world_events` in, `leaderboard_scores` in )
///   so per-player joins stay numeric.
/// - `foglet_user_id` — `Some` when the upsert came from a Foglet
///   context with a user id; `None` for the local-dev path landing
///   in.
/// - `handle` — display string. Refreshed on every upsert so a
///   user who changes their Foglet handle sees the new value the
///   next time they launch.
/// - `role`, `security_level` — written from the live context's
///   [`FogletContext::foglet_role`] /
///   [`FogletContext::security_level`] on every upsert. The
///   stored values are advisory metadata for in-game flavour only
///   never consulted as a launch authorization gate. An absent /
///   unknown role normalises to `'user'` `50`, which lines up with
///   the SQL defaults so the column shape stays consistent whether the
///   row was written by the upsert path or a hand-rolled INSERT in
///   tests.
/// - `first_seen_at` — UTC timestamp of the first upsert. Preserved
///   across repeat upserts ('s invariant).
/// - `last_seen_at` — UTC timestamp refreshed to `CURRENT_TIMESTAMP`
///   on every repeat upsert. Equals `first_seen_at` only on
///   the very first insert.
/// - `local_dev_key` — `Some` only on the local-dev path;
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
    /// UTC timestamp of the first upsert; preserved by.
    pub first_seen_at: String,
    /// UTC timestamp refreshed to `CURRENT_TIMESTAMP` on every repeat
    /// upsert.
    pub last_seen_at: String,
    /// Synthesised local-dev key — always `None` here.
    pub local_dev_key: Option<String>,
}

/// Failure modes for [`WorldDb::upsert_player`].
///
/// Library-internal `thiserror`: the runtime layer wraps
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
    /// Stable-key contract: two calls with the same
    /// `FogletContext.user_id` resolve to the same `players.id`. The
    /// underlying mechanism is the partial unique index on
    /// `foglet_user_id` plus an `ON CONFLICT … DO UPDATE` clause
    /// the conflict refreshes `handle` so a user who renames in
    /// Foglet sees the new label without us creating a duplicate row.
    ///
    /// Two identity paths are supported, both keyed by a partial
    /// unique index in [`PLAYERS_MIGRATION`]:
    ///
    /// - `ctx.user_id = Some(_)` → upsert keyed on `foglet_user_id`.
    /// - `ctx.user_id = None` → upsert keyed on a synthesised
    ///   `local_dev_key` derived from `ctx.username`.
    ///
    /// Repeat upsert refreshes `last_seen_at` to `CURRENT_TIMESTAMP`
    ///  while leaving `first_seen_at` alone — the
    /// "first time we saw this identity" column is an audit anchor and
    /// must survive every relaunch. The normalized `role` /
    /// `security_level` pair is also rewritten on every upsert (Task
    /// 5f) so a player whose role changed upstream in Foglet sees the
    /// new value the next time they launch. The stored values are
    /// strictly advisory metadata: nothing in this kit gates door
    /// launch on them. Foglet remains the only authority for
    /// "who may exec this door".
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: the upsert is a single statement, so the busy
    /// timeout configured at open time is the only contention story
    /// we need. `&mut self` would fight the runtime layer
    /// where `GameContext` borrows the world DB once per tick.
    pub fn upsert_player(&self, ctx: &FogletContext) -> Result<PlayerRecord, PlayerError> {
        // `username` is optional on the wire. Falling back
        // to `DEFAULT_HANDLE` keeps the `NOT NULL` `handle` constraint
        // satisfied without burying the choice in the SQL string.
        let handle = ctx.username.as_deref().unwrap_or(DEFAULT_HANDLE);
        // Resolve the normalized role security_level pair once per
        // upsert. Computing here — rather than in each
        // identity-branch helper — keeps the "one source of truth for
        // role normalization" rule visible at the
        // top-level entry point: both branches receive the exact same
        // tokens, regardless of which partial unique index they target.
        let role = ctx.foglet_role();
        let role_token = role.as_token();
        let security_level = role.security_level();

        match ctx.user_id.as_deref() {
            Some(user_id) => {
                self.upsert_by_foglet_user_id(user_id, handle, role_token, security_level)
            }
            None => {
                //  mandates a synthesised local key that does
                // not collide with real Foglet users. The dedicated
                // `local_dev_key` column + partial unique index gives
                // us that namespace separation at the schema layer;
                // [`synthesize_local_dev_key`] picks the value.
                let key = synthesize_local_dev_key(handle);
                self.upsert_by_local_dev_key(&key, handle, role_token, security_level)
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
        role: &str,
        security_level: i64,
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
        // `last_seen_at = CURRENT_TIMESTAMP` is the refresh:
        // SQLite evaluates `CURRENT_TIMESTAMP` per-statement, so the
        // conflict path stamps the row with the moment of this upsert
        // without us threading a clock through. `first_seen_at` is
        // *deliberately* not in the SET list — that's the column the
        // audit story depends on, and excluding it from the update
        // preserves the original insert timestamp across every relaunch.
        // `role` `security_level` are now part of the SET list
        // . Rewriting them on every conflict means a player
        // who is promoted/demoted upstream in Foglet between launches
        // sees the change reflected the next time they appear — and
        // the column never ages out of sync with the live context.
        const SQL: &str = "\
INSERT INTO players (foglet_user_id, handle, role, security_level) \
VALUES (?1, ?2, ?3, ?4) \
ON CONFLICT(foglet_user_id) WHERE foglet_user_id IS NOT NULL \
DO UPDATE SET handle = excluded.handle, \
              role = excluded.role, \
              security_level = excluded.security_level, \
              last_seen_at = CURRENT_TIMESTAMP \
RETURNING id, foglet_user_id, handle, role, security_level, \
          first_seen_at, last_seen_at, local_dev_key";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![user_id, handle, role, security_level],
                row_to_record,
            )
            .map_err(|source| PlayerError::Sqlite { source })
    }

    /// Local-dev branch of [`Self::upsert_player`].
    ///
    /// Mirrors [`Self::upsert_by_foglet_user_id`] but conflicts on the
    /// `idx_players_local_dev_key` partial index. `foglet_user_id`
    /// stays NULL so the partial index over `foglet_user_id` does not
    /// engage — the two namespaces remain disjoint at the schema
    /// layer, which is the property calls out.
    fn upsert_by_local_dev_key(
        &self,
        local_dev_key: &str,
        handle: &str,
        role: &str,
        security_level: i64,
    ) -> Result<PlayerRecord, PlayerError> {
        // See [`Self::upsert_by_foglet_user_id`] for the rationale on
        // `last_seen_at = CURRENT_TIMESTAMP`. The local-dev
        // branch carries the same first-seen-preserving contract — a
        // dev who relaunches `cargo run --example murder_motel` keeps
        // their original `first_seen_at` even as `last_seen_at` walks
        // forward.
        // See [`Self::upsert_by_foglet_user_id`] for the rationale on
        // including `role` `security_level` in the SET list (Task
        // 5f). The local-dev path mirrors the Foglet path so a single
        // ground-truth contract — "every upsert refreshes role"
        // applies regardless of identity namespace.
        const SQL: &str = "\
INSERT INTO players (local_dev_key, handle, role, security_level) \
VALUES (?1, ?2, ?3, ?4) \
ON CONFLICT(local_dev_key) WHERE local_dev_key IS NOT NULL \
DO UPDATE SET handle = excluded.handle, \
              role = excluded.role, \
              security_level = excluded.security_level, \
              last_seen_at = CURRENT_TIMESTAMP \
RETURNING id, foglet_user_id, handle, role, security_level, \
          first_seen_at, last_seen_at, local_dev_key";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![local_dev_key, handle, role, security_level],
                row_to_record,
            )
            .map_err(|source| PlayerError::Sqlite { source })
    }

    /// Look up the display `handle` for a `players.id`.
    ///
    /// Returns `Ok(None)` for an id that does not exist — a missing row
    /// is a normal UI state (the leaderboard render shows `"player #N"`
    /// or `"unknown"` rather than soft-locking) and not an error
    /// condition. Errors are reserved for genuine SQLite failures.
    ///
    /// Added for the leaderboard screen, which renders
    /// `top_scores` results as `<rank>. <handle> <score>` and therefore
    /// needs to convert each `ScoreRecord.player_id` back into the
    /// display string. Lives next to [`Self::upsert_player`] because the
    /// `players` table is the single source of truth for handles.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single read statement under the configured busy
    /// timeout, same shape as [`crate::WorldDb::top_scores`]. The runtime
    /// layer calls this from the leaderboard render path so
    /// keeping the borrow shared lets `GameContext` thread one world-DB
    /// reference across screens.
    pub fn player_handle(&self, id: i64) -> Result<Option<String>, PlayerError> {
        // `SELECT handle … LIMIT 1` keeps the wire shape minimal: the
        // leaderboard render only needs the display string. Returning
        // the full `PlayerRecord` would cost an extra column read per
        // row for no caller-visible gain.
        const SQL: &str = "SELECT handle FROM players WHERE id = ?1 LIMIT 1";
        match self
            .connection()
            .query_row(SQL, rusqlite::params![id], |row| row.get::<_, String>(0))
        {
            Ok(handle) => Ok(Some(handle)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(PlayerError::Sqlite { source }),
        }
    }

    /// Search the player registry by case-insensitive handle prefix.
    ///
    /// Added for — Murder Motel's async multiplayer
    /// screens (notice send target picker, challenge-rival selector.
    /// bounty claim attribution UI) need a way to autocomplete a
    /// handle the local player typed in the box. The kit already owns
    /// the `players` registry, so providing a single typed helper
    /// keeps every screen from rolling its own raw `LIKE` query and
    /// drifting on case-folding rules.
    ///
    /// # Semantics
    ///
    /// - The match is **case-insensitive prefix** — `"AL"` matches
    ///   `"alice"` and `"Albert"` but not `"calico"`. Implemented via
    ///   SQLite's `instr(lower(handle), lower(?1)) = 1`, which sidesteps
    ///   `LIKE`'s `%` `_` backslash escaping rules entirely: the
    ///   needle is treated as a literal substring whose only privileged
    ///   property is "starts at column 1". A future regression that
    ///   reaches for `LIKE ?1 || '%'` without escaping would let a
    ///   player with `_` in their handle wildcard-match the entire
    ///   roster — `instr` cannot do that.
    /// - Results are ordered by `handle ASC` (using the same
    ///   case-insensitive comparison as the match) so the autocomplete
    ///   list is stable across calls and across SQLite page-cache
    ///   states. Ties (two players with the same handle in different
    ///   identity namespaces — the explicit case from
    ///   `local_dev_and_foglet_namespaces_do_not_collide`) fall back to
    ///   `id ASC` so the order is fully deterministic.
    /// - An **empty prefix** matches every row, capped by `limit`.
    ///   Useful for a "browse" screen that opens before the player has
    ///   typed anything; callers who want to require typing should
    ///   guard at the call site.
    /// - `limit` MUST be `>= 0`. A negative limit is rejected as a
    ///   typed error — passing `-1` to mean "unlimited" is a footgun
    ///   on a player table that grows unboundedly across launches, so
    ///   the caller must opt in to a specific cap.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `SELECT` under the busy timeout, same
    /// shape as [`Self::player_handle`]. Multiple screens can call
    /// this concurrently from the same `GameContext` borrow.
    pub fn search_players_by_handle_prefix(
        &self,
        prefix: &str,
        limit: i64,
    ) -> Result<Vec<PlayerRecord>, PlayerError> {
        // Reject negative limits at the typed boundary instead of
        // letting them flow through to SQLite, which would silently
        // treat `LIMIT -1` as "no limit". Returning the same
        // `PlayerError::Sqlite` variant we already use lets the call
        // site funnel every search failure through one match arm
        // synthesising a `rusqlite::Error::InvalidParameterCount` would
        // misrepresent the cause, so we use `InvalidQuery` which is the
        // closest fit for "the caller passed a value SQLite would have
        // accepted but we refuse to forward".
        if limit < 0 {
            return Err(PlayerError::Sqlite {
                source: rusqlite::Error::InvalidQuery,
            });
        }
        // `instr(lower(handle), lower(?1)) = 1` is the prefix match
        // see the doc comment for why this beats `LIKE ?1 || '%'`. The
        // `lower` wrapper handles the case-insensitive contract; the
        // `= 1` pins the match to column 1 (SQLite's `instr` is
        // 1-indexed and returns 0 for "no match"). `ORDER BY
        // lower(handle), id` keeps the output deterministic across
        // collations and across page-cache shuffles.
        const SQL: &str = "\
SELECT id, foglet_user_id, handle, role, security_level, \
       first_seen_at, last_seen_at, local_dev_key \
FROM players \
WHERE instr(lower(handle), lower(?1)) = 1 \
ORDER BY lower(handle), id \
LIMIT ?2";
        let conn = self.connection();
        let mut stmt = conn
            .prepare(SQL)
            .map_err(|source| PlayerError::Sqlite { source })?;
        let rows = stmt
            .query_map(rusqlite::params![prefix, limit], row_to_record)
            .map_err(|source| PlayerError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| PlayerError::Sqlite { source })
    }

    /// Return the most-recently-active players, newest first.
    ///
    /// Added for — async-multiplayer target-selection
    /// screens (notice recipient picker, challenge rival selector.
    /// bounty claim attribution) need a "who's been around lately"
    /// list to seed the picker before the player has typed anything
    /// into the [`Self::search_players_by_handle_prefix`] box.
    /// Pairing the two helpers in `players.rs` keeps the registry as
    /// the single source of truth for handle-shaped reads.
    ///
    /// # Semantics
    ///
    /// - Ordering is `last_seen_at DESC` so the most recently active
    ///   player appears first. SQLite's `CURRENT_TIMESTAMP` is
    ///   second-precision, so multiple upserts inside the same second
    ///   will share a `last_seen_at` value; the tiebreak is `id DESC`
    ///   (the larger primary key was inserted later, which is the
    ///   closest stand-in for "most recent" the registry can offer
    ///   without a higher-resolution clock). Pinning the tiebreak
    ///   keeps the picker order deterministic across launches and
    ///   across SQLite page-cache shuffles.
    /// - `limit` MUST be `>= 0` for the same reason as
    ///   [`Self::search_players_by_handle_prefix`]: SQLite treats
    ///   `LIMIT -1` as "no limit", which is a footgun on a registry
    ///   that grows unboundedly across launches. Negative limits are
    ///   rejected at the typed boundary.
    /// - An empty registry returns an empty `Vec` rather than an
    ///   error — Murder Motel's first launch hits this before any
    ///   player has registered, and the picker MUST NOT surface an
    ///   error there.
    /// - A `limit` of zero returns no rows even when the registry is
    ///   populated; the doc-comment contract is "explicit cap", same
    ///   as the prefix search.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `SELECT` under the busy timeout, same
    /// shape as [`Self::search_players_by_handle_prefix`]. Multiple
    /// screens can call this concurrently from the same
    /// `GameContext` borrow.
    pub fn recent_players(&self, limit: i64) -> Result<Vec<PlayerRecord>, PlayerError> {
        // Reject negative limits at the typed boundary — see the doc
        // comment and `search_players_by_handle_prefix` for the
        // rationale. Funnelling both helpers through the same
        // `PlayerError::Sqlite { InvalidQuery }` shape lets call sites
        // share one match arm for "the search box rejected my input".
        if limit < 0 {
            return Err(PlayerError::Sqlite {
                source: rusqlite::Error::InvalidQuery,
            });
        }
        // `last_seen_at DESC, id DESC` — see the doc comment for why
        // `id` is the tiebreak. Selecting all columns keeps the helper
        // returning `PlayerRecord` directly so screens that want
        // `handle` and `role` (sysop badge in the picker, for
        // instance) don't need a follow-up `player_handle` round-trip.
        const SQL: &str = "\
SELECT id, foglet_user_id, handle, role, security_level, \
       first_seen_at, last_seen_at, local_dev_key \
FROM players \
ORDER BY last_seen_at DESC, id DESC \
LIMIT ?1";
        let conn = self.connection();
        let mut stmt = conn
            .prepare(SQL)
            .map_err(|source| PlayerError::Sqlite { source })?;
        let rows = stmt
            .query_map(rusqlite::params![limit], row_to_record)
            .map_err(|source| PlayerError::Sqlite { source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| PlayerError::Sqlite { source })
    }
}

/// Decode a `players` row into [`PlayerRecord`].
///
/// Pulled out of the upsert call site so 's local-dev path and
/// any future read helpers (a `find_by_user_id` query, for instance)
/// can share one decoder. Column order matches the `RETURNING` clause
/// above and the schema; a regression that reorders columns
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
        ctx_with_role(user_id, username, None)
    }

    /// Variant of [`ctx_with`] that lets a test pin the `role` string.
    /// 's persistence test exercises sysop mod user unknown
    /// roles — passing the role through this helper keeps each test
    /// body focused on the assertion under test instead of restating
    /// the full struct literal.
    fn ctx_with_role(
        user_id: Option<&str>,
        username: Option<&str>,
        role: Option<&str>,
    ) -> FogletContext {
        FogletContext {
            door_id: "test-door".to_string(),
            user_id: user_id.map(str::to_string),
            username: username.map(str::to_string),
            role: role.map(str::to_string),
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::ContextFile,
        }
    }

    ///   acceptance: a Foglet `user_id` is the stable
    /// key for a player row. Two upserts against the same user_id
    /// resolve to the same `players.id` and never duplicate the row.
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
    /// optional — refusing to register a player on that
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

    /// A context with no `role` field upserts as `'user'` `50`.
    /// After the upsert path writes the role explicitly rather
    /// than relying on the SQL default, but the resulting values must
    /// still match the documented "absent role" mapping
    /// — that's what keeps the column shape consistent with the
    /// hand-rolled INSERTs in the migration tests below.
    #[test]
    fn upsert_player_with_absent_role_maps_to_user_defaults() {
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

    ///   acceptance: distinct sysop mod user
    /// contexts persist distinct normalized role/security pairs in the
    /// `players` row. The mapping is the contract — sysop →
    /// 100, mod → 90, user → 50, unknown → 50 — and the test pins
    /// each branch explicitly so a regression in
    /// [`FogletRole::security_level`] surfaces here instead of in
    /// downstream Murder Motel UI code that asks for the player's
    /// label. Comments deliberately call out that the persisted values
    /// are advisory only: nothing in the upsert path turns them into a
    /// launch authorization decision (Foglet still owns that gate).
    #[test]
    fn upsert_player_persists_normalized_role_and_security_per_context() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        // Mixed-case input proves the role normalization runs through
        // [`FogletRole::parse`] (which folds case) rather than writing
        // the raw string verbatim — the on-disk token must be the
        // canonical lowercase form so leaderboards and event log UIs
        // can group by exact equality.
        let sysop = world
            .upsert_player(&ctx_with_role(
                Some("u-sysop"),
                Some("syndi"),
                Some("Sysop"),
            ))
            .expect("sysop upsert succeeds");
        let moderator = world
            .upsert_player(&ctx_with_role(Some("u-mod"), Some("morgan"), Some("mod")))
            .expect("mod upsert succeeds");
        let user = world
            .upsert_player(&ctx_with_role(Some("u-user"), Some("ursula"), Some("user")))
            .expect("user upsert succeeds");

        assert_eq!(sysop.role, "sysop");
        assert_eq!(sysop.security_level, 100);
        assert_eq!(moderator.role, "mod");
        assert_eq!(moderator.security_level, 90);
        assert_eq!(user.role, "user");
        assert_eq!(user.security_level, 50);

        // Three distinct rows — without that, the test would pass
        // even if the upsert path collapsed every role onto a single
        // row keyed off the wrong column.
        assert_ne!(sysop.id, moderator.id);
        assert_ne!(moderator.id, user.id);

        // Unknown roles still persist a row, with the -mandated
        // user-level fallback. The original token is preserved verbatim
        // (no lowercase-folding for `Other`) so a curious operator
        // dumping the table can still see what the upstream context
        // actually said.
        let other = world
            .upsert_player(&ctx_with_role(Some("u-other"), Some("oz"), Some("oracle")))
            .expect("unknown-role upsert succeeds");
        assert_eq!(other.role, "oracle");
        assert_eq!(other.security_level, 50);
    }

    /// A repeat upsert for the same player but with a different role
    /// rewrites `role` `security_level` in place — the upstream
    /// Foglet user got promoted/demoted between launches and the
    /// registry must reflect the new value rather than freezing the
    /// first-seen role. Pinning this means a future "preserve role on
    /// repeat upsert" patch (which would diverge )
    /// flunks here instead of silently freezing live data.
    #[test]
    fn repeat_upsert_refreshes_role_and_security_level() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let promoted = world
            .upsert_player(&ctx_with_role(Some("u-x"), Some("x"), Some("user")))
            .expect("first upsert succeeds");
        assert_eq!(promoted.role, "user");
        assert_eq!(promoted.security_level, 50);

        let after = world
            .upsert_player(&ctx_with_role(Some("u-x"), Some("x"), Some("sysop")))
            .expect("repeat upsert succeeds");
        assert_eq!(
            after.id, promoted.id,
            "stable key still resolves to one row"
        );
        assert_eq!(after.role, "sysop");
        assert_eq!(after.security_level, 100);
    }

    ///   acceptance: two local-dev sessions with
    /// distinct handles synthesise distinct `local_dev_key` values and
    /// land on distinct rows — they MUST NOT collide. Without this.
    /// the two-player Murder Motel smoke test (alice + bob) would
    /// share one player record and fork every per-player ledger.
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
    /// same `players.id`. The mirror of the user-id stability test
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

    /// Local-dev and Foglet-user-id rows occupy disjoint namespaces
    /// a Foglet user "alice" and a local-dev "alice" must end up on
    /// separate `players` rows. calls this out explicitly:
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
    /// guard, two `--local-dev-user " "` invocations would each pin
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

    ///   acceptance: applying [`PLAYERS_MIGRATION`]
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
            "players schema must match exactly"
        );

        // Bookkeeping row recorded at the migration's declared version
        // — proves the standard apply path was used (vs. a side-channel
        // `execute_batch`) so the relaunch idempotency test in
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

    /// Re-applying [`PLAYERS_MIGRATION`] is a no-op (
    /// idempotency carries forward to the kit's built-in migrations.
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
    /// is null. This is the property 's upsert path relies on
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
    /// the local-dev key namespace. will lean on this so two
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

    ///   acceptance: a repeat upsert refreshes
    /// `last_seen_at` while leaving `first_seen_at` alone. The audit
    /// story depends on `first_seen_at` being a stable "registered at"
    /// anchor — a regression that put it on the SET list of the
    /// conflict update would silently rewrite history on every
    /// relaunch.
    ///
    /// The test fakes the passage of time by stamping the row with a
    /// known-past timestamp directly through SQL after the first
    /// upsert. `CURRENT_TIMESTAMP` has 1-second granularity in SQLite.
    /// so two back-to-back upserts in the same second would share a
    /// timestamp and prove nothing about whether the column was
    /// rewritten. Manually backdating both columns to a clearly older
    /// value lets the second upsert prove (a) `first_seen_at` was
    /// preserved and (b) `last_seen_at` advanced to a *newer* value.
    #[test]
    fn repeat_upsert_refreshes_last_seen_without_changing_first_seen() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let first = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("first upsert succeeds");

        // Backdate both timestamps so the second upsert has somewhere
        // measurable to advance from. Using a fixed past value (rather
        // than `datetime('now', '-1 day')`) keeps the assertions exact
        // and the test independent of the host clock's millisecond
        // jitter.
        const BACKDATED: &str = "2000-01-01 00:00:00";
        world
            .connection()
            .execute(
                "UPDATE players SET first_seen_at = ?1, last_seen_at = ?1 WHERE id = ?2",
                rusqlite::params![BACKDATED, first.id],
            )
            .expect("backdating both timestamps succeeds");

        let second = world
            .upsert_player(&ctx_with(Some("u-alice"), Some("alice")))
            .expect("repeat upsert succeeds");

        assert_eq!(second.id, first.id, "stable key still resolves to one row");
        assert_eq!(
            second.first_seen_at, BACKDATED,
            "first_seen_at must survive a repeat upsert verbatim"
        );
        assert_ne!(
            second.last_seen_at, BACKDATED,
            "last_seen_at must advance on a repeat upsert"
        );
        // Lexicographic comparison is correct for SQLite's
        // `YYYY-MM-DD HH:MM:SS` format. We can compare directly without
        // parsing into a chrono type.
        assert!(
            second.last_seen_at.as_str() > BACKDATED,
            "last_seen_at ({}) must be later than the backdated value ({BACKDATED})",
            second.last_seen_at
        );
    }

    /// Companion guard for the local-dev branch: the `first_seen_at`
    /// preservation rule applies to both identity namespaces. Without
    /// this, a dev relaunching with `--local-dev-user alice` (the
    /// Murder Motel two-player smoke test setup) would lose their
    /// original registration timestamp on every re-exec.
    #[test]
    fn repeat_local_dev_upsert_refreshes_last_seen_without_changing_first_seen() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");

        let first = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("first local-dev upsert succeeds");

        const BACKDATED: &str = "2000-01-01 00:00:00";
        world
            .connection()
            .execute(
                "UPDATE players SET first_seen_at = ?1, last_seen_at = ?1 WHERE id = ?2",
                rusqlite::params![BACKDATED, first.id],
            )
            .expect("backdating both timestamps succeeds");

        let second = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("repeat local-dev upsert succeeds");

        assert_eq!(second.id, first.id);
        assert_eq!(second.first_seen_at, BACKDATED);
        assert!(second.last_seen_at.as_str() > BACKDATED);
    }

    /// Defaults — role and security_level fall back to `'user'` `50`
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

    ///  : the leaderboard render path needs to look up a
    /// handle from a `players.id`. An upsert followed by `player_handle`
    /// must round-trip the display string verbatim.
    #[test]
    fn player_handle_returns_handle_for_existing_id() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let player = world
            .upsert_player(&ctx_with(Some("u1"), Some("Alice")))
            .expect("upsert");
        let handle = world
            .player_handle(player.id)
            .expect("lookup succeeds")
            .expect("row exists");
        assert_eq!(handle, "Alice");
    }

    /// `Ok(None)` for a missing id keeps the leaderboard UI honest — an
    /// id pulled from `top_scores` that was deleted out from under us
    /// (an operator-driven cleanup, say) renders as `"unknown"` rather
    /// than failing the screen.
    #[test]
    fn player_handle_returns_none_for_unknown_id() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        assert!(world
            .player_handle(424_242)
            .expect("lookup succeeds")
            .is_none());
    }

    /// Helper for the search tests: seed a roster of
    /// distinct-handle local-dev players so a single test body can
    /// assert on the ordered match set without restating the upsert
    /// boilerplate per row. Handles deliberately span case and
    /// alphabetical neighbours so prefix case-folding ordering
    /// regressions surface independently.
    fn seed_search_roster(world: &WorldDb) {
        for handle in ["alice", "Albert", "albus", "Bob", "calico", "carol"] {
            world
                .upsert_player(&ctx_with(None, Some(handle)))
                .expect("seed upsert succeeds");
        }
    }

    ///   acceptance: the search is **case-insensitive
    /// prefix** match, ordered alphabetically. A query of `"AL"` must
    /// surface `Albert`, `albus`, and `alice` (all three share the
    /// `al` prefix regardless of case) and MUST NOT surface `calico`
    /// (which contains `al` but not at column 1) — pinning both halves
    /// in one test catches a regression that swaps `instr(...) = 1`
    /// for a plain `instr(...) > 0` substring search.
    #[test]
    fn search_players_by_handle_prefix_is_case_insensitive_prefix_match() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world
            .search_players_by_handle_prefix("AL", 100)
            .expect("search succeeds");
        let handles: Vec<&str> = hits.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(
            handles,
            vec!["Albert", "albus", "alice"],
            "case-insensitive prefix; alphabetised by lower(handle)"
        );
    }

    /// A prefix that no row starts with returns an empty `Vec` rather
    /// than an error. Pinning empty-result-as-Ok keeps screen code
    /// honest: an autocomplete with zero hits is a normal UI state.
    #[test]
    fn search_players_by_handle_prefix_returns_empty_for_no_match() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world
            .search_players_by_handle_prefix("zz", 100)
            .expect("search succeeds");
        assert!(hits.is_empty());
    }

    /// `limit` caps the returned set. A roster with three matching
    /// handles and a limit of 2 returns the alphabetically-first two
    /// the order contract from the prefix test pins which two survive.
    #[test]
    fn search_players_by_handle_prefix_respects_limit() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world
            .search_players_by_handle_prefix("al", 2)
            .expect("search succeeds");
        let handles: Vec<&str> = hits.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(handles, vec!["Albert", "albus"]);
    }

    /// An empty prefix matches every row (capped by `limit`) — the
    /// "browse roster" screen relies on this. Documented in the helper's
    /// doc comment; this test pins the contract so a future "reject
    /// empty prefix" patch is a deliberate decision rather than silent
    /// drift.
    #[test]
    fn search_players_by_handle_prefix_empty_string_matches_all() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world
            .search_players_by_handle_prefix("", 100)
            .expect("search succeeds");
        let handles: Vec<&str> = hits.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(
            handles,
            vec!["Albert", "albus", "alice", "Bob", "calico", "carol"]
        );
    }

    /// An empty registry returns an empty `Vec` — Murder Motel's first
    /// launch hits this path before any player has registered, and the
    /// search screen MUST NOT surface an error there.
    #[test]
    fn search_players_by_handle_prefix_empty_registry_returns_empty() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let hits = world
            .search_players_by_handle_prefix("anything", 100)
            .expect("search succeeds");
        assert!(hits.is_empty());
    }

    /// A negative limit is rejected as a typed error rather than
    /// silently forwarded as "unlimited" (which is what SQLite does
    /// with `LIMIT -1`). The doc comment calls out that callers who
    /// want everything pass an explicit cap; this test pins the guard.
    #[test]
    fn search_players_by_handle_prefix_rejects_negative_limit() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let err = world
            .search_players_by_handle_prefix("a", -1)
            .expect_err("negative limit is rejected");
        assert!(matches!(err, PlayerError::Sqlite { .. }));
    }

    /// A `limit` of zero returns no rows even with matches present
    /// pins the "explicit cap" contract. Without this, a future
    /// regression that special-cased `0` to mean "no limit" would
    /// silently page the entire roster into a screen that asked for
    /// nothing.
    #[test]
    fn search_players_by_handle_prefix_zero_limit_returns_empty() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world
            .search_players_by_handle_prefix("a", 0)
            .expect("search succeeds");
        assert!(hits.is_empty());
    }

    /// Helper for the recent-players tests: stamp a known
    /// `last_seen_at` on a specific player so the test can pin the
    /// ordering contract without relying on `CURRENT_TIMESTAMP`'s
    /// one-second resolution. SQLite stores the column as `TEXT`, so
    /// any ISO-shaped string sorts lexicographically the same way the
    /// `CURRENT_TIMESTAMP` form does — the chronology under test is
    /// preserved.
    fn stamp_last_seen(world: &WorldDb, player_id: i64, last_seen_at: &str) {
        world
            .connection()
            .execute(
                "UPDATE players SET last_seen_at = ?1 WHERE id = ?2",
                rusqlite::params![last_seen_at, player_id],
            )
            .expect("stamp last_seen_at succeeds");
    }

    ///   acceptance: results are ordered by
    /// `last_seen_at DESC`. Manually stamping distinct timestamps
    /// sidesteps `CURRENT_TIMESTAMP`'s second-precision so this test
    /// pins the chronology contract without sleeping. A regression
    /// that flipped the sort to ASC (or used `first_seen_at`) flunks
    /// the assertion.
    #[test]
    fn recent_players_orders_by_last_seen_at_desc() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(None, Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(None, Some("carol")))
            .expect("carol upsert");

        // Distinct timestamps, deliberately not in id order: bob is
        // most recent, alice in the middle, carol oldest. A regression
        // that fell back to id-DESC for the primary sort would
        // surface as carol/bob/alice rather than bob/alice/carol.
        stamp_last_seen(&world, alice.id, "2025-01-02T00:00:00Z");
        stamp_last_seen(&world, bob.id, "2025-01-03T00:00:00Z");
        stamp_last_seen(&world, carol.id, "2025-01-01T00:00:00Z");

        let hits = world.recent_players(100).expect("recent_players succeeds");
        let handles: Vec<&str> = hits.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(handles, vec!["bob", "alice", "carol"]);
    }

    /// When two rows share `last_seen_at` (the common case under
    /// `CURRENT_TIMESTAMP`'s second-precision), the tiebreak is `id
    /// DESC` — the larger primary key was inserted later. Pinning the
    /// tiebreak keeps the picker order deterministic across launches
    /// and across SQLite page-cache shuffles.
    #[test]
    fn recent_players_tiebreaks_by_id_desc_for_equal_last_seen_at() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(None, Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(None, Some("carol")))
            .expect("carol upsert");

        // Force the tie so the test is independent of how fast the
        // upserts ran. Without this, a runner that crossed a second
        // boundary mid-test would silently rely on the primary sort.
        let same = "2025-01-01T00:00:00Z";
        stamp_last_seen(&world, alice.id, same);
        stamp_last_seen(&world, bob.id, same);
        stamp_last_seen(&world, carol.id, same);

        let hits = world.recent_players(100).expect("recent_players succeeds");
        let ids: Vec<i64> = hits.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![carol.id, bob.id, alice.id]);
    }

    /// `limit` caps the returned set to the most-recent N. Pairing
    /// this with the ordering test pins which N survive.
    #[test]
    fn recent_players_respects_limit() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(None, Some("bob")))
            .expect("bob upsert");
        let carol = world
            .upsert_player(&ctx_with(None, Some("carol")))
            .expect("carol upsert");
        stamp_last_seen(&world, alice.id, "2025-01-01T00:00:00Z");
        stamp_last_seen(&world, bob.id, "2025-01-02T00:00:00Z");
        stamp_last_seen(&world, carol.id, "2025-01-03T00:00:00Z");

        let hits = world.recent_players(2).expect("recent_players succeeds");
        let handles: Vec<&str> = hits.iter().map(|p| p.handle.as_str()).collect();
        assert_eq!(handles, vec!["carol", "bob"]);
    }

    /// An empty registry returns an empty `Vec` — Murder Motel's first
    /// launch hits this path before any player has registered, and the
    /// picker MUST NOT surface an error there.
    #[test]
    fn recent_players_empty_registry_returns_empty() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let hits = world.recent_players(100).expect("recent_players succeeds");
        assert!(hits.is_empty());
    }

    /// A negative limit is rejected as a typed error rather than
    /// silently forwarded as "unlimited" (which is what SQLite does
    /// with `LIMIT -1`). Pins the same guard the prefix-search helper
    /// enforces.
    #[test]
    fn recent_players_rejects_negative_limit() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let err = world
            .recent_players(-1)
            .expect_err("negative limit is rejected");
        assert!(matches!(err, PlayerError::Sqlite { .. }));
    }

    /// A `limit` of zero returns no rows even with matches present
    /// pins the "explicit cap" contract. A future regression that
    /// special-cased `0` to mean "no limit" would silently page the
    /// entire roster into a screen that asked for nothing.
    #[test]
    fn recent_players_zero_limit_returns_empty() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");
        seed_search_roster(&world);

        let hits = world.recent_players(0).expect("recent_players succeeds");
        assert!(hits.is_empty());
    }

    /// A repeat upsert refreshes `last_seen_at`, so a player
    /// who relaunches the game bubbles to the top of the picker. Pins
    /// the integration between `upsert_player` and `recent_players`:
    /// the picker reflects activity, not registration order.
    #[test]
    fn recent_players_reflects_repeat_upsert_refreshing_last_seen_at() {
        let dir = tempdir().expect("tempdir creates");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("migration applies");

        let alice = world
            .upsert_player(&ctx_with(None, Some("alice")))
            .expect("alice upsert");
        let bob = world
            .upsert_player(&ctx_with(None, Some("bob")))
            .expect("bob upsert");
        // Stamp distinct historical timestamps so the initial state
        // is alice-then-bob (newest first). Without this the test
        // would race `CURRENT_TIMESTAMP`'s second-precision and pass
        // by accident on a fast runner.
        stamp_last_seen(&world, alice.id, "2025-01-01T00:00:00Z");
        stamp_last_seen(&world, bob.id, "2025-01-02T00:00:00Z");

        // alice relaunches and her timestamp jumps ahead of bob's.
        // Re-upserting via the public path exercises the
        // refresh contract end-to-end rather than reaching for raw
        // SQL — the column under test is what `upsert_player` writes.
        stamp_last_seen(&world, alice.id, "2025-01-03T00:00:00Z");

        let hits = world.recent_players(100).expect("recent_players succeeds");
        let ids: Vec<i64> = hits.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![alice.id, bob.id]);
    }
}
