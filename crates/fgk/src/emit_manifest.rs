//! `fgk emit-manifest` — generate Foglet operator manifest JSON.
//!
//! Given a project's `assets/game.toml` and an absolute
//! `--install-dir`, emit the Foglet operator manifest JSON. This
//! module is the library half of that subcommand; `main.rs` is the
//! thin operator-facing wrapper.
//!
//! ## Why this is library code
//!
//! The same shape we emit on the CLI is used by `fgk package` when it
//! writes `manifest.json` next to the binary. Putting the logic here
//! means `package` can call [`emit_manifest_json`] directly instead of
//! shelling out to itself, and unit tests can drive both call sites
//! without `assert_cmd`-ing the binary for every assertion.
//!
//! ## Inputs and validation
//!
//! - `project_dir`: path to a scaffolded project; we read
//!   `<project_dir>/assets/game.toml` from it. Defaults to `.` at
//!   the CLI layer.
//! - `install_dir`: where the door will live on the Foglet host
//!   (typically `/srv/foglet/doors/<slug>`). MUST be absolute — the
//!   manifest's `command` and `working_dir` are derived from it, and
//!   relative paths would resolve against whatever CWD Foglet
//!   happens to be in.
//!
//! ## What we override vs. inherit
//!
//! [`foglet_game::FogletManifest::new`] fills in default timeouts,
//! visibility, auth_scope, env, env_allowlist, and pty values. The
//! author's `[manifest]` section in `assets/game.toml` may tighten any
//! of those — we apply those overrides here rather than teaching
//! `FogletManifest::new` about `GameConfig`, so the manifest crate
//! stays usable from contexts that don't have a TOML config
//! (e.g. ad-hoc tests, future programmatic callers).

use std::path::{Path, PathBuf};

use foglet_game::{ConfigError, FogletManifest, GameConfig, ManifestError, ManifestInputs};

/// Default location of the game config relative to a project root.
///
/// Hard-coded rather than derived from `GameConfig` because the path
/// *is* the contract — `fgk` and the runtime both look here, and a
/// project that ships its config elsewhere isn't `fgk`-compatible.
pub const GAME_TOML_RELATIVE: &str = "assets/game.toml";

/// Errors raised while assembling a manifest.
///
/// Library-internal `thiserror` per the project convention; the CLI
/// boundary in `main.rs` converts to `anyhow::Error` via `?`.
#[derive(Debug, thiserror::Error)]
pub enum EmitManifestError {
    /// `--install-dir` was not an absolute path. We catch this here
    /// (in addition to the manifest crate's own check) so the user
    /// gets the error before we try to open `assets/game.toml` —
    /// fail-fast on the clearly-malformed input.
    #[error("--install-dir must be an absolute path (starts with `/`); got `{0}`")]
    InstallDirNotAbsolute(String),

    /// Failed to load `assets/game.toml`. Echoes the path so the
    /// operator knows which project we tried to read.
    #[error("failed to load game config from `{path}`: {source}")]
    Config {
        /// The full path we tried to read.
        path: String,
        /// Underlying parse/IO error from the foglet_game crate.
        #[source]
        source: ConfigError,
    },

