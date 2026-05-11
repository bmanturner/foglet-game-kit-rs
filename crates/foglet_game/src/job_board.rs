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

use crate::bounties::BountyError;
use crate::bounties::BOUNTY_DESCRIPTION_MAX_CHARS;
use crate::challenges::ChallengeError;
use crate::config::MultiplayerSection;
use crate::contracts::{AvailableContractsFilter, ContractError};
use crate::notices::NOTICE_SUBJECT_MAX_CHARS;
use crate::world_db::WorldDb;

/// Stable source bucket for an aggregated [`JobBoardEntry`].
///
/// Source labels remain separate from game-facing copy so the kit can
/// sort/filter entries by origin without mutating provider-supplied
/// display strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JobBoardSource {
    /// Entry sourced from the `contracts` primitive.
    Contract,
    /// Entry sourced from the `bounties` primitive.
    Bounty,
    /// Entry sourced from the `challenges` primitive.
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
/// The default strategy follows the contract:
///
/// - first by `expires_at` ascending.
/// - then by `source`.
/// - then by `source_id`.
///
/// Games that want to preserve provider emission order exactly can use
/// [`Self::ProviderOrder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JobBoardSort {
    /// Canonical default ordering.
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

/// v3-bounded cap for short player-facing strings projected into the
/// board list row (title column).
const JOB_BOARD_TITLE_MAX_CHARS: usize = NOTICE_SUBJECT_MAX_CHARS;
/// v3-bounded cap for longer player-facing strings projected into the
/// board detail body (summary column).
const JOB_BOARD_SUMMARY_MAX_CHARS: usize = BOUNTY_DESCRIPTION_MAX_CHARS;

