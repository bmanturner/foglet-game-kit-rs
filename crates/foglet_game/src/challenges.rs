//! `challenges` — shared-world rival-challenge schema (SPEC_v3 §4.2 /
//! §Task 4a).
//!
//! v3 introduces durable async player-to-player challenges: one
//! investigator dares another to solve a clue chain or beat a score
//! within a deadline, the target accepts/declines, and (eventually) one
//! side resolves the challenge with a game-defined outcome. The whole
//! lifecycle sits on top of one `challenges` table whose shape is pinned
//! by [`CHALLENGES_MIGRATION`]. This module exists only to declare that
//! schema and prove it applies; the `Challenge` Rust type and the
//! `create_challenge` / `accept_challenge` / `decline_challenge` /
//! `resolve_challenge` / `expire_open_challenges` helpers land in
//! subsequent §Task 4 sub-items (4b–4g). Splitting the migration into
//! its own commit keeps the bisect signal sharp — a column rename, a
//! relaxed `CHECK`, or a dropped partial index flunks the schema test in
//! this module rather than a higher-level state-machine test that's
//! harder to attribute.
//!
//! # Why a dedicated table
//!
//! SPEC_v3 §3 lists challenges alongside notices, market listings,
//! factions, and bounties as separate primitives. We follow the same
//! v2/v3 convention as [`crate::notices`]: one table, one migration, one
//! named index family. Folding challenges onto the `world_events` log
//! would conflate the append-only event stream with mutable lifecycle
//! state (`state`, `accepted_at`, `resolved_at`) — fundamentally
//! different write patterns, the same reason notices got their own
//! table.
//!
//! # Why `version = 7`
//!
//! v2 occupies migration versions 1–5 (see `docs/shared-world.md`
//! §8.1). v3 claims `6` and above, dense and grouped per primitive.
//! Notices took version 6 (the first v3 primitive to land). Challenges
//! take version 7. Subsequent v3 migrations (market listings, factions,
//! bounties) MUST pick the next available kit version — game-authored
//! migrations live in their own higher band and are not affected.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// Schema for the rival-challenge table — SPEC_v3 §4.2 / §Task 4a.
///
/// One row per challenge. Challenges are mutable in the narrow sense
/// that `state`, `accepted_at`, `resolved_at`, and `result` are flipped
/// by the typed helpers landing in Tasks 4c–4f; the addressing,
/// `kind`, `stake`, and creation timestamp are write-once. The kit's
/// contract is "if you only go through the public API, the only state
/// changes are the documented transitions, and every transition runs
/// inside a SQLite transaction" (SPEC_v3 §4.2 / §7). An operator with
/// `sqlite3` can of course rewrite anything; that's the same caveat as
/// [`crate::events::WORLD_EVENTS_MIGRATION`] and
/// [`crate::notices::NOTICES_MIGRATION`].
///
/// # Column shape
///
/// - `id` — `INTEGER PRIMARY KEY`. Autoincrement-aliased rowid. Doubles
///   as the deterministic tiebreaker for queries that order by
///   `created_at` and need a stable secondary sort (matching the same
///   pattern as `notices` / `world_events`).
/// - `created_at` — `TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP`. UTC
///   timestamp written by SQLite at insert time. ISO text so it sorts
///   lexically the same way it sorts chronologically and reads cleanly
///   in the `sqlite3` CLI — the same contract as every other v2/v3
///   timestamp column.
/// - `challenger_player_id` — `INTEGER NOT NULL REFERENCES players(id)`.
///   The investigator initiating the challenge. Required: a challenge
///   without an attributable challenger has no resolution path (who
///   gets credit?), unlike notices where system messages are a real
///   use case. Foreign-keyed for the same reason as the turn ledger
///   and notices: a phantom id should never land here. SQLite enforces
///   FKs only when `PRAGMA foreign_keys = ON`, which the runtime is
///   responsible for; until then the constraint is documentation but
///   the column shape is correct.
/// - `target_player_id` — `INTEGER NOT NULL REFERENCES players(id)`.
///   The investigator being challenged. Required: every challenge has
///   exactly one addressee, mirroring the inbox model. "Open
///   challenges" (no specific target, anyone can accept) are
///   intentionally not modelled here — a future v3.1 sub-task can add
///   a nullable target if the gameplay calls for it; v3 ships only
///   directed challenges to keep the state machine simple.
/// - `kind` — `TEXT NOT NULL`. Short machine-readable label
///   (e.g. `"clue_race"`, `"deduction_duel"`). Game authors pick the
///   namespace; the kit's only rule is "round-trips as text". Used by
///   challenge UIs to filter listings by category and by game code to
///   route resolution logic.
/// - `stake` — `TEXT`, nullable. Opaque JSON describing what's wagered
///   (XP, in-game currency, a clue token). The kit treats this column
///   as opaque — the game owns the schema, the kit owns the lifecycle.
///   Same contract as `metadata` on notices and world events. Nullable
///   because some challenges may carry no stake (a friendly duel for
///   bragging rights), and modelling "no stake" as the absence of the
///   field is cleaner than reserving a sentinel JSON value.
/// - `state` — `TEXT NOT NULL DEFAULT 'open'` with a `CHECK` constraint
///   pinning `state IN ('open','accepted','declined','resolved',
///   'expired')`. The state-machine vocabulary lives in the schema so
///   a regression that introduced a new state in code without a
///   matching migration would fail at INSERT/UPDATE time, not silently
///   in production. SPEC §4.2 documents the legal transitions:
///   `open -> accepted -> resolved`, `open -> declined`,
///   `open -> expired`. Default `'open'` matches Task 4b's "starts in
///   open" acceptance.
/// - `accepted_at` — `TEXT`, nullable. ISO timestamp the target
///   accepted the challenge. Stays `NULL` for challenges that never
///   moved past `open` (declined, expired, or still open). Storing the
///   first-accepted timestamp lets a future audit view show "accepted
///   2026-05-09 17:42 UTC" without a separate event lookup, the same
///   trick `notices.read_at` plays for the read lifecycle.
/// - `resolved_at` — `TEXT`, nullable. ISO timestamp the challenge was
///   resolved. Only populated on the `accepted -> resolved` transition;
///   declined and expired challenges leave it `NULL`. Same audit-view
///   rationale as `accepted_at`.
/// - `expires_at` — `TEXT`, nullable. ISO timestamp after which an
///   `open` challenge can be transitioned to `expired` by Task 4f's
///   sweeper. Nullable so a challenge can be open-ended (no deadline)
///   without reserving a sentinel value; the partial index below
///   filters on `expires_at IS NOT NULL` so the sweeper only walks
///   challenges that *can* expire. SPEC §4.2 requires "Expired
///   challenges cannot be accepted" — the helper enforcing that lives
///   in Task 4c, but the timestamp it consults lives here.
/// - `result` — `TEXT`, nullable. Opaque JSON describing the
///   resolution outcome (winner, payouts, narrative beats). Game code
///   computes and stores this on the `accepted -> resolved`
///   transition; the kit treats it as opaque, same contract as
///   `stake`. Nullable so a challenge that hasn't been resolved (or
///   that ended in `declined`/`expired`) is distinguishable from one
///   that resolved with an empty result object — a sentinel JSON
///   would conflate the two.
///
/// # Indexes
///
/// Two partial indexes are created up-front so the lookup patterns
/// Tasks 4c–4f rely on are seek-bound from the moment they land.
/// Adding them later would require a follow-up migration and a
/// backfill window where the query path scans the table; pay the
/// index cost at the same migration that creates the table — the same
/// rationale as `idx_notices_inbox` on [`crate::notices`].
///
/// - `idx_challenges_target_open` is a partial index over
///   `(target_player_id, created_at, id)` `WHERE state = 'open'`. This
///   covers "open challenges aimed at me" — the inbox-like screen the
///   target sees when deciding whether to accept or decline. The
///   partial predicate keeps the index small (resolved/declined/
///   expired challenges fall out of the index entirely once they leave
///   `open`) and matches the planned default query exactly.
/// - `idx_challenges_open_expiring` is a partial index over
///   `(expires_at, id)` `WHERE state = 'open' AND expires_at IS NOT
///   NULL`. The Task 4f sweeper walks this in `expires_at` order and
///   stops at the first row where `expires_at > now`, so the cost of
///   "expire all due challenges" stays proportional to the number of
///   challenges that actually need expiring — not to the total
///   challenge count. Same shape as the partial-index strategy on
///   notices.
///
/// # Version
///
/// `version = 7`. Notices claim version 6 (see
/// [`crate::notices::NOTICES_MIGRATION`]); challenges are the second
/// v3 primitive to land, so they take 7. Subsequent v3 migrations
/// (market listings, factions, bounties) take 8 and onward.
pub const CHALLENGES_MIGRATION: WorldMigration = WorldMigration {
    version: 7,
    name: "create_challenges",
    sql: "\
CREATE TABLE IF NOT EXISTS challenges (\n\
    id                    INTEGER PRIMARY KEY,\n\
    created_at            TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    challenger_player_id  INTEGER NOT NULL REFERENCES players(id),\n\
    target_player_id      INTEGER NOT NULL REFERENCES players(id),\n\
    kind                  TEXT NOT NULL,\n\
    stake                 TEXT,\n\
    state                 TEXT NOT NULL DEFAULT 'open'\n\
        CHECK (state IN ('open','accepted','declined','resolved','expired')),\n\
    accepted_at           TEXT,\n\
    resolved_at           TEXT,\n\
    expires_at            TEXT,\n\
    result                TEXT\n\
);\n\
CREATE INDEX IF NOT EXISTS idx_challenges_target_open\n\
    ON challenges(target_player_id, created_at, id) WHERE state = 'open';\n\
CREATE INDEX IF NOT EXISTS idx_challenges_open_expiring\n\
    ON challenges(expires_at, id) WHERE state = 'open' AND expires_at IS NOT NULL;\n\
",
};

