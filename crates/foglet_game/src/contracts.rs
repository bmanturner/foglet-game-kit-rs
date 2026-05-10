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
    /// No `contracts` row exists with the given id.
    ///
    /// Surfaced by lifecycle transitions when a caller targets an id that
    /// no longer exists in the shared world database.
    #[error("contract {id} does not exist")]
    NotFound {
        /// Caller-supplied id that could not be found.
        id: i64,
    },
    /// A lifecycle transition was attempted from an incompatible source state.
    ///
    /// Carrying both the observed `from` state and desired `to` state lets
    /// game UIs explain the failure precisely.
    #[error("cannot transition contract {id} from {from:?} to {to:?}")]
    InvalidTransition {
        /// Contract id the caller attempted to mutate.
        id: i64,
        /// Current persisted state read from SQLite.
        from: String,
        /// Desired state the helper attempted to set.
        to: ContractState,
    },
    /// `accept_contract` failed because another player already accepted it.
    ///
    /// This dedicated variant separates "already held" from the broader
    /// `InvalidTransition` bucket so games can render the right UX copy.
    #[error("contract {id} is already accepted by player {acceptor_player_id}")]
    AlreadyAccepted {
        /// Contract id the caller attempted to accept.
        id: i64,
        /// Existing acceptor recorded on the row.
        acceptor_player_id: i64,
    },
    /// `accept_contract` failed because the row's deadline already lapsed.
    ///
    /// This remains separate from `InvalidTransition` so games can present a
    /// clear "expired before accept" message.
    #[error("contract {id} expired before it could be accepted")]
    ExpiredOnAccept {
        /// Contract id the caller attempted to accept.
        id: i64,
    },
    /// A game callback rejected a transition while the SQL transaction was open.
    ///
    /// The helper always rolls back in this case; this variant carries both
    /// the attempted lifecycle edge and the callback error source.
    #[error("on_commit rejected transition for contract {id} to {attempted:?}: {source}")]
    OnCommitRejected {
        /// Contract id being transitioned when the callback failed.
        id: i64,
        /// Target state attempted by the helper.
        attempted: ContractState,
        /// Underlying callback error.
        #[source]
        source: rusqlite::Error,
    },
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
    /// Contracts whose `expires_at` deadline is at-or-before "now" are not
    /// accepted by this helper.
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
  AND acceptor_player_id IS NULL\n\
  AND (expires_at IS NULL OR datetime(expires_at) > CURRENT_TIMESTAMP)\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let accepted = match tx.query_row(
            SQL,
            rusqlite::params![contract_id, ContractState::Accepted.as_str(), player_id],
            row_to_contract,
        ) {
            Ok(contract) => contract,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::diagnose_failed_accept_contract(&tx, contract_id));
            }
            Err(source) => return Err(ContractError::Sqlite { source }),
        };

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &accepted).map_err(|source| ContractError::OnCommitRejected {
                id: contract_id,
                attempted: ContractState::Accepted,
                source,
            })?;
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(accepted)
    }

    /// Complete one accepted contract inside a single SQLite transaction.
    ///
    /// This is the Task 4e lifecycle transition primitive. It updates one
    /// contract row from `accepted` to `completed` and stamps
    /// `completed_at = CURRENT_TIMESTAMP` atomically.
    ///
    /// Contracts not currently in the `accepted` state are rejected so games
    /// cannot accidentally complete rows still available (or already terminal).
    ///
    /// The optional `on_commit` callback runs after the SQL mutation while
    /// still inside the active transaction. If the callback returns `Err`,
    /// the transaction rolls back and no completion persists.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, completing a freight contract can
    ///   pay out station credits in the same transaction.
    /// - In a **dungeon crawler**, completing a recovery commission can
    ///   grant guild standing in the same transaction.
    pub fn complete_contract<F>(
        &mut self,
        contract_id: i64,
        on_commit: Option<F>,
    ) -> Result<Contract, ContractError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &Contract) -> Result<(), rusqlite::Error>,
    {
        const SQL: &str = "\
UPDATE contracts\n\
SET state = ?2,\n\
    completed_at = CURRENT_TIMESTAMP\n\
WHERE id = ?1\n\
  AND state = ?3\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let completed = match tx.query_row(
            SQL,
            rusqlite::params![
                contract_id,
                ContractState::Completed.as_str(),
                ContractState::Accepted.as_str()
            ],
            row_to_contract,
        ) {
            Ok(contract) => contract,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::diagnose_failed_state_transition(
                    &tx,
                    contract_id,
                    ContractState::Completed,
                ));
            }
            Err(source) => return Err(ContractError::Sqlite { source }),
        };

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &completed).map_err(|source| ContractError::OnCommitRejected {
                id: contract_id,
                attempted: ContractState::Completed,
                source,
            })?;
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(completed)
    }

    /// Mark one accepted contract as failed inside a single SQLite transaction.
    ///
    /// This is the Task 4f lifecycle transition primitive. It updates one
    /// contract row from `accepted` to `failed` atomically.
    ///
    /// Contracts not currently in the `accepted` state are rejected so games
    /// cannot accidentally fail rows still available (or already terminal).
    ///
    /// The optional `on_commit` callback runs after the SQL mutation while
    /// still inside the active transaction. If the callback returns `Err`,
    /// the transaction rolls back and no failure persists.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, failing a freight contract can
    ///   revoke a station permit in the same transaction.
    /// - In a **dungeon crawler**, failing a guild commission can lower
    ///   faction standing in the same transaction.
    pub fn fail_contract<F>(
        &mut self,
        contract_id: i64,
        on_commit: Option<F>,
    ) -> Result<Contract, ContractError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &Contract) -> Result<(), rusqlite::Error>,
    {
        const SQL: &str = "\
UPDATE contracts\n\
SET state = ?2\n\
WHERE id = ?1\n\
  AND state = ?3\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let failed = match tx.query_row(
            SQL,
            rusqlite::params![
                contract_id,
                ContractState::Failed.as_str(),
                ContractState::Accepted.as_str()
            ],
            row_to_contract,
        ) {
            Ok(contract) => contract,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::diagnose_failed_state_transition(
                    &tx,
                    contract_id,
                    ContractState::Failed,
                ));
            }
            Err(source) => return Err(ContractError::Sqlite { source }),
        };

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &failed).map_err(|source| ContractError::OnCommitRejected {
                id: contract_id,
                attempted: ContractState::Failed,
                source,
            })?;
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(failed)
    }

    /// Mark one accepted contract as abandoned inside a single SQLite transaction.
    ///
    /// This is the Task 4f lifecycle transition primitive. It updates one
    /// contract row from `accepted` to `abandoned` atomically and clears
    /// `acceptor_player_id` so later analytics can distinguish abandoned rows
    /// from actively held commitments.
    ///
    /// Contracts not currently in the `accepted` state are rejected so games
    /// cannot accidentally abandon rows still available (or already terminal).
    ///
    /// The optional `on_commit` callback runs after the SQL mutation while
    /// still inside the active transaction. If the callback returns `Err`,
    /// the transaction rolls back and no abandonment persists.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, abandoning a freight contract can
    ///   clear cargo reservations tied to the current pilot.
    /// - In a **dungeon crawler**, abandoning a guild commission can remove
    ///   active objective markers from the player's journal.
    pub fn abandon_contract<F>(
        &mut self,
        contract_id: i64,
        on_commit: Option<F>,
    ) -> Result<Contract, ContractError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &Contract) -> Result<(), rusqlite::Error>,
    {
        const SQL: &str = "\
UPDATE contracts\n\
SET state = ?2,\n\
    acceptor_player_id = NULL\n\
WHERE id = ?1\n\
  AND state = ?3\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let abandoned = match tx.query_row(
            SQL,
            rusqlite::params![
                contract_id,
                ContractState::Abandoned.as_str(),
                ContractState::Accepted.as_str()
            ],
            row_to_contract,
        ) {
            Ok(contract) => contract,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::diagnose_failed_state_transition(
                    &tx,
                    contract_id,
                    ContractState::Abandoned,
                ));
            }
            Err(source) => return Err(ContractError::Sqlite { source }),
        };

        if let Some(on_commit) = on_commit {
            on_commit(&tx, &abandoned).map_err(|source| ContractError::OnCommitRejected {
                id: contract_id,
                attempted: ContractState::Abandoned,
                source,
            })?;
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(abandoned)
    }

    /// Expire one overdue available contract inside a single transaction.
    ///
    /// This transition is intentionally narrow:
    ///
    /// - Only rows still in `available` are eligible.
    /// - Only rows with `expires_at <= CURRENT_TIMESTAMP` are eligible.
    ///
    /// That guard keeps expiry behavior predictable for games that may still
    /// inspect accepted/completed rows after their nominal deadlines.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, expire a stale station freight offer
    ///   that timed out before any pilot accepted it.
    /// - In a **dungeon crawler**, expire an unclaimed guild commission after
    ///   the tavern notice board rollover.
    pub fn expire_contract(&mut self, contract_id: i64) -> Result<Contract, ContractError> {
        const SQL: &str = "\
UPDATE contracts\n\
SET state = ?2\n\
WHERE id = ?1\n\
  AND state = ?3\n\
  AND expires_at IS NOT NULL\n\
  AND datetime(expires_at) <= CURRENT_TIMESTAMP\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let expired = match tx.query_row(
            SQL,
            rusqlite::params![
                contract_id,
                ContractState::Expired.as_str(),
                ContractState::Available.as_str()
            ],
            row_to_contract,
        ) {
            Ok(contract) => contract,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(Self::diagnose_failed_state_transition(
                    &tx,
                    contract_id,
                    ContractState::Expired,
                ));
            }
            Err(source) => return Err(ContractError::Sqlite { source }),
        };

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(expired)
    }

    /// Map a failed `accept_contract` mutation onto a typed lifecycle error.
    ///
    /// The acceptance SQL gate intentionally combines multiple invariants:
    /// "still available", "no existing acceptor", and "deadline not lapsed".
    /// When SQLite reports "no rows updated", this diagnostic query identifies
    /// which invariant failed so callers get a precise variant.
    fn diagnose_failed_accept_contract(
        tx: &rusqlite::Transaction<'_>,
        contract_id: i64,
    ) -> ContractError {
        const DIAG_SQL: &str = "\
SELECT state, acceptor_player_id, \
       CASE \
           WHEN expires_at IS NOT NULL \
               AND datetime(expires_at) <= CURRENT_TIMESTAMP \
           THEN 1 ELSE 0 \
       END AS deadline_lapsed \
FROM contracts \
WHERE id = ?1";

        let row = tx.query_row(DIAG_SQL, rusqlite::params![contract_id], |row| {
            let state: String = row.get(0)?;
            let acceptor_player_id: Option<i64> = row.get(1)?;
            let deadline_lapsed: i64 = row.get(2)?;
            Ok((state, acceptor_player_id, deadline_lapsed != 0))
        });

        match row {
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                ContractError::NotFound { id: contract_id }
            }
            Err(source) => ContractError::Sqlite { source },
            Ok((_state, Some(acceptor_player_id), _)) => ContractError::AlreadyAccepted {
                id: contract_id,
                acceptor_player_id,
            },
            Ok((state, _, true)) if state == ContractState::Available.as_str() => {
                ContractError::ExpiredOnAccept { id: contract_id }
            }
            Ok((state, _, _)) => ContractError::InvalidTransition {
                id: contract_id,
                from: state,
                to: ContractState::Accepted,
            },
        }
    }

    /// Map a failed state-to-state transition onto `NotFound` or `InvalidTransition`.
    ///
    /// Lifecycle helpers with fixed source-state preconditions call this after
    /// a guarded `UPDATE ... RETURNING` reports no rows. The diagnostic read is
    /// intentionally tiny: if the row exists we surface its current `state` as
    /// `from`; otherwise we surface `NotFound`.
    fn diagnose_failed_state_transition(
        tx: &rusqlite::Transaction<'_>,
        contract_id: i64,
        attempted: ContractState,
    ) -> ContractError {
        let row = tx.query_row(
            "SELECT state FROM contracts WHERE id = ?1",
            rusqlite::params![contract_id],
            |row| row.get::<_, String>(0),
        );

        match row {
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                ContractError::NotFound { id: contract_id }
            }
            Err(source) => ContractError::Sqlite { source },
            Ok(state) => ContractError::InvalidTransition {
                id: contract_id,
                from: state,
                to: attempted,
            },
        }
    }

    /// Expire all overdue available contracts whose deadline is at-or-before `now`.
    ///
    /// The sweep is one transaction to avoid partial state when a process exits
    /// mid-pass. It first selects due ids in deterministic deadline order, then
    /// updates each id under the same transaction so callers can report exactly
    /// which rows transitioned.
    ///
    /// Genre-neutral usage:
    ///
    /// - In a **space exploration** game, sweep stale station freight postings
    ///   before rendering a board refresh.
    /// - In a **dungeon crawler**, sweep expired guild commissions when opening
    ///   the mission hall screen.
    pub fn sweep_expired_contracts(&mut self, now: &str) -> Result<Vec<Contract>, ContractError> {
        const DUE_IDS_SQL: &str = "\
SELECT id\n\
FROM contracts\n\
WHERE state = ?1\n\
  AND expires_at IS NOT NULL\n\
  AND datetime(expires_at) <= datetime(?2)\n\
ORDER BY datetime(expires_at) ASC, id ASC";

        const EXPIRE_ONE_SQL: &str = "\
UPDATE contracts\n\
SET state = ?2\n\
WHERE id = ?1\n\
  AND state = ?3\n\
RETURNING id, key, kind, issuer_owner_kind, issuer_owner_id, \
          acceptor_player_id, state, objective_json, reward_json, metadata_json, \
          created_at, accepted_at, completed_at, expires_at";

        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| ContractError::Sqlite { source })?;

        let due_ids = {
            let mut statement = tx
                .prepare(DUE_IDS_SQL)
                .map_err(|source| ContractError::Sqlite { source })?;
            let due_ids = statement
                .query_map(
                    rusqlite::params![ContractState::Available.as_str(), now],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|source| ContractError::Sqlite { source })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|source| ContractError::Sqlite { source })?;
            due_ids
        };

        let mut expired = Vec::with_capacity(due_ids.len());
        for contract_id in due_ids {
            let contract = tx
                .query_row(
                    EXPIRE_ONE_SQL,
                    rusqlite::params![
                        contract_id,
                        ContractState::Expired.as_str(),
                        ContractState::Available.as_str()
                    ],
                    row_to_contract,
                )
                .map_err(|source| ContractError::Sqlite { source })?;
            expired.push(contract);
        }

        tx.commit()
            .map_err(|source| ContractError::Sqlite { source })?;

        Ok(expired)
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

    #[test]
    fn accept_contract_rejects_when_acceptor_already_set() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("guild-escort"),
                kind: "escort",
                issuer_owner_kind: "guild",
                issuer_owner_id: 14,
                objective_json: r#"{"from":"north-gate","to":"sanctum"}"#,
                reward_json: r#"{"credits":900}"#,
                metadata_json: Some(r#"{"danger":"medium"}"#),
                expires_at: None,
            })
            .expect("create_contract succeeds");

        let first_accept = world
            .accept_contract(
                created.id,
                51,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("first accept succeeds");

        let second_accept = world.accept_contract(
            created.id,
            88,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                second_accept,
                Err(super::ContractError::AlreadyAccepted {
                    id,
                    acceptor_player_id: 51
                })
                if id == created.id
            ),
            "second accept should fail when an acceptor is already recorded"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("contract row still exists");
        assert_eq!(
            persisted.acceptor_player_id, first_accept.acceptor_player_id,
            "failed second accept must not replace the original acceptor"
        );
        assert_eq!(
            persisted.accepted_at, first_accept.accepted_at,
            "failed second accept must not rewrite accepted_at"
        );
    }

    #[test]
    fn accept_contract_rejects_when_expired_at_or_before_now() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let expires_now: String = world
            .connection()
            .query_row("SELECT CURRENT_TIMESTAMP", [], |row| row.get(0))
            .expect("current timestamp query succeeds");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("dockside-bounty"),
                kind: "bounty",
                issuer_owner_kind: "harbor",
                issuer_owner_id: 23,
                objective_json: r#"{"target":"smuggler"}"#,
                reward_json: r#"{"credits":700}"#,
                metadata_json: Some(r#"{"difficulty":"high"}"#),
                expires_at: Some(expires_now.as_str()),
            })
            .expect("create_contract succeeds");

        let accept = world.accept_contract(
            created.id,
            31,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                accept,
                Err(super::ContractError::ExpiredOnAccept { id }) if id == created.id
            ),
            "accept should fail when expires_at is at-or-before current timestamp"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("contract row still exists");
        assert_eq!(
            persisted.state,
            ContractState::Available.as_str(),
            "failed accept on expired contract must keep state unchanged"
        );
        assert_eq!(
            persisted.acceptor_player_id, None,
            "failed accept on expired contract must not set an acceptor"
        );
        assert_eq!(
            persisted.accepted_at, None,
            "failed accept on expired contract must not set accepted_at"
        );
    }

    #[test]
    fn accept_contract_rolls_back_when_on_commit_returns_error() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("market-delivery"),
                kind: "delivery",
                issuer_owner_kind: "market",
                issuer_owner_id: 12,
                objective_json: r#"{"pickup":"district-east","dropoff":"district-west"}"#,
                reward_json: r#"{"credits":180}"#,
                metadata_json: Some(r#"{"urgency":"normal"}"#),
                expires_at: None,
            })
            .expect("create_contract succeeds");

        let accept = world.accept_contract(
            created.id,
            44,
            Some(
                |_tx: &rusqlite::Transaction<'_>,
                 _accepted: &super::Contract|
                 -> Result<(), rusqlite::Error> {
                    // Force the callback failure branch to verify transaction rollback.
                    Err(rusqlite::Error::InvalidQuery)
                },
            ),
        );
        assert!(
            matches!(
                accept,
                Err(super::ContractError::OnCommitRejected {
                    id,
                    attempted: ContractState::Accepted,
                    source: rusqlite::Error::InvalidQuery
                }) if id == created.id
            ),
            "accept should surface callback error when on_commit returns Err"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("contract row still exists");
        assert_eq!(
            persisted.state,
            ContractState::Available.as_str(),
            "callback error must roll back state transition"
        );
        assert_eq!(
            persisted.acceptor_player_id, None,
            "callback error must roll back acceptor assignment"
        );
        assert_eq!(
            persisted.accepted_at, None,
            "callback error must roll back accepted timestamp"
        );
    }

    #[test]
    fn complete_contract_sets_state_completed_and_completed_timestamp() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("caravan-delivery"),
                kind: "delivery",
                issuer_owner_kind: "market",
                issuer_owner_id: 18,
                objective_json: r#"{"from":"river-gate","to":"hill-district"}"#,
                reward_json: r#"{"credits":260}"#,
                metadata_json: Some(r#"{"season":"harvest"}"#),
                expires_at: None,
            })
            .expect("create_contract succeeds");

        let accepted = world
            .accept_contract(
                created.id,
                65,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("accept_contract succeeds");
        let completed = world
            .complete_contract(
                created.id,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("complete_contract succeeds");

        assert_eq!(completed.id, created.id);
        assert_eq!(completed.state, ContractState::Completed.as_str());
        assert_eq!(completed.acceptor_player_id, accepted.acceptor_player_id);
        assert!(
            completed.completed_at.is_some(),
            "complete_contract should stamp completed_at"
        );
        assert_eq!(
            completed.accepted_at, accepted.accepted_at,
            "completion should preserve the original acceptance timestamp"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("completed row exists");
        assert_eq!(persisted.state, ContractState::Completed.as_str());
        assert_eq!(persisted.completed_at, completed.completed_at);
    }

    #[test]
    fn complete_contract_rejects_when_state_is_not_accepted() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let created = world
            .create_contract(CreateContractInput {
                key: Some("crypt-recovery"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 5,
                objective_json: r#"{"zone":"west-crypt"}"#,
                reward_json: r#"{"credits":540}"#,
                metadata_json: Some(r#"{"rank":"silver"}"#),
                expires_at: None,
            })
            .expect("create_contract succeeds");

        let complete = world.complete_contract(
            created.id,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                complete,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Completed
                }) if id == created.id && from == ContractState::Available.as_str()
            ),
            "complete should fail unless the contract is currently accepted"
        );

        let persisted = world
            .contract_by_id(created.id)
            .expect("lookup succeeds")
            .expect("contract row still exists");
        assert_eq!(
            persisted.state,
            ContractState::Available.as_str(),
            "failed completion must leave available contracts unchanged"
        );
        assert_eq!(
            persisted.completed_at, None,
            "failed completion must not set completed_at"
        );
    }

    #[test]
    fn fail_contract_transitions_from_accepted_and_rejects_invalid_source_states() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let accepted = world
            .create_contract(CreateContractInput {
                key: Some("station-fail-ready"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 21,
                objective_json: r#"{"from":"dock-2","to":"dock-8"}"#,
                reward_json: r#"{"credits":120}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("accepted candidate insert succeeds");
        let available = world
            .create_contract(CreateContractInput {
                key: Some("station-still-available"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 21,
                objective_json: r#"{"from":"dock-1","to":"dock-3"}"#,
                reward_json: r#"{"credits":90}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("available candidate insert succeeds");

        world
            .accept_contract(
                accepted.id,
                700,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("accept succeeds");

        let failed = world
            .fail_contract(
                accepted.id,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("fail_contract succeeds from accepted");
        assert_eq!(failed.state, ContractState::Failed.as_str());
        assert_eq!(
            failed.acceptor_player_id,
            Some(700),
            "failing should preserve who held the contract when it failed"
        );

        let reject_available = world.fail_contract(
            available.id,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                reject_available,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Failed
                }) if id == available.id && from == ContractState::Available.as_str()
            ),
            "fail_contract should reject when source state is not accepted"
        );

        let reject_already_failed = world.fail_contract(
            accepted.id,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                reject_already_failed,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Failed
                }) if id == accepted.id && from == ContractState::Failed.as_str()
            ),
            "fail_contract should reject terminal rows on repeated invocation"
        );
    }

    #[test]
    fn abandon_contract_transitions_from_accepted_clears_acceptor_and_rejects_invalid_states() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let accepted = world
            .create_contract(CreateContractInput {
                key: Some("guild-abandon-ready"),
                kind: "escort",
                issuer_owner_kind: "guild",
                issuer_owner_id: 32,
                objective_json: r#"{"from":"hall","to":"tower"}"#,
                reward_json: r#"{"favor":{"guild":1}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("accepted candidate insert succeeds");
        let available = world
            .create_contract(CreateContractInput {
                key: Some("guild-unaccepted"),
                kind: "escort",
                issuer_owner_kind: "guild",
                issuer_owner_id: 32,
                objective_json: r#"{"from":"market","to":"river"}"#,
                reward_json: r#"{"favor":{"guild":1}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("available candidate insert succeeds");

        let accepted = world
            .accept_contract(
                accepted.id,
                808,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("accept succeeds");
        assert_eq!(accepted.acceptor_player_id, Some(808));

        let abandoned = world
            .abandon_contract(
                accepted.id,
                None::<
                    fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>,
                >,
            )
            .expect("abandon_contract succeeds from accepted");
        assert_eq!(abandoned.state, ContractState::Abandoned.as_str());
        assert_eq!(
            abandoned.acceptor_player_id, None,
            "abandon_contract should clear the acceptor as documented"
        );

        let persisted = world
            .contract_by_id(accepted.id)
            .expect("lookup succeeds")
            .expect("abandoned contract persists");
        assert_eq!(persisted.state, ContractState::Abandoned.as_str());
        assert_eq!(
            persisted.acceptor_player_id, None,
            "persisted abandoned row should keep acceptor cleared"
        );

        let reject_available = world.abandon_contract(
            available.id,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                reject_available,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Abandoned
                }) if id == available.id && from == ContractState::Available.as_str()
            ),
            "abandon_contract should reject when source state is not accepted"
        );

        let reject_already_abandoned = world.abandon_contract(
            accepted.id,
            None::<fn(&rusqlite::Transaction<'_>, &super::Contract) -> Result<(), rusqlite::Error>>,
        );
        assert!(
            matches!(
                reject_already_abandoned,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Abandoned
                }) if id == accepted.id && from == ContractState::Abandoned.as_str()
            ),
            "abandon_contract should reject terminal rows on repeated invocation"
        );
    }

    #[test]
    fn expire_contract_transitions_due_available_row_only() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let due_available = world
            .create_contract(CreateContractInput {
                key: Some("due-available"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 44,
                objective_json: r#"{"to":"dock-east"}"#,
                reward_json: r#"{"credits":40}"#,
                metadata_json: None,
                expires_at: Some("2000-01-01 00:00:00"),
            })
            .expect("due available insert succeeds");
        let future_available = world
            .create_contract(CreateContractInput {
                key: Some("future-available"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 44,
                objective_json: r#"{"to":"dock-west"}"#,
                reward_json: r#"{"credits":50}"#,
                metadata_json: None,
                expires_at: Some("2999-01-01 00:00:00"),
            })
            .expect("future available insert succeeds");
        let accepted_past_due = world
            .create_contract(CreateContractInput {
                key: Some("accepted-past-due"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 11,
                objective_json: r#"{"room":"deep-crypt"}"#,
                reward_json: r#"{"favor":{"guild":3}}"#,
                metadata_json: None,
                expires_at: Some("2000-01-01 00:00:00"),
            })
            .expect("accepted candidate insert succeeds");
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Accepted.as_str(),
                    909_i64,
                    accepted_past_due.id
                ],
            )
            .expect("marking accepted contract succeeds");

        let expired = world
            .expire_contract(due_available.id)
            .expect("due available row should expire");
        assert_eq!(expired.state, ContractState::Expired.as_str());

        let reject_future = world.expire_contract(future_available.id);
        assert!(
            matches!(
                reject_future,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Expired
                }) if id == future_available.id && from == ContractState::Available.as_str()
            ),
            "future deadline rows should not expire early"
        );

        let reject_accepted = world.expire_contract(accepted_past_due.id);
        assert!(
            matches!(
                reject_accepted,
                Err(super::ContractError::InvalidTransition {
                    id,
                    from,
                    to: ContractState::Expired
                }) if id == accepted_past_due.id && from == ContractState::Accepted.as_str()
            ),
            "accepted rows should not transition via expire_contract"
        );
    }

    #[test]
    fn sweep_expired_contracts_moves_only_past_due_available_rows() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let due_available = world
            .create_contract(CreateContractInput {
                key: Some("due-available"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 12,
                objective_json: r#"{"to":"ring-a"}"#,
                reward_json: r#"{"credits":70}"#,
                metadata_json: None,
                expires_at: Some("2026-05-10 11:59:59"),
            })
            .expect("due available insert succeeds");
        let future_available = world
            .create_contract(CreateContractInput {
                key: Some("future-available"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 12,
                objective_json: r#"{"to":"ring-b"}"#,
                reward_json: r#"{"credits":90}"#,
                metadata_json: None,
                expires_at: Some("2026-05-10 12:00:01"),
            })
            .expect("future available insert succeeds");
        let no_deadline_available = world
            .create_contract(CreateContractInput {
                key: Some("no-deadline"),
                kind: "escort",
                issuer_owner_kind: "guild",
                issuer_owner_id: 9,
                objective_json: r#"{"from":"market","to":"tower"}"#,
                reward_json: r#"{"favor":{"guild":1}}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("no deadline insert succeeds");
        let past_due_accepted = world
            .create_contract(CreateContractInput {
                key: Some("past-due-accepted"),
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 9,
                objective_json: r#"{"room":"catacomb"}"#,
                reward_json: r#"{"favor":{"guild":4}}"#,
                metadata_json: None,
                expires_at: Some("2026-05-10 11:59:00"),
            })
            .expect("past due accepted insert succeeds");
        world
            .connection()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2, accepted_at = CURRENT_TIMESTAMP WHERE id = ?3",
                rusqlite::params![
                    ContractState::Accepted.as_str(),
                    301_i64,
                    past_due_accepted.id
                ],
            )
            .expect("accepted row update succeeds");

        let swept = world
            .sweep_expired_contracts("2026-05-10 12:00:00")
            .expect("sweep succeeds");
        assert_eq!(
            swept.iter().map(|contract| contract.id).collect::<Vec<_>>(),
            vec![due_available.id],
            "sweep should only transition rows that are both due and available"
        );

        let due_after = world
            .contract_by_id(due_available.id)
            .expect("lookup due row succeeds")
            .expect("due row still exists");
        assert_eq!(due_after.state, ContractState::Expired.as_str());

        let future_after = world
            .contract_by_id(future_available.id)
            .expect("lookup future row succeeds")
            .expect("future row still exists");
        assert_eq!(future_after.state, ContractState::Available.as_str());

        let no_deadline_after = world
            .contract_by_id(no_deadline_available.id)
            .expect("lookup no-deadline row succeeds")
            .expect("no-deadline row still exists");
        assert_eq!(no_deadline_after.state, ContractState::Available.as_str());

        let accepted_after = world
            .contract_by_id(past_due_accepted.id)
            .expect("lookup accepted row succeeds")
            .expect("accepted row still exists");
        assert_eq!(accepted_after.state, ContractState::Accepted.as_str());
        assert_eq!(
            accepted_after.acceptor_player_id,
            Some(301),
            "sweep should not alter accepted rows even if their deadline is past due"
        );
    }
}
