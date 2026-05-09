//! Modal NPC dialog screen and its tests.
//!
//! Owns a parsed [`Dialog`] graph and a [`DialogState`] cursor walking
//! it. The shared [`FlagSet`] is borrowed from the [`MapScreen`]
//! beneath us via an `Rc<RefCell<_>>`, so flags set during this
//! conversation (the Night Clerk's `heard_rumor`, `has_key`, etc.)
//! survive after the screen pops.

use std::cell::RefCell;
use std::rc::Rc;

use foglet_game::{
    load_dialog, render_menu_list, Dialog, DialogState, FlagSet, GameContext, Input, MenuList,
    Screen, ScreenCommand,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::map::Npc;

/// Modal dialog screen pushed when the player talks to an NPC.
///
/// Owns a parsed [`Dialog`] graph and a [`DialogState`] cursor walking
/// it. The shared [`FlagSet`] is borrowed from the [`crate::map::MapScreen`]
/// beneath us via an `Rc<RefCell<_>>`, so flags set during this
/// conversation (the Night Clerk's `heard_rumor`, `has_key`, etc.)
/// survive after the screen pops and are visible to later conversations
/// and to future game logic (locked doors in 13f, win conditions in 13g).
///
/// ## Render contract
///
/// While the cursor sits on a line, the body shows the speaker's name
/// and the line text plus a "[Enter] continue" hint. Once the lines on
/// the current node are exhausted, the body shows the available
/// choices through the shared [`MenuList`] widget. When the dialog has
/// finished (no more lines, no more choices, no goto) the body shows
/// a "[Esc] leave" hint.
///
/// ## Input contract
///
/// - `Up` / `Down` (and `j`/`k`) move the choice cursor when choices
///   are visible.
/// - `Enter` advances the next line, picks the highlighted choice, or
///   pops the screen when the dialog is finished.
/// - `Esc` / `Backspace` pops at any time — the player can always walk
///   away mid-conversation.
/// - `Q` / `Ctrl-C` still hard-quit, matching the rest of the kit.
pub struct DialogScreen {
    /// Speaker label rendered in the dialog block's title. Owned as
    /// `&'static str` because [`Npc`] is `Copy` and lives in
    /// [`crate::map::MapScreen::NPCS`].
    speaker: &'static str,
    /// Parsed dialog graph. Held by value so `DialogState` can borrow
    /// it across multiple input dispatches without lifetime gymnastics.
    dialog: Dialog,
    /// Cursor walking [`Self::dialog`].
    state: DialogState,
    /// Shared narrative-flag store. Cloned from
    /// [`crate::map::MapScreen::flags`] at construction time.
    flags: Rc<RefCell<FlagSet>>,
    /// Highlighted choice when choices are visible. Clamped against
    /// `available_choices().len()` at render time, so it's safe to
    /// keep around even when choices change between frames.
    selected_choice: usize,
}

impl DialogScreen {
    /// Build a dialog screen for the given NPC, sharing the supplied
    /// flag store. The dialog YAML is parsed eagerly here so any
    /// schema error surfaces at the moment the player presses Enter
    /// rather than on the first render.
    ///
    /// `expect` is acceptable because the YAML ships in the binary
    /// (`include_str!`); a parse failure is a build-time bug, not a
    /// runtime input.
    pub fn new(npc: &Npc, flags: Rc<RefCell<FlagSet>>) -> Self {
        let dialog = load_dialog(npc.dialog_yaml).expect("embedded NPC dialog parses");
        let state = {
            let mut fs = flags.borrow_mut();
            DialogState::start(&dialog, &mut fs)
        };
        Self {
            speaker: npc.name,
            dialog,
            state,
            flags,
            selected_choice: 0,
        }
    }

    /// Speaker name. Exposed for tests asserting which NPC is on
    /// screen without poking at private fields.
    pub fn speaker(&self) -> &str {
        self.speaker
    }

    /// Whether the dialog cursor reports finished. Read-only: tests
    /// assert end-state without driving the screen through input.
    pub fn is_finished(&self) -> bool {
        self.state.is_finished()
    }

    /// Borrow the underlying dialog state. Mostly for tests; gameplay
    /// code routes through `handle_input`.
    pub fn state(&self) -> &DialogState {
        &self.state
    }

    /// Snapshot of the choice labels currently presented. Useful for
    /// tests that want to assert "the unlocked branch appears after
    /// the rumor flag is set" without owning a `Frame`.
    pub fn current_choice_labels(&self) -> Vec<String> {
        let flags = self.flags.borrow();
        self.state
            .available_choices(&self.dialog, &flags)
            .into_iter()
            .map(|c| c.text.clone())
            .collect()
    }
}

impl Screen for DialogScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Dialog modal sits across the bottom of the screen — high
        // enough to fit the longest two-line node in the night clerk's
        // script plus a hint row, and wide enough to fit the longest
        // choice label without truncation. We centre it horizontally
        // so the player's eye returns to roughly where the map sat.
        let outer = frame.area();
        let modal_w = outer.width.min(60);
        let modal_h = outer.height.min(10);
        let area = Rect {
            x: outer.x + outer.width.saturating_sub(modal_w) / 2,
            y: outer.y + outer.height.saturating_sub(modal_h) / 2,
            width: modal_w,
            height: modal_h,
        };

        let block = Block::default().borders(Borders::ALL).title(self.speaker);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Body decision: line-pumping mode, choice mode, or finished
        // mode. Each mode renders its own widget into `inner`.
        if let Some(line) = self.state.current_line(&self.dialog) {
            // Reserve a one-row hint band at the bottom of `inner` for
            // the "[Enter] continue" prompt; the rest is the line text
            // wrapped to fit. `Wrap { trim: false }` keeps the
            // author's literal punctuation but still wraps long lines.
            let hint_h = 1.min(inner.height);
            let body_h = inner.height.saturating_sub(hint_h);
            let body = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: body_h,
            };
            let hint = Rect {
                x: inner.x,
                y: inner.y + body_h,
                width: inner.width,
                height: hint_h,
            };
            let body_widget = Paragraph::new(line.to_string())
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false });
            frame.render_widget(body_widget, body);
            if hint_h > 0 {
                frame.render_widget(
                    Paragraph::new("[Enter] continue   [Esc] leave")
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(Color::DarkGray)),
                    hint,
                );
            }
            return;
        }

        if self.is_finished() {
            // Terminal node — the dialog ran off the end. Nothing to
            // render except a leave hint; pressing Esc (or Enter)
            // returns to the map.
            let hint = Paragraph::new("(They turn away.)\n\n[Enter / Esc] leave")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint, inner);
            return;
        }

        // Choice mode. Build owned `String`s for the menu widget and
        // clamp the selection cursor against the live choice count.
        let choice_labels = self.current_choice_labels();
        if choice_labels.is_empty() {
            // Defensive: validator guarantees this shape can only
            // happen if every choice is gated *and* the node has no
            // goto; render a leave hint so the player isn't stuck.
            let hint = Paragraph::new("(There's nothing more to say.)\n\n[Esc] leave")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint, inner);
            return;
        }
        let selected = self.selected_choice.min(choice_labels.len() - 1);
        let menu = MenuList {
            title: None,
            items: &choice_labels,
            selected,
        };
        // Reserve a hint band at the bottom of the modal.
        let hint_h = 1.min(inner.height);
        let body_h = inner.height.saturating_sub(hint_h);
        let body = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h,
        };
        let hint = Rect {
            x: inner.x,
            y: inner.y + body_h,
            width: inner.width,
            height: hint_h,
        };
        render_menu_list(frame, body, &menu);
        if hint_h > 0 {
            frame.render_widget(
                Paragraph::new("[Up/Down] choose    [Enter] pick    [Esc] leave")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                hint,
            );
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Walk away from the conversation.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,

            // Cursor movement applies only while choices are showing;
            // outside that mode we treat it as inert (silent reject)
            // so a stray arrow keystroke doesn't accidentally advance
            // the line cursor.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected_choice > 0 {
                    self.selected_choice -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                let labels = self.current_choice_labels();
                if !labels.is_empty() && self.selected_choice + 1 < labels.len() {
                    self.selected_choice += 1;
                }
                ScreenCommand::None
            }

            Input::Enter => {
                if self.is_finished() {
                    return ScreenCommand::Pop;
                }
                let mut flags = self.flags.borrow_mut();
                if self.state.current_line(&self.dialog).is_some() {
                    // Pump the next line. `advance` only errors if
                    // already finished, which we just checked.
                    let _ = self.state.advance(&self.dialog, &mut flags);
                    return ScreenCommand::None;
                }
                // Choice mode. Use the live count to clamp the index;
                // an out-of-range pick returns NoChoices/OutOfRange
                // and we fall through to a no-op rather than crashing.
                let labels_len = self.state.available_choices(&self.dialog, &flags).len();
                if labels_len == 0 {
                    // Hub with all choices gated + no goto: try
                    // advancing to honour any fallback the validator
                    // permitted. If `advance` finishes the dialog,
                    // the next Enter will Pop.
                    let _ = self.state.advance(&self.dialog, &mut flags);
                    return ScreenCommand::None;
                }
                let idx = self.selected_choice.min(labels_len - 1);
                let _ = self.state.choose(&self.dialog, &mut flags, idx);
                // Reset cursor for the next node so a long previous
                // selection doesn't carry over to a short choice list.
                self.selected_choice = 0;
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the modal NPC dialog: branching gates, advancing
    //! through lines, choosing options, and the dialog↔map shared-flag
    //! contract that powers SPEC §13's win condition.

    use super::*;
    use crate::map::MapScreen;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{FlagSet, GameContext};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Construct a Night Clerk dialog screen with an empty flag store
    /// for tests that need to drive the conversation directly.
    fn fresh_clerk_dialog() -> (DialogScreen, Rc<RefCell<FlagSet>>) {
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let clerk = MapScreen::NPCS
            .iter()
            .find(|n| n.name == "Night Clerk")
            .expect("night clerk in roster");
        let screen = DialogScreen::new(clerk, Rc::clone(&flags));
        (screen, flags)
    }

    #[test]
    fn dialog_starts_on_speaker_and_first_line() {
        let (screen, _flags) = fresh_clerk_dialog();
        assert_eq!(screen.speaker(), "Night Clerk");
        assert!(!screen.is_finished());
        assert_eq!(screen.state().current_node(), "greeting");
    }

    #[test]
    fn dialog_clerk_branch_is_gated_until_rumor_flag_set() {
        // Branching contract: the "Got a master key" choice must NOT
        // appear before the player asks about the murder, and MUST
        // appear after the rumor flag has been set.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        // Advance through both greeting lines so choices are visible.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        let initial = screen.current_choice_labels();
        assert!(
            !initial.iter().any(|t| t.contains("master key")),
            "master-key choice should be gated initially; saw {initial:?}"
        );

        // Set the flag directly to keep the test focused on the gate
        // rather than re-driving the whole conversation; the choose()
        // path is exercised by the round-trip test below.
        flags.borrow_mut().insert("heard_rumor".into());
        let unlocked = screen.current_choice_labels();
        assert!(
            unlocked.iter().any(|t| t.contains("master key")),
            "master-key choice should unlock once `heard_rumor` is set; saw {unlocked:?}"
        );
    }

    #[test]
    fn dialog_clerk_choose_rumor_then_key_sets_flags() {
        // End-to-end: drive the conversation through the rumor →
        // key handover branch and assert both flags persist on the
        // shared store.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        // Pump greeting lines.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // Highlight "I heard about the murder." (index 1) and pick it.
        screen.handle_input(&mut ctx, Input::Down);
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "rumor");
        assert!(flags.borrow().contains("heard_rumor"));
        // Pump rumor lines.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // First choice on rumor is "What about that key?" — pick it.
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "key_handed_over");
        assert!(flags.borrow().contains("has_key"));
    }

    #[test]
    fn dialog_clerk_hub_loops_and_exhausts_topics() {
        // Regression coverage for the hub-loop refactor: the player
        // can take any branch, return to the hub, and pick another;
        // each choice disappears once it has been taken; once every
        // topic is exhausted only "Never mind." remains.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();

        // Walk past the greeting lines into the hub.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        let initial = screen.current_choice_labels();
        assert_eq!(
            initial,
            vec![
                "Just checking in.".to_string(),
                "I heard about the murder.".to_string(),
                "Never mind.".to_string(),
            ],
            "hub on first entry must show the two unflagged topics plus the exit"
        );

        // Pick "Just checking in." (index 0). Walk its single line
        // back to the hub.
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "checkin");
        screen.handle_input(&mut ctx, Input::Enter); // line + chase -> hub
        assert_eq!(screen.state().current_node(), "hub");
        let after_checkin = screen.current_choice_labels();
        assert_eq!(
            after_checkin,
            vec![
                "I heard about the murder.".to_string(),
                "Never mind.".to_string(),
            ],
            "checking in must hide the room topic"
        );

        // Pick "I heard about the murder." (index 0 now that the
        // checkin topic is gone). Walk its lines + pick "Thanks. I'll
        // be careful." (index 1) to bounce back to the hub without
        // taking the key from the rumor branch.
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "rumor");
        screen.handle_input(&mut ctx, Input::Enter); // rumor line 1
        screen.handle_input(&mut ctx, Input::Enter); // rumor line 2; choices visible
        screen.handle_input(&mut ctx, Input::Down); // index 0 -> 1
        screen.handle_input(&mut ctx, Input::Enter); // pick "Thanks..."
        assert_eq!(screen.state().current_node(), "hub");
        let after_rumor = screen.current_choice_labels();
        assert_eq!(
            after_rumor,
            vec![
                "Got a master key I can borrow?".to_string(),
                "Never mind.".to_string(),
            ],
            "the rumor topic must hide and the master key must unlock"
        );

        // Pick the master key (index 0). Walk its two lines back to
        // the hub; only "Never mind." remains.
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "key_handed_over");
        screen.handle_input(&mut ctx, Input::Enter); // key line 1
        screen.handle_input(&mut ctx, Input::Enter); // key line 2 + chase -> hub
        assert_eq!(screen.state().current_node(), "hub");
        let exhausted = screen.current_choice_labels();
        assert_eq!(
            exhausted,
            vec!["Never mind.".to_string()],
            "every topic taken must leave only the exit choice"
        );
    }

    #[test]
    fn dialog_esc_pops() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn dialog_q_quits() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Char('q')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn dialog_finished_then_enter_pops() {
        // Walk the linear Bellhop dialog to its terminal node and
        // confirm one more Enter pops the screen rather than
        // erroring.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let bellhop = MapScreen::NPCS
            .iter()
            .find(|n| n.name == "Bellhop")
            .expect("bellhop in roster");
        let mut screen = DialogScreen::new(bellhop, Rc::clone(&flags));
        // Two greeting lines, then advance past the last line into
        // the goto, then a final advance to land on the empty `end`
        // node and finish.
        for _ in 0..4 {
            screen.handle_input(&mut ctx, Input::Enter);
        }
        assert!(
            screen.is_finished(),
            "expected linear dialog to finish after 4 advances"
        );
        let cmd = screen.handle_input(&mut ctx, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn dialog_renders_into_test_backend() {
        // Headless render check: the speaker name appears in the
        // border and the first greeting line shows up in the body.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
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
            found.contains("Night Clerk"),
            "expected speaker name in dialog frame; buffer was:\n{found}"
        );
        assert!(
            found.contains("slouches"),
            "expected first greeting line in dialog body; buffer was:\n{found}"
        );
    }

    #[test]
    fn win_flag_matches_dialog_yaml() {
        // Renaming the flag in either place would silently un-gate the
        // win condition. Drive the Night Clerk's rumor branch and
        // confirm the flag the dialog sets is the same one the map
        // checks.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // Highlight "I heard about the murder." (index 1) and pick it.
        screen.handle_input(&mut ctx, Input::Down);
        screen.handle_input(&mut ctx, Input::Enter);
        assert!(
            flags.borrow().contains(MapScreen::WIN_FLAG),
            "Night Clerk rumor branch must set the WIN_FLAG; saw {:?}",
            flags.borrow()
        );
    }
}
