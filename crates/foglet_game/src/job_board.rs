//! `job_board` — opportunity aggregation primitives for v5.
//!
//! This module defines the render-shaped opportunity row and provider
//! contract used by the upcoming unified Job Board surface.
//!
//! The shape is intentionally genre-neutral:
//!
//! - In a **space exploration** game, a row can represent a cargo
//!   contract posted by a dock authority.
//! - In a **dungeon crawler**, a row can represent a guild commission
//!   to clear a crypt wing.

use std::cmp::Ordering;

use serde::Deserialize;
use thiserror::Error;

use crate::contracts::{AvailableContractsFilter, ContractError};
use crate::world_db::WorldDb;

/// Stable source bucket for an aggregated [`JobBoardEntry`].
///
/// Source labels remain separate from game-facing copy so the kit can
/// sort/filter entries by origin without mutating provider-supplied
/// display strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobBoardSource {
    /// Entry sourced from the v5 `contracts` primitive.
    Contract,
    /// Entry sourced from the v3 `bounties` primitive.
    Bounty,
    /// Entry sourced from the v3 `challenges` primitive.
    Challenge,
    /// Entry sourced from a game-defined external provider.
    External,
}

impl JobBoardSource {
    /// Canonical wire/display tag for this source bucket.
    pub fn as_str(self) -> &'static str {
        match self {
            JobBoardSource::Contract => "contract",
            JobBoardSource::Bounty => "bounty",
            JobBoardSource::Challenge => "challenge",
            JobBoardSource::External => "external",
        }
    }
}

/// Aggregated, render-ready opportunity row.
///
/// This struct is not persisted directly. Providers synthesize it from
/// their source rows so the board surface can render one uniform list.
///
/// Genre-neutral examples:
///
/// - A **space exploration** game can expose a starlane freight job
///   with `reward_preview = Some("1,200 credits")`.
/// - A **dungeon crawler** can expose a relic recovery commission with
///   `location_preview = Some("Sunken Vault")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobBoardEntry {
    /// Origin family (`contract`, `bounty`, `challenge`, `external`).
    pub source: JobBoardSource,
    /// Stable id within `source`.
    pub source_id: i64,
    /// Provider-authored kind label rendered in list rows.
    pub kind_label: String,
    /// Provider-authored lifecycle label rendered in list rows.
    pub state_label: String,
    /// Provider-authored short title rendered in list rows.
    pub title: String,
    /// Provider-authored longer summary shown in details.
    pub summary: String,
    /// Optional provider-authored reward preview.
    pub reward_preview: Option<String>,
    /// Optional provider-authored location preview.
    pub location_preview: Option<String>,
    /// Optional UTC expiry timestamp.
    pub expires_at: Option<String>,
    /// Optional opaque token forwarded to game accept/claim handlers.
    pub accept_action: Option<String>,
}

/// Sort strategy for [`JobBoard::query`].
///
/// The default strategy follows the v5 contract:
///
/// - first by `expires_at` ascending,
/// - then by `source`,
/// - then by `source_id`.
///
/// Games that want to preserve provider emission order exactly can use
/// [`Self::ProviderOrder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobBoardSort {
    /// Canonical v5 default ordering.
    #[default]
    Default,
    /// Leave rows in provider emission order.
    ProviderOrder,
}

/// Query filter for [`JobBoard::query`].
///
/// `allowed_sources` defaults to empty (include all sources). This
/// keeps the default behavior broad enough for cross-genre boards:
///
/// - A **space exploration** board can show contracts plus bounties.
/// - A **town simulation** board can show municipal contracts plus
///   game-defined external opportunities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobBoardFilter {
    /// Whitelist of sources to include.
    ///
    /// Empty means "include every source the providers return".
    pub allowed_sources: Vec<JobBoardSource>,
    /// Ordering strategy for the aggregated rows.
    pub sort: JobBoardSort,
}

impl JobBoardFilter {
    /// Returns `true` when an entry from `source` should be included in
    /// the result set.
    #[must_use]
    pub fn includes_source(&self, source: JobBoardSource) -> bool {
        self.allowed_sources.is_empty() || self.allowed_sources.contains(&source)
    }
}

/// Aggregation façade for composing rows from multiple opportunity
/// providers.
///
/// The board itself is stateless; it combines provider output for one
/// query call and applies a deterministic ordering policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct JobBoard;

