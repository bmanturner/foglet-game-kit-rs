//! Murder Motel shared-world schema (SPEC_v2 §Task 12).
//!
//! v2 introduces a single SQLite database the kit owns and games extend
//! with their own migrations on top of the kit's reserved versions
//! (1–99 today: `world_migrations`, `players`, `turn_ledger`,
//! `world_events`, `leaderboard_scores`). This module hosts Murder
//! Motel's first game-authored migration: a tiny key/value table the
//! later Task 12 sub-iterations populate to remember world state that
//! is shared across players (notably "who unlocked Room 7 first, and
//! when").
//!
//! ## Why a key/value table instead of a typed schema
//!
//! Murder Motel is the kit's acceptance fixture, not a real production
//! game. The shared-world surface it needs to demonstrate is small —
//! one or two facts about Room 7 — and locking each fact into its own
//! typed column would force a follow-up migration every time we add
//! another shared flag during the Task 12/13 build-out. A
//! `(key TEXT PRIMARY KEY, value TEXT NOT NULL)` shape lets the
//! example evolve under one stable migration while still giving an
//! operator a legible `sqlite3 motel_world_state` story.
//!
//! Real games are encouraged to declare typed tables (see
//! `LEADERBOARD_SCORES_MIGRATION` for the pattern); the kit imposes no
//! key/value convention.
//!
//! ## Version band
//!
//! Game-authored migrations start at [`MOTEL_MIGRATION_VERSION_BASE`]
//! to leave plenty of room above the kit's reserved range. The base is
//! a deliberate jump (not "kit_max + 1") so that adding a kit
//! migration tomorrow doesn't push game versions around — the kit and
//! the game evolve in independent number bands.

use foglet_game::{
    DateProvider, FeedbackLine, FogletContext, GameConfig, TurnError, WorldDb, WorldMigration,
};
use rusqlite::OptionalExtension;

/// First version number a Murder Motel migration may use. Picked far
/// above the kit's reserved 1–5 range so kit growth never collides
/// with game-authored versions.
pub const MOTEL_MIGRATION_VERSION_BASE: i64 = 100;

/// Murder Motel's shared key/value world state table.
///
/// Rows are owned by the game; the kit never reads or writes them.
/// Sub-iterations 12b–12d populate `room_7_opened_at` and
/// `room_7_opened_by` here so any later player can see the room was
/// already unlocked. Future shared facts (e.g. "lobby clue X seen by
/// at least one investigator") slot in as additional rows under the
/// same migration.
///
/// The table is keyed by an opaque string so an operator running
/// `sqlite3 world/world.sqlite "SELECT * FROM motel_world_state"`
/// gets a self-describing dump without needing the source.
pub const MOTEL_WORLD_STATE_MIGRATION: WorldMigration = WorldMigration {
    version: MOTEL_MIGRATION_VERSION_BASE,
    name: "create_motel_world_state",
    sql: "\
CREATE TABLE IF NOT EXISTS motel_world_state (\n\
    key   TEXT PRIMARY KEY,\n\
    value TEXT NOT NULL\n\
);\n\
",
};

/// Key under which `motel_world_state` stores the SQLite-side
/// `CURRENT_TIMESTAMP` of the moment the *first* player stepped onto
/// the Room 7 stairs with the key in hand. Pulled out as a constant
/// so tests, the bulletin (Task 13d), and the "someone else opened
/// it" surface (Task 12c) all reference the same string.
pub const ROOM_7_OPENED_AT_KEY: &str = "room_7_opened_at";

/// Key under which `motel_world_state` stores the `players.id` of the
/// first player to open Room 7. Stored as a TEXT column (the table is
/// schema-light key/value) and parsed back to `i64` in the helpers
/// below.
pub const ROOM_7_OPENED_BY_KEY: &str = "room_7_opened_by";

