//! Static / single-purpose modal screens: help, win, inventory.
//!
//! Grouped because each one is small enough not to warrant its own
//! file but they share no state: the help screen is a fixed reference
//! card, the win screen is the terminal "case closed" beat, and the
//! inventory screen reads from the shared inventory store the map
//! screen writes to.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{
    render_inventory_list, GameContext, Input, InventoryList, Screen, ScreenCommand,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::layout::centred_rect;
use crate::map::MapScreen;

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
        "Talk          Enter (when standing next to an NPC)",
        "Select        Enter",
        "Inventory     I",
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

/// Terminal "you win" modal pushed when the player solves the case
/// (Task 13g).
///
/// Reaching the win modal requires the player to have collected the
/// brass key (otherwise Room 4 is unreachable) and to have asked the
/// Night Clerk about the murder (otherwise the win flag is unset). In
/// other words: the modal can only appear if the player has actually
/// played through the SPEC §13 acceptance loop.
///
/// The screen is intentionally terminal — any keystroke quits the
/// runtime. A future Task 13h save loop can introduce a "play again"
/// affordance; for the 13g acceptance fixture, "Press any key to quit"
/// is the simplest faithful end-state.
#[derive(Debug, Default)]
pub struct WinScreen;

impl WinScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Case Closed";
    /// Body lines for the modal. Stored as `&'static str` so the screen
    /// is allocation-free per frame and tests can assert exact strings.
    pub const LINES: &'static [&'static str] = &[
        "You found the smoking gun. Lipstick on the wall spells out a name.",
        "",
        "The Night Clerk shrugs. 'Knew it'd be one of 'em. Lock up on your way out.'",
        "",
        "[Any key] quit",
    ];
}

impl Screen for WinScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre the modal. Width matches the longest line plus border
        // padding; height is the line count plus two for the border so
        // every line is visible on an exactly-80x24 terminal.
        let area = centred_rect(72, (Self::LINES.len() as u16) + 2, frame.area());
        let lines: Vec<Line<'_>> = Self::LINES.iter().map(|s| Line::from(*s)).collect();
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
        // Any key quits — the modal is the end of the game.
        ScreenCommand::Quit
    }
}

/// Modal inventory screen pushed from the [`MapScreen`] via `i`.
///
/// Reads from the same `Rc<RefCell<BTreeSet<String>>>` the map screen
/// writes to, so the screen always reflects the current pocket
/// contents. Looks each ID up in [`MapScreen::ITEMS`] to recover its
/// display name; items in the set without a catalog entry are skipped
/// silently — that shape is unreachable today but a forward-compatible
/// no-op keeps the screen safe across future Task 13h save migrations
/// that might surface a stale ID.
///
/// ## Render contract
///
/// Items show as a [`InventoryList`] with the screen title on the
/// border and a "(nothing in your pockets)" empty hint when the set
/// is empty. A one-row band beneath the list shows close affordances.
///
/// ## Input contract
///
/// - `Up` / `Down` (and `j`/`k`) move the highlight cursor.
/// - `Esc` / `Backspace` / `i` pop back to the map.
/// - `Q` / `Ctrl-C` still hard-quit, matching the rest of the kit.
pub struct InventoryScreen {
    /// Shared store cloned from [`MapScreen::inventory`] at construction.
    inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Highlighted row. Clamped against the visible item count at
    /// render time so an item collected mid-frame never points the
    /// cursor off the end of the list.
    selected: usize,
}

impl InventoryScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Inventory";
    /// Empty-state hint shown when the player's pockets are empty.
    /// Pulled out as a constant so the renderer test can assert it
    /// without binding to incidental wording elsewhere in the code.
    pub const EMPTY_HINT: &'static str = "(nothing in your pockets)";

    /// Build an inventory screen sharing the supplied store.
    pub fn new(inventory: Rc<RefCell<BTreeSet<String>>>) -> Self {
        Self {
            inventory,
            selected: 0,
        }
    }

    /// Display labels for each currently held item, in catalog order.
    ///
    /// Walks the static [`MapScreen::ITEMS`] catalog rather than
    /// iterating the set directly so the UI order is stable and
    /// independent of insertion order. Tests use this to assert
    /// "exactly the picked-up items appear, with their canonical
    /// names" without going through a `Frame`.
    pub fn current_labels(&self) -> Vec<String> {
        let held = self.inventory.borrow();
        MapScreen::ITEMS
            .iter()
            .filter(|item| held.contains(item.id))
            .map(|item| item.name.to_string())
            .collect()
    }
}

