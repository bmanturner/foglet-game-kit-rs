//! `GameConfig` — typed view of `assets/game.toml`.
//!
//! Game authors describe their door's identity and Foglet manifest
//! defaults declaratively in `assets/game.toml` (SPEC §9.1). The
//! runtime, the `fgk` CLI's `emit-manifest` / `package` commands, and
//! the example games all read the same struct so behaviour can't
//! drift between the library's view and the CLI's view of a project.
//!
//! # Why parse-then-validate (vs. pure serde)
//!
//! `serde`'s "missing field" errors are accurate but unfriendly:
//! authors get a single line about `start_map` and have to go hunting
//! for which TOML key they fat-fingered. The loader funnels `serde`
//! errors and our own validation through one [`ConfigError`] enum so
//! the `fgk` CLI can wrap them with `anyhow` at the process boundary
//! and the author sees one clean chain.
//!
//! # What's required vs. optional
//!
//! Per SPEC §5.2, every field listed in the fields block belongs in
//! `GameConfig`. The `[game]` section is fully required — those values
//! identify the door and seed the player's spawn point, so silently
//! defaulting them would mask authoring bugs. The `[save]` and
//! `[manifest]` sections have SPEC-documented defaults
//! (§5.6 / §10.3); we accept both "section omitted" and "section
//! present but partial" by defaulting at the field level.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::manifest::{
    DEFAULT_AUTH_SCOPE, DEFAULT_IDLE_TIMEOUT_MS, DEFAULT_TIMEOUT_MS, DEFAULT_VISIBILITY,
};

/// Parsed `assets/game.toml`.
///
/// The shape mirrors the SPEC §9.1 example one-to-one. `serde` drives
/// the actual deserialization; the wrapping [`ConfigError`] just
/// presents a uniform error surface to the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameConfig {
    /// `[game]` section — identity + spawn metadata.
    pub game: GameSection,
    /// `[save]` section. Optional in the file; defaults applied on
    /// load so missing sections don't blow up downstream consumers.
    #[serde(default)]
    pub save: SaveSection,
    /// `[manifest]` section — Foglet manifest defaults. Optional in
    /// the file; defaults applied on load.
    #[serde(default)]
    pub manifest: ManifestSection,
    /// `[world]` section — shared SQLite world DB settings (SPEC v2 §5).
    /// Optional in the file; absent section defaults to disabled so v1
    /// games keep working unchanged.
    #[serde(default)]
    pub world: WorldSection,
    /// `[turns]` section — daily turn ledger (SPEC v2 §4.6, §5).
    ///
    /// Modeled as `Option` rather than a defaulted struct because the
    /// SPEC explicitly says "Missing `[turns]` means no turn system":
    /// fabricating a default allowance for a game that didn't ask for
    /// turns would silently change runtime behavior. `None` is the
    /// "off" signal the runtime checks before opening a ledger.
    #[serde(default)]
    pub turns: Option<TurnsSection>,
    /// `[[leaderboards]]` array — built-in leaderboards (SPEC v2 §5).
    ///
    /// Modeled as a `Vec` defaulted to empty rather than `Option<Vec>`
    /// because "no leaderboards" and "an empty list of leaderboards"
    /// are the same SPEC-level statement ("Missing `[[leaderboards]]`
    /// means no built-in leaderboards"). Downstream consumers iterate
    /// the vec; an empty vec is the natural off signal.
    #[serde(default, rename = "leaderboards")]
    pub leaderboards: Vec<LeaderboardSection>,
    /// `[multiplayer]` section — v3 BBS-native async multiplayer
    /// primitives (SPEC v3 §5.2).
    ///
    /// Modeled as `Option` rather than a defaulted struct because
    /// SPEC v3 §5.2 says "Every primitive is opt-in" and "Generated
    /// v1/v2 projects are not forced to ship multiplayer screens":
    /// fabricating a default block for a game that did not request
    /// multiplayer would silently expose mailbox / market / bounty
    /// surface area the author never asked for. `None` is the off
    /// signal the runtime checks before wiring any v3 primitive.
    #[serde(default)]
    pub multiplayer: Option<MultiplayerSection>,
    /// `[spatial]` section — v4 location graph primitives (Task 3).
    ///
    /// `enabled` is default false so omitting this section keeps
    /// existing games unchanged and off the default path.
    #[serde(default)]
    pub spatial: SpatialSection,
    /// `[presence]` section — v4 player-location tracking primitives
    /// (Task 5).
    ///
    /// Absent section and `enabled = false` are treated the same:
    /// movement APIs are not wired until the game opts in.
    #[serde(default)]
    pub presence: PresenceSection,
    /// `[place_recall]` section — v4 discovered-place memory
    /// primitives for fog-of-war style UIs (Task 6).
    ///
    /// Keeping this separate from `presence` lets games choose whether
    /// to persist discovered state independently from movement.
    #[serde(default)]
    pub place_recall: PlaceRecallSection,
    /// `[inventory]` section — v4 owner-keyed stockpile primitives
    /// (Task 7 and 8).
    ///
    /// Explicitly disabled by default so older games are unaffected
    /// until they opt in to stockpile APIs.
    #[serde(default)]
    pub inventory: InventorySection,
    /// `[world_ticks]` section — v4 durable scheduled callback
    /// controls (Task 9).
    ///
    /// Defaults to disabled because v4 scheduling is additive and
    /// never on by default.
    #[serde(default)]
    pub world_ticks: WorldTicksSection,
    /// `[[factions.seed]]` array — game-authored faction definitions
    /// (SPEC v3 §5.2 example).
    ///
    /// Modeled as a defaulted `FactionsSection` (whose `seed` field is
    /// itself a `Vec` defaulted to empty) rather than `Option<…>` for
    /// the same reason as `[[leaderboards]]`: "no seeded factions" and
    /// "an empty seed list" are the same SPEC-level statement, and
    /// downstream consumers want to iterate. The section sits at the
    /// top level (not inside `[multiplayer]`) because TOML's
    /// `[[factions.seed]]` syntax declares a top-level `factions`
    /// table containing a `seed` array — it deliberately does not
    /// collide with the `multiplayer.factions` toggle bool.
    #[serde(default)]
    pub factions: FactionsSection,
}

