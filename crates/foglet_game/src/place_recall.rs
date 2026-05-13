//! `place_recall` — discovered-place memory schema and touch/query primitives
//! .
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
//! By, callers can only create the table and run it through the
//! existing migration pipeline.
//!
//! In a **space exploration** game, `place_recall` can retain the last
//! seen station-level fog map state for each crew member.
//! In a **dungeon crawler**, it can record each chamber a character has
//! entered for mini-map recoloration.

use rusqlite::OptionalExtension;
use thiserror::Error;

use crate::world_db::WorldDb;
use crate::world_db::WorldMigration;

/// Snapshot row for one player's recall of one place.
///
/// `PlaceRecallRecord` is intentionally tiny and owned by the shared
/// world DB. A game may treat it as:
///
/// - A **space exploration** "sector observation log", where each row ties
///   a captain to a waypoint and a last-seen timestamp.
/// - A **dungeon crawler** "room memory entry", where each row drives
///   mini-map fog recoloring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRecallRecord {
    /// Player identifier from `players.id`.
    pub player_id: i64,
    /// Place identifier from `places.id`.
    pub place_id: i64,
    /// Time this player first observed the place.
    pub first_seen_at: String,
    /// Time this player last observed the place.
    pub last_seen_at: String,
    /// Optional opaque JSON snapshot captured with the touch.
    pub snapshot_json: Option<String>,
}

/// Errors for place-recall writes and reads.
///
/// Keeping the write path errors explicit and named lets call sites
/// present clean diagnostics to players while still logging the exact
/// SQLite cause in logs.
#[derive(Debug, Error)]
pub enum PlaceRecallError {
    /// Inserted/updated recall state could not be committed.
    #[error("failed to touch recall for player `{player_id}` at place `{place_id}`: {source}")]
    TouchFailed {
        /// Player identifier used in `touch_recall`.
        player_id: i64,
        /// Place identifier used in `touch_recall`.
        place_id: i64,
        /// Underlying SQL failure (constraint, lock, corruption, etc.).
        #[source]
        source: rusqlite::Error,
    },
    /// Querying a player's recall list failed.
    #[error("failed to list recall entries for player `{player_id}`: {source}")]
    ListFailed {
        /// Player identifier used in `recall_for_player`.
        player_id: i64,
        /// Underlying SQL failure (constraint, lock, corruption, etc.).
        #[source]
        source: rusqlite::Error,
    },
    /// Reading the existing snapshot before a merge failed.
    #[error(
        "failed to read recall snapshot for player `{player_id}` at place `{place_id}`: {source}"
    )]
    MergeReadFailed {
        /// Player identifier used in `merge_recall_snapshot`.
        player_id: i64,
        /// Place identifier used in `merge_recall_snapshot`.
        place_id: i64,
        /// Underlying SQL failure (constraint, lock, corruption, etc.).
        #[source]
        source: rusqlite::Error,
    },
    /// Writing the merged snapshot failed.
    #[error("failed to write merged recall snapshot for player `{player_id}` at place `{place_id}`: {source}")]
    MergeWriteFailed {
        /// Player identifier used in `merge_recall_snapshot`.
        player_id: i64,
        /// Place identifier used in `merge_recall_snapshot`.
        place_id: i64,
        /// Underlying SQL failure (constraint, lock, corruption, etc.).
        #[source]
        source: rusqlite::Error,
    },
    /// Existing `snapshot_json` could not be parsed as JSON.
    #[error(
        "invalid recall snapshot JSON for player `{player_id}` at place `{place_id}`: {source}"
    )]
    InvalidSnapshotJson {
        /// Player identifier used in `merge_recall_snapshot`.
        player_id: i64,
        /// Place identifier used in `merge_recall_snapshot`.
        place_id: i64,
        /// Existing invalid snapshot payload.
        snapshot_json: String,
        /// JSON parse error.
        #[source]
        source: serde_json::Error,
    },
    /// Existing `snapshot_json` parsed, but was not a JSON object.
    #[error(
        "recall snapshot JSON for player `{player_id}` at place `{place_id}` must be an object"
    )]
    SnapshotNotObject {
        /// Player identifier used in `merge_recall_snapshot`.
        player_id: i64,
        /// Place identifier used in `merge_recall_snapshot`.
        place_id: i64,
        /// Existing non-object snapshot payload.
        snapshot_json: String,
    },
    /// A namespace merge was requested without at least one path segment.
    #[error(
        "recall namespace path for player `{player_id}` at place `{place_id}` must not be empty"
    )]
    NamespacePathEmpty {
        /// Player identifier used in `merge_recall_namespace`.
        player_id: i64,
        /// Place identifier used in `merge_recall_namespace`.
        place_id: i64,
    },
    /// A namespace merge path contained an empty segment.
    #[error(
        "recall namespace path segment {index} for player `{player_id}` at place `{place_id}` must not be empty"
    )]
    NamespacePathSegmentEmpty {
        /// Player identifier used in `merge_recall_namespace`.
        player_id: i64,
        /// Place identifier used in `merge_recall_namespace`.
        place_id: i64,
        /// Segment index in the namespace path.
        index: usize,
    },
    /// A namespace merge payload was not a JSON object.
    #[error(
        "recall namespace merge for `{namespace_path}` at player `{player_id}` place `{place_id}` must be a JSON object"
    )]
    NamespaceMergeNotObject {
        /// Player identifier used in `merge_recall_namespace`.
        player_id: i64,
        /// Place identifier used in `merge_recall_namespace`.
        place_id: i64,
        /// Human-readable namespace path.
        namespace_path: String,
    },
    /// A namespace path hit an existing non-object value.
    #[error(
        "recall namespace `{namespace_path}` for player `{player_id}` at place `{place_id}` contains non-object segment `{segment}`"
    )]
    NamespaceNotObject {
        /// Player identifier used in `merge_recall_namespace`.
        player_id: i64,
        /// Place identifier used in `merge_recall_namespace`.
        place_id: i64,
        /// Human-readable namespace path.
        namespace_path: String,
        /// Segment whose existing value was not an object.
        segment: String,
    },
    /// Serializing the merged snapshot failed.
    #[error("failed to serialize merged recall snapshot for player `{player_id}` at place `{place_id}`: {source}")]
    SnapshotSerializeFailed {
        /// Player identifier used in `merge_recall_snapshot`.
        player_id: i64,
        /// Place identifier used in `merge_recall_snapshot`.
        place_id: i64,
        /// JSON serialization error.
        #[source]
        source: serde_json::Error,
    },
}

