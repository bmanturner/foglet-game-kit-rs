//! Foglet door identity + terminal metadata loader.
//!
//! Foglet launches `:external_pty` doors with a `FOGLET_DOOR_CONTEXT`
//! environment variable that points to a JSON file on disk. That file
//! is the **only** trustworthy source of door/user identity inside the
//! game (SPEC §2.4 forbids reading inherited host environment for any
//! other purpose). This module parses that file into a typed
//! [`FogletContext`].
//!
//! The loader is split across three sub-tasks:
//!
//! - **Task 2a** — the [`FogletContext`] type and the
//!   `FOGLET_DOOR_CONTEXT` path → file → parse → typed value path.
//! - **Task 2b (this commit)** — env-var fallback (`FOGLET_DOOR_ID`,
//!   etc.) when the context file is absent, plus the top-level
//!   [`load_context`] orchestrator that enforces the SPEC §5.1
//!   precedence rule "`FOGLET_DOOR_CONTEXT` JSON MUST win over
//!   individual env vars".
//! - **Task 2c** — local-dev synthesis when neither the file nor the
//!   env vars are present, plus `--local-dev-fallback` semantics for
//!   malformed JSON.
//!
//! All env-var inspection in this module goes through a caller-supplied
//! `Fn(&str) -> Option<String>` closure rather than touching
//! `std::env::var` directly. This is deliberate: process-environment
//! mutation in tests is racy under `cargo test`'s default thread pool,
//! and the orchestrator's whole job is "what does the env look like
//! right now?". A closure makes that question pure and the tests
//! parallel-safe. Callers who genuinely want process env can pass
//! [`process_env`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Where a loaded [`FogletContext`] originated from.
///
/// Captured on the context itself so downstream code can branch on
/// "are we running under Foglet right now or not?" without having to
/// re-inspect the environment. The save manager (Task 8) needs this to
/// pick between the per-user production path and the local-dev path,
/// and the runtime uses it to decide whether to emit user-visible
/// "running in local-dev mode" hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// Loaded from the JSON file pointed at by `FOGLET_DOOR_CONTEXT`.
    /// This is the only source Foglet itself produces in production.
    ContextFile,
    /// Synthesised from individual `FOGLET_*` environment variables.
    /// Used when running directly in a shell without Foglet wrapping
    /// the door (Task 2b).
    Env,
    /// Synthesised defaults — no Foglet context at all (Task 2c).
    LocalDev,
}

/// Normalised Foglet door identity + terminal metadata.
///
/// Mirrors the field set described in SPEC §5.1. The struct is what
/// game code consumes; the on-disk JSON shape is documented in
/// SPEC §2.4. Optional fields stay `None` when Foglet does not supply
/// them — per SPEC §5.1 the loader MUST NOT fail on missing optionals.
///
/// `username` accepts either `username` or `handle` as the JSON key
/// because SPEC §2.4 documents the JSON example with `handle` while
/// §5.1 names the typed field `username`. The serde alias bridges
/// that ambiguity so we are robust to whichever spelling Foglet
/// actually emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogletContext {
    /// Stable identifier for the door instance. Always present.
    pub door_id: String,

    /// Foglet user id. Optional — Foglet does not always populate it
    /// (e.g. anonymous-access doors).
    #[serde(default)]
    pub user_id: Option<String>,

    /// Display handle / username. Accepts the JSON key `handle` as
    /// an alias to match the example in SPEC §2.4.
    #[serde(default, alias = "handle")]
    pub username: Option<String>,

    /// Foglet-side role string (e.g. `"sysop"`). Games SHOULD treat
    /// this as advisory, not as an authorization decision.
    #[serde(default)]
    pub role: Option<String>,

    /// Foglet session id. Useful for correlating game logs with
    /// Foglet-side activity logs but never required.
    #[serde(default)]
    pub session_id: Option<String>,

    /// Terminal width in columns. Foglet always populates this; the
    /// runtime cross-checks it against the game's declared minimum
    /// before entering raw mode (SPEC §7.1).
    pub terminal_width: u16,

    /// Terminal height in rows. Same contract as
    /// [`Self::terminal_width`].
    pub terminal_height: u16,

    /// Where this context came from. Set by the loader, never read
    /// from JSON (the `#[serde(default)]` lets older context files
    /// without the field still parse cleanly).
    #[serde(default = "default_source")]
    pub source: ContextSource,
}

