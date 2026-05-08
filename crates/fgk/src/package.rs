//! `fgk package --out <dir>` — assemble a deployable Foglet door bundle.
//!
//! SPEC §10.4 says: given a scaffolded project, produce a directory
//! containing the release binary, a boring `run.sh` wrapper,
//! `manifest.json`, and a copy of `assets/`. This module is the
//! library half of that subcommand; `main.rs` is the thin CLI shell.
//!
//! ## Two entry points
//!
//! - [`package_project`] is the full pipeline an operator wants:
//!   `cargo build --release` in the project, then assemble the bundle
//!   from the produced binary. This is what `fgk package` runs.
//! - [`assemble_bundle`] is the same pipeline minus the build step —
//!   it takes a path to an already-built binary. Tests use this to
//!   exercise the layout/exec-bit/run.sh logic without paying for a
//!   release build per assertion. Operators with bespoke build
//!   pipelines (cross-compile, distroless, etc.) can call it via
//!   `--binary <path>`.
//!
//! ## What we do *not* do
//!
//! - We do not strip, sign, or compress the binary. SPEC §13.3 leaves
//!   sandboxing/hardening to the Foglet manifest and host deployment.
//! - We do not interpolate any user-supplied data into `run.sh` beyond
//!   the validated slug. SPEC §10.4 explicitly forbids it; the slug is
//!   already constrained to lowercase ASCII alphanumerics and `-` by
//!   `GameConfig` validation, so there is no shell-metacharacter
//!   surface to escape.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use foglet_game::{ConfigError, GameConfig};

use crate::emit_manifest::{self, EmitManifestError, GAME_TOML_RELATIVE};

/// Boring, auditable wrapper script per SPEC §10.4.
///
/// Stored as a single static string so the bytes the loop produces
/// match the SPEC example character-for-character (modulo the slug
/// substitution). The only placeholder is `{slug}`; everything else
/// is fixed shell.
///
/// **Do not** add new placeholders here without revisiting SPEC §10.4
/// — the wrapper's auditability story rests on the file being short
/// enough to read in one sitting and free of any path that interpolates
/// arbitrary user input.
const RUN_SH_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
USER_ID="${FOGLET_USER_ID:-local-dev}"

exec "$DIR/{slug}" \
  --assets "$DIR/assets" \
  --save-dir "${FGK_SAVE_DIR:-$DIR/saves/$USER_ID}"
"#;

/// Errors surfaced by the packaging pipeline.
///
/// Library-internal `thiserror` per the project convention; the CLI
/// boundary in `main.rs` converts to `anyhow::Error` via `?`. Variants
/// carry enough context (paths, exit codes, underlying IO error) to be
/// actionable without trawling logs.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    /// Failed to load `assets/game.toml` from the project root. We
    /// echo the path so the operator knows which project we tried to
    /// read.
    #[error("failed to load game config from `{path}`: {source}")]
    Config {
        /// The full path we tried to read.
        path: String,
        /// Underlying parse/IO error from the foglet_game crate.
        #[source]
        source: ConfigError,
    },

    /// Manifest assembly failed (relative `--install-dir`, missing
    /// config, validation rejection, etc.). Forwarded verbatim so the
    /// operator gets the same error text whether they call
    /// `emit-manifest` or `package`.
    #[error(transparent)]
    Manifest(#[from] EmitManifestError),

    /// The output directory already exists and is not empty. We refuse
    /// to merge into a populated directory rather than risk clobbering
    /// files the operator cares about.
    #[error("output directory `{0}` already exists and is not empty — pass a fresh path or empty directory")]
    OutDirNotEmpty(PathBuf),

    /// The release binary expected at `path` does not exist. Most
    /// commonly: `cargo build --release` succeeded but the binary lives
    /// at a different name than the slug. The operator can fix this by
    /// passing `--binary <path>` with the actual location.
    #[error("expected release binary at `{0}` but it does not exist — pass `--binary <path>` if your project builds it elsewhere")]
    BinaryMissing(PathBuf),

    /// `cargo build --release` returned a non-zero exit status. The
    /// status code is included so CI can distinguish "build failed"
    /// from "we couldn't even spawn cargo".
    #[error("`cargo build --release` failed (exit status {0}) — see cargo's stderr above")]
    CargoBuildFailed(i32),

    /// We could not spawn `cargo`. Almost always a missing toolchain
    /// or PATH issue on the build host.
    #[error("failed to spawn `cargo`: {0}")]
    CargoSpawn(#[source] std::io::Error),

    /// Generic IO failure (mkdir, copy, write, chmod). Echoes the
    /// path so the operator can tell which step in the pipeline blew
    /// up.
    #[error("filesystem error at `{path}`: {source}")]
    Io {
        /// Path the failing call was targeting.
        path: PathBuf,
        /// Underlying [`std::io::Error`].
        #[source]
        source: std::io::Error,
    },
}

/// Result alias scoped to packaging operations.
pub type PackageResult<T> = Result<T, PackageError>;

/// Inputs to [`package_project`] / [`assemble_bundle`].
///
/// Grouped into a struct (rather than a long positional argument list)
/// so future fields — code-signing, cross-target, additional asset
/// roots — can land without breaking call sites.
#[derive(Debug, Clone)]
pub struct PackageInputs<'a> {
    /// Project directory (the one containing `assets/game.toml`).
    pub project_dir: &'a Path,
    /// Output directory the bundle is written to. Must be empty or
    /// non-existent.
    pub out_dir: &'a Path,
    /// Absolute path the door will be installed at on the Foglet host
    /// (typically `/srv/foglet/doors/<slug>`). When `None`, defaults to
    /// `/srv/foglet/doors/<slug>` derived from the project's slug.
    pub install_dir: Option<&'a str>,
}

