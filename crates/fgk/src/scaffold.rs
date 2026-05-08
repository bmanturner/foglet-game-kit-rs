//! Project scaffolding for `fgk new <path>`.
//!
//! Task 10b materialises the embedded [`crate::templates::TEMPLATES`]
//! fixtures onto disk under a fresh project directory. The scaffolder
//! is deliberately small: it validates the project name, creates the
//! destination tree, and writes each template after running
//! [`crate::templates::substitute`] on its body. Anything richer
//! (post-generation hooks, Git init, optional plugins) is out of
//! scope until SPEC asks for it.
//!
//! ## Why the scaffolder lives in the library, not `main.rs`
//!
//! Splitting generation out of the CLI binary lets unit tests drive
//! it directly with a `tempfile::TempDir` root, which is far
//! cheaper and more deterministic than `assert_cmd`-ing the binary
//! for every assertion. `main.rs` stays a one-liner that parses
//! arguments and delegates here.
//!
//! ## Failure model
//!
//! Errors are typed via [`ScaffoldError`] (thiserror) so callers can
//! match on them in tests. The CLI boundary in `main.rs` converts
//! into `anyhow::Error` for free via `?`. Every error variant carries
//! enough context (path, name, underlying IO error) to be actionable
//! in a terminal without the operator hunting through logs.

use std::path::{Path, PathBuf};

use crate::templates::{substitute, TEMPLATES};

/// Errors surfaced by [`scaffold_project`].
///
/// Variants are intentionally narrow so tests can assert on the
/// specific failure mode without string-matching error messages.
#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    /// The destination path's final component could not be derived
    /// (e.g. a path that ends in `..` or `/`).
    #[error("could not derive a project name from path `{0}` — pass a path whose final component is the project name")]
    NameFromPath(PathBuf),

    /// The derived (or supplied) project name does not satisfy the
    /// SPEC §9.1 slug rule. The same rule is also Cargo's crate-name
    /// rule for the shape we accept, so a single check covers both.
    #[error(
        "project name `{0}` is not a valid slug — must be lowercase ASCII alphanumeric or `-`, \
         must not start or end with `-`, and must be non-empty (e.g. `murder-motel`)"
    )]
    InvalidName(String),

    /// The destination already exists and is not an empty directory.
    /// We refuse to merge into a non-empty directory rather than risk
    /// clobbering files the operator cares about.
    #[error(
        "destination `{0}` already exists and is not empty — pass a fresh path or empty directory"
    )]
    DestinationNotEmpty(PathBuf),

    /// An IO error from the host filesystem. Wrapped so callers can
    /// distinguish IO failures from validation failures without
    /// downcasting.
    #[error("filesystem error at `{path}`: {source}")]
    Io {
        /// Path the failing IO call was targeting.
        path: PathBuf,
        /// The underlying [`std::io::Error`].
        #[source]
        source: std::io::Error,
    },
}

/// Result alias scoped to scaffolder operations.
pub type ScaffoldResult<T> = Result<T, ScaffoldError>;

/// Create a fresh game project at `dest` using `dest`'s final path
/// component as the `{name}` substitution.
///
/// This is the convenience wrapper most callers want; it keeps the
/// CLI surface to a single positional argument. If you need to
/// override the name independently of the path (e.g. when generating
/// into a directory that's already named differently), use
/// [`scaffold_project_with_name`].
pub fn scaffold_project(dest: &Path) -> ScaffoldResult<()> {
    let name = name_from_path(dest)?;
    scaffold_project_with_name(dest, &name)
}

/// Create a fresh game project at `dest` with an explicit project
/// name (used as the `{name}` substitution).
///
/// `dest` may either not exist, or exist as an empty directory; any
/// other state is rejected via [`ScaffoldError::DestinationNotEmpty`].
/// Intermediate parent directories are created as needed (the
/// equivalent of `mkdir -p`) so callers don't have to pre-stage the
/// path tree.
pub fn scaffold_project_with_name(dest: &Path, name: &str) -> ScaffoldResult<()> {
    if !is_valid_slug(name) {
        return Err(ScaffoldError::InvalidName(name.to_string()));
    }

    ensure_empty_destination(dest)?;
    create_dir_all(dest)?;

    for template in TEMPLATES {
        let rel = Path::new(template.dest_path);
        let abs = dest.join(rel);
        if let Some(parent) = abs.parent() {
            create_dir_all(parent)?;
        }
        let body = substitute(template.contents, name);
        write_file(&abs, body.as_bytes())?;
    }

    Ok(())
}

/// Derive the project name from the destination path's final
/// component.
///
/// Public so the CLI can validate the name *before* doing anything
/// destructive (e.g. printing it back to the operator) without
/// reaching into the scaffolder's private internals.
pub fn name_from_path(dest: &Path) -> ScaffoldResult<String> {
    dest.file_name()
        .and_then(|os| os.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| ScaffoldError::NameFromPath(dest.to_path_buf()))
}

/// SPEC §9.1 slug rule, mirrored locally so the scaffolder can
/// reject bad names *before* writing any files.
///
/// Kept in sync with the validator inside `foglet_game::config`. We
/// duplicate the rule rather than re-export it to avoid widening the
/// `foglet_game` public surface for a CLI-only concern; if the rule
/// drifts, a unit test in 10a (`game_toml_template_parses_as_game_config`)
/// fails fast because the rendered `assets/game.toml` would no longer
/// pass `GameConfig` validation.
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

