//! Save-path resolution for Foglet door games (SPEC §12).
//!
//! Task 8 splits into two halves: this module is **path resolution
//! only** (Task 8a). Atomic write/read lands in Task 8b.
//!
//! # Why a dedicated module
//!
//! SPEC §7.1 step 5 names "Resolve per-user save path" as part of the
//! startup ordering — it happens *before* terminal raw mode is entered,
//! so the runtime can surface a clear error (e.g. malformed
//! `FGK_SAVE_DIR`) without first wrecking the terminal. Keeping the
//! resolution pure and side-effect-free (no directory creation, no
//! file I/O) means the runtime can call it from the unsafe pre-raw-
//! mode startup band without worrying about partial state.
//!
//! # Precedence
//!
//! Resolution honours four sources in order, matching the wrapper
//! script in SPEC §10.4 and the deployment paths in SPEC §12:
//!
//! 1. **`SaveStrategy::None`** — game opted out of persistence
//!    (`assets/game.toml` says so). Returns `Ok(None)`.
//! 2. **`--save-dir <dir>`** CLI override. The wrapper passes this on
//!    every production launch (`--save-dir "$DIR/saves/$USER_ID"`).
//!    Treated as the directory that already contains (or will contain)
//!    `save.json` — no further `<user_id>` interpolation happens here.
//! 3. **`FGK_SAVE_DIR`** environment variable. Same shape as the CLI
//!    override; lets operators redirect saves without rebuilding the
//!    wrapper. Per SPEC §10.4 the wrapper itself reads this var, but
//!    that wrapper isn't used in `cargo run` smoke tests, so the
//!    runtime honours the env directly as a redundant safety net.
//! 4. **Fallback by [`ContextSource`]**:
//!    - `ContextFile` / `Env` (running under Foglet) →
//!      `/srv/foglet/doors/<slug>/saves/<user_id>/save.json` per
//!      SPEC §12. Missing `user_id` falls back to a literal
//!      `anonymous` segment so saves still go *somewhere* per-door —
//!      consistent with Foglet supporting anonymous-access doors
//!      (SPEC §5.1).
//!    - `LocalDev` → project-local `.fgk/saves/local-dev/save.json`,
//!      relative to the current working directory.
//!
//! # Side effects
//!
//! None at this layer. The returned [`PathBuf`] always ends in
//! `save.json` and may point at a directory that does not yet exist;
//! Task 8b's writer is responsible for `mkdir -p` and atomic write.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::SaveStrategy;
use crate::foglet::{ContextSource, FogletContext};

/// Filename stored at the resolved save directory.
///
/// Lifted to a constant so Task 8b's writer and any future tooling
/// (e.g. `fgk` debug commands that locate a user's save) agree on one
/// spelling. SPEC §12 documents `save.json` explicitly.
pub const SAVE_FILENAME: &str = "save.json";

/// Environment variable that overrides the resolved save directory.
///
/// Matches the variable the `run.sh` wrapper reads in SPEC §10.4. The
/// runtime honours it directly in addition to the wrapper's expansion
/// so `cargo run` smoke tests of the binary still respect operator
/// overrides without going through `run.sh`.
pub const SAVE_DIR_ENV: &str = "FGK_SAVE_DIR";

/// Production root for per-user saves. SPEC §12.
const PROD_SAVE_ROOT: &str = "/srv/foglet/doors";

/// Local-dev save directory, relative to the current working dir.
const LOCAL_DEV_SAVE_DIR: &str = ".fgk/saves/local-dev";

/// Filesystem segment used when running under Foglet but `user_id` is
/// absent. Foglet supports anonymous doors (SPEC §5.1), so the loader
/// surfaces `user_id = None`; rather than failing here we direct those
/// saves to a stable per-door `anonymous` bucket. Documented for the
/// operator reading the failure mode in `docs/foglet-install.md`
/// (Task 14b).
const ANONYMOUS_USER_SEGMENT: &str = "anonymous";

/// Errors produced while resolving a save path.
///
/// Library-internal (`thiserror`); the CLI converts at the boundary
/// per the PROMPT crate budget.
#[derive(Debug, Error)]
pub enum SavePathError {
    /// `slug` was empty. The runtime treats this as a programmer
    /// error — `GameConfig` parsing already rejects empty slugs
    /// (SPEC §5.2 / `config::GameConfig` validation) — but this layer
    /// double-checks so a hand-built [`SavePathInputs`] (e.g. in
    /// tests) can't sneak past and produce a path like
    /// `/srv/foglet/doors//saves/<user>/save.json`.
    #[error("save path resolution requires a non-empty slug")]
    EmptySlug,
}

/// Inputs required to resolve a save path.
///
/// Bundled into a struct (rather than a long argument list) so the
/// runtime can construct it once at startup and pass it through to
/// Task 8b's writer alongside the resolved path. New knobs (e.g. a
/// future `force_per_machine` mode) land additively here without
/// rippling through call sites.
#[derive(Debug, Clone, Copy)]
pub struct SavePathInputs<'a> {
    /// Slug from `[game].slug` — the directory name under
    /// `/srv/foglet/doors/`.
    pub slug: &'a str,
    /// Save strategy from `[save].strategy`. Drives the
    /// "no persistence" short-circuit.
    pub strategy: SaveStrategy,
    /// Loaded Foglet context. Provides `user_id` and `source`, which
    /// together pick between the production and local-dev paths.
    pub context: &'a FogletContext,
    /// `--save-dir` from the CLI, if the operator passed one. Wins
    /// over `FGK_SAVE_DIR` and over the SPEC §12 default roots.
    pub cli_override: Option<&'a Path>,
}

