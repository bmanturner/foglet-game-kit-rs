//! `spatial` — directed location graph primitives (SPEC_v4 Tasks 3 and 4).
//!
//! The module covers `places` and `routes`, and stays
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

/// A directed edge in the v4 location graph.
///
/// The row is designed to be a very small, durable adjacency fact:
///
/// - `from_place_id` and `to_place_id` are explicit directed pointers.
/// - `kind` describes the edge channel in game-local vocabulary (e.g.
///   `"airlock"`, `"chute"`), with no kit-side semantics.
/// - `requirements_json` can hold game-defined constraints (minimum
///   player level, locked-door conditions, one-time gating flags).
/// - `metadata_json` carries optional opaque payload for your game’s
///   route metadata model.
///
/// In a **space exploration** game, one route might model a one-way cargo
/// gate while another can represent a return shuttle between the same
/// pair. In a **dungeon crawler**, one route can represent a fragile
/// rope ladder while another represents a locked spiral staircase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Source place row id.
    pub from_place_id: i64,
    /// Destination place row id.
    pub to_place_id: i64,
    /// Stable row identifier.
    pub id: i64,
    /// Game-defined channel label.
    pub kind: String,
    /// Optional requirements payload (opaque JSON).
    pub requirements_json: Option<String>,
    /// Optional route metadata payload (opaque JSON).
    pub metadata_json: Option<String>,
    /// UTC creation timestamp assigned by SQLite (`CURRENT_TIMESTAMP`).
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

