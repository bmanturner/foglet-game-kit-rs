//! `murder_motel` — the SPEC §13 acceptance-fixture sample game.
//!
//! Each Task 13 sub-iteration grows this binary one screen at a time.
//! Task 13a stood up the project scaffold and a title screen with a
//! menu hint; Task 13b (this commit) adds a real main menu and a
//! controls/help screen behind it. Later sub-tasks layer the map,
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
    load_context, process_env, render_menu_list, Game, GameConfig, GameContext, Input, MenuList,
    Screen, ScreenCommand,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
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
/// Shows the configured title, a one-line tagline, and a "Press Enter"
/// prompt that leads into the main menu added in Task 13b. Esc / Q /
/// Ctrl-C still quit directly so a player who lands on the title screen
/// with no patience for menus has an immediate exit.
#[derive(Debug, Default)]
pub struct TitleScreen;

impl TitleScreen {
    /// The label rendered in the bordered title block. Pulled out as a
    /// constant so the integration test can assert it without touching
    /// the runtime.
    pub const HEADING: &'static str = "Murder Motel";
    /// Single-line tagline shown directly under the heading.
    pub const TAGLINE: &'static str = "A noir whodunit at the edge of town.";
    /// Prompt rendered beneath the tagline. The title screen is now a
    /// pure splash gate — Enter advances to the real menu.
    pub const ENTER_HINT: &'static str = "Press Enter to begin   ([Q] Quit)";
}

impl Screen for TitleScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Three centred lines: heading, tagline, prompt. Plain
        // `Paragraph` is enough for a splash; the live menu lives one
        // screen deeper in `MainMenuScreen`.
        let lines = vec![
            Line::from(Span::styled(
                Self::HEADING,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Self::TAGLINE),
            Line::from(""),
            Line::from(Self::ENTER_HINT),
        ];
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(Self::HEADING));
        frame.render_widget(widget, frame.area());
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Enter advances into the main menu. We push rather than
            // replace so a future "back to title" affordance from the
            // menu is a one-line `Pop` away if we ever want it.
            Input::Enter => ScreenCommand::Push(Box::new(MainMenuScreen::new())),
            // Direct quit affordances. Esc + Q + Ctrl-C match the
            // controls every other screen in the kit honors.
            Input::Esc | Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Identifies a row in the main menu by intent rather than by index, so
/// activation logic doesn't read like "if selected == 2 then push help".
///
/// Stays in lockstep with [`MainMenuScreen::ITEMS`]; the order of those
/// labels and the order of these variants is the same on purpose so the
/// screen can map between them with a plain `as usize` cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainMenuItem {
    /// Start a fresh game. Until Task 13c lands the map screen, this
    /// is a clean quit so the menu is honest about what it can deliver.
    NewGame,
    /// Resume a saved game. Until Task 13h lands save persistence, this
    /// behaves identically to [`Self::NewGame`].
    Continue,
    /// Push the help/controls screen.
    Help,
    /// Exit the runtime.
    Quit,
}

impl MainMenuItem {
    /// Convert a row index into a [`MainMenuItem`]. Out-of-range values
    /// are treated as no-op so a stray activation never panics.
    fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::NewGame),
            1 => Some(Self::Continue),
            2 => Some(Self::Help),
            3 => Some(Self::Quit),
            _ => None,
        }
    }

    /// The [`ScreenCommand`] this menu item should produce when
    /// activated. Centralised here so [`MainMenuScreen`] can react to
    /// both Enter-on-selection and the per-item shortcut keys with one
    /// call.
    fn activate(self) -> ScreenCommand {
        match self {
            // New Game / Continue stay placeholders until Task 13c and
            // 13h land respectively; quitting is the most honest thing
            // they can do today and matches the title-screen behaviour
            // the previous iteration shipped.
            Self::NewGame | Self::Continue => ScreenCommand::Quit,
            Self::Help => ScreenCommand::Push(Box::new(HelpScreen)),
            Self::Quit => ScreenCommand::Quit,
        }
    }
}

