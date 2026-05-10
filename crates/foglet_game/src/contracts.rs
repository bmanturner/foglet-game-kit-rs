//! `contracts` — durable contract lifecycle schema (SPEC_v5 Task 3a).
//!
//! This module defines the shared-world table shape for contract-style
//! opportunities. The schema stays intentionally genre-neutral:
//!
//! - In a **space exploration** game, a row can represent a freight
//!   contract offered by a station authority.
//! - In a **dungeon crawler**, a row can represent a guild commission
//!   to recover an artifact from a crypt.
//!
//! Later v5 tasks layer CRUD and lifecycle transitions on top of this
//! durable shape.

use thiserror::Error;

use crate::world_db::{WorldDb, WorldMigration};

/// One durable contract row in the shared world database.
///
/// This read model mirrors the `contracts` table one-for-one so game code
/// can reason about opportunities without touching raw SQL rows.
///
/// Genre-neutral by design:
///
/// - In a **space exploration** game, `kind = "freight"` might describe a
///   station-to-station cargo haul with a JSON objective payload.
/// - In a **dungeon crawler**, `kind = "recovery"` might describe a guild
///   commission to retrieve an artifact from a crypt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// Stable SQLite row id.
    pub id: i64,
    /// Optional game-defined stable key.
    pub key: Option<String>,
    /// Game-defined category label for the contract.
    pub kind: String,
    /// Game-defined owner bucket for the issuer.
    pub issuer_owner_kind: String,
    /// Game-defined issuer id inside `issuer_owner_kind`.
    pub issuer_owner_id: i64,
    /// Foglet player id that accepted the contract, or `None` while available.
    pub acceptor_player_id: Option<i64>,
    /// Lifecycle state persisted in SQLite.
    pub state: String,
    /// Opaque objective payload (typically JSON), round-tripped verbatim.
    pub objective_json: String,
    /// Opaque reward payload (typically JSON), round-tripped verbatim.
    pub reward_json: String,
    /// Optional opaque metadata payload.
    pub metadata_json: Option<String>,
    /// UTC creation timestamp emitted by SQLite (`CURRENT_TIMESTAMP`).
    pub created_at: String,
    /// UTC timestamp for when acceptance occurred, if accepted.
    pub accepted_at: Option<String>,
    /// UTC timestamp for when completion occurred, if completed.
    pub completed_at: Option<String>,
    /// Optional UTC expiry deadline.
    pub expires_at: Option<String>,
}

/// Lifecycle state vocabulary for [`Contract`] rows.
///
/// Keeping this vocabulary typed prevents stringly-typed mistakes in write
/// paths while preserving SQLite's text storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContractState {
    /// Awaiting acceptance by a player.
    Available,
    /// Accepted by a player and now in-progress.
    Accepted,
    /// Completed successfully.
    Completed,
    /// Failed by game-defined rules.
    Failed,
    /// Abandoned by the player.
    Abandoned,
    /// Expired by deadline sweep.
    Expired,
}

impl ContractState {
    /// Text encoding used in SQLite.
    pub fn as_str(self) -> &'static str {
        match self {
            ContractState::Available => "available",
            ContractState::Accepted => "accepted",
            ContractState::Completed => "completed",
            ContractState::Failed => "failed",
            ContractState::Abandoned => "abandoned",
            ContractState::Expired => "expired",
        }
    }
}

/// Errors for contract write/read helpers.
#[derive(Debug, Error)]
pub enum ContractError {
    /// SQLite failed during a contract mutation or readback.
    #[error("failed to persist contract row: {source}")]
    Sqlite {
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
}

/// Input payload for [`WorldDb::create_contract`].
///
/// Grouping parameters into one struct keeps call sites explicit and avoids
/// positional-argument mistakes when a game fills optional fields.
#[derive(Debug, Clone, Copy)]
pub struct CreateContractInput<'a> {
    /// Optional stable key controlled by the game.
    pub key: Option<&'a str>,
    /// Game-defined contract kind label.
    pub kind: &'a str,
    /// Game-defined owner bucket for the issuer (for example, `"station"` or `"guild"`).
    pub issuer_owner_kind: &'a str,
    /// Issuer id inside `issuer_owner_kind`.
    pub issuer_owner_id: i64,
    /// Opaque objective payload (typically JSON), persisted verbatim.
    pub objective_json: &'a str,
    /// Opaque reward payload (typically JSON), persisted verbatim.
    pub reward_json: &'a str,
    /// Optional opaque metadata payload.
    pub metadata_json: Option<&'a str>,
    /// Optional expiry deadline as UTC text.
    pub expires_at: Option<&'a str>,
}