impl JobBoard {
    /// Aggregate rows from all `providers`, apply `filter`, and return
    /// one stable-ordered list.
    ///
    /// Default ordering is SPEC_v5 §4.2: `expires_at` ascending, then
    /// `source`, then `source_id`.
    pub fn query(
        world_db: &WorldDb,
        filter: &JobBoardFilter,
        providers: &[&dyn OpportunityProvider],
    ) -> Result<Vec<JobBoardEntry>, JobBoardError> {
        let mut decorated = Vec::new();
        for (provider_index, provider) in providers.iter().enumerate() {
            let provider_entries = provider.entries(world_db)?;
            for (entry_index, entry) in provider_entries.into_iter().enumerate() {
                if filter.includes_source(entry.source) {
                    decorated.push(DecoratedEntry {
                        provider_index,
                        entry_index,
                        entry,
                    });
                }
            }
        }

        if matches!(filter.sort, JobBoardSort::Default) {
            decorated.sort_by(compare_default_order);
        }

        Ok(decorated.into_iter().map(|item| item.entry).collect())
    }
}

#[derive(Debug)]
struct DecoratedEntry {
    provider_index: usize,
    entry_index: usize,
    entry: JobBoardEntry,
}

fn compare_default_order(left: &DecoratedEntry, right: &DecoratedEntry) -> Ordering {
    // `None` expires_at means "no deadline", so it sorts after rows
    // with concrete timestamps.
    compare_expiration(
        left.entry.expires_at.as_deref(),
        right.entry.expires_at.as_deref(),
    )
    .then_with(|| left.entry.source.cmp(&right.entry.source))
    .then_with(|| left.entry.source_id.cmp(&right.entry.source_id))
    // Preserve deterministic output even when providers emit two
    // rows with matching source/id/expiry.
    .then_with(|| left.provider_index.cmp(&right.provider_index))
    .then_with(|| left.entry_index.cmp(&right.entry_index))
}

