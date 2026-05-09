//! Murder Motel shared-world schema.
//!
//! Hosts the example's first game-authored SQLite migration — a small
//! key/value table that records "who unlocked Room 7 first, and when"
//! plus related shared facts. Game migrations live above the kit's
//! reserved 1–99 band, starting at [`MOTEL_MIGRATION_VERSION_BASE`].
//! Real games are free to use typed schemas; this module's `key/value`
//! shape is a fixture choice, not a kit convention.

use foglet_game::{
    DateProvider, FeedbackLine, FogletContext, GameConfig, TurnError, WorldDb, WorldMigration,
};
use rusqlite::OptionalExtension;

/// First version number a Murder Motel migration may use, well above
/// the kit's reserved range so kit growth never collides.
pub const MOTEL_MIGRATION_VERSION_BASE: i64 = 100;

/// Murder Motel's shared key/value world state table. Rows are owned
/// by the game; the kit never reads or writes them. The opaque-key
/// shape makes `sqlite3 ... SELECT * FROM motel_world_state` a
/// self-describing dump.
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

/// `motel_world_state` key for the `CURRENT_TIMESTAMP` of the first
/// Room 7 opening.
pub const ROOM_7_OPENED_AT_KEY: &str = "room_7_opened_at";

/// `motel_world_state` key for the `players.id` of the first opener
/// (stored as TEXT, parsed back to `i64`).
pub const ROOM_7_OPENED_BY_KEY: &str = "room_7_opened_by";

/// Canonical "Room 7 was opened" facts. `first_opening` distinguishes
/// "this call wrote the row" from "the row already existed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room7Opening {
    /// SQLite `'YYYY-MM-DD HH:MM:SS'` UTC text — same shape as
    /// `players.first_seen_at`.
    pub opened_at: String,
    /// First opener's `players.id`.
    pub opened_by_player_id: i64,
    /// `true` only when *this* call inserted the row, so the caller
    /// can branch on "first opener" vs "later observer" without a
    /// second read.
    pub first_opening: bool,
}

/// First-writer-wins record of Room 7's opening. Two `INSERT OR
/// IGNORE` statements (timestamp, opener id) make later callers
/// SQL-level no-ops; the function then reads both keys back so the
/// returned [`Room7Opening`] is canonical. Uses [`WorldDb::connection`]
/// (shared borrow) to match the runtime's `Option<&WorldDb>` lifetime
/// during `handle_input`. Atomicity across the two writes isn't
/// required — a half-written pair is finished by the next caller.
pub fn record_room_7_opening(
    world: &WorldDb,
    player_id: i64,
) -> Result<Room7Opening, rusqlite::Error> {
    let conn = world.connection();
    let opened_at_inserted = conn.execute(
        "INSERT OR IGNORE INTO motel_world_state (key, value) \
         VALUES (?1, CURRENT_TIMESTAMP)",
        rusqlite::params![ROOM_7_OPENED_AT_KEY],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO motel_world_state (key, value) \
         VALUES (?1, ?2)",
        rusqlite::params![ROOM_7_OPENED_BY_KEY, player_id],
    )?;
    // Read back so the returned pair reflects the row that landed,
    // not what we tried to write.
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
        // Coerce parse failure to SQL-layer error so callers keep a
        // single error variant.
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

/// Read-only view of the Room 7 opening. `None` when *either* key is
/// missing — a half-written pair is treated as "not yet opened".
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
        _ => Ok(None),
    }
}

/// Arrival feedback for a player stepping into Room 7. Returns `None`
/// when the current player is the opener (no banner needed); otherwise
/// a single info line citing the prior opening's timestamp without
/// the opener's handle (the kit has no id→handle lookup and SPEC §4.5
/// keeps identities advisory).
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

/// Event-log `kind` label for the first Room 7 unlock.
pub const ROOM_7_OPENED_EVENT_KIND: &str = "room_7_opened";

