//! `murder_motel` — the SPEC §13 acceptance-fixture sample game.
//!
//! Run with `cargo run --example murder_motel`. Source is split across
//! domain modules so `fgk new`-generated scaffolds read as a multi-file
//! template rather than one giant `main.rs`.
//!
//! Asset paths resolve against [`CARGO_MANIFEST_DIR`] so the example
//! runs the same regardless of CWD. Standalone scaffolds copied out of
//! this tree should swap the constant for `"assets/game.toml"`.
//!
//! [`CARGO_MANIFEST_DIR`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html

// Domain modules expose `pub` items purely for their own `#[cfg(test)]`
// blocks; silence the lint on non-test builds so genuinely dead code
// in test builds still trips it.
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

/// Absolute path to `examples/murder_motel/assets/game.toml`,
/// resolved at compile time so test fixtures and live runs agree.
pub const GAME_TOML_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/murder_motel/assets/game.toml"
);

fn main() -> anyhow::Result<()> {
    let config = GameConfig::load(GAME_TOML_PATH)?;
    let foglet = load_context(process_env)?;

    // Load (or fail to parse) the save before the TUI takes the
    // terminal so a malformed save exits cleanly on stderr.
    let save_path = resolve_save_path(
        &SavePathInputs {
            slug: &config.game.slug,
            strategy: config.save.strategy,
            context: &foglet,
            cli_override: None,
        },
        process_env,
    )?;
    // Build the slots directly from the loaded SaveState so the
    // per-field Rc aliases share identity with the SaveSlot's inner
    // state from frame zero. `SaveSlot::load_or_default` is the right
    // tool when `T` stays opaque — see DECISIONS.md "Task 8c".
    let slots = match save_path.as_deref() {
        Some(path) => match read_save::<SaveState>(path)? {
            Some(loaded) => SharedSlots::with_save_state(loaded),
            None => SharedSlots::default(),
        },
        None => SharedSlots::default(),
    };

    // Persistence runs through the runtime save-handler hook so the
    // disk write happens inside the terminal guard's lifetime. Skipping
    // the hook when `save_path` is `None` preserves v2's "no path → no
    // persistence" semantics (SPEC §13.5).
    let mut game = Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen::with_slots(slots.clone())));
    if let Some(path) = save_path.as_deref() {
        game = game.with_save_handler(slots.save.save_handler(path.to_path_buf()));
    }
    game.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Proves `main`'s wiring chain: mutations through per-field Rc
    //! aliases must land in the file the handler writes on Quit drain.
    //! `BuiltGame::save_handler` is crate-private, so we install the
    //! same `SaveSlot::save_handler` closure as `on_save` directly —
    //! the builder→runtime plumbing is covered by the runtime crate.
    use crate::state::{SaveState, SharedSlots};
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{
        run_with_io, EventSource, ExitReason, Game, GameContext, GameError, Input, Screen,
        ScreenCommand,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::VecDeque;
    use std::time::Duration;
    use tempfile::tempdir;

    struct QuitOnInput;
    impl Screen for QuitOnInput {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
        fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
            ScreenCommand::Quit
        }
    }

    struct VecEvents {
        events: VecDeque<Option<Input>>,
    }
    impl EventSource for VecEvents {
        fn next_input(&mut self, _: Duration) -> std::io::Result<Option<Input>> {
            Ok(self.events.pop_front().flatten())
        }
    }

    /// Mutating `slots` through a per-field Rc alias must land in the
    /// file the runtime save handler writes on Quit drain.
    #[test]
    fn save_handler_persists_slot_on_quit_drain() {
        let dir = tempdir().expect("tempdir");
        let save_path = dir.path().join("save.json");

        let slots = SharedSlots::default();
        slots.player.borrow_mut().cash = 1234;

        let built = Game::new("save-handler-test".to_string())
            .min_size(80, 24)
            .with_config(fixture_config())
            .with_foglet_context(fixture_context())
            .push_screen(Box::new(QuitOnInput))
            .build()
            .expect("build");
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