/// `[game]` section: every field is required.
///
/// `min_width` / `min_height` are the *minimum* terminal size the
/// game expects; the runtime guard (Task 7) compares this against the
/// live terminal before entering raw mode (SPEC §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSection {
    /// Human-facing display name (rendered on the title screen).
    pub title: String,
    /// URL-safe identifier used in manifest paths and save dirs.
    /// Validated against the slug rule at load time (lowercase
    /// ASCII alphanumeric or `-`, no leading/trailing `-`) so a typo
    /// doesn't propagate into a filesystem path.
    pub slug: String,
    /// One-sentence description shown in Foglet's door listing.
    pub description: String,
    /// Minimum terminal columns the game renders into.
    pub min_width: u16,
    /// Minimum terminal rows the game renders into.
    pub min_height: u16,
    /// Map name the player starts on — must correspond to a map file
    /// the game ships in `assets/`. Cross-referenced by the map
    /// loader (Task 9), not here.
    pub start_map: String,
    /// Player spawn column on `start_map`.
    pub start_x: u16,
    /// Player spawn row on `start_map`.
    pub start_y: u16,
}

/// `[save]` section.
///
/// SPEC §5.6 says "Save paths MUST be per-user by default", so the
/// default strategy is [`SaveStrategy::PerFogletUser`]. Authors who
/// want a save-less door (e.g. a kiosk-style demo) opt in explicitly
/// with `strategy = "none"`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveSection {
    /// Where the save manager (Task 8) writes files, expressed as a
    /// strategy rather than a path so the CLI can resolve a path that
    /// matches the deployment environment.
    #[serde(default)]
    pub strategy: SaveStrategy,
}

/// Save strategies recognised by the save manager.
///
/// Closed enum (no `Other(String)` variant) so an unrecognised value
/// in `assets/game.toml` is a load-time error, not a silent fallback
/// to "do nothing on save".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SaveStrategy {
    /// One save file per Foglet user (the SPEC §5.6 default).
    #[default]
    PerFogletUser,
    /// Door explicitly opts out of persistence.
    None,
}

/// `[manifest]` section: defaults for `fgk emit-manifest` (Task 11).
///
/// Authors usually leave this blank and inherit the SPEC §10.3
/// defaults; surfacing the fields here is what lets a paranoid door
/// pin shorter timeouts or restrict visibility without a CLI flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSection {
    /// Per-session timeout in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Idle timeout in milliseconds.
    #[serde(default = "default_idle_timeout_ms")]
    pub idle_timeout_ms: u64,
    /// `members` / `public` / etc. — opaque string at this layer,
    /// validated by Foglet at install time.
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// `site` / `none` / etc. — opaque string at this layer.
    #[serde(default = "default_auth_scope")]
    pub auth_scope: String,
}

impl Default for ManifestSection {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
            visibility: DEFAULT_VISIBILITY.to_string(),
            auth_scope: DEFAULT_AUTH_SCOPE.to_string(),
        }
    }
}

/// `[world]` section: shared SQLite world DB settings (SPEC v2 §5).
///
/// Every field is field-level optional with a SPEC-documented default
/// so an author can write the section as `[world]\nenabled = true` and
/// inherit safe values for path, busy timeout, and journal mode. The
/// section as a whole is also optional: a v1 game with no `[world]`
/// block parses cleanly and behaves as if `enabled = false`.
///
/// We do not validate `path` against the filesystem here — that's the
/// world DB layer's job (Task 3). Storing it as a `String` rather than
/// a `PathBuf` keeps cross-platform packaging predictable: a config
/// authored on macOS deploys cleanly to a Linux Foglet host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSection {
    /// Whether the runtime should open a shared-world SQLite DB. The
    /// safe default is `false` so v1 projects without a `[world]`
    /// section continue to behave exactly as they did pre-v2.
    #[serde(default)]
    pub enabled: bool,
    /// Path to the SQLite file relative to the package root. The
    /// default matches the SPEC v2 §5 example so an operator who
    /// inspects a packaged door knows where to look without consulting
    /// the config.
    #[serde(default = "default_world_path")]
    pub path: String,
    /// SQLite `busy_timeout` in milliseconds. Applied during DB open
    /// (Task 3c) so contention from a parallel local-dev session
    /// retries instead of erroring out immediately.
    #[serde(default = "default_world_busy_timeout_ms")]
    pub busy_timeout_ms: u64,
    /// Journal mode applied during DB open (Task 3d). Stored as a
    /// `String` rather than a closed enum because SQLite has more
    /// journal modes than v2 currently uses; the world DB layer
    /// validates the value at apply time and falls back if a host
    /// rejects WAL.
    #[serde(default = "default_world_journal_mode")]
    pub journal_mode: String,
}

impl Default for WorldSection {
    fn default() -> Self {
        Self {
            enabled: false,
            path: default_world_path(),
            busy_timeout_ms: default_world_busy_timeout_ms(),
            journal_mode: default_world_journal_mode(),
        }
    }
}

/// `[turns]` section: daily turn allowance settings (SPEC v2 §4.6, §5).
///
/// `daily_allowance` is required because there's no defensible default
/// value — a game that opts into turns is making a design statement
/// about pacing, and silently picking a number would mask authoring
/// bugs (the same reason `[game]` fields aren't defaulted).
///
/// `reset` and `carryover_max` are field-level optional. The only
/// documented reset cadence in v2 is local midnight (SPEC §5 example),
/// so it has a default. `carryover_max = 0` is the SPEC-implied
/// "no carryover" behavior: a player who doesn't spend today does not
/// bank turns for tomorrow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnsSection {
    /// Number of turns granted at the start of each reset cycle.
    /// Required; rejected at validation time if zero.
    pub daily_allowance: u32,
    /// When the ledger rolls over. v2 ships only `LocalMidnight`; the
    /// closed enum means an unknown reset string surfaces as a parse
    /// error rather than silently disabling resets.
    #[serde(default)]
    pub reset: TurnReset,
    /// Maximum number of unspent turns carried into the next cycle.
    /// `0` means no carryover (the safe default for a brand-new
    /// configuration). Stored as `u32` so a TOML negative number is
    /// rejected at parse time.
    #[serde(default)]
    pub carryover_max: u32,
}