/// Migration for the `contracts` table (SPEC_v5 Task 3a).
///
/// The schema captures one contract lifecycle row with optional
/// acceptance/completion timestamps and a state machine guard:
///
/// - `id` is the stable numeric row identity.
/// - `key` is an optional caller-defined stable identifier.
/// - `kind` and issuer owner fields are caller-defined taxonomy.
/// - `acceptor_player_id` is `NULL` until a player accepts.
/// - `state` is constrained to documented enum values.
/// - `objective_json`, `reward_json`, and `metadata_json` hold opaque
///   game-authored payloads.
/// - `created_at` is always present; other lifecycle timestamps are
///   nullable until each transition occurs.
///
/// This shape does not prescribe economics or story vocabulary. A
/// trading game can store delivery payloads while a fantasy game stores
/// escort objectives, both using the same table contract.
pub const CONTRACTS_MIGRATION: WorldMigration = WorldMigration {
    version: 16,
    name: "create_contracts",
    sql: "\
CREATE TABLE IF NOT EXISTS contracts (\n\
    id                  INTEGER PRIMARY KEY,\n\
    key                 TEXT UNIQUE,\n\
    kind                TEXT NOT NULL,\n\
    issuer_owner_kind   TEXT NOT NULL,\n\
    issuer_owner_id     INTEGER NOT NULL,\n\
    acceptor_player_id  INTEGER,\n\
    state               TEXT NOT NULL CHECK (state IN ('available', 'accepted', 'completed', 'failed', 'abandoned', 'expired')),\n\
    objective_json      TEXT NOT NULL,\n\
    reward_json         TEXT NOT NULL,\n\
    metadata_json       TEXT,\n\
    created_at          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,\n\
    accepted_at         TEXT,\n\
    completed_at        TEXT,\n\
    expires_at          TEXT\n\
);\n\
",
};

impl WorldDb {
    /// Create one contract in the initial `available` lifecycle state.
    ///
    /// The write path is intentionally explicit about the initial state: games
    /// cannot accidentally create rows already in `accepted` or terminal states.
    ///
    /// Genre-neutral usage:
    ///
    /// - A **space exploration** game can issue a `"freight"` contract from a
    ///   station owner to move cargo between sectors.
    /// - A **dungeon crawler** game can issue a `"recovery"` contract from a
    ///   guild owner to retrieve a relic from a room graph.
    ///
    /// `objective_json`, `reward_json`, and `metadata_json` are stored as
    /// opaque text and are not parsed or normalized by the kit.
    pub fn create_contract(
        &self,
        input: CreateContractInput<'_>,
    ) -> Result<Contract, ContractError> {
        const SQL: &str = "\
INSERT INTO contracts \
    (key, kind, issuer_owner_kind, issuer_owner_id, state, objective_json, reward_json, metadata_json, expires_at) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        self.connection()
            .query_row(
                SQL,
                rusqlite::params![
                    input.key,
                    input.kind,
                    input.issuer_owner_kind,
                    input.issuer_owner_id,
                    ContractState::Available.as_str(),
                    input.objective_json,
                    input.reward_json,
                    input.metadata_json,
                    input.expires_at,
                ],
                row_to_contract,
            )
            .map_err(|source| ContractError::Sqlite { source })
    }
}

