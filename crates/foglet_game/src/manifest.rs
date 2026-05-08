//! `FogletManifest` — the JSON object Foglet operators drop into the
//! manifest directory to register a door.
//!
//! # Why this lives in `foglet_game` (and not just in `fgk`)
//!
//! The CLI (`fgk emit-manifest`, Task 11) is the primary writer, but
//! the manifest **shape** is a contract between this kit and Foglet
//! itself (SPEC §5.7, §10.3). Putting the typed model in the library
//! means:
//!
//! - downstream tooling can consume / validate manifests without
//!   shelling out to `fgk`,
//! - integration tests can round-trip the SPEC example through the
//!   same struct the CLI uses, and
//! - the absolute-path invariants live next to the type that needs
//!   to enforce them, not scattered across CLI flag parsing.
//!
//! # Invariants enforced at construction
//!
//! Per SPEC §5.7 and §10.3:
//!
//! - `runtime` is `"external_pty"` (constant for the v1 slice).
//! - `command` and `working_dir` are absolute Unix-style paths.
//!   Foglet only ever runs these on Linux, so we reject anything
//!   that doesn't start with `/` regardless of the host the manifest
//!   is generated on.
//! - `pty` is `true`.
//! - `env_allowlist` is non-empty when `env` has entries (an
//!   allowlist of `[]` paired with non-empty `env` is almost always
//!   an authoring bug — Foglet would refuse to forward those vars).
//!
//! Field defaults match the SPEC §10.3 example so a typical authoring
//! flow (`FogletManifest::new(...)` → tweak → serialize) produces the
//! canonical shape without ceremony.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The runtime identifier required for the v1 slice.
///
/// SPEC §3.2 / §5.7 forbid `classic_dropfile` and `native_elixir` in
/// the first release, so `external_pty` is currently the only legal
/// value. Exposed as a `pub const` so tests and downstream tooling
/// can compare against it without stringly-typing the literal.
pub const RUNTIME_EXTERNAL_PTY: &str = "external_pty";

/// Default per-session timeout (30 minutes), per SPEC §10.3 example.
pub const DEFAULT_TIMEOUT_MS: u64 = 1_800_000;

/// Default idle timeout (5 minutes), per SPEC §10.3 example.
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Default visibility — `members` matches the SPEC example and is
/// the safer default than `public` for an unaudited door.
pub const DEFAULT_VISIBILITY: &str = "members";

/// Default auth scope — `site` mirrors the SPEC example.
pub const DEFAULT_AUTH_SCOPE: &str = "site";

/// Errors raised while building or validating a [`FogletManifest`].
///
/// These are library-internal (`thiserror`) per the PROMPT.md
/// convention; the CLI re-wraps them with `anyhow` at the process
/// boundary so the user sees a single coherent error chain.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// `command` or `working_dir` was not an absolute Unix path.
    /// Foglet runs doors as `/path/to/run.sh` from a fixed working
    /// directory; relative paths would be resolved against whatever
    /// CWD the Foglet runtime happens to be in, which is exactly the
    /// kind of ambient-state coupling SPEC §13.2 warns against.
    #[error("{field} must be an absolute path (starts with `/`); got `{value}`")]
    NotAbsolute {
        /// Which field failed validation (e.g. `command`).
        field: &'static str,
        /// The offending value, echoed back so error messages are
        /// actionable without re-reading the user's input.
        value: String,
    },

    /// `env` contains keys but `env_allowlist` is empty. Foglet only
    /// forwards env vars listed in `env_allowlist`; pairing a
    /// populated `env` with an empty allowlist silently drops them.
    /// We treat that as an error rather than producing a manifest
    /// the operator would have to debug at install time.
    #[error("env has {env_count} entries but env_allowlist is empty")]
    EnvAllowlistEmpty {
        /// How many entries `env` had — surfaced so the operator can
        /// confirm which of their fixtures triggered the check.
        env_count: usize,
    },
}