/// `[[leaderboards]]` entry: one named scoreboard (SPEC v2 §5).
///
/// `name` is the stable identifier used by the eventual leaderboard
/// helpers (Task 8) to scope writes and reads — duplicates would make
/// "increment score on board X" ambiguous, so they're rejected at
/// load time rather than silently coalescing entries.
///
/// `sort` is a closed enum because the only meaningful values for a
/// scoreboard are "highest is best" or "lowest is best"; an
/// unrecognised string almost certainly means the author misspelled
/// one of those, not that they want a third behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardSection {
    /// Stable identifier used to scope reads/writes. Validated as
    /// non-empty at load time; the helpers in Task 8 will further key
    /// SQL rows by this string, so a typo here would silently shard
    /// the scoreboard.
    pub name: String,
    /// Sort direction. `Desc` is the default because the typical
    /// scoreboard ranks "highest score first"; `Asc` exists for
    /// time-trial-style boards where lower is better.
    #[serde(default)]
    pub sort: LeaderboardSort,
}

/// Sort directions understood by leaderboard helpers.
///
/// Closed enum so an unrecognised value in `assets/game.toml` is a
/// load-time error, not a silent fallback to "descending".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardSort {
    /// Highest score first (the typical case).
    #[default]
    Desc,
    /// Lowest score first (time trials, golf-style scoring).
    Asc,
}

/// Reset cadences understood by the turn ledger.
///
/// Closed enum: an unrecognised value in `assets/game.toml` is a
/// load-time error, not a silent fallback to "never reset". v2 only
/// ships `local_midnight`; later versions will extend this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnReset {
    /// Roll the ledger over at the operator's local midnight.
    #[default]
    LocalMidnight,
}

/// `[multiplayer]` section: v3 BBS-native async multiplayer toggles
/// (SPEC v3 §5.2).
///
/// Each `bool` controls whether the corresponding primitive is wired
/// up at runtime. They default to `false` so that an author who writes
/// `[multiplayer]\nnotices = true` does *not* accidentally light up
/// challenges, the market, factions, and bounties as well — opting
/// into one primitive should not opt the door into all of them.
///
/// `max_notice_body_chars` is field-level optional with a SPEC-derived
/// default. SPEC v3 §7 requires player-authored text to be bounded;
/// 1000 chars matches the example in SPEC v3 §5.2 and is small enough
/// to render safely in an 80×24 terminal without sanitization
/// surprises. We keep it on this section (rather than a dedicated
/// `[limits]` block) so the cap travels alongside the toggle that
/// enables the surface where it applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiplayerSection {
    /// Enable v3 notices/mail (Task 3). When `false`, the runtime
    /// MUST NOT expose notice send/inbox APIs to game code; the kit
    /// still creates the schema migration to keep the on-disk DB
    /// shape predictable across operator-driven config changes.
    #[serde(default)]
    pub notices: bool,
    /// Enable v3 challenge lifecycle (Task 4).
    #[serde(default)]
    pub challenges: bool,
    /// Enable v3 shared market listings (Task 5).
    #[serde(default)]
    pub market: bool,
    /// Enable v3 factions and shared goals (Task 6).
    #[serde(default)]
    pub factions: bool,
    /// Enable v3 bounty board (Task 7).
    #[serde(default)]
    pub bounties: bool,
    /// Maximum subject+body length for player-authored notices, in
    /// characters. SPEC v3 §7 requires bounded text; the default
    /// (1000) matches the SPEC v3 §5.2 example. Stored as `u32` so a
    /// negative number is rejected at parse time.
    #[serde(default = "default_max_notice_body_chars")]
    pub max_notice_body_chars: u32,
}

impl Default for MultiplayerSection {
    fn default() -> Self {
        Self {
            notices: false,
            challenges: false,
            market: false,
            factions: false,
            bounties: false,
            max_notice_body_chars: default_max_notice_body_chars(),
        }
    }
}

fn default_max_notice_body_chars() -> u32 {
    1_000
}

/// `[factions]` section: holder for the `[[factions.seed]]` array
/// (SPEC v3 §5.2). The section itself carries no other knobs today;
/// it exists so `serde` has a stable parent for the array of seeds.
///
/// Defaults to an empty `seed` list so games that never declare a
/// faction (and v1/v2 games that predate the section entirely) parse
/// without ceremony.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FactionsSection {
    /// One entry per faction the game ships out of the box.
    /// Validated for slug shape, non-empty display name, and global
    /// uniqueness on slug at load time so a typo can't silently merge
    /// two intended-distinct agencies.
    #[serde(default)]
    pub seed: Vec<FactionSeed>,
}

/// `[spatial]` section: location graph primitives (v4 Task 3).
///
/// `enabled` is the contract-safe off-switch for place/route APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpatialSection {
    /// Enable directed location graph support for this project.
    #[serde(default)]
    pub enabled: bool,
}

/// `[presence]` section: player location tracking (v4 Task 5).
///
/// This section stays focused on whether movement APIs are
/// exposed; movement rules and gating remain game-defined and happen
/// via callbacks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PresenceSection {
    /// Enable current-location state and movement helpers.
    #[serde(default)]
    pub enabled: bool,
}

/// `[place_recall]` section: per-player visited-place history (v4 Task 6).
///
/// `enabled` gates storage and helper APIs for fog-of-war / map
/// memory. The schema and payload shape remain game-defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaceRecallSection {
    /// Enable visit tracking and recall listing helpers.
    #[serde(default)]
    pub enabled: bool,
}

/// `[inventory]` section: owner-keyed stockpile primitives (v4 Task 7/8).
///
/// This toggle enables stockpile persistence and transfer helpers;
/// it intentionally does not add economics, caps, or pricing policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InventorySection {
    /// Enable inventory CRUD and atomic transfer helpers.
    #[serde(default)]
    pub enabled: bool,
}

/// `[world_ticks]` section: durable scheduler controls (v4 Task 9).
///
/// `enabled` gates registration and execution entrypoints. The
/// catch-up budget defaults to `100` and must be positive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldTicksSection {
    /// Enable scheduled world-tick callbacks.
    #[serde(default)]
    pub enabled: bool,

    /// Maximum number of due tasks to process in one `run_due_ticks`
    /// invocation.
    #[serde(default = "default_max_catchup_per_call")]
    pub max_catchup_per_call: u32,
}

