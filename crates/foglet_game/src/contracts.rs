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
    /// SQLite failed during a contract write or read helper.
    #[error("failed to query contract row: {source}")]
    Sqlite {
        /// Underlying SQL failure.
        #[source]
        source: rusqlite::Error,
    },
}

/// Optional filters for [`WorldDb::available_contracts`].
///
/// `available_contracts` always scopes to `state = "available"`. This
/// struct narrows that set further without introducing game-specific
/// vocabulary:
///
/// - In a **space exploration** game, `kind = Some("delivery")` can list
///   only freight jobs currently open at a station.
/// - In a **dungeon crawler**, `issuer_owner_kind = Some("guild")` can
///   list only open guild commissions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvailableContractsFilter {
    /// Optional contract kind match (exact string equality).
    pub kind: Option<String>,
    /// Optional issuer owner-kind match.
    pub issuer_owner_kind: Option<String>,
    /// Optional issuer owner-id match.
    pub issuer_owner_id: Option<i64>,
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

    /// Load a single contract by numeric id.
    ///
    /// Returns `Ok(None)` when the id is unknown. This keeps callers
    /// explicit about "not found" as a normal state while still surfacing
    /// SQL failures as typed errors.
    ///
    /// Genre-neutral examples:
    ///
    /// - In a **space exploration** game, open a delivery detail view for
    ///   a selected board row id.
    /// - In a **dungeon crawler**, resolve a guild mission id referenced by
    ///   a room interaction script.
    pub fn contract_by_id(&self, contract_id: i64) -> Result<Option<Contract>, ContractError> {
        const SQL: &str = "\
SELECT id, key, kind, issuer_owner_kind, issuer_owner_id, \
       acceptor_player_id, state, objective_json, reward_json, metadata_json, \
       created_at, accepted_at, completed_at, expires_at\n\
FROM contracts\n\
WHERE id = ?1";

        match self
            .connection()
            .query_row(SQL, rusqlite::params![contract_id], row_to_contract)
        {
            Ok(contract) => Ok(Some(contract)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(source) => Err(ContractError::Sqlite { source }),
        }
    }

    /// List all currently available contracts in deterministic order.
    ///
    /// The helper always scopes rows to `state = "available"` and then
    /// applies optional filter fields from [`AvailableContractsFilter`].
    ///
    /// Deterministic ordering:
    ///
    /// - Primary sort: `created_at` ascending.
    /// - Secondary sort: `id` ascending.
    ///
    /// The `id` tiebreaker avoids timestamp-collision nondeterminism when
    /// multiple contracts are inserted within the same second.
    pub fn available_contracts(
        &self,
        filter: &AvailableContractsFilter,
    ) -> Result<Vec<Contract>, ContractError> {
        const SQL: &str = "\
SELECT id, key, kind, issuer_owner_kind, issuer_owner_id, \
       acceptor_player_id, state, objective_json, reward_json, metadata_json, \
       created_at, accepted_at, completed_at, expires_at\n\
FROM contracts\n\
WHERE state = ?1\n\
  AND (?2 IS NULL OR kind = ?2)\n\
  AND (?3 IS NULL OR issuer_owner_kind = ?3)\n\
  AND (?4 IS NULL OR issuer_owner_id = ?4)\n\
ORDER BY created_at ASC, id ASC";

        let mut statement = self
            .connection()
            .prepare(SQL)
            .map_err(|source| ContractError::Sqlite { source })?;

        let rows = statement
            .query_map(
                rusqlite::params![
                    ContractState::Available.as_str(),
                    filter.kind.as_deref(),
                    filter.issuer_owner_kind.as_deref(),
                    filter.issuer_owner_id,
                ],
                row_to_contract,
            )
            .map_err(|source| ContractError::Sqlite { source })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(rows)
    }

    /// List contracts accepted by one player, optionally narrowed by state.
    ///
    /// The helper intentionally keys by Foglet `player_id` so games can show
    /// one player's active/finished commitments without joining against any
    /// game-specific profile table.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, list all contracts accepted by the
    ///   current pilot to render a captain's ledger.
    /// - In a **dungeon crawler**, list all commissions accepted by the
    ///   current adventurer before entering a guild hall.
    ///
    /// Deterministic ordering:
    ///
    /// - Primary sort: `created_at` ascending.
    /// - Secondary sort: `id` ascending.
    pub fn contracts_for_acceptor(
        &self,
        player_id: i64,
        state: Option<ContractState>,
    ) -> Result<Vec<Contract>, ContractError> {
        const SQL: &str = "\
SELECT id, key, kind, issuer_owner_kind, issuer_owner_id, \
       acceptor_player_id, state, objective_json, reward_json, metadata_json, \
       created_at, accepted_at, completed_at, expires_at\n\
FROM contracts\n\
WHERE acceptor_player_id = ?1\n\
  AND (?2 IS NULL OR state = ?2)\n\
ORDER BY created_at ASC, id ASC";

        let mut statement = self
            .connection()
            .prepare(SQL)
            .map_err(|source| ContractError::Sqlite { source })?;

        let rows = statement
            .query_map(
                rusqlite::params![player_id, state.map(ContractState::as_str)],
                row_to_contract,
            )
            .map_err(|source| ContractError::Sqlite { source })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(rows)
    }

    /// List contracts issued by one owner bucket and owner id.
    ///
    /// Issuer indexing stays generic so the same API works for station boards,
    /// tavern guild boards, town councils, or any other game-defined issuer.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, query all contracts emitted by a
    ///   specific station authority.
    /// - In a **dungeon crawler**, query all contracts emitted by a specific
    ///   adventurers' guild.
    ///
    /// Deterministic ordering:
    ///
    /// - Primary sort: `created_at` ascending.
    /// - Secondary sort: `id` ascending.
    pub fn contracts_by_issuer(
        &self,
        owner_kind: &str,
        owner_id: i64,
        state: Option<ContractState>,
    ) -> Result<Vec<Contract>, ContractError> {
        const SQL: &str = "\
SELECT id, key, kind, issuer_owner_kind, issuer_owner_id, \
       acceptor_player_id, state, objective_json, reward_json, metadata_json, \
       created_at, accepted_at, completed_at, expires_at\n\
FROM contracts\n\
WHERE issuer_owner_kind = ?1\n\
  AND issuer_owner_id = ?2\n\
  AND (?3 IS NULL OR state = ?3)\n\
ORDER BY created_at ASC, id ASC";

        let mut statement = self
            .connection()
            .prepare(SQL)
            .map_err(|source| ContractError::Sqlite { source })?;

        let rows = statement
            .query_map(
                rusqlite::params![owner_kind, owner_id, state.map(ContractState::as_str)],
                row_to_contract,
            )
            .map_err(|source| ContractError::Sqlite { source })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(rows)
    }

    /// Accept one contract for a player inside a single SQLite transaction.
    ///
    /// This is the Task 4a lifecycle transition primitive. It updates
    /// one contract row to `accepted`, records `acceptor_player_id`, and
    /// stamps `accepted_at = CURRENT_TIMESTAMP` atomically.
    ///
    /// The optional `on_commit` callback runs after the SQL mutation while
    /// still inside the active transaction. If the callback returns `Err`,
    /// the transaction rolls back and no acceptance persists.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, accepting a freight offer can
    ///   reserve station cargo in the same transaction.
    /// - In a **dungeon crawler**, accepting a guild commission can append
    ///   a quest-log row in the same transaction.
    pub fn accept_contract<F>(
        &mut self,
        contract_id: i64,
        player_id: i64,
        on_commit: Option<F>,
    ) -> Result<Contract, ContractError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &Contract) -> Result<(), rusqlite::Error>,
    {
        const SQL: &str = "\
UPDATE contracts\n\
SET state = ?2,\n\
    acceptor_player_id = ?3,\n\
    accepted_at = CURRENT_TIMESTAMP\n\
WHERE id = ?1\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let accepted = tx
            .query_row(
                SQL,
                rusqlite::params![contract_id, ContractState::Accepted.as_str(), player_id],
                row_to_contract,
            )
            .map_err(|source| ContractError::Sqlite { source })?;

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &accepted).map_err(|source| ContractError::Sqlite { source })?;
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(accepted)
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
    use super::{
        AvailableContractsFilter, ContractState, CreateContractInput, CONTRACTS_MIGRATION,
    };
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use std::thread::sleep;
    use std::time::Duration;
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

    #[test]
    fn contract_by_id_returns_row_when_present_and_none_when_missing() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("guild-relic-001"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 17,
                objective_json: r#"{"target":"obsidian-idol"}"#,
                reward_json: r#"{"favor":{"guild":5}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("contract creation succeeds");

        let loaded = world
            .contract_by_id(created.id)
            .expect("lookup by id succeeds")
            .expect("inserted id should exist");
        assert_eq!(loaded, created);

        let missing = world
            .contract_by_id(created.id + 999)
            .expect("query succeeds");
        assert_eq!(missing, None);
    }

    #[test]
    fn available_contracts_orders_deterministically_and_filters_fields() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let first = world
            .create_contract(CreateContractInput {
                key: Some("station-delivery"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 10,
                objective_json: r#"{"to":"sector-1"}"#,
                reward_json: r#"{"credits":180}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("first insert succeeds");

        sleep(Duration::from_millis(1_100));

        let second = world
            .create_contract(CreateContractInput {
                key: Some("guild-clearance"),
                kind: "clearance",
                issuer_owner_kind: "guild",
                issuer_owner_id: 77,
                objective_json: r#"{"room":"catacomb-west"}"#,
                reward_json: r#"{"favor":{"guild":2}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("second insert succeeds");

        // Ensure non-available rows are excluded from the "available" list.
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1 WHERE id = ?2",
                rusqlite::params![ContractState::Accepted.as_str(), second.id],
            )
            .expect("state update succeeds");

        let third = world
            .create_contract(CreateContractInput {
                key: Some("guild-recovery"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 77,
                objective_json: r#"{"target":"sun-seal"}"#,
                reward_json: r#"{"items":["moon-key"]}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("third insert succeeds");

        let all_available = world
            .available_contracts(&AvailableContractsFilter::default())
            .expect("available list query succeeds");
        assert_eq!(
            all_available
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![first.id, third.id],
            "stable ordering should follow created_at ASC with id tie-breaks"
        );

        let guild_only = world
            .available_contracts(&AvailableContractsFilter {
                kind: None,
                issuer_owner_kind: Some("guild".to_string()),
                issuer_owner_id: None,
            })
            .expect("issuer-kind filter query succeeds");
        assert_eq!(
            guild_only
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![third.id]
        );

        let recovery_only = world
            .available_contracts(&AvailableContractsFilter {
                kind: Some("recovery".to_string()),
                issuer_owner_kind: None,
                issuer_owner_id: None,
            })
            .expect("kind filter query succeeds");
        assert_eq!(
            recovery_only
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![third.id]
        );

        let station_issuer = world
            .available_contracts(&AvailableContractsFilter {
                kind: None,
                issuer_owner_kind: None,
                issuer_owner_id: Some(10),
            })
            .expect("issuer-id filter query succeeds");
        assert_eq!(
            station_issuer
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![first.id]
        );
    }

    #[test]
    fn contracts_for_acceptor_and_contracts_by_issuer_filter_results() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let station_open = world
            .create_contract(CreateContractInput {
                key: Some("station-open"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 1,
                objective_json: r#"{"to":"ring-a"}"#,
                reward_json: r#"{"credits":25}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("station open insert succeeds");
        let station_accepted_alice = world
            .create_contract(CreateContractInput {
                key: Some("station-accepted-alice"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 1,
                objective_json: r#"{"to":"ring-b"}"#,
                reward_json: r#"{"credits":55}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("station accepted/alice insert succeeds");
        let guild_accepted_alice = world
            .create_contract(CreateContractInput {
                key: Some("guild-accepted-alice"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 2,
                objective_json: r#"{"room":"ossuary"}"#,
                reward_json: r#"{"favor":{"guild":2}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("guild accepted/alice insert succeeds");
        let station_completed_alice = world
            .create_contract(CreateContractInput {
                key: Some("station-completed-alice"),
                kind: "escort",
                issuer_owner_kind: "station",
                issuer_owner_id: 1,
                objective_json: r#"{"to":"dock-c"}"#,
                reward_json: r#"{"credits":80}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("station completed/alice insert succeeds");
        let station_accepted_bob = world
            .create_contract(CreateContractInput {
                key: Some("station-accepted-bob"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 1,
                objective_json: r#"{"to":"ring-d"}"#,
                reward_json: r#"{"credits":40}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("station accepted/bob insert succeeds");

        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Accepted.as_str(),
                    100_i64,
                    station_accepted_alice.id
                ],
            )
            .expect("station accepted/alice row update succeeds");
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Accepted.as_str(),
                    100_i64,
                    guild_accepted_alice.id
                ],
            )
            .expect("guild accepted/alice row update succeeds");
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP, completed_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Completed.as_str(),
                    100_i64,
                    station_completed_alice.id
                ],
            )
            .expect("station completed/alice row update succeeds");
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Accepted.as_str(),
                    200_i64,
                    station_accepted_bob.id
                ],
            )
            .expect("station accepted/bob row update succeeds");

        let alice_all = world
            .contracts_for_acceptor(100, None)
            .expect("acceptor query succeeds");
        assert_eq!(
            alice_all
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![
                station_accepted_alice.id,
                guild_accepted_alice.id,
                station_completed_alice.id
            ],
            "acceptor filter should include only rows accepted by that player"
        );

        let alice_accepted = world
            .contracts_for_acceptor(100, Some(ContractState::Accepted))
            .expect("acceptor+state query succeeds");
        assert_eq!(
            alice_accepted
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![station_accepted_alice.id, guild_accepted_alice.id]
        );

        let station_all_states = world
            .contracts_by_issuer("station", 1, None)
            .expect("issuer query succeeds");
        assert_eq!(
            station_all_states
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![
                station_open.id,
                station_accepted_alice.id,
                station_completed_alice.id,
                station_accepted_bob.id
            ],
            "issuer filter should include all states when no state filter is provided"
        );

        let station_only_accepted = world
            .contracts_by_issuer("station", 1, Some(ContractState::Accepted))
            .expect("issuer+state query succeeds");
        assert_eq!(
            station_only_accepted
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![station_accepted_alice.id, station_accepted_bob.id]
        );

        let guild_only_accepted = world
            .contracts_by_issuer("guild", 2, Some(ContractState::Accepted))
            .expect("guild issuer query succeeds");
        assert_eq!(
            guild_only_accepted
                .iter()
                .map(|contract| contract.id)
                .collect::<Vec<_>>(),
            vec![guild_accepted_alice.id]
        );
    }

    #[test]
    fn accept_contract_sets_state_acceptor_and_accepted_timestamp() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("harbor-run"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 9,
                objective_json: r#"{"to":"sector-19"}"#,
                reward_json: r#"{"credits":300}"#,
                metadata_json: Some(r#"{"tier":"starter"}"#),
                expires_at: None,
            })
            .expect("create_contract succeeds");

        let accepted = world
            .accept_contract(
                created.id,
                77,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("accept_contract succeeds");

        assert_eq!(accepted.id, created.id);
        assert_eq!(accepted.state, ContractState::Accepted.as_str());
        assert_eq!(accepted.acceptor_player_id, Some(77));
        assert!(
            accepted.accepted_at.is_some(),
            "accept_contract should stamp accepted_at"
        );
        assert_eq!(
            accepted.completed_at, None,
            "accepting should not set completed_at"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("accepted row exists");
        assert_eq!(persisted.state, ContractState::Accepted.as_str());
        assert_eq!(persisted.acceptor_player_id, Some(77));
        assert_eq!(persisted.accepted_at, accepted.accepted_at);
    }
}