/// Filesystem layout produced by a successful packaging run.
///
/// Returned to the CLI so the success message can name the exact
/// artifact paths an operator needs to copy into place. Each path is
/// absolute (via the same canonicalization we apply to `out_dir`).
#[derive(Debug, Clone)]
pub struct PackageOutputs {
    /// Absolute path to the bundle root (the `--out` directory).
    pub out_dir: PathBuf,
    /// `<out>/<slug>` — the executable game binary.
    pub binary: PathBuf,
    /// `<out>/run.sh` — the SPEC §10.4 wrapper script.
    pub run_sh: PathBuf,
    /// `<out>/manifest.json` — the Foglet operator manifest.
    pub manifest: PathBuf,
    /// `<out>/assets/` — directory containing the project's assets.
    pub assets: PathBuf,
    /// Slug derived from the project's `game.toml`, exposed so callers
    /// (the CLI summary line, future packagers) don't have to re-read
    /// the config.
    pub slug: String,
}

/// Full pipeline: build the project's release binary, then assemble.
///
/// Invokes `cargo build --release` in `project_dir` and resolves the
/// produced binary at `<project_dir>/target/release/<slug>`. Use
/// [`assemble_bundle`] directly if your build flow lives outside cargo.
pub fn package_project(inputs: PackageInputs<'_>) -> PackageResult<PackageOutputs> {
    let slug = load_slug(inputs.project_dir)?;
    cargo_build_release(inputs.project_dir)?;
    let binary = inputs
        .project_dir
        .join("target")
        .join("release")
        .join(&slug);
    assemble_bundle(inputs, &binary)
}

