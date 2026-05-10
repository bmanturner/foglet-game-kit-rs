//! `presence` — current-location tracking schema (SPEC_v4 Task 5a).
//!
//! This module owns the durable schema for per-player current place.
//! Movement and transactional APIs come in later tasks; this iteration
//! adds only the migration table shape and the schema contract tests.
//!
//! In a **space exploration** game, a row here tracks that a captain is
//! currently at a specific docking node. In a **dungeon crawler**, it
//! tracks which chamber or corridor the player currently occupies. The
//! table intentionally stays minimal so every genre authorship decision
//! remains outside the kit.

use crate::world_db::WorldMigration;

/// Per-player current location row shape.
///
/// The table itself is introduced in Task 5a and consumed by
/// later API tasks:
///
/// - `set_presence(player_id, place_id, metadata_json)` creates/updates
///   this record (Task 5b).
/// - `move_player(player_id, dest_place_id, on_commit)` updates it in a
///   single transaction with movement callbacks (Task 5c).
/// - `get_presence(player_id)` and `players_at(place_id)` read this row
///   (Task 5e, 5f).
///
/// Both game families are kept in mind:
///
/// - In a **space exploration** game, `player_id` and `place_id` are
///   crew/member and station IDs.
/// - In a **dungeon crawler**, they are player/chamber IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceRecord {
    /// Stable player identifier from `players.id`.
    pub player_id: i64,
    /// Place identifier from `places.id` for the current location.
    pub place_id: i64,
    /// Time this presence row was created and last updated by movement.
    pub entered_at: String,
    /// Optional opaque JSON payload for game-authored metadata.
    pub metadata_json: Option<String>,
}

/// Migration for `presence` (Task 5a).
///
/// The schema is intentionally compact:
///
/// - `player_id` is the primary key and one-to-one locator for the
///   current player position.
/// - `place_id` points at the player's current place.
/// - `entered_at` captures when this row became current.
/// - `metadata_json` stores any game-authored payload.
///
/// The `Task 5a` contract requires this exact column shape; no
/// movement semantics or auto-placement policy live here.
pub const PRESENCE_MIGRATION: WorldMigration = WorldMigration {
    version: 12,
    name: "create_presence",
    sql: "\
CREATE TABLE IF NOT EXISTS presence (\n\
    player_id      INTEGER PRIMARY KEY,\n\
    place_id       INTEGER NOT NULL,\n\
    entered_at     TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    metadata_json  TEXT,\n\
    FOREIGN KEY (player_id) REFERENCES players(id),\n\
    FOREIGN KEY (place_id) REFERENCES places(id)\n\
);\n\
",
};

#[cfg(test)]
mod tests {
    use super::PRESENCE_MIGRATION;
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v4 Task 5a requires that the `presence` migration applies
    /// and creates the documented columns in table order.
    #[test]
    fn applies_presence_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        // Presence references `places`, so we reuse the spatial migration
        // from Task 3 to keep the FK target available for CREATE TABLE.
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&PRESENCE_MIGRATION)
            .expect("presence migration applies");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('presence') ORDER BY cid")
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
                "entered_at".to_string(),
                "metadata_json".to_string(),
            ],
            "presence schema must match SPEC_v4 Task 5a exactly"
        );

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params![PRESENCE_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(row_count, PRESENCE_MIGRATION.version);
    }
}
