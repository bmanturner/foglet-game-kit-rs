//! `presence` — current-location tracking schema (SPEC_v4 Tasks 5a–5c).
//!
//! This module owns the durable schema for per-player current place.
//! Movement and transactional APIs are now available for atomic movement
//! transitions with a callback hook.
//!
//! In a **space exploration** game, a row here tracks that a captain is
//! currently at a specific docking node. In a **dungeon crawler**, it
//! tracks which chamber or corridor the player currently occupies. The
//! table intentionally stays minimal so every genre authorship decision
//! remains outside the kit.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

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

/// Errors for presence write operations.
///
/// The error surface is intentionally explicit so caller code can
/// preserve player-facing diagnostics for failures while still surfacing
/// low-level database details through `#[source]`.
#[derive(Debug, Error)]
pub enum PresenceError {
    /// A presence SQL statement failed (constraint, lock timeout, invalid
    /// foreign key, or corruption).
    #[error("failed to set presence for player `{player_id}`: {source}")]
    SetFailed {
        /// Player identifier supplied to `set_presence`.
        player_id: i64,
        /// Underlying `rusqlite` error with the true SQLite cause.
        #[source]
        source: rusqlite::Error,
    },
    /// A move was requested for a player that has no existing row.
    #[error("player `{player_id}` has no presence row; call `set_presence` before moving")]
    MissingPresence {
        /// Player identifier supplied to `move_player`.
        player_id: i64,
    },
    /// SQL or transaction failure while moving the player.
    #[error("failed to move player `{player_id}` to place `{place_id}`: {source}")]
    MoveFailed {
        /// Player identifier supplied to `move_player`.
        player_id: i64,
        /// Destination place supplied to `move_player`.
        place_id: i64,
        /// Underlying `rusqlite` error from the update or commit.
        #[source]
        source: rusqlite::Error,
    },
    /// A movement callback rejected or failed.
    #[error(
        "movement callback rejected move of player `{player_id}` to place `{place_id}`: {source}"
    )]
    MoveRejected {
        /// Player identifier supplied to `move_player`.
        player_id: i64,
        /// Destination place supplied to `move_player`.
        place_id: i64,
        /// Underlying callback error surfaced through the `rusqlite::Error` channel.
        #[source]
        source: rusqlite::Error,
    },
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

