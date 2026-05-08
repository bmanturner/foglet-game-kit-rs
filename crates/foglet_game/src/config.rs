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
    fn slug_validator_rejects_bad_inputs() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-leading"));
        assert!(!is_valid_slug("trailing-"));
        assert!(!is_valid_slug("UPPER"));
        assert!(!is_valid_slug("under_score"));
        assert!(!is_valid_slug("white space"));
    }
}
