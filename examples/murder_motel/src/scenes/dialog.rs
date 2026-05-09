//! Modal NPC dialog scene — a thin wrapper around
//! [`foglet_game::DialogScreen`].
//!
//! v2 hand-rolled the entire `Screen` impl here: a centred 60×10 modal
//! frame, line-pumping vs choice-mode body switching, an `Up`/`Down`
//! cursor over `available_choices`, and a one-key Esc/Q quit affordance.
//! v2.1 ships those mechanics in [`foglet_game::DialogScreen`] (Tasks
//! 2a–2g), so the example only needs to add what is genuinely
//! game-specific:
//!
//! 1. The **speaker name** painted into the modal's top border row —
//!    the kit's [`Dialog`] schema does not carry one, so the wrapper
//!    overlays it after the kit has drawn the (untitled) bordered
//!    block.
//! 2. **Quit affordances** — `Q`/`Ctrl-C` hard-quit and `Backspace`
//!    pops, neither of which the kit adapter routes itself. The kit
//!    deliberately stays out of the global hotkey conversation so each
//!    example can pick its own (a help screen, a save scene, …); the
//!    wrapper restores the v2 muscle memory.
//! 3. **`j`/`k` aliases** for `Down`/`Up`, kept because the v2 example
//!    advertised them in its hint band.
//! 4. **`current_choice_labels()`** — a tiny read-only helper used by
//!    the gating tests in this module. It re-derives the visible label
//!    list from the kit's `state()` / `dialog()` plus a private clone
//!    of the shared `FlagSet` handle.

use std::cell::RefCell;
use std::rc::Rc;

use foglet_game::{
    self as kit, load_dialog, DialogAction, DialogState, FlagSet, GameContext, Input, Screen,
    ScreenCommand,
};
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::map::Npc;

/// Modal dialog screen pushed when the player talks to an NPC.
///
/// All dialog mechanics — line pumping, choice navigation, branch-flag
/// application, `>9` choice scrolling — live in
/// [`kit::DialogScreen`]. This wrapper exists for the three things the
/// kit deliberately does not do (speaker title overlay, Q/Ctrl-C
/// quitting, `j`/`k` aliases) plus a tiny read accessor used by the
/// existing tests.
pub struct DialogScreen {
    /// Speaker label rendered into the top border of the modal frame.
    /// Stored as `&'static str` because [`Npc`] is `Copy` and lives in
    /// [`crate::map::MapScreen::NPCS`].
    speaker: &'static str,
    /// Shared narrative-flag store, cloned from the kit screen so the
    /// `current_choice_labels()` helper can re-derive the visible label
    /// list without poking at kit-internal state.
    flags: Rc<RefCell<FlagSet>>,
    /// The kit-side dialog adapter doing the actual work.
    inner: kit::DialogScreen,
}

impl DialogScreen {
    /// Build a dialog screen for the given NPC, sharing the supplied
    /// flag store. The dialog YAML is parsed eagerly here so any schema
    /// error surfaces at the moment the player presses Enter rather
    /// than on the first render. `expect` is acceptable because the
    /// YAML ships in the binary (`include_str!`); a parse failure is a
    /// build-time bug, not a runtime input.
    pub fn new(npc: &Npc, flags: Rc<RefCell<FlagSet>>) -> Self {
        let dialog = load_dialog(npc.dialog_yaml).expect("embedded NPC dialog parses");
        let start = {
            let mut fs = flags.borrow_mut();
            DialogState::start(&dialog, &mut fs)
        };
        // Map every kit-emitted action onto the v2 observable
        // behaviour: a picked choice keeps the dialog open (None);
        // Finished and Cancelled both walk the player back to the map.
        let inner =
            kit::DialogScreen::new(dialog, start, Rc::clone(&flags), |action| match action {
                DialogAction::ChoicePicked { .. } => ScreenCommand::None,
                DialogAction::Finished | DialogAction::Cancelled => ScreenCommand::Pop,
            });
        Self {
            speaker: npc.name,
            flags,
            inner,
        }
    }

    /// Speaker name — exposed for tests asserting which NPC is on
    /// screen without poking at private fields.
    pub fn speaker(&self) -> &str {
        self.speaker
    }

    /// Whether the dialog cursor reports finished. Read-only: tests
    /// assert end-state without driving the screen through input.
    pub fn is_finished(&self) -> bool {
        self.inner.state().is_finished()
    }

    /// Borrow the underlying dialog state. Mostly for tests; gameplay
    /// code routes through `handle_input`.
    pub fn state(&self) -> &DialogState {
        self.inner.state()
    }

    /// Snapshot of the choice labels currently presented. Useful for
    /// tests that want to assert "the unlocked branch appears after
    /// the rumor flag is set" without owning a `Frame`. Re-derives
    /// from the kit screen's `state()` / `dialog()` accessors plus the
    /// wrapper's flag clone, so the result always matches what
    /// `kit::DialogScreen::render` would draw on the next frame.
    pub fn current_choice_labels(&self) -> Vec<String> {
        let flags = self.flags.borrow();
        self.inner
            .state()
            .available_choices(self.inner.dialog(), &flags)
            .into_iter()
            .map(|c| c.text.clone())
            .collect()
    }
}

impl Screen for DialogScreen {
    fn render(&mut self, ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Let the kit paint its bordered modal + body first, then
        // overlay the speaker name into the top border row. ratatui
        // renders into a flat buffer so the later write wins; the
        // border `─` glyphs the kit drew get replaced by the title
        // text in the overlap range only.
        self.inner.render(ctx, frame);
        let area = frame.area();
        if area.height == 0 || area.width <= 4 {
            return;
        }
        let label = format!(" {} ", self.speaker);
        let label_w = (label.chars().count() as u16).min(area.width.saturating_sub(2));
        let title_area = Rect {
            x: area.x + 2,
            y: area.y,
            width: label_w,
            height: 1,
        };
        frame.render_widget(Paragraph::new(label), title_area);
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on quit affordances. The kit adapter intentionally
            // leaves these to the host so each game can pick its own
            // global keys; v2 used Q / Ctrl-C and we keep that here.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Backspace as an alias for Esc — same v2 behaviour.
            Input::Backspace => self.inner.handle_input(ctx, Input::Esc),
            // Vi-style aliases. The kit only knows Up/Down; translate
            // before delegating so author-side hint text ("[Up/Down]
            // choose") and player muscle memory (`j`/`k`) both work.
            Input::Char('k') | Input::Char('K') => self.inner.handle_input(ctx, Input::Up),
            Input::Char('j') | Input::Char('J') => self.inner.handle_input(ctx, Input::Down),
            other => self.inner.handle_input(ctx, other),
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
        // border (overlay) and the first greeting line shows up in
        // the body (kit render).
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
