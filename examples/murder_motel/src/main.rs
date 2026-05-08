//! `murder_motel` — the SPEC §13 acceptance-fixture sample game.
//!
//! Each Task 13 sub-iteration grows this binary one screen at a time.
//! Task 13a (this commit) only stands up the project scaffold and a
//! title screen with a menu hint; later sub-tasks layer the menu, map,
//! NPCs, dialog, inventory, locked door, win condition, and save flow
//! on top of it.
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

use foglet_game::{
    load_context, process_env, Game, GameConfig, GameContext, Input, Screen, ScreenCommand,
};
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Absolute path to `examples/murder_motel/assets/game.toml`.
///
/// Computed at compile time from the host package's manifest dir so
/// the example runs the same regardless of where `cargo` was invoked.
const GAME_TOML_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/murder_motel/assets/game.toml"
);

/// First screen the player sees on launch.
///
/// Shows the configured title, a one-line tagline, and a menu hint
/// pointing at the keys that will become real menu items in Task 13b.
/// Until the menu lands, **Enter** and **N** behave as quit aliases so
/// a player who follows the on-screen instructions never gets stuck.
#[derive(Debug, Default)]
pub struct TitleScreen;

impl TitleScreen {
    /// The label rendered in the bordered title block. Pulled out as a
    /// constant so the integration test can assert it without touching
    /// the runtime.
    pub const HEADING: &'static str = "Murder Motel";
    /// Single-line tagline shown directly under the heading.
    pub const TAGLINE: &'static str = "A noir whodunit at the edge of town.";
    /// Menu hint shown beneath the tagline. The bracketed keys mirror
    /// the menu items Task 13b will introduce; treating them as quit
    /// aliases here keeps the screen honest for early playtesters.
    pub const MENU_HINT: &'static str = "[N]ew Game   [C]ontinue   [H]elp   [Q]uit";
}

impl Screen for TitleScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Three centred lines: heading, tagline, menu hint. Plain
        // `Paragraph` is enough — Task 13b will replace the hint with a
        // real `MenuList` widget once we have one to point at.
        let lines = vec![
            Line::from(Span::styled(
                Self::HEADING,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Self::TAGLINE),
            Line::from(""),
            Line::from(Self::MENU_HINT),
        ];
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(Self::HEADING));
        frame.render_widget(widget, frame.area());
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Direct quit affordances. Esc + Q + Ctrl-C match the
            // controls every other screen in the kit honors.
            Input::Esc | Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            // Until Task 13b stands up the menu, every advertised menu
            // key falls through to a clean quit so the on-screen hint
            // is not a lie. The behavior changes the moment the menu
            // screen exists.
            Input::Enter
            | Input::Char('n')
            | Input::Char('N')
            | Input::Char('c')
            | Input::Char('C')
            | Input::Char('h')
            | Input::Char('H') => ScreenCommand::Quit,
            _ => ScreenCommand::None,
        }
    }
}

fn main() -> anyhow::Result<()> {
    let config = GameConfig::load(GAME_TOML_PATH)?;
    let foglet = load_context(process_env)?;

    Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen))
        .run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Tests exercise the pure-input contract of the title screen
    //! without standing up a real terminal. These run under
    //! `cargo test --examples` (and `cargo test --workspace
    //! --all-targets`) but not the bare `cargo test --workspace`
    //! gate — the workspace gate is covered by the integration test
    //! at `crates/foglet_game/tests/murder_motel_scaffold.rs`, which
    //! verifies the assets parse and the example target is registered.

    use super::*;
    use foglet_game::{ContextSource, FogletContext};

    fn fixture_config() -> GameConfig {
        GameConfig::load(GAME_TOML_PATH).expect("scaffold game.toml parses")
    }

    fn fixture_context() -> FogletContext {
        FogletContext {
            door_id: "murder-motel".into(),
            user_id: Some("u-test".into()),
            username: Some("tester".into()),
            role: None,
            session_id: Some("s-test".into()),
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::LocalDev,
        }
    }

    fn dispatch(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        TitleScreen.handle_input(&mut ctx, input)
    }

    #[test]
    fn esc_quits() {
        assert!(matches!(dispatch(Input::Esc), ScreenCommand::Quit));
    }

    #[test]
    fn lowercase_q_quits() {
        assert!(matches!(dispatch(Input::Char('q')), ScreenCommand::Quit));
    }

    #[test]
    fn ctrl_c_quits() {
        assert!(matches!(dispatch(Input::Ctrl('c')), ScreenCommand::Quit));
    }

    #[test]
    fn advertised_menu_keys_quit_until_menu_lands() {
        // Every key shown in the menu hint must do *something* the
        // player would expect. Until Task 13b lands, that means a
        // clean quit. If this assertion ever fails because Task 13b
        // started routing these keys somewhere else, update the test
        // alongside the new screen — don't loosen it.
        for key in ['n', 'N', 'c', 'C', 'h', 'H'] {
            assert!(
                matches!(dispatch(Input::Char(key)), ScreenCommand::Quit),
                "Char({key:?}) should quit while menu hint is the only affordance"
            );
        }
        assert!(matches!(dispatch(Input::Enter), ScreenCommand::Quit));
    }

    #[test]
    fn unrelated_keys_are_inert() {
        // Arrow keys, random letters — anything not part of the menu
        // hint should not transition state. Keeps the title screen
        // safe against stray keystrokes during launch.
        for input in [
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Char('x'),
            Input::Char('1'),
        ] {
            assert!(matches!(dispatch(input), ScreenCommand::None));
        }
    }
}