/// Resolve the save file path for the current launch.
///
/// `getenv` is injected so tests don't have to mutate process env.
/// Production callers pass [`crate::foglet::process_env`].
///
/// Returns `Ok(None)` exactly when the game opts out of saves
/// ([`SaveStrategy::None`]). All other branches return
/// `Ok(Some(<path>/save.json))`.
pub fn resolve_save_path<F>(
    inputs: &SavePathInputs<'_>,
    getenv: F,
) -> Result<Option<PathBuf>, SavePathError>
where
    F: Fn(&str) -> Option<String>,
{
    if inputs.slug.is_empty() {
        return Err(SavePathError::EmptySlug);
    }

    if matches!(inputs.strategy, SaveStrategy::None) {
        return Ok(None);
    }

    // 1. CLI override wins outright. The wrapper script in SPEC §10.4
    //    builds this path from `FOGLET_USER_ID`, so by the time we see
    //    it the per-user component is already encoded — we just append
    //    the filename.
    if let Some(dir) = inputs.cli_override {
        return Ok(Some(dir.join(SAVE_FILENAME)));
    }

    // 2. Env override behaves identically to the CLI flag. Empty values
    //    are treated as "unset" so a stray `FGK_SAVE_DIR=` in a shell
    //    profile doesn't collapse the save into the cwd.
    if let Some(dir) = getenv(SAVE_DIR_ENV).filter(|s| !s.is_empty()) {
        return Ok(Some(PathBuf::from(dir).join(SAVE_FILENAME)));
    }

    // 3. Default roots, picked by where the Foglet context came from.
    //    `ContextFile`/`Env` mean we believe Foglet (or a Foglet-like
    //    shell) is in the loop, so the SPEC §12 production root
    //    applies. `LocalDev` means we synthesised the context and
    //    must not write to `/srv/foglet/...`.
    let path = match inputs.context.source {
        ContextSource::ContextFile | ContextSource::Env => {
            let user = inputs
                .context
                .user_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(ANONYMOUS_USER_SEGMENT);
            PathBuf::from(PROD_SAVE_ROOT)
                .join(inputs.slug)
                .join("saves")
                .join(user)
                .join(SAVE_FILENAME)
        }
        ContextSource::LocalDev => PathBuf::from(LOCAL_DEV_SAVE_DIR).join(SAVE_FILENAME),
    };

    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(source: ContextSource, user: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "door-1".into(),
            user_id: user.map(str::to_string),
            username: None,
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source,
        }
    }

    fn empty_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn strategy_none_short_circuits() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap();
        assert!(path.is_none(), "SaveStrategy::None must disable saves");
    }

    #[test]
    fn cli_override_wins_over_everything_else() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let dir = Path::new("/tmp/explicit");
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: Some(dir),
            },
            |name| {
                // Even if the env var is set, the CLI flag must win.
                if name == SAVE_DIR_ENV {
                    Some("/tmp/from-env".into())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .expect("non-None strategy should yield a path");
        assert_eq!(path, PathBuf::from("/tmp/explicit/save.json"));
    }

    #[test]
    fn fgk_save_dir_env_used_when_no_cli_override() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            |name| {
                if name == SAVE_DIR_ENV {
                    Some("/var/saves/door".into())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(path, PathBuf::from("/var/saves/door/save.json"));
    }

    #[test]
    fn empty_fgk_save_dir_is_ignored() {
        // Empty string from a stray `export FGK_SAVE_DIR=` must NOT be
        // treated as "save next to cwd"; it falls through to defaults.
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            |name| {
                if name == SAVE_DIR_ENV {
                    Some(String::new())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-1/save.json")
        );
    }

    #[test]
    fn production_path_uses_user_id_under_context_file_source() {
        let context = ctx(ContextSource::ContextFile, Some("u-42"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-42/save.json")
        );
    }

    #[test]
    fn production_path_used_when_context_came_from_env_vars() {
        // Source = Env means a Foglet-like shell wrapped us with
        // `FOGLET_*` env vars but no JSON context file. Same SPEC §12
        // production path applies.
        let context = ctx(ContextSource::Env, Some("u-99"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-99/save.json")
        );
    }

    #[test]
    fn anonymous_segment_used_when_user_id_missing_in_production() {
        let context = ctx(ContextSource::ContextFile, None);
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/anonymous/save.json")
        );
    }

    #[test]
    fn empty_user_id_treated_as_missing() {
        let context = ctx(ContextSource::ContextFile, Some(""));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/anonymous/save.json")
        );
    }

    #[test]
    fn local_dev_path_is_relative_to_cwd() {
        let context = ctx(ContextSource::LocalDev, None);
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(path, PathBuf::from(".fgk/saves/local-dev/save.json"));
    }

    #[test]
    fn empty_slug_is_an_error() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let err = resolve_save_path(
            &SavePathInputs {
                slug: "",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap_err();
        assert!(matches!(err, SavePathError::EmptySlug));
    }

    #[test]
    fn empty_slug_is_an_error_even_when_strategy_is_none() {
        // Strategy::None short-circuits *after* validation so an empty
        // slug never silently masks a programmer bug.
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let err = resolve_save_path(
            &SavePathInputs {
                slug: "",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap_err();
        assert!(matches!(err, SavePathError::EmptySlug));
    }

    #[test]
    fn cli_override_with_strategy_none_still_returns_none() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let dir = Path::new("/tmp/explicit");
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: Some(dir),
            },
            empty_env,
        )
        .unwrap();
        assert!(path.is_none());
    }
}