/// Typed model of the JSON Foglet expects in its manifest directory.
///
/// Field order in the struct matches the SPEC §10.3 example; serde
/// preserves struct field order during JSON serialization, so the
/// output is stable across runs and easy to eyeball-diff.
///
/// Use [`FogletManifest::new`] for the common case (fills in the
/// SPEC §10.3 defaults); construct the struct directly only when a
/// test needs to inspect a specific malformed shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FogletManifest {
    /// Stable identifier used by Foglet to address the door
    /// internally. Conventionally identical to `slug`.
    pub id: String,

    /// URL-safe slug shown in operator UIs and used in install
    /// paths. Conventionally `kebab-case`.
    pub slug: String,

    /// Human-readable name shown to BBS users in the door menu.
    pub display_name: String,

    /// One-paragraph description shown alongside `display_name`.
    pub description: String,

    /// Runtime identifier — always [`RUNTIME_EXTERNAL_PTY`] for v1.
    pub runtime: String,

    /// Absolute path Foglet executes to launch the door. For
    /// `fgk`-packaged games this points at the boring `run.sh`
    /// wrapper from SPEC §10.4.
    pub command: String,

    /// Extra args passed to `command`. Usually empty because the
    /// `run.sh` wrapper handles the asset/save-dir flags itself
    /// (SPEC §10.4).
    pub args: Vec<String>,

    /// Absolute working directory Foglet `chdir`s into before
    /// executing `command`.
    pub working_dir: String,

    /// Hard wall-clock cap on a single session, in milliseconds.
    pub timeout_ms: u64,

    /// Idle cap (no input) before Foglet kills the session, in
    /// milliseconds.
    pub idle_timeout_ms: u64,

    /// Foglet visibility — typically `"members"` or `"public"`.
    /// Stringly-typed because Foglet may extend the set; we don't
    /// want to break older manifests against a newer kit.
    pub visibility: String,

    /// Foglet auth scope — typically `"site"`.
    pub auth_scope: String,

    /// Environment variables Foglet should set when spawning the
    /// door. Stored in a [`BTreeMap`] for deterministic JSON output,
    /// since hash-map ordering would make `cargo test` flake on
    /// snapshot-style assertions.
    pub env: BTreeMap<String, String>,

    /// Subset of env-var names Foglet is allowed to forward from the
    /// host environment into the door. Anything not listed is
    /// silently dropped by Foglet, so this is the operator-facing
    /// audit surface for what the door sees.
    pub env_allowlist: Vec<String>,

    /// Whether to allocate a real PTY for the door. Always `true`
    /// for `external_pty`; included as a field rather than a derived
    /// constant because Foglet's schema (SPEC §5.7) treats it as
    /// part of the manifest payload and may key behavior off it.
    pub pty: bool,
}

/// Inputs the typical authoring flow already has on hand by the time
/// it asks for a manifest.
///
/// Held as a struct (rather than a five-arg function) so future
/// fields (icon path, tags, ...) can be added without breaking
/// callers. All fields are required because the SPEC §10.3 example
/// does not define defaults for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInputs<'a> {
    /// Door slug (e.g. `"murder-motel"`).
    pub slug: &'a str,
    /// Display name (e.g. `"Murder Motel"`).
    pub display_name: &'a str,
    /// One-paragraph description.
    pub description: &'a str,
    /// Absolute install directory, typically
    /// `/srv/foglet/doors/<slug>`. The `command` and `working_dir`
    /// fields on the manifest are derived from it.
    pub install_dir: &'a str,
}

