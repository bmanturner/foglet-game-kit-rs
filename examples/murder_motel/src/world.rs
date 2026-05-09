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

use foglet_game::WorldMigration;

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
}