impl Screen for InventoryScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let labels = self.current_labels();
        // Modal sized for the longest possible item label plus the
        // selection gutter, with vertical room for all five items
        // and a hint row. Clamps inside `centred_rect` if the
        // terminal is smaller (e.g. `TestBackend` fixtures).
        let outer = frame.area();
        let area = centred_rect(36, (MapScreen::ITEMS.len() as u16) + 4, outer);

        // Reserve a one-row hint band at the bottom of the modal so
        // the controls are always visible regardless of inventory
        // contents.
        let hint_h = 1.min(area.height);
        let body_h = area.height.saturating_sub(hint_h);
        let body = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: body_h,
        };
        let hint = Rect {
            x: area.x,
            y: area.y + body_h,
            width: area.width,
            height: hint_h,
        };

        let selected = if labels.is_empty() {
            0
        } else {
            self.selected.min(labels.len() - 1)
        };
        let inv = InventoryList {
            title: Some(Self::TITLE),
            items: &labels,
            selected,
            empty_hint: Self::EMPTY_HINT,
        };
        render_inventory_list(frame, body, &inv);
        if hint_h > 0 {
            let widget = Paragraph::new("[Up/Down] choose    [Esc/I] close")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(widget, hint);
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on hard-quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Esc / Backspace / `i` close the modal — `i` toggles so
            // the same keystroke that opens the screen also closes
            // it, which is the muscle-memory shortcut every BBS
            // inventory screen has trained players to expect.
            Input::Esc | Input::Backspace | Input::Char('i') | Input::Char('I') => {
                ScreenCommand::Pop
            }
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                let len = self.inventory.borrow().len();
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                }
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the static help, terminal win, and modal inventory
    //! screens. Each test stands the screen up directly and dispatches
    //! one input — no map / dialog state is required.

    use super::*;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::GameContext;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Dispatch one input to a fresh help screen.
    fn dispatch_help(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        HelpScreen.handle_input(&mut ctx, input)
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

    // ---- WinScreen -----------------------------------------------------

    #[test]
    fn win_screen_quits_on_any_key() {
        // The terminal modal must quit on every reasonable keystroke.
        // Iterating a representative cross-section is enough to lock
        // in the contract without enumerating every Input variant.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for key in [
            Input::Enter,
            Input::Esc,
            Input::Char('q'),
            Input::Char('x'),
            Input::Up,
            Input::Backspace,
        ] {
            let mut screen = WinScreen;
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} must quit the win screen"
            );
        }
    }

    #[test]
    fn win_screen_renders_into_test_backend() {
        // Sanity-check that the modal actually paints. We assert the
        // title and a sentinel substring of the body so a future
        // regression that drops the paragraph entirely surfaces here.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = WinScreen;
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(found.contains(WinScreen::TITLE));
        assert!(
            found.contains("smoking gun"),
            "expected win body in render; buffer was:\n{found}"
        );
    }

    // ---- InventoryScreen ----------------------------------------------

    #[test]
    fn inventory_empty_renders_hint() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
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
            found.contains(InventoryScreen::EMPTY_HINT),
            "empty inventory must surface the empty-hint; buffer was:\n{found}"
        );
    }

    #[test]
    fn inventory_lists_collected_item_names() {
        // Stuff two known IDs into the shared store and confirm the
        // screen renders their canonical names from the catalog.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("matchbook".to_string());
        inv.borrow_mut().insert("brass_key".to_string());
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        let labels = screen.current_labels();
        assert_eq!(
            labels,
            vec!["Brass key".to_string(), "Matchbook".to_string()],
            "labels must follow catalog order, not insertion order"
        );
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(found.contains(InventoryScreen::TITLE));
        assert!(
            found.contains("Matchbook"),
            "Matchbook must render in the inventory list; buffer was:\n{found}"
        );
        assert!(
            found.contains("Brass key"),
            "Brass key must render in the inventory list; buffer was:\n{found}"
        );
    }

    #[test]
    fn inventory_unknown_id_is_skipped() {
        // Forward-compatibility: a stale ID from a future save must
        // not crash the screen — it just doesn't render.
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("ghost_item".to_string());
        let screen = InventoryScreen::new(Rc::clone(&inv));
        assert!(
            screen.current_labels().is_empty(),
            "unknown IDs must be silently ignored"
        );
    }

    #[test]
    fn inventory_esc_pops() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Esc),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn inventory_i_toggles_closed() {
        // The same key that opens the screen also closes it — muscle
        // memory shortcut every BBS inventory does.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Char('i')),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn inventory_quit_keys_quit() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        for key in [Input::Char('q'), Input::Char('Q'), Input::Ctrl('c')] {
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} should quit the inventory screen"
            );
        }
    }

    #[test]
    fn inventory_cursor_clamps() {
        // Selection cursor must not advance past the live item count
        // and must not underflow at zero.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("matchbook".to_string());
        inv.borrow_mut().insert("brass_key".to_string());
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(screen.selected, 1, "cursor must clamp at last row");
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Up);
        }
        assert_eq!(screen.selected, 0, "cursor must clamp at top row");
    }
}
