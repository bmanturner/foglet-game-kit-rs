//! `contract_jobs` — reusable read models for player-visible work.
//!
//! This module complements [`crate::job_board`]. The Job Board lists
//! available opportunities from several providers; contract job views render
//! a player's available, accepted, ready, and terminal contract rows without
//! teaching the kit what any `objective_json` or `reward_json` means.

use serde::Deserialize;
use thiserror::Error;

use crate::contracts::{AvailableContractsFilter, Contract, ContractError};
use crate::world_db::WorldDb;

/// Player-facing lifecycle state for contract/job views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobLifecycleView {
    /// Available to accept.
    Available,
    /// Accepted and still in progress.
    Accepted,
    /// Accepted and ready for game-owned completion.
    ReadyToComplete,
    /// Completed successfully.
    Completed,
    /// Failed by game-defined rules.
    Failed,
    /// Abandoned by the player.
    Abandoned,
    /// Expired by deadline.
    Expired,
}

impl JobLifecycleView {
    /// Stable lower-case label for default rendering.
    #[must_use]
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Accepted => "accepted",
            Self::ReadyToComplete => "ready",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
            Self::Expired => "expired",
        }
    }
}

/// One requirement row for a contract detail surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRequirementView {
    /// Player-facing requirement label.
    pub label: String,
    /// Whether the requirement is met.
    pub met: bool,
}

/// Game-supplied objective/readiness projection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContractObjectiveView {
    /// Whether an accepted contract can be completed now.
    pub ready_to_complete: bool,
    /// Optional next action text for the player.
    pub next_step: Option<String>,
    /// Requirement rows for list/detail UI.
    pub requirements: Vec<JobRequirementView>,
    /// Optional opaque token for game completion handlers.
    pub complete_action: Option<String>,
}

/// Render-ready contract job row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractJobView {
    /// Stable contract row id.
    pub contract_id: i64,
    /// Optional game-defined stable key.
    pub key: Option<String>,
    /// Player-facing title.
    pub title: String,
    /// Player-facing summary/detail text.
    pub summary: String,
    /// Player-facing kind label.
    pub kind_label: String,
    /// Typed lifecycle for filtering and rendering.
    pub state: JobLifecycleView,
    /// Player-facing lifecycle label.
    pub state_label: String,
    /// Optional reward preview.
    pub reward_preview: Option<String>,
    /// Optional location preview.
    pub location_preview: Option<String>,
    /// Optional next action text.
    pub next_step: Option<String>,
    /// Requirement rows.
    pub requirements: Vec<JobRequirementView>,
    /// Optional opaque token for accept handlers.
    pub accept_action: Option<String>,
    /// Optional opaque token for completion handlers.
    pub complete_action: Option<String>,
}

/// Readiness extension point for game-owned objective semantics.
pub trait ContractObjectiveViewProvider {
    /// Resolve objective readiness for one contract.
    fn readiness(
        &self,
        world_db: &WorldDb,
        contract: &Contract,
    ) -> Result<ContractObjectiveView, ContractJobError>;
}

/// Default provider that treats objectives as opaque and never ready.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoObjectiveViewProvider;

impl ContractObjectiveViewProvider for NoObjectiveViewProvider {
    fn readiness(
        &self,
        _world_db: &WorldDb,
        _contract: &Contract,
    ) -> Result<ContractObjectiveView, ContractJobError> {
        Ok(ContractObjectiveView::default())
    }
}

impl<F> ContractObjectiveViewProvider for F
where
    F: Fn(&WorldDb, &Contract) -> Result<ContractObjectiveView, ContractJobError>,
{
    fn readiness(
        &self,
        world_db: &WorldDb,
        contract: &Contract,
    ) -> Result<ContractObjectiveView, ContractJobError> {
        self(world_db, contract)
    }
}

