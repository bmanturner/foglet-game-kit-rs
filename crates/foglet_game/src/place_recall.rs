//! `place_recall` — discovered-place memory schema (SPEC_v4 Task 6a).
//!
//! This module introduces the storage primitive for fog-of-war and
//! place-memory features. The schema is intentionally minimal and keeps
//! all semantics game-defined:
//!
//! - `player_id` and `place_id` identify a player's observed visit.
//! - `first_seen_at` records when that location was first observed.
//! - `last_seen_at` tracks the latest observed touch time.
//! - `snapshot_json` carries any optional game-authored payload.
//!
//! By Task 6a, callers can only create the table and run it through the
//! existing migration pipeline. Query/mutation APIs land in later tasks so
//! a game can decide exactly how recall rows should be interpreted.

use crate::world_db::WorldMigration;

/// Migration for `place_recall` rows (SPEC_v4 Task 6a).
///
/// The table intentionally stores one row per `(player_id, place_id)`:
///
/// - `first_seen_at` and `last_seen_at` are both timestamp strings that
///   default to `CURRENT_TIMESTAMP`. The table-level semantics are:
///   every touch should advance `last_seen_at`, while preserving
///   `first_seen_at` as the initial observation time.
/// - `snapshot_json` remains opaque text for game-specific display
///   payloads (a dungeon map tile, a ship-sector card excerpt, etc.).
///
/// In a **space exploration** game, `place_recall` can retain the last
/// seen station-level fog map state for each crew member.
/// In a **dungeon crawler**, it can record each chamber a character has
/// entered for mini-map recoloration.
pub const PLACE_RECALL_MIGRATION: WorldMigration = WorldMigration {
    version: 13,
    name: "create_place_recall",
    sql: "\
CREATE TABLE IF NOT EXISTS place_recall (\n\
    player_id      INTEGER NOT NULL,\n\
    place_id       INTEGER NOT NULL,\n\
    first_seen_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    last_seen_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    snapshot_json  TEXT,\n\
    PRIMARY KEY (player_id, place_id),\n\
    FOREIGN KEY (player_id) REFERENCES players(id),\n\
    FOREIGN KEY (place_id) REFERENCES places(id)\n\
);\n\
",
};

#[cfg(test)]
mod tests {
    use super::PLACE_RECALL_MIGRATION;
    use crate::players::PLAYERS_MIGRATION;
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// Task 6a accepts that the recall migration applies and publishes
    /// the documented table shape to SQLite.
    ///
    /// The order matters for later upsert/merge work: both player and
    /// place foreign keys must exist before touch operations are added in
    /// 6b+.
    #[test]
    fn applies_place_recall_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&PLACE_RECALL_MIGRATION)
            .expect("place_recall migration applies");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('place_recall') ORDER BY cid")
            .expect("pragma_table_info preparable");
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            columns,
            vec![
                "player_id".to_string(),
                "place_id".to_string(),
                "first_seen_at".to_string(),
                "last_seen_at".to_string(),
                "snapshot_json".to_string(),
            ],
            "place_recall schema must match SPEC_v4 Task 6a exactly"
        );

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params![PLACE_RECALL_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(row_count, PLACE_RECALL_MIGRATION.version);
    }
}