/// Assemble pipeline: copy a pre-built binary plus assets, render
/// `run.sh`, write `manifest.json`.
///
/// `binary` MUST already exist; we don't run cargo here. This is the
/// path tests take, and the path operators take when they pass
/// `--binary <path>` (e.g. for cross-compiled or stripped binaries).
pub fn assemble_bundle(inputs: PackageInputs<'_>, binary: &Path) -> PackageResult<PackageOutputs> {
    let PackageInputs {
        project_dir,
        out_dir,
        install_dir,
    } = inputs;

    if !binary.is_file() {
        return Err(PackageError::BinaryMissing(binary.to_path_buf()));
    }

    let config = load_config(project_dir)?;
    let slug = config.game.slug.clone();
    let install_dir_owned = match install_dir {
        Some(s) => s.to_string(),
        None => format!("/srv/foglet/doors/{slug}"),
    };

    ensure_out_dir(out_dir)?;

    // Copy the binary into place under its slug name. We re-copy even
    // if the source is already named `<slug>` so the destination's
    // permissions are ours to set rather than inherited from cargo.
    let dest_binary = out_dir.join(&slug);
    copy_file(binary, &dest_binary)?;
    set_executable(&dest_binary)?;

    // Render and write run.sh. SPEC §10.4 requires the wrapper be
    // executable; without the +x bit Foglet would run it via /bin/sh
    // anyway, but operators expect to be able to invoke it directly.
    let run_sh_path = out_dir.join("run.sh");
    let run_sh_body = render_run_sh(&slug);
    write_file(&run_sh_path, run_sh_body.as_bytes())?;
    set_executable(&run_sh_path)?;

    // Manifest. Reuse `emit_manifest_json` so the bytes match what
    // `fgk emit-manifest` would print — single source of truth.
    let manifest_path = out_dir.join("manifest.json");
    let manifest_body = emit_manifest::emit_manifest_json(project_dir, &install_dir_owned)?;
    write_file(&manifest_path, manifest_body.as_bytes())?;

    // Assets. Copy the entire tree so authors can ship maps, dialog
    // YAML, art, etc., without enumerating each file in the kit.
    let dest_assets = out_dir.join("assets");
    let src_assets = project_dir.join("assets");
    copy_dir_recursive(&src_assets, &dest_assets)?;

    Ok(PackageOutputs {
        out_dir: out_dir.to_path_buf(),
        binary: dest_binary,
        run_sh: run_sh_path,
        manifest: manifest_path,
        assets: dest_assets,
        slug,
    })
}

/// Render the SPEC §10.4 wrapper for a given slug.
///
/// Public so tests (and any future operator-facing diagnostic that
/// wants to preview the wrapper before writing it) can call it without
/// going through the full assemble pipeline.
pub fn render_run_sh(slug: &str) -> String {
    RUN_SH_TEMPLATE.replace("{slug}", slug)
}

/// Read just the slug out of `assets/game.toml`.
///
/// Used by the build phase, which needs the slug *before* we have a
/// reason to load the full manifest. Echoing this through the same
/// `Config` error variant keeps error messages aligned with the
/// assemble step.
fn load_slug(project_dir: &Path) -> PackageResult<String> {
    Ok(load_config(project_dir)?.game.slug)
}

fn load_config(project_dir: &Path) -> PackageResult<GameConfig> {
    let path = project_dir.join(GAME_TOML_RELATIVE);
    GameConfig::load(&path).map_err(|source| PackageError::Config {
        path: path.display().to_string(),
        source,
    })
}

/// Refuse to package into a non-empty directory. An empty existing
/// directory is fine — operators occasionally `mkdir dist && fgk package --out dist`.
fn ensure_out_dir(out_dir: &Path) -> PackageResult<()> {
    match fs::read_dir(out_dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(PackageError::OutDirNotEmpty(out_dir.to_path_buf()))
            } else {
                Ok(())
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(out_dir).map_err(|source| PackageError::Io {
                path: out_dir.to_path_buf(),
                source,
            })
        }
        Err(source) => Err(PackageError::Io {
            path: out_dir.to_path_buf(),
            source,
        }),
    }
}

/// Run `cargo build --release` in the project dir. Inherits stdio so
/// the operator sees cargo's progress in real time; we only need the
/// exit status.
fn cargo_build_release(project_dir: &Path) -> PackageResult<()> {
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(project_dir)
        .status()
        .map_err(PackageError::CargoSpawn)?;

    if status.success() {
        Ok(())
    } else {
        // `code()` is None on Unix when the process was killed by a
        // signal. Map that to -1 so the error variant has a single i32
        // to render; the user-visible message is still actionable.
        Err(PackageError::CargoBuildFailed(status.code().unwrap_or(-1)))
    }
}

fn copy_file(src: &Path, dest: &Path) -> PackageResult<()> {
    fs::copy(src, dest)
        .map(|_| ())
        .map_err(|source| PackageError::Io {
            path: dest.to_path_buf(),
            source,
        })
}

