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

use std::cell::RefCell;
use std::rc::Rc;

use crate::dialog::{Dialog, DialogState, FlagSet};
use crate::screen::ScreenCommand;

// The `Screen` trait impl (render, handle_input) lands in Tasks 2c
// and 2d. Task 2b just stands up the struct and its constructor so
// downstream tasks have a concrete type to reach for; we intentionally
// avoid pulling in `Screen` / `GameContext` here because doing so
// before there is a `Screen for DialogScreen` impl would trip the
// `-D warnings` gate on unused imports.

/// Boxed callback type translating a [`DialogAction`] outcome into a
/// [`ScreenCommand`]. Aliased so the field type stays readable in
/// `Debug` impls and so the trait-object bound (`'static` +
/// [`FnMut`]) lives in one place. Mirrors
/// [`crate::prompt_screen`]'s `ActionCallback<T>`.
type ActionCallback = Box<dyn FnMut(DialogAction) -> ScreenCommand + 'static>;

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

/// `Screen` adapter wrapping a [`Dialog`] / [`DialogState`] /
/// [`FlagSet`] triple plus a callback that converts each
/// [`DialogAction`] into a [`ScreenCommand`] (SPEC_v2_1.md §4.2).
///
/// # Lifecycle (Tasks 2c–2d will fill these in)
///
/// 1. `render` will paint the speaker name (when present), the
///    current node body, and the filtered choice list using the
///    configured [`DialogLayout`]. Modal layout draws a centred
///    bordered block; compact draws the same content unboxed.
/// 2. `handle_input` will route navigation keys (`Up`/`Down`) into a
///    cursor over the available-choices list, and `Enter` into a
///    [`crate::dialog::dialog_handle_prompt_input`] call that
///    advances [`DialogState`] and applies branch flags. The
///    resulting [`DialogAction`] is passed to `on_action`.
/// 3. `tick` and `on_resize` fall back to the [`crate::screen::Screen`]
///    defaults — dialogs have no animation and resize is covered by
///    the runtime re-rendering against the new size.
///
/// # Why `flags` is `Rc<RefCell<FlagSet>>`
///
/// The flag store is owned by the game-side state (`SharedSlots` in
/// the Murder Motel example), and multiple screens have to read and
/// mutate it across frames — a quest-flag set during a Lost-and-Found
/// dialog has to be visible to the next door scene. A shared
/// `Rc<RefCell<FlagSet>>` is the same shape every existing
/// hand-rolled dialog scene already uses; the adapter takes the same
/// shape so authors can hand it the slot they already have rather
/// than restructure their state.
///
/// # Why `on_action` does not see `GameContext`
///
/// Same rationale as [`crate::prompt_screen::PromptScreen`]: the
/// callback fires *after* `handle_input` has already classified the
/// outcome, so by construction it cannot peek at the per-frame
/// runtime context. Authors who need that level of access should
/// drop one layer down to
/// [`crate::dialog::dialog_handle_prompt_input`] inside a custom
/// [`crate::screen::Screen`] impl.
// Fields are read by Tasks 2c (render), 2d (input), and 2e
// (accessors). Task 2b lands the storage shape only; the
// `dead_code` lint correctly flags that there is not yet a non-test
// reader, but adding the trait impls here would balloon the commit
// past the one-task budget. The `allow` is removed when 2c–2e land.
#[allow(dead_code)]
pub struct DialogScreen {
    /// The parsed, validated dialog graph the screen is walking.
    /// Owned (not borrowed) so a `DialogScreen` can outlive whatever
    /// loaded the YAML — the typical Murder Motel pattern is
    /// `load_dialog(asset_yaml).map(|d| DialogScreen::new(d, ...))`,
    /// after which the asset string is dropped.
    dialog: Dialog,
    /// The mutable cursor through `dialog`. Constructed by the
    /// caller via [`DialogState::start`] so the first node's
    /// entry-set flags are applied before the screen ever renders.
    state: DialogState,
    /// Shared flag store. The adapter only mutates it inside the
    /// branch-transition path described by the loaded `Dialog`
    /// (SPEC_v2_1.md §4.2 forbids side-channel writes); other
    /// screens are free to read or mutate concurrently between
    /// frames.
    flags: Rc<RefCell<FlagSet>>,
    /// Index into [`DialogState::available_choices`] highlighted on
    /// the current frame. Reset to `0` on construction; Task 2d
    /// updates it on `Up`/`Down` navigation. Stored as `usize`
    /// rather than `Option<usize>` because the cursor always points
    /// at a real choice once one exists — when the choice list is
    /// empty (line-pumping phase or finished dialog) the field is
    /// simply ignored by the render path.
    choice_cursor: usize,
    /// Render shape — modal frame (default) or compact unboxed.
    layout: DialogLayout,
    /// Game-supplied callback translating each [`DialogAction`] into
    /// a [`ScreenCommand`]. Boxed `FnMut` matches
    /// [`crate::prompt_screen::PromptScreen`] so authors can close
    /// over their own `Rc<RefCell<_>>` state.
    on_action: ActionCallback,
}

