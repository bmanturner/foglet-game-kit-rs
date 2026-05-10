//! `fgk` — the foglet-game-kit CLI entry point.
//!
//! This binary is a thin shell over the [`fgk`] library. The library
//! holds the actual logic (scaffolding, manifest emission, packaging)
//! so unit tests can exercise it without spawning a process; this file
//! only parses arguments via `clap` and dispatches to the right
//! library entry point.
//!
//! Subcommand status (per SPEC §15):
//!   - `fgk new <path>`            ← Task 10b
//!   - `fgk emit-manifest`         ← Task 11
//!   - `fgk package`               ← Task 12
//!   - `fgk tick --project <path>` ← Task 10

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// Operator-facing CLI for foglet-game-kit-rs.
#[derive(Debug, Parser)]
#[command(
    name = "fgk",
    about = "Scaffold and package Foglet door games.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Top-level subcommands. Each variant maps to one library entry
/// point; future subcommands extend this enum without touching the
/// dispatch logic in [`main`].
#[derive(Debug, Subcommand)]
enum Command {
    /// Scaffold a new game project at the given path.
    ///
    /// The final path component is used as both the Cargo crate name
    /// and the SPEC §9.1 game slug, so it must satisfy the slug rule
    /// (lowercase ASCII alphanumeric or `-`, no leading/trailing `-`).
    New {
        /// Destination directory. Must either not exist or be empty.
        path: PathBuf,
    },

    /// Emit a Foglet operator manifest JSON for the current project.
    ///
    /// Reads `<project>/assets/game.toml` and prints the SPEC §10.3
    /// JSON to stdout. Operators redirect into the Foglet manifest
    /// directory (`fgk emit-manifest ... > /etc/foglet/manifests/...`).
    EmitManifest {
        /// Absolute path the door will be installed at on the Foglet
        /// host (typically `/srv/foglet/doors/<slug>`). The manifest's
        /// `command` and `working_dir` are derived from this; relative
        /// paths are rejected to keep Foglet from resolving them
        /// against an unintended CWD (SPEC §10.3 + §13.2).
        #[arg(long, value_name = "ABSOLUTE_PATH")]
        install_dir: String,

        /// Project directory (the one containing `assets/game.toml`).
        /// Defaults to the current directory so authors can run
        /// `fgk emit-manifest --install-dir ...` from inside their
        /// project tree without extra ceremony.
        #[arg(long, default_value = ".", value_name = "DIR")]
        project: PathBuf,
    },

    /// Build and assemble a deployable Foglet door bundle.
    ///
    /// Runs `cargo build --release` in the project, then writes
    /// `<out>/{<slug>, run.sh, manifest.json, assets/}` per SPEC §10.4.
    /// The output directory must be empty or non-existent.
    Package {
        /// Output directory for the bundle. Required so operators
        /// always say where the artifacts go — there is no implicit
        /// `dist/` because we don't want to surprise CI by writing
        /// into the project tree.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,

        /// Project directory (the one containing `assets/game.toml`).
        /// Defaults to the current directory so authors can run
        /// `fgk package --out ...` from inside their project tree.
        #[arg(long, default_value = ".", value_name = "DIR")]
        project: PathBuf,

        /// Absolute install path on the Foglet host. Defaults to
        /// `/srv/foglet/doors/<slug>` derived from the project's slug
        /// — pass this when shipping into a non-default operator
        /// layout.
        #[arg(long, value_name = "ABSOLUTE_PATH")]
        install_dir: Option<String>,

        /// Path to a pre-built release binary. When set, skips
        /// `cargo build --release` and copies this file into
        /// `<out>/<slug>` instead. Useful for cross-compiled or
        /// stripped binaries produced by an external build pipeline.
        #[arg(long, value_name = "PATH")]
        binary: Option<PathBuf>,
    },

