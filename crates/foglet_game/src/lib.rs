//! `foglet_game` — authoring kit for Foglet `:external_pty` terminal
//! door games.
//!
//! This crate's role, in one sentence: provide the runtime, terminal
//! safety guarantees, and primitives a game author needs so their
//! `main.rs` is "wire up screens, hand control to `Game::run()`".
//!
//! # Module map (target — populated across the implementation tasks)
//!
//! The eventual module layout follows SPEC §6:
//!
//! - `terminal` — raw-mode/alt-screen guard (Task 5)
//! - `foglet`  — `FogletContext` loader (Task 2)
//! - `input`   — `crossterm` event → `Input` normalization (Task 6, done)
//! - `screen`  — `Screen` trait + `ScreenCommand` (Task 7)
//! - `runtime` — top-level `Game` builder + loop (Task 7)
//! - `save`    — atomic save manager (Task 8)
//! - `world`, `map`, `entity`, `dialog`, `widgets` — primitives
//!   (Task 9)
//! - `error`   — library-internal `thiserror` types
//!
//! Task 1 only stands up the crate so the workspace builds. Modules
//! land alongside the tasks that exercise them.
//!
//! # Stability
//!
//! Pre-1.0. The public API is allowed to break between minor versions
//! while we converge on the SPEC §8 contract. Breaking changes will
//! be called out in commit messages and (eventually) `CHANGELOG.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod config;
pub mod foglet;
pub mod input;
pub mod manifest;
pub mod screen;
pub mod terminal;

pub use config::{
    ConfigError, GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy,
};
pub use foglet::{
    load_context, load_context_from_env, load_context_from_file, load_context_with_options,
    process_env, synthesize_local_dev, ContextError, ContextSource, FogletContext, LoadOptions,
};
pub use input::{from_event, from_key_event, Input};
pub use manifest::{
    FogletManifest, ManifestError, ManifestInputs, DEFAULT_AUTH_SCOPE, DEFAULT_IDLE_TIMEOUT_MS,
    DEFAULT_TIMEOUT_MS, DEFAULT_VISIBILITY, RUNTIME_EXTERNAL_PTY,
};
pub use screen::{
    apply_command, ExitReason, GameContext, Screen, ScreenCommand, ScreenStack, SideEffect,
};
pub use terminal::{
    arm_panic_hook, disarm_panic_hook, flush_stdout, install_panic_hook, install_panic_hook_with,
    is_panic_hook_armed, CrosstermBackend, PanicRestoreFn, TerminalBackend, TerminalError,
    TerminalGuard,
};

/// Crate version string, sourced from `Cargo.toml` at build time.
///
/// Exposed primarily so the `fgk` CLI and example games can print
/// "built against foglet_game vX.Y.Z" diagnostics. Authoring code
/// usually has no reason to read this directly.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check: the crate compiles and `VERSION` is wired to the
    /// Cargo manifest. Replaced with real coverage as modules land.
    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty(), "CARGO_PKG_VERSION should be set");
    }
}