impl JobBoard {
    /// Aggregate rows from all `providers`, apply `filter`, and return
    /// one stable-ordered list.
    ///
    /// Default ordering is: `expires_at` ascending, then
    /// `source`, then `source_id`.
    pub fn query(
        world_db: &WorldDb,
        filter: &JobBoardFilter,
        providers: &[&dyn OpportunityProvider],
    ) -> Result<Vec<JobBoardEntry>, JobBoardError> {
        let mut decorated = Vec::new();
        for (provider_index, provider) in providers.iter().enumerate() {
            let provider_entries = provider.entries(world_db)?;
            for (entry_index, mut entry) in provider_entries.into_iter().enumerate() {
                //  requires sanitization at the aggregation
                // boundary regardless of provider. We sanitize here so
                // both built-in and external rows obey the same
                // terminal-safety + bounded-text contract before any
                // screen renders them.
                sanitize_entry_text(&mut entry);
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

fn sanitize_entry_text(entry: &mut JobBoardEntry) {
    entry.title = sanitize_for_terminal(&entry.title, JOB_BOARD_TITLE_MAX_CHARS, false);
    entry.summary = sanitize_for_terminal(&entry.summary, JOB_BOARD_SUMMARY_MAX_CHARS, true);
}

fn sanitize_for_terminal(raw: &str, max_chars: usize, preserve_multiline: bool) -> String {
    let mut output = String::with_capacity(raw.len().min(max_chars));
    let mut visible_chars = 0_usize;
    for ch in raw.chars() {
        let Some(normalized) = normalize_terminal_char(ch, preserve_multiline) else {
            continue;
        };
        if visible_chars >= max_chars {
            break;
        }
        output.push(normalized);
        visible_chars += 1;
    }
    output
}

fn normalize_terminal_char(ch: char, preserve_multiline: bool) -> Option<char> {
    // `char::is_control` strips both C0 and C1 ranges, including ESC.
    // We optionally preserve LF/tab for multi-line detail surfaces.
    if ch.is_control() {
        if preserve_multiline && matches!(ch, '\n' | '\t') {
            return Some(ch);
        }
        return None;
    }
    // Strip/neutralize the highest-risk invisible text-shaping chars.
    if is_zero_width_or_bidi_override(ch) {
        return Some('\u{FFFD}');
    }
    Some(ch)
}

fn is_zero_width_or_bidi_override(ch: char) -> bool {
    matches!(
        ch,
        '\u{200B}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
    )
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
    /// Querying bounty rows for the built-in provider failed.
    #[error("failed to project bounties into job-board entries: {source}")]
    BountyQuery {
        /// Underlying bounty query failure.
        #[source]
        source: BountyError,
    },
    /// Querying challenge rows for the built-in provider failed.
    #[error("failed to project challenges into job-board entries: {source}")]
    ChallengeQuery {
        /// Underlying challenge query failure.
        #[source]
        source: ChallengeError,
    },
}

/// Built-in provider that projects `contracts` rows into board
/// entries.
///
/// The provider returns one [`JobBoardEntry`] per **available**
/// contract. Display fields are read from optional metadata overrides
/// when present and otherwise fall back to contract fields so games can
/// start with zero extra schema.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContractProvider;

impl ContractProvider {
    /// Construct a provider for contract-backed opportunities.
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

/// Built-in provider that projects `bounties` rows into board
/// entries.
///
/// The provider returns one [`JobBoardEntry`] per `open` bounty:
///
/// - In a **space exploration** game this can represent "escort a
///   freighter through a contested lane".
/// - In a **dungeon crawler** it can represent "recover a relic from a
///   flooded crypt".
#[derive(Debug, Default, Clone, Copy)]
pub struct BountyProvider;

impl BountyProvider {
    /// Construct a provider for bounty-backed opportunities.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl OpportunityProvider for BountyProvider {
    fn entries(&self, world_db: &WorldDb) -> Result<Vec<JobBoardEntry>, JobBoardError> {
        const SQL: &str = "\
SELECT id, title, description, reward, state, expires_at \
FROM bounties \
WHERE state = 'open' \
ORDER BY created_at ASC, id ASC";

        let mut stmt =
            world_db
                .connection()
                .prepare(SQL)
                .map_err(|source| JobBoardError::BountyQuery {
                    source: BountyError::Sqlite { source },
                })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(BountyBoardRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    reward: row.get(3)?,
                    state: row.get(4)?,
                    expires_at: row.get(5)?,
                })
            })
            .map_err(|source| JobBoardError::BountyQuery {
                source: BountyError::Sqlite { source },
            })?;
        let rows = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| JobBoardError::BountyQuery {
                source: BountyError::Sqlite { source },
            })?;

        Ok(rows
            .into_iter()
            .map(|row| JobBoardEntry {
                source: JobBoardSource::Bounty,
                source_id: row.id,
                kind_label: "Bounty".to_string(),
                state_label: row.state,
                title: row.title,
                summary: row.description,
                reward_preview: Some(row.reward),
                location_preview: None,
                expires_at: row.expires_at,
                accept_action: Some(format!("bounty:{}", row.id)),
            })
            .collect())
    }
}

/// Built-in provider that projects `challenges` rows into board
/// entries.
///
/// The provider returns one [`JobBoardEntry`] per `open` challenge:
///
/// - In a **space exploration** game this can represent an open
///   navigation race challenge.
/// - In a **town simulation** game this can represent a civic
///   improvement challenge.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChallengeProvider;

impl ChallengeProvider {
    /// Construct a provider for challenge-backed opportunities.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl OpportunityProvider for ChallengeProvider {
    fn entries(&self, world_db: &WorldDb) -> Result<Vec<JobBoardEntry>, JobBoardError> {
        const SQL: &str = "\
SELECT id, kind, stake, state, challenger_player_id, target_player_id, expires_at \
FROM challenges \
WHERE state = 'open' \
ORDER BY created_at ASC, id ASC";

        let mut stmt =
            world_db
                .connection()
                .prepare(SQL)
                .map_err(|source| JobBoardError::ChallengeQuery {
                    source: ChallengeError::Sqlite { source },
                })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ChallengeBoardRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    stake: row.get(2)?,
                    state: row.get(3)?,
                    challenger_player_id: row.get(4)?,
                    target_player_id: row.get(5)?,
                    expires_at: row.get(6)?,
                })
            })
            .map_err(|source| JobBoardError::ChallengeQuery {
                source: ChallengeError::Sqlite { source },
            })?;
        let rows = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|source| JobBoardError::ChallengeQuery {
                source: ChallengeError::Sqlite { source },
            })?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let fallback_summary = format!(
                    "Open challenge between player {} and player {}",
                    row.challenger_player_id, row.target_player_id
                );
                JobBoardEntry {
                    source: JobBoardSource::Challenge,
                    source_id: row.id,
                    kind_label: "Challenge".to_string(),
                    state_label: row.state,
                    title: format!("Challenge: {}", row.kind),
                    summary: row.stake.clone().unwrap_or(fallback_summary),
                    reward_preview: row.stake,
                    location_preview: None,
                    expires_at: row.expires_at,
                    accept_action: Some(format!("challenge:{}", row.id)),
                }
            })
            .collect())
    }
}