/// Errors surfaced while building contract job views.
#[derive(Debug, Error)]
pub enum ContractJobError {
    /// Contract query failed.
    #[error("failed to query contracts for job views: {source}")]
    ContractQuery {
        /// Underlying contract error.
        #[source]
        source: ContractError,
    },
}

/// List available contracts as job views.
pub fn available_contract_job_views(
    world_db: &WorldDb,
    filter: &AvailableContractsFilter,
) -> Result<Vec<ContractJobView>, ContractJobError> {
    let contracts = world_db
        .available_contracts(filter)
        .map_err(|source| ContractJobError::ContractQuery { source })?;

    Ok(contracts
        .iter()
        .map(|contract| project_contract_job_view(contract, None))
        .collect())
}

/// List contract jobs accepted by one player.
pub fn player_contract_job_views(
    world_db: &WorldDb,
    player_id: i64,
    objective_provider: &dyn ContractObjectiveViewProvider,
) -> Result<Vec<ContractJobView>, ContractJobError> {
    let contracts = world_db
        .contracts_for_acceptor(player_id, None)
        .map_err(|source| ContractJobError::ContractQuery { source })?;

    contracts
        .iter()
        .map(|contract| {
            let objective = match lifecycle_for_contract(contract) {
                JobLifecycleView::Accepted => {
                    Some(objective_provider.readiness(world_db, contract)?)
                }
                _ => None,
            };
            Ok(project_contract_job_view(contract, objective))
        })
        .collect()
}

/// Project one contract row into a render-ready job view.
#[must_use]
pub fn project_contract_job_view(
    contract: &Contract,
    objective: Option<ContractObjectiveView>,
) -> ContractJobView {
    let metadata = parse_contract_job_metadata(contract.metadata_json.as_deref());
    let mut state = lifecycle_for_contract(contract);
    let objective = objective.unwrap_or_default();
    if matches!(state, JobLifecycleView::Accepted) && objective.ready_to_complete {
        state = JobLifecycleView::ReadyToComplete;
    }

    let fallback_title = contract
        .key
        .clone()
        .unwrap_or_else(|| format!("Contract #{}", contract.id));
    let state_label = metadata
        .state_label
        .unwrap_or_else(|| state.as_label().to_string());

    ContractJobView {
        contract_id: contract.id,
        key: contract.key.clone(),
        title: metadata.title.unwrap_or(fallback_title),
        summary: metadata
            .summary
            .unwrap_or_else(|| contract.objective_json.clone()),
        kind_label: metadata.kind_label.unwrap_or_else(|| contract.kind.clone()),
        state,
        state_label,
        reward_preview: metadata
            .reward_preview
            .or_else(|| Some(contract.reward_json.clone())),
        location_preview: metadata.location_preview,
        next_step: objective.next_step,
        requirements: objective.requirements,
        accept_action: metadata.accept_action.or_else(|| {
            matches!(state, JobLifecycleView::Available)
                .then(|| format!("contract:{}", contract.id))
        }),
        complete_action: objective.complete_action,
    }
}

fn lifecycle_for_contract(contract: &Contract) -> JobLifecycleView {
    match contract.state.as_str() {
        "available" => JobLifecycleView::Available,
        "accepted" => JobLifecycleView::Accepted,
        "completed" => JobLifecycleView::Completed,
        "failed" => JobLifecycleView::Failed,
        "abandoned" => JobLifecycleView::Abandoned,
        "expired" => JobLifecycleView::Expired,
        _ => JobLifecycleView::Accepted,
    }
}

#[derive(Debug, Default, Deserialize)]
struct ContractJobMetadata {
    kind_label: Option<String>,
    state_label: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    reward_preview: Option<String>,
    location_preview: Option<String>,
    accept_action: Option<String>,
}