/// Snapshot of the canonical "Room 7 was opened" facts as stored in
/// `motel_world_state`. The two fields together identify *who* opened
/// the room and *when*; `first_opening` distinguishes "this call is
/// what wrote the row" from "the row already existed when we looked".
///
/// The struct is the return shape of both
/// [`record_room_7_opening`] (write-then-read-back) and
/// [`room_7_opening`] (read-only) so callers handle one type
/// regardless of whether they're observing or recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room7Opening {
    /// SQLite `CURRENT_TIMESTAMP` text recorded on the first opening.
    /// Format is the SQLite default `'YYYY-MM-DD HH:MM:SS'` UTC text
    /// — the same shape the kit's `players.first_seen_at` column uses,
    /// so renderers can reuse one formatter for both surfaces.
    pub opened_at: String,
    /// `players.id` of the first opener. Stored as TEXT in the
    /// key/value table (Task 12a kept the schema deliberately
    /// loose) and parsed back to a typed `i64` here so callers don't
    /// have to repeat the parse at every read site.
    pub opened_by_player_id: i64,
    /// `true` only when *this* call inserted the row. Lets the caller
    /// distinguish "first opener — emit the bulletin event" from
    /// "later opener — show the 'someone got here first' surface"
    /// without a follow-up read. Tasks 12c/13c key off this bit.
    pub first_opening: bool,
}

/// Record that `player_id` just unlocked Room 7, but only if no
/// earlier player got there first. Implements SPEC_v2 §Task 12b's
/// "first-writer-wins" contract.
///
/// Mechanism: two `INSERT OR IGNORE` statements against
/// [`MOTEL_WORLD_STATE_MIGRATION`]'s key/value table — one for
/// [`ROOM_7_OPENED_AT_KEY`] (`CURRENT_TIMESTAMP`), one for
/// [`ROOM_7_OPENED_BY_KEY`] (the player id). `INSERT OR IGNORE` makes
/// the second-and-later callers no-ops at the SQL level, which is
/// exactly the shared-world semantics Murder Motel needs: every
/// player who steps onto the stairs runs this code, but only the
/// first one's identity and timestamp are remembered.
///
/// Both keys are written through [`WorldDb::connection`] (a `&self`
/// shared borrow) rather than [`WorldDb::transaction`] (`&mut self`).
/// The shared borrow matches the runtime's `Option<&WorldDb>` handed
/// to screens during `handle_input`, so the call site in
/// [`crate::map::MapScreen`] can run this on the same tick that emits
/// the screen transition. Strict atomicity isn't required because
/// each `INSERT OR IGNORE` is itself atomic and the write is
/// permanently first-writer-wins: if the process is killed between
/// the two statements, the next caller's `INSERT OR IGNORE` finishes
/// the pair using its own (later) player id, which is acceptable —
/// Task 12b's promise is "the first observed opening is recorded",
/// not "the two keys are written in a single SQL transaction".
///
/// After both inserts the function reads both keys back so callers
/// always see the canonical pair, regardless of whether this call
/// wrote them. `first_opening` reflects whether the *opened_at*
/// insert affected a row in this call (mirrors SQLite's
/// `Connection::execute` rows-affected return).
///
/// # Errors
///
/// Returns `rusqlite::Error` from any of the four statements. Callers
/// (the map screen) treat a DB error as a non-fatal logging concern —
/// the player still transitions to Room 7; the bulletin just misses
/// a record. The kit's terminal-safety contract forbids panicking out
/// of `handle_input`, so the screen layer log-and-continues.
pub fn record_room_7_opening(
    world: &WorldDb,
    player_id: i64,
) -> Result<Room7Opening, rusqlite::Error> {
    let conn = world.connection();
    // Statement 1: stamp the opening time. `INSERT OR IGNORE` makes
    // the no-op path explicit; the rows-affected return tells us
    // whether *this* call won the race.
    let opened_at_inserted = conn.execute(
        "INSERT OR IGNORE INTO motel_world_state (key, value) \
         VALUES (?1, CURRENT_TIMESTAMP)",
        rusqlite::params![ROOM_7_OPENED_AT_KEY],
    )?;
    // Statement 2: stamp the opener id. Stored as TEXT — the
    // key/value table is intentionally schema-light (see Task 12a).
    // We bind the i64 directly; rusqlite will format it as the
    // canonical decimal text representation.
    conn.execute(
        "INSERT OR IGNORE INTO motel_world_state (key, value) \
         VALUES (?1, ?2)",
        rusqlite::params![ROOM_7_OPENED_BY_KEY, player_id],
    )?;
    // Read back the canonical pair. We don't trust our own writes to
    // be the visible state — a concurrent process may have raced us
    // — so the returned struct always reflects the row that landed.
    let opened_at: String = conn.query_row(
        "SELECT value FROM motel_world_state WHERE key = ?1",
        rusqlite::params![ROOM_7_OPENED_AT_KEY],
        |row| row.get(0),
    )?;
    let opened_by_text: String = conn.query_row(
        "SELECT value FROM motel_world_state WHERE key = ?1",
        rusqlite::params![ROOM_7_OPENED_BY_KEY],
        |row| row.get(0),
    )?;
    let opened_by_player_id: i64 = opened_by_text.parse().map_err(|_err| {
        // Surface a parse failure as a SQL-layer error so the caller's
        // `Result<_, rusqlite::Error>` keeps a single error variant.
        // Reaching this branch implies the key/value column was
        // hand-edited to a non-integer — a schema violation worth
        // surfacing rather than silently coercing.
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "{ROOM_7_OPENED_BY_KEY} stored non-integer value: {opened_by_text:?}"
            )),
        )
    })?;
    Ok(Room7Opening {
        opened_at,
        opened_by_player_id,
        first_opening: opened_at_inserted == 1,
    })
}