/// Migration for `place_recall` rows.
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

impl WorldDb {
    /// Record or refresh a player's recall observation of a place.
    ///
    /// The contract for is idempotent upsert:
    ///
    /// - first call for `(player_id, place_id)` inserts a row with both
    ///   `first_seen_at` and `last_seen_at` seeded from `CURRENT_TIMESTAMP`;
    /// - repeated calls leave `first_seen_at` untouched and advance
    ///   `last_seen_at`.
    ///
    /// This is the primitive that implements per-player fog-of-war memory
    /// (for example, "which docking bays have we already inspected?" in
    /// a **space exploration** map, or "which chambers are already
    /// revisited?" in a **dungeon crawler** map). Games decide whether
    /// movement should call this automatically.
    pub fn touch_recall(
        &self,
        player_id: i64,
        place_id: i64,
        snapshot_json: Option<&str>,
    ) -> Result<PlaceRecallRecord, PlaceRecallError> {
        const SQL: &str = "\
INSERT INTO place_recall (player_id, place_id, first_seen_at, last_seen_at, snapshot_json)\n\
VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?3)\n\
ON CONFLICT(player_id, place_id) DO UPDATE\n\
SET last_seen_at = CURRENT_TIMESTAMP,\n\
    snapshot_json = excluded.snapshot_json\n\
RETURNING player_id, place_id, first_seen_at, last_seen_at, snapshot_json";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![player_id, place_id, snapshot_json],
                row_to_place_recall,
            )
            .map_err(|source| PlaceRecallError::TouchFailed {
                player_id,
                place_id,
                source,
            })
    }

    /// List a player's known places in recall order (most recently
    /// touched first).
    ///
    /// The primitive is intentionally thin. It does **not** infer "seen"
    /// semantics; games own that decision. The query returns every known
    /// `(place_id, first_seen_at, last_seen_at)` row for one player
    /// sorted deterministically so fog-of-war and minimap UIs remain
    /// stable across process restarts.
    ///
    /// # Genre-neutral usage
    ///
    /// - In a **space exploration** game, this gives a crew officer a
    ///   stable list of recently scanned waypoints to render as a
    ///   recently-visited list.
    /// - In a **dungeon crawler**, it yields a deterministic revisit
    ///   order for room-memory panes after a character moves through
    ///   multiple halls.
    pub fn recall_for_player(
        &self,
        player_id: i64,
    ) -> Result<Vec<PlaceRecallRecord>, PlaceRecallError> {
        const SQL: &str = "\
SELECT player_id, place_id, first_seen_at, last_seen_at, snapshot_json\n\
FROM place_recall\n\
WHERE player_id = ?1\n\
ORDER BY last_seen_at DESC, place_id DESC";

        let mut statement = self
            .connection()
            .prepare(SQL)
            .map_err(|source| PlaceRecallError::ListFailed { player_id, source })?;
        let rows = statement
            .query_map(rusqlite::params![player_id], row_to_place_recall)
            .map_err(|source| PlaceRecallError::ListFailed { player_id, source })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| PlaceRecallError::ListFailed { player_id, source })
    }

    /// Merge one game-owned recall snapshot object and write it back.
    ///
    /// This method is the connection-scoped convenience wrapper around
    /// [`merge_recall_snapshot_on`]. It does not open a transaction; use
    /// the free function with an active `&rusqlite::Transaction` when a
    /// larger game action needs the merge to roll back with surrounding
    /// work.
    pub fn merge_recall_snapshot<F>(
        &self,
        player_id: i64,
        place_id: i64,
        merge: F,
    ) -> Result<PlaceRecallRecord, PlaceRecallError>
    where
        F: FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    {
        merge_recall_snapshot_on(self.connection(), player_id, place_id, merge)
    }

    /// Deep-merge a JSON object into one namespace path inside a recall
    /// snapshot.
    ///
    /// This is the connection-scoped convenience wrapper around
    /// [`merge_recall_namespace_on`]. It creates missing namespace
    /// objects, preserves sibling namespaces, and rejects existing
    /// non-object values at the requested path with typed errors.
    pub fn merge_recall_namespace(
        &self,
        player_id: i64,
        place_id: i64,
        namespace_path: &[&str],
        merge_value: serde_json::Value,
    ) -> Result<PlaceRecallRecord, PlaceRecallError> {
        merge_recall_namespace_on(
            self.connection(),
            player_id,
            place_id,
            namespace_path,
            merge_value,
        )
    }
}