impl Default for WorldTicksSection {
    fn default() -> Self {
        Self {
            enabled: false,
            max_catchup_per_call: default_max_catchup_per_call(),
        }
    }
}

/// A single seeded faction definition (SPEC v3 §5.2).
///
/// Seeds are pure data: the runtime upserts them into the
/// `factions` table at startup (Task 6b) so game authors can edit
/// `assets/game.toml` without writing migrations. The fields mirror
/// the SPEC v3 §5.2 example one-to-one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactionSeed {
    /// URL-safe identifier; validated against the same slug rule as
    /// `[game].slug` so the value is safe to use as a primary key in
    /// the v3 `factions` table and stable across renames of the
    /// human-facing `display_name`.
    pub slug: String,
    /// Human-facing name shown on screens and in notices. Required
    /// (non-empty after trim) because every UI surface we plan to
    /// build for v3 renders it; an empty value would produce a
    /// blank menu entry.
    pub display_name: String,
    /// One-sentence flavour text shown on faction selection screens.
    /// Required for the same reason as `display_name`: the agency
    /// selector in `murder_motel` (Task 12) reads this verbatim.
    pub description: String,
}

fn default_world_path() -> String {
    "world/world.sqlite".to_string()
}
fn default_world_busy_timeout_ms() -> u64 {
    5_000
}
fn default_world_journal_mode() -> String {
    "wal".to_string()
}
fn default_max_catchup_per_call() -> u32 {
    100
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}
fn default_idle_timeout_ms() -> u64 {
    DEFAULT_IDLE_TIMEOUT_MS
}
fn default_visibility() -> String {
    DEFAULT_VISIBILITY.to_string()
}
fn default_auth_scope() -> String {
    DEFAULT_AUTH_SCOPE.to_string()
}

/// Errors raised while loading or validating a [`GameConfig`].
///
/// Library-internal `thiserror` per the PROMPT.md convention; the CLI
/// wraps this with `anyhow` at the boundary.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Could not read the config file from disk. Carries the path
    /// echoed back for an actionable error message.
    #[error("failed to read game config from `{path}`: {source}")]
    Read {
        /// Path that failed to read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// TOML failed to parse. Wraps the underlying `toml` error so the
    /// author still sees the line/column hint `toml` produces.
    #[error("failed to parse game config: {0}")]
    Parse(#[from] toml::de::Error),

    /// A field was structurally valid but semantically rejected
    /// (e.g. empty slug, zero `min_width`). Separate from `Parse` so
    /// CLI callers can distinguish "your TOML is malformed" from
    /// "your values violate the SPEC".
    #[error("invalid game config: {0}")]
    Validate(String),
}

impl GameConfig {
    /// Parse a [`GameConfig`] from a TOML string.
    ///
    /// Prefer this in tests and any caller that already has the file
    /// contents in memory. [`Self::load`] is the disk-aware wrapper.
    pub fn from_toml_str(s: &str) -> Result<Self, ConfigError> {
        let parsed: GameConfig = toml::from_str(s)?;
        parsed.validate()?;
        Ok(parsed)
    }

    /// Read and parse `assets/game.toml` from disk.
    ///
    /// Two-step error funnel: I/O first, then parsing+validation. We
    /// keep them separate so a missing file (common authoring mistake
    /// — forgot to scaffold) doesn't get reported as "TOML parse
    /// error".
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path_ref = path.as_ref();
        let raw = std::fs::read_to_string(path_ref).map_err(|source| ConfigError::Read {
            path: path_ref.display().to_string(),
            source,
        })?;
        Self::from_toml_str(&raw)
    }

    /// Re-emit the config as TOML.
    ///
    /// Used in tests to assert lossless round-tripping; the CLI never
    /// writes `game.toml` (authors own that file). Errors are
    /// deliberately not surfaced — `toml::to_string` only fails for
    /// types our schema doesn't expose.
    pub fn to_toml_string(&self) -> String {
        toml::to_string(self).expect("GameConfig serializes to TOML")
    }

    /// Cross-field validation that `serde` can't express.
    ///
    /// Kept in one place rather than scattered across field setters
    /// so the rules are discoverable next to [`GameConfig`].
    fn validate(&self) -> Result<(), ConfigError> {
        let g = &self.game;
        if g.title.trim().is_empty() {
            return Err(ConfigError::Validate(
                "[game].title must not be empty".into(),
            ));
        }
        if !is_valid_slug(&g.slug) {
            return Err(ConfigError::Validate(format!(
                "[game].slug `{}` must be lowercase alphanumeric with hyphens (e.g. `murder-motel`)",
                g.slug
            )));
        }
        if g.start_map.trim().is_empty() {
            return Err(ConfigError::Validate(
                "[game].start_map must not be empty".into(),
            ));
        }
        if g.min_width == 0 || g.min_height == 0 {
            return Err(ConfigError::Validate(format!(
                "[game].min_width and [game].min_height must be > 0 (got {}x{})",
                g.min_width, g.min_height
            )));
        }
        if self.manifest.visibility.trim().is_empty() {
            return Err(ConfigError::Validate(
                "[manifest].visibility must not be empty".into(),
            ));
        }
        if self.manifest.auth_scope.trim().is_empty() {
            return Err(ConfigError::Validate(
                "[manifest].auth_scope must not be empty".into(),
            ));
        }
        if let Some(turns) = &self.turns {
            // SPEC v2 §4.6 says "Initialize a player with today's
            // allowance" — an allowance of zero would create a game
            // where every action is rejected on day one, which is
            // almost certainly an authoring mistake. Negative values
            // are rejected earlier at parse time by `u32`.
            if turns.daily_allowance == 0 {
                return Err(ConfigError::Validate(
                    "[turns].daily_allowance must be greater than zero".into(),
                ));
            }
        }
        if self.world_ticks.max_catchup_per_call == 0 {
            return Err(ConfigError::Validate(
                "[world_ticks].max_catchup_per_call must be greater than zero".into(),
            ));
        }
        // Leaderboard names must be non-empty and unique. Task 8 will
        // key SQL rows by `name`, so a duplicate would silently merge
        // two boards that the author intended to keep separate, and an
        // empty name would produce ambiguous error messages downstream.
        let mut seen: Vec<&str> = Vec::with_capacity(self.leaderboards.len());
        for board in &self.leaderboards {
            if board.name.trim().is_empty() {
                return Err(ConfigError::Validate(
                    "[[leaderboards]].name must not be empty".into(),
                ));
            }
            if seen.contains(&board.name.as_str()) {
                return Err(ConfigError::Validate(format!(
                    "duplicate [[leaderboards]].name `{}`",
                    board.name
                )));
            }
            seen.push(board.name.as_str());
        }
        // Faction seeds: slug shape, non-empty display name and
        // description, and slug uniqueness. We validate even when
        // `multiplayer.factions` is `false` so an authoring mistake
        // surfaces the moment it lands in the file rather than the
        // first time someone flips the toggle on.
        let mut faction_slugs: Vec<&str> = Vec::with_capacity(self.factions.seed.len());
        for seed in &self.factions.seed {
            if !is_valid_slug(&seed.slug) {
                return Err(ConfigError::Validate(format!(
                    "[[factions.seed]].slug `{}` must be lowercase alphanumeric with hyphens",
                    seed.slug
                )));
            }
            if seed.display_name.trim().is_empty() {
                return Err(ConfigError::Validate(format!(
                    "[[factions.seed]] `{}` is missing a non-empty display_name",
                    seed.slug
                )));
            }
            if seed.description.trim().is_empty() {
                return Err(ConfigError::Validate(format!(
                    "[[factions.seed]] `{}` is missing a non-empty description",
                    seed.slug
                )));
            }
            if faction_slugs.contains(&seed.slug.as_str()) {
                return Err(ConfigError::Validate(format!(
                    "duplicate [[factions.seed]].slug `{}`",
                    seed.slug
                )));
            }
            faction_slugs.push(seed.slug.as_str());
        }
        Ok(())
    }
}