/// Read-only view of the Room 7 opening record, returning `None` when
/// no player has unlocked it yet. Used by the lobby UI surfaces in
/// Task 12c/13d to decide whether to render the "someone already got
/// here" affordance.
///
/// Returns `None` when *either* key is missing — covers the degenerate
/// "process killed mid-write" case from
/// [`record_room_7_opening`]'s docs by treating a half-written pair
/// as "not yet opened" rather than fabricating one half from the
/// other.
pub fn room_7_opening(world: &WorldDb) -> Result<Option<Room7Opening>, rusqlite::Error> {
    let conn = world.connection();
    let opened_at: Option<String> = conn
        .query_row(
            "SELECT value FROM motel_world_state WHERE key = ?1",
            rusqlite::params![ROOM_7_OPENED_AT_KEY],
            |row| row.get(0),
        )
        .optional()?;
    let opened_by_text: Option<String> = conn
        .query_row(
            "SELECT value FROM motel_world_state WHERE key = ?1",
            rusqlite::params![ROOM_7_OPENED_BY_KEY],
            |row| row.get(0),
        )
        .optional()?;
    match (opened_at, opened_by_text) {
        (Some(opened_at), Some(opened_by_text)) => {
            let opened_by_player_id: i64 = opened_by_text.parse().map_err(|_err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::<dyn std::error::Error + Send + Sync>::from(format!(
                        "{ROOM_7_OPENED_BY_KEY} stored non-integer value: {opened_by_text:?}"
                    )),
                )
            })?;
            Ok(Some(Room7Opening {
                opened_at,
                opened_by_player_id,
                first_opening: false,
            }))
        }
        // Half-written pair (only one of the two keys present) is
        // intentionally treated as "no opening recorded yet". See
        // `record_room_7_opening` for why this state is reachable.
        _ => Ok(None),
    }
}