/// Errors for shared-route mutations.
///
/// This keeps route creation errors distinct from place creation
/// so callers can present route-specific diagnostics or branch on
/// expected database failures while still preserving typed errors at
/// the boundary.
#[derive(Debug, Error)]
pub enum RouteError {
    /// The underlying SQL `INSERT` failed (constraint violation, disk,
    /// lock timeout, schema mismatch).
    #[error("failed to create route from `{from_place_id}` to `{to_place_id}`: {source}")]
    Sqlite {
        /// Source place id for diagnostics.
        from_place_id: i64,
        /// Destination place id for diagnostics.
        to_place_id: i64,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// The underlying SQL query for outbound adjacency failed.
    #[error("failed to list outbound routes for `{from_place_id}`: {source}")]
    ListOutbound {
        /// Source place id for diagnostics.
        from_place_id: i64,
        /// Underlying `rusqlite` error.
        #[source]
        source: rusqlite::Error,
    },
    /// The underlying SQL query for inbound adjacency failed.
    #[error("failed to list inbound routes for `{to_place_id}`: {source}")]
    ListInbound {
        /// Destination place id for diagnostics.
        to_place_id: i64,
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

/// Migration for the shared `routes` table (SPEC_v4 Task 4a).
///
/// `routes` encodes directed edges in the graph built from
/// [`PLACES_MIGRATION`].
///
/// - `from_place_id` — origin node.
/// - `to_place_id` — destination node.
/// - `kind` — game-defined edge label (e.g. `"airlock"`, `"corridor"`).
/// - `requirements_json` — optional, opaque requirements payload.
/// - `metadata_json` — optional, opaque game-defined payload.
/// - `created_at` — UTC creation timestamp.
///
/// This migration keeps graph edges explicit and directional:
/// a pair of rows can model bidirectionality, and repeated rows with
/// different `kind` values can model distinct channels between the same
/// place pair.
///
/// In a space game, one edge can model a one-way cargo transfer while
/// another models a different atmospheric gate between the same docking
/// nodes.
///
/// In a dungeon, one edge can encode a one-way chute while another can
/// encode a locked stairway in the opposite direction.
pub const ROUTES_MIGRATION: WorldMigration = WorldMigration {
    version: 11,
    name: "create_routes",
    sql: "\
CREATE TABLE IF NOT EXISTS routes (\n\
    id INTEGER PRIMARY KEY,\n\
    from_place_id      INTEGER NOT NULL,\n\
    to_place_id        INTEGER NOT NULL,\n\
    kind               TEXT NOT NULL,\n\
    requirements_json  TEXT,\n\
    metadata_json      TEXT,\n\
    created_at         TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    FOREIGN KEY (from_place_id) REFERENCES places(id),\n\
    FOREIGN KEY (to_place_id) REFERENCES places(id)\n\
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

    /// Create one directed route row and return the stored record.
    ///
    /// Route creation is intentionally orthogonal and explicit:
    ///
    /// - No route is implied by place creation.
    /// - No route directionality is inferred by convention.
    /// - `requirements_json` / `metadata_json` are opaque, so game
    ///   authors can encode any constraints they want.
    ///
    /// This is the first write API for the route table and returns the
    /// inserted row via `RETURNING` so callers can verify idempotent
    /// bootstrapping and debug immediately.
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space exploration** game, create a one-way cargo gate
    ///   from `"station-omega-dock"` to `"outpost-gamma"` with kind
    ///   `"cargo-channel"` and a clearance payload.
    /// - In a **dungeon crawler** game, create a one-way chute from
    ///   `"tower-catwalk"` to `"basement-corridor"` with kind
    ///   `"chute"` and custom hazard metadata.
    pub fn create_route(
        &self,
        from_place_id: i64,
        to_place_id: i64,
        kind: &str,
        requirements_json: Option<&str>,
        metadata_json: Option<&str>,
    ) -> Result<Route, RouteError> {
        const SQL: &str = "\
INSERT INTO routes (\n\
    from_place_id, to_place_id, kind, requirements_json, metadata_json\n\
)\n\
VALUES (?1, ?2, ?3, ?4, ?5)\n\
RETURNING from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    from_place_id,
                    to_place_id,
                    kind,
                    requirements_json,
                    metadata_json
                ],
                row_to_route,
            )
            .map_err(|source| RouteError::Sqlite {
                from_place_id,
                to_place_id,
                source,
            })
    }

    /// Query all routes whose source is `from_place_id`.
    ///
    /// This is the directed adjacency view used by movement logic. It
    /// does **not** auto-materialise reverse edges; a bidirectional
    /// connection must be represented as two explicit route rows by game
    /// code.
    ///
    /// In a **space exploration** game, this gives a craft the outgoing
    /// options from a dock node (toward a mining outpost or customs
    /// station) without implying any return lanes.
    ///
    /// In a **dungeon crawler**, this yields all corridors/chutes/stair
    /// passages that depart from a chamber, while the reverse direction
    /// is only available if authored separately.
    pub fn outbound_routes(&self, from_place_id: i64) -> Result<Vec<Route>, RouteError> {
        const SQL: &str = "\
SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at\n\
FROM routes\n\
WHERE from_place_id = ?1\n\
ORDER BY to_place_id ASC, id ASC";

        let mut stmt =
            self.connection()
                .prepare(SQL)
                .map_err(|source| RouteError::ListOutbound {
                    from_place_id,
                    source,
                })?;
        let routes = stmt
            .query_map(rusqlite::params![from_place_id], row_to_route)
            .map_err(|source| RouteError::ListOutbound {
                from_place_id,
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| RouteError::ListOutbound {
                from_place_id,
                source,
            })?;

        Ok(routes)
    }

    /// Query all routes whose destination is `to_place_id`.
    ///
    /// This returns the inbound half of the directed graph and allows
    /// the same location to be queried for both outward exits and incoming
    /// entrances. It intentionally does not infer reciprocal edges:
    /// callers should only call this if they need inbound visibility.
    ///
    /// In a **space exploration** game, this supports displays like
    /// "where can I come from right now?" for a docking pad.
    ///
    /// In a **dungeon crawler** game, this supports a fog map layer that
    /// reveals how a chamber can be reached without assuming any of those
    /// passages are bidirectional by default.
    pub fn inbound_routes(&self, to_place_id: i64) -> Result<Vec<Route>, RouteError> {
        const SQL: &str = "\
SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at\n\
FROM routes\n\
WHERE to_place_id = ?1\n\
ORDER BY from_place_id ASC, id ASC";

        let mut stmt =
            self.connection()
                .prepare(SQL)
                .map_err(|source| RouteError::ListInbound {
                    to_place_id,
                    source,
                })?;
        let routes = stmt
            .query_map(rusqlite::params![to_place_id], row_to_route)
            .map_err(|source| RouteError::ListInbound {
                to_place_id,
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| RouteError::ListInbound {
                to_place_id,
                source,
            })?;

        Ok(routes)
    }

    /// Load one place by its stable `key`.
    ///
    /// This is the primary lookup for game code that stores authored
    /// identifiers such as:
    ///
    /// - In a **space exploration** game, resolving a docking location
    ///   like `"sector-lima-gate"` when loading a map.
    /// - In a **dungeon crawler**, resolving a room node such as
    ///   `"crypt-door-03"` when validating movement targets.
    ///
    /// Return `Ok(None)` when the key is unknown; all other SQL errors
    /// still surface through [`PlaceError`] so callers can preserve the
    /// same transaction/error handling expectations used by `insert_place`.
    pub fn get_place_by_key(&self, key: &str) -> Result<Option<Place>, PlaceError> {
        const SQL: &str = "\
SELECT id, key, display_name, kind, metadata_json, created_at\n\
FROM places\n\
WHERE key = ?1";

        match self
            .connection()
            .query_row(SQL, rusqlite::params![key], row_to_place)
        {
            Ok(place) => Ok(Some(place)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(PlaceError::Sqlite {
                key: key.to_string(),
                source,
            }),
        }
    }

    /// Return all places in deterministic key order.
    ///
    /// The API intentionally sorts by `key` so that command/UI code gets
    /// stable lists suitable for snapshots, auto-complete menus, and
    /// debug consoles. This is a neutral primitive:
    ///
    /// - In a **space exploration** game, the ordering makes route
    ///   builders render predictable station lists.
    /// - In a **dungeon crawler**, it keeps room navigation menus
    ///   deterministic across runs.
    pub fn list_places(&self) -> Result<Vec<Place>, PlaceError> {
        const SQL: &str = "\
SELECT id, key, display_name, kind, metadata_json, created_at\n\
FROM places\n\
ORDER BY key ASC, id ASC";

        let mut stmt = self
            .connection()
            .prepare(SQL)
            .map_err(|source| PlaceError::Sqlite {
                key: "<list_places>".to_string(),
                source,
            })?;
        let places = stmt
            .query_map([], row_to_place)
            .map_err(|source| PlaceError::Sqlite {
                key: "<list_places>".to_string(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PlaceError::Sqlite {
                key: "<list_places>".to_string(),
                source,
            })?;

        Ok(places)
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

fn row_to_route(row: &rusqlite::Row<'_>) -> rusqlite::Result<Route> {
    Ok(Route {
        from_place_id: row.get(0)?,
        to_place_id: row.get(1)?,
        id: row.get(2)?,
        kind: row.get(3)?,
        requirements_json: row.get(4)?,
        metadata_json: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{PLACES_MIGRATION, ROUTES_MIGRATION};
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
    }

    /// SPEC_v4 Task 4a accepts that `create_routes` applies after
    /// `create_places` and declares both edge columns plus both place
    /// foreign keys.
    ///
    /// The test asserts:
    ///
    /// - Exact schema ordering for predictable SQL introspection.
    /// - Two foreign-key edges to `places(id)`, proving direct
    ///   directed-graph representation.
    #[test]
    fn applies_routes_migration_with_place_fks() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&ROUTES_MIGRATION)
            .expect("routes migration applies");

        let mut stmt = world
            .connection()
            .prepare("SELECT name FROM pragma_table_info('routes') ORDER BY cid")
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
                "from_place_id".to_string(),
                "to_place_id".to_string(),
                "kind".to_string(),
                "requirements_json".to_string(),
                "metadata_json".to_string(),
                "created_at".to_string(),
            ],
            "routes schema must match SPEC_v4 Task 4a exactly"
        );

        let mut fk_stmt = world
            .connection()
            .prepare("PRAGMA foreign_key_list('routes')")
            .expect("foreign_key_list preparable");
        let fk_rows = fk_stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .expect("foreign-key rows decode");

        let mut keys = vec![];
        for result in fk_rows {
            keys.push(result.expect("query row decodes"));
        }

        assert!(keys.contains(&(
            "places".to_string(),
            "from_place_id".to_string(),
            "id".to_string()
        )));
        assert!(keys.contains(&(
            "places".to_string(),
            "to_place_id".to_string(),
            "id".to_string()
        )));
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

    /// SPEC_v4 Task 3c requires key-level uniqueness enforcement.
    ///
    /// Insert the same logical key twice and confirm the second
    /// operation fails through the `insert_place` path with a
    /// diagnostic that names the key and the failed operation.
    #[test]
    fn insert_place_rejects_duplicate_key() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");

        let first = world
            .insert_place(
                "shared-hub",
                "Shared Hub",
                "junction",
                Some(r#"{"beacons": 3}"#),
            )
            .expect("first insert with this key succeeds");
        assert!(first.id > 0, "first insert should create a concrete row");

        let duplicate = world.insert_place(
            "shared-hub",
            "Second Entry For Same Key",
            "chamber",
            Some(r#"{"beacons": 1}"#),
        );
        let err = duplicate.expect_err("duplicate place key must fail");
        let rendered = err.to_string();
        assert!(
            rendered.contains("shared-hub"),
            "error should include the conflicting key"
        );
        assert!(
            rendered.contains("failed to insert place"),
            "error should be emitted from the insert path"
        );
    }

    /// SPEC_v4 Task 3d requires deterministic list ordering so game-side
    /// UIs and tests can depend on stable output.
    #[test]
    fn list_places_returns_sorted_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");

        let alpha = world
            .insert_place("zeta-dock", "Zeta Dock", "dock", None)
            .expect("insert by key works");
        world
            .insert_place("alpha-hall", "Alpha Hall", "chamber", None)
            .expect("insert by key works");
        world
            .insert_place("midpoint", "Midpoint Hub", "hub", None)
            .expect("insert by key works");

        let rows = world.list_places().expect("listing places is queryable");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].key, "alpha-hall");
        assert_eq!(rows[1].key, "midpoint");
        assert_eq!(rows[2].key, "zeta-dock");
        assert_eq!(rows[0].display_name, "Alpha Hall");
        assert_eq!(rows[1].display_name, "Midpoint Hub");
        assert_eq!(rows[2].display_name, "Zeta Dock");
        let ids = rows.iter().map(|place| place.id).collect::<Vec<_>>();
        assert!(
            ids.iter().all(|id| *id > 0),
            "listed rows should be real rows"
        );
        assert!(alpha.id > 0, "inserted fixture row should have concrete id");
    }

    /// SPEC_v4 Task 3d also requires key lookup semantics.
    #[test]
    fn get_place_by_key_returns_match_or_none() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");

        let inserted = world
            .insert_place(
                "frontier-node",
                "Frontier Node",
                "node",
                Some(r#"{"hazard": 3}"#),
            )
            .expect("insert by key works");

        let found = world
            .get_place_by_key("frontier-node")
            .expect("place lookup should succeed");
        assert_eq!(found, Some(inserted.clone()));

        let missing = world
            .get_place_by_key("missing-node")
            .expect("missing lookup should be Ok(None)");
        assert!(
            missing.is_none(),
            "unknown keys should return None without SQL error"
        );
    }

    /// SPEC_v4 Task 3e requires opaque metadata storage semantics: the
    /// byte sequence in `metadata_json` must be preserved by the write path.
    ///
    /// This uses a dense JSON fixture with whitespace, nested objects,
    /// and escaped characters to catch accidental normalization.
    #[test]
    fn insert_place_preserves_metadata_json_byte_for_byte() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");

        let metadata = "{\n  \"lore\": \"sector-alpha\\nencounter\\\"gate\\\"\",\n  \"tags\": [\"dock\", \"refuel\"],\n  \"danger\": 7\n}";
        let inserted = world
            .insert_place("echo-bay", "Echo Bay", "docking-bay", Some(metadata))
            .expect("insert_by_key works");

        assert_eq!(inserted.metadata_json.as_deref(), Some(metadata));

        let from_db: String = world
            .connection()
            .query_row(
                "SELECT metadata_json FROM places WHERE id = ?1",
                rusqlite::params![inserted.id],
                |row| row.get(0),
            )
            .expect("direct metadata read should return a row");
        assert_eq!(from_db, metadata);
    }

    /// SPEC_v4 Task 4b accepts `create_route` and that the returned
    /// `Route` row is the same row written to SQLite.
    ///
    /// This test creates two place nodes, then one directed route and
    /// reads it back by primary id:
    ///
    /// - The row includes the caller-provided `kind`, `requirements_json`,
    ///   and `metadata_json`.
    /// - The `created_at` timestamp is non-empty and comes from SQLite.
    #[test]
    fn create_route_returns_stored_row() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&ROUTES_MIGRATION)
            .expect("routes migration applies");

        let source = world
            .insert_place("docking-ring", "Docking Ring", "dock", None)
            .expect("source place inserts");
        let target = world
            .insert_place("bridge-corridor", "Bridge Corridor", "corridor", None)
            .expect("target place inserts");

        let route = world
            .create_route(
                source.id,
                target.id,
                "airlock",
                Some(r#"{"cargo_ok": true}"#),
                Some(r#"{"notes":"forward-only, pressure-locked"}"#),
            )
            .expect("route creation succeeds");

        assert!(
            route.id > 0,
            "SQLite should assign a concrete autoincrement id"
        );
        assert_eq!(route.from_place_id, source.id);
        assert_eq!(route.to_place_id, target.id);
        assert_eq!(route.kind, "airlock");
        assert_eq!(
            route.requirements_json.as_deref(),
            Some(r#"{"cargo_ok": true}"#)
        );
        assert_eq!(
            route.metadata_json.as_deref(),
            Some(r#"{"notes":"forward-only, pressure-locked"}"#)
        );
        assert!(
            !route.created_at.is_empty(),
            "SQLite CURRENT_TIMESTAMP should populate created_at"
        );

        let by_id = world
            .connection()
            .query_row(
                "SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at \
                 FROM routes WHERE id = ?1",
                rusqlite::params![route.id],
                super::row_to_route,
            )
            .expect("route row should be queryable by id");

        assert_eq!(route, by_id);
    }

    /// SPEC_v4 Task 4c — `outbound_routes` must reflect only the source
    /// side of the directed route row.
    ///
    /// This is a strict-direction test: one route A→B and one route B→A
    /// are both valid rows, but querying outbound from A only returns A→B.
    #[test]
    fn outbound_routes_filters_to_source_only() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&ROUTES_MIGRATION)
            .expect("routes migration applies");

        let star_hatch = world
            .insert_place("star-hatch", "Star Hatch", "hatch", None)
            .expect("source place inserts");
        let refinery = world
            .insert_place("refinery", "Refinery", "facility", None)
            .expect("destination place inserts");
        let hangar = world
            .insert_place("hangar", "Hangar", "hangar", None)
            .expect("alternate destination inserts");

        let forward = world
            .create_route(
                star_hatch.id,
                refinery.id,
                "airlock",
                None,
                Some(r#"{"route":"hatch-to-refinery"}"#),
            )
            .expect("outbound route inserts");
        world
            .create_route(
                refinery.id,
                star_hatch.id,
                "airlock",
                None,
                Some(r#"{"route":"refinery-return"}"#),
            )
            .expect("inbound route inserts");
        let second_outbound = world
            .create_route(
                star_hatch.id,
                hangar.id,
                "supply-shaft",
                None,
                Some(r#"{"route":"hatch-to-hangar"}"#),
            )
            .expect("second outbound route inserts");

        let routes = world
            .outbound_routes(star_hatch.id)
            .expect("outbound query should succeed");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].from_place_id, star_hatch.id);
        assert_eq!(routes[1].from_place_id, star_hatch.id);
        assert_eq!(routes[0].to_place_id, refinery.id);
        assert_eq!(routes[1].to_place_id, hangar.id);
        assert_eq!(routes[0].kind, "airlock");
        assert_eq!(routes[1].kind, "supply-shaft");
        assert_eq!(routes, vec![forward, second_outbound]);

        let inbound_routes: Vec<_> = world
            .connection()
            .prepare("SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at FROM routes WHERE from_place_id = ?1")
            .expect("inbound query preparable")
            .query_map(rusqlite::params![refinery.id], super::row_to_route)
            .expect("inbound queryable")
            .collect::<Result<Vec<_>, _>>()
            .expect("inbound decode");
        assert_eq!(inbound_routes.len(), 1);
        assert_eq!(inbound_routes[0].from_place_id, refinery.id);
        assert_eq!(inbound_routes[0].to_place_id, star_hatch.id);
    }

    /// SPEC_v4 Task 4d — `inbound_routes` should return only routes whose
    /// destination is the provided place.
    ///
    /// The test keeps two separate source nodes feeding a common destination
    /// and confirms outbound rows from unrelated places are not returned.
    #[test]
    fn inbound_routes_filters_to_destination_only() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&ROUTES_MIGRATION)
            .expect("routes migration applies");

        let star_hatch = world
            .insert_place("star-hatch", "Star Hatch", "hatch", None)
            .expect("source place inserts");
        let refinery = world
            .insert_place("refinery", "Refinery", "facility", None)
            .expect("destination place inserts");
        let hangar = world
            .insert_place("hangar", "Hangar", "hangar", None)
            .expect("alternate source inserts");

        let incoming_refinery = world
            .create_route(
                refinery.id,
                star_hatch.id,
                "cargo-bay",
                None,
                Some(r#"{"route":"refinery-to-hatch"}"#),
            )
            .expect("inbound route inserts");
        let incoming_hangar = world
            .create_route(
                hangar.id,
                star_hatch.id,
                "maintenance-passage",
                None,
                Some(r#"{"route":"hangar-to-hatch"}"#),
            )
            .expect("inbound route inserts");
        world
            .create_route(
                star_hatch.id,
                refinery.id,
                "airlock",
                None,
                Some(r#"{"route":"hatch-to-refinery"}"#),
            )
            .expect("outbound route inserts");

        let inbound = world
            .inbound_routes(star_hatch.id)
            .expect("inbound query should succeed");

        assert_eq!(inbound.len(), 2);
        assert_eq!(inbound[0], incoming_refinery);
        assert_eq!(inbound[1], incoming_hangar);
        assert_eq!(inbound[0].to_place_id, star_hatch.id);
        assert_eq!(inbound[1].to_place_id, star_hatch.id);
        assert_eq!(inbound[0].from_place_id, refinery.id);
        assert_eq!(inbound[1].from_place_id, hangar.id);
    }
}