/// Slug rule: non-empty, lowercase ASCII alphanumeric or `-`, must
/// not start or end with `-`. Mirrors the constraint Foglet imposes
/// on door identifiers and what we eventually want safe to splice
/// into a filesystem path under `/srv/foglet/doors/<slug>/`.
fn is_valid_slug(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact TOML from SPEC §9.1. Kept verbatim so a future
    /// SPEC tweak that breaks compatibility shows up as a failing
    /// test, not a silent shift in field semantics.
    const SPEC_EXAMPLE: &str = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = "A tiny BBS mystery built with foglet-game-kit."
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 12
start_y = 8

[save]
strategy = "per_foglet_user"

[manifest]
timeout_ms = 1800000
idle_timeout_ms = 300000
visibility = "members"
auth_scope = "site"
"#;

    #[test]
    fn parses_spec_example() {
        let config = GameConfig::from_toml_str(SPEC_EXAMPLE).expect("SPEC §9.1 example parses");
        assert_eq!(config.game.title, "Murder Motel");
        assert_eq!(config.game.slug, "murder-motel");
        assert_eq!(config.game.min_width, 80);
        assert_eq!(config.game.min_height, 24);
        assert_eq!(config.game.start_map, "lobby");
        assert_eq!(config.game.start_x, 12);
        assert_eq!(config.game.start_y, 8);
        assert_eq!(config.save.strategy, SaveStrategy::PerFogletUser);
        assert_eq!(config.manifest.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(config.manifest.idle_timeout_ms, DEFAULT_IDLE_TIMEOUT_MS);
        assert_eq!(config.manifest.visibility, DEFAULT_VISIBILITY);
        assert_eq!(config.manifest.auth_scope, DEFAULT_AUTH_SCOPE);
    }

    #[test]
    fn round_trips_through_toml() {
        let config = GameConfig::from_toml_str(SPEC_EXAMPLE).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn applies_defaults_when_save_and_manifest_omitted() {
        let minimal = r#"
[game]
title = "Minimal"
slug = "minimal"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(minimal).unwrap();
        assert_eq!(config.save.strategy, SaveStrategy::PerFogletUser);
        assert_eq!(config.manifest.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(config.manifest.visibility, DEFAULT_VISIBILITY);
    }

    #[test]
    fn applies_field_level_defaults_in_partial_manifest_section() {
        // An author overrides only `timeout_ms` — the rest of
        // `[manifest]` should still come from SPEC §10.3 defaults.
        let partial = r#"
[game]
title = "Partial"
slug = "partial"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[manifest]
timeout_ms = 60000
"#;
        let config = GameConfig::from_toml_str(partial).unwrap();
        assert_eq!(config.manifest.timeout_ms, 60_000);
        assert_eq!(config.manifest.idle_timeout_ms, DEFAULT_IDLE_TIMEOUT_MS);
        assert_eq!(config.manifest.visibility, DEFAULT_VISIBILITY);
        assert_eq!(config.manifest.auth_scope, DEFAULT_AUTH_SCOPE);
    }

    #[test]
    fn missing_required_field_is_a_clear_parse_error() {
        // No `slug` — `serde` should report it. We assert on
        // `Parse`, not `Validate`, because shape errors land before
        // semantic validation.
        let bad = r#"
[game]
title = "No Slug"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
        let msg = format!("{err}");
        assert!(msg.contains("slug"), "error should mention `slug`: {msg}");
    }

    #[test]
    fn unknown_save_strategy_is_a_parse_error() {
        // `SaveStrategy` is intentionally a closed enum.
        let bad = r#"
[game]
title = "X"
slug = "x"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[save]
strategy = "every_full_moon"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn invalid_slug_is_rejected_with_validate() {
        let bad = r#"
[game]
title = "Caps"
slug = "Murder_Motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => {
                assert!(msg.contains("slug"), "{msg}");
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn zero_min_size_is_rejected() {
        let bad = r#"
[game]
title = "Zero"
slug = "zero"
description = ""
min_width = 0
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn empty_title_is_rejected() {
        let bad = r#"
[game]
title = "   "
slug = "empty"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Validate(_)));
    }

    #[test]
    fn malformed_toml_is_parse_error() {
        let err = GameConfig::from_toml_str("this is not toml = =").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn load_from_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.toml");
        std::fs::write(&path, SPEC_EXAMPLE).unwrap();
        let config = GameConfig::load(&path).unwrap();
        assert_eq!(config.game.slug, "murder-motel");
    }

    #[test]
    fn load_missing_file_is_read_error() {
        let err = GameConfig::load("/no/such/path/game.toml").unwrap_err();
        match err {
            ConfigError::Read { path, .. } => {
                assert!(path.contains("game.toml"));
            }
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[test]
    fn slug_validator_accepts_canonical_examples() {
        assert!(is_valid_slug("murder-motel"));
        assert!(is_valid_slug("door1"));
        assert!(is_valid_slug("a"));
    }

    #[test]
    fn absent_world_section_defaults_to_disabled() {
        // SPEC v2 §5: a v1 project without `[world]` MUST continue to
        // work, with the world layer effectively off.
        let v1_style = r#"
[game]
title = "V1 Game"
slug = "v1-game"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(v1_style).unwrap();
        assert!(!config.world.enabled);
        // Defaults still get populated so downstream consumers never
        // have to special-case "disabled".
        assert_eq!(config.world.path, "world/world.sqlite");
        assert_eq!(config.world.busy_timeout_ms, 5_000);
        assert_eq!(config.world.journal_mode, "wal");
    }

    #[test]
    fn absent_v4_sections_are_disabled_by_default() {
        // SPEC_v4 Task 2a: all v4 sections are optional and safe to
        // omit; every `enabled` flag must default to false.
        let config = GameConfig::from_toml_str(
            r#"
[game]
title = "Legacy"
slug = "legacy"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#,
        )
        .unwrap();
        assert!(!config.spatial.enabled);
        assert!(!config.presence.enabled);
        assert!(!config.place_recall.enabled);
        assert!(!config.inventory.enabled);
        assert!(!config.world_ticks.enabled);
        assert_eq!(config.world_ticks.max_catchup_per_call, 100);
    }

    #[test]
    fn parses_v4_sections_with_explicit_enabled_flags() {
        // Explicitly declared sections must parse as their payloads.
        let config = GameConfig::from_toml_str(
            r#"
[game]
title = "V4"
slug = "v4"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[spatial]
enabled = true

[presence]
enabled = true

[place_recall]
enabled = true

[inventory]
enabled = true

[world_ticks]
enabled = true
"#,
        )
        .unwrap();
        assert!(config.spatial.enabled);
        assert!(config.presence.enabled);
        assert!(config.place_recall.enabled);
        assert!(config.inventory.enabled);
        assert!(config.world_ticks.enabled);
        assert_eq!(config.world_ticks.max_catchup_per_call, 100);
    }

    #[test]
    fn parses_world_ticks_with_default_catchup_bound_when_omitted() {
        // Task 2b: if the catch-up key is omitted, apply the
        // documented default so catch-up remains bounded.
        let config = GameConfig::from_toml_str(
            r#"
[game]
title = "Bounded"
slug = "bounded"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world_ticks]
enabled = true
"#,
        )
        .unwrap();
        assert!(config.world_ticks.enabled);
        assert_eq!(config.world_ticks.max_catchup_per_call, 100);
    }

    #[test]
    fn world_ticks_rejects_zero_catchup_bound() {
        // Zero is explicitly invalid because it guarantees no progress
        // through the due-task queue.
        let err = GameConfig::from_toml_str(
            r#"
[game]
title = "Zero Catchup"
slug = "zero-catchup"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world_ticks]
enabled = true
max_catchup_per_call = 0
"#,
        )
        .unwrap_err();
        match err {
            ConfigError::Validate(msg) => {
                assert!(
                    msg.contains("max_catchup_per_call"),
                    "error should mention field: {msg}"
                );
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn world_ticks_rejects_negative_catchup_bound() {
        // Negative values must fail during parsing so authors can
        // correct config before runtime.
        let err = GameConfig::from_toml_str(
            r#"
[game]
title = "Negative Catchup"
slug = "negative-catchup"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world_ticks]
enabled = true
max_catchup_per_call = -1
"#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
        let msg = format!("{err}");
        assert!(
            msg.contains("max_catchup_per_call") || msg.contains("invalid"),
            "error should be informative: {msg}"
        );
    }

    #[test]
    fn parses_full_world_section_from_spec_example() {
        // Verbatim from SPEC v2 §5 so a future SPEC tweak surfaces as
        // a failing test rather than silent drift.
        let v2 = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world]
enabled = true
path = "world/world.sqlite"
busy_timeout_ms = 5000
journal_mode = "wal"
"#;
        let config = GameConfig::from_toml_str(v2).unwrap();
        assert!(config.world.enabled);
        assert_eq!(config.world.path, "world/world.sqlite");
        assert_eq!(config.world.busy_timeout_ms, 5_000);
        assert_eq!(config.world.journal_mode, "wal");
    }

    #[test]
    fn world_section_applies_field_level_defaults() {
        // Author opts in but only sets `enabled` — every other field
        // should fall back to the SPEC-documented default.
        let partial = r#"
[game]
title = "Partial World"
slug = "partial-world"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world]
enabled = true
"#;
        let config = GameConfig::from_toml_str(partial).unwrap();
        assert!(config.world.enabled);
        assert_eq!(config.world.path, "world/world.sqlite");
        assert_eq!(config.world.busy_timeout_ms, 5_000);
        assert_eq!(config.world.journal_mode, "wal");
    }

    #[test]
    fn world_section_round_trips_through_toml() {
        let original = r#"
[game]
title = "RT World"
slug = "rt-world"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[world]
enabled = true
path = "data/shared.sqlite"
busy_timeout_ms = 7500
journal_mode = "delete"
"#;
        let config = GameConfig::from_toml_str(original).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
        assert_eq!(reparsed.world.path, "data/shared.sqlite");
        assert_eq!(reparsed.world.busy_timeout_ms, 7_500);
        assert_eq!(reparsed.world.journal_mode, "delete");
    }

    #[test]
    fn absent_turns_section_means_no_turn_system() {
        // SPEC v2 §5: "Missing `[turns]` means no turn system." We
        // must not synthesize a default — `None` is the off signal the
        // runtime checks before opening a ledger.
        let v1_style = r#"
[game]
title = "No Turns"
slug = "no-turns"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(v1_style).unwrap();
        assert!(config.turns.is_none());
    }

    #[test]
    fn parses_full_turns_section_from_spec_example() {
        // Verbatim from SPEC v2 §5 so a future SPEC tweak surfaces as
        // a failing test rather than silent drift.
        let v2 = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = 30
reset = "local_midnight"
carryover_max = 10
"#;
        let config = GameConfig::from_toml_str(v2).unwrap();
        let turns = config.turns.expect("turns section parsed");
        assert_eq!(turns.daily_allowance, 30);
        assert_eq!(turns.reset, TurnReset::LocalMidnight);
        assert_eq!(turns.carryover_max, 10);
    }

    #[test]
    fn turns_section_applies_field_level_defaults() {
        // Author opts in but only sets the required `daily_allowance`
        // — `reset` and `carryover_max` should fall back to defaults.
        let partial = r#"
[game]
title = "Partial Turns"
slug = "partial-turns"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = 5
"#;
        let config = GameConfig::from_toml_str(partial).unwrap();
        let turns = config.turns.expect("turns section parsed");
        assert_eq!(turns.daily_allowance, 5);
        assert_eq!(turns.reset, TurnReset::LocalMidnight);
        assert_eq!(turns.carryover_max, 0);
    }

    #[test]
    fn turns_section_rejects_zero_allowance_with_validate() {
        // Zero would create a game where every action is rejected on
        // day one. We surface this as `Validate` so the author sees
        // a SPEC-level message, not a generic parse error.
        let bad = r#"
[game]
title = "Zero Turns"
slug = "zero-turns"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = 0
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => {
                assert!(msg.contains("daily_allowance"), "{msg}");
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn turns_section_rejects_negative_allowance_at_parse_time() {
        // `u32` rejects negatives at parse time. We assert the field
        // name lands in the error so the author knows where to look.
        let bad = r#"
[game]
title = "Negative"
slug = "negative"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = -1
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
        let msg = format!("{err}");
        assert!(
            msg.contains("daily_allowance") || msg.contains("invalid"),
            "error should be informative: {msg}"
        );
    }

    #[test]
    fn turns_section_rejects_unknown_reset_cadence() {
        // `TurnReset` is intentionally a closed enum so an
        // unrecognised cadence fails loudly rather than silently
        // disabling resets.
        let bad = r#"
[game]
title = "Bad Reset"
slug = "bad-reset"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = 10
reset = "every_full_moon"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn turns_section_round_trips_through_toml() {
        let original = r#"
[game]
title = "RT Turns"
slug = "rt-turns"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[turns]
daily_allowance = 25
reset = "local_midnight"
carryover_max = 7
"#;
        let config = GameConfig::from_toml_str(original).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
        let turns = reparsed.turns.unwrap();
        assert_eq!(turns.daily_allowance, 25);
        assert_eq!(turns.carryover_max, 7);
    }

    #[test]
    fn absent_leaderboards_means_no_built_in_boards() {
        // SPEC v2 §5: "Missing `[[leaderboards]]` means no built-in
        // leaderboards." We model that as an empty vec rather than an
        // `Option`, so consumers iterate uniformly.
        let v1_style = r#"
[game]
title = "No Boards"
slug = "no-boards"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(v1_style).unwrap();
        assert!(config.leaderboards.is_empty());
    }

    #[test]
    fn parses_full_leaderboards_section_from_spec_example() {
        // Verbatim from SPEC v2 §5 so a future SPEC tweak surfaces as
        // a failing test rather than silent drift.
        let v2 = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
sort = "desc"
"#;
        let config = GameConfig::from_toml_str(v2).unwrap();
        assert_eq!(config.leaderboards.len(), 1);
        assert_eq!(config.leaderboards[0].name, "investigators");
        assert_eq!(config.leaderboards[0].sort, LeaderboardSort::Desc);
    }

    #[test]
    fn leaderboard_sort_defaults_to_desc_when_omitted() {
        // The typical scoreboard ranks highest-first; an author who
        // omits `sort` should get that without ceremony.
        let partial = r#"
[game]
title = "Default Sort"
slug = "default-sort"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
"#;
        let config = GameConfig::from_toml_str(partial).unwrap();
        assert_eq!(config.leaderboards[0].sort, LeaderboardSort::Desc);
    }

    #[test]
    fn parses_multiple_leaderboards_with_mixed_sort() {
        let multi = r#"
[game]
title = "Multi"
slug = "multi"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
sort = "desc"

[[leaderboards]]
name = "fastest_solve"
sort = "asc"
"#;
        let config = GameConfig::from_toml_str(multi).unwrap();
        assert_eq!(config.leaderboards.len(), 2);
        assert_eq!(config.leaderboards[1].name, "fastest_solve");
        assert_eq!(config.leaderboards[1].sort, LeaderboardSort::Asc);
    }

    #[test]
    fn duplicate_leaderboard_names_are_rejected() {
        // Task 8 will key SQL rows by `name` — a duplicate would
        // silently merge boards the author meant to keep separate.
        let bad = r#"
[game]
title = "Dup"
slug = "dup"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
sort = "desc"

[[leaderboards]]
name = "investigators"
sort = "asc"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => {
                assert!(msg.contains("investigators"), "{msg}");
                assert!(msg.contains("duplicate"), "{msg}");
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn empty_leaderboard_name_is_rejected() {
        let bad = r#"
[game]
title = "Empty Name"
slug = "empty-name"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = ""
sort = "desc"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Validate(_)), "got {err:?}");
    }

    #[test]
    fn unknown_leaderboard_sort_is_a_parse_error() {
        // `LeaderboardSort` is intentionally a closed enum.
        let bad = r#"
[game]
title = "Bad Sort"
slug = "bad-sort"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
sort = "sideways"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn leaderboards_round_trip_through_toml() {
        let original = r#"
[game]
title = "RT Boards"
slug = "rt-boards"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[leaderboards]]
name = "investigators"
sort = "desc"

[[leaderboards]]
name = "fastest_solve"
sort = "asc"
"#;
        let config = GameConfig::from_toml_str(original).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
        assert_eq!(reparsed.leaderboards.len(), 2);
    }

    #[test]
    fn absent_multiplayer_section_disables_all_primitives() {
        // SPEC v3 §5.2: "Every primitive is opt-in" and v1/v2 projects
        // must not be forced into multiplayer. We model that as
        // `None` — the off signal the runtime checks before wiring any
        // v3 primitive.
        let v1_style = r#"
[game]
title = "No Multiplayer"
slug = "no-multiplayer"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(v1_style).unwrap();
        assert!(config.multiplayer.is_none());
    }

    #[test]
    fn parses_full_multiplayer_section_from_spec_example() {
        // Verbatim from SPEC v3 §5.2 (minus the [[factions.seed]]
        // block, which lands in Task 2b). A future SPEC tweak that
        // breaks compatibility shows up as a failing test.
        let v3 = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[multiplayer]
notices = true
challenges = true
market = true
factions = true
bounties = true
max_notice_body_chars = 1000
"#;
        let config = GameConfig::from_toml_str(v3).unwrap();
        let mp = config.multiplayer.expect("multiplayer section parsed");
        assert!(mp.notices);
        assert!(mp.challenges);
        assert!(mp.market);
        assert!(mp.factions);
        assert!(mp.bounties);
        assert_eq!(mp.max_notice_body_chars, 1_000);
    }

    #[test]
    fn multiplayer_toggles_default_to_false_when_omitted() {
        // Opting into `[multiplayer]` to set one flag must not silently
        // light up the others — each primitive is independently opt-in.
        let partial = r#"
[game]
title = "Just Notices"
slug = "just-notices"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[multiplayer]
notices = true
"#;
        let config = GameConfig::from_toml_str(partial).unwrap();
        let mp = config.multiplayer.expect("multiplayer section parsed");
        assert!(mp.notices);
        assert!(!mp.challenges);
        assert!(!mp.market);
        assert!(!mp.factions);
        assert!(!mp.bounties);
        // Default cap kicks in even when only `notices` is set.
        assert_eq!(mp.max_notice_body_chars, 1_000);
    }

    #[test]
    fn multiplayer_section_round_trips_through_toml() {
        let original = r#"
[game]
title = "RT MP"
slug = "rt-mp"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[multiplayer]
notices = true
challenges = false
market = true
factions = true
bounties = false
max_notice_body_chars = 500
"#;
        let config = GameConfig::from_toml_str(original).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
        let mp = reparsed.multiplayer.unwrap();
        assert_eq!(mp.max_notice_body_chars, 500);
        assert!(mp.market);
        assert!(!mp.bounties);
    }

    #[test]
    fn multiplayer_section_rejects_negative_char_cap_at_parse_time() {
        // `u32` rejects negatives at parse time — the field name should
        // land in the error so the author knows where to look.
        let bad = r#"
[game]
title = "Negative Cap"
slug = "negative-cap"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[multiplayer]
notices = true
max_notice_body_chars = -1
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn absent_factions_section_yields_empty_seed_list() {
        // SPEC v3 §5.2: seeded factions are opt-in. v1/v2 games and v3
        // games that ship with no agencies must parse without ceremony.
        let v1_style = r#"
[game]
title = "No Factions"
slug = "no-factions"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0
"#;
        let config = GameConfig::from_toml_str(v1_style).unwrap();
        assert!(config.factions.seed.is_empty());
    }

    #[test]
    fn parses_factions_seed_array_from_spec_example() {
        // Verbatim shape from SPEC v3 §5.2; pinned so a SPEC tweak that
        // breaks compatibility surfaces here.
        let v3 = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[factions.seed]]
slug = "blue-desk"
display_name = "Blue Desk Agency"
description = "Investigators who trust paperwork more than hunches."

[[factions.seed]]
slug = "red-room"
display_name = "Red Room Agency"
description = "Investigators who chase hunches into smoky alleys."
"#;
        let config = GameConfig::from_toml_str(v3).unwrap();
        assert_eq!(config.factions.seed.len(), 2);
        assert_eq!(config.factions.seed[0].slug, "blue-desk");
        assert_eq!(config.factions.seed[0].display_name, "Blue Desk Agency");
        assert!(config.factions.seed[0]
            .description
            .starts_with("Investigators"));
        assert_eq!(config.factions.seed[1].slug, "red-room");
    }

    #[test]
    fn factions_seed_round_trips_through_toml() {
        let original = r#"
[game]
title = "RT Factions"
slug = "rt-factions"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[factions.seed]]
slug = "alpha"
display_name = "Alpha Squad"
description = "Order of operations."

[[factions.seed]]
slug = "beta"
display_name = "Beta Squad"
description = "Counterpoint."
"#;
        let config = GameConfig::from_toml_str(original).unwrap();
        let serialized = config.to_toml_string();
        let reparsed = GameConfig::from_toml_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn duplicate_faction_slug_is_rejected() {
        // Task 2b's named acceptance: two seeds with the same slug
        // would silently merge into a single row at upsert time, so we
        // catch it at config load with a clearly-attributed error.
        let bad = r#"
[game]
title = "Dup Slugs"
slug = "dup-slugs"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[factions.seed]]
slug = "blue-desk"
display_name = "Blue Desk Agency"
description = "First."

[[factions.seed]]
slug = "blue-desk"
display_name = "Other Blue Desk"
description = "Second."
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => {
                assert!(msg.contains("blue-desk"), "{msg}");
                assert!(msg.contains("duplicate"), "{msg}");
            }
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn invalid_faction_slug_is_rejected() {
        // Slug shape rule must apply to seeded factions too; otherwise
        // an UPPERCASE slug ends up in the world DB and breaks any
        // path-style consumer downstream.
        let bad = r#"
[game]
title = "Bad Slug"
slug = "bad-slug"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[factions.seed]]
slug = "Blue_Desk"
display_name = "Bad"
description = "Bad."
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => assert!(msg.contains("Blue_Desk"), "{msg}"),
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn empty_faction_display_name_is_rejected() {
        let bad = r#"
[game]
title = "Empty Name"
slug = "empty-name"
description = ""
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 0
start_y = 0

[[factions.seed]]
slug = "blue-desk"
display_name = "   "
description = "anything"
"#;
        let err = GameConfig::from_toml_str(bad).unwrap_err();
        match err {
            ConfigError::Validate(msg) => assert!(msg.contains("display_name"), "{msg}"),
            other => panic!("expected Validate, got {other:?}"),
        }
    }

    #[test]
    fn slug_validator_rejects_bad_inputs() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-leading"));
        assert!(!is_valid_slug("trailing-"));
        assert!(!is_valid_slug("UPPER"));
        assert!(!is_valid_slug("under_score"));
        assert!(!is_valid_slug("white space"));
    }
}