fn parse_contract_job_metadata(raw: Option<&str>) -> ContractJobMetadata {
    raw.and_then(|value| serde_json::from_str::<ContractJobMetadata>(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        player_contract_job_views, ContractJobError, ContractObjectiveView, JobLifecycleView,
        JobRequirementView,
    };
    use crate::contracts::{
        Contract, ContractError, ContractState, CreateContractInput, CONTRACTS_MIGRATION,
    };
    use crate::inventory::{transfer_on, InventorySlot, INVENTORY_SLOTS_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
    use crate::presence::{get_presence_on, PRESENCE_MIGRATION};
    use crate::spatial::PLACES_MIGRATION;
    use crate::world_db::WorldDb;
    use serde::Deserialize;
    use tempfile::tempdir;

    #[test]
    fn player_contract_job_views_project_accepted_and_completed_rows() {
        let Fixture {
            mut world,
            player_id,
            _tempdir,
        } = setup_contract_job_world();
        let accepted = world
            .create_and_accept_contract(
                CreateContractInput {
                    key: Some("guild-delivery"),
                    kind: "delivery",
                    issuer_owner_kind: "guild",
                    issuer_owner_id: 1,
                    objective_json: r#"{"dropoff":"archive"}"#,
                    reward_json: r#"{"coins":25}"#,
                    metadata_json: Some(
                        r#"{
                            "title":"Archive Delivery",
                            "summary":"Carry sealed records.",
                            "kind_label":"Guild Work",
                            "reward_preview":"25 coins",
                            "location_preview":"Archive"
                        }"#,
                    ),
                    expires_at: None,
                },
                player_id,
                None::<fn(&rusqlite::Transaction<'_>, &crate::Contract) -> rusqlite::Result<()>>,
            )
            .expect("accepted contract inserts");
        let completed = world
            .create_and_accept_contract(
                CreateContractInput {
                    key: Some("done"),
                    kind: "survey",
                    issuer_owner_kind: "guild",
                    issuer_owner_id: 1,
                    objective_json: r#"{"room":"tower"}"#,
                    reward_json: r#"{"xp":5}"#,
                    metadata_json: None,
                    expires_at: None,
                },
                player_id,
                None::<fn(&rusqlite::Transaction<'_>, &crate::Contract) -> rusqlite::Result<()>>,
            )
            .expect("second contract inserts");
        world
            .complete_contract(
                completed.id,
                None::<fn(&rusqlite::Transaction<'_>, &crate::Contract) -> rusqlite::Result<()>>,
            )
            .expect("contract completes");

        let views = player_contract_job_views(&world, player_id, &super::NoObjectiveViewProvider)
            .expect("views project");

        assert_eq!(views.len(), 2);
        let accepted_view = views
            .iter()
            .find(|view| view.contract_id == accepted.id)
            .expect("accepted view exists");
        assert_eq!(accepted_view.state, JobLifecycleView::Accepted);
        assert_eq!(accepted_view.title, "Archive Delivery");
        assert_eq!(accepted_view.summary, "Carry sealed records.");
        assert_eq!(accepted_view.reward_preview.as_deref(), Some("25 coins"));
        assert_eq!(accepted_view.location_preview.as_deref(), Some("Archive"));
        assert!(accepted_view.complete_action.is_none());

        let completed_view = views
            .iter()
            .find(|view| view.contract_id == completed.id)
            .expect("completed view exists");
        assert_eq!(completed_view.state, JobLifecycleView::Completed);
        assert_eq!(completed_view.state_label, "completed");
    }

    #[test]
    fn readiness_provider_can_mark_one_accepted_contract_ready_to_complete() {
        let Fixture {
            mut world,
            player_id,
            _tempdir,
        } = setup_contract_job_world();
        let ready = world
            .create_and_accept_contract(
                CreateContractInput {
                    key: Some("ready"),
                    kind: "delivery",
                    issuer_owner_kind: "station",
                    issuer_owner_id: 2,
                    objective_json: r#"{"dropoff":"relay"}"#,
                    reward_json: r#"{"credits":100}"#,
                    metadata_json: None,
                    expires_at: None,
                },
                player_id,
                None::<fn(&rusqlite::Transaction<'_>, &crate::Contract) -> rusqlite::Result<()>>,
            )
            .expect("ready contract inserts");
        let blocked = world
            .create_and_accept_contract(
                CreateContractInput {
                    key: Some("blocked"),
                    kind: "delivery",
                    issuer_owner_kind: "station",
                    issuer_owner_id: 2,
                    objective_json: r#"{"dropoff":"pier"}"#,
                    reward_json: r#"{"credits":50}"#,
                    metadata_json: None,
                    expires_at: None,
                },
                player_id,
                None::<fn(&rusqlite::Transaction<'_>, &crate::Contract) -> rusqlite::Result<()>>,
            )
            .expect("blocked contract inserts");

        let views = player_contract_job_views(
            &world,
            player_id,
            &|_: &WorldDb, contract: &crate::contracts::Contract| {
                Ok(ContractObjectiveView {
                    ready_to_complete: contract.key.as_deref() == Some("ready"),
                    next_step: Some("Dock at the destination.".to_string()),
                    requirements: vec![JobRequirementView {
                        label: "Arrived at destination".to_string(),
                        met: contract.key.as_deref() == Some("ready"),
                    }],
                    complete_action: (contract.key.as_deref() == Some("ready"))
                        .then(|| format!("complete:{}", contract.id)),
                })
            },
        )
        .expect("views project with readiness");

        let ready_view = views
            .iter()
            .find(|view| view.contract_id == ready.id)
            .expect("ready view exists");
        assert_eq!(ready_view.state, JobLifecycleView::ReadyToComplete);
        assert_eq!(
            ready_view.complete_action.as_deref(),
            Some(format!("complete:{}", ready.id).as_str())
        );
        assert!(ready_view.requirements[0].met);

        let blocked_view = views
            .iter()
            .find(|view| view.contract_id == blocked.id)
            .expect("blocked view exists");
        assert_eq!(blocked_view.state, JobLifecycleView::Accepted);
        assert!(!blocked_view.requirements[0].met);
        assert!(blocked_view.complete_action.is_none());
    }

    struct Fixture {
        world: WorldDb,
        player_id: i64,
        _tempdir: tempfile::TempDir,
    }

    fn setup_contract_job_world() -> Fixture {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");
        let player_id = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES ('jobber') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("player inserts");

        Fixture {
            world,
            player_id,
            _tempdir: dir,
        }
    }

    #[test]
    fn contract_state_names_still_match_lifecycle_projection() {
        assert_eq!(ContractState::Available.as_str(), "available");
        assert_eq!(ContractState::Accepted.as_str(), "accepted");
        assert_eq!(ContractState::Completed.as_str(), "completed");
    }

    #[test]
    fn custom_objective_provider_requires_place_cargo_and_proof() {
        let CustomObjectiveFixture {
            world,
            player_id,
            contract,
            archive_id,
            _tempdir,
        } = setup_custom_objective_world();

        let blocked_views = player_contract_job_views(&world, player_id, &cargo_proof_readiness)
            .expect("blocked views project");
        let blocked = blocked_views
            .iter()
            .find(|view| view.contract_id == contract.id)
            .expect("blocked contract view exists");
        assert_eq!(blocked.state, JobLifecycleView::Accepted);
        assert_eq!(
            blocked.next_step.as_deref(),
            Some("Bring the cargo and proof to the archive.")
        );
        assert_eq!(
            blocked
                .requirements
                .iter()
                .map(|requirement| (requirement.label.as_str(), requirement.met))
                .collect::<Vec<_>>(),
            vec![
                ("At archive", false),
                ("Has black-box", false),
                ("Proof recorded", false),
            ]
        );

        world
            .set_presence(player_id, archive_id, None)
            .expect("player moves to archive");
        world
            .create_slot("player", player_id, "black-box", 1, None, None)
            .expect("cargo inserts");
        world
            .connection()
            .execute(
                "INSERT INTO salvage_proofs (player_id, proof_key, details) VALUES (?1, ?2, ?3)",
                rusqlite::params![player_id, "wreck-alpha", "Recovered recorder hash."],
            )
            .expect("proof inserts");

        let ready_views = player_contract_job_views(&world, player_id, &cargo_proof_readiness)
            .expect("ready views project");
        let ready = ready_views
            .iter()
            .find(|view| view.contract_id == contract.id)
            .expect("ready contract view exists");
        assert_eq!(ready.state, JobLifecycleView::ReadyToComplete);
        assert_eq!(ready.next_step.as_deref(), Some("Complete at the archive."));
        assert!(ready.requirements.iter().all(|requirement| requirement.met));
        assert_eq!(
            ready.complete_action.as_deref(),
            Some(format!("complete:{}", contract.id).as_str())
        );
    }

    #[test]
    fn custom_objective_completion_transfers_inventory_and_preserves_proof() {
        let CustomObjectiveFixture {
            mut world,
            player_id,
            contract,
            archive_id,
            _tempdir,
        } = setup_custom_objective_world();
        world
            .set_presence(player_id, archive_id, None)
            .expect("player moves to archive");
        world
            .create_slot("player", player_id, "black-box", 1, None, None)
            .expect("cargo inserts");
        world
            .connection()
            .execute(
                "INSERT INTO salvage_proofs (player_id, proof_key, details) VALUES (?1, ?2, ?3)",
                rusqlite::params![player_id, "wreck-alpha", "Recovered recorder hash."],
            )
            .expect("proof inserts");

        let completed = world
            .complete_contract(
                contract.id,
                Some(
                    |tx: &rusqlite::Transaction<'_>, _: &Contract| -> rusqlite::Result<()> {
                        transfer_on(
                            tx,
                            ("player", player_id),
                            ("archive", archive_id),
                            "black-box",
                            1,
                            None::<
                                fn(
                                    &rusqlite::Connection,
                                    &InventorySlot,
                                    &InventorySlot,
                                ) -> rusqlite::Result<()>,
                            >,
                        )
                        .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
                        Ok(())
                    },
                ),
            )
            .expect("contract completes");
        assert_eq!(completed.state, ContractState::Completed.as_str());

        assert_eq!(
            world
                .get_slot("player", player_id, "black-box")
                .expect("player cargo reads")
                .expect("player cargo slot remains")
                .quantity,
            0
        );
        assert_eq!(
            world
                .get_slot("archive", archive_id, "black-box")
                .expect("archive cargo reads")
                .expect("archive cargo exists")
                .quantity,
            1
        );
        let proof_count: i64 = world
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM salvage_proofs WHERE player_id = ?1 AND proof_key = ?2",
                rusqlite::params![player_id, "wreck-alpha"],
                |row| row.get(0),
            )
            .expect("proof count reads");
        assert_eq!(
            proof_count, 1,
            "game-owned proof history should survive item consumption"
        );

        let duplicate = world.complete_contract(
            contract.id,
            None::<fn(&rusqlite::Transaction<'_>, &Contract) -> rusqlite::Result<()>>,
        );
        match duplicate {
            Err(ContractError::InvalidTransition { from, to, .. }) => {
                assert_eq!(from, ContractState::Completed.as_str());
                assert_eq!(to, ContractState::Completed);
            }
            other => panic!("expected duplicate completion lifecycle failure, got {other:?}"),
        }
        assert_eq!(
            world
                .get_slot("archive", archive_id, "black-box")
                .expect("archive cargo reads")
                .expect("archive cargo exists")
                .quantity,
            1,
            "duplicate completion must not consume cargo again"
        );
    }

    #[derive(Debug, Deserialize)]
    struct CargoProofObjective {
        item_key: String,
        proof_key: String,
        place_id: i64,
    }

    fn cargo_proof_readiness(
        world: &WorldDb,
        contract: &Contract,
    ) -> Result<ContractObjectiveView, ContractJobError> {
        let objective: CargoProofObjective =
            serde_json::from_str(&contract.objective_json).expect("fixture objective parses");
        let player_id = contract
            .acceptor_player_id
            .expect("fixture contract is accepted");
        let at_place = get_presence_on(world.connection(), player_id)
            .expect("presence lookup succeeds")
            .is_some_and(|presence| presence.place_id == objective.place_id);
        let has_cargo = world
            .get_slot("player", player_id, &objective.item_key)
            .expect("inventory lookup succeeds")
            .is_some_and(|slot| slot.quantity > 0);
        let has_proof: bool = world
            .connection()
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM salvage_proofs
                    WHERE player_id = ?1 AND proof_key = ?2
                )",
                rusqlite::params![player_id, &objective.proof_key],
                |row| row.get(0),
            )
            .expect("proof lookup succeeds");
        let ready_to_complete = at_place && has_cargo && has_proof;

        Ok(ContractObjectiveView {
            ready_to_complete,
            next_step: Some(
                if ready_to_complete {
                    "Complete at the archive."
                } else {
                    "Bring the cargo and proof to the archive."
                }
                .to_string(),
            ),
            requirements: vec![
                JobRequirementView {
                    label: "At archive".to_string(),
                    met: at_place,
                },
                JobRequirementView {
                    label: format!("Has {}", objective.item_key),
                    met: has_cargo,
                },
                JobRequirementView {
                    label: "Proof recorded".to_string(),
                    met: has_proof,
                },
            ],
            complete_action: ready_to_complete.then(|| format!("complete:{}", contract.id)),
        })
    }

    struct CustomObjectiveFixture {
        world: WorldDb,
        player_id: i64,
        contract: Contract,
        archive_id: i64,
        _tempdir: tempfile::TempDir,
    }

    fn setup_custom_objective_world() -> CustomObjectiveFixture {
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
        world
            .apply_migration(&INVENTORY_SLOTS_MIGRATION)
            .expect("inventory migration applies");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");
        world
            .connection()
            .execute(
                "CREATE TABLE salvage_proofs (
                    player_id INTEGER NOT NULL,
                    proof_key TEXT NOT NULL,
                    details TEXT NOT NULL,
                    PRIMARY KEY (player_id, proof_key)
                )",
                [],
            )
            .expect("proof table creates");

        let player_id = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES ('salvager') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("player inserts");
        let airlock = world
            .insert_place("airlock", "Airlock", "room", None)
            .expect("airlock inserts");
        let archive = world
            .insert_place("archive", "Archive", "room", None)
            .expect("archive inserts");
        world
            .set_presence(player_id, airlock.id, None)
            .expect("initial presence sets");

        let objective_json = format!(
            r#"{{"item_key":"black-box","proof_key":"wreck-alpha","place_id":{}}}"#,
            archive.id
        );
        let contract = world
            .create_and_accept_contract(
                CreateContractInput {
                    key: Some("recover-black-box"),
                    kind: "custom-proof",
                    issuer_owner_kind: "archive",
                    issuer_owner_id: archive.id,
                    objective_json: &objective_json,
                    reward_json: r#"{"credits":250}"#,
                    metadata_json: Some(
                        r#"{
                            "title":"Recover the Black Box",
                            "summary":"Bring cargo and proof to the archive.",
                            "kind_label":"Salvage",
                            "location_preview":"Archive"
                        }"#,
                    ),
                    expires_at: None,
                },
                player_id,
                None::<fn(&rusqlite::Transaction<'_>, &Contract) -> rusqlite::Result<()>>,
            )
            .expect("contract accepted");

        CustomObjectiveFixture {
            world,
            player_id,
            contract,
            archive_id: archive.id,
            _tempdir: dir,
        }
    }
}