/// Decoded `challenges` row — SPEC_v3 §4.2 read model.
///
/// Mirrors the column shape pinned by [`CHALLENGES_MIGRATION`]
/// one-for-one, in the same order, so the SQL `RETURNING` clause and
/// the `query_map` row decoder share a single column list. Authoring
/// code consumes this struct rather than reaching into raw
/// `rusqlite::Row`s — that keeps the schema-to-Rust mapping in one
/// place and turns a column rename into a single compile error
/// instead of a fan-out of decode failures.
///
/// All timestamps stay as raw SQLite ISO text, the same contract as
/// [`crate::notices::Notice`] and [`crate::events::EventRecord`]:
/// parsing into a richer type would be a one-way trip that hides
/// corrupt data and forces a chrono / time dependency on every
/// consumer. `stake` and `result` are likewise opaque text — game
/// code that wants structured payloads serialises JSON before
/// handing it to the kit, and decodes on read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// Autoincrement primary key. Doubles as the deterministic
    /// tiebreaker for queries that order by `created_at` and need a
    /// stable secondary sort, the same role as `notices.id`.
    pub id: i64,
    /// UTC timestamp written by SQLite at insert time
    /// (`CURRENT_TIMESTAMP`). Kept as ISO text — see struct docs.
    pub created_at: String,
    /// Challenger's `players.id`. Required by the schema — every
    /// challenge has exactly one initiator.
    pub challenger_player_id: i64,
    /// Target's `players.id`. Required by the schema — v3 ships only
    /// directed challenges (see [`CHALLENGES_MIGRATION`] notes).
    pub target_player_id: i64,
    /// Game-authored kind label (e.g. `"clue_race"`,
    /// `"deduction_duel"`). Round-tripped verbatim; the kit imposes
    /// no namespace.
    pub kind: String,
    /// Optional opaque stake payload (typically JSON). Stored as text
    /// so `sqlite3 -json` can pretty-print it; the kit does not parse
    /// it. `None` for a friendly duel with no wager.
    pub stake: Option<String>,
    /// Lifecycle state — one of `open`, `accepted`, `declined`,
    /// `resolved`, `expired`. The schema-level `CHECK` constraint
    /// pins the vocabulary; see [`CHALLENGES_MIGRATION`].
    pub state: String,
    /// ISO timestamp the target accepted the challenge, or `None`
    /// while it has not been accepted (still open, declined,
    /// expired).
    pub accepted_at: Option<String>,
    /// ISO timestamp the challenge was resolved, or `None` if it has
    /// not been resolved.
    pub resolved_at: Option<String>,
    /// Optional ISO deadline. After this time, an `open` challenge
    /// can be transitioned to `expired` by the Task 4f sweeper.
    /// `None` means open-ended (no deadline).
    pub expires_at: Option<String>,
    /// Optional opaque result payload (typically JSON describing
    /// winner, payouts, narrative beats). Populated by Task 4e on
    /// the `accepted -> resolved` transition; the kit treats it as
    /// opaque, same contract as `stake`.
    pub result: Option<String>,
}

/// Lifecycle state for a [`Challenge`] — SPEC_v3 §4.2 vocabulary.
///
/// The kit's helpers ([`WorldDb::create_challenge`] and the upcoming
/// 4c–4f transitions) use this enum at their boundaries so call
/// sites get exhaustive matches and a typed transition target rather
/// than stringly-typed magic. The wire/storage representation stays
/// as `TEXT` (see [`CHALLENGES_MIGRATION`]); [`Self::as_str`] is the
/// one place the mapping lives so a future state addition is a
/// single edit, schema migration plus enum variant.
///
/// Values are listed in the natural lifecycle order — `Open` first,
/// terminal states last — so `Debug` output reads naturally in
/// failure messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChallengeState {
    /// Freshly created and awaiting the target's accept/decline.
    /// SPEC §4.2 default; matches the schema-level `DEFAULT 'open'`.
    Open,
    /// Target accepted; the challenge is in flight and awaiting
    /// resolution by Task 4e.
    Accepted,
    /// Target declined the challenge. Terminal state.
    Declined,
    /// Resolved by game code via Task 4e. Terminal state; carries
    /// the `result` payload.
    Resolved,
    /// Expired without acceptance via Task 4f's deadline sweeper.
    /// Terminal state.
    Expired,
}

impl ChallengeState {
    /// Short text encoding used in the `state` column and the
    /// schema-level `CHECK` constraint. The kit never persists a
    /// state via any other path, so this method is the single
    /// source-of-truth for how the enum hits SQLite.
    pub fn as_str(self) -> &'static str {
        match self {
            ChallengeState::Open => "open",
            ChallengeState::Accepted => "accepted",
            ChallengeState::Declined => "declined",
            ChallengeState::Resolved => "resolved",
            ChallengeState::Expired => "expired",
        }
    }
}

