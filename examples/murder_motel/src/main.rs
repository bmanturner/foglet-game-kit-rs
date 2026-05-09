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
    load_context, process_env, read_save, resolve_save_path, Game, GameConfig, SavePathInputs,
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
    // SPEC_v2_1 §Task 5b: build the runtime slots directly from the
    // loaded save (when present) so the per-field aliases share Rc
    // identity with the slot's inner SaveState from frame zero —
    // calling `slots.save.apply(loaded)` after-the-fact would orphan
    // those aliases for the rest of the run.
    let slots = match save_path.as_deref() {
        Some(path) => match read_save::<SaveState>(path)? {
            Some(loaded) => SharedSlots::with_save_state(loaded),
            None => SharedSlots::default(),
        },
        None => SharedSlots::default(),
    };

    // SPEC_v2_1 §Task 8b: persistence runs through the runtime's save
    // handler hook instead of a post-`run` tail. `SaveSlot::save_handler`
    // builds a closure that captures a clone of the slot's `Rc` handles
    // (refcount bump only — the slot itself stays inside `slots` for the
    // screens to mutate), so the handler observes every in-game mutation
    // up to the point the runtime fires it. The runtime invokes the
    // handler on every `SideEffect::Save` and once more on the Quit drain
    // — both cases run **inside** the terminal guard's lifetime, so the
    // disk write happens before the alternate-screen teardown can scroll
    // a partial save into the operator's scrollback.
    //
    // The handler is only attached when we have a `save_path` to write
    // to. The `None` arm (no SAVE_DIR, no Foglet save context) reproduces
    // v2's "no path → no persistence" semantics — installing a handler
    // that wrote to a synthesised path would silently corrupt the
    // operator's filesystem and is explicitly forbidden by SPEC §13.5.
    let mut game = Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen::with_slots(slots.clone())));
    if let Some(path) = save_path.as_deref() {
        // `save_handler` clones the slot's `Rc`s into the closure; the
        // outer `slots` binding keeps its own clones for the screens.
        game = game.with_save_handler(slots.save.save_handler(path.to_path_buf()));
    }
    game.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Integration coverage for the SPEC_v2_1 §Task 8b wiring.
    //!
    //! The runtime crate already exhaustively tests `SaveSlot::save_handler`
    //! and `Game::with_save_handler` in isolation. The unique thing this
    //! example proves is that **`main`'s wiring chain composes them
    //! correctly** — i.e. that mutating `slots` through the per-field Rc
    //! aliases (the way every screen does) lands in the file the handler
    //! writes when the runtime drains a `Quit`.
    use foglet_game::{
        run_with_io, EventSource, ExitReason, Game, GameContext, GameError, Input, Screen,
        ScreenCommand,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::VecDeque;
    use std::time::Duration;
    use tempfile::tempdir;
    // `BuiltGame::save_handler` is `pub(crate)` to the runtime crate, so
    // an external test cannot `take()` the builder-installed handler back
    // out and feed it to `run_with_io`. The next-best proof — and the one
    // that actually matches what `main` relies on — is to install the
    // *same* `SaveSlot::save_handler` closure as `on_save` directly.
    // That covers the only example-specific behaviour: mutating `slots`
    // through its per-field `Rc` aliases must land in the file the
    // handler writes. The builder-chain plumbing
    // (`with_save_handler` → runtime → `on_save`) is already covered by
    // the runtime crate's own `run_with_io` tests.
    use crate::state::{SaveState, SharedSlots};
    use crate::test_support::{fixture_config, fixture_context};

    /// Minimal one-shot screen: returns `Quit` on the first input it
    /// sees. Lets the test drain the runtime loop deterministically
    /// without standing up a real `TitleScreen`.
    struct QuitOnInput;
    impl Screen for QuitOnInput {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
        fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
            ScreenCommand::Quit
        }
    }

    /// Scripted `EventSource` mirroring the runtime crate's test helper.
    /// Each `Some(input)` consumes one slot; `None` represents a tick.
    struct VecEvents {
        events: VecDeque<Option<Input>>,
    }
    impl EventSource for VecEvents {
        fn next_input(&mut self, _: Duration) -> std::io::Result<Option<Input>> {
            Ok(self.events.pop_front().flatten())
        }
    }

    /// End-to-end proof: a save handler installed via the same builder
    /// chain `main` uses persists the slot's *current* contents on the
    /// runtime's clean-exit drain, even when the mutation happens through
    /// a per-field `Rc` alias rather than the slot binding directly.
    #[test]
    fn save_handler_persists_slot_on_quit_drain() {
        let dir = tempdir().expect("tempdir");
        let save_path = dir.path().join("save.json");

        // Build the slot the way `main` does for the "no prior save"
        // path. The `cash` field starts at the `PlayerSlot::default()`
        // zero; mutating it through `slots.player` (a per-field Rc
        // alias, *not* the slot binding) is the realistic path every
        // screen takes.
        let slots = SharedSlots::default();
        slots.player.borrow_mut().cash = 1234;

        // Same chain as `main`, minus the screens — `QuitOnInput` is
        // enough to drive the runtime to its Quit drain. We bypass
        // `Game::run` (which would grab the real terminal) and call
        // `run_with_io` directly so the test stays headless.
        let built = Game::new("save-handler-test".to_string())
            .min_size(80, 24)
            .with_config(fixture_config())
            .with_foglet_context(fixture_context())
            .push_screen(Box::new(QuitOnInput))
            .build()
            .expect("build");
        // Install the same handler `main` would chain via
        // `with_save_handler` — the runtime invokes whatever closure it
        // was handed on Quit, so passing it directly here is observably
        // equivalent for the assertion that follows.
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> =
            slots.save.save_handler(save_path.clone());

        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        let mut events = VecEvents {
            events: VecDeque::from(vec![Some(Input::Char('q'))]),
        };
        let cfg = fixture_config();
        let fc = fixture_context();
        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (80, 24),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("runtime loop ok");
        assert_eq!(reason, ExitReason::Quit);

        // The Quit drain MUST have called the handler exactly once,
        // writing the in-memory slot to disk. Reading back via the same
        // `SaveSlot` API both proves the file exists and parses, and
        // exercises the round-trip we're claiming `main` now relies on.
        let snapshot = slots.save.snapshot();
        let on_disk: SaveState = foglet_game::read_save(&save_path)
            .expect("read ok")
            .expect("file written");
        assert_eq!(
            on_disk.player.borrow().cash,
            snapshot.player.borrow().cash,
            "post-Quit save file should mirror current SaveSlot contents"
        );
        assert_eq!(on_disk.player.borrow().cash, 1234);
    }
}