impl WorldDb {
    /// Create or replace a presence row for a player.
    ///
    /// This is the v4 Task 5b "initial-placement" API:
    ///
    /// - It writes one presence row for a player if absent.
    /// - It refreshes `entered_at` whenever called again, which lets
    ///   callers reuse one helper in both bootstrap and explicit
    ///   re-placement flows.
    ///
    /// Why this method uses UPSERT semantics:
    ///
    /// - It keeps the primitive focused on "set current presence"
    ///   instead of forcing call sites to choose between two write
    ///   paths.
    /// - `place_id` can be reset if a game intentionally remaps a
    ///   player between rooms before movement rules begin (for example,
    ///   moving from a loading room to a dock in a **space exploration**
    ///   flow or from an entry hall to a vault in a **dungeon crawler**
    ///   flow).
    ///
    /// Transactionality note for later tasks:
    ///
    /// This method intentionally writes a single row with `INSERT …
    /// ON CONFLICT ... DO UPDATE` and returns the committed row via
    /// `RETURNING`. A future movement wrapper (`move_player`) should
    /// use an explicit `Connection::transaction` to keep callbacks and
    /// presence updates atomic.
    pub fn set_presence(
        &self,
        player_id: i64,
        place_id: i64,
        metadata_json: Option<&str>,
    ) -> Result<PresenceRecord, PresenceError> {
        const SQL: &str = "\
INSERT INTO presence (player_id, place_id, metadata_json, entered_at)\n\
VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)\n\
ON CONFLICT(player_id) DO UPDATE\n\
SET place_id = excluded.place_id,\n\
    metadata_json = excluded.metadata_json,\n\
    entered_at = CURRENT_TIMESTAMP\n\
RETURNING player_id, place_id, entered_at, metadata_json";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![player_id, place_id, metadata_json],
                row_to_presence,
            )
            .map_err(|source| PresenceError::SetFailed { player_id, source })
    }

    /// Move a player to a new place inside one SQLite transaction.
    ///
    /// This is the v4 Task 5c `move_player` primitive. `move_player`
    /// always updates one presence row and passes the post-move snapshot to
    /// `on_commit` so game logic can run additional checks inside the same
    /// transaction.
    ///
    /// - The callback receives the active [`rusqlite::Transaction`]
    ///   plus the canonical post-move presence row.
    /// - If the callback returns `Err`, the `UPDATE` rolls back and this
    ///   method returns [`PresenceError::MoveRejected`].
    /// - The caller gets the committed presence snapshot on success.
    ///
    /// This shape stays genre-neutral:
    ///
    /// - In a **space exploration** game, a callback can append a
    ///   transfer-log row or enforce docking restrictions before commit.
    /// - In a **dungeon crawler** game, the same callback can debit
    ///   movement stamina or log discovered chambers while keeping the
    ///   update atomic.
    pub fn move_player<F>(
        &mut self,
        player_id: i64,
        dest_place_id: i64,
        on_commit: F,
    ) -> Result<PresenceRecord, PresenceError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &PresenceRecord) -> Result<(), rusqlite::Error>,
    {
        const SQL: &str = "\
UPDATE presence\n\
SET place_id = ?2,\n\
    entered_at = CURRENT_TIMESTAMP\n\
WHERE player_id = ?1\n\
RETURNING player_id, place_id, entered_at, metadata_json";

        let tx =
            self.connection_mut()
                .transaction()
                .map_err(|source| PresenceError::MoveFailed {
                    player_id,
                    place_id: dest_place_id,
                    source,
                })?;

        let moved = match tx.query_row(
            SQL,
            rusqlite::params![player_id, dest_place_id],
            row_to_presence,
        ) {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(PresenceError::MissingPresence { player_id });
            }
            Err(source) => {
                return Err(PresenceError::MoveFailed {
                    player_id,
                    place_id: dest_place_id,
                    source,
                });
            }
        };

        on_commit(&tx, &moved).map_err(|source| PresenceError::MoveRejected {
            player_id,
            place_id: dest_place_id,
            source,
        })?;

        tx.commit().map_err(|source| PresenceError::MoveFailed {
            player_id,
            place_id: dest_place_id,
            source,
        })?;

        Ok(moved)
    }
}