impl FogletManifest {
    /// Build a manifest from the minimum inputs `fgk emit-manifest`
    /// has access to, filling in the SPEC §10.3 defaults for
    /// timeouts, visibility, env, and the PTY flag.
    ///
    /// `command` is derived as `<install_dir>/run.sh` to match the
    /// SPEC §10.3 example and the wrapper that `fgk package`
    /// produces (Task 12). Override fields on the returned struct if
    /// a specific game needs to deviate.
    pub fn new(inputs: ManifestInputs<'_>) -> Result<Self, ManifestError> {
        require_absolute("install_dir", inputs.install_dir)?;

        // Trim a trailing slash so `command` doesn't end up with a
        // double-slash like `/srv/foglet/doors/x//run.sh`. Cosmetic,
        // but JSON diffs are easier to read.
        let install_dir = inputs.install_dir.trim_end_matches('/').to_string();
        let command = format!("{install_dir}/run.sh");

        let mut env = BTreeMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("LANG".to_string(), "C.UTF-8".to_string());

        let manifest = Self {
            id: inputs.slug.to_string(),
            slug: inputs.slug.to_string(),
            display_name: inputs.display_name.to_string(),
            description: inputs.description.to_string(),
            runtime: RUNTIME_EXTERNAL_PTY.to_string(),
            command,
            args: Vec::new(),
            working_dir: install_dir,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
            visibility: DEFAULT_VISIBILITY.to_string(),
            auth_scope: DEFAULT_AUTH_SCOPE.to_string(),
            env,
            env_allowlist: vec!["TERM".to_string(), "LANG".to_string()],
            pty: true,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    /// Re-run the construction-time invariants on a manifest that
    /// was assembled by other means (e.g. `serde_json::from_str`
    /// during a CLI sanity check).
    ///
    /// Kept separate from `new` so callers can mutate the struct
    /// freely between build and validate without fighting the
    /// borrow checker.
    pub fn validate(&self) -> Result<(), ManifestError> {
        require_absolute("command", &self.command)?;
        require_absolute("working_dir", &self.working_dir)?;
        if !self.env.is_empty() && self.env_allowlist.is_empty() {
            return Err(ManifestError::EnvAllowlistEmpty {
                env_count: self.env.len(),
            });
        }
        Ok(())
    }

    /// Serialize to compact JSON (single line, no trailing newline).
    /// Useful for snapshot tests and round-trip assertions where
    /// whitespace would only add noise.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Serialize to pretty-printed JSON with a trailing newline.
    /// This is the on-disk format `fgk emit-manifest` and
    /// `fgk package` write — operators read these by hand.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}

/// Validate that `value` looks like an absolute Unix path.
///
/// Implemented manually (rather than via [`Path::is_absolute`])
/// because `Path::is_absolute` on Windows demands a drive letter,
/// and we want this check to behave identically regardless of where
/// `fgk` is run — a developer on macOS generating a manifest for a
/// Linux Foglet host should get the same answer as CI on Linux.
fn require_absolute(field: &'static str, value: &str) -> Result<(), ManifestError> {
    // `Path::new(value).is_absolute()` would also work on Unix hosts,
    // but the explicit `/` check makes the intent obvious to readers
    // and avoids platform-conditional behavior in tests.
    if value.starts_with('/') && !value.is_empty() {
        // Sanity: reject paths with embedded NULs — JSON would accept
        // them but they'd corrupt any downstream syscall.
        if value.contains('\0') {
            return Err(ManifestError::NotAbsolute {
                field,
                value: value.to_string(),
            });
        }
        // Reference Path so the dependency is visible to readers
        // and so a future refactor toward typed paths is mechanical.
        let _ = Path::new(value);
        Ok(())
    } else {
        Err(ManifestError::NotAbsolute {
            field,
            value: value.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Helper: the canonical inputs from SPEC §10.3.
    fn murder_motel_inputs() -> ManifestInputs<'static> {
        ManifestInputs {
            slug: "murder-motel",
            display_name: "Murder Motel",
            description: "A tiny BBS mystery built with foglet-game-kit.",
            install_dir: "/srv/foglet/doors/murder-motel",
        }
    }

    #[test]
    fn new_fills_spec_10_3_defaults() {
        let m = FogletManifest::new(murder_motel_inputs()).expect("valid inputs");

        assert_eq!(m.id, "murder-motel");
        assert_eq!(m.slug, "murder-motel");
        assert_eq!(m.display_name, "Murder Motel");
        assert_eq!(m.runtime, RUNTIME_EXTERNAL_PTY);
        assert_eq!(m.command, "/srv/foglet/doors/murder-motel/run.sh");
        assert_eq!(m.working_dir, "/srv/foglet/doors/murder-motel");
        assert!(m.args.is_empty());
        assert_eq!(m.timeout_ms, 1_800_000);
        assert_eq!(m.idle_timeout_ms, 300_000);
        assert_eq!(m.visibility, "members");
        assert_eq!(m.auth_scope, "site");
        assert_eq!(
            m.env.get("TERM").map(String::as_str),
            Some("xterm-256color")
        );
        assert_eq!(m.env.get("LANG").map(String::as_str), Some("C.UTF-8"));
        assert_eq!(m.env_allowlist, vec!["TERM", "LANG"]);
        assert!(m.pty);
    }

    #[test]
    fn new_trims_trailing_slash_from_install_dir() {
        let mut inputs = murder_motel_inputs();
        inputs.install_dir = "/srv/foglet/doors/murder-motel/";
        let m = FogletManifest::new(inputs).expect("valid inputs");
        assert_eq!(m.command, "/srv/foglet/doors/murder-motel/run.sh");
        assert_eq!(m.working_dir, "/srv/foglet/doors/murder-motel");
    }

    #[test]
    fn new_rejects_relative_install_dir() {
        let mut inputs = murder_motel_inputs();
        inputs.install_dir = "relative/path";
        let err = FogletManifest::new(inputs).expect_err("relative path must fail");
        match err {
            ManifestError::NotAbsolute { field, value } => {
                assert_eq!(field, "install_dir");
                assert_eq!(value, "relative/path");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_relative_command() {
        let mut m = FogletManifest::new(murder_motel_inputs()).unwrap();
        m.command = "run.sh".to_string();
        let err = m.validate().expect_err("relative command must fail");
        assert!(matches!(
            err,
            ManifestError::NotAbsolute {
                field: "command",
                ..
            }
        ));
    }

    #[test]
    fn validate_rejects_empty_allowlist_when_env_present() {
        let mut m = FogletManifest::new(murder_motel_inputs()).unwrap();
        m.env_allowlist.clear();
        let err = m.validate().expect_err("populated env demands allowlist");
        match err {
            ManifestError::EnvAllowlistEmpty { env_count } => assert_eq!(env_count, 2),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn validate_allows_empty_env_with_empty_allowlist() {
        let mut m = FogletManifest::new(murder_motel_inputs()).unwrap();
        m.env.clear();
        m.env_allowlist.clear();
        m.validate().expect("empty env + empty allowlist is fine");
    }

    /// Round-trip: serialize → parse → all SPEC §10.3 fields match.
    /// We compare via parsed `Value` rather than raw strings so
    /// JSON-object key ordering doesn't make the test brittle.
    #[test]
    fn json_round_trip_matches_spec_10_3_example() {
        let m = FogletManifest::new(murder_motel_inputs()).unwrap();
        let json = m.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).expect("parses as JSON");

        assert_eq!(v["id"], "murder-motel");
        assert_eq!(v["slug"], "murder-motel");
        assert_eq!(v["display_name"], "Murder Motel");
        assert_eq!(
            v["description"],
            "A tiny BBS mystery built with foglet-game-kit."
        );
        assert_eq!(v["runtime"], "external_pty");
        assert_eq!(v["command"], "/srv/foglet/doors/murder-motel/run.sh");
        assert!(v["args"].as_array().unwrap().is_empty());
        assert_eq!(v["working_dir"], "/srv/foglet/doors/murder-motel");
        assert_eq!(v["timeout_ms"], 1_800_000);
        assert_eq!(v["idle_timeout_ms"], 300_000);
        assert_eq!(v["visibility"], "members");
        assert_eq!(v["auth_scope"], "site");
        assert_eq!(v["env"]["TERM"], "xterm-256color");
        assert_eq!(v["env"]["LANG"], "C.UTF-8");
        let allowlist: Vec<&str> = v["env_allowlist"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(allowlist, vec!["TERM", "LANG"]);
        assert_eq!(v["pty"], true);
    }

    /// Deserialize the literal SPEC §10.3 example back into the
    /// typed struct. Catches schema drift: if someone reorders or
    /// renames a field on `FogletManifest`, this test fails.
    #[test]
    fn deserializes_spec_10_3_example_verbatim() {
        let example = r#"{
          "id": "murder-motel",
          "slug": "murder-motel",
          "display_name": "Murder Motel",
          "description": "A tiny BBS mystery built with foglet-game-kit.",
          "runtime": "external_pty",
          "command": "/srv/foglet/doors/murder-motel/run.sh",
          "args": [],
          "working_dir": "/srv/foglet/doors/murder-motel",
          "timeout_ms": 1800000,
          "idle_timeout_ms": 300000,
          "visibility": "members",
          "auth_scope": "site",
          "env": {
            "TERM": "xterm-256color",
            "LANG": "C.UTF-8"
          },
          "env_allowlist": ["TERM", "LANG"],
          "pty": true
        }"#;

        let m: FogletManifest = serde_json::from_str(example).expect("SPEC example parses");
        m.validate().expect("SPEC example is valid");
        assert_eq!(m.runtime, RUNTIME_EXTERNAL_PTY);
        assert!(m.pty);
        assert_eq!(m.command, "/srv/foglet/doors/murder-motel/run.sh");
    }

    #[test]
    fn pretty_output_ends_with_newline() {
        let m = FogletManifest::new(murder_motel_inputs()).unwrap();
        let s = m.to_json_pretty().unwrap();
        assert!(s.ends_with('\n'), "operators expect a trailing newline");
    }
}