/// Failure modes for [`WorldDb::create_challenge`] and the upcoming
/// challenge-lifecycle helpers.
///
/// Library-internal `thiserror` shape — the runtime wraps these with
/// `anyhow` at the process boundary. Mirrors
/// [`crate::notices::NoticeError`] so all v3 multiplayer write paths
/// surface errors with the same shape (a future code review pass
/// can fold these into a single multiplayer-error trait if a third
/// primitive needs the same variants, but two primitives doesn't
/// justify the abstraction yet — SPEC tenet "no premature
/// abstraction").
///
/// Task 4b–4e need: `EmptyKind`, `EmptyResult`, `Sqlite`,
/// `NotFound`, `InvalidTransition`, and `Expired`. Validation
/// variants for length caps or self-challenge are deferred to
/// follow-up tasks if SPEC ever calls for them; SPEC §4.2 does not
/// require either.
#[derive(Debug, Error)]
pub enum ChallengeError {
    /// `kind` was empty. SPEC §4.2 lists `kind` as required; the kit
    /// additionally rejects an empty string here so a challenge can
    /// always be filtered/rendered by category. A regression that
    /// silently accepted `""` would surface as a phantom row in
    /// every "challenges of kind X" query.
    #[error("challenge kind must not be empty")]
    EmptyKind,
    /// `result` was empty on a resolve call. SPEC §4.2 lists
    /// `result JSON` as the resolution payload; the kit additionally
    /// rejects an empty string at the boundary so a regression that
    /// dropped the result mid-call (an `unwrap_or_default()` pattern,
    /// say) surfaces as a typed error rather than as an
    /// indistinguishable-from-declined `""` payload in the audit
    /// view. Game code that genuinely has no structured result MUST
    /// still pass an explicit JSON value (e.g. `"{}"`) so the
    /// "resolved with no payload" case is intentional, not
    /// accidental.
    #[error("challenge result must not be empty")]
    EmptyResult,
    /// The `INSERT … RETURNING` round-trip (or `UPDATE` on a
    /// transition) failed. Wrapping `rusqlite::Error` keeps the call
    /// site readable while preserving the underlying cause for
    /// `tracing` and operator-facing messages.
    #[error("failed to write challenge to world database: {source}")]
    Sqlite {
        /// Underlying `rusqlite` error from the statement.
        #[source]
        source: rusqlite::Error,
    },
    /// No `challenges` row exists with the given id. Surfaced by
    /// transition helpers (`accept_challenge`, `decline_challenge`,
    /// `resolve_challenge`) when the caller's id is stale or the
    /// row has been removed by an operator.
    #[error("challenge {id} does not exist")]
    NotFound {
        /// The id the caller looked up. Echoed so log lines and
        /// operator-facing errors can name the missing row.
        id: i64,
    },
    /// The transition is not legal from the challenge's current
    /// state. SPEC §4.2 mandates exactly four transitions:
    /// `open -> accepted`, `open -> declined`, `open -> expired`,
    /// `accepted -> resolved`. Every other (from, to) pair fails
    /// with this variant. Carrying both `from` (the actual state
    /// the row was found in) and the desired `to` lets the UI
    /// render a precise message — "this challenge has already been
    /// declined" vs "this challenge is in an unexpected state". The
    /// kit's helpers ([`Self::Expired`] is the one specialisation)
    /// would otherwise have to fall back on stringly-typed errors.
    #[error("cannot transition challenge {id} from {from:?} to {to:?}")]
    InvalidTransition {
        /// Challenge id the caller targeted. Echoed so log lines
        /// can name the offending row.
        id: i64,
        /// The state the row was in when the helper read it. Kept
        /// as a `String` (rather than [`ChallengeState`]) so a
        /// future schema state added without a matching enum
        /// variant still surfaces here verbatim instead of crashing
        /// on decode.
        from: String,
        /// The state the helper attempted to set.
        to: ChallengeState,
    },
    /// The challenge was still in `open` but its `expires_at`
    /// deadline has already passed. SPEC §4.2 requires "Expired
    /// challenges cannot be accepted"; a separate variant from
    /// [`Self::InvalidTransition`] lets the UI distinguish "the
    /// deadline lapsed before you got here" from "this challenge
    /// is in some other terminal state". Note that the row may
    /// still be `state = 'open'` in the database — the Task 4f
    /// sweeper hasn't run yet — but the helper refuses to accept
    /// regardless, so a slow sweeper can't widen the window in
    /// which an already-stale challenge is acceptable.
    #[error("challenge {id} expired before it could be accepted")]
    Expired {
        /// Challenge id the caller targeted.
        id: i64,
    },
}

