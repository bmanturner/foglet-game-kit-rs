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
//! - **Task 2a (this commit)** — the [`FogletContext`] type and the
//!   `FOGLET_DOOR_CONTEXT` path → file → parse → typed value path.
//! - **Task 2b** — env-var fallback (`FOGLET_DOOR_ID`, etc.) when the
//!   context file is absent.
//! - **Task 2c** — local-dev synthesis when neither the file nor the
//!   env vars are present, plus `--local-dev-fallback` semantics for
//!   malformed JSON.
//!
//! Only the file-based path is wired up here so each sub-task ships
//! with its own focused tests; the public entry point will gain the
//! fallback branches in 2b/2c without breaking 2a's contract.

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