    /// The manifest crate's own validation rejected the result. In
    /// practice this fires when an author manages to put an invalid
    /// override in `[manifest]` — e.g. an empty visibility string.
    #[error("manifest validation failed: {0}")]
    Manifest(#[from] ManifestError),
}

/// Build a [`FogletManifest`] for the given project + install dir.
///
/// Returns the typed manifest so callers can mutate it further (e.g.
/// `fgk package` may want to set additional env vars before writing
/// the file). For the simple CLI flow, prefer [`emit_manifest_json`]
/// which goes straight to the on-disk pretty-printed form.
pub fn build_manifest(
    project_dir: &Path,
    install_dir: &str,
) -> Result<FogletManifest, EmitManifestError> {
    if !install_dir.starts_with('/') {
        return Err(EmitManifestError::InstallDirNotAbsolute(
            install_dir.to_string(),
        ));
    }

    let config_path = config_path(project_dir);
    let config = GameConfig::load(&config_path).map_err(|source| EmitManifestError::Config {
        path: config_path.display().to_string(),
        source,
    })?;

    let mut manifest = FogletManifest::new(ManifestInputs {
        slug: &config.game.slug,
        display_name: &config.game.title,
        description: &config.game.description,
        install_dir,
    })?;

    // Apply `[manifest]` overrides from the project's TOML. The
    // defaults baked into `FogletManifest::new` mirror the canonical
    // example, so a config that omits the section produces the same
    // bytes either way; this only matters when an author has pinned
    // tighter timeouts or a different visibility/scope.
    manifest.timeout_ms = config.manifest.timeout_ms;
    manifest.idle_timeout_ms = config.manifest.idle_timeout_ms;
    manifest.visibility = config.manifest.visibility.clone();
    manifest.auth_scope = config.manifest.auth_scope.clone();

    // Re-validate after overrides — `validate()` is cheap and catches
    // the case where an author's TOML wrote an empty override that
    // bypassed our config-level checks (defense in depth).
    manifest.validate()?;
    Ok(manifest)
}

/// Build a manifest and serialize it to the on-disk pretty-printed
/// form (trailing newline, sorted env keys). This is what
/// `fgk emit-manifest` writes to stdout and what `fgk package`
/// writes next to the binary.
pub fn emit_manifest_json(
    project_dir: &Path,
    install_dir: &str,
) -> Result<String, EmitManifestError> {
    let manifest = build_manifest(project_dir, install_dir)?;
    // `to_json_pretty` only fails for serde types we don't expose;
    // unwrap-via-expect makes the impossibility visible to readers.
    Ok(manifest
        .to_json_pretty()
        .expect("FogletManifest serializes to JSON"))
}

/// Resolve `<project_dir>/assets/game.toml`. Pulled out as a helper
/// so tests can assert the path independently of the file existing.
fn config_path(project_dir: &Path) -> PathBuf {
    project_dir.join(GAME_TOML_RELATIVE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Canonical game config fixture — replicated here so the test
    /// is self-contained.
    const CANONICAL_GAME_TOML: &str = r#"
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

    /// Materialize a project tree containing only `assets/game.toml`
    /// — enough for the manifest pipeline; nothing more.
    fn project_with_config(toml: &str) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        let assets = dir.path().join("assets");
        fs::create_dir_all(&assets).expect("mkdir assets");
        fs::write(assets.join("game.toml"), toml).expect("write game.toml");
        dir
    }

    #[test]
    fn emits_canonical_manifest_shape() {
        let project = project_with_config(CANONICAL_GAME_TOML);
        let json = emit_manifest_json(project.path(), "/srv/foglet/doors/murder-motel")
            .expect("manifest emits");

        let v: serde_json::Value = serde_json::from_str(&json).expect("emitted JSON parses");
        assert_eq!(v["id"], "murder-motel");
        assert_eq!(v["slug"], "murder-motel");
        assert_eq!(v["display_name"], "Murder Motel");
        assert_eq!(v["runtime"], "external_pty");
        assert_eq!(v["command"], "/srv/foglet/doors/murder-motel/run.sh");
        assert_eq!(v["working_dir"], "/srv/foglet/doors/murder-motel");
        assert_eq!(v["timeout_ms"], 1_800_000);
        assert_eq!(v["idle_timeout_ms"], 300_000);
        assert_eq!(v["visibility"], "members");
        assert_eq!(v["auth_scope"], "site");
        assert_eq!(v["pty"], true);
        assert!(json.ends_with('\n'), "operators expect a trailing newline");
    }

    #[test]
    fn rejects_relative_install_dir() {
        let project = project_with_config(CANONICAL_GAME_TOML);
        let err = emit_manifest_json(project.path(), "relative/path")
            .expect_err("relative path must be rejected");
        match err {
            EmitManifestError::InstallDirNotAbsolute(value) => {
                assert_eq!(value, "relative/path");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_install_dir() {
        let project = project_with_config(CANONICAL_GAME_TOML);
        let err = emit_manifest_json(project.path(), "").expect_err("empty path must be rejected");
        assert!(matches!(err, EmitManifestError::InstallDirNotAbsolute(_)));
    }

    #[test]
    fn applies_manifest_overrides_from_config() {
        let toml = r#"
[game]
title = "Tight Timeouts"
slug = "tight-timeouts"
description = "Pin shorter caps than the defaults."
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 1
start_y = 1

[manifest]
timeout_ms = 600000
idle_timeout_ms = 60000
visibility = "public"
auth_scope = "none"
"#;
        let project = project_with_config(toml);
        let manifest = build_manifest(project.path(), "/srv/foglet/doors/tight-timeouts")
            .expect("manifest builds");

        assert_eq!(manifest.timeout_ms, 600_000);
        assert_eq!(manifest.idle_timeout_ms, 60_000);
        assert_eq!(manifest.visibility, "public");
        assert_eq!(manifest.auth_scope, "none");
    }

    #[test]
    fn inherits_defaults_when_manifest_section_omitted() {
        // [manifest] is absent — the loader fills in defaults at the
        // field level, so the emitted shape matches the canonical output.
        let toml = r#"
[game]
title = "Murder Motel"
slug = "murder-motel"
description = "A tiny BBS mystery built with foglet-game-kit."
min_width = 80
min_height = 24
start_map = "lobby"
start_x = 12
start_y = 8
"#;
        let project = project_with_config(toml);
        let manifest =
            build_manifest(project.path(), "/srv/foglet/doors/murder-motel").expect("builds");

        assert_eq!(manifest.timeout_ms, 1_800_000);
        assert_eq!(manifest.idle_timeout_ms, 300_000);
        assert_eq!(manifest.visibility, "members");
        assert_eq!(manifest.auth_scope, "site");
    }

    #[test]
    fn surfaces_missing_config_with_path_in_error() {
        let dir = TempDir::new().expect("tempdir");
        let err = emit_manifest_json(dir.path(), "/srv/foglet/doors/x")
            .expect_err("missing game.toml must error");
        match err {
            EmitManifestError::Config { path, .. } => {
                assert!(
                    path.ends_with("assets/game.toml"),
                    "error path should point at assets/game.toml, got `{path}`"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn surfaces_malformed_config_as_config_error() {
        let project = project_with_config("not = valid = toml");
        let err = emit_manifest_json(project.path(), "/srv/foglet/doors/x")
            .expect_err("garbage TOML must error");
        assert!(matches!(err, EmitManifestError::Config { .. }));
    }

    /// `--install-dir` with a trailing slash is normalized by the
    /// manifest crate; sanity-check that the CLI flow still produces
    /// the canonical (slash-free) form so operators don't get
    /// double-slashed paths in `command`.
    #[test]
    fn normalizes_trailing_slash_on_install_dir() {
        let project = project_with_config(CANONICAL_GAME_TOML);
        let manifest = build_manifest(project.path(), "/srv/foglet/doors/murder-motel/")
            .expect("trailing slash is fine");
        assert_eq!(manifest.command, "/srv/foglet/doors/murder-motel/run.sh");
        assert_eq!(manifest.working_dir, "/srv/foglet/doors/murder-motel");
    }
}