/// Merge one game-owned recall snapshot object using an existing
/// connection or transaction.
///
/// The helper reads the current `(player_id, place_id)` row, treats a
/// missing or `NULL` snapshot as an empty JSON object, lets `merge`
/// mutate that object, and writes the merged object back. It preserves
/// `first_seen_at`, advances `last_seen_at`, and creates the row when
/// it does not yet exist.
///
/// Existing invalid JSON returns [`PlaceRecallError::InvalidSnapshotJson`]
/// and leaves the row unchanged. Existing valid non-object JSON returns
/// [`PlaceRecallError::SnapshotNotObject`] because object merging cannot
/// preserve unrelated keys inside arrays, strings, booleans, or numbers.
pub fn merge_recall_snapshot_on<F>(
    conn: &rusqlite::Connection,
    player_id: i64,
    place_id: i64,
    merge: F,
) -> Result<PlaceRecallRecord, PlaceRecallError>
where
    F: FnOnce(&mut serde_json::Map<String, serde_json::Value>),
{
    merge_recall_snapshot_on_result(conn, player_id, place_id, |snapshot| {
        merge(snapshot);
        Ok(())
    })
}

/// Deep-merge a JSON object into a namespace path in one recall snapshot.
///
/// `namespace_path` is a sequence such as
/// `["salvage", "derelicts", "derelict.key"]`. Missing path segments
/// are created as JSON objects. Existing sibling keys and sibling
/// namespaces are preserved. If an existing value at the namespace path
/// is not an object, the helper returns
/// [`PlaceRecallError::NamespaceNotObject`] and leaves the row unchanged.
///
/// The helper accepts an existing SQLite connection or transaction and
/// does not commit, so larger game actions can roll back recall facts
/// with inventory, events, proof rows, or movement.
pub fn merge_recall_namespace_on(
    conn: &rusqlite::Connection,
    player_id: i64,
    place_id: i64,
    namespace_path: &[&str],
    merge_value: serde_json::Value,
) -> Result<PlaceRecallRecord, PlaceRecallError> {
    let path = normalize_namespace_path(namespace_path, player_id, place_id)?;
    let namespace_path = namespace_path_label(&path);
    let serde_json::Value::Object(merge_object) = merge_value else {
        return Err(PlaceRecallError::NamespaceMergeNotObject {
            player_id,
            place_id,
            namespace_path,
        });
    };

    merge_recall_snapshot_on_result(conn, player_id, place_id, |snapshot| {
        let namespace =
            namespace_object_mut(snapshot, &path, &namespace_path, player_id, place_id)?;
        deep_merge_object(namespace, merge_object);
        Ok(())
    })
}