/// Top-level main menu shown after the title splash.
///
/// Owns the item label vector and the selection cursor. Rendering goes
/// through the shared [`MenuList`] widget so the visual baseline stays
/// consistent with later screens (dialog choices, inventory selection)
/// that will reuse the same widget.
#[derive(Debug)]
pub struct MainMenuScreen {
    /// Cached item labels owned by the screen. The widget borrows this
    /// each frame; storing it on the screen avoids re-allocating four
    /// `String`s per render.
    items: Vec<String>,
    /// Index of the currently highlighted row. `MenuList` clamps for us
    /// so out-of-range values are safe, but we still maintain it
    /// faithfully so keyboard navigation feels correct.
    selected: usize,
}

impl MainMenuScreen {
    /// Title rendered on the menu's bordered block.
    pub const TITLE: &'static str = "Main Menu";
    /// Order of menu items. Kept aligned with [`MainMenuItem`] so the
    /// row index can be cast straight to an item.
    pub const ITEMS: [&'static str; 4] = ["New Game", "Continue", "Help", "Quit"];

    /// Build a menu with the cursor on `New Game`.
    pub fn new() -> Self {
        Self {
            items: Self::ITEMS.iter().map(|s| s.to_string()).collect(),
            selected: 0,
        }
    }

    /// Move the selection cursor by `delta`, clamping at both ends.
    /// Pulled out of `handle_input` so tests can exercise the cursor
    /// behaviour directly.
    fn move_cursor(&mut self, delta: i32) {
        let len = self.items.len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }
}

impl Default for MainMenuScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for MainMenuScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre a fixed-size menu inside the frame so the look is the
        // same regardless of terminal dimensions (within the SPEC §7.1
        // 80x24 floor).
        let area = centred_rect(40, 8, frame.area());
        let menu = MenuList {
            title: Some(Self::TITLE),
            items: &self.items,
            selected: self.selected,
        };
        render_menu_list(frame, area, &menu);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Vertical navigation. We deliberately accept both arrows
            // and the classic `j`/`k` so vi-flavoured BBS callers feel
            // at home — same as we'll do on the map screen later.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.move_cursor(-1);
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.move_cursor(1);
                ScreenCommand::None
            }
            // Activate the highlighted item.
            Input::Enter => MainMenuItem::from_index(self.selected)
                .map(MainMenuItem::activate)
                .unwrap_or(ScreenCommand::None),
            // Per-item shortcuts mirror the first letter of each label.
            // `Q` doubles as a generic quit affordance — both meanings
            // resolve to `ScreenCommand::Quit` so there's no ambiguity.
            Input::Char('n') | Input::Char('N') => MainMenuItem::NewGame.activate(),
            Input::Char('c') | Input::Char('C') => MainMenuItem::Continue.activate(),
            Input::Char('h') | Input::Char('H') => MainMenuItem::Help.activate(),
            Input::Char('q') | Input::Char('Q') | Input::Esc | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Static help / controls reference, pushed from the main menu.
///
/// SPEC §13 calls out a help screen as part of the Task 13b acceptance
/// fixture; it lists the controls every subsequent screen will rely on
/// (movement, selection, save, quit) so a player who lands here knows
/// what to expect across the whole game.
#[derive(Debug, Default)]
pub struct HelpScreen;

impl HelpScreen {
    /// Block title used in render and in tests.
    pub const TITLE: &'static str = "Controls";
    /// Lines shown in the body. Stored as `&'static str` so they're
    /// trivially testable and so the screen does no allocation per
    /// frame.
    pub const LINES: &'static [&'static str] = &[
        "Move          Arrow keys, or h/j/k/l",
        "Select        Enter",
        "Back / Cancel Esc or Backspace",
        "Save          S",
        "Quit          Q  (Ctrl-C also works anywhere)",
        "",
        "Press Esc or Backspace to return to the main menu.",
    ];
}

