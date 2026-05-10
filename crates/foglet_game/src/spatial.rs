//! `spatial` — directed location graph primitives (SPEC_v4 Task 3).
//!
//! The module currently covers the `places` concept only. It stays
//! intentionally genre-neutral:
//!
//! - In a **space exploration** game, a row can represent a docking
//!   dock, star gate, or sector node.
//! - In a **dungeon crawler** game, a row can represent a cave room,
//!   bridge crossing, or hidden vault.
//!
//! This lets the table evolve through adjacency and movement tasks
//! without ever baking one story vocabulary into shared library code.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// A durable location node in the v4 spatial graph.
///
/// The row is intentionally minimal. It stores what every game world
/// needs to reason about a place and nothing more:
///
/// - `key` identifies the place in game-authored data (`"airlock-01"`,
///   `"forge-room"`, `"north-gate"`). It is unique for the life of
///   the world so migration code and saved references remain stable.
/// - `display_name` is a user-facing short label.
/// - `kind` is game-defined and unopinionated (e.g. `"dock"`,
///   `"chamber"`, `"market"`).
/// - `metadata_json` carries any optional arbitrary JSON that the game
///   chooses (descriptive text, fog-of-war flags, encounter lists).
/// - `created_at` is assigned by SQLite (`CURRENT_TIMESTAMP`) and can
///   be surfaced in admin/debug workflows.
///
/// A space station can store weather metadata or route-specific lore in
/// `metadata_json` while a dungeon can keep trap seeds or atmospheric
/// clues in the same column. The shared kit does not interpret this
/// string at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// Stable row identifier for direct foreign-key references.
    pub id: i64,
    /// Game-defined stable key.
    pub key: String,
    /// Human-readable label.
    pub display_name: String,
    /// Game-defined kind/tag.
    pub kind: String,
    /// Optional opaque JSON payload.
    pub metadata_json: Option<String>,
    /// UTC creation timestamp stored as text by SQLite.
    pub created_at: String,
}

/// Errors for shared-place mutations.
///
/// Public write paths return a single enum so callers can branch on
/// "insertion failed" versus other module-specific errors as the API
/// expands in Task 3c.
#[derive(Debug, Error)]
pub enum PlaceError {
    /// The underlying SQL `INSERT` failed (constraint violation, disk,
    /// lock timeout, schema mismatch).
    #[error("failed to insert place `{key}`: {source}")]
    Sqlite {
        /// Place key for caller-friendly diagnostics.
        key: String,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
}

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

impl WorldDb {
    /// Insert one `Place` row and return the full stored record.
    ///
    /// Callers use this from their door bootstrap / admin code or from a
    /// world-building screen to seed place nodes. The method intentionally
    /// returns the inserted row, so callers get:
    ///
    /// - Stable `id` for later route creation.
    /// - Canonical `metadata_json` as written (or NULL) for immediate
    ///   consistency checks.
    /// - `created_at` as stored by SQLite instead of assuming timezone
    ///   formatting in Rust.
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space game**, a call can seed `"hangar-a"` with kind
    ///   `"port"` and a metadata blob like `{"docking_bays": 3}`.
    /// - In a **dungeon game**, a call can seed `"chamber-riverside"`
    ///   with kind `"chamber"` and `{"mobile_density": 0.4}`.
    ///
    /// # Concurrency
    ///
    /// Uses one `INSERT` statement with `RETURNING`; readers can execute
    /// it through `&self`, matching existing v2 shared-world patterns.
    /// Unique-key conflicts are surfaced as [`PlaceError::Sqlite`] and are
    /// validated explicitly in the follow-up task for key collisions.
    pub fn insert_place(
        &self,
        key: &str,
        display_name: &str,
        kind: &str,
        metadata_json: Option<&str>,
    ) -> Result<Place, PlaceError> {
        const SQL: &str = "\
INSERT INTO places (key, display_name, kind, metadata_json)\n\
VALUES (?1, ?2, ?3, ?4)\n\
RETURNING id, key, display_name, kind, metadata_json, created_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![key, display_name, kind, metadata_json],
                row_to_place,
            )
            .map_err(|source| PlaceError::Sqlite {
                key: key.to_string(),
                source,
            })
    }
}

fn row_to_place(row: &rusqlite::Row<'_>) -> rusqlite::Result<Place> {
    Ok(Place {
        id: row.get(0)?,
        key: row.get(1)?,
        display_name: row.get(2)?,
        kind: row.get(3)?,
        metadata_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

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

    /// SPEC_v4 Task 3b requires a typed round-trip surface for a
    /// single place insert.
    ///
    /// The test seeds one space-themed place and asserts that:
    ///
    /// - The row reports a concrete `id` and populated `created_at`.
    /// - The value inserted as JSON metadata comes back unchanged.
    /// - A direct SQL read by `id` reproduces exactly the same struct.
    #[test]
    fn insert_place_returns_stored_row() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");

        let inserted = world
            .insert_place(
                "hab-module-9",
                "Habitation Module 9",
                "habitat",
                Some(r#"{"oxygen": "stable"}"#),
            )
            .expect("insert_place inserts a valid place");
        assert!(
            inserted.id > 0,
            "SQLite AUTOINCREMENT row id should be a positive integer"
        );
        assert!(
            !inserted.created_at.is_empty(),
            "created_at should be set by sqlite"
        );
        assert_eq!(
            inserted.metadata_json,
            Some(r#"{"oxygen": "stable"}"#.to_string())
        );

        let by_id = world
            .connection()
            .query_row(
                "SELECT id, key, display_name, kind, metadata_json, created_at \
                 FROM places WHERE id = ?1",
                rusqlite::params![inserted.id],
                super::row_to_place,
            )
            .expect("stored row should be queryable");
        assert_eq!(inserted, by_id);
    }
}