fn merge_recall_snapshot_on_result<F>(
    conn: &rusqlite::Connection,
    player_id: i64,
    place_id: i64,
    merge: F,
) -> Result<PlaceRecallRecord, PlaceRecallError>
where
    F: FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), PlaceRecallError>,
{
    let current_snapshot: Option<Option<String>> = conn
        .query_row(
            "SELECT snapshot_json FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
            rusqlite::params![player_id, place_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|source| PlaceRecallError::MergeReadFailed {
            player_id,
            place_id,
            source,
        })?;

    let mut object = match current_snapshot.flatten() {
        Some(snapshot_json) => {
            let value: serde_json::Value =
                serde_json::from_str(&snapshot_json).map_err(|source| {
                    PlaceRecallError::InvalidSnapshotJson {
                        player_id,
                        place_id,
                        snapshot_json: snapshot_json.clone(),
                        source,
                    }
                })?;
            match value {
                serde_json::Value::Object(object) => object,
                _ => {
                    return Err(PlaceRecallError::SnapshotNotObject {
                        player_id,
                        place_id,
                        snapshot_json,
                    });
                }
            }
        }
        None => serde_json::Map::new(),
    };

    merge(&mut object)?;

    let merged_json =
        serde_json::to_string(&serde_json::Value::Object(object)).map_err(|source| {
            PlaceRecallError::SnapshotSerializeFailed {
                player_id,
                place_id,
                source,
            }
        })?;

    const SQL: &str = "\
INSERT INTO place_recall (player_id, place_id, first_seen_at, last_seen_at, snapshot_json)\n\
VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ?3)\n\
ON CONFLICT(player_id, place_id) DO UPDATE\n\
SET last_seen_at = CURRENT_TIMESTAMP,\n\
    snapshot_json = excluded.snapshot_json\n\
RETURNING player_id, place_id, first_seen_at, last_seen_at, snapshot_json";

    conn.query_row(
        SQL,
        rusqlite::params![player_id, place_id, merged_json],
        row_to_place_recall,
    )
    .map_err(|source| PlaceRecallError::MergeWriteFailed {
        player_id,
        place_id,
        source,
    })
}

fn normalize_namespace_path(
    namespace_path: &[&str],
    player_id: i64,
    place_id: i64,
) -> Result<Vec<String>, PlaceRecallError> {
    if namespace_path.is_empty() {
        return Err(PlaceRecallError::NamespacePathEmpty {
            player_id,
            place_id,
        });
    }

    namespace_path
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            if segment.is_empty() {
                Err(PlaceRecallError::NamespacePathSegmentEmpty {
                    player_id,
                    place_id,
                    index,
                })
            } else {
                Ok((*segment).to_string())
            }
        })
        .collect()
}

fn namespace_path_label(namespace_path: &[String]) -> String {
    namespace_path.join("/")
}

fn namespace_object_mut<'a>(
    object: &'a mut serde_json::Map<String, serde_json::Value>,
    path: &[String],
    namespace_path: &str,
    player_id: i64,
    place_id: i64,
) -> Result<&'a mut serde_json::Map<String, serde_json::Value>, PlaceRecallError> {
    let Some((segment, rest)) = path.split_first() else {
        return Ok(object);
    };

    let value = object
        .entry(segment.clone())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    match value {
        serde_json::Value::Object(child) => {
            namespace_object_mut(child, rest, namespace_path, player_id, place_id)
        }
        _ => Err(PlaceRecallError::NamespaceNotObject {
            player_id,
            place_id,
            namespace_path: namespace_path.to_string(),
            segment: segment.clone(),
        }),
    }
}

fn deep_merge_object(
    target: &mut serde_json::Map<String, serde_json::Value>,
    patch: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in patch {
        match value {
            serde_json::Value::Object(patch_child) => {
                if let Some(serde_json::Value::Object(target_child)) = target.get_mut(&key) {
                    deep_merge_object(target_child, patch_child);
                } else {
                    target.insert(key, serde_json::Value::Object(patch_child));
                }
            }
            other => {
                target.insert(key, other);
            }
        }
    }
}