    /// Run due world-tick tasks once for a project.
    ///
    /// This is one-shot maintenance for operator cron or manual
    /// invocation; it never runs a daemon loop.
    Tick {
        /// Project directory (the one containing `assets/game.toml`).
        #[arg(long, default_value = ".", value_name = "DIR")]
        project: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New { path } => run_new(path),
        Command::EmitManifest {
            install_dir,
            project,
        } => run_emit_manifest(&project, &install_dir),
        Command::Package {
            out,
            project,
            install_dir,
            binary,
        } => run_package(&project, &out, install_dir.as_deref(), binary.as_deref()),
        Command::Tick { project } => run_tick(&project),
    }
}

/// Entry point for `fgk new <path>`.
///
/// Kept as a free function (rather than inlined into `main`) so the
/// dispatch reads top-down even as more subcommands land. Errors
/// surface to the operator via `anyhow`'s default chain printer; the
/// scaffolder's typed errors implement `std::error::Error` and slot
/// in cleanly.
fn run_new(path: PathBuf) -> Result<()> {
    fgk::scaffold::scaffold_project(&path)
        .with_context(|| format!("failed to scaffold new project at `{}`", path.display()))?;
    println!(
        "Created new Foglet game project at {}\n\
         Next steps:\n  cd {}\n  cargo run",
        path.display(),
        path.display()
    );
    Ok(())
}

/// Entry point for `fgk emit-manifest --install-dir <path>`.
///
/// Writes the rendered JSON straight to stdout via `print!`
/// (the helper already includes a trailing newline) so operators
/// can pipe into a file or `tee` without the CLI adding extra
/// whitespace. The library does the heavy lifting; this wrapper just
/// translates `EmitManifestError` into an `anyhow` chain.
fn run_emit_manifest(project: &std::path::Path, install_dir: &str) -> Result<()> {
    let json = fgk::emit_manifest::emit_manifest_json(project, install_dir).with_context(|| {
        format!(
            "failed to emit manifest for project `{}` (install_dir = `{}`)",
            project.display(),
            install_dir
        )
    })?;
    print!("{json}");
    Ok(())
}

/// Entry point for `fgk package --out <dir>`.
///
/// Branches between the full `package_project` pipeline (cargo build
/// then assemble) and the `assemble_bundle` shortcut (when `--binary`
/// is supplied). The success summary names every artifact path so the
/// operator can copy them into a Foglet door directory without
/// re-deriving the layout.
fn run_package(
    project: &std::path::Path,
    out: &std::path::Path,
    install_dir: Option<&str>,
    binary: Option<&std::path::Path>,
) -> Result<()> {
    let inputs = fgk::package::PackageInputs {
        project_dir: project,
        out_dir: out,
        install_dir,
    };
    let outputs = match binary {
        Some(path) => fgk::package::assemble_bundle(inputs, path),
        None => fgk::package::package_project(inputs),
    }
    .with_context(|| {
        format!(
            "failed to package project `{}` into `{}`",
            project.display(),
            out.display()
        )
    })?;

    println!(
        "Packaged `{}` at {}\n  binary:   {}\n  run.sh:   {}\n  manifest: {}\n  assets:   {}",
        outputs.slug,
        outputs.out_dir.display(),
        outputs.binary.display(),
        outputs.run_sh.display(),
        outputs.manifest.display(),
        outputs.assets.display(),
    );
    Ok(())
}

/// Entry point for `fgk tick --project <path>`.
///
/// This wrapper is intentionally thin so tick scheduling logic stays in
/// `fgk::tick`, where integration tests can drive it directly.
fn run_tick(project: &std::path::Path) -> Result<()> {
    let summary = fgk::tick::run_tick(project)
        .with_context(|| format!("failed to run scheduled tasks for `{}`", project.display()))?;
    println!(
        "Ran {tasks_run} task(s), skipped {tasks_skipped} task(s)",
        tasks_run = summary.tasks_run,
        tasks_skipped = summary.tasks_skipped
    );
    Ok(())
}