/// Refuse to scaffold into a non-empty directory. An empty existing
/// directory is fine — operators occasionally `mkdir my-game && cd my-game`
/// before running `fgk new .`.
fn ensure_empty_destination(dest: &Path) -> ScaffoldResult<()> {
    match std::fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(ScaffoldError::DestinationNotEmpty(dest.to_path_buf()))
            } else {
                Ok(())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ScaffoldError::Io {
            path: dest.to_path_buf(),
            source: e,
        }),
    }
}

fn create_dir_all(path: &Path) -> ScaffoldResult<()> {
    std::fs::create_dir_all(path).map_err(|e| ScaffoldError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> ScaffoldResult<()> {
    std::fs::write(path, bytes).map_err(|e| ScaffoldError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical good name. Mirrors the SPEC §9.1 example (`murder-motel`)
    /// without colliding with the actual sample game's slug.
    const SAMPLE_NAME: &str = "test-game";

    fn fresh_dest(td: &tempfile::TempDir, name: &str) -> PathBuf {
        // Use a sub-path so the TempDir itself remains a clean root —
        // makes "destination must be empty" assertions cleaner.
        td.path().join(name)
    }

    #[test]
    fn slug_validator_accepts_canonical_examples() {
        assert!(is_valid_slug("murder-motel"));
        assert!(is_valid_slug("door1"));
        assert!(is_valid_slug("a"));
        assert!(is_valid_slug(SAMPLE_NAME));
    }

    #[test]
    fn slug_validator_rejects_bad_inputs() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-leading"));
        assert!(!is_valid_slug("trailing-"));
        assert!(!is_valid_slug("Has_Underscore"));
        assert!(!is_valid_slug("CapitalCase"));
        assert!(!is_valid_slug("with space"));
        assert!(!is_valid_slug("dot.path"));
    }

    #[test]
    fn name_from_path_uses_final_component() {
        assert_eq!(
            name_from_path(Path::new("/tmp/foo/test-game")).unwrap(),
            "test-game"
        );
        assert_eq!(name_from_path(Path::new("test-game")).unwrap(), "test-game");
    }

    #[test]
    fn scaffold_creates_every_template_file() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        scaffold_project(&dest).expect("scaffold should succeed");

        for template in TEMPLATES {
            let path = dest.join(template.dest_path);
            assert!(path.is_file(), "expected file at {}", path.display());
        }
    }

    #[test]
    fn scaffold_substitutes_name_in_cargo_toml() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        scaffold_project(&dest).unwrap();

        let cargo = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&cargo).unwrap();
        let pkg_name = parsed
            .get("package")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(pkg_name, SAMPLE_NAME);
    }

    #[test]
    fn scaffold_produces_game_toml_that_passes_game_config_validation() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        scaffold_project(&dest).unwrap();

        let body = std::fs::read_to_string(dest.join("assets/game.toml")).unwrap();
        let cfg = foglet_game::GameConfig::from_toml_str(&body)
            .expect("scaffolded game.toml must pass GameConfig validation");
        assert_eq!(cfg.game.slug, SAMPLE_NAME);
    }

    #[test]
    fn scaffold_into_existing_empty_dir_succeeds() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        std::fs::create_dir_all(&dest).unwrap();
        scaffold_project(&dest).expect("empty existing dir is fine");
        assert!(dest.join("Cargo.toml").is_file());
    }

    #[test]
    fn scaffold_into_non_empty_dir_is_rejected() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("EXISTING"), "hi").unwrap();

        let err = scaffold_project(&dest).unwrap_err();
        assert!(
            matches!(err, ScaffoldError::DestinationNotEmpty(_)),
            "expected DestinationNotEmpty, got {err:?}"
        );
        // The pre-existing file must remain untouched on a rejected
        // scaffold — no partial writes.
        assert_eq!(
            std::fs::read_to_string(dest.join("EXISTING")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn scaffold_rejects_invalid_name() {
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, "Bad_Name");

        let err = scaffold_project(&dest).unwrap_err();
        assert!(
            matches!(err, ScaffoldError::InvalidName(ref n) if n == "Bad_Name"),
            "expected InvalidName(\"Bad_Name\"), got {err:?}"
        );
        // Invalid name must short-circuit *before* anything is
        // created on disk.
        assert!(
            !dest.exists(),
            "scaffolder must not create the dest dir on validation failure"
        );
    }

    #[test]
    fn scaffold_creates_intermediate_parent_dirs() {
        let td = tempfile::tempdir().unwrap();
        let dest = td.path().join("nested/parents/test-game");
        scaffold_project(&dest).expect("nested parents should be created");
        assert!(dest.join("Cargo.toml").is_file());
    }

    #[test]
    fn scaffold_starter_main_rs_parses_as_rust() {
        // Regression guard: the scaffolder must produce a main.rs
        // that's at least syntactically valid Rust. Full-build
        // validation is the manual smoke step documented in 10b's
        // commit body — too slow to run on every `cargo test`.
        let td = tempfile::tempdir().unwrap();
        let dest = fresh_dest(&td, SAMPLE_NAME);
        scaffold_project(&dest).unwrap();

        let main_rs = std::fs::read_to_string(dest.join("src/main.rs")).unwrap();
        syn::parse_file(&main_rs).expect("scaffolded main.rs must parse");
    }
}