impl WorldDb {
    /// Insert one row into `challenges` and return the canonical
    /// [`Challenge`] SQLite produced (SPEC_v3 §4.2 / §Task 4b).
    ///
    /// The contract is "the challenge I asked you to create is now
    /// durably in the table, addressed to the named target, in
    /// state `open`, with the id and `created_at` SQLite assigned,
    /// and `accepted_at` / `resolved_at` / `result` all still
    /// `NULL`". The state is intentionally not a parameter — SPEC
    /// §4.2 mandates "open" as the entry state, and the kit owns
    /// that invariant. The schema-level `DEFAULT 'open'` plus this
    /// helper's `RETURNING` round-trip keeps the lifecycle honest:
    /// even an operator who tampered with the helper signature
    /// can't smuggle a row in at `accepted` without also dropping
    /// the migration's CHECK.
    ///
    /// `challenger_player_id` and `target_player_id` are required
    /// by the schema. `kind` is required text (e.g. `"clue_race"`).
    /// `stake`, `expires_at` are optional — `None` means "no
    /// wager" / "no deadline". The `result` column is intentionally
    /// not a parameter on this path: a brand-new challenge has no
    /// result, and exposing it as a parameter would invite a
    /// regression where game code populated it before the
    /// `accepted -> resolved` transition.
    ///
    /// # Validation order
    ///
    /// The empty-kind check (the only validation Task 4b needs)
    /// runs **before** the SQL round-trip so a rejected challenge
    /// never produces a row, an autoincrement gap, or an event-log
    /// entry. Same rationale and ordering as
    /// [`Self::send_notice`].
    ///
    /// We use SQLite's `RETURNING` clause (≥ 3.35) to read the
    /// canonical row — `id`, the SQL-side `created_at`, plus every
    /// other column — without a second round-trip, the same pattern
    /// as [`Self::send_notice`] and [`Self::append_event`].
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single insert statement under the configured
    /// busy timeout. SPEC §4.2 requires "Challenge state transitions
    /// MUST be transactional"; the *creation* path is a single
    /// `INSERT` and thus already atomic, so no explicit
    /// transactional wrapper is needed here. The 4c–4f transition
    /// helpers will need explicit transactions because they read
    /// the current state, validate, and then write.
    pub fn create_challenge(
        &self,
        challenger_player_id: i64,
        target_player_id: i64,
        kind: &str,
        stake: Option<&str>,
        expires_at: Option<&str>,
    ) -> Result<Challenge, ChallengeError> {
        // Validation runs before the SQL round-trip so a rejected
        // challenge never produces a row. SPEC §4.2 lists `kind` as
        // required; the kit additionally rejects the empty string.
        if kind.is_empty() {
            return Err(ChallengeError::EmptyKind);
        }

        // `RETURNING` echoes the full row back — including the
        // SQL-side `CURRENT_TIMESTAMP` default for `created_at` and
        // the `'open'` default for `state`. The column order here
        // matches `row_to_challenge` so all read paths share one
        // decoder.
        const SQL: &str = "\
INSERT INTO challenges \
    (challenger_player_id, target_player_id, kind, stake, expires_at) \
VALUES (?1, ?2, ?3, ?4, ?5) \
RETURNING id, created_at, challenger_player_id, target_player_id, \
          kind, stake, state, accepted_at, resolved_at, expires_at, result";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    challenger_player_id,
                    target_player_id,
                    kind,
                    stake,
                    expires_at,
                ],
                row_to_challenge,
            )
            .map_err(|source| ChallengeError::Sqlite { source })
    }

    /// Transition a challenge from `open` to `accepted` and stamp
    /// `accepted_at` (SPEC_v3 §4.2 / §Task 4c).
    ///
    /// The contract is "if and only if the row was still open and
    /// not past its deadline, it is now `accepted` with an
    /// `accepted_at` timestamp; otherwise the row is unchanged and
    /// the helper returns a typed error explaining why". SPEC §4.2
    /// requires "Challenge state transitions MUST be transactional";
    /// the implementation is one conditional `UPDATE … RETURNING`
    /// statement, which is natively atomic in SQLite — same shape
    /// as [`WorldDb::mark_read`]. No explicit
    /// `BEGIN`/`COMMIT` is needed for a single statement; the
    /// transactional wrapper exists for multi-statement helpers
    /// (e.g. the upcoming market-buy path).
    ///
    /// # Conditional UPDATE shape
    ///
    /// The `WHERE` clause folds three checks into the UPDATE so the
    /// success path is a single round-trip:
    ///
    /// 1. `id = ?1` — addresses the row.
    /// 2. `state = 'open'` — only the `open -> accepted` transition
    ///    is legal (SPEC §4.2); any other current state must fall
    ///    through to the diagnostic SELECT and become an
    ///    [`ChallengeError::InvalidTransition`].
    /// 3. `expires_at IS NULL OR datetime(expires_at) > datetime('now')`
    ///    — open-ended challenges (`expires_at IS NULL`) are always
    ///    acceptable; deadlined challenges are acceptable only
    ///    while the deadline is still in the future. Wrapping both
    ///    sides in `datetime(…)` normalises the two ISO forms the
    ///    kit accepts (`'YYYY-MM-DDTHH:MM:SSZ'` from callers,
    ///    `'YYYY-MM-DD HH:MM:SS'` from `CURRENT_TIMESTAMP`) so the
    ///    comparison is chronological rather than lexicographic.
    ///
    /// The bookkeeping fields are written in the same statement:
    /// `state = 'accepted'` and `accepted_at = CURRENT_TIMESTAMP`
    /// (which `RETURNING` echoes back as the canonical row).
    ///
    /// # Diagnostic SELECT
    ///
    /// On `QueryReturnedNoRows` the helper performs one diagnostic
    /// SELECT to differentiate the failure modes — `NotFound` if
    /// no row exists, `Expired` if the row is still open but past
    /// its deadline, otherwise `InvalidTransition` carrying the
    /// row's actual state. The diagnostic is read-only and races
    /// only on the *error category* surfaced to the caller (a
    /// concurrent UPDATE could change the row between the failed
    /// UPDATE and the diagnostic SELECT); the helper never returns
    /// stale state because it only returns error categories on
    /// this path.
    ///
    /// # Failure
    ///
    /// - [`ChallengeError::NotFound`] — no row matches `id`.
    /// - [`ChallengeError::Expired`] — row is still `open` but its
    ///   `expires_at` deadline has passed.
    /// - [`ChallengeError::InvalidTransition`] — row is in any
    ///   state other than `open` (already accepted, declined,
    ///   resolved, or swept to `expired` by Task 4f).
    /// - [`ChallengeError::Sqlite`] — any other `rusqlite` error.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an optional
    /// diagnostic `SELECT` under the configured busy timeout. Same
    /// borrow shape as [`Self::create_challenge`] and
    /// [`WorldDb::mark_read`].
    pub fn accept_challenge(&self, challenge_id: i64) -> Result<Challenge, ChallengeError> {
        // Conditional UPDATE: only the `open + still-fresh` row
        // gets transitioned. SPEC §4.2 transitions are exhaustive
        // — any non-matching row falls through to the diagnostic
        // SELECT below for typed-error mapping.
        const UPDATE_SQL: &str = "\
UPDATE challenges \
SET state = 'accepted', accepted_at = CURRENT_TIMESTAMP \
WHERE id = ?1 \
  AND state = 'open' \
  AND (expires_at IS NULL OR datetime(expires_at) > datetime('now')) \
RETURNING id, created_at, challenger_player_id, target_player_id, \
          kind, stake, state, accepted_at, resolved_at, expires_at, result";

        match self.connection().query_row(
            UPDATE_SQL,
            rusqlite::params![challenge_id],
            row_to_challenge,
        ) {
            Ok(challenge) => Ok(challenge),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(self.diagnose_failed_transition(challenge_id, ChallengeState::Accepted))
            }
            Err(source) => Err(ChallengeError::Sqlite { source }),
        }
    }

    /// Transition a challenge from `open` to `declined` (SPEC_v3 §4.2
    /// / §Task 4d).
    ///
    /// The contract is "if and only if the row was still `open`, it
    /// is now `declined`; otherwise the row is unchanged and the
    /// helper returns a typed error explaining why". SPEC §4.2 lists
    /// `open -> declined` as the only legal decline transition; any
    /// other current state (`accepted`, `declined`, `resolved`,
    /// `expired`) must surface as
    /// [`ChallengeError::InvalidTransition`].
    ///
    /// Unlike [`Self::accept_challenge`], the decline path does
    /// **not** gate on `expires_at`: a target can always decline
    /// while the row is open, even if its deadline has lapsed and
    /// the sweeper hasn't run yet. SPEC §4.2's "Expired challenges
    /// cannot be accepted" is specifically scoped to the accept
    /// transition; declining a stale challenge is harmless and
    /// occasionally the right outcome (the target sees the lapsed
    /// challenge in their inbox and explicitly says "no thanks"
    /// before the sweeper runs).
    ///
    /// There is no `declined_at` column in the schema (see
    /// [`CHALLENGES_MIGRATION`]) — declined is a terminal state with
    /// no follow-up audit timestamp, so the transition only flips
    /// `state`. If a future SPEC revision adds a decline timestamp,
    /// the migration and `Challenge` struct change first, and this
    /// helper stamps it in the same `UPDATE`.
    ///
    /// # Failure
    ///
    /// - [`ChallengeError::NotFound`] — no row matches `id`.
    /// - [`ChallengeError::InvalidTransition`] — row is in any state
    ///   other than `open`.
    /// - [`ChallengeError::Sqlite`] — any other `rusqlite` error.
    ///
    /// Note that [`ChallengeError::Expired`] is intentionally not in
    /// the failure set: per SPEC §4.2 the expired-deadline gate
    /// applies only to acceptance, and the shared
    /// `diagnose_failed_transition` helper already scopes `Expired`
    /// to `attempted == Accepted` so a future caller can't
    /// accidentally surface it on the decline path.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an optional
    /// diagnostic `SELECT` under the configured busy timeout. Same
    /// borrow shape as [`Self::accept_challenge`].
    pub fn decline_challenge(&self, challenge_id: i64) -> Result<Challenge, ChallengeError> {
        // Conditional UPDATE: only an `open` row gets transitioned.
        // No deadline gate — see helper docs for the SPEC §4.2
        // rationale.
        const UPDATE_SQL: &str = "\
UPDATE challenges \
SET state = 'declined' \
WHERE id = ?1 \
  AND state = 'open' \
RETURNING id, created_at, challenger_player_id, target_player_id, \
          kind, stake, state, accepted_at, resolved_at, expires_at, result";

        match self.connection().query_row(
            UPDATE_SQL,
            rusqlite::params![challenge_id],
            row_to_challenge,
        ) {
            Ok(challenge) => Ok(challenge),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(self.diagnose_failed_transition(challenge_id, ChallengeState::Declined))
            }
            Err(source) => Err(ChallengeError::Sqlite { source }),
        }
    }

    /// Transition a challenge from `accepted` to `resolved` and stamp
    /// `resolved_at` + `result` (SPEC_v3 §4.2 / §Task 4e).
    ///
    /// The contract is "if and only if the row was in `accepted`, it
    /// is now `resolved` with a `resolved_at` timestamp and the caller-
    /// provided `result` payload; otherwise the row is unchanged and
    /// the helper returns a typed error explaining why". SPEC §4.2
    /// lists `accepted -> resolved` as the only legal resolve
    /// transition; any other current state (`open`, `declined`,
    /// `resolved`, `expired`) must surface as
    /// [`ChallengeError::InvalidTransition`].
    ///
    /// Game code owns result calculation (SPEC §4.2 "Game code owns
    /// result calculation; the kit owns durable lifecycle
    /// invariants"). The kit treats `result` as opaque text — same
    /// contract as `stake` on create — so a future `result` shape
    /// change is a game-side concern that doesn't require a kit
    /// migration.
    ///
    /// # Conditional UPDATE shape
    ///
    /// The `WHERE` clause folds two checks into the UPDATE so the
    /// success path is one round-trip:
    ///
    /// 1. `id = ?1` — addresses the row.
    /// 2. `state = 'accepted'` — only `accepted -> resolved` is
    ///    legal. SPEC §4.2 forbids resolving an `open` challenge
    ///    (the target hasn't agreed) and resolving any terminal
    ///    state (already resolved/declined/expired).
    ///
    /// The bookkeeping fields are written in the same statement:
    /// `state = 'resolved'`, `resolved_at = CURRENT_TIMESTAMP`, and
    /// `result = ?2`. `RETURNING` echoes back the canonical row.
    ///
    /// Note this path does NOT gate on `expires_at`: an `accepted`
    /// challenge has already passed the deadline gate via
    /// [`Self::accept_challenge`], and the deadline is irrelevant
    /// once the challenge is in flight. SPEC §4.2's "Expired
    /// challenges cannot be accepted" is scoped to acceptance only,
    /// and the shared `diagnose_failed_transition` helper restricts
    /// `Expired` to `attempted == Accepted` so a future caller can't
    /// accidentally surface it on the resolve path.
    ///
    /// # Validation order
    ///
    /// The empty-`result` check runs **before** the SQL round-trip,
    /// same ordering rationale as [`Self::create_challenge`]: a
    /// rejected resolve never produces a state flip, never burns an
    /// autoincrement, and never lands a partial row.
    ///
    /// # Failure
    ///
    /// - [`ChallengeError::EmptyResult`] — caller passed `""`.
    /// - [`ChallengeError::NotFound`] — no row matches `id`.
    /// - [`ChallengeError::InvalidTransition`] — row is in any state
    ///   other than `accepted`.
    /// - [`ChallengeError::Sqlite`] — any other `rusqlite` error.
    ///
    /// # Concurrency
    ///
    /// Takes `&self`: a single `UPDATE … RETURNING` plus an optional
    /// diagnostic `SELECT` under the configured busy timeout. Same
    /// borrow shape as [`Self::accept_challenge`] and
    /// [`Self::decline_challenge`].
    pub fn resolve_challenge(
        &self,
        challenge_id: i64,
        result: &str,
    ) -> Result<Challenge, ChallengeError> {
        // Validation runs before the SQL round-trip so a rejected
        // resolve never produces a state flip — same ordering as
        // `create_challenge`'s `EmptyKind` check.
        if result.is_empty() {
            return Err(ChallengeError::EmptyResult);
        }

        // Conditional UPDATE: only an `accepted` row gets
        // transitioned. `result = ?2` is written in the same
        // statement so the row never exists in a "resolved with NULL
        // result" intermediate state — SPEC §4.2's `result JSON` is
        // mandatory on the resolved row.
        const UPDATE_SQL: &str = "\
UPDATE challenges \
SET state = 'resolved', resolved_at = CURRENT_TIMESTAMP, result = ?2 \
WHERE id = ?1 \
  AND state = 'accepted' \
RETURNING id, created_at, challenger_player_id, target_player_id, \
          kind, stake, state, accepted_at, resolved_at, expires_at, result";

        match self.connection().query_row(
            UPDATE_SQL,
            rusqlite::params![challenge_id, result],
            row_to_challenge,
        ) {
            Ok(challenge) => Ok(challenge),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(self.diagnose_failed_transition(challenge_id, ChallengeState::Resolved))
            }
            Err(source) => Err(ChallengeError::Sqlite { source }),
        }
    }

    /// Map a no-rows response from a transition `UPDATE` onto the
    /// right typed [`ChallengeError`] by reading the row state.
    ///
    /// Pulled out of [`Self::accept_challenge`] so the upcoming
    /// `decline_challenge` and `resolve_challenge` helpers (Tasks
    /// 4d/4e) can share the same diagnostic. The function does
    /// exactly one SELECT and returns the most specific error —
    /// `NotFound` > `Expired` > `InvalidTransition` — without ever
    /// returning `Ok`. Internal-only; not part of the public API.
    fn diagnose_failed_transition(
        &self,
        challenge_id: i64,
        attempted: ChallengeState,
    ) -> ChallengeError {
        // We need the current state plus a "is the deadline already
        // past?" flag. Compute the deadline check in SQL so it uses
        // the same `datetime()` normalisation as the UPDATE — a
        // mismatch here would let the diagnostic disagree with the
        // gate that produced the no-rows in the first place.
        const DIAG_SQL: &str = "\
SELECT state, \
       CASE \
           WHEN expires_at IS NOT NULL \
               AND datetime(expires_at) <= datetime('now') \
           THEN 1 ELSE 0 \
       END AS deadline_lapsed \
FROM challenges WHERE id = ?1";

        let row = self
            .connection()
            .query_row(DIAG_SQL, rusqlite::params![challenge_id], |row| {
                let state: String = row.get(0)?;
                let deadline_lapsed: i64 = row.get(1)?;
                Ok((state, deadline_lapsed != 0))
            });

        match row {
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                ChallengeError::NotFound { id: challenge_id }
            }
            Err(source) => ChallengeError::Sqlite { source },
            // Row still open but its deadline already lapsed — the
            // sweeper hasn't run, but the helper refuses to accept.
            // Only meaningful when `attempted == Accepted` (the only
            // transition that gates on the deadline today).
            Ok((state, true))
                if state == ChallengeState::Open.as_str()
                    && attempted == ChallengeState::Accepted =>
            {
                ChallengeError::Expired { id: challenge_id }
            }
            Ok((state, _)) => ChallengeError::InvalidTransition {
                id: challenge_id,
                from: state,
                to: attempted,
            },
        }
    }
}