/// Built-in provider bundle selected from v5/config toggles.
///
/// The bundle always includes [`ContractProvider`] because v5's
/// `job_board` primitive depends on `contracts`. providers are
/// included only when their specific multiplayer toggles are enabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltInProviders {
    contract: ContractProvider,
    bounty: Option<BountyProvider>,
    challenge: Option<ChallengeProvider>,
}

impl BuiltInProviders {
    /// Build the built-in provider set from multiplayer toggles.
    ///
    /// Gating rules:
    ///
    /// - `multiplayer = None` (disabled): only contracts.
    /// - `multiplayer.bounties = true`: include [`BountyProvider`].
    /// - `multiplayer.challenges = true`: include [`ChallengeProvider`].
    #[must_use]
    pub fn from_multiplayer(multiplayer: Option<&MultiplayerSection>) -> Self {
        let bounty = multiplayer
            .filter(|section| section.bounties)
            .map(|_| BountyProvider::new());
        let challenge = multiplayer
            .filter(|section| section.challenges)
            .map(|_| ChallengeProvider::new());
        Self {
            contract: ContractProvider::new(),
            bounty,
            challenge,
        }
    }

    /// Return this bundle as trait-object references for
    /// [`JobBoard::query`].
    ///
    /// This is the bridging helper games use before task 11 wires
    /// these providers into runtime handles.
    #[must_use]
    pub fn as_provider_refs(&self) -> Vec<&dyn OpportunityProvider> {
        let mut providers: Vec<&dyn OpportunityProvider> = vec![&self.contract];
        if let Some(provider) = self.bounty.as_ref() {
            providers.push(provider);
        }
        if let Some(provider) = self.challenge.as_ref() {
            providers.push(provider);
        }
        providers
    }
}

#[derive(Debug)]
struct BountyBoardRow {
    id: i64,
    title: String,
    description: String,
    reward: String,
    state: String,
    expires_at: Option<String>,
}