/// Build the arrival-time feedback line for a player who just stepped
/// into Room 7. Implements SPEC_v2 §Task 12c's "show later players that
/// Room 7 was already opened by someone else" surface.
///
/// Decision matrix:
///
/// - `opening.opened_by_player_id == current_player_id` → returns
///   `None`. The current player either *just* opened the room
///   (`first_opening` true) or returned to one they previously opened
///   themselves; in both cases the lobby's stale feedback was already
///   cleared, so leaving the slot empty keeps Room 7 quiet on arrival
///   and lets the body's flavour line (the only narration Room 7
///   itself emits) own the slot.
/// - Otherwise → returns `Some(info(...))` with a single-sentence
///   narration that calls out the prior opening *without* naming the
///   other investigator. We don't have a player-handle lookup keyed
///   by `players.id` yet (Task 12 hasn't shipped one), and the kit's
///   shared-world contract is "first-writer-wins, identities are
///   advisory" — surfacing the timestamp tells later players "you're
///   not the first" without leaking another user's handle through a
///   side channel.
///
/// Returned as an `Option` rather than always-`Some` so the caller can
/// distinguish "no banner to show" from "show this banner" with a
/// single match — the call site in [`crate::map::MapScreen`] writes
/// the result straight into [`crate::state::SharedSlots::feedback`].
pub fn shared_room_7_arrival_feedback(
    opening: &Room7Opening,
    current_player_id: i64,
) -> Option<FeedbackLine> {
    if opening.opened_by_player_id == current_player_id {
        return None;
    }
    Some(FeedbackLine::info(format!(
        "Another investigator already unlocked Room 7 (first opened {}).",
        opening.opened_at
    )))
}

/// Number of daily turns one clue-inspection action consumes — SPEC_v2
/// §7 ("Daily clue turns: examining clue hotspots spends turns") and
/// §Task 13a. Centralised so the helper, the lobby's X-press handler,
/// and the tests all agree on the cost.
pub const CLUE_INSPECTION_TURN_COST: u32 = 1;

/// Player-facing line surfaced when a clue-inspection press is rejected
/// because today's balance is exhausted. Phrased to make the recovery
/// path ("come back tomorrow") obvious without naming a specific
/// timezone — SPEC_v2 §4.6's reset is configured per-game and the
/// example's [`crate::clock::SystemDateProvider`] runs in UTC, so a
/// player-friendly summary is the safest copy.
pub const NO_CLUE_TURNS_FEEDBACK: &str = "You're out of clue turns for today — come back tomorrow.";

/// Outcome of a clue-inspection turn-spend attempt (SPEC_v2 §Task 13a).
///
/// Modelled as an enum rather than `Result<i64, _>` because three of
/// the four arms are non-error "this is what happened, here's the new
/// state" outcomes the caller routes on directly:
///
/// - [`Self::Spent`] — turn deducted; carry on with the inspection.
/// - [`Self::InsufficientTurns`] — balance was zero; surface the
///   `NO_CLUE_TURNS_FEEDBACK` line and **do not** open the prompt.
/// - [`Self::NotConfigured`] — `[turns]` is absent from `game.toml`,
///   meaning the author opted out of the daily allowance system.
///   Caller treats this exactly like a single-player run: open the
///   prompt unconditionally.
/// - [`Self::Failed`] — DB or upsert error. Logged-and-swallowed at
///   the call site (the kit's terminal-safety contract forbids
///   panicking out of `handle_input`); caller still opens the prompt
///   so a transient SQLite hiccup doesn't soft-lock the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClueInspectionOutcome {
    /// Turn was deducted. `remaining` is the post-spend balance — the
    /// status UI in Task 13b will paint this to the screen.
    Spent {
        /// Player's remaining clue-turn balance after the spend.
        remaining: i64,
    },
    /// Today's balance is below [`CLUE_INSPECTION_TURN_COST`].
    /// `balance` is the canonical pre-spend value (see SPEC_v2 §Task
    /// 6e on `TurnError::InsufficientTurns`'s no-mutation guarantee).
    InsufficientTurns {
        /// Current (unchanged) balance at the moment the spend was
        /// rejected.
        balance: i64,
    },
    /// The game.toml has no `[turns]` section, so spending is a no-op.
    /// Authors who never opt into the turn ledger get the full pre-v2
    /// behaviour: every clue inspection is free.
    NotConfigured,
    /// SQLite or upsert error during the spend. The boxed string is a
    /// formatted message suitable for tracing/logging; the caller
    /// should not surface it to the player.
    Failed(String),
}