fn write_file(path: &Path, bytes: &[u8]) -> PackageResult<()> {
    fs::write(path, bytes).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Recursive directory copy. Standard library doesn't ship one, and
/// pulling in `fs_extra` for ~30 lines of code is more dependency
/// surface than the kit needs.
fn copy_dir_recursive(src: &Path, dest: &Path) -> PackageResult<()> {
    fs::create_dir_all(dest).map_err(|source| PackageError::Io {
        path: dest.to_path_buf(),
        source,
    })?;

    let entries = fs::read_dir(src).map_err(|source| PackageError::Io {
        path: src.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PackageError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source| PackageError::Io {
            path: from.clone(),
            source,
        })?;

        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            copy_file(&from, &to)?;
        }
        // Symlinks intentionally skipped: a packaged door must be
        // self-contained on the operator host. If a project relies on
        // symlinked assets, that's a build-pipeline concern, not a
        // packaging one.
    }
    Ok(())
}

/// Set the executable bit on Unix. No-op on non-Unix because Windows
/// has no posix exec bit; SPEC's deployment target is Linux Foglet
/// hosts, but we keep the kit cross-platform for development.
#[cfg(unix)]
fn set_executable(path: &Path) -> PackageResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|source| PackageError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    // 0o755: rwxr-xr-x — readable+executable for everyone, writable
    // only by owner. Matches what `chmod +x` produces on a freshly
    // 0o644 file.
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|source| PackageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> PackageResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Verbatim SPEC §9.1 game config — same fixture
    /// `emit_manifest::tests` uses, replicated so packaging tests are
    /// self-contained.
    const SPEC_GAME_TOML: &str = r#"
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

    /// Materialize a project tree with `assets/game.toml` plus a
    /// nested asset file so `copy_dir_recursive` is actually exercised.
    fn project_with_assets(toml: &str) -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        let assets = dir.path().join("assets");
        let maps = assets.join("maps");
        fs::create_dir_all(&maps).expect("mkdir assets/maps");
        fs::write(assets.join("game.toml"), toml).expect("write game.toml");
        fs::write(maps.join("lobby.txt"), "..#..\n.....\n").expect("write lobby.txt");
        dir
    }

    /// Materialize a fake "release binary" — a tiny shell script the
    /// tests can copy, set +x on, and inspect. Avoids paying for a
    /// real `cargo build --release` per assertion.
    fn fake_binary(td: &TempDir, name: &str) -> PathBuf {
        let p = td.path().join(name);
        fs::write(&p, b"#!/bin/sh\necho fake\n").expect("write fake binary");
        p
    }

    #[test]
    fn run_sh_matches_spec_10_4_for_canonical_slug() {
        // SPEC §10.4 example, byte-for-byte. If this ever drifts the
        // diff is auditable in one screen.
        let expected = "#!/usr/bin/env bash\n\
                        set -euo pipefail\n\
                        \n\
                        DIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
                        USER_ID=\"${FOGLET_USER_ID:-local-dev}\"\n\
                        \n\
                        exec \"$DIR/murder-motel\" \\\n  \
                        --assets \"$DIR/assets\" \\\n  \
                        --save-dir \"${FGK_SAVE_DIR:-$DIR/saves/$USER_ID}\"\n";
        assert_eq!(render_run_sh("murder-motel"), expected);
    }

    #[test]
    fn run_sh_substitutes_slug() {
        let body = render_run_sh("test-game");
        assert!(body.contains(r#"exec "$DIR/test-game""#));
        assert!(!body.contains("{slug}"));
    }

    #[test]
    fn assemble_writes_full_layout() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");

        let outputs = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &bin,
        )
        .expect("assemble");

        assert_eq!(outputs.slug, "murder-motel");
        assert!(outputs.binary.is_file(), "binary missing");
        assert!(outputs.run_sh.is_file(), "run.sh missing");
        assert!(outputs.manifest.is_file(), "manifest.json missing");
        assert!(outputs.assets.is_dir(), "assets/ missing");

        // Asset tree is copied verbatim, including nested paths.
        let copied_map = outputs.assets.join("maps").join("lobby.txt");
        assert!(copied_map.is_file(), "nested asset not copied");
        assert_eq!(
            fs::read_to_string(&copied_map).unwrap(),
            "..#..\n.....\n",
            "asset content must match the source"
        );
    }

    #[test]
    fn assemble_marks_binary_and_run_sh_executable() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");

        let outputs = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &bin,
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin_mode = fs::metadata(&outputs.binary).unwrap().permissions().mode();
            let run_mode = fs::metadata(&outputs.run_sh).unwrap().permissions().mode();
            assert_eq!(bin_mode & 0o777, 0o755, "binary must be 0o755");
            assert_eq!(run_mode & 0o777, 0o755, "run.sh must be 0o755");
        }

        // On non-unix the +x notion doesn't apply; fall through.
        let _ = outputs;
    }

    #[test]
    fn assemble_writes_valid_manifest_json() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");

        let outputs = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &bin,
        )
        .unwrap();

        let body = fs::read_to_string(&outputs.manifest).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).expect("manifest is valid JSON");
        assert_eq!(v["slug"], "murder-motel");
        assert_eq!(v["command"], "/srv/foglet/doors/murder-motel/run.sh");
        assert_eq!(v["working_dir"], "/srv/foglet/doors/murder-motel");
        assert_eq!(v["runtime"], "external_pty");
    }

    #[test]
    fn assemble_defaults_install_dir_from_slug_when_unset() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");

        let outputs = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: None,
            },
            &bin,
        )
        .unwrap();

        let body = fs::read_to_string(&outputs.manifest).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["command"], "/srv/foglet/doors/murder-motel/run.sh");
    }

    #[test]
    fn assemble_rejects_relative_install_dir() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");

        let err = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("relative/path"),
            },
            &bin,
        )
        .expect_err("relative install_dir must be rejected");

        assert!(
            matches!(
                err,
                PackageError::Manifest(EmitManifestError::InstallDirNotAbsolute(_))
            ),
            "expected InstallDirNotAbsolute, got {err:?}"
        );
    }

    #[test]
    fn assemble_rejects_non_empty_out_dir() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");
        fs::create_dir_all(&out).unwrap();
        fs::write(out.join("EXISTING"), b"hi").unwrap();

        let err = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &bin,
        )
        .expect_err("non-empty out_dir must be rejected");

        assert!(matches!(err, PackageError::OutDirNotEmpty(_)));
        // Pre-existing file must be untouched on rejection.
        assert_eq!(fs::read_to_string(out.join("EXISTING")).unwrap(), "hi");
    }

    #[test]
    fn assemble_into_existing_empty_dir_succeeds() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let bin_td = TempDir::new().unwrap();
        let bin = fake_binary(&bin_td, "murder-motel");
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");
        fs::create_dir_all(&out).unwrap();

        let outputs = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &bin,
        )
        .expect("empty existing dir is fine");
        assert!(outputs.binary.is_file());
    }

    #[test]
    fn assemble_errors_when_binary_missing() {
        let project = project_with_assets(SPEC_GAME_TOML);
        let out_td = TempDir::new().unwrap();
        let out = out_td.path().join("dist");
        let missing = out_td.path().join("nope");

        let err = assemble_bundle(
            PackageInputs {
                project_dir: project.path(),
                out_dir: &out,
                install_dir: Some("/srv/foglet/doors/murder-motel"),
            },
            &missing,
        )
        .expect_err("missing binary must error");

        assert!(matches!(err, PackageError::BinaryMissing(_)));
        // Out dir must not have been created when validation fails
        // before we reach `ensure_out_dir`.
        assert!(!out.exists(), "out_dir created despite validation failure");
    }

    /// Sanity-check that the produced wrapper is at least syntactically
    /// valid bash. `bash -n` exits non-zero on parse errors without
    /// executing the script.
    #[cfg(unix)]
    #[test]
    fn rendered_run_sh_passes_bash_syntax_check() {
        let body = render_run_sh("murder-motel");
        let td = TempDir::new().unwrap();
        let p = td.path().join("run.sh");
        fs::write(&p, &body).unwrap();

        let status = Command::new("bash").arg("-n").arg(&p).status();
        // If bash isn't installed (rare on dev hosts), skip rather
        // than fail — the syntactic invariant is still asserted by
        // the byte-equality test above.
        if let Ok(status) = status {
            assert!(status.success(), "bash -n rejected the rendered run.sh");
        }
    }
}