impl Screen for HelpScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let area = centred_rect(60, (Self::LINES.len() as u16) + 2, frame.area());
        let lines: Vec<Line<'_>> = Self::LINES.iter().map(|s| Line::from(*s)).collect();
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Esc / Backspace pop back to the menu beneath us. Q and
            // Ctrl-C still quit outright so the help screen never traps
            // a player who just wants out.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            _ => ScreenCommand::None,
        }
    }
}

/// Centre a `width × height` rectangle inside `outer`, clamping the
/// inner size if `outer` is smaller than requested.
///
/// Pulled out so both the menu and the help screen lay out the same
/// way; SPEC §7.1 already guarantees an 80x24 floor so the clamping
/// path only matters for the unit tests that hand in tiny `TestBackend`
/// frames.
fn centred_rect(width: u16, height: u16, outer: Rect) -> Rect {
    let w = width.min(outer.width);
    let h = height.min(outer.height);
    let h_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((outer.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(outer);
    let v_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((outer.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(h_layout[1]);
    v_layout[1]
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
    //! Tests exercise the pure-input contract of every screen without
    //! standing up a real terminal. These run under `cargo test
    //! --examples` (and `cargo test --workspace --all-targets`) but not
    //! the bare `cargo test --workspace` gate — the workspace gate is
    //! covered by the integration test at
    //! `crates/foglet_game/tests/murder_motel_scaffold.rs`, which
    //! verifies the assets parse and the example target is registered.

    use super::*;
    use foglet_game::{ContextSource, FogletContext};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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

    /// Dispatch one input to the title screen.
    fn dispatch_title(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        TitleScreen.handle_input(&mut ctx, input)
    }

    /// Dispatch one input to a fresh main menu.
    fn dispatch_menu(input: Input) -> (MainMenuScreen, ScreenCommand) {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        let cmd = menu.handle_input(&mut ctx, input);
        (menu, cmd)
    }

    /// Dispatch one input to a fresh help screen.
    fn dispatch_help(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        HelpScreen.handle_input(&mut ctx, input)
    }

    // ---- TitleScreen ---------------------------------------------------

    #[test]
    fn title_esc_quits() {
        assert!(matches!(dispatch_title(Input::Esc), ScreenCommand::Quit));
    }

    #[test]
    fn title_lowercase_q_quits() {
        assert!(matches!(
            dispatch_title(Input::Char('q')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn title_ctrl_c_quits() {
        assert!(matches!(
            dispatch_title(Input::Ctrl('c')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn title_enter_pushes_main_menu() {
        // Enter is the only "advance" affordance on the title screen
        // now that the real menu exists. Pushing (rather than replacing)
        // keeps the title splash on the stack so a future "back to
        // title" can be a one-line Pop.
        assert!(matches!(
            dispatch_title(Input::Enter),
            ScreenCommand::Push(_)
        ));
    }

    #[test]
    fn title_unrelated_keys_are_inert() {
        // Arrow keys, random letters — anything not part of the title
        // affordances should not transition state. Keeps the title
        // screen safe against stray keystrokes during launch.
        for input in [
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Char('x'),
            Input::Char('1'),
            Input::Char('n'),
            Input::Char('h'),
        ] {
            assert!(
                matches!(dispatch_title(input), ScreenCommand::None),
                "{input:?} should not transition from the title splash"
            );
        }
    }

    // ---- MainMenuScreen ------------------------------------------------

    #[test]
    fn menu_starts_on_first_item() {
        let menu = MainMenuScreen::new();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.items.len(), MainMenuScreen::ITEMS.len());
    }

    #[test]
    fn menu_down_advances_cursor() {
        let (menu, cmd) = dispatch_menu(Input::Down);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn menu_up_clamps_at_top() {
        let (menu, cmd) = dispatch_menu(Input::Up);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(menu.selected, 0, "Up at the top must stay at the top");
    }

    #[test]
    fn menu_down_clamps_at_bottom() {
        // Hammer Down past the end of the list and confirm we stop at
        // the last row instead of wrapping or panicking.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        for _ in 0..10 {
            menu.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(menu.selected, MainMenuScreen::ITEMS.len() - 1);
    }

    #[test]
    fn menu_vi_keys_navigate() {
        // `j`/`k` mirror Down/Up. We assert one full round trip so a
        // future regression that wires them to the wrong direction
        // surfaces here.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.handle_input(&mut ctx, Input::Char('j'));
        menu.handle_input(&mut ctx, Input::Char('j'));
        assert_eq!(menu.selected, 2);
        menu.handle_input(&mut ctx, Input::Char('k'));
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn menu_enter_on_help_pushes_help_screen() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 2; // Help
        let cmd = menu.handle_input(&mut ctx, Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "Enter on Help must push HelpScreen, got {cmd:?}"
        );
    }

    #[test]
    fn menu_enter_on_quit_quits() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 3; // Quit
        assert!(matches!(
            menu.handle_input(&mut ctx, Input::Enter),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn menu_enter_on_new_game_quits_until_map_lands() {
        // Documents the placeholder behaviour: until Task 13c lands,
        // New Game has nowhere to push to, so it quits cleanly. If this
        // test ever fails because 13c hooked up the map screen, update
        // it alongside the new behaviour — don't loosen the assertion.
        let (_, cmd) = dispatch_menu(Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Quit));
    }

    #[test]
    fn menu_h_shortcut_pushes_help_regardless_of_cursor() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 0; // cursor on New Game
        let cmd = menu.handle_input(&mut ctx, Input::Char('h'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        // Cursor stays put — shortcut activation must not move the
        // visual selection.
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn menu_quit_keys_quit() {
        for key in [
            Input::Char('q'),
            Input::Char('Q'),
            Input::Esc,
            Input::Ctrl('c'),
        ] {
            let (_, cmd) = dispatch_menu(key);
            assert!(
                matches!(cmd, ScreenCommand::Quit),
                "{key:?} should quit the main menu, got {cmd:?}"
            );
        }
    }

    #[test]
    fn menu_renders_into_test_backend() {
        // End-to-end render proof: draw the menu into a TestBackend and
        // confirm the title and every item appear in the buffer. Locks
        // in the contract that `MainMenuScreen` actually paints itself.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| menu.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(
            found.contains(MainMenuScreen::TITLE),
            "menu render missing title; buffer was:\n{found}"
        );
        for item in MainMenuScreen::ITEMS {
            assert!(
                found.contains(item),
                "menu render missing item {item:?}; buffer was:\n{found}"
            );
        }
    }

    // ---- HelpScreen ----------------------------------------------------

    #[test]
    fn help_esc_pops() {
        // Esc takes the player back to the menu beneath us — a Pop, not
        // a Quit. Conflating those would lose the menu state on every
        // accidental Esc.
        assert!(matches!(dispatch_help(Input::Esc), ScreenCommand::Pop));
    }

    #[test]
    fn help_backspace_pops() {
        assert!(matches!(
            dispatch_help(Input::Backspace),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn help_q_still_quits() {
        // Q remains a hard quit even from inside the help screen so the
        // controls reference doesn't trap a player who just wants out.
        assert!(matches!(
            dispatch_help(Input::Char('q')),
            ScreenCommand::Quit
        ));
        assert!(matches!(
            dispatch_help(Input::Ctrl('c')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn help_unrelated_keys_are_inert() {
        for input in [
            Input::Up,
            Input::Down,
            Input::Enter,
            Input::Char('x'),
            Input::Char('1'),
        ] {
            assert!(
                matches!(dispatch_help(input), ScreenCommand::None),
                "{input:?} should not transition from the help screen"
            );
        }
    }

    #[test]
    fn help_renders_into_test_backend() {
        // Prove the help body actually paints. We assert the screen
        // title plus a sentinel substring from one of the body lines —
        // good enough to catch a future regression that drops the
        // paragraph entirely without locking in incidental whitespace.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut help = HelpScreen;
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| help.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(found.contains(HelpScreen::TITLE));
        assert!(
            found.contains("Arrow keys"),
            "expected help body to mention 'Arrow keys'; buffer was:\n{found}"
        );
    }
}
