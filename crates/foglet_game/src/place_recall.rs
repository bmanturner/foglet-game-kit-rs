//! `place_recall` — discovered-place memory schema and touch/query primitives
//! (SPEC_v4 Task 6a, Task 6b, Task 6e).
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
//! existing migration pipeline.
//!
//! In a **space exploration** game, `place_recall` can retain the last
//! seen station-level fog map state for each crew member.
//! In a **dungeon crawler**, it can record each chamber a character has
//! entered for mini-map recoloration.

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
}

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

impl WorldDb {
    /// Record or refresh a player's recall observation of a place.
    ///
    /// The contract for v4 is idempotent upsert:
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
    use super::{row_to_place_recall, PlaceRecallRecord, PLACE_RECALL_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use std::thread::sleep;
    use std::time::Duration;
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

    /// Task 6b requires `touch_recall` to insert a fresh row on first
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

    /// Task 6c requires `first_seen_at` to stay fixed after the first
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

    /// Task 6d requires each touch call to refresh `snapshot_json`
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

    /// Task 6e requires `recall_for_player` to return records in
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
}