/// Spend one clue-inspection turn for the player identified by `foglet`
/// against the shared `world` database (SPEC_v2 §Task 13a).
///
/// This is the single chokepoint Murder Motel uses for "examining a
/// clue hotspot". Right now the only consumer is the lobby's
/// Lost-and-Found Drawer search affordance ([`crate::map::MapScreen`]'s
/// `'x'`/`'X'` handler); future Task 13 sub-iterations (Room 7 body,
/// matchbook pickup) route through the same helper so the per-action
/// cost stays consistent and the status UI in Task 13b has one source
/// of truth.
///
/// ## Why a helper rather than inline code in the screen
///
/// 1. The spend is a sequence of three SQL writes (player upsert →
///    today's ledger row ensure → atomic decrement). Inlining all
///    three in `handle_input` would push the screen layer into
///    SQLite-shaped territory.
/// 2. Tests can drive the helper directly with a [`FixedDateProvider`]
///    (SPEC_v2 §Task 6f's deterministic-reset story) without
///    constructing a `MapScreen`.
/// 3. Failure routing (insufficient vs not-configured vs SQLite
///    error) is enumerated in [`ClueInspectionOutcome`] so the screen
///    handler stays a small `match`.
///
/// ## Concurrency
///
/// Three statements, executed serially through the same `&WorldDb`
/// shared borrow that [`crate::map::MapScreen::handle_input`] already
/// holds. The kit's busy timeout (configured at open time) is the
/// only contention story we need; SPEC_v2 §Task 6e's atomic-decrement
/// guard inside [`WorldDb::spend_turns`] keeps two racing spenders
/// from both satisfying `balance >= 1` against the same starting
/// balance.
///
/// ## Errors are non-fatal
///
/// On a SQLite or upsert failure the function returns
/// [`ClueInspectionOutcome::Failed`] with a formatted message rather
/// than propagating an `Err`. The caller (the screen handler) opens
/// the prompt unconditionally on this branch — a transient DB hiccup
/// must not soft-lock the player out of clue inspection. The kit's
/// `tracing` boundary will pick the message up once it's wired into
/// `foglet_game` (see SPEC §Task 12b's note).
pub fn spend_clue_inspection_turn(
    world: &WorldDb,
    foglet: &FogletContext,
    cfg: &GameConfig,
    date: &dyn DateProvider,
) -> ClueInspectionOutcome {
    // Fast-path: when the author hasn't opted into the turn ledger,
    // every inspection is free. Returning early before the upsert
    // keeps a `[turns]`-free game.toml from spinning the players
    // table for read-only inspections.
    let Some(turns) = cfg.turns.as_ref() else {
        return ClueInspectionOutcome::NotConfigured;
    };

    // Player upsert is the single SQL call that produces the
    // `players.id` the ledger row is keyed by. Failure here is rare
    // (the row already exists by the time the player reaches the
    // drawer in normal play) and we route it through the
    // logged-and-swallowed branch so the inspection still lands.
    let player = match world.upsert_player(foglet) {
        Ok(p) => p,
        Err(err) => return ClueInspectionOutcome::Failed(format!("upsert_player failed: {err}")),
    };

    match world.spend_turns(
        player.id,
        CLUE_INSPECTION_TURN_COST,
        turns.daily_allowance,
        turns.carryover_max,
        date,
    ) {
        Ok(row) => ClueInspectionOutcome::Spent {
            remaining: row.balance,
        },
        Err(TurnError::InsufficientTurns { balance, .. }) => {
            ClueInspectionOutcome::InsufficientTurns { balance }
        }
        Err(other) => ClueInspectionOutcome::Failed(format!("spend_turns failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    //! Migration smoke tests — apply the migration into a temp DB and
    //! assert the table is reachable through the same `WorldDb` API
    //! Task 12b will use to populate it.

    use super::*;
    use foglet_game::WorldDb;
    use tempfile::tempdir;

    /// Apply the migration once and confirm the resulting table is
    /// reachable for both reads and writes through the public
    /// [`WorldDb::transaction`] entry point. We deliberately go
    /// through `transaction` rather than reach for a crate-private
    /// connection accessor because that's the same surface
    /// sub-iterations 12b–12d will use to populate the table.
    #[test]
    fn migration_creates_motel_world_state_table() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");

        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");

        // Round-trip a representative key/value pair. We use a
        // transaction (committed) instead of pragma introspection so a
        // future schema rename surfaces here as a clear write failure
        // rather than a silent column-name drift.
        let stored: String = world
            .transaction(|tx| {
                tx.execute(
                    "INSERT INTO motel_world_state (key, value) VALUES (?1, ?2)",
                    ["room_7_opened_at", "2026-05-09T00:00:00Z"],
                )
                .expect("insert sentinel row");
                let value: String = tx
                    .query_row(
                        "SELECT value FROM motel_world_state WHERE key = ?1",
                        ["room_7_opened_at"],
                        |row| row.get(0),
                    )
                    .expect("read sentinel row");
                Ok(value)
            })
            .expect("transaction round-trip");
        assert_eq!(stored, "2026-05-09T00:00:00Z");
    }

    /// Re-applying the migration is a no-op (idempotent), matching the
    /// kit's `apply_migration` contract. Without this guarantee the
    /// relaunch path would fail every second open.
    #[test]
    fn migration_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");

        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("first apply");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("second apply must be a no-op");

        // Confirm exactly one row landed in the kit's
        // `world_migrations` bookkeeping table for this version.
        let recorded: i64 = world
            .transaction(|tx| {
                let n: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM world_migrations WHERE version = ?1",
                        [MOTEL_MIGRATION_VERSION_BASE],
                        |row| row.get(0),
                    )
                    .expect("count migration rows");
                Ok(n)
            })
            .expect("transaction count");
        assert_eq!(
            recorded, 1,
            "idempotent re-apply must not record a duplicate world_migrations row"
        );
    }

    /// Version sits above the kit's reserved band so a future kit
    /// migration can claim the next sequential number without
    /// colliding with Murder Motel's schema.
    #[test]
    fn version_lives_above_kit_reserved_band() {
        // Kit currently reserves versions 1–5 (see leaderboards.rs).
        // The base sits well above that to leave runway for kit growth.
        assert!(
            MOTEL_MIGRATION_VERSION_BASE >= 100,
            "game-authored migrations must start at or above the documented base"
        );
        assert_eq!(
            MOTEL_WORLD_STATE_MIGRATION.version, MOTEL_MIGRATION_VERSION_BASE,
            "first Murder Motel migration must occupy the base slot"
        );
    }

    /// Set up a `WorldDb` with the motel migration applied. Used by
    /// the Room 7 opening tests below to keep the boilerplate out of
    /// each test body.
    fn world_with_motel_state(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        world
    }

    /// First call must record both keys and report `first_opening`.
    /// SPEC_v2 §Task 12b: the moment a player unlocks Room 7, the
    /// timestamp and opener id land in `motel_world_state`.
    #[test]
    fn record_room_7_opening_writes_both_keys_on_first_call() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_motel_state(&dir);

        let outcome = record_room_7_opening(&world, 42).expect("first opening succeeds");
        assert!(
            outcome.first_opening,
            "first call must report it wrote the row"
        );
        assert_eq!(outcome.opened_by_player_id, 42);
        assert!(
            !outcome.opened_at.is_empty(),
            "opened_at must be populated by CURRENT_TIMESTAMP"
        );
    }

    /// First-writer-wins: a second player calling
    /// [`record_room_7_opening`] must NOT overwrite the original
    /// opener id or timestamp. The shared-world fixture's whole point
    /// is that "who opened Room 7" is one canonical answer across
    /// all players.
    #[test]
    fn record_room_7_opening_is_first_writer_wins() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_motel_state(&dir);

        let alice = record_room_7_opening(&world, 1).expect("alice records first");
        assert!(alice.first_opening);

        let bob = record_room_7_opening(&world, 2).expect("bob's call must succeed");
        assert!(
            !bob.first_opening,
            "subsequent caller must report the row already existed"
        );
        assert_eq!(
            bob.opened_by_player_id, 1,
            "second caller must observe the original opener id, not their own"
        );
        assert_eq!(
            bob.opened_at, alice.opened_at,
            "second caller must observe the original timestamp, not a new one"
        );
    }

    /// [`room_7_opening`] returns `None` until the first opener writes
    /// the pair. This is the read path Tasks 12c/13d will call from the
    /// lobby UI to decide whether to render the "someone got here"
    /// affordance.
    #[test]
    fn room_7_opening_returns_none_before_first_open() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_motel_state(&dir);

        let observed = room_7_opening(&world).expect("read succeeds even with no row");
        assert!(observed.is_none(), "no opener yet => None");
    }

    /// After [`record_room_7_opening`] runs, [`room_7_opening`]
    /// returns the same canonical pair (with `first_opening: false`,
    /// since the read path is observational only).
    #[test]
    fn room_7_opening_returns_recorded_pair_after_first_open() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_motel_state(&dir);

        let written = record_room_7_opening(&world, 7).expect("record");
        let observed = room_7_opening(&world)
            .expect("read succeeds")
            .expect("row exists after record");
        assert_eq!(observed.opened_by_player_id, 7);
        assert_eq!(observed.opened_at, written.opened_at);
        assert!(
            !observed.first_opening,
            "read path always reports first_opening=false; only the writer learns 'I won the race'"
        );
    }

    /// Task 12c: a player whose id matches the opener gets no arrival
    /// banner. Covers both the "I am the first opener" and the "I came
    /// back to a room I unlocked previously" flavours, since the
    /// helper's branching only looks at id equality.
    #[test]
    fn shared_room_7_feedback_is_none_when_current_player_is_opener() {
        let opening = Room7Opening {
            opened_at: "2026-05-09 12:34:56".to_string(),
            opened_by_player_id: 42,
            first_opening: true,
        };
        assert!(
            shared_room_7_arrival_feedback(&opening, 42).is_none(),
            "opener viewing their own row must get no arrival banner"
        );
    }

    /// Task 12c: a *different* player gets a single-line info banner
    /// that includes the original opening timestamp. The handle of the
    /// original opener is intentionally not surfaced (the kit has no
    /// id→handle lookup and identities are advisory per SPEC §4.5).
    #[test]
    fn shared_room_7_feedback_is_set_when_someone_else_opened_first() {
        let opening = Room7Opening {
            opened_at: "2026-05-09 12:34:56".to_string(),
            opened_by_player_id: 1,
            first_opening: false,
        };
        let line = shared_room_7_arrival_feedback(&opening, 2)
            .expect("later player must see the arrival banner");
        let text = line.rendered_text();
        assert!(
            text.contains("Another investigator"),
            "banner must call out the prior opening explicitly: {text}"
        );
        assert!(
            text.contains("2026-05-09 12:34:56"),
            "banner must include the original opening timestamp: {text}"
        );
    }

    // ---- SPEC_v2 §Task 13a clue-inspection turn spend ---------------

    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{FixedDateProvider, GameConfig, LocalDate, PLAYERS_MIGRATION};

    /// Stand up a world DB with the migrations a real Murder Motel
    /// install would have applied by the time the lobby's X-press
    /// fires: kit `players` and `turn_ledger`, plus the example's
    /// `motel_world_state`. Pulled out so each Task 13a test reads
    /// "set up world, drive helper, assert" instead of repeating
    /// six lines of boilerplate.
    fn world_with_turn_stack(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&foglet_game::TURN_LEDGER_MIGRATION)
            .expect("apply turn_ledger migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        world
    }

    fn fixed_today() -> FixedDateProvider {
        FixedDateProvider::new(LocalDate::parse("2026-05-09").expect("valid date"))
    }

    /// First call against a fresh ledger consumes one turn from the
    /// configured `daily_allowance` (3 in the scaffold's `game.toml`)
    /// and returns the post-spend balance.
    #[test]
    fn spend_clue_inspection_first_call_decrements_balance() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

        let outcome = spend_clue_inspection_turn(&world, &fc, &cfg, &date);
        assert_eq!(
            outcome,
            ClueInspectionOutcome::Spent { remaining: 2 },
            "scaffold game.toml has daily_allowance=3, so spending 1 leaves 2"
        );
    }

    /// Repeat calls walk the balance down to zero, matching the
    /// "examining a clue hotspot costs one turn" SPEC §7 contract.
    #[test]
    fn spend_clue_inspection_walks_balance_to_zero() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

        let mut observed = Vec::new();
        for _ in 0..3 {
            observed.push(spend_clue_inspection_turn(&world, &fc, &cfg, &date));
        }
        assert_eq!(
            observed,
            vec![
                ClueInspectionOutcome::Spent { remaining: 2 },
                ClueInspectionOutcome::Spent { remaining: 1 },
                ClueInspectionOutcome::Spent { remaining: 0 },
            ],
            "three back-to-back inspections must consume the day's allowance"
        );
    }

    /// Once the day's allowance is exhausted, the helper rejects with
    /// `InsufficientTurns` and reports the unchanged balance. SPEC_v2
    /// §Task 6e's no-mutation guarantee on `TurnError::InsufficientTurns`
    /// means the row is *not* decremented past zero.
    #[test]
    fn spend_clue_inspection_rejects_when_balance_exhausted() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

        // Drain the allowance.
        for _ in 0..3 {
            assert!(matches!(
                spend_clue_inspection_turn(&world, &fc, &cfg, &date),
                ClueInspectionOutcome::Spent { .. }
            ));
        }

        let outcome = spend_clue_inspection_turn(&world, &fc, &cfg, &date);
        assert_eq!(
            outcome,
            ClueInspectionOutcome::InsufficientTurns { balance: 0 },
            "fourth attempt must reject with the unchanged zero balance"
        );

        // Re-spending again still fails — the rejected attempt didn't
        // mutate the row, so the balance stays at zero rather than
        // sliding negative.
        let outcome_again = spend_clue_inspection_turn(&world, &fc, &cfg, &date);
        assert_eq!(
            outcome_again,
            ClueInspectionOutcome::InsufficientTurns { balance: 0 },
            "repeat rejected attempts must observe the same zero balance"
        );
    }

    /// A `game.toml` without a `[turns]` section opts out of the daily
    /// ledger entirely. The helper short-circuits to `NotConfigured` so
    /// pre-v2 games keep working — every clue inspection stays free.
    #[test]
    fn spend_clue_inspection_returns_not_configured_when_turns_missing() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let mut cfg: GameConfig = fixture_config();
        // Strip the turns section so we exercise the fast-path.
        cfg.turns = None;
        let fc = fixture_context();
        let date = fixed_today();

        let outcome = spend_clue_inspection_turn(&world, &fc, &cfg, &date);
        assert_eq!(
            outcome,
            ClueInspectionOutcome::NotConfigured,
            "missing [turns] config must short-circuit to NotConfigured"
        );
    }
}
