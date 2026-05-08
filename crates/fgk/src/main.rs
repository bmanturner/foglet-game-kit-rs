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
//!   - `fgk package`               ← Task 12 (not yet wired)

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New { path } => run_new(path),
        Command::EmitManifest {
            install_dir,
            project,
        } => run_emit_manifest(&project, &install_dir),
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