/// Decode a `challenges` row into [`Challenge`].
///
/// Pulled out so the Task 4b write path and the upcoming 4c–4f
/// transition / query helpers can share one decoder. Column order
/// matches the `RETURNING` clause in [`WorldDb::create_challenge`];
/// a regression that reorders columns will surface here as a type
/// error rather than as a silent field swap. Same shape as
/// [`crate::notices::Notice`]'s `row_to_notice`.
fn row_to_challenge(row: &rusqlite::Row<'_>) -> rusqlite::Result<Challenge> {
    Ok(Challenge {
        id: row.get(0)?,
        created_at: row.get(1)?,
        challenger_player_id: row.get(2)?,
        target_player_id: row.get(3)?,
        kind: row.get(4)?,
        stake: row.get(5)?,
        state: row.get(6)?,
        accepted_at: row.get(7)?,
        resolved_at: row.get(8)?,
        expires_at: row.get(9)?,
        result: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::players::PLAYERS_MIGRATION;
    use tempfile::tempdir;

    /// Helper: build a [`FogletContext`] just complete enough for
    /// `upsert_player` to land a row. Mirrors the helper in
    /// `notices::tests`; deliberate duplication so the two
    /// primitives' tests don't reach across modules.
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

    /// Helper: open a fresh world DB with players + challenges
    /// migrations applied. Used by every Task 4 behavioural test.
    fn world_with_challenges() -> (tempfile::TempDir, WorldDb) {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");
        (dir, world)
    }

    /// SPEC_v3 §Task 4a acceptance: applying [`CHALLENGES_MIGRATION`]
    /// creates the documented `challenges` table with the column shape
    /// SPEC §4.2 pins. Asserts both:
    ///
    /// 1. The table exists in `sqlite_master` (so a regression that
    ///    silently dropped the migration body would flunk).
    /// 2. The columns and order match the SPEC §4.2 contract (so a
    ///    later edit that renames or reorders a column flunks here
    ///    rather than buried in a 4b–4g behavioural test).
    ///
    /// The players migration is applied first because `challenges`
    /// references `players(id)` via two foreign keys (challenger and
    /// target). With FK enforcement off (the SQLite default until the
    /// runtime turns it on) the migration would succeed even without
    /// the parent table, but exercising the real dependency order here
    /// mirrors how the runtime startup path drives migrations on a
    /// real door open.
    #[test]
    fn migration_creates_challenges_table() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'challenges'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs");
        assert_eq!(
            count, 1,
            "challenges table must exist after migration applies"
        );

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('challenges') ORDER BY cid")
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
                "challenger_player_id".to_string(),
                "target_player_id".to_string(),
                "kind".to_string(),
                "stake".to_string(),
                "state".to_string(),
                "accepted_at".to_string(),
                "resolved_at".to_string(),
                "expires_at".to_string(),
                "result".to_string(),
            ],
            "challenges column shape must match the SPEC_v3 §4.2 contract"
        );
    }

    /// SPEC §4.2 enumerates the legal states: `open`, `accepted`,
    /// `declined`, `resolved`, `expired`. The migration encodes that
    /// vocabulary via a `CHECK` constraint so a code-path that ever
    /// tried to write an out-of-vocabulary state (e.g. a typo like
    /// `'accpeted'`, or a future state added in code without a matching
    /// migration) fails at INSERT/UPDATE time rather than silently
    /// landing a corrupt row. Pin both halves here:
    ///
    /// 1. Every spec-listed state is accepted by the constraint.
    /// 2. An obviously-invalid state is rejected.
    ///
    /// This is the schema-level safety net for the typed state
    /// machine that lands in Tasks 4b–4f. The Rust helpers will
    /// additionally enforce *which* transitions are legal between
    /// states; this test only proves the alphabet itself is locked
    /// down.
    #[test]
    fn challenges_state_check_constraint_locks_vocabulary() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        // Two real player rows so the FK columns reference something
        // sensible; the FK itself isn't enforced without `PRAGMA
        // foreign_keys = ON`, but using real ids keeps the test honest
        // about what a production row looks like.
        let challenger_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-alice', 'alice') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert challenger");
        let target_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) \
                 VALUES ('u-bob', 'bob') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("insert target");

        // Every spec-listed state must be accepted by the CHECK.
        for state in ["open", "accepted", "declined", "resolved", "expired"] {
            world
                .connection()
                .execute(
                    "INSERT INTO challenges \
                     (challenger_player_id, target_player_id, kind, state) \
                     VALUES (?1, ?2, 'unit_test', ?3)",
                    rusqlite::params![challenger_id, target_id, state],
                )
                .unwrap_or_else(|err| panic!("state {state:?} must be accepted by CHECK: {err}"));
        }

        // An out-of-vocabulary state must be rejected. We pick an
        // obvious typo of "accepted" — the kind of mistake a future
        // refactor could plausibly introduce.
        let bogus = world.connection().execute(
            "INSERT INTO challenges \
             (challenger_player_id, target_player_id, kind, state) \
             VALUES (?1, ?2, 'unit_test', 'accpeted')",
            rusqlite::params![challenger_id, target_id],
        );
        assert!(
            bogus.is_err(),
            "CHECK constraint must reject states outside the SPEC §4.2 vocabulary"
        );
    }

    /// The Task 4c–4f helpers will read challenges via two query
    /// shapes: "open challenges aimed at this target" (the accept/
    /// decline screen) and "open challenges past their deadline" (the
    /// expiry sweeper). Both paths walk partial indexes the migration
    /// creates up-front; if either ever stops being created, the
    /// reads silently become full table scans. Pin both indexes
    /// (and their partial predicates) here so a regression flunks at
    /// `cargo test` rather than in production under load.
    ///
    /// Same rationale as `notices_inbox_partial_index_is_present` for
    /// `idx_notices_inbox` — one schema-level test per index family,
    /// asserting both its existence and the partial-predicate text.
    #[test]
    fn challenges_partial_indexes_are_present() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        // `sqlite_master.sql` for an index includes the partial
        // predicate text verbatim, so we can pin the index name and
        // its `WHERE` clause in one query.
        let target_open_sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_challenges_target_open'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_challenges_target_open");
        assert!(
            target_open_sql.contains("state = 'open'"),
            "idx_challenges_target_open must filter to state = 'open'; got: {target_open_sql}"
        );
        assert!(
            target_open_sql.contains("target_player_id"),
            "idx_challenges_target_open must lead with target_player_id; got: {target_open_sql}"
        );

        let expiring_sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master \
                 WHERE type = 'index' AND name = 'idx_challenges_open_expiring'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master query runs for idx_challenges_open_expiring");
        assert!(
            expiring_sql.contains("state = 'open'"),
            "idx_challenges_open_expiring must filter to state = 'open'; got: {expiring_sql}"
        );
        assert!(
            expiring_sql.contains("expires_at IS NOT NULL"),
            "idx_challenges_open_expiring must filter expires_at IS NOT NULL; got: {expiring_sql}"
        );
    }

    /// The migration is idempotent. v2's relaunch path applies the
    /// same migration list every open; v3 inherits that contract. A
    /// second `apply_migration(&CHALLENGES_MIGRATION)` MUST be a
    /// no-op (the version is already in `world_migrations`), not an
    /// error from `CREATE TABLE` on an existing table. Same shape as
    /// `notices_migration_is_idempotent`.
    #[test]
    fn challenges_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("first challenges migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("second challenges migration applies (idempotent)");
    }

    /// SPEC_v3 §Task 4b acceptance: a freshly created challenge
    /// lands in state `open`, with the schema-side `created_at`
    /// populated, the `RETURNING` row matching the request inputs,
    /// and `accepted_at` / `resolved_at` / `result` all `NULL`.
    /// The "starts in open" half is the explicit Task 4b ask; the
    /// other field assertions guard against a refactor that
    /// quietly populated a transition timestamp at insert time
    /// (which would defeat the audit-view contract documented on
    /// [`Challenge::accepted_at`]).
    ///
    /// We also verify the row is visible by primary key directly,
    /// so a regression that returned a `Challenge` from `RETURNING`
    /// without actually persisting (e.g. a future change that
    /// wrapped the insert in a transaction and forgot to commit)
    /// flunks here rather than only in a 4c/4f follow-up test.
    /// Same shape as `send_notice_stores_unread_notice`.
    #[test]
    fn create_challenge_starts_in_open() {
        let (_dir, world) = world_with_challenges();
        let alice = world
            .upsert_player(&ctx("u-alice", "alice"))
            .expect("alice upsert succeeds");
        let bob = world
            .upsert_player(&ctx("u-bob", "bob"))
            .expect("bob upsert succeeds");

        let created = world
            .create_challenge(
                alice.id,
                bob.id,
                "clue_race",
                Some(r#"{"xp":100}"#),
                Some("2026-12-31T23:59:59Z"),
            )
            .expect("create_challenge succeeds");

        // The central Task 4b claim: starts in `open`.
        assert_eq!(
            created.state,
            ChallengeState::Open.as_str(),
            "freshly created challenge must be in state 'open' (SPEC §4.2)"
        );

        // Returned record reflects the inputs and SQL-side defaults.
        assert!(created.id > 0, "RETURNING must echo an autoincrement id");
        assert!(
            !created.created_at.is_empty(),
            "RETURNING must echo CURRENT_TIMESTAMP"
        );
        assert_eq!(created.challenger_player_id, alice.id);
        assert_eq!(created.target_player_id, bob.id);
        assert_eq!(created.kind, "clue_race");
        assert_eq!(created.stake.as_deref(), Some(r#"{"xp":100}"#));
        assert_eq!(created.expires_at.as_deref(), Some("2026-12-31T23:59:59Z"));

        // Transition timestamps and result MUST be NULL on a fresh
        // challenge — populating any of them at insert time would
        // break the lifecycle contract (see SPEC §4.2 transitions).
        assert!(
            created.accepted_at.is_none(),
            "freshly created challenge must not be accepted"
        );
        assert!(
            created.resolved_at.is_none(),
            "freshly created challenge must not be resolved"
        );
        assert!(
            created.result.is_none(),
            "freshly created challenge must have no result payload"
        );

        // Round-trip via primary-key SELECT to prove the row really
        // landed — guards against a future refactor that returns
        // the row from `RETURNING` without committing.
        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, challenger_player_id, target_player_id, \
                        kind, stake, state, accepted_at, resolved_at, expires_at, result \
                 FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                row_to_challenge,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, created, "stored row must equal RETURNING row");
    }

    /// SPEC §4.2 lists `kind` as required. The schema's `NOT NULL`
    /// would accept `""`; the kit refuses at the boundary so a
    /// "challenges of kind X" filter never has to skip phantom
    /// rows. Pin both the typed error and the "no row landed"
    /// invariant — same shape as `send_notice_rejects_empty_subject`.
    #[test]
    fn create_challenge_rejects_empty_kind() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let err = world
            .create_challenge(alice.id, bob.id, "", None, None)
            .expect_err("empty kind must be rejected");
        assert!(matches!(err, ChallengeError::EmptyKind), "got {err:?}");

        let count: i64 = world
            .connection()
            .query_row("SELECT COUNT(*) FROM challenges", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "rejected challenge must not produce a row");
    }

    /// `stake` and `expires_at` are both optional on the create
    /// path. Pin that `None` round-trips as `NULL` and the rest of
    /// the lifecycle still starts clean. Without this test, a
    /// future signature change that "helpfully" defaulted either
    /// field to a sentinel (e.g. the empty JSON object `{}` for
    /// stake) would silently break SPEC §4.2's "challenges may
    /// carry no stake" contract.
    #[test]
    fn create_challenge_supports_optional_stake_and_deadline() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();

        let created = world
            .create_challenge(alice.id, bob.id, "deduction_duel", None, None)
            .expect("create_challenge with no stake/deadline succeeds");

        assert_eq!(created.state, ChallengeState::Open.as_str());
        assert!(
            created.stake.is_none(),
            "None stake must round-trip as NULL"
        );
        assert!(
            created.expires_at.is_none(),
            "None expires_at must round-trip as NULL"
        );
    }

    /// SPEC_v3 §Task 4c acceptance: the canonical happy path. A
    /// freshly created `open` challenge with no deadline (or a
    /// future deadline) flips to `accepted` and gets `accepted_at`
    /// populated by SQL. The other transition timestamps stay
    /// `NULL` — accept is a one-step transition, not a "stamp
    /// everything" shortcut. Round-tripping through a primary-key
    /// SELECT also proves the new row state is durable, not just
    /// reflected in the `RETURNING` echo.
    #[test]
    fn accept_challenge_transitions_open_to_accepted() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .expect("create_challenge succeeds");

        let accepted = world
            .accept_challenge(created.id)
            .expect("accept_challenge succeeds on a fresh open challenge");

        assert_eq!(
            accepted.state,
            ChallengeState::Accepted.as_str(),
            "open challenge must transition to 'accepted'"
        );
        assert!(
            accepted.accepted_at.is_some(),
            "accept_challenge must stamp accepted_at"
        );
        assert!(
            accepted.resolved_at.is_none(),
            "accept_challenge must not stamp resolved_at"
        );
        assert!(
            accepted.result.is_none(),
            "accept_challenge must not populate result"
        );

        // Durability: re-read by primary key and confirm the row
        // matches the `RETURNING` echo. A regression that returned
        // a phantom row from `RETURNING` without committing would
        // flunk here.
        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, challenger_player_id, target_player_id, \
                        kind, stake, state, accepted_at, resolved_at, expires_at, result \
                 FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                row_to_challenge,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, accepted, "stored row must equal RETURNING row");
    }

    /// SPEC §4.2 says "Expired challenges cannot be accepted". An
    /// `open` row whose `expires_at` is already in the past must
    /// return [`ChallengeError::Expired`] *and* leave the row
    /// untouched — neither `state` nor `accepted_at` advance. We
    /// pin both halves: the typed error AND the row-unchanged
    /// invariant. Without the second half, a future regression
    /// that flipped the order of the `WHERE` checks could let a
    /// stale challenge slip through with a "fail" return value
    /// while still mutating the row.
    #[test]
    fn accept_challenge_rejects_expired() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        // A deadline well in the past so the `datetime()`
        // comparison in `accept_challenge` rejects regardless of
        // wall-clock skew.
        let created = world
            .create_challenge(
                alice.id,
                bob.id,
                "clue_race",
                None,
                Some("2000-01-01T00:00:00Z"),
            )
            .expect("create_challenge succeeds");

        let err = world
            .accept_challenge(created.id)
            .expect_err("accepting an expired challenge must fail");
        assert!(
            matches!(err, ChallengeError::Expired { id } if id == created.id),
            "expected Expired {{ id: {} }}, got {err:?}",
            created.id
        );

        // Row-unchanged invariant: state still 'open',
        // accepted_at still NULL.
        let (state, accepted_at): (String, Option<String>) = world
            .connection()
            .query_row(
                "SELECT state, accepted_at FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "open", "expired-but-unswept row must stay 'open'");
        assert!(
            accepted_at.is_none(),
            "rejected accept must leave accepted_at NULL"
        );
    }

    /// Accept on a row that is no longer `open` must fail with
    /// [`ChallengeError::InvalidTransition`] carrying the actual
    /// `from` state. Pin the canonical case — a second accept on
    /// an already-accepted row — because that's the regression
    /// most likely to slip in (a UI re-fires accept after a
    /// double-keypress). The other terminal states (`declined`,
    /// `resolved`, `expired`) ride the same code path; Task 4g's
    /// invalid-transition matrix will cover them exhaustively.
    #[test]
    fn accept_challenge_rejects_already_accepted() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();

        let first = world
            .accept_challenge(created.id)
            .expect("first accept succeeds");
        let second = world
            .accept_challenge(created.id)
            .expect_err("second accept must fail");
        assert!(
            matches!(
                &second,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "accepted" && *to == ChallengeState::Accepted
            ),
            "expected InvalidTransition from 'accepted' to Accepted, got {second:?}"
        );

        // accepted_at must NOT have moved between the two calls —
        // the second attempt is a no-op on the row, mirroring the
        // idempotency contract on `mark_read` (only the *typed
        // error* differs because the state machine is stricter
        // here than the read-flag flip).
        let post: Option<String> = world
            .connection()
            .query_row(
                "SELECT accepted_at FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            post, first.accepted_at,
            "second accept must not advance accepted_at"
        );
    }

    /// A stale id (challenge was deleted, or the caller fabricated
    /// one) must surface [`ChallengeError::NotFound`] rather than a
    /// generic SQL error. Same shape as `mark_read_missing_id_returns_not_found`.
    #[test]
    fn accept_challenge_missing_id_returns_not_found() {
        let (_dir, world) = world_with_challenges();
        let err = world
            .accept_challenge(424_242)
            .expect_err("missing id must fail");
        assert!(
            matches!(err, ChallengeError::NotFound { id } if id == 424_242),
            "expected NotFound, got {err:?}"
        );
    }

    /// SPEC_v3 §Task 4d acceptance: the canonical happy path. A
    /// freshly created `open` challenge transitions to `declined` on
    /// the first decline call. Declined is terminal and there is no
    /// `declined_at` column, so we additionally pin that
    /// `accepted_at`, `resolved_at`, and `result` all stay `NULL` —
    /// a regression that "helpfully" stamped one of those alongside
    /// the state flip would defeat the audit-view contract on
    /// [`Challenge::accepted_at`] / [`Challenge::resolved_at`] and
    /// quietly diverge from the schema's transition timestamps.
    /// Round-trip through a primary-key SELECT to also prove the
    /// `RETURNING` row is durable.
    #[test]
    fn decline_challenge_transitions_open_to_declined() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .expect("create_challenge succeeds");

        let declined = world
            .decline_challenge(created.id)
            .expect("decline_challenge succeeds on a fresh open challenge");

        assert_eq!(
            declined.state,
            ChallengeState::Declined.as_str(),
            "open challenge must transition to 'declined'"
        );
        assert!(
            declined.accepted_at.is_none(),
            "decline_challenge must not stamp accepted_at"
        );
        assert!(
            declined.resolved_at.is_none(),
            "decline_challenge must not stamp resolved_at"
        );
        assert!(
            declined.result.is_none(),
            "decline_challenge must not populate result"
        );

        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, challenger_player_id, target_player_id, \
                        kind, stake, state, accepted_at, resolved_at, expires_at, result \
                 FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                row_to_challenge,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, declined, "stored row must equal RETURNING row");
    }

    /// SPEC §4.2's deadline gate ("Expired challenges cannot be
    /// accepted") applies only to acceptance. Declining a stale open
    /// challenge is legitimate — the target sees the lapsed entry in
    /// their inbox before the sweeper runs and explicitly says "no
    /// thanks". Pin that contract so a future regression that copy-
    /// pasted the accept gate onto the decline path (and started
    /// returning `Expired` here) flunks immediately.
    #[test]
    fn decline_challenge_allows_lapsed_deadline() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(
                alice.id,
                bob.id,
                "clue_race",
                None,
                Some("2000-01-01T00:00:00Z"),
            )
            .expect("create_challenge succeeds");

        let declined = world
            .decline_challenge(created.id)
            .expect("decline_challenge must accept a lapsed-deadline open challenge");
        assert_eq!(declined.state, ChallengeState::Declined.as_str());
    }

    /// SPEC §4.2: only `open -> declined` is a legal decline. Any
    /// non-open current state must surface as
    /// [`ChallengeError::InvalidTransition`] carrying the actual
    /// `from`. Pin the canonical regression — declining an already-
    /// accepted challenge — and additionally pin the row-unchanged
    /// invariant so a future helper that flipped state then
    /// returned an error couldn't slip through. Other terminal
    /// states ride the same code path; Task 4g's invalid-transition
    /// matrix covers them exhaustively.
    #[test]
    fn decline_challenge_rejects_already_accepted() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();
        world.accept_challenge(created.id).expect("accept succeeds");

        let err = world
            .decline_challenge(created.id)
            .expect_err("declining an accepted challenge must fail");
        assert!(
            matches!(
                &err,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "accepted" && *to == ChallengeState::Declined
            ),
            "expected InvalidTransition from 'accepted' to Declined, got {err:?}"
        );

        // Row-unchanged invariant: state still 'accepted', no decline
        // sneaked in alongside the typed error.
        let state: String = world
            .connection()
            .query_row(
                "SELECT state FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "accepted", "rejected decline must not mutate state");
    }

    /// Decline on a re-decline must surface
    /// [`ChallengeError::InvalidTransition`] from `'declined'` rather
    /// than masquerading as a successful no-op. SPEC §4.2 is strict
    /// here: declined is terminal, and a UI that fires decline twice
    /// (a double-keypress, an at-least-once retry) needs the typed
    /// signal to render the right message. Same shape rationale as
    /// `accept_challenge_rejects_already_accepted`.
    #[test]
    fn decline_challenge_rejects_already_declined() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();
        world
            .decline_challenge(created.id)
            .expect("first decline succeeds");

        let err = world
            .decline_challenge(created.id)
            .expect_err("second decline must fail");
        assert!(
            matches!(
                &err,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "declined" && *to == ChallengeState::Declined
            ),
            "expected InvalidTransition from 'declined' to Declined, got {err:?}"
        );
    }

    /// A stale id (challenge was deleted, or the caller fabricated
    /// one) must surface [`ChallengeError::NotFound`] rather than a
    /// generic SQL error. Same shape as
    /// `accept_challenge_missing_id_returns_not_found`.
    #[test]
    fn decline_challenge_missing_id_returns_not_found() {
        let (_dir, world) = world_with_challenges();
        let err = world
            .decline_challenge(424_242)
            .expect_err("missing id must fail");
        assert!(
            matches!(err, ChallengeError::NotFound { id } if id == 424_242),
            "expected NotFound, got {err:?}"
        );
    }

    /// SPEC_v3 §Task 4e acceptance: the canonical happy path. An
    /// `accepted` challenge transitions to `resolved`, the caller-
    /// provided `result` JSON is stored verbatim, `resolved_at` is
    /// stamped by SQL, and `accepted_at` from the prior transition
    /// stays put. Round-trip via primary-key SELECT to prove the
    /// `RETURNING` row is durable. Same shape as
    /// `accept_challenge_transitions_open_to_accepted`.
    #[test]
    fn resolve_challenge_transitions_accepted_to_resolved() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .expect("create_challenge succeeds");
        let accepted = world
            .accept_challenge(created.id)
            .expect("accept_challenge succeeds");

        let payload = r#"{"winner":"alice","xp":100}"#;
        let resolved = world
            .resolve_challenge(created.id, payload)
            .expect("resolve_challenge succeeds on an accepted challenge");

        assert_eq!(
            resolved.state,
            ChallengeState::Resolved.as_str(),
            "accepted challenge must transition to 'resolved'"
        );
        assert!(
            resolved.resolved_at.is_some(),
            "resolve_challenge must stamp resolved_at"
        );
        assert_eq!(
            resolved.result.as_deref(),
            Some(payload),
            "resolve_challenge must store the result JSON verbatim"
        );
        assert_eq!(
            resolved.accepted_at, accepted.accepted_at,
            "resolve_challenge must preserve the prior accepted_at"
        );

        let stored = world
            .connection()
            .query_row(
                "SELECT id, created_at, challenger_player_id, target_player_id, \
                        kind, stake, state, accepted_at, resolved_at, expires_at, result \
                 FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                row_to_challenge,
            )
            .expect("primary-key SELECT decodes the row");
        assert_eq!(stored, resolved, "stored row must equal RETURNING row");
    }

    /// SPEC §4.2: only `accepted -> resolved` is legal. Resolving an
    /// `open` row (target hasn't accepted yet) must fail with
    /// [`ChallengeError::InvalidTransition`] from `'open'` and leave
    /// the row entirely untouched — no state flip, no `resolved_at`,
    /// no `result`. Pin the row-unchanged invariant so a regression
    /// that wrote partial state then returned the typed error
    /// couldn't slip through.
    #[test]
    fn resolve_challenge_rejects_open() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();

        let err = world
            .resolve_challenge(created.id, r#"{"winner":"alice"}"#)
            .expect_err("resolving an open challenge must fail");
        assert!(
            matches!(
                &err,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "open" && *to == ChallengeState::Resolved
            ),
            "expected InvalidTransition from 'open' to Resolved, got {err:?}"
        );

        // Row-unchanged invariant: state still 'open', resolved_at
        // and result still NULL.
        let (state, resolved_at, result): (String, Option<String>, Option<String>) = world
            .connection()
            .query_row(
                "SELECT state, resolved_at, result FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "open", "rejected resolve must not mutate state");
        assert!(
            resolved_at.is_none(),
            "rejected resolve must leave resolved_at NULL"
        );
        assert!(result.is_none(), "rejected resolve must leave result NULL");
    }

    /// Re-resolving an already-resolved challenge must surface
    /// [`ChallengeError::InvalidTransition`] from `'resolved'` rather
    /// than overwriting the original result payload. SPEC §4.2 makes
    /// `resolved` terminal; a UI that fires resolve twice (a double-
    /// keypress, an at-least-once retry, two referees racing) needs
    /// the typed signal to render the right message AND the original
    /// payload must survive — overwriting it would corrupt the audit
    /// trail.
    #[test]
    fn resolve_challenge_rejects_already_resolved() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();
        world.accept_challenge(created.id).unwrap();
        let first = world
            .resolve_challenge(created.id, r#"{"winner":"alice"}"#)
            .expect("first resolve succeeds");

        let err = world
            .resolve_challenge(created.id, r#"{"winner":"bob"}"#)
            .expect_err("second resolve must fail");
        assert!(
            matches!(
                &err,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "resolved" && *to == ChallengeState::Resolved
            ),
            "expected InvalidTransition from 'resolved' to Resolved, got {err:?}"
        );

        // Original payload must survive — second resolve does not
        // overwrite the audit trail.
        let post: Option<String> = world
            .connection()
            .query_row(
                "SELECT result FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            post, first.result,
            "second resolve must not overwrite the stored result"
        );
    }

    /// Resolving a `declined` challenge must surface
    /// [`ChallengeError::InvalidTransition`] from `'declined'`. SPEC
    /// §4.2 makes `declined` terminal, and there is no path back to
    /// `accepted` — covering this case here (alongside the open and
    /// already-resolved cases) gives Task 4e three of the four
    /// non-accepted starting states; the fourth (`expired`) lands
    /// with Task 4f's sweeper. Task 4g's invalid-transition matrix
    /// covers the full grid.
    #[test]
    fn resolve_challenge_rejects_declined() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();
        world.decline_challenge(created.id).unwrap();

        let err = world
            .resolve_challenge(created.id, r#"{"winner":"alice"}"#)
            .expect_err("resolving a declined challenge must fail");
        assert!(
            matches!(
                &err,
                ChallengeError::InvalidTransition { id, from, to }
                    if *id == created.id && from == "declined" && *to == ChallengeState::Resolved
            ),
            "expected InvalidTransition from 'declined' to Resolved, got {err:?}"
        );
    }

    /// An empty `result` must surface [`ChallengeError::EmptyResult`]
    /// at the boundary, before any SQL runs. Pin both the typed
    /// error and the row-unchanged invariant — a regression that
    /// "helpfully" defaulted `""` to `"{}"` would silently land
    /// payloads game code never produced. The accepted challenge
    /// must remain in `accepted` so the next (well-formed) resolve
    /// call still succeeds.
    #[test]
    fn resolve_challenge_rejects_empty_result() {
        let (_dir, world) = world_with_challenges();
        let alice = world.upsert_player(&ctx("u-a", "alice")).unwrap();
        let bob = world.upsert_player(&ctx("u-b", "bob")).unwrap();
        let created = world
            .create_challenge(alice.id, bob.id, "clue_race", None, None)
            .unwrap();
        world.accept_challenge(created.id).unwrap();

        let err = world
            .resolve_challenge(created.id, "")
            .expect_err("empty result must be rejected");
        assert!(matches!(err, ChallengeError::EmptyResult), "got {err:?}");

        // Row-unchanged invariant: state still 'accepted',
        // resolved_at still NULL, result still NULL.
        let (state, resolved_at, result): (String, Option<String>, Option<String>) = world
            .connection()
            .query_row(
                "SELECT state, resolved_at, result FROM challenges WHERE id = ?1",
                rusqlite::params![created.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "accepted");
        assert!(resolved_at.is_none());
        assert!(result.is_none());

        // The next well-formed resolve must still succeed — the
        // rejected attempt did not poison the row.
        let resolved = world
            .resolve_challenge(created.id, r#"{"ok":true}"#)
            .expect("subsequent well-formed resolve succeeds");
        assert_eq!(resolved.state, ChallengeState::Resolved.as_str());
    }

    /// A stale id (challenge was deleted, or the caller fabricated
    /// one) must surface [`ChallengeError::NotFound`] rather than a
    /// generic SQL error. Same shape as
    /// `accept_challenge_missing_id_returns_not_found`.
    #[test]
    fn resolve_challenge_missing_id_returns_not_found() {
        let (_dir, world) = world_with_challenges();
        let err = world
            .resolve_challenge(424_242, r#"{"ok":true}"#)
            .expect_err("missing id must fail");
        assert!(
            matches!(err, ChallengeError::NotFound { id } if id == 424_242),
            "expected NotFound, got {err:?}"
        );
    }

    /// [`ChallengeState::as_str`] is the single source-of-truth
    /// mapping the enum to the storage encoding pinned by the
    /// schema-level `CHECK` constraint. A regression that ever
    /// returned the wrong text for any variant would silently break
    /// the create / transition paths (state would not match the
    /// `CHECK` vocabulary). Pin every variant here.
    #[test]
    fn challenge_state_as_str_matches_schema_vocabulary() {
        assert_eq!(ChallengeState::Open.as_str(), "open");
        assert_eq!(ChallengeState::Accepted.as_str(), "accepted");
        assert_eq!(ChallengeState::Declined.as_str(), "declined");
        assert_eq!(ChallengeState::Resolved.as_str(), "resolved");
        assert_eq!(ChallengeState::Expired.as_str(), "expired");
    }
}
