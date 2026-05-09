//! Modal NPC dialog scene — a thin wrapper around
//! [`foglet_game::DialogScreen`].
//!
//! The kit adapter (Tasks 2a–2g) handles line pumping, choice
//! navigation, branch-flag application, and `>9`-choice scrolling.
//! This wrapper only adds what is genuinely game-specific:
//!
//! 1. **Speaker name** painted into the modal's top border row — the
//!    kit's [`Dialog`] schema does not carry one, so we overlay it
//!    after the kit has drawn the (untitled) bordered block.
//! 2. **Quit affordances** — `Q`/`Ctrl-C` hard-quit and `Backspace`
//!    pops, none of which the kit routes itself (it stays out of the
//!    global hotkey conversation so each game can pick its own).
//! 3. **`j`/`k` aliases** for `Down`/`Up`, kept for v2 muscle memory.
//! 4. **`current_choice_labels()`** — read-only helper used by tests
//!    to assert visible labels without owning a `Frame`.

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

/// Modal dialog screen pushed when the player talks to an NPC. All
/// dialog mechanics live in [`kit::DialogScreen`]; this wrapper exists
/// for the speaker title overlay, Q/Ctrl-C/Backspace quitting,
/// `j`/`k` aliases, and a tiny read accessor used by tests.
pub struct DialogScreen {
    /// Speaker label rendered into the top border of the modal frame.
    speaker: &'static str,
    /// Shared narrative-flag store, cloned from the kit screen so
    /// `current_choice_labels()` can re-derive the visible label list.
    flags: Rc<RefCell<FlagSet>>,
    /// The kit-side dialog adapter doing the actual work.
    inner: kit::DialogScreen,
}

impl DialogScreen {
    /// Build a dialog screen for the given NPC, sharing the supplied
    /// flag store. The dialog YAML is parsed eagerly so any schema
    /// error surfaces here rather than on first render. `expect` is
    /// fine: the YAML ships in the binary (`include_str!`); a parse
    /// failure is a build-time bug, not runtime input.
    pub fn new(npc: &Npc, flags: Rc<RefCell<FlagSet>>) -> Self {
        let dialog = load_dialog(npc.dialog_yaml).expect("embedded NPC dialog parses");
        let start = {
            let mut fs = flags.borrow_mut();
            DialogState::start(&dialog, &mut fs)
        };
        // Map kit actions onto v2 observable behaviour: a picked
        // choice keeps the dialog open; Finished/Cancelled pop.
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

    /// Whether the dialog cursor reports finished.
    pub fn is_finished(&self) -> bool {
        self.inner.state().is_finished()
    }

    /// Borrow the underlying dialog state. Mostly for tests; gameplay
    /// code routes through `handle_input`.
    pub fn state(&self) -> &DialogState {
        self.inner.state()
    }

    /// Snapshot of the choice labels currently presented. Re-derives
    /// from the kit screen's `state()` / `dialog()` plus this wrapper's
    /// flag clone, so the result matches the next frame's render.
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
        // renders into a flat buffer so the later write wins.
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
            // Quit/back affordances; the kit leaves these to the host.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            Input::Backspace => self.inner.handle_input(ctx, Input::Esc),
            // Vi aliases — kit only knows Up/Down.
            Input::Char('k') | Input::Char('K') => self.inner.handle_input(ctx, Input::Up),
            Input::Char('j') | Input::Char('J') => self.inner.handle_input(ctx, Input::Down),
            other => self.inner.handle_input(ctx, other),
        }
    }
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