fn compare_expiration(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (Some(left_ts), Some(right_ts)) => left_ts.cmp(right_ts),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Shared provider contract for Job Board opportunity sources.
///
/// Providers convert one primitive's rows into [`JobBoardEntry`] values
/// without prescribing gameplay semantics:
///
/// - A **space exploration** provider can project freight contracts.
/// - A **town simulation** provider can project municipal work orders.
pub trait OpportunityProvider {
    /// Produce board entries from the current shared-world state.
    ///
    /// Implementations should keep strings genre/authorship-faithful
    /// and leave sanitization to the aggregation/render boundary.
    fn entries(&self, world_db: &WorldDb) -> Result<Vec<JobBoardEntry>, JobBoardError>;
}

/// Errors surfaced by Job Board providers.
#[derive(Debug, Error)]
pub enum JobBoardError {
    /// Querying contract rows for the built-in provider failed.
    #[error("failed to project contracts into job-board entries: {source}")]
    ContractQuery {
        /// Underlying contract query failure.
        #[source]
        source: ContractError,
    },
}

/// Built-in provider that projects v5 `contracts` rows into board
/// entries.
///
/// The provider returns one [`JobBoardEntry`] per **available**
/// contract. Display fields are read from optional metadata overrides
/// when present and otherwise fall back to contract fields so games can
/// start with zero extra schema.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContractProvider;

impl ContractProvider {
    /// Construct a provider for v5 contract-backed opportunities.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl OpportunityProvider for ContractProvider {
    fn entries(&self, world_db: &WorldDb) -> Result<Vec<JobBoardEntry>, JobBoardError> {
        let contracts = world_db
            .available_contracts(&AvailableContractsFilter::default())
            .map_err(|source| JobBoardError::ContractQuery { source })?;
        let entries = contracts
            .into_iter()
            .map(|contract| {
                let overrides = parse_contract_board_metadata(contract.metadata_json.as_deref());
                let source_id = contract.id;
                let fallback_title = contract
                    .key
                    .clone()
                    .unwrap_or_else(|| format!("Contract #{source_id}"));
                JobBoardEntry {
                    source: JobBoardSource::Contract,
                    source_id,
                    kind_label: overrides
                        .kind_label
                        .unwrap_or_else(|| contract.kind.clone()),
                    state_label: overrides
                        .state_label
                        .unwrap_or_else(|| contract.state.clone()),
                    title: overrides.title.unwrap_or(fallback_title),
                    summary: overrides.summary.unwrap_or(contract.objective_json.clone()),
                    reward_preview: overrides
                        .reward_preview
                        .or_else(|| Some(contract.reward_json.clone())),
                    location_preview: overrides.location_preview,
                    expires_at: contract.expires_at.clone(),
                    accept_action: overrides
                        .accept_action
                        .or_else(|| Some(format!("contract:{source_id}"))),
                }
            })
            .collect();
        Ok(entries)
    }
}

/// Optional per-contract projection hints stored in `metadata_json`.
///
/// Deserialization is best-effort so contracts that use metadata for
/// unrelated gameplay state still appear on the board.
#[derive(Debug, Default, Deserialize)]
struct ContractBoardMetadata {
    kind_label: Option<String>,
    state_label: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    reward_preview: Option<String>,
    location_preview: Option<String>,
    accept_action: Option<String>,
}

fn parse_contract_board_metadata(raw: Option<&str>) -> ContractBoardMetadata {
    raw.and_then(|value| serde_json::from_str::<ContractBoardMetadata>(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        ContractProvider, JobBoard, JobBoardEntry, JobBoardFilter, JobBoardSort, JobBoardSource,
        OpportunityProvider,
    };
    use crate::contracts::{ContractState, CreateContractInput, CONTRACTS_MIGRATION};
    use crate::world_db::WorldDb;
    use rusqlite::params;
    use tempfile::tempdir;

    #[derive(Debug)]
    struct StaticProvider {
        entries: Vec<JobBoardEntry>,
    }

    impl OpportunityProvider for StaticProvider {
        fn entries(&self, _world_db: &WorldDb) -> Result<Vec<JobBoardEntry>, super::JobBoardError> {
            Ok(self.entries.clone())
        }
    }

    #[test]
    fn contract_provider_returns_one_entry_per_available_contract_with_documented_shape() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&CONTRACTS_MIGRATION)
            .expect("contracts migration applies");

        let available_with_overrides = world
            .create_contract(CreateContractInput {
                key: Some("dock-run-7"),
                kind: "delivery",
                issuer_owner_kind: "station",
                issuer_owner_id: 11,
                objective_json: r#"{"route":"A->B"}"#,
                reward_json: r#"{"credits":900}"#,
                metadata_json: Some(
                    r#"{
                        "kind_label": "Dock Contract",
                        "state_label": "Open",
                        "title": "Move sealed crates to Pier B",
                        "summary": "Hazardous cargo, quiet route requested.",
                        "reward_preview": "900 credits + docking voucher",
                        "location_preview": "Pier B",
                        "accept_action": "accept:dock-run-7"
                    }"#,
                ),
                expires_at: Some("2035-05-10T14:00:00Z"),
            })
            .expect("available contract inserts");

        let available_with_fallbacks = world
            .create_contract(CreateContractInput {
                key: None,
                kind: "recovery",
                issuer_owner_kind: "guild",
                issuer_owner_id: 22,
                objective_json: r#"{"wing":"sunken-vault"}"#,
                reward_json: r#"{"xp":50}"#,
                metadata_json: Some(r#"{"unrelated":"value"}"#),
                expires_at: None,
            })
            .expect("second available contract inserts");

        let unavailable = world
            .create_contract(CreateContractInput {
                key: Some("already-accepted"),
                kind: "escort",
                issuer_owner_kind: "guild",
                issuer_owner_id: 22,
                objective_json: r#"{"target":"merchant"}"#,
                reward_json: r#"{"xp":20}"#,
                metadata_json: None,
                expires_at: None,
            })
            .expect("third contract inserts");
        world
            .connection_mut()
            .execute(
                "UPDATE contracts SET state = ?1, acceptor_player_id = ?2 WHERE id = ?3",
                params![ContractState::Accepted.as_str(), 77_i64, unavailable.id],
            )
            .expect("accepted state update succeeds");

        let provider = ContractProvider::new();
        let entries = provider.entries(&world).expect("provider query succeeds");

        assert_eq!(
            entries.len(),
            2,
            "provider should emit one row per available contract only"
        );

        let first = entries
            .iter()
            .find(|entry| entry.source_id == available_with_overrides.id)
            .expect("entry for metadata-overridden contract exists");
        assert_eq!(first.source, JobBoardSource::Contract);
        assert_eq!(first.kind_label, "Dock Contract");
        assert_eq!(first.state_label, "Open");
        assert_eq!(first.title, "Move sealed crates to Pier B");
        assert_eq!(first.summary, "Hazardous cargo, quiet route requested.");
        assert_eq!(
            first.reward_preview.as_deref(),
            Some("900 credits + docking voucher")
        );
        assert_eq!(first.location_preview.as_deref(), Some("Pier B"));
        assert_eq!(first.expires_at.as_deref(), Some("2035-05-10T14:00:00Z"));
        assert_eq!(first.accept_action.as_deref(), Some("accept:dock-run-7"));

        let second = entries
            .iter()
            .find(|entry| entry.source_id == available_with_fallbacks.id)
            .expect("entry for fallback-mapped contract exists");
        assert_eq!(second.source, JobBoardSource::Contract);
        assert_eq!(second.kind_label, "recovery");
        assert_eq!(second.state_label, "available");
        assert_eq!(
            second.title,
            format!("Contract #{}", available_with_fallbacks.id)
        );
        assert_eq!(second.summary, r#"{"wing":"sunken-vault"}"#);
        assert_eq!(second.reward_preview.as_deref(), Some(r#"{"xp":50}"#));
        assert_eq!(second.location_preview, None);
        assert_eq!(second.expires_at, None);
        assert_eq!(
            second.accept_action.as_deref(),
            Some(format!("contract:{}", available_with_fallbacks.id).as_str())
        );
    }

    #[test]
    fn query_default_sort_orders_by_expiry_then_source_then_source_id_stably() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");

        let alpha = StaticProvider {
            entries: vec![
                JobBoardEntry {
                    source: JobBoardSource::External,
                    source_id: 42,
                    kind_label: "Town Errand".to_string(),
                    state_label: "open".to_string(),
                    title: "alpha external tie".to_string(),
                    summary: "alpha summary".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: None,
                    accept_action: None,
                },
                JobBoardEntry {
                    source: JobBoardSource::Bounty,
                    source_id: 2,
                    kind_label: "Bounty".to_string(),
                    state_label: "open".to_string(),
                    title: "bounty row".to_string(),
                    summary: "bounty summary".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2030-01-02T00:00:00Z".to_string()),
                    accept_action: None,
                },
            ],
        };
        let bravo = StaticProvider {
            entries: vec![
                JobBoardEntry {
                    source: JobBoardSource::Contract,
                    source_id: 7,
                    kind_label: "Contract".to_string(),
                    state_label: "available".to_string(),
                    title: "contract row".to_string(),
                    summary: "contract summary".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2030-01-02T00:00:00Z".to_string()),
                    accept_action: None,
                },
                JobBoardEntry {
                    source: JobBoardSource::Challenge,
                    source_id: 4,
                    kind_label: "Challenge".to_string(),
                    state_label: "open".to_string(),
                    title: "challenge row".to_string(),
                    summary: "challenge summary".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2029-12-31T23:00:00Z".to_string()),
                    accept_action: None,
                },
                JobBoardEntry {
                    source: JobBoardSource::External,
                    source_id: 42,
                    kind_label: "Town Errand".to_string(),
                    state_label: "open".to_string(),
                    title: "bravo external tie".to_string(),
                    summary: "bravo summary".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: None,
                    accept_action: None,
                },
            ],
        };

        let providers: [&dyn OpportunityProvider; 2] = [&alpha, &bravo];
        let entries = JobBoard::query(&world, &JobBoardFilter::default(), &providers)
            .expect("query succeeds");

        let titles: Vec<&str> = entries.iter().map(|entry| entry.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "challenge row",
                "contract row",
                "bounty row",
                "alpha external tie",
                "bravo external tie",
            ]
        );
    }

    #[test]
    fn query_can_preserve_provider_order_when_requested() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");

        let provider = StaticProvider {
            entries: vec![
                JobBoardEntry {
                    source: JobBoardSource::External,
                    source_id: 5,
                    kind_label: "Guild".to_string(),
                    state_label: "open".to_string(),
                    title: "first".to_string(),
                    summary: "first".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2035-01-01T00:00:00Z".to_string()),
                    accept_action: None,
                },
                JobBoardEntry {
                    source: JobBoardSource::Contract,
                    source_id: 1,
                    kind_label: "Contract".to_string(),
                    state_label: "available".to_string(),
                    title: "second".to_string(),
                    summary: "second".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2020-01-01T00:00:00Z".to_string()),
                    accept_action: None,
                },
            ],
        };

        let filter = JobBoardFilter {
            allowed_sources: Vec::new(),
            sort: JobBoardSort::ProviderOrder,
        };
        let providers: [&dyn OpportunityProvider; 1] = [&provider];
        let entries = JobBoard::query(&world, &filter, &providers).expect("query succeeds");
        let titles: Vec<&str> = entries.iter().map(|entry| entry.title.as_str()).collect();
        assert_eq!(titles, vec!["first", "second"]);
    }
}
