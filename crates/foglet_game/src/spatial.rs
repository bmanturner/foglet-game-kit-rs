//! `spatial` — directed location graph primitives (SPEC_v4 Task 3).
//!
//! Task 3a only introduces the `places` table migration so
//! route/route traversal APIs can be added on top in the next
//! v4 tasks without revisiting schema ownership later.
//!
//! The table stays deliberately generic:
//!
//! - In a **space exploration** game, a row can represent a docking
//!   port, star gate, or station sector.
//! - In a **dungeon crawler** game, a row can represent a cave
//!   chamber, bridge gate, or hidden cavern.
//!
//! This avoids encoding any one genre in the schema and keeps the
//! migration reusable.

use crate::world_db::WorldMigration;

/// Migration for the shared `places` table (SPEC_v4 Task 3a).
///
/// The table intentionally stores only the structural fields the kit
/// needs for navigation:
///
/// - `id` — row-local integer key for stable foreign references from
///   `routes`.
/// - `key` — stable string key authored by the game (must be unique).
/// - `display_name` — human-readable label for UI.
/// - `kind` — caller-owned subtype label (e.g. `"dock"` or `"chamber"`),
///   intentionally unopinionated.
/// - `metadata_json` — opaque game-defined payload.
/// - `created_at` — UTC creation timestamp.
///
/// In a starship game you might treat `kind` as orbital context
/// (`"hangar"`, `"hangar-bay"`) and in a dungeon as `"corridor"` or
/// `"vault"`; this migration keeps both valid without adding any
/// one-vocabulary semantics.
pub const PLACES_MIGRATION: WorldMigration = WorldMigration {
    version: 10,
    name: "create_places",
    sql: "\
CREATE TABLE IF NOT EXISTS places (\n\
    id              INTEGER PRIMARY KEY,\n\
    key             TEXT NOT NULL UNIQUE,\n\
    display_name    TEXT NOT NULL,\n\
    kind            TEXT NOT NULL,\n\
    metadata_json   TEXT,\n\
    created_at      TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n\
);\n\
",
};

#[cfg(test)]
mod tests {
    use super::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    /// SPEC_v4 Task 3a accepts that `create_places` applies and
    /// creates the documented columns in order, including metadata
    /// openness.
    #[test]
    fn applies_places_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies cleanly to a fresh DB");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('places') ORDER BY cid")
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
                "key".to_string(),
                "display_name".to_string(),
                "kind".to_string(),
                "metadata_json".to_string(),
                "created_at".to_string(),
            ],
            "places schema must match SPEC_v4 Task 3a exactly"
        );

        let row_count: i64 = world
            .connection()
            .query_row(
                "SELECT version FROM world_migrations WHERE name = ?1",
                rusqlite::params![PLACES_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("recorded migration row is queryable");
        assert_eq!(row_count, PLACES_MIGRATION.version);

        world
            .connection()
            .execute(
                "INSERT INTO places (key, display_name, kind, metadata_json) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "docking-bay-01",
                    "Forward Docking Bay",
                    "dock",
                    r#"{\"light\": \"blue\"}"#
                ],
            )
            .expect("inserting a sample place works");

        let duplicate = world.connection().execute(
            "INSERT INTO places (key, display_name, kind, metadata_json) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                "docking-bay-01",
                "Duplicate Dock",
                "chamber",
                r#"{\"light\": \"red\"}#"#
            ],
        );
        assert!(duplicate.is_err(), "place keys must be unique");
    }
}