#[derive(Debug)]
struct ChallengeBoardRow {
    id: i64,
    kind: String,
    stake: Option<String>,
    state: String,
    challenger_player_id: i64,
    target_player_id: i64,
    expires_at: Option<String>,
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
        BountyProvider, BuiltInProviders, ChallengeProvider, ContractProvider, JobBoard,
        JobBoardEntry, JobBoardFilter, JobBoardSort, JobBoardSource, OpportunityProvider,
    };
    use crate::bounties::BOUNTIES_MIGRATION;
    use crate::challenges::CHALLENGES_MIGRATION;
    use crate::config::MultiplayerSection;
    use crate::contracts::{ContractState, CreateContractInput, CONTRACTS_MIGRATION};
    use crate::players::PLAYERS_MIGRATION;
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

    #[derive(Debug)]
    struct ExternalProvider {
        entries: Vec<JobBoardEntry>,
    }

    impl OpportunityProvider for ExternalProvider {
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
    fn query_accepts_game_supplied_external_provider_with_stable_ordering() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");

        let first_custom = ExternalProvider {
            entries: vec![
                JobBoardEntry {
                    source: JobBoardSource::External,
                    source_id: 9,
                    kind_label: "Tavern".to_string(),
                    state_label: "open".to_string(),
                    title: "Bandit sightings".to_string(),
                    summary: "Patrol the old bridge.".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2032-01-01T00:00:00Z".to_string()),
                    accept_action: Some("external:bandits".to_string()),
                },
                JobBoardEntry {
                    source: JobBoardSource::External,
                    source_id: 22,
                    kind_label: "Harbor Office".to_string(),
                    state_label: "open".to_string(),
                    title: "Dock inventory count".to_string(),
                    summary: "Audit warehouse crates.".to_string(),
                    reward_preview: None,
                    location_preview: None,
                    expires_at: Some("2032-01-02T00:00:00Z".to_string()),
                    accept_action: Some("external:inventory".to_string()),
                },
            ],
        };
        let second_custom = ExternalProvider {
            entries: vec![JobBoardEntry {
                source: JobBoardSource::External,
                source_id: 9,
                kind_label: "Guild Hall".to_string(),
                state_label: "open".to_string(),
                title: "Bandit sightings (duplicate tie)".to_string(),
                summary: "Cross-check yesterday's reports.".to_string(),
                reward_preview: None,
                location_preview: None,
                expires_at: Some("2032-01-01T00:00:00Z".to_string()),
                accept_action: Some("external:bandits-followup".to_string()),
            }],
        };
        let providers: [&dyn OpportunityProvider; 2] = [&first_custom, &second_custom];

        let entries = JobBoard::query(&world, &JobBoardFilter::default(), &providers)
            .expect("query succeeds");
        let external_entries: Vec<&JobBoardEntry> = entries
            .iter()
            .filter(|entry| entry.source == JobBoardSource::External)
            .collect();

        assert_eq!(external_entries.len(), 3, "all custom rows are retained");
        assert_eq!(
            external_entries
                .iter()
                .map(|entry| entry.source.as_str())
                .collect::<Vec<_>>(),
            vec!["external", "external", "external"],
            "custom provider rows keep the external source marker"
        );
        assert_eq!(
            external_entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Bandit sightings",
                "Bandit sightings (duplicate tie)",
                "Dock inventory count",
            ],
            "ties use deterministic provider/index fallback ordering"
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

    #[test]
    fn query_sanitizes_provider_title_and_summary_with_v3_bounded_text_rules() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let world = WorldDb::open(&db_path).expect("open succeeds");

        let overlong_title = format!(
            "A\u{200E}{}\u{001B}[2J\u{0085}TAIL",
            "T".repeat(super::JOB_BOARD_TITLE_MAX_CHARS)
        );
        let overlong_summary = format!(
            "First line\nSecond\tline\u{200D}{}\u{001B}[31m\u{0085}TAIL",
            "S".repeat(super::JOB_BOARD_SUMMARY_MAX_CHARS)
        );
        let provider = StaticProvider {
            entries: vec![JobBoardEntry {
                source: JobBoardSource::External,
                source_id: 51,
                kind_label: "Guild".to_string(),
                state_label: "open".to_string(),
                title: overlong_title,
                summary: overlong_summary,
                reward_preview: None,
                location_preview: None,
                expires_at: None,
                accept_action: None,
            }],
        };

        let providers: [&dyn OpportunityProvider; 1] = [&provider];
        let entries = JobBoard::query(&world, &JobBoardFilter::default(), &providers)
            .expect("query succeeds");
        let entry = &entries[0];

        assert_eq!(
            entry.title.chars().count(),
            super::JOB_BOARD_TITLE_MAX_CHARS,
            "title should be truncated to the short-text bound after sanitization"
        );
        assert_eq!(
            entry.summary.chars().count(),
            super::JOB_BOARD_SUMMARY_MAX_CHARS,
            "summary should be truncated to the long-text bound after sanitization"
        );
        assert!(
            !entry.title.contains('\u{001B}') && !entry.summary.contains('\u{001B}'),
            "ESC control characters should be stripped"
        );
        assert!(
            !entry.title.contains('\u{0085}') && !entry.summary.contains('\u{0085}'),
            "C1 controls should be stripped"
        );
        assert!(
            entry.title.contains('\u{FFFD}') || entry.summary.contains('\u{FFFD}'),
            "zero-width / bidi override chars should be neutralized"
        );
        assert!(
            entry.summary.contains('\n') && entry.summary.contains('\t'),
            "summary sanitizer should preserve multiline separators for detail views"
        );
    }

    #[test]
    fn bounty_provider_returns_only_open_bounties() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&BOUNTIES_MIGRATION)
            .expect("bounties migration applies");

        let open = world
            .post_bounty(
                None,
                "Resupply Lantern Post",
                "Deliver fuel cells before dusk.",
                r#"{"credits":1200}"#,
                Some("2035-05-10T14:00:00Z"),
            )
            .expect("open bounty inserts");
        let claimed = world
            .post_bounty(
                None,
                "Recover Archive Ledger",
                "Retrieve the ledger from the flooded vault.",
                r#"{"reputation":3}"#,
                None,
            )
            .expect("second bounty inserts");
        world
            .connection_mut()
            .execute(
                "UPDATE bounties SET state = 'claimed' WHERE id = ?1",
                params![claimed.id],
            )
            .expect("state update succeeds");

        let provider = BountyProvider::new();
        let entries = provider.entries(&world).expect("provider query succeeds");
        assert_eq!(entries.len(), 1, "only open bounties should be listed");
        assert_eq!(entries[0].source, JobBoardSource::Bounty);
        assert_eq!(entries[0].source_id, open.id);
        assert_eq!(entries[0].title, "Resupply Lantern Post");
        assert_eq!(entries[0].summary, "Deliver fuel cells before dusk.");
        assert_eq!(entries[0].state_label, "open");
    }

    #[test]
    fn challenge_provider_returns_only_open_challenges() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&CHALLENGES_MIGRATION)
            .expect("challenges migration applies");

        let challenger_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) VALUES ('u-1', 'orbit-a') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("challenger inserts");
        let target_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (foglet_user_id, handle) VALUES ('u-2', 'guild-b') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("target inserts");

        let open = world
            .create_challenge(
                challenger_id,
                target_id,
                "district-race",
                Some(r#"{"stake":"market permit"}"#),
                Some("2035-05-10T14:00:00Z"),
            )
            .expect("open challenge inserts");
        let accepted = world
            .create_challenge(challenger_id, target_id, "catacomb-sprint", None, None)
            .expect("second challenge inserts");
        world
            .accept_challenge(accepted.id)
            .expect("accept transition succeeds");

        let provider = ChallengeProvider::new();
        let entries = provider.entries(&world).expect("provider query succeeds");
        assert_eq!(entries.len(), 1, "only open challenges should be listed");
        assert_eq!(entries[0].source, JobBoardSource::Challenge);
        assert_eq!(entries[0].source_id, open.id);
        assert_eq!(entries[0].title, "Challenge: district-race");
        assert_eq!(entries[0].summary, r#"{"stake":"market permit"}"#);
        assert_eq!(entries[0].state_label, "open");
    }

    #[test]
    fn builtin_providers_omit_v3_sources_when_multiplayer_is_disabled() {
        let builtins = BuiltInProviders::from_multiplayer(None);
        let providers = builtins.as_provider_refs();
        assert_eq!(providers.len(), 1, "only contracts should be wired");
    }

    #[test]
    fn builtin_providers_include_v3_sources_when_multiplayer_toggles_are_enabled() {
        let multiplayer = MultiplayerSection {
            challenges: true,
            bounties: true,
            ..MultiplayerSection::default()
        };
        let builtins = BuiltInProviders::from_multiplayer(Some(&multiplayer));
        let providers = builtins.as_provider_refs();
        assert_eq!(
            providers.len(),
            3,
            "contracts + bounty + challenge providers should be wired"
        );
    }
}