impl DialogScreen {
    /// Build a new `DialogScreen` from a [`Dialog`], its starting
    /// [`DialogState`], a shared [`FlagSet`] handle, and a callback
    /// mapping [`DialogAction`] outcomes to [`ScreenCommand`] values
    /// (SPEC_v2_1.md §4.2 required API).
    ///
    /// Defaults to [`DialogLayout::Modal`]; the `compact` builder
    /// method (Task 2e) switches to the unboxed layout. The
    /// starting [`DialogState`] is supplied by the
    /// caller rather than constructed internally because
    /// [`DialogState::start`] requires `&mut FlagSet` access — the
    /// same flag store the caller already owns — and reaching into
    /// the `Rc<RefCell<_>>` from the constructor would either
    /// duplicate that borrow or introduce a hidden re-entrancy gate.
    /// Letting the caller build the state first keeps both concerns
    /// explicit at the call site.
    ///
    /// The callback is stored as a boxed `FnMut`, so it can mutate
    /// captured state across calls — the typical Murder Motel shape
    /// is closing over a shared feedback slot to set a one-line
    /// message before returning [`ScreenCommand::Pop`].
    pub fn new<F>(
        dialog: Dialog,
        start: DialogState,
        flags: Rc<RefCell<FlagSet>>,
        on_action: F,
    ) -> Self
    where
        F: FnMut(DialogAction) -> ScreenCommand + 'static,
    {
        Self {
            dialog,
            state: start,
            flags,
            choice_cursor: 0,
            layout: DialogLayout::Modal,
            on_action: Box::new(on_action),
        }
    }

    /// Inspect the configured [`DialogLayout`].
    ///
    /// Exposed now (rather than waiting for Task 2e's full accessor
    /// suite) because Task 2b's tests need to pin that the
    /// constructor defaults to [`DialogLayout::Modal`] — the
    /// authoring rule of thumb that motivates the whole adapter.
    /// The remaining accessors (`dialog`, `state`, `modal`,
    /// `compact`) land in Task 2e alongside the `Screen` impl from
    /// Tasks 2c/2d.
    pub fn layout(&self) -> DialogLayout {
        self.layout
    }
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

    use crate::dialog::{load_dialog, DialogState, FlagSet};
    use crate::screen::ScreenCommand;

    /// Smallest possible dialog graph that both validates and is
    /// safe to drive through `DialogState::start`. Only one terminal
    /// node, no choices, no flag predicates — the constructor tests
    /// only need a well-formed [`Dialog`] to hand to `new`, not the
    /// full Murder Motel scene shape.
    fn fixture_dialog_yaml() -> &'static str {
        r#"
start: greeting
nodes:
  greeting:
    lines: ["Hello, traveller."]
    goto: end
  end: {}
"#
    }

    #[test]
    fn new_stores_dialog_state_flags_and_callback() {
        // Build the same `(dialog, state, flags)` triple a real
        // game would: load YAML, create `DialogState::start` against
        // a fresh flag store, then hand both into the adapter
        // alongside a callback that records every action it sees.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();

        let screen = DialogScreen::new(dialog.clone(), start.clone(), flags.clone(), move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::None
        });

        // Storage check — the constructor must hand the dialog,
        // state, and flag handle through unchanged. We compare via
        // the private fields (the test module sees them) so the
        // assertion does not depend on Task 2e's `dialog()` /
        // `state()` accessors landing first.
        assert_eq!(screen.dialog, dialog);
        assert_eq!(screen.state, start);
        assert!(Rc::ptr_eq(&screen.flags, &flags));
        // Cursor starts at zero — Task 2d will move it on Up/Down.
        assert_eq!(screen.choice_cursor, 0);
        // The callback must not have fired yet — the constructor
        // is meant to be inert with respect to the action stream.
        assert!(recorded.borrow().is_empty());
    }

    #[test]
    fn new_defaults_to_modal_layout() {
        // SPEC_v2_1.md §4.2 picks `Modal` as the default because
        // every Murder Motel dialog scene already renders that way;
        // pin that here so a future contributor switching the
        // default has to revisit the spec.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None);
        assert_eq!(screen.layout(), DialogLayout::Modal);
    }

    #[test]
    fn new_shares_flag_handle_with_caller() {
        // Pin that the adapter holds the *same* `Rc` the caller
        // passed in, not a clone of the inner `FlagSet`. The Murder
        // Motel state shape relies on this: outer screens mutate the
        // flag store between frames and expect the adapter to see
        // the new flags on the next render.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let screen = DialogScreen::new(dialog, start, flags.clone(), |_| ScreenCommand::None);

        // Mutate via the *caller's* handle and observe the change
        // through the screen's stored handle — the only way both
        // borrows can see the same insertion is if they point at the
        // same `RefCell`.
        flags.borrow_mut().insert("heard_rumor".to_string());
        assert!(screen.flags.borrow().contains("heard_rumor"));
    }
}