/// Bulletin message for [`ROOM_7_OPENED_EVENT_KIND`]. No handle in the
/// copy — the row's `player_id` carries attribution.
pub const ROOM_7_OPENED_EVENT_MESSAGE: &str = "Room 7 was unlocked.";

/// Append a `world_events` row for the first Room 7 unlock. Returns
/// `true` only when this call inserted (gated on `opening.first_opening`
/// and underlying-write success). Errors are swallowed so a transient
/// SQLite hiccup cannot soft-lock the Room 7 transition mid-input.
pub fn append_room_7_opened_event(world: &WorldDb, opening: &Room7Opening) -> bool {
    if !opening.first_opening {
        return false;
    }
    world
        .append_event(
            ROOM_7_OPENED_EVENT_KIND,
            Some(opening.opened_by_player_id),
            ROOM_7_OPENED_EVENT_MESSAGE,
            None,
        )
        .is_ok()
}

/// Event-log `kind` for taking a major clue from the Lost-and-Found
/// Drawer. Per-item identity rides in `metadata.item_id`.
pub const CLUE_FOUND_EVENT_KIND: &str = "clue_found";

/// Pending `clue_found` event, queued by a prompt callback and drained
/// by the lobby tick. Owns its strings so the callback can drop its
/// captured slots immediately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingClueEvent {
    pub item_id: String,
    pub message: String,
}

pub const CLUE_FOUND_ROOM_7_KEY_MESSAGE: &str = "Investigator recovered the Room 7 key.";
pub const CLUE_FOUND_MATCHBOOK_MESSAGE: &str = "Investigator pocketed the cracked matchbook.";

/// Map a Lost-and-Found outcome to a `PendingClueEvent` when the
/// press produced a *new* clue. `had_matchbook_before` gates only the
/// matchbook arm; the `(K)` arm is already gated by the prompt's
/// `disabled_if(has_room_7_key, …)` rule.
pub fn pending_clue_event_for(
    outcome: crate::scenes::lost_and_found::LostAndFoundOutcome,
    had_matchbook_before: bool,
) -> Option<PendingClueEvent> {
    use crate::scenes::lost_and_found::LostAndFoundOutcome;
    match outcome {
        LostAndFoundOutcome::TookRoom7Key => Some(PendingClueEvent {
            item_id: crate::map::MapScreen::ROOM_7_KEY_ID.to_string(),
            message: CLUE_FOUND_ROOM_7_KEY_MESSAGE.to_string(),
        }),
        LostAndFoundOutcome::PocketedMatchbook if !had_matchbook_before => Some(PendingClueEvent {
            item_id: crate::map::MapScreen::MATCHBOOK_ID.to_string(),
            message: CLUE_FOUND_MATCHBOOK_MESSAGE.to_string(),
        }),
        LostAndFoundOutcome::PocketedMatchbook
        | LostAndFoundOutcome::ReadReceipt
        | LostAndFoundOutcome::Left => None,
    }
}

/// Leaderboard name for the clue-finder ranking. Must match the
/// `[[leaderboards]]` entry in `assets/game.toml`.
pub const INVESTIGATORS_LEADERBOARD_NAME: &str = "investigators";

/// Score delta one `clue_found` event contributes.
pub const CLUE_FOUND_LEADERBOARD_DELTA: i64 = 1;

/// Drain queued clue events into `world_events`, crediting the player
/// on the investigators board for each landed row. The board
/// increment is gated on the event-append succeeding so a board point
/// always has a matching audit row. Errors are swallowed; the queue
/// is always drained so a flaky disk cannot double-write next tick.
/// Returns the count of events written.
pub fn flush_pending_clue_events(
    world: &WorldDb,
    foglet: &FogletContext,
    slots: &crate::state::SharedSlots,
) -> usize {
    let mut queue = slots.pending_clue_events.borrow_mut();
    if queue.is_empty() {
        return 0;
    }
    // Drain unconditionally so a partial-failure batch can't double-write next tick.
    let drained: Vec<PendingClueEvent> = queue.drain(..).collect();
    drop(queue);
    let player = match world.upsert_player(foglet) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let mut written = 0usize;
    for event in drained {
        let metadata = format!("{{\"item_id\":\"{}\"}}", event.item_id);
        if world
            .append_event(
                CLUE_FOUND_EVENT_KIND,
                Some(player.id),
                &event.message,
                Some(metadata.as_str()),
            )
            .is_ok()
        {
            written += 1;
            // One row, one point. Swallowed on error — the audit row
            // is enough to backfill rank later.
            let _ = world.increment_score(
                INVESTIGATORS_LEADERBOARD_NAME,
                player.id,
                CLUE_FOUND_LEADERBOARD_DELTA,
            );
        }
    }
    written
}