fn row_to_place_recall(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlaceRecallRecord> {
    Ok(PlaceRecallRecord {
        player_id: row.get(0)?,
        place_id: row.get(1)?,
        first_seen_at: row.get(2)?,
        last_seen_at: row.get(3)?,
        snapshot_json: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{row_to_place_recall, PlaceRecallError, PlaceRecallRecord, PLACE_RECALL_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use std::thread::sleep;
    use std::time::Duration;
    use tempfile::tempdir;

    ///  accepts that the recall migration applies and publishes
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
            "place_recall schema must match exactly"
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

    ///  requires `touch_recall` to insert a fresh row on first
    /// call and refresh `last_seen_at` on subsequent calls.
    ///
    /// The test inserts explicit fixture rows first, so foreign keys
    /// and idempotent-upsert behavior can be observed in isolation.
    #[test]
    fn touch_recall_inserts_then_refreshes_last_seen_at() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Scout Relay"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");

        let docking_bay = world
            .insert_place("d-04", "Docking Bay D-04", "dock", None)
            .expect("fixture place insert");
        let first = world
            .touch_recall(player_id, docking_bay.id, Some(r#"{"glyph":"🛰"}"#))
            .expect("initial touch");
        let first_seen_at = first.first_seen_at;

        sleep(Duration::from_secs(1));

        let second = world
            .touch_recall(
                player_id,
                docking_bay.id,
                Some(r#"{"glyph":"🛰","status":"warmed"}"#),
            )
            .expect("second touch");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
                rusqlite::params![player_id, docking_bay.id],
                |row| row.get(0),
            )
            .expect("count is queryable");

        assert_eq!(
            count, 1,
            "touch_recall must remain one row per (player, place)"
        );
        assert_eq!(second.player_id, player_id);
        assert_eq!(second.place_id, docking_bay.id);
        assert_eq!(
            first.place_id, second.place_id,
            "place_id should remain stable across touch"
        );
        assert_eq!(
            first_seen_at, second.first_seen_at,
            "first_seen_at should stay unchanged"
        );
        assert!(
            second.last_seen_at > first.last_seen_at,
            "second touch should refresh last_seen_at"
        );
        assert_eq!(
            second.snapshot_json,
            Some(r#"{"glyph":"🛰","status":"warmed"}"#.to_string())
        );

        let loaded: PlaceRecallRecord = world
            .connection()
            .query_row(
                "SELECT player_id, place_id, first_seen_at, last_seen_at, snapshot_json FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
                rusqlite::params![player_id, docking_bay.id],
                row_to_place_recall,
            )
            .expect("touched recall row query");

        assert_eq!(second, loaded);
    }

    #[test]
    fn merge_recall_snapshot_inserts_missing_row() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Cartographer"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let place = world
            .insert_place("map-room", "Map Room", "room", None)
            .expect("fixture place inserts");

        let record = world
            .merge_recall_snapshot(player_id, place.id, |snapshot| {
                snapshot.insert(
                    "map_annotation".to_string(),
                    serde_json::Value::String("marked northern stairs".to_string()),
                );
            })
            .expect("merge inserts missing recall row");

        assert_eq!(record.player_id, player_id);
        assert_eq!(record.place_id, place.id);
        assert!(!record.first_seen_at.is_empty());
        assert!(!record.last_seen_at.is_empty());

        let snapshot = snapshot_value(&record);
        assert_eq!(
            snapshot["map_annotation"],
            serde_json::Value::String("marked northern stairs".to_string())
        );
    }

    #[test]
    fn merge_recall_snapshot_preserves_unrelated_keys_and_advances_last_seen() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Surveyor"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let place = world
            .insert_place("hazard-hall", "Hazard Hall", "hall", None)
            .expect("fixture place inserts");

        let first = world
            .touch_recall(
                player_id,
                place.id,
                Some(
                    r#"{"map_annotation":"old note","scanned_exits":["north"],"room_hazards":{"gas":true}}"#,
                ),
            )
            .expect("initial recall touch");
        sleep(Duration::from_secs(1));

        let merged = world
            .merge_recall_snapshot(player_id, place.id, |snapshot| {
                snapshot.insert(
                    "map_annotation".to_string(),
                    serde_json::Value::String("updated note".to_string()),
                );
                snapshot.insert("scan_depth".to_string(), serde_json::Value::from(3));
            })
            .expect("merge updates existing object");

        assert_eq!(
            merged.first_seen_at, first.first_seen_at,
            "merge must preserve first_seen_at"
        );
        assert!(
            merged.last_seen_at > first.last_seen_at,
            "merge must advance last_seen_at"
        );

        let snapshot = snapshot_value(&merged);
        assert_eq!(snapshot["map_annotation"], "updated note");
        assert_eq!(snapshot["scan_depth"], 3);
        assert_eq!(snapshot["scanned_exits"], serde_json::json!(["north"]));
        assert_eq!(snapshot["room_hazards"], serde_json::json!({"gas": true}));
    }

    #[test]
    fn merge_recall_snapshot_rejects_invalid_existing_json_without_rewriting() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Archivist"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let place = world
            .insert_place("broken-memory", "Broken Memory", "room", None)
            .expect("fixture place inserts");

        world
            .touch_recall(player_id, place.id, Some("{not-json"))
            .expect("touch_recall stores opaque game-owned payload");

        let err = world
            .merge_recall_snapshot(player_id, place.id, |snapshot| {
                snapshot.insert("map_annotation".to_string(), serde_json::json!("repaired"));
            })
            .expect_err("invalid existing JSON should fail before writing");
        match err {
            PlaceRecallError::InvalidSnapshotJson {
                player_id: err_player,
                place_id: err_place,
                snapshot_json,
                ..
            } => {
                assert_eq!(err_player, player_id);
                assert_eq!(err_place, place.id);
                assert_eq!(snapshot_json, "{not-json");
            }
            other => panic!("expected InvalidSnapshotJson, got {other:?}"),
        }

        let stored: String = world
            .connection()
            .query_row(
                "SELECT snapshot_json FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
                rusqlite::params![player_id, place.id],
                |row| row.get(0),
            )
            .expect("stored snapshot is queryable");
        assert_eq!(
            stored, "{not-json",
            "failed merge must not rewrite invalid game-owned payload"
        );
    }

    #[test]
    fn merge_recall_snapshot_on_observes_transaction_rollback() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Mapper"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let place = world
            .insert_place("rollback-room", "Rollback Room", "room", None)
            .expect("fixture place inserts");

        {
            let tx = world
                .connection_mut()
                .transaction()
                .expect("transaction begins");
            let record = super::merge_recall_snapshot_on(&tx, player_id, place.id, |snapshot| {
                snapshot.insert("scanned_exit".to_string(), serde_json::json!("east"));
            })
            .expect("merge works inside transaction");
            assert_eq!(snapshot_value(&record)["scanned_exit"], "east");
        }

        let rows = world
            .recall_for_player(player_id)
            .expect("recall query succeeds after dropped transaction");
        assert!(
            rows.is_empty(),
            "dropped transaction should roll back inserted recall merge"
        );
    }

    #[test]
    fn merge_recall_namespace_preserves_sibling_namespaces() {
        let NamespaceFixture {
            world,
            player_id,
            place_id,
            _dir,
        } = setup_namespace_fixture();

        world
            .touch_recall(
                player_id,
                place_id,
                Some(
                    r#"{
                        "navigation":{"visited":true},
                        "salvage":{
                            "summary":"mapped",
                            "derelicts":{
                                "other":{"status":"open"},
                                "derelict.key":{"status":"old","nested":{"first":1}}
                            }
                        }
                    }"#,
                ),
            )
            .expect("initial namespaced recall inserts");

        let merged = world
            .merge_recall_namespace(
                player_id,
                place_id,
                &["salvage", "derelicts", "derelict.key"],
                serde_json::json!({
                    "status": "stripped",
                    "nested": {"second": 2},
                    "proof": "black-box"
                }),
            )
            .expect("namespace merge succeeds");

        let snapshot = snapshot_value(&merged);
        assert_eq!(snapshot["navigation"], serde_json::json!({"visited": true}));
        assert_eq!(snapshot["salvage"]["summary"], "mapped");
        assert_eq!(
            snapshot["salvage"]["derelicts"]["other"],
            serde_json::json!({"status": "open"})
        );
        assert_eq!(
            snapshot["salvage"]["derelicts"]["derelict.key"],
            serde_json::json!({
                "status": "stripped",
                "nested": {"first": 1, "second": 2},
                "proof": "black-box"
            }),
            "one system should update its namespace without erasing sibling facts"
        );
    }

    #[test]
    fn merge_recall_namespace_rejects_non_object_path_without_rewriting() {
        let NamespaceFixture {
            world,
            player_id,
            place_id,
            _dir,
        } = setup_namespace_fixture();
        let original = r#"{"navigation":{"visited":true},"salvage":{"derelicts":"sealed"}}"#;
        world
            .touch_recall(player_id, place_id, Some(original))
            .expect("initial recall inserts");

        let err = world
            .merge_recall_namespace(
                player_id,
                place_id,
                &["salvage", "derelicts", "derelict.key"],
                serde_json::json!({"status":"mapped"}),
            )
            .expect_err("non-object namespace path should fail");

        match err {
            PlaceRecallError::NamespaceNotObject {
                player_id: err_player,
                place_id: err_place,
                namespace_path,
                segment,
            } => {
                assert_eq!(err_player, player_id);
                assert_eq!(err_place, place_id);
                assert_eq!(namespace_path, "salvage/derelicts/derelict.key");
                assert_eq!(segment, "derelicts");
            }
            other => panic!("expected NamespaceNotObject, got {other:?}"),
        }

        let stored: String = world
            .connection()
            .query_row(
                "SELECT snapshot_json FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
                rusqlite::params![player_id, place_id],
                |row| row.get(0),
            )
            .expect("stored snapshot reads");
        assert_eq!(
            stored, original,
            "failed namespace merge must not rewrite sibling facts"
        );
    }

    #[test]
    fn merge_recall_namespace_rejects_non_object_merge_value() {
        let NamespaceFixture {
            world,
            player_id,
            place_id,
            _dir,
        } = setup_namespace_fixture();

        let err = world
            .merge_recall_namespace(
                player_id,
                place_id,
                &["salvage"],
                serde_json::json!(["not", "an", "object"]),
            )
            .expect_err("namespace merge payload must be an object");

        match err {
            PlaceRecallError::NamespaceMergeNotObject {
                player_id: err_player,
                place_id: err_place,
                namespace_path,
            } => {
                assert_eq!(err_player, player_id);
                assert_eq!(err_place, place_id);
                assert_eq!(namespace_path, "salvage");
            }
            other => panic!("expected NamespaceMergeNotObject, got {other:?}"),
        }
        assert!(
            world
                .recall_for_player(player_id)
                .expect("recall reads")
                .is_empty(),
            "invalid merge value should not create a recall row"
        );
    }

    #[test]
    fn merge_recall_namespace_on_observes_transaction_rollback() {
        let NamespaceFixture {
            mut world,
            player_id,
            place_id,
            _dir,
        } = setup_namespace_fixture();

        {
            let tx = world
                .connection_mut()
                .transaction()
                .expect("transaction begins");
            let record = super::merge_recall_namespace_on(
                &tx,
                player_id,
                place_id,
                &["salvage", "derelicts", "derelict.key"],
                serde_json::json!({"status":"mapped"}),
            )
            .expect("namespace merge works inside transaction");
            assert_eq!(
                snapshot_value(&record)["salvage"]["derelicts"]["derelict.key"]["status"],
                "mapped"
            );
        }

        assert!(
            world
                .recall_for_player(player_id)
                .expect("recall reads after rollback")
                .is_empty(),
            "dropping the caller transaction should roll back namespace merge"
        );
    }

    ///  requires `first_seen_at` to stay fixed after the first
    /// touch, even while `last_seen_at` updates.
    ///
    /// This captures the "fog-of-war memory" contract directly: revisiting a
    /// **dungeon room** should not erase the first-seen breadcrumb, while
    /// revisits should still refresh the latest-seen time.
    #[test]
    fn touch_recall_preserves_first_seen_at_without_merging_metadata() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Helmsman"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");

        let corridor = world
            .insert_place(
                "d-05",
                "Narrow Corridor",
                "corridor",
                Some(r#"{"zone":"A"}"#),
            )
            .expect("fixture place inserts");

        let first = world
            .touch_recall(
                player_id,
                corridor.id,
                Some(r#"{"glyph":"◉","note":"cold"}"#),
            )
            .expect("initial touch");

        sleep(Duration::from_secs(1));

        let second = world
            .touch_recall(
                player_id,
                corridor.id,
                Some(r#"{"glyph":"◉","note":"warm"}"#),
            )
            .expect("re-touch");

        assert_eq!(
            first.first_seen_at, second.first_seen_at,
            "first_seen_at must remain constant across repeated touches"
        );
        assert!(
            second.last_seen_at > first.last_seen_at,
            "last_seen_at must refresh on repeat touches"
        );
        assert_eq!(
            second.snapshot_json,
            Some(r#"{"glyph":"◉","note":"warm"}"#.to_string()),
            "latest snapshot should still replace the previous one"
        );
    }

    ///  requires each touch call to refresh `snapshot_json`
    /// independently of historical memory.
    ///
    /// Fog-of-war replay in a **space exploration** flow should allow a
    /// station to be revisited with fresh sensor packets, and a
    /// **dungeon crawl** should likewise retain the most recent room
    /// context for rendering revisit hints.
    #[test]
    fn touch_recall_updates_snapshot_json_each_time() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Archivist"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");

        let observatory = world
            .insert_place(
                "obs-dome",
                "Observation Dome",
                "dome",
                Some(r#"{"lighting":"dim"}"#),
            )
            .expect("fixture place insert");

        world
            .touch_recall(player_id, observatory.id, Some(r#"{"phase":"dock"}"#))
            .expect("first snapshot capture");
        sleep(Duration::from_secs(1));

        let second = world
            .touch_recall(player_id, observatory.id, Some(r#"{"phase":"scan"}"#))
            .expect("second snapshot capture");
        assert_eq!(
            second.snapshot_json,
            Some(r#"{"phase":"scan"}"#.to_string()),
            "latest snapshot should replace prior payload"
        );

        let third = world
            .touch_recall(player_id, observatory.id, Some(r#"{"phase":"alarm"}"#))
            .expect("third snapshot capture");
        assert_eq!(
            third.snapshot_json,
            Some(r#"{"phase":"alarm"}"#.to_string()),
            "snapshot should update on every touch, including after prior updates"
        );

        let row: PlaceRecallRecord = world
            .connection()
            .query_row(
                "SELECT player_id, place_id, first_seen_at, last_seen_at, snapshot_json FROM place_recall WHERE player_id = ?1 AND place_id = ?2",
                rusqlite::params![player_id, observatory.id],
                row_to_place_recall,
            )
            .expect("row query");

        assert_eq!(
            row.snapshot_json, third.snapshot_json,
            "persistent row should mirror latest in-memory touch result"
        );
    }

    ///  requires `recall_for_player` to return records in
    /// deterministic newest-first order.
    ///
    /// The contract is intentionally simple: a higher-level game UI can
    /// render visit history for recall panels without reordering, and if
    /// multiple rows share a timestamp the implementation stays stable.
    /// This supports both **space exploration** scout logs and **dungeon**
    /// revisit maps.
    #[test]
    fn recall_for_player_returns_newest_first_deterministically() {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Signal Operator"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");

        let hub = world
            .insert_place("orbital-hub", "Orbital Hub", "hub", None)
            .expect("fixture place insert");
        let tunnel = world
            .insert_place("tunnel", "Maintenance Tunnel", "corridor", None)
            .expect("fixture place insert");
        let vault = world
            .insert_place("copper-vault", "Copper Vault", "vault", None)
            .expect("fixture place insert");

        let hub_recall = world
            .touch_recall(player_id, hub.id, Some(r#"{"channel":"beacon"}"#))
            .expect("older recall entry");
        sleep(Duration::from_secs(1));
        let tunnel_recall = world
            .touch_recall(player_id, tunnel.id, Some(r#"{"channel":"maintenance"}"#))
            .expect("newer recall entry");
        sleep(Duration::from_secs(1));
        let vault_recall = world
            .touch_recall(player_id, vault.id, Some(r#"{"channel":"alarms"}"#))
            .expect("newest recall entry");

        let items = world
            .recall_for_player(player_id)
            .expect("recall_for_player succeeds");

        assert_eq!(items.len(), 3);
        assert_eq!(items[0], vault_recall);
        assert_eq!(items[1], tunnel_recall);
        assert_eq!(items[2], hub_recall);
    }

    struct NamespaceFixture {
        world: WorldDb,
        player_id: i64,
        place_id: i64,
        _dir: tempfile::TempDir,
    }

    fn setup_namespace_fixture() -> NamespaceFixture {
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

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Namespace Tester"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let place = world
            .insert_place("namespace-room", "Namespace Room", "room", None)
            .expect("fixture place inserts");

        NamespaceFixture {
            world,
            player_id,
            place_id: place.id,
            _dir: dir,
        }
    }

    fn snapshot_value(record: &PlaceRecallRecord) -> serde_json::Value {
        serde_json::from_str(
            record
                .snapshot_json
                .as_deref()
                .expect("snapshot_json should be present"),
        )
        .expect("snapshot_json should parse")
    }
}
