//! `DialogScreen` — optional [`crate::screen::Screen`] adapter that
//! drives a [`crate::dialog::Dialog`] / [`crate::dialog::DialogState`]
//! pair through the standard render and input loop, parallel to
//! [`crate::prompt_screen::PromptScreen`] (SPEC_v2_1.md §4.2).
//!
//! # When to reach for this
//!
//! Use `DialogScreen` for any screen whose entire job is "show an NPC
//! conversation, let the player navigate `requires`-gated choices, and
//! pop back when the dialog finishes." That is the shape of every
//! talk-to-an-NPC scene in the Murder Motel example, and it has been
//! the shape of the example's hand-rolled `scenes/dialog.rs` ever
//! since v1 — ~562 lines of generic plumbing that never needed to be
//! per-game.
//!
//! Compose [`crate::dialog::Dialog`] / [`crate::dialog::DialogState`]
//! manually inside a custom [`crate::screen::Screen`] when the dialog
//! is one panel among several (e.g. an NPC
//! talking head next to a live inventory pane), or when the screen
//! owns mutable state the dialog has to consult across frames (e.g. a
//! shopkeeper whose available branches depend on live cash). The
//! adapter's `on_action` callback intentionally does *not* receive the
//! per-frame [`crate::screen::GameContext`] — it runs *after*
//! `handle_input`, so games that need to peek at runtime data while
//! choosing their [`crate::screen::ScreenCommand`] should drop one
//! layer down to [`crate::dialog::dialog_handle_prompt_input`]
//! directly.
//!
//! See `docs/dialog-screens.md` (Task 9b) for the full authoring rule
//! of thumb on choosing between this adapter and a hand-rolled
//! [`crate::screen::Screen`] that composes [`crate::dialog::Dialog`]
//! manually.
//!
//! # Why a callback, not a return-value-only design
//!
//! [`crate::screen::ScreenCommand`] is the SPEC §5.5 vocabulary the
//! runtime understands; [`DialogAction`] is this adapter's narrower
//! vocabulary. The mapping between them is game-specific (one game's
//! `Finished` becomes `Pop`; another's becomes `Replace` into a new
//! cutscene), so the adapter must defer to the author. A boxed
//! `FnMut` keeps the type concrete (no extra generic parameter on
//! `DialogScreen`) and lets games close over their own state — the
//! most common case is a captured `Rc<RefCell<_>>` holding a quest
//! log or feedback line that the callback updates before returning
//! [`crate::screen::ScreenCommand::Pop`]. This mirrors the deliberate
//! design choice made for [`crate::prompt_screen::PromptScreen`].
//!
//! # Layout default differs from `PromptScreen`
//!
//! Unlike [`crate::prompt_screen::PromptScreen`], which defaults to
//! the SPEC §4.3 compact layout, `DialogScreen` defaults to
//! [`DialogLayout::Modal`]. Real Foglet door-game dialogs are almost
//! always centred, bordered, and titled with the speaker's name —
//! that is what every Murder Motel NPC scene already does, and the
//! default should reflect the common case. Authors who want the
//! compact unboxed layout will be able to opt in with the
//! `DialogScreen::compact` builder method (Task 2e).

// `Screen`, `GameContext`, and `ScreenCommand` from `crate::screen`
// will be imported in Task 2b alongside the `DialogScreen` struct and
// its `Screen` impl. Task 2a keeps this module to types only so the
// `-D warnings` gate stays green without `#[allow(unused_imports)]`
// shims that we'd just have to remove a commit later.

/// Layout mode for `DialogScreen` rendering (Task 2b lands the
/// struct itself).
///
/// Mirrors [`crate::prompt_screen::PromptLayout`]: a bordered modal
/// frame (the dialog default) versus an unboxed compact layout. The
/// adapter keeps both available rather than picking one because real
/// games occasionally want an inline dialog (e.g. a barker shouting
/// across a room) instead of the canonical centred conversation
/// modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogLayout {
    /// Bordered modal layout: a [`ratatui::widgets::Block`] frame with
    /// the speaker name in the top border and the body + choices
    /// rendered into the centred inner rect. This is the default for
    /// `DialogScreen` because every Murder Motel dialog scene uses
    /// this shape and it is what most door-game NPC conversations
    /// expect.
    Modal,
    /// Compact unboxed layout: no border, body and choices flow
    /// top-down inside the frame area. Useful for inline dialogs that
    /// share the screen with another widget (e.g. a map plus an
    /// inline NPC line) or for very short barker-style one-liners
    /// where a modal frame would feel heavy.
    Compact,
}