fn row_to_presence(row: &rusqlite::Row<'_>) -> rusqlite::Result<PresenceRecord> {
    Ok(PresenceRecord {
        player_id: row.get(0)?,
        place_id: row.get(1)?,
        entered_at: row.get(2)?,
        metadata_json: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{row_to_presence, PresenceError, PRESENCE_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use std::thread::sleep;
    use std::time::Duration;
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

    /// SPEC_v4 Task 5b requires a timestamped placement row for the
    /// first call to `set_presence`.
    ///
    /// A valid player and place are required because both foreign-key
    /// constraints exist on `presence`, so the test seeds fixtures
    /// from the adjacent module migrations and then asserts the returned
    /// row is stable and persists in the database.
    #[test]
    fn set_presence_inserts_row_with_timestamp() -> Result<(), PresenceError> {
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
            .apply_migration(&PRESENCE_MIGRATION)
            .expect("presence migration applies");

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Captain Quill"],
                |row| row.get(0),
            )
            .expect("fixture player insert works");

        let docking_gate = world
            .insert_place("port-alpha", "Port Alpha", "harbor", None)
            .expect("fixture place insert works");

        let placed = world.set_presence(player_id, docking_gate.id, Some(r#"{"zone":"entry"}"#))?;

        let loaded = world
            .connection()
            .query_row(
                "SELECT player_id, place_id, entered_at, metadata_json FROM presence WHERE player_id = ?1",
                rusqlite::params![player_id],
                row_to_presence,
            )
            .expect("presence row is queryable");

        assert_eq!(placed.player_id, player_id);
        assert_eq!(placed.place_id, docking_gate.id);
        assert_eq!(
            placed.metadata_json,
            Some(r#"{"zone":"entry"}"#.to_string())
        );
        assert!(
            !placed.entered_at.is_empty(),
            "placement must persist a non-empty timestamp"
        );
        assert_eq!(placed, loaded);

        Ok(())
    }

    /// SPEC_v4 Task 5c requires `move_player` to update the player's
    /// current location atomically and refresh `entered_at`.
    ///
    /// This test also proves the callback receives the committed
    /// destination row by asserting destination place and metadata inside
    /// the callback.
    #[test]
    fn move_player_updates_presence_and_entered_at() -> Result<(), PresenceError> {
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
            .apply_migration(&PRESENCE_MIGRATION)
            .expect("presence migration applies");

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Captain Quill"],
                |row| row.get(0),
            )
            .expect("fixture player insert works");

        let docking_gate = world
            .insert_place("port-alpha", "Port Alpha", "harbor", None)
            .expect("fixture place insert works");
        let market_square = world
            .insert_place("cargo-hub", "Cargo Hub", "dock", None)
            .expect("fixture place insert works");

        let initial =
            world.set_presence(player_id, docking_gate.id, Some(r#"{"zone":"entry"}"#))?;
        sleep(Duration::from_secs(1));

        let moved = world.move_player(player_id, market_square.id, |_, row| {
            assert_eq!(row.player_id, player_id);
            assert_eq!(row.place_id, market_square.id);
            assert_eq!(row.metadata_json, Some(r#"{"zone":"entry"}"#.to_string()));
            Ok(())
        })?;

        let loaded = world
            .connection()
            .query_row(
                "SELECT player_id, place_id, entered_at, metadata_json FROM presence WHERE player_id = ?1",
                rusqlite::params![player_id],
                row_to_presence,
            )
            .expect("presence row is queryable");

        assert_eq!(moved, loaded);
        assert_eq!(moved.player_id, player_id);
        assert_eq!(moved.place_id, market_square.id);
        assert_eq!(moved.metadata_json, Some(r#"{"zone":"entry"}"#.to_string()));
        assert_ne!(
            moved.entered_at, initial.entered_at,
            "move should refresh entered_at via CURRENT_TIMESTAMP"
        );

        Ok(())
    }

    /// A movement callback rejection must roll back the write, because
    /// `move_player` is defined as an atomic transaction that includes
    /// both the row mutation and the callback side effects.
    ///
    /// This proof is genre-neutral:
    ///
    /// - In a **space exploration** game, a rejected docking authorization
    ///   callback must not teleport a captain even temporarily.
    /// - In a **dungeon crawler** game, a failed "trap check" should not
    ///   alter the player's chamber just because the callback failed.
    #[test]
    fn move_player_rolls_back_when_callback_rejects() {
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
            .apply_migration(&PRESENCE_MIGRATION)
            .expect("presence migration applies");

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Captain Quill"],
                |row| row.get(0),
            )
            .expect("fixture player insert works");

        let docking_gate = world
            .insert_place("port-alpha", "Port Alpha", "harbor", None)
            .expect("fixture place insert works");
        let market_square = world
            .insert_place("cargo-hub", "Cargo Hub", "dock", None)
            .expect("fixture place insert works");

        let initial = world
            .set_presence(player_id, docking_gate.id, Some(r#"{"zone":"entry"}"#))
            .expect("initial presence row exists");
        sleep(Duration::from_secs(1));

        let reject_result = world.move_player(player_id, market_square.id, |_, _| {
            Err(rusqlite::Error::InvalidQuery)
        });
        match reject_result {
            Err(PresenceError::MoveRejected {
                player_id: row_id, ..
            }) => {
                assert_eq!(row_id, player_id);
            }
            _ => panic!("expected a move-reject error"),
        }

        let loaded = world
            .connection()
            .query_row(
                "SELECT player_id, place_id, entered_at, metadata_json FROM presence WHERE player_id = ?1",
                rusqlite::params![player_id],
                row_to_presence,
            )
            .expect("presence row is queryable");

        assert_eq!(loaded.player_id, player_id);
        assert_eq!(loaded.place_id, docking_gate.id);
        assert_eq!(
            loaded.metadata_json,
            Some(r#"{"zone":"entry"}"#.to_string())
        );
        assert_eq!(loaded.entered_at, initial.entered_at);
    }
}