/// Default source for any [`FogletContext`] deserialised without an
/// explicit `source` field. Foglet's emitted JSON does not include a
/// `source` key, so deserialising from `FOGLET_DOOR_CONTEXT` must
/// default to [`ContextSource::ContextFile`]. Other code paths
/// override this after parsing.
fn default_source() -> ContextSource {
    ContextSource::ContextFile
}

/// Errors produced while loading a [`FogletContext`].
///
/// Library-internal (`thiserror`); the CLI converts these to
/// `anyhow::Error` at the process boundary (PROMPT.md crate budget).
#[derive(Debug, Error)]
pub enum ContextError {
    /// `FOGLET_DOOR_CONTEXT` named a path that could not be read.
    /// Wraps the underlying I/O error so callers can distinguish
    /// "missing file" from "permission denied" without parsing the
    /// display string.
    #[error("failed to read Foglet context file at {path}: {source}")]
    ReadFile {
        /// The path Foglet asked us to read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The bytes at `FOGLET_DOOR_CONTEXT` were not valid JSON, or did
    /// not match the [`FogletContext`] shape. Hit on every "garbage in
    /// the env var path" case; SPEC §5.1 wants a clear error here
    /// unless `--local-dev-fallback` is set (handled in Task 2c, not
    /// at this layer).
    #[error("failed to parse Foglet context file at {path}: {source}")]
    ParseFile {
        /// The path the bad JSON came from.
        path: PathBuf,
        /// The underlying serde error.
        #[source]
        source: serde_json::Error,
    },

    /// A required `FOGLET_*` environment variable was not set when the
    /// loader fell back to the env-var path (no `FOGLET_DOOR_CONTEXT`).
    /// "Required" here means the field is non-optional on
    /// [`FogletContext`]: `door_id`, `terminal_width`, `terminal_height`.
    /// Optional identity fields (`user_id`, `username`, `role`,
    /// `session_id`) silently default to `None` per SPEC §5.1.
    #[error("required environment variable {var} is not set")]
    MissingEnv {
        /// The variable name we looked for.
        var: &'static str,
    },

    /// A `FOGLET_*` environment variable was set but did not parse as
    /// the expected type. Only the numeric `FOGLET_TERMINAL_WIDTH` /
    /// `FOGLET_TERMINAL_HEIGHT` vars can hit this branch — the string
    /// fields are accepted as-is.
    #[error("environment variable {var}={value:?} could not be parsed: {source}")]
    InvalidEnv {
        /// The variable name whose value did not parse.
        var: &'static str,
        /// The raw string value that failed parsing. Captured so the
        /// operator-facing error message can show what Foglet (or the
        /// dev shell) actually set, rather than just "parse error".
        value: String,
        /// The underlying parse error.
        #[source]
        source: std::num::ParseIntError,
    },
}

pub mod env_vars {
    //! `FOGLET_*` env-var names, mirrored from SPEC §2.2.
    //!
    //! Centralised as constants so the loader, the error messages, and
    //! (eventually) the manifest emitter in Task 3 all reference
    //! exactly the same string.

    /// Path to the Foglet context JSON file. When set, takes
    /// precedence over every other `FOGLET_*` variable per SPEC §5.1.
    pub const DOOR_CONTEXT: &str = "FOGLET_DOOR_CONTEXT";
    /// Stable door instance identifier. Required in env-fallback mode.
    pub const DOOR_ID: &str = "FOGLET_DOOR_ID";
    /// Foglet user id. Optional.
    pub const USER_ID: &str = "FOGLET_USER_ID";
    /// Display handle / username. Optional.
    pub const USERNAME: &str = "FOGLET_USERNAME";
    /// Foglet session id. Optional.
    pub const SESSION_ID: &str = "FOGLET_SESSION_ID";
    /// Terminal width in columns. Required in env-fallback mode.
    pub const TERMINAL_WIDTH: &str = "FOGLET_TERMINAL_WIDTH";
    /// Terminal height in rows. Required in env-fallback mode.
    pub const TERMINAL_HEIGHT: &str = "FOGLET_TERMINAL_HEIGHT";
}

/// Read and parse a Foglet context JSON file from `path`.
///
/// This is the lowest-level loader: it does no env-var inspection and
/// no fallback. Callers in Task 2b/2c orchestrate which path to call
/// it with; callers writing tests can drive it directly without
/// needing to mutate process environment.
///
/// On success the returned [`FogletContext`] has
/// [`ContextSource::ContextFile`] regardless of what the JSON itself
/// said — the source-of-truth for "where did this come from" is the
/// loader, not the file. Foglet's emitted JSON does not carry a
/// `source` field today; if a future Foglet version adds one, the
/// loader still overrides it so the field's invariant ("set by the
/// loader") stays true.
pub fn load_context_from_file(path: impl AsRef<Path>) -> Result<FogletContext, ContextError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| ContextError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let mut ctx: FogletContext =
        serde_json::from_slice(&bytes).map_err(|source| ContextError::ParseFile {
            path: path.to_path_buf(),
            source,
        })?;
    ctx.source = ContextSource::ContextFile;
    Ok(ctx)
}