/// Outcome of a single `DialogScreen::handle_input` call (Task 2d),
/// surfaced to the author's callback.
///
/// `DialogAction` exists so the adapter does not have to express
/// every possible game-side reaction inline. `ChoicePicked` is the
/// hot path — every Enter on a non-terminal node — and carries enough
/// information for the author to decide whether to log, animate,
/// stash a quest flag, or simply ignore. `Finished` fires when the
/// player advances past a terminal node, and `Cancelled` fires when
/// the player presses [`crate::input::Input::Esc`] on a cancellable
/// dialog.
///
/// The variants intentionally cover the full surface of "what just
/// happened in this dialog frame" so a callback that wants to map all
/// three to [`crate::screen::ScreenCommand::Pop`] (the typical Murder
/// Motel shape) can do so with a single `match` arm fall-through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    /// The player picked a choice on the current node. `choice_index`
    /// is the index into the `flags`-filtered choice list returned by
    /// [`crate::dialog::DialogState::available_choices`] *for the
    /// frame the input was processed in* — callbacks that re-query
    /// the dialog with a different `FlagSet` snapshot must not assume
    /// the index still resolves to the same choice. `target_node` is
    /// the [`crate::dialog::Choice::goto`] field of the picked
    /// choice, captured before the state advances so the callback can
    /// use it for analytics, logging, or branch-specific UI without
    /// re-deriving it from the post-advance [`crate::dialog::DialogState`].
    ChoicePicked {
        /// Position of the picked choice in the filtered choice list
        /// the player saw. Zero-based.
        choice_index: usize,
        /// Node id the dialog will advance to as a result of this
        /// choice. May be a terminal node (in which case the next
        /// frame will fire [`DialogAction::Finished`]).
        target_node: String,
    },
    /// The dialog has reached and advanced past a terminal node. No
    /// further choices are available; the typical reaction is
    /// [`crate::screen::ScreenCommand::Pop`].
    Finished,
    /// The player pressed [`crate::input::Input::Esc`] on a
    /// cancellable dialog. Callbacks that want to require seeing the
    /// dialog through to a terminal node should ignore this and
    /// return [`crate::screen::ScreenCommand::None`]; callbacks that
    /// want to model "press Esc to walk away" should map it to
    /// [`crate::screen::ScreenCommand::Pop`].
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construction sanity check for [`DialogLayout`]. The variants
    /// are `Copy` so we can assert equality without cloning, and the
    /// `PartialEq` impl is what downstream tests will lean on when
    /// pinning the default layout in Task 2b.
    #[test]
    fn dialog_layout_variants_are_distinct() {
        assert_ne!(DialogLayout::Modal, DialogLayout::Compact);
    }

    /// Round-trips a `ChoicePicked` through `Clone` to pin that
    /// callbacks may stash the action for later inspection (the
    /// captured-`Rc<RefCell<Vec<_>>>` pattern from
    /// [`crate::prompt_screen`]'s tests). `target_node` is `String`
    /// so this clone is a real heap allocation, not a `Copy`, which
    /// is the contract Task 2b's tests will rely on.
    #[test]
    fn dialog_action_choice_picked_clones() {
        let action = DialogAction::ChoicePicked {
            choice_index: 2,
            target_node: "lobby_outro".to_string(),
        };
        let cloned = action.clone();
        assert_eq!(action, cloned);
        match cloned {
            DialogAction::ChoicePicked {
                choice_index,
                target_node,
            } => {
                assert_eq!(choice_index, 2);
                assert_eq!(target_node, "lobby_outro");
            }
            _ => panic!("expected ChoicePicked"),
        }
    }

    /// Pin that `Finished` and `Cancelled` are distinct from each
    /// other and from `ChoicePicked`. Future contributors who want to
    /// merge variants (a recurring temptation: "couldn't `Cancelled`
    /// just be `Finished` with a flag?") will trip this test and
    /// have to revisit SPEC_v2_1.md §4.2 first.
    #[test]
    fn dialog_action_terminal_variants_are_distinct() {
        assert_ne!(DialogAction::Finished, DialogAction::Cancelled);
        assert_ne!(
            DialogAction::Finished,
            DialogAction::ChoicePicked {
                choice_index: 0,
                target_node: "n".into(),
            }
        );
    }
}