/// Daily turns one clue-inspection action consumes.
pub const CLUE_INSPECTION_TURN_COST: u32 = 1;

/// Feedback surfaced when a clue-inspection press is rejected.
pub const NO_CLUE_TURNS_FEEDBACK: &str = "You're out of clue turns for today — come back tomorrow.";

/// Outcome of a clue-inspection turn-spend attempt. Caller routes on
/// the variant: `Spent` continues, `InsufficientTurns` surfaces
/// [`NO_CLUE_TURNS_FEEDBACK`] and aborts, `NotConfigured` and `Failed`
/// both fall through to opening the prompt (the kit's terminal-safety
/// contract forbids panicking out of `handle_input`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClueInspectionOutcome {
    Spent {
        remaining: i64,
    },
    /// `balance` is the unchanged pre-spend value — `TurnError::
    /// InsufficientTurns` guarantees no mutation on rejection.
    InsufficientTurns {
        balance: i64,
    },
    /// `[turns]` absent from `game.toml`; spending is a no-op.
    NotConfigured,
    /// SQLite or upsert error; message is for tracing, not players.
    Failed(String),
}

/// Spend one clue-inspection turn for `foglet` against `world`. This
/// is the single chokepoint for examining clue hotspots. Errors map
/// to [`ClueInspectionOutcome::Failed`] rather than propagating so a
/// transient DB hiccup cannot soft-lock the player out of inspection.
pub fn spend_clue_inspection_turn(
    world: &WorldDb,
    foglet: &FogletContext,
    cfg: &GameConfig,
    date: &dyn DateProvider,
) -> ClueInspectionOutcome {
    // No `[turns]` ⇒ every inspection is free; skip the upsert.
    let Some(turns) = cfg.turns.as_ref() else {
        return ClueInspectionOutcome::NotConfigured;
    };

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

/// Today's clue-turn balance plus the configured daily allowance, so
/// the renderer can format `Turns: remaining/daily_allowance` without
/// threading `&GameConfig` through the hint code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemainingTurns {
    pub remaining: i64,
    pub daily_allowance: u32,
}