/// Build a [`FogletContext`] from individual `FOGLET_*` environment
/// variables.
///
/// This is the env-var fallback path described in SPEC §5.1: used when
/// `FOGLET_DOOR_CONTEXT` is not set but the operator (or a developer
/// running the door directly in a shell) has populated the discrete
/// `FOGLET_*` vars from SPEC §2.2.
///
/// `getenv` is the lookup closure. Tests pass a closure backed by a
/// `HashMap` so the env state is purely local; production code passes
/// [`process_env`].
///
/// Required vars: `FOGLET_DOOR_ID`, `FOGLET_TERMINAL_WIDTH`,
/// `FOGLET_TERMINAL_HEIGHT`. Anything missing yields
/// [`ContextError::MissingEnv`]. Width/height that do not parse as
/// `u16` yield [`ContextError::InvalidEnv`]. Optional identity fields
/// quietly default to `None` per SPEC §5.1.
///
/// The returned context is stamped with [`ContextSource::Env`] —
/// callers (including the save manager in Task 8) use this to switch
/// between production and local-dev paths.
pub fn load_context_from_env<F>(getenv: F) -> Result<FogletContext, ContextError>
where
    F: Fn(&str) -> Option<String>,
{
    // Required: door id. Without it, the save path resolver and the
    // manifest emitter both lose their primary key, so we error rather
    // than fabricate a default at this layer (Task 2c handles the
    // "fabricate" case explicitly under a different `source`).
    let door_id = getenv(env_vars::DOOR_ID).ok_or(ContextError::MissingEnv {
        var: env_vars::DOOR_ID,
    })?;

    let terminal_width = parse_u16_env(env_vars::TERMINAL_WIDTH, &getenv)?;
    let terminal_height = parse_u16_env(env_vars::TERMINAL_HEIGHT, &getenv)?;

    Ok(FogletContext {
        door_id,
        // Optionals: empty-string env values are treated as absent.
        // Foglet-side conventions on whether unset vars are absent or
        // empty are not load-bearing here; either way a blank handle
        // is not useful.
        user_id: optional_env(env_vars::USER_ID, &getenv),
        username: optional_env(env_vars::USERNAME, &getenv),
        // `role` has no documented `FOGLET_*` env spelling in SPEC §2.2;
        // it only appears in the JSON context. Env-fallback callers
        // therefore never see a role and we leave it `None`.
        role: None,
        session_id: optional_env(env_vars::SESSION_ID, &getenv),
        terminal_width,
        terminal_height,
        source: ContextSource::Env,
    })
}

/// Read a `u16` env var or surface a structured error.
fn parse_u16_env<F>(var: &'static str, getenv: &F) -> Result<u16, ContextError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = getenv(var).ok_or(ContextError::MissingEnv { var })?;
    raw.parse::<u16>()
        .map_err(|source| ContextError::InvalidEnv {
            var,
            value: raw,
            source,
        })
}

/// Read an optional string env var, treating empty strings as absent.
fn optional_env<F>(var: &str, getenv: &F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    getenv(var).filter(|s| !s.is_empty())
}

