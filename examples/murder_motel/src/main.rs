//! `murder_motel` — the SPEC §13 acceptance-fixture sample game.
//!
//! Each Task 13 sub-iteration grew this binary one screen at a time.
//! The May 2026 refactor split `main.rs` across domain modules so
//! `fgk new`-generated scaffolds read as a multi-file template instead
//! of one 5,000-line file:
//!
//! - [`state`] holds the on-disk save and the runtime [`SharedSlots`]
//!   bundle every screen mutates;
//! - [`title_menu`] hosts the title splash and main menu;
//! - [`map`] owns the lobby map screen, NPC/item catalogs, and the
//!   embedded map/dialog assets;
//! - [`scenes`] groups push-on-demand modal scenes (NPC dialog, the
//!   two SPEC §9 proof scenes);
//! - [`modals`] groups the small static screens (help, win, inventory);
//! - [`layout`] is the single shared layout helper.
//!
//! ## Running
//!
//! ```bash
//! cargo run --example murder_motel
//! ```
//!
//! The example target lives in `crates/foglet_game/Cargo.toml`; its
//! source path points at this directory so the project also reads as
//! a self-contained scaffold an author can copy as a starting point.
//!
//! ## Asset loading
//!
//! `cargo run --example` sets the current working directory to the
//! workspace root, but tests, IDEs, and downstream callers can run the
//! binary from anywhere. We resolve `assets/` against
//! [`CARGO_MANIFEST_DIR`] (the example's host package — `foglet_game`)
//! so the path is stable regardless of CWD. If you copy this scaffold
//! into a standalone Cargo project, swap the constant for a plain
//! `"assets/game.toml"` — `fgk new` already does that for the
//! generated template.
//!
//! [`CARGO_MANIFEST_DIR`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html

// Several `pub` items in the domain modules exist to give the
// per-module `#[cfg(test)] mod tests` blocks a stable surface to drive
// (`MapScreen::new_lobby`, `DialogScreen::state`, etc.). They are
// genuinely "test-only public API" — the binary path doesn't reach
// them — so we silence the dead-code lint on non-test builds. Test
// builds still trip the lint if a method goes unused for real,
// because `cargo test` compiles the whole `mod tests` tree.
#![cfg_attr(not(test), allow(dead_code))]

mod clock;
mod layout;
mod map;
mod modals;
mod room_7;
mod scenes;
mod state;
mod title_menu;
mod world;

#[cfg(test)]
mod test_support;

use foglet_game::{
    load_context, process_env, read_save, resolve_save_path, write_atomic, Game, GameConfig,
    SavePathInputs,
};

use crate::state::{SaveState, SharedSlots};
use crate::title_menu::TitleScreen;

/// Absolute path to `examples/murder_motel/assets/game.toml`.
///
/// Computed at compile time from the host package's manifest dir so
/// the example runs the same regardless of where `cargo` was invoked.
/// `pub` so the per-module `#[cfg(test)] mod tests` blocks can load
/// the scaffold config through [`test_support::fixture_config`].
pub const GAME_TOML_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/murder_motel/assets/game.toml"
);

fn main() -> anyhow::Result<()> {
    let config = GameConfig::load(GAME_TOML_PATH)?;
    let foglet = load_context(process_env)?;

    // Resolve where this user's save lives and (if a previous run wrote
    // one) load it before the runtime takes over the terminal. Doing
    // both before `Game::run` means a malformed save surfaces as a
    // clean-error exit on stderr instead of a corrupted post-TUI scroll.
    let save_path = resolve_save_path(
        &SavePathInputs {
            slug: &config.game.slug,
            strategy: config.save.strategy,
            context: &foglet,
            cli_override: None,
        },
        process_env,
    )?;
    let slots = SharedSlots::default();
    if let Some(path) = save_path.as_deref() {
        if let Some(loaded) = read_save::<SaveState>(path)? {
            slots.apply(loaded);
        }
    }

    Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen::with_slots(slots.clone())))
        .run()?;

    // Persist on clean exit. The runtime returns Ok only when a screen
    // emitted Quit (or popped to empty), so reaching this line means
    // the player has finished a session and the slot mutations on
    // `slots` are the canonical state worth keeping. Errors propagate
    // *after* the terminal has been restored — `Game::run` already
    // tore the guard down — so the operator sees a clean message
    // rather than a scrambled one.
    if let Some(path) = save_path {
        write_atomic(&path, &slots.snapshot())?;
    }
    Ok(())
}