/// Read today's clue-turn balance, returning `None` when `[turns]` is
/// absent or the read fails. Lazily ensures today's row via
/// `ensure_today_turns`; safe from `tick` because the write is atomic,
/// but forbidden in `render` (SPEC §Task 10d).
pub fn read_remaining_turns(
    world: &WorldDb,
    foglet: &FogletContext,
    cfg: &GameConfig,
    date: &dyn DateProvider,
) -> Option<RemainingTurns> {
    let turns = cfg.turns.as_ref()?;
    let player = world.upsert_player(foglet).ok()?;
    let row = world
        .ensure_today_turns(player.id, turns.daily_allowance, turns.carryover_max, date)
        .ok()?;
    Some(RemainingTurns {
        remaining: row.balance,
        daily_allowance: turns.daily_allowance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use foglet_game::WorldDb;
    use tempfile::tempdir;

    #[test]
    fn migration_creates_motel_world_state_table() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");

        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");

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

    #[test]
    fn version_lives_above_kit_reserved_band() {
        // Kit reserves 1–5; base sits above to leave growth room.
        assert!(
            MOTEL_MIGRATION_VERSION_BASE >= 100,
            "game-authored migrations must start at or above the documented base"
        );
        assert_eq!(
            MOTEL_WORLD_STATE_MIGRATION.version, MOTEL_MIGRATION_VERSION_BASE,
            "first Murder Motel migration must occupy the base slot"
        );
    }

    fn world_with_motel_state(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        world
    }

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

    #[test]
    fn room_7_opening_returns_none_before_first_open() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_motel_state(&dir);

        let observed = room_7_opening(&world).expect("read succeeds even with no row");
        assert!(observed.is_none(), "no opener yet => None");
    }

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

    fn world_with_event_stack(dir: &tempfile::TempDir) -> WorldDb {
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&foglet_game::WORLD_EVENTS_MIGRATION)
            .expect("apply world_events migration");
        // `flush_pending_clue_events` also increments the leaderboard
        // — bring the table along or 13e tests can't observe writes.
        world
            .apply_migration(&foglet_game::LEADERBOARD_SCORES_MIGRATION)
            .expect("apply leaderboard_scores migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        world
    }

    #[test]
    fn append_room_7_opened_event_logs_first_opening() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);

        // Seed an opener row through `&Connection` (not `transaction`,
        // which needs `&mut self`) so the runtime's borrow shape holds.
        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'alice', 'user', 50, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed alice");
        let alice_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle = 'alice'", [], |r| {
                r.get(0)
            })
            .expect("read alice id");

        let opening =
            record_room_7_opening(&world, alice_id).expect("record alice's first opening");
        assert!(opening.first_opening, "alice must be the first opener");

        let appended = append_room_7_opened_event(&world, &opening);
        assert!(
            appended,
            "first opener must produce a world_events row (returned false)"
        );

        let events = world
            .recent_events(10)
            .expect("read back recent events for assertion");
        assert_eq!(
            events.len(),
            1,
            "exactly one row should land for the first opening"
        );
        let row = &events[0];
        assert_eq!(row.kind, ROOM_7_OPENED_EVENT_KIND);
        assert_eq!(row.message, ROOM_7_OPENED_EVENT_MESSAGE);
        assert_eq!(
            row.player_id,
            Some(alice_id),
            "event must attribute the opening to the first-writer player id"
        );
        assert!(
            row.metadata.is_none(),
            "Room 7 opening event has no metadata payload (player_id covers attribution)"
        );
    }

    #[test]
    fn append_room_7_opened_event_is_noop_for_later_opener() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);

        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'alice', 'user', 50, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP), \
                        (NULL, 'bob', 'user', 50, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed alice + bob");
        let alice_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle = 'alice'", [], |r| {
                r.get(0)
            })
            .expect("alice id");
        let bob_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle = 'bob'", [], |r| {
                r.get(0)
            })
            .expect("bob id");

        let alice_opening = record_room_7_opening(&world, alice_id).expect("alice opens");
        assert!(append_room_7_opened_event(&world, &alice_opening));

        let bob_opening = record_room_7_opening(&world, bob_id).expect("bob arrives");
        assert!(
            !bob_opening.first_opening,
            "second caller must observe first_opening=false (precondition for this test)"
        );
        let bob_appended = append_room_7_opened_event(&world, &bob_opening);
        assert!(
            !bob_appended,
            "later opener must not append a second bulletin row"
        );

        let events = world.recent_events(10).expect("read events");
        assert_eq!(
            events.len(),
            1,
            "lobby bulletin must show exactly one Room 7 opening across all players"
        );
        assert_eq!(
            events[0].player_id,
            Some(alice_id),
            "the single event must remain attributed to the first opener"
        );
    }

    /// Feeding the read path into the event helper must not double-log.
    #[test]
    fn append_room_7_opened_event_is_noop_for_read_only_observation() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);

        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'alice', 'user', 50, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed alice");
        let alice_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle = 'alice'", [], |r| {
                r.get(0)
            })
            .expect("alice id");

        let writer_opening = record_room_7_opening(&world, alice_id).expect("alice opens");
        assert!(append_room_7_opened_event(&world, &writer_opening));

        let observation = room_7_opening(&world)
            .expect("read succeeds")
            .expect("row exists after writer landed");
        assert!(
            !observation.first_opening,
            "read path always reports first_opening=false (precondition for this test)"
        );
        let appended = append_room_7_opened_event(&world, &observation);
        assert!(
            !appended,
            "feeding a read-only observation in must never produce an event row"
        );

        let count = world.recent_events(10).expect("read events").len();
        assert_eq!(
            count, 1,
            "bulletin must still show exactly one Room 7 event after the read-path probe"
        );
    }

    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{FixedDateProvider, GameConfig, LocalDate, PLAYERS_MIGRATION};

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

    #[test]
    fn spend_clue_inspection_rejects_when_balance_exhausted() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

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

        let outcome_again = spend_clue_inspection_turn(&world, &fc, &cfg, &date);
        assert_eq!(
            outcome_again,
            ClueInspectionOutcome::InsufficientTurns { balance: 0 },
            "repeat rejected attempts must observe the same zero balance"
        );
    }

    #[test]
    fn read_remaining_turns_returns_daily_allowance_on_first_call() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

        let status = read_remaining_turns(&world, &fc, &cfg, &date)
            .expect("status populates with [turns] configured");
        assert_eq!(
            status,
            RemainingTurns {
                remaining: 3,
                daily_allowance: 3,
            },
            "fresh ledger reports the full allowance"
        );
    }

    #[test]
    fn read_remaining_turns_reflects_prior_spend() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let date = fixed_today();

        let _ = spend_clue_inspection_turn(&world, &fc, &cfg, &date);

        let status =
            read_remaining_turns(&world, &fc, &cfg, &date).expect("status populates after spend");
        assert_eq!(
            status,
            RemainingTurns {
                remaining: 2,
                daily_allowance: 3,
            },
            "one spend must leave 3 - 1 = 2 turns observable"
        );
    }

    #[test]
    fn read_remaining_turns_returns_none_when_turns_unconfigured() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let mut cfg: GameConfig = fixture_config();
        cfg.turns = None;
        let fc = fixture_context();
        let date = fixed_today();

        assert!(
            read_remaining_turns(&world, &fc, &cfg, &date).is_none(),
            "missing [turns] must short-circuit to None"
        );
    }

    #[test]
    fn spend_clue_inspection_returns_not_configured_when_turns_missing() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let mut cfg: GameConfig = fixture_config();
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

    /// Day 1 drains, day 2 reads the full allowance and a fresh spend
    /// decrements it — proves the reset row is real, not a phantom read.
    #[test]
    fn daily_reset_restores_clue_turns_for_murder_motel_player() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();

        let mut date =
            FixedDateProvider::new(LocalDate::parse("2026-05-09").expect("day 1 parses"));
        for _ in 0..3 {
            assert!(matches!(
                spend_clue_inspection_turn(&world, &fc, &cfg, &date),
                ClueInspectionOutcome::Spent { .. }
            ));
        }
        assert_eq!(
            spend_clue_inspection_turn(&world, &fc, &cfg, &date),
            ClueInspectionOutcome::InsufficientTurns { balance: 0 },
            "day 1 must end with a zero-balance rejection so day 2 has \
             something concrete to reset"
        );

        date.set(LocalDate::parse("2026-05-10").expect("day 2 parses"));

        let status = read_remaining_turns(&world, &fc, &cfg, &date)
            .expect("turns configured ⇒ status populates");
        assert_eq!(
            status,
            RemainingTurns {
                remaining: 3,
                daily_allowance: 3,
            },
            "carryover_max = 0 means day 2 reads the full allowance regardless \
             of how day 1 ended"
        );

        assert_eq!(
            spend_clue_inspection_turn(&world, &fc, &cfg, &date),
            ClueInspectionOutcome::Spent { remaining: 2 },
            "first spend on day 2 must decrement the freshly-reset row to 2"
        );
    }

    /// Re-set to the same date must not refill — proves the reset is
    /// keyed on calendar day, not on any "provider touched" signal.
    #[test]
    fn same_day_reset_does_not_restore_clue_turns() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_turn_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();

        let mut date = FixedDateProvider::new(LocalDate::parse("2026-05-09").expect("day parses"));
        for _ in 0..3 {
            assert!(matches!(
                spend_clue_inspection_turn(&world, &fc, &cfg, &date),
                ClueInspectionOutcome::Spent { .. }
            ));
        }

        date.set(LocalDate::parse("2026-05-09").expect("day parses"));

        assert_eq!(
            spend_clue_inspection_turn(&world, &fc, &cfg, &date),
            ClueInspectionOutcome::InsufficientTurns { balance: 0 },
            "re-setting to today must not refill the allowance"
        );
    }

    use crate::scenes::lost_and_found::LostAndFoundOutcome;
    use crate::state::SharedSlots;

    #[test]
    fn pending_clue_event_for_take_room_7_key_emits_event() {
        let event = pending_clue_event_for(LostAndFoundOutcome::TookRoom7Key, false)
            .expect("Room 7 key pickup must always queue a clue_found event");
        assert_eq!(event.item_id, crate::map::MapScreen::ROOM_7_KEY_ID);
        assert_eq!(event.message, CLUE_FOUND_ROOM_7_KEY_MESSAGE);

        // `had_matchbook_before` must not affect the (K) path.
        let event_with_flag =
            pending_clue_event_for(LostAndFoundOutcome::TookRoom7Key, true).expect("still queued");
        assert_eq!(event_with_flag, event);
    }

    #[test]
    fn pending_clue_event_for_pocket_matchbook_emits_only_when_new() {
        let new_pickup = pending_clue_event_for(LostAndFoundOutcome::PocketedMatchbook, false)
            .expect("first pocket must queue an event");
        assert_eq!(new_pickup.item_id, crate::map::MapScreen::MATCHBOOK_ID);
        assert_eq!(new_pickup.message, CLUE_FOUND_MATCHBOOK_MESSAGE);

        assert!(
            pending_clue_event_for(LostAndFoundOutcome::PocketedMatchbook, true).is_none(),
            "re-pressing (M) after the matchbook is held must not re-queue an event"
        );
    }

    #[test]
    fn pending_clue_event_for_read_receipt_and_left_are_none() {
        assert!(pending_clue_event_for(LostAndFoundOutcome::ReadReceipt, false).is_none());
        assert!(pending_clue_event_for(LostAndFoundOutcome::ReadReceipt, true).is_none());
        assert!(pending_clue_event_for(LostAndFoundOutcome::Left, false).is_none());
        assert!(pending_clue_event_for(LostAndFoundOutcome::Left, true).is_none());
    }

    #[test]
    fn flush_pending_clue_events_no_op_on_empty_queue() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);
        let slots = SharedSlots::default();
        let fc = fixture_context();

        let written = flush_pending_clue_events(&world, &fc, &slots);
        assert_eq!(written, 0, "empty queue must not write any rows");
        assert_eq!(
            world.recent_events(10).expect("read events").len(),
            0,
            "no rows should land in world_events"
        );
    }

    #[test]
    fn flush_pending_clue_events_writes_rows_drains_queue_and_attributes_player() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);
        let slots = SharedSlots::default();
        let fc = fixture_context();

        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: crate::map::MapScreen::ROOM_7_KEY_ID.to_string(),
                message: CLUE_FOUND_ROOM_7_KEY_MESSAGE.to_string(),
            });
        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: crate::map::MapScreen::MATCHBOOK_ID.to_string(),
                message: CLUE_FOUND_MATCHBOOK_MESSAGE.to_string(),
            });

        let written = flush_pending_clue_events(&world, &fc, &slots);
        assert_eq!(written, 2, "both queued events must reach the DB");
        assert!(
            slots.pending_clue_events.borrow().is_empty(),
            "successful flush must drain the mailbox so the next tick is a no-op"
        );

        let events = world.recent_events(10).expect("read events");
        assert_eq!(events.len(), 2);
        for row in &events {
            assert_eq!(row.kind, CLUE_FOUND_EVENT_KIND);
            assert!(
                row.player_id.is_some(),
                "every clue_found row must attribute the taking player via FK"
            );
            let metadata = row
                .metadata
                .as_deref()
                .expect("clue_found rows ride with item_id metadata");
            assert!(
                metadata.contains("\"item_id\""),
                "metadata must embed the catalog id under item_id: {metadata}"
            );
        }
        let combined = events
            .iter()
            .filter_map(|e| e.metadata.as_deref())
            .collect::<Vec<_>>()
            .join("|");
        assert!(combined.contains(crate::map::MapScreen::ROOM_7_KEY_ID));
        assert!(combined.contains(crate::map::MapScreen::MATCHBOOK_ID));
    }

    #[test]
    fn flush_pending_clue_events_credits_investigators_board_per_event() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);
        let slots = SharedSlots::default();
        let fc = fixture_context();

        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: crate::map::MapScreen::ROOM_7_KEY_ID.to_string(),
                message: CLUE_FOUND_ROOM_7_KEY_MESSAGE.to_string(),
            });

        let written = flush_pending_clue_events(&world, &fc, &slots);
        assert_eq!(written, 1);

        let player = world.upsert_player(&fc).expect("upsert player");
        let top = world
            .top_scores(
                INVESTIGATORS_LEADERBOARD_NAME,
                foglet_game::LeaderboardSort::Desc,
                5,
            )
            .expect("read top scores");
        assert_eq!(top.len(), 1, "exactly one investigator row after one clue");
        assert_eq!(top[0].player_id, player.id);
        assert_eq!(
            top[0].score, CLUE_FOUND_LEADERBOARD_DELTA,
            "first clue must seed the row at the canonical delta"
        );
    }

    /// Catches an `increment_score` → `set_score` regression.
    #[test]
    fn flush_pending_clue_events_accumulates_score_for_repeat_finds() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);
        let slots = SharedSlots::default();
        let fc = fixture_context();

        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: crate::map::MapScreen::ROOM_7_KEY_ID.to_string(),
                message: CLUE_FOUND_ROOM_7_KEY_MESSAGE.to_string(),
            });
        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: crate::map::MapScreen::MATCHBOOK_ID.to_string(),
                message: CLUE_FOUND_MATCHBOOK_MESSAGE.to_string(),
            });

        let written = flush_pending_clue_events(&world, &fc, &slots);
        assert_eq!(written, 2);

        let player = world.upsert_player(&fc).expect("upsert player");
        let rank = world
            .player_rank(
                INVESTIGATORS_LEADERBOARD_NAME,
                foglet_game::LeaderboardSort::Desc,
                player.id,
            )
            .expect("rank lookup");
        assert_eq!(
            rank,
            Some(1),
            "the only investigator must be ranked first after two clues"
        );

        let top = world
            .top_scores(
                INVESTIGATORS_LEADERBOARD_NAME,
                foglet_game::LeaderboardSort::Desc,
                5,
            )
            .expect("read top scores");
        assert_eq!(top.len(), 1, "two clues must collapse onto one row");
        assert_eq!(
            top[0].score,
            2 * CLUE_FOUND_LEADERBOARD_DELTA,
            "two clues must accumulate, not overwrite"
        );
    }

    #[test]
    fn flush_pending_clue_events_does_not_touch_board_on_empty_queue() {
        let dir = tempdir().expect("tempdir");
        let world = world_with_event_stack(&dir);
        let slots = SharedSlots::default();
        let fc = fixture_context();

        let written = flush_pending_clue_events(&world, &fc, &slots);
        assert_eq!(written, 0);

        let top = world
            .top_scores(
                INVESTIGATORS_LEADERBOARD_NAME,
                foglet_game::LeaderboardSort::Desc,
                5,
            )
            .expect("read top scores");
        assert!(
            top.is_empty(),
            "an empty drain must not seed a leaderboard row"
        );
    }
}
