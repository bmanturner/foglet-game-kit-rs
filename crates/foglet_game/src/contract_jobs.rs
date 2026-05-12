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
        player_contract_job_views, ContractObjectiveView, JobLifecycleView, JobRequirementView,
    };
    use crate::contracts::{ContractState, CreateContractInput, CONTRACTS_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
    use crate::world_db::WorldDb;
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
        assert_eq!(ready_view.requirements[0].met, true);

        let blocked_view = views
            .iter()
            .find(|view| view.contract_id == blocked.id)
            .expect("blocked view exists");
        assert_eq!(blocked_view.state, JobLifecycleView::Accepted);
        assert_eq!(blocked_view.requirements[0].met, false);
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
}
