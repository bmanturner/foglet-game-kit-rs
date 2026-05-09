//! Title splash and main menu — the first two screens the player sees.
//!
//! Both screens hold a [`SharedSlots`] handle so the slots loaded by
//! `main` survive the title → menu → map pushes without a static.

use foglet_game::{render_menu_list, GameContext, Input, MenuList, Screen, ScreenCommand};
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::layout::centred_rect;
use crate::map::MapScreen;
use crate::modals::HelpScreen;
use crate::state::SharedSlots;

/// First screen the player sees on launch.
///
/// Shows the configured title, a one-line tagline, and a "Press Enter"
/// prompt that leads into the main menu added in Task 13b. Esc / Q /
/// Ctrl-C still quit directly so a player who lands on the title screen
/// with no patience for menus has an immediate exit.
#[derive(Default)]
pub struct TitleScreen {
    /// Shared runtime state propagated into the [`MainMenuScreen`] when
    /// the player presses Enter. Held on the title so the slots loaded
    /// by `main` (Task 13h) survive the title → menu transition without
    /// a static.
    slots: SharedSlots,
}

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

    /// Build a title screen wired to the supplied shared slots.
    pub fn with_slots(slots: SharedSlots) -> Self {
        Self { slots }
    }
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
            Input::Enter => {
                ScreenCommand::Push(Box::new(MainMenuScreen::with_slots(self.slots.clone())))
            }
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
    /// Start a fresh game. Pushes the lobby [`MapScreen`] with the
    /// player at the spawn coordinates from `assets/game.toml`.
    NewGame,
    /// Resume a saved game. Until Task 13h lands save persistence,
    /// "continue" is identical to [`Self::NewGame`] — push a fresh
    /// lobby. The placeholder is honest about its limits: the screen
    /// is reachable, but the state is not yet restored from disk.
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
    ///
    /// `ctx` is borrowed so New Game / Continue can read the spawn
    /// coordinates from `assets/game.toml` rather than hard-coding
    /// them — the map screen pushes with whatever `start_x` /
    /// `start_y` the config carries, which keeps the example honest
    /// if those values are ever retuned.
    fn activate(self, ctx: &GameContext<'_>, slots: &SharedSlots) -> ScreenCommand {
        let (sx, sy) = (ctx.config.game.start_x, ctx.config.game.start_y);
        match self {
            // New Game wipes the shared slots so a leftover loaded save
            // does not bleed in, then pushes a fresh map at the
            // configured spawn.
            Self::NewGame => {
                slots.reset(sx, sy);
                ScreenCommand::Push(Box::new(MapScreen::with_shared(sx, sy, slots.clone())))
            }
            // Continue keeps whatever state the slots already carry —
            // either the loaded save (if `main` populated them at
            // startup) or the same default-zero state New Game would
            // otherwise have built. The walkability fallback inside
            // `MapScreen::with_shared` keeps an empty-default Continue
            // safe even when no save was loaded.
            Self::Continue => {
                ScreenCommand::Push(Box::new(MapScreen::with_shared(sx, sy, slots.clone())))
            }
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
    /// Shared runtime state. Routed into `MainMenuItem::activate` so
    /// "New Game" can reset the slots and "Continue" can pass the
    /// already-populated slots straight to a fresh [`MapScreen`].
    slots: SharedSlots,
}

impl MainMenuScreen {
    /// Title rendered on the menu's bordered block.
    pub const TITLE: &'static str = "Main Menu";
    /// Order of menu items. Kept aligned with [`MainMenuItem`] so the
    /// row index can be cast straight to an item.
    pub const ITEMS: [&'static str; 4] = ["New Game", "Continue", "Help", "Quit"];

    /// Build a menu with the cursor on `New Game` and freshly defaulted
    /// shared slots. Used by tests and by callers that don't need to
    /// participate in the save/load handshake. The non-test entry point
    /// uses [`Self::with_slots`] so the slots threaded through `main`
    /// reach the activate path.
    pub fn new() -> Self {
        Self::with_slots(SharedSlots::default())
    }

    /// Build a menu wired to the supplied shared slots. The slots are
    /// the bridge between the disk save (loaded by `main`) and the map
    /// screen the menu pushes — see [`MainMenuItem::activate`].
    pub fn with_slots(slots: SharedSlots) -> Self {
        Self {
            items: Self::ITEMS.iter().map(|s| s.to_string()).collect(),
            selected: 0,
            slots,
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

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
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
                .map(|item| item.activate(ctx, &self.slots))
                .unwrap_or(ScreenCommand::None),
            // Per-item shortcuts mirror the first letter of each label.
            // `Q` doubles as a generic quit affordance — both meanings
            // resolve to `ScreenCommand::Quit` so there's no ambiguity.
            Input::Char('n') | Input::Char('N') => MainMenuItem::NewGame.activate(ctx, &self.slots),
            Input::Char('c') | Input::Char('C') => {
                MainMenuItem::Continue.activate(ctx, &self.slots)
            }
            Input::Char('h') | Input::Char('H') => MainMenuItem::Help.activate(ctx, &self.slots),
            Input::Char('q') | Input::Char('Q') | Input::Esc | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            _ => ScreenCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the title splash and main menu — the input contracts,
    //! cursor movement, shortcut routing, and the New Game / Continue
    //! handshake with the shared slot bundle.

    use super::*;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::GameContext;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Dispatch one input to the title screen.
    fn dispatch_title(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        TitleScreen::default().handle_input(&mut ctx, input)
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
    fn menu_enter_on_new_game_pushes_map_screen() {
        // Task 13c wires the map screen behind New Game. Before 13c
        // landed this asserted `Quit`; the contract is now to push a
        // screen so the main menu actually leads somewhere. The
        // companion `n` shortcut is tested separately below.
        let (_, cmd) = dispatch_menu(Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "Enter on New Game must push the map screen, got {cmd:?}"
        );
    }

    #[test]
    fn menu_n_shortcut_pushes_map_screen() {
        let (_, cmd) = dispatch_menu(Input::Char('n'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
    }

    #[test]
    fn menu_enter_on_continue_pushes_map_screen() {
        // Continue is a 13h-shaped placeholder today: same destination
        // as New Game until persistence lands. The screen is reachable
        // from the menu so the player isn't left at a dead end.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 1; // Continue
        let cmd = menu.handle_input(&mut ctx, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Push(_)));
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

    // ---- New Game / Continue handshake with shared slots --------------

    #[test]
    fn main_menu_new_game_resets_slots_before_pushing_map() {
        // "New Game" wipes whatever was in the slots so a stale loaded
        // save cannot leak items or flags into the new run.
        let slots = SharedSlots::default();
        slots.flags.borrow_mut().insert("heard_rumor".into());
        slots.inventory.borrow_mut().insert("brass_key".into());
        let mut menu = MainMenuScreen::with_slots(slots.clone());
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = menu.handle_input(&mut ctx, Input::Char('n'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        assert!(slots.flags.borrow().is_empty());
        assert!(slots.inventory.borrow().is_empty());
        let p = slots.player.borrow();
        assert_eq!((p.x, p.y), (cfg.game.start_x, cfg.game.start_y));
    }

    #[test]
    fn main_menu_continue_preserves_slots() {
        // "Continue" must not touch the slots — they came from the
        // loaded save and the map screen reads them on construction.
        let slots = SharedSlots::default();
        slots.flags.borrow_mut().insert("heard_rumor".into());
        slots.inventory.borrow_mut().insert("brass_key".into());
        slots.player.borrow_mut().x = 39;
        slots.player.borrow_mut().y = 5;
        let mut menu = MainMenuScreen::with_slots(slots.clone());
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = menu.handle_input(&mut ctx, Input::Char('c'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        assert!(slots.flags.borrow().contains("heard_rumor"));
        assert!(slots.inventory.borrow().contains("brass_key"));
        assert_eq!(slots.player.borrow().x, 39);
    }
}