/// Decode one `contracts` row in the column order used by this module.
fn row_to_contract(row: &rusqlite::Row<'_>) -> rusqlite::Result<Contract> {
    Ok(Contract {
        id: row.get(0)?,
        key: row.get(1)?,
        kind: row.get(2)?,
        issuer_owner_kind: row.get(3)?,
        issuer_owner_id: row.get(4)?,
        acceptor_player_id: row.get(5)?,
        state: row.get(6)?,
        objective_json: row.get(7)?,
        reward_json: row.get(8)?,
        metadata_json: row.get(9)?,
        created_at: row.get(10)?,
        accepted_at: row.get(11)?,
        completed_at: row.get(12)?,
        expires_at: row.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{ContractState, CreateContractInput, CONTRACTS_MIGRATION};
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use tempfile::tempdir;

    #[test]
    fn applies_contracts_migration_with_documented_columns() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let mut stmt = world
            .connection()
            .prepare(
                "SELECT name, \"notnull\", pk, type\nFROM pragma_table_info('contracts')\nORDER BY cid",
            )
            .expect("pragma_table_info preparable");
        let rows: Vec<(String, i64, i64, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .expect("query_map runs")
            .collect::<Result<_, _>>()
            .expect("rows decode");
        assert_eq!(
            rows,
            vec![
                ("id".to_string(), 0, 1, "INTEGER".to_string()),
                ("key".to_string(), 0, 0, "TEXT".to_string()),
                ("kind".to_string(), 1, 0, "TEXT".to_string()),
                ("issuer_owner_kind".to_string(), 1, 0, "TEXT".to_string()),
                ("issuer_owner_id".to_string(), 1, 0, "INTEGER".to_string()),
                (
                    "acceptor_player_id".to_string(),
                    0,
                    0,
                    "INTEGER".to_string()
                ),
                ("state".to_string(), 1, 0, "TEXT".to_string()),
                ("objective_json".to_string(), 1, 0, "TEXT".to_string()),
                ("reward_json".to_string(), 1, 0, "TEXT".to_string()),
                ("metadata_json".to_string(), 0, 0, "TEXT".to_string()),
                ("created_at".to_string(), 1, 0, "TEXT".to_string()),
                ("accepted_at".to_string(), 0, 0, "TEXT".to_string()),
                ("completed_at".to_string(), 0, 0, "TEXT".to_string()),
                ("expires_at".to_string(), 0, 0, "TEXT".to_string()),
            ],
            "contracts schema must match Task 3a contract"
        );

        let sql: String = world
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'contracts'",
                [],
                |row| row.get(0),
            )
            .expect("sqlite_master contains contracts create statement");
        assert!(
            sql.contains(
                "CHECK (state IN ('available', 'accepted', 'completed', 'failed', 'abandoned', 'expired'))"
            ),
            "state should be constrained to the documented lifecycle enum"
        );
    }

    #[test]
    fn contracts_migration_is_idempotent() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");

        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("first contracts migration applies");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("second contracts migration applies");

        let count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM world_migrations WHERE name = ?1",
                params![CONTRACTS_MIGRATION.name],
                |row| row.get(0),
            )
            .expect("world_migrations query succeeds");
        assert_eq!(count, 1, "contracts migration record should be idempotent");
    }

    #[test]
    fn create_contract_returns_available_row() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("relay-run-alpha"),
                kind: "freight",
                issuer_owner_kind: "station",
                issuer_owner_id: 42,
                objective_json: r#"{"route":"sector-7","cargo":"medical-crates"}"#,
                reward_json: r#"{"credits":1200,"rep":{"harbor-guild":2}}"#,
                metadata_json: Some(r#"{"priority":"urgent"}"#),
                expires_at: Some("2031-05-10T12:34:56Z"),
            })
            .expect("contract creation succeeds");

        assert!(created.id > 0, "sqlite should assign a concrete row id");
        assert_eq!(created.key.as_deref(), Some("relay-run-alpha"));
        assert_eq!(created.kind, "freight");
        assert_eq!(created.issuer_owner_kind, "station");
        assert_eq!(created.issuer_owner_id, 42);
        assert_eq!(created.state, ContractState::Available.as_str());
        assert_eq!(created.acceptor_player_id, None);
        assert_eq!(created.accepted_at, None);
        assert_eq!(created.completed_at, None);
        assert_eq!(created.expires_at.as_deref(), Some("2031-05-10T12:34:56Z"));
        assert!(
            !created.created_at.is_empty(),
            "sqlite should populate created_at"
        );
    }

    #[test]
    fn create_contract_preserves_objective_and_reward_json_byte_for_byte() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let objective_json = "{\n  \"room\": \"crypt-7\",\n  \"clues\": [\"bone key\", \"sigil\\ntrace\"],\n  \"notes\": \"recover \\\"before dawn\\\"\"\n}";
        let reward_json = "{\n  \"items\": [\"moon-vial\", \"rusted token\"],\n  \"favor\": {\"guild\": 3},\n  \"credits\": 90\n}";

        let created = world
            .create_contract(CreateContractInput {
                key: Some("crypt-recovery"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 8,
                objective_json,
                reward_json,
                metadata_json: None,
                expires_at: None,
            })
            .expect("contract creation succeeds");

        assert_eq!(created.objective_json, objective_json);
        assert_eq!(created.reward_json, reward_json);

        let (stored_objective, stored_reward): (String, String) = world
            .connection()
            .query_row(
                "SELECT objective_json, reward_json FROM contracts WHERE id = ?1",
                rusqlite::params![created.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("direct sql read returns contract payloads");
        assert_eq!(stored_objective, objective_json);
        assert_eq!(stored_reward, reward_json);
    }
}