/// Closure suitable for [`load_context`] / [`load_context_from_env`]
/// that reads from the real process environment.
///
/// Pulled out so production callers spell their intent — "use real
/// process env" — at the call site, and so tests can be obvious about
/// not using it. Using `std::env::var` directly inside the loaders
/// would have made them harder to test and easier to accidentally
/// couple to global state.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Top-level Foglet context loader.
///
/// Implements the SPEC §5.1 precedence rule:
///
/// 1. If `FOGLET_DOOR_CONTEXT` is set, load that JSON file. The file
///    wins even if individual `FOGLET_*` vars are also populated — we
///    do not "merge" them, because the JSON is Foglet's authoritative
///    snapshot and the env vars exist primarily for local dev where
///    the file is not produced.
/// 2. Otherwise, fall back to [`load_context_from_env`].
///
/// Task 2c will extend step 2 with local-dev synthesis when no env
/// vars are present and with `--local-dev-fallback` semantics for
/// malformed JSON; until then, both branches surface their failures
/// as [`ContextError`].
///
/// `getenv` is injected for the same reason as in
/// [`load_context_from_env`] — testability without process-env
/// mutation. Production callers use [`process_env`].
pub fn load_context<F>(getenv: F) -> Result<FogletContext, ContextError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(path) = optional_env(env_vars::DOOR_CONTEXT, &getenv) {
        return load_context_from_file(path);
    }
    load_context_from_env(getenv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Foglet's documented JSON shape (SPEC §2.4) with the `handle`
    /// spelling parses cleanly into a [`FogletContext`] whose typed
    /// `username` field carries the value across the alias.
    #[test]
    fn loads_documented_json_shape() {
        let json = r#"{
            "door_id": "door-42",
            "user_id": "u-7",
            "handle": "ada",
            "role": "sysop",
            "session_id": "s-1",
            "terminal_width": 80,
            "terminal_height": 24
        }"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(json.as_bytes()).expect("write json");

        let ctx = load_context_from_file(file.path()).expect("load context");

        assert_eq!(ctx.door_id, "door-42");
        assert_eq!(ctx.user_id.as_deref(), Some("u-7"));
        assert_eq!(ctx.username.as_deref(), Some("ada"));
        assert_eq!(ctx.role.as_deref(), Some("sysop"));
        assert_eq!(ctx.session_id.as_deref(), Some("s-1"));
        assert_eq!(ctx.terminal_width, 80);
        assert_eq!(ctx.terminal_height, 24);
        assert_eq!(ctx.source, ContextSource::ContextFile);
    }

    /// Optionals default to `None` and the loader does NOT fail just
    /// because they're missing — SPEC §5.1 explicitly forbids that.
    #[test]
    fn missing_optionals_default_to_none() {
        let json = r#"{
            "door_id": "door-1",
            "terminal_width": 100,
            "terminal_height": 30
        }"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(json.as_bytes()).expect("write json");

        let ctx = load_context_from_file(file.path()).expect("load context");

        assert_eq!(ctx.door_id, "door-1");
        assert!(ctx.user_id.is_none());
        assert!(ctx.username.is_none());
        assert!(ctx.role.is_none());
        assert!(ctx.session_id.is_none());
    }

    /// `username` should also accept the canonical `username` JSON
    /// key, not just the documented `handle` alias — both must work
    /// because SPEC §5.1 names the field `username` while SPEC §2.4
    /// documents `handle`.
    #[test]
    fn accepts_username_key_directly() {
        let json = r#"{
            "door_id": "door-1",
            "username": "ada",
            "terminal_width": 80,
            "terminal_height": 24
        }"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(json.as_bytes()).expect("write json");

        let ctx = load_context_from_file(file.path()).expect("load context");

        assert_eq!(ctx.username.as_deref(), Some("ada"));
    }

    /// A path that does not exist surfaces as [`ContextError::ReadFile`]
    /// — distinct from a parse error so callers can react differently
    /// (e.g. fall back to env vars in Task 2b without papering over
    /// genuine JSON corruption).
    #[test]
    fn missing_file_returns_read_error() {
        let nonexistent = std::env::temp_dir().join("foglet-context-does-not-exist.json");
        let err = load_context_from_file(&nonexistent).expect_err("must fail");
        assert!(
            matches!(err, ContextError::ReadFile { .. }),
            "expected ReadFile, got {err:?}"
        );
    }

    /// Garbage bytes surface as [`ContextError::ParseFile`] with the
    /// originating path attached so the operator-facing error message
    /// names the file Foglet handed us.
    #[test]
    fn malformed_json_returns_parse_error() {
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(b"not json {").expect("write garbage");

        let err = load_context_from_file(file.path()).expect_err("must fail");
        match err {
            ContextError::ParseFile { path, .. } => {
                assert_eq!(path, file.path());
            }
            other => panic!("expected ParseFile, got {other:?}"),
        }
    }

    /// Build a `getenv`-style closure backed by a literal slice of
    /// `(name, value)` pairs. Keeps the env-loader tests parallel-safe
    /// (no `std::env::set_var` race) and self-documenting at the call
    /// site.
    fn fake_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            pairs.iter().find_map(|(k, v)| {
                if *k == name {
                    Some((*v).to_owned())
                } else {
                    None
                }
            })
        }
    }

    /// Env-only happy path: every required var present, every optional
    /// var present. The resulting context carries [`ContextSource::Env`]
    /// so downstream code can branch on "we are in env-fallback mode".
    #[test]
    fn env_fallback_loads_full_set() {
        let env = fake_env(&[
            ("FOGLET_DOOR_ID", "door-env-1"),
            ("FOGLET_USER_ID", "u-env-9"),
            ("FOGLET_USERNAME", "ada-env"),
            ("FOGLET_SESSION_ID", "s-env-2"),
            ("FOGLET_TERMINAL_WIDTH", "120"),
            ("FOGLET_TERMINAL_HEIGHT", "40"),
        ]);

        let ctx = load_context_from_env(env).expect("load env context");

        assert_eq!(ctx.door_id, "door-env-1");
        assert_eq!(ctx.user_id.as_deref(), Some("u-env-9"));
        assert_eq!(ctx.username.as_deref(), Some("ada-env"));
        // `role` is not in SPEC §2.2's env-var list, so env mode never
        // populates it regardless of any custom var the operator might
        // have set.
        assert!(ctx.role.is_none());
        assert_eq!(ctx.session_id.as_deref(), Some("s-env-2"));
        assert_eq!(ctx.terminal_width, 120);
        assert_eq!(ctx.terminal_height, 40);
        assert_eq!(ctx.source, ContextSource::Env);
    }

    /// Optionals are genuinely optional — present door id + terminal
    /// dims is enough; the loader does not fail just because user/
    /// session metadata is unset (SPEC §5.1).
    #[test]
    fn env_fallback_tolerates_missing_optionals() {
        let env = fake_env(&[
            ("FOGLET_DOOR_ID", "door-env-2"),
            ("FOGLET_TERMINAL_WIDTH", "80"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let ctx = load_context_from_env(env).expect("load env context");

        assert_eq!(ctx.door_id, "door-env-2");
        assert!(ctx.user_id.is_none());
        assert!(ctx.username.is_none());
        assert!(ctx.session_id.is_none());
    }

    /// Empty-string env vars are treated as absent. Some shells and
    /// orchestration tools "unset" a var by exporting it empty; we
    /// don't want that to surface as `Some("")` user ids that confuse
    /// the save-path resolver in Task 8.
    #[test]
    fn env_fallback_treats_empty_strings_as_absent() {
        let env = fake_env(&[
            ("FOGLET_DOOR_ID", "door-env-3"),
            ("FOGLET_USER_ID", ""),
            ("FOGLET_USERNAME", ""),
            ("FOGLET_TERMINAL_WIDTH", "80"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let ctx = load_context_from_env(env).expect("load env context");

        assert!(ctx.user_id.is_none());
        assert!(ctx.username.is_none());
    }

    /// Missing required var → typed [`ContextError::MissingEnv`] with
    /// the variable name attached so the operator-facing message can
    /// say *which* one.
    #[test]
    fn env_fallback_errors_on_missing_required_var() {
        // Door id missing.
        let env = fake_env(&[
            ("FOGLET_TERMINAL_WIDTH", "80"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let err = load_context_from_env(env).expect_err("must fail");
        match err {
            ContextError::MissingEnv { var } => assert_eq!(var, "FOGLET_DOOR_ID"),
            other => panic!("expected MissingEnv, got {other:?}"),
        }
    }

    /// Non-numeric width surfaces as [`ContextError::InvalidEnv`] with
    /// both the variable name and the offending value, so an operator
    /// staring at the message can see what they actually exported.
    #[test]
    fn env_fallback_errors_on_non_numeric_width() {
        let env = fake_env(&[
            ("FOGLET_DOOR_ID", "door-env-4"),
            ("FOGLET_TERMINAL_WIDTH", "wide"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let err = load_context_from_env(env).expect_err("must fail");
        match err {
            ContextError::InvalidEnv { var, value, .. } => {
                assert_eq!(var, "FOGLET_TERMINAL_WIDTH");
                assert_eq!(value, "wide");
            }
            other => panic!("expected InvalidEnv, got {other:?}"),
        }
    }

    /// SPEC §5.1: `FOGLET_DOOR_CONTEXT` JSON MUST win over individual
    /// env vars. When both are set the JSON's identity must surface,
    /// not the env vars'. We assert both the door id (proves the file
    /// was read) and `source = ContextFile` (proves the file path won
    /// the precedence check, not env-then-overwrite).
    #[test]
    fn json_context_wins_over_env_vars() {
        let json = r#"{
            "door_id": "door-from-json",
            "user_id": "u-from-json",
            "terminal_width": 100,
            "terminal_height": 30
        }"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(json.as_bytes()).expect("write json");

        let path_str = file.path().to_str().expect("utf-8 path").to_owned();
        let env = move |name: &str| match name {
            "FOGLET_DOOR_CONTEXT" => Some(path_str.clone()),
            "FOGLET_DOOR_ID" => Some("door-from-env".to_owned()),
            "FOGLET_USER_ID" => Some("u-from-env".to_owned()),
            "FOGLET_TERMINAL_WIDTH" => Some("80".to_owned()),
            "FOGLET_TERMINAL_HEIGHT" => Some("24".to_owned()),
            _ => None,
        };

        let ctx = load_context(env).expect("load context");

        assert_eq!(ctx.door_id, "door-from-json");
        assert_eq!(ctx.user_id.as_deref(), Some("u-from-json"));
        assert_eq!(ctx.terminal_width, 100);
        assert_eq!(ctx.terminal_height, 30);
        assert_eq!(ctx.source, ContextSource::ContextFile);
    }

    /// When `FOGLET_DOOR_CONTEXT` is unset, the orchestrator delegates
    /// to the env path. This is the "running directly in a dev shell
    /// with FOGLET_* exported" scenario.
    #[test]
    fn load_context_falls_back_to_env_when_no_context_file() {
        let env = fake_env(&[
            ("FOGLET_DOOR_ID", "door-env-5"),
            ("FOGLET_TERMINAL_WIDTH", "80"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let ctx = load_context(env).expect("load context");
        assert_eq!(ctx.door_id, "door-env-5");
        assert_eq!(ctx.source, ContextSource::Env);
    }

    /// An empty `FOGLET_DOOR_CONTEXT` env var is treated as unset, not
    /// as "open the file at path empty-string". Otherwise we'd surface
    /// a confusing read error for what is really a "var was exported
    /// blank" situation.
    #[test]
    fn empty_door_context_var_falls_back_to_env() {
        let env = fake_env(&[
            ("FOGLET_DOOR_CONTEXT", ""),
            ("FOGLET_DOOR_ID", "door-env-6"),
            ("FOGLET_TERMINAL_WIDTH", "80"),
            ("FOGLET_TERMINAL_HEIGHT", "24"),
        ]);

        let ctx = load_context(env).expect("load context");
        assert_eq!(ctx.source, ContextSource::Env);
    }

    /// The loader stamps `source = ContextFile` even if the JSON
    /// claimed something else — the invariant in [`FogletContext`]'s
    /// docs is that the source field reflects how the value was
    /// obtained, not what the file said.
    #[test]
    fn source_is_overridden_to_context_file() {
        let json = r#"{
            "door_id": "door-1",
            "terminal_width": 80,
            "terminal_height": 24,
            "source": "local_dev"
        }"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(json.as_bytes()).expect("write json");

        let ctx = load_context_from_file(file.path()).expect("load context");
        assert_eq!(ctx.source, ContextSource::ContextFile);
    }
}
