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

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::dialog::{
    dialog_choice_prompt_window, dialog_handle_prompt_input_window, Dialog, DialogState, FlagSet,
    DIALOG_PROMPT_MAX_CHOICES,
};
use crate::input::Input;
use crate::prompt::PromptAction;
use crate::screen::{GameContext, Screen, ScreenCommand};

// Task 2e (accessors) will pull a couple more items in alongside its
// own commit; this module's import set otherwise stabilises here.

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
    /// Top of the visible-choices window when the current node has
    /// more than [`DIALOG_PROMPT_MAX_CHOICES`] available branches
    /// (SPEC_v2_1.md §4.2 Task 2f). `0` for any node within the cap,
    /// which is the common case — only wide branching nodes (12+
    /// suspects, long item lists threaded through dialog) ever set
    /// this above zero. Maintained as the invariant
    /// `choice_scroll <= choice_cursor < choice_scroll + DIALOG_PROMPT_MAX_CHOICES`
    /// so the cursor always points into the visible window.
    choice_scroll: usize,
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
            choice_scroll: 0,
            layout: DialogLayout::Modal,
            on_action: Box::new(on_action),
        }
    }

    /// Inspect the configured [`DialogLayout`].
    ///
    /// Mirrors [`crate::prompt_screen::PromptScreen::layout`]. Useful
    /// for tests that want to pin the constructor default
    /// ([`DialogLayout::Modal`], SPEC_v2_1.md §4.2) and for screens that
    /// compose a [`DialogScreen`] inside a larger layout and need to
    /// know whether the adapter is drawing its own border.
    pub fn layout(&self) -> DialogLayout {
        self.layout
    }

    /// Borrow the underlying [`Dialog`] — the parsed graph the screen
    /// is walking. Useful for tests and for outer screens that want to
    /// peek at speaker metadata or the start node id without keeping a
    /// second handle to the loaded asset alongside the
    /// [`DialogScreen`].
    ///
    /// Mirrors [`crate::prompt_screen::PromptScreen::prompt`]. Returned
    /// as `&Dialog` (not `&mut`) because mutating the graph mid-walk
    /// would invalidate the [`DialogState`] cursor — authors who want
    /// to swap dialogs should rebuild the screen instead.
    pub fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// Borrow the live [`DialogState`] cursor.
    ///
    /// The state is the moving part of the adapter — every `Enter` in
    /// line-pumping mode and every `ChoicePicked` in choice mode
    /// advances it. Tests use this to assert the cursor landed on the
    /// expected node after an input; outer screens occasionally use it
    /// to render speaker metadata derived from the current node.
    /// Returned as `&DialogState` rather than `&mut` because driving
    /// the state directly would bypass the [`DialogScreen`]'s flag
    /// borrow discipline — drive it through `handle_input` instead.
    pub fn state(&self) -> &DialogState {
        &self.state
    }

    /// Switch the layout to [`DialogLayout::Modal`] (SPEC_v2_1.md §4.2
    /// default). Builder-style so the call site reads
    /// `DialogScreen::new(...).modal()` even though it is the default —
    /// useful when an author wants to spell the layout choice out
    /// explicitly for readers (or to undo a prior `.compact()` in a
    /// builder chain).
    pub fn modal(mut self) -> Self {
        self.layout = DialogLayout::Modal;
        self
    }

    /// Switch the layout to [`DialogLayout::Compact`] — no border, body
    /// and choices flow inside the supplied frame area. Use for inline
    /// dialogs that share the screen with another widget (e.g. a map
    /// pane plus an NPC line) or for short barker-style one-liners
    /// where a modal frame would feel heavy.
    ///
    /// Mirrors [`crate::prompt_screen::PromptScreen::compact`]; the two
    /// adapters intentionally use the same builder-method names so
    /// authors can switch between them with the same muscle memory.
    pub fn compact(mut self) -> Self {
        self.layout = DialogLayout::Compact;
        self
    }
}

impl Screen for DialogScreen {
    /// Paint the dialog into `frame`, picking the body shape from the
    /// current [`DialogState`] cursor (line-pumping, choice mode, or
    /// finished) and the framing from [`Self::layout`] (Task 2c).
    ///
    /// # Layout
    ///
    /// `DialogLayout::Modal` draws a single bordered [`Block`] over
    /// the entire frame area and renders the body into the inner rect;
    /// `DialogLayout::Compact` skips the block and renders directly
    /// into the frame area. The dialog's title (speaker name) is
    /// **not** rendered here because the kit's [`Dialog`] schema does
    /// not carry one — the modal frame is left untitled and games that
    /// want a speaker label will compose `crate::widgets::render_modal`
    /// (Task 3b — not yet landed) themselves before pushing the
    /// screen, or wrap a
    /// [`DialogScreen`] in a custom [`Screen`] that paints the title
    /// row first. The Murder Motel refactor (Task 6) will spell that
    /// pattern out concretely.
    ///
    /// # Body modes
    ///
    /// 1. **Line-pumping.** [`DialogState::current_line`] returns
    ///    `Some(line)` while the cursor sits on a script line. Render
    ///    the line text wrapped to the body area and a centred
    ///    `[Enter] continue   [Esc] leave` hint along the bottom row.
    /// 2. **Finished.** [`DialogState::is_finished`] is `true` after
    ///    the cursor has walked off the end of the graph. Render a
    ///    `(They turn away.)` leave hint — the dialog has nothing more
    ///    to say. Task 2d will translate the next `Enter`/`Esc` into a
    ///    [`DialogAction::Finished`] / [`DialogAction::Cancelled`]
    ///    callback.
    /// 3. **Choice mode.** Lines exhausted, dialog not yet finished.
    ///    Build a [`crate::prompt::ChoicePrompt`] from the live flag
    ///    snapshot via [`crate::dialog::dialog_choice_prompt`] (the SPEC §8 helper
    ///    that already filters `requires` / `requires_not` predicates
    ///    and caps the choice list at [`crate::dialog::DIALOG_PROMPT_MAX_CHOICES`])
    ///    and delegate rendering to [`crate::prompt::ChoicePrompt::render`].
    ///    The screen's `choice_cursor` is fed in via the prompt's
    ///    public `selected` field, clamped against the live choice
    ///    count so a cursor stranded by a flag change cannot point off
    ///    the end of the list.
    ///
    /// # Why delegate to `dialog_choice_prompt` rather than re-render
    ///
    /// SPEC_v2_1.md §4.2 explicitly forbids re-implementing dialog
    /// mechanics here: "The adapter MUST use `dialog_choice_prompt`
    /// and `dialog_handle_prompt_input` internally rather than
    /// re-implementing dialog mechanics." Routing through the helper
    /// also means the visible-choice cap and numeric-hotkey assignment
    /// stay in one place — adapter and helper cannot drift.
    ///
    /// # Why borrow `flags` short-lived
    ///
    /// The shared [`FlagSet`] is `Rc<RefCell<_>>`, so the borrow has
    /// to be released before any subsequent code path could grab a
    /// `&mut FlagSet`. Render keeps the borrow inside the choice
    /// branch and drops it at the end of the function, so a future
    /// `Screen` method that wanted a `&mut` borrow during the same
    /// frame (none today) would not deadlock.
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let outer = frame.area();
        let inner = match self.layout {
            DialogLayout::Modal => {
                let block = Block::default().borders(Borders::ALL);
                let inside = block.inner(outer);
                frame.render_widget(block, outer);
                inside
            }
            DialogLayout::Compact => outer,
        };
        // A zero-sized inner rect happens on tiny terminals (or when
        // a parent layout has clipped the frame to one cell). Bail
        // rather than asking ratatui to render into nothing — the
        // helpers below tolerate it, but the early return saves the
        // borrow on `flags` and keeps the render trace shallow.
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // 1. Line-pumping mode: cursor sits on a script line.
        if let Some(line) = self.state.current_line(&self.dialog) {
            let line_text = line.to_string();
            let (body_area, hint_area) = split_body_and_hint(inner);
            frame.render_widget(
                Paragraph::new(line_text)
                    .alignment(Alignment::Left)
                    .wrap(Wrap { trim: false }),
                body_area,
            );
            if let Some(hint_area) = hint_area {
                frame.render_widget(
                    Paragraph::new("[Enter] continue   [Esc] leave")
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(Color::DarkGray)),
                    hint_area,
                );
            }
            return;
        }

        // 2. Finished: nothing left to say.
        if self.state.is_finished() {
            frame.render_widget(
                Paragraph::new("(They turn away.)\n\n[Enter / Esc] leave")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        // 3. Choice mode. Build the prompt under an immutable flag
        // borrow; mutate nothing — render is read-only with respect
        // to game state per SPEC §7. We use the *windowed* helper
        // here so a node with more than `DIALOG_PROMPT_MAX_CHOICES`
        // available branches scrolls deterministically (Task 2f) —
        // the un-windowed helper is the offset-0 case.
        let flags = self.flags.borrow();
        let total = self.state.available_choices(&self.dialog, &flags).len();
        if total == 0 {
            // Defensive: every choice gated and no goto fallback.
            // Render a leave hint so the player isn't stuck looking at
            // a blank modal — the validator allows this shape and the
            // example covered it before the refactor.
            drop(flags);
            frame.render_widget(
                Paragraph::new("(There's nothing more to say.)\n\n[Esc] leave")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }
        // Recompute the scroll window from the live `total` so a
        // mid-frame flag flip (which can shrink the list and strand
        // the saved offset past the end) cannot leave the prompt
        // pointing at no rows. `cursor` is then clamped into the
        // window, and `prompt.selected` is the *window-relative*
        // position because the prompt only knows about its own rows.
        let scroll = self.visible_window_offset(total);
        let mut prompt = dialog_choice_prompt_window(&self.state, &self.dialog, &flags, scroll);
        let visible = prompt.choices.len();
        // Defensive: a stale `choice_cursor` past the live `total`
        // (e.g. a flag flip between frames hid the cursor's choice)
        // is clamped to the last visible row. Rendering would
        // otherwise hand the prompt an out-of-range `selected` index.
        let clamped_cursor = self.choice_cursor.min(total - 1);
        let window_relative = clamped_cursor.saturating_sub(scroll).min(visible - 1);
        prompt.selected = Some(window_relative);
        let (body_area, hint_area) = split_body_and_hint(inner);
        let buf = frame.buffer_mut();
        prompt.render(body_area, buf);
        if let Some(hint_area) = hint_area {
            frame.render_widget(
                Paragraph::new("[Up/Down] choose    [Enter] pick    [Esc] leave")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                hint_area,
            );
        }
    }

    /// Drive the dialog forward in response to a single [`Input`] and
    /// translate the outcome into a [`ScreenCommand`] via the
    /// configured `on_action` callback (Task 2d / SPEC_v2_1.md §4.2).
    ///
    /// # Routing rules
    ///
    /// 1. **`Esc`** always emits [`DialogAction::Cancelled`]. The
    ///    adapter does not gate cancellation on a `cancellable` flag —
    ///    every Murder Motel dialog has historically allowed Esc to
    ///    walk away, and games that want to forbid it can simply
    ///    return [`ScreenCommand::None`] from their callback for the
    ///    `Cancelled` arm.
    /// 2. **Finished state.** When [`DialogState::is_finished`] is
    ///    true, `Enter` emits [`DialogAction::Finished`] and any other
    ///    non-`Esc` key is absorbed silently — the SPEC §4.2 contract
    ///    is "the dialog has nothing more to say," so we don't react
    ///    to stray input.
    /// 3. **Line-pumping mode.** While [`DialogState::current_line`]
    ///    yields `Some`, `Enter` calls
    ///    [`DialogState::advance`]. If that advance walks off the
    ///    end of the graph (terminal node, no further lines or
    ///    choices) the same press promotes to
    ///    [`DialogAction::Finished`] without forcing the player to
    ///    press Enter twice — the v2 example's hand-rolled scene
    ///    behaved the same way and the refactor MUST preserve that.
    /// 4. **Choice mode.** `Up`/`Down` move `choice_cursor` over
    ///    the *full* available-choice list. When the node has more
    ///    than [`DIALOG_PROMPT_MAX_CHOICES`] branches (Task 2f), the
    ///    adapter maintains a `choice_scroll` window so the visible
    ///    page follows the cursor: walking off the bottom edge
    ///    advances the page; walking off the top retreats it. `Enter`
    ///    synthesises the *window-relative* numeric hotkey
    ///    (`'1'..'9'`) and routes it through
    ///    [`crate::dialog::dialog_handle_prompt_input_window`], so
    ///    this adapter never re-implements the SPEC §8 selection /
    ///    flag-application pipeline. A literal `Char('1'..'9')` press
    ///    goes through the same windowed helper and is interpreted
    ///    page-relative — `2` always picks the second visible row
    ///    regardless of the current page.
    ///
    /// # Why synthesise a digit for Enter rather than call
    /// [`DialogState::choose`] directly
    ///
    /// SPEC_v2_1.md §4.2 mandates that the adapter "MUST use
    /// `dialog_choice_prompt` and `dialog_handle_prompt_input`
    /// internally rather than re-implementing dialog mechanics." The
    /// helper applies the picked choice's `set:` flags, advances the
    /// state machine, and returns the typed [`PromptAction`]
    /// outcome — the exact pipeline a hand-rolled `Enter` handler
    /// would otherwise duplicate. Synthesising a digit keeps the
    /// adapter as a thin keyboard router on top of the existing
    /// helper rather than a parallel implementation that could drift
    /// from it.
    ///
    /// # Cursor cap and scroll window
    ///
    /// The cursor is bounded by the live `available_choices` count
    /// (no `DIALOG_PROMPT_MAX_CHOICES` cap on the cursor itself —
    /// only on the visible window). When the node has more branches
    /// than fit on a page, an internal `visible_window_offset`
    /// helper keeps the `choice_scroll` field anchored so the
    /// cursor row is always
    /// visible. The render path uses the same offset so display and
    /// input agree on which page the player is looking at.
    ///
    /// # Borrow discipline
    ///
    /// Every `RefCell` borrow on `flags` is short-lived: we drop the
    /// immutable preview borrow before opening the `&mut` borrow the
    /// helper requires, and drop the mutable borrow before invoking
    /// `on_action` (callbacks routinely close over their own
    /// `Rc<RefCell<_>>` state and must be free to re-borrow `flags`
    /// indirectly). Forgetting either drop would deadlock the very
    /// next frame.
    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        // 1. Esc — uniform cancellation across every body mode. Routed
        // first so a finished dialog can still be left, and so a
        // line-pumping screen with no remaining choices doesn't
        // accidentally trap the player.
        if input == Input::Esc {
            return (self.on_action)(DialogAction::Cancelled);
        }

        // 2. Finished — only Enter is meaningful (the screen renders
        // "(They turn away.)"). Anything else is absorbed.
        if self.state.is_finished() {
            if input == Input::Enter {
                return (self.on_action)(DialogAction::Finished);
            }
            return ScreenCommand::None;
        }

        // 3. Line-pumping mode — walk the cursor forward one line per
        // Enter. Other keys are ignored so the player cannot, e.g.,
        // accidentally pick a hotkey before reading the setup text.
        if self.state.current_line(&self.dialog).is_some() {
            if input == Input::Enter {
                {
                    let mut flags = self.flags.borrow_mut();
                    // `advance` only errors on a finished dialog, which
                    // we already excluded above. Treat any future error
                    // surface as a no-op rather than panicking — the
                    // adapter's job is to keep the runtime alive even
                    // when an asset is malformed.
                    let _ = self.state.advance(&self.dialog, &mut flags);
                }
                if self.state.is_finished() {
                    return (self.on_action)(DialogAction::Finished);
                }
            }
            return ScreenCommand::None;
        }

        // 4. Choice mode. Compute the live total-choice count once
        // up-front so navigation, hotkey routing, and the synthesised
        // Enter path all agree on the same boundary. The cursor
        // ranges over the *full* available-choices list — Task 2f's
        // scroll window keeps it visible inside the
        // `DIALOG_PROMPT_MAX_CHOICES` cap.
        let total = {
            let flags = self.flags.borrow();
            self.state.available_choices(&self.dialog, &flags).len()
        };
        if total == 0 {
            // Nothing to pick. The render path shows a leave hint, so
            // anything other than Esc (handled above) is absorbed.
            return ScreenCommand::None;
        }
        // Re-anchor the scroll window against the live `total` so a
        // flag flip that shrank the list cannot leave the cursor
        // outside the visible page.
        self.choice_scroll = self.visible_window_offset(total);
        // Cursor cannot point past the live list — clamp before any
        // navigation arithmetic so subsequent `+ 1` / `- 1` operate
        // on a valid index. This also covers the case where a stale
        // cursor inherited from a longer list points off the end.
        if self.choice_cursor >= total {
            self.choice_cursor = total - 1;
        }

        match input {
            Input::Up => {
                // Saturating decrement keeps the cursor at 0 rather
                // than wrapping — matches what
                // `ChoicePrompt::step_from_input` does on the standard
                // navigation path, and is the convention every Murder
                // Motel scene already used. After moving, pull the
                // window up if the cursor walked off the top edge so
                // the player always sees the row they're highlighting.
                if self.choice_cursor > 0 {
                    self.choice_cursor -= 1;
                }
                if self.choice_cursor < self.choice_scroll {
                    self.choice_scroll = self.choice_cursor;
                }
                ScreenCommand::None
            }
            Input::Down => {
                if self.choice_cursor + 1 < total {
                    self.choice_cursor += 1;
                }
                // Push the window down if the cursor walked past the
                // bottom edge. The `+ 1` here is the "first row past
                // the visible page"; subtracting `MAX_CHOICES - 1`
                // lands the new top-of-window so the cursor sits on
                // the last visible row.
                if self.choice_cursor >= self.choice_scroll + DIALOG_PROMPT_MAX_CHOICES {
                    self.choice_scroll = self.choice_cursor + 1 - DIALOG_PROMPT_MAX_CHOICES;
                }
                ScreenCommand::None
            }
            Input::Enter => {
                // Capture the picked choice's `goto` *before*
                // applying — once the helper advances the state,
                // `current_node` reflects the destination and the
                // pre-advance metadata is gone. The cursor is the
                // full-list index; the windowed helper consumes it
                // directly.
                let target_node = {
                    let flags = self.flags.borrow();
                    self.state
                        .available_choices(&self.dialog, &flags)
                        .get(self.choice_cursor)
                        .map(|c| c.goto.clone())
                        .unwrap_or_default()
                };
                // Synthesise the matching numeric hotkey (`'1'..='9'`)
                // for the cursor's *window-relative* position. The
                // window invariant guarantees
                // `0 <= cursor - scroll < DIALOG_PROMPT_MAX_CHOICES`,
                // so the digit conversion is total.
                let window_relative = self.choice_cursor - self.choice_scroll;
                let digit = char::from_digit((window_relative as u32) + 1, 10)
                    .expect("window_relative < DIALOG_PROMPT_MAX_CHOICES (=9) by invariant");
                let synth = Input::Char(digit);
                let scroll = self.choice_scroll;
                let action = {
                    let mut flags = self.flags.borrow_mut();
                    dialog_handle_prompt_input_window(
                        &mut self.state,
                        &self.dialog,
                        &mut flags,
                        synth,
                        scroll,
                    )
                };
                // Reset cursor *and* scroll unconditionally — the next
                // frame's filtered choice list may be smaller (a
                // `set:` flag could have hidden a previously-visible
                // branch) and landing back at index 0 / page 0 is the
                // conservative default. Without this reset, paging
                // through suspect dialogs would leave the next dialog
                // mid-page on its first frame.
                self.choice_cursor = 0;
                self.choice_scroll = 0;
                match action {
                    Ok(PromptAction::Selected(idx)) => {
                        (self.on_action)(DialogAction::ChoicePicked {
                            choice_index: idx,
                            target_node,
                        })
                    }
                    // The helper returns Ok for non-Selected outcomes
                    // (None / Disabled / Cancelled). None of those are
                    // reachable from a synthesised numeric hotkey on a
                    // visible choice, but we surface them as no-ops
                    // rather than panicking so a future change to the
                    // helper's contract can't crash the runtime.
                    _ => ScreenCommand::None,
                }
            }
            Input::Char(c) if c.is_ascii_digit() && c != '0' => {
                // Direct numeric hotkey — players who learned to type
                // `2` instead of arrow-arrow-Enter keep that muscle
                // memory. The digit is *window-relative*: `2` always
                // picks the second visible row, even when the player
                // has scrolled to a later page. Map it back to a
                // full-list index for the `target_node` capture and
                // for the helper, which expects the windowed offset.
                let digit = c.to_digit(10).expect("ascii_digit") as usize;
                let scroll = self.choice_scroll;
                let target_node = {
                    let flags = self.flags.borrow();
                    self.state
                        .available_choices(&self.dialog, &flags)
                        .get(scroll + digit - 1)
                        .map(|c| c.goto.clone())
                        .unwrap_or_default()
                };
                let action = {
                    let mut flags = self.flags.borrow_mut();
                    dialog_handle_prompt_input_window(
                        &mut self.state,
                        &self.dialog,
                        &mut flags,
                        input,
                        scroll,
                    )
                };
                self.choice_cursor = 0;
                self.choice_scroll = 0;
                match action {
                    Ok(PromptAction::Selected(idx)) => {
                        (self.on_action)(DialogAction::ChoicePicked {
                            choice_index: idx,
                            target_node,
                        })
                    }
                    _ => ScreenCommand::None,
                }
            }
            _ => ScreenCommand::None,
        }
    }
}

impl DialogScreen {
    /// Compute the top-of-window offset for the visible choice page,
    /// given the live `total` available-choice count (Task 2f).
    ///
    /// Centralises the scroll-clamping arithmetic so the render path
    /// and `handle_input` agree byte-for-byte: a flag flip that
    /// shrank the list could otherwise leave one path scrolled past
    /// the end while the other bailed early. The rule is "the cursor
    /// must be visible, and the window cannot extend past `total`":
    ///
    /// 1. If `total <= MAX`, every choice fits on one page — the
    ///    offset is always `0`.
    /// 2. Otherwise the saved scroll cannot exceed `total - MAX` (or
    ///    the last visible row would be past the end).
    /// 3. Finally, if a stale cursor still falls below the saved
    ///    scroll, pull the window up so the cursor row is on screen.
    ///    This case fires when a flag flip removed choices *above*
    ///    the cursor between frames.
    fn visible_window_offset(&self, total: usize) -> usize {
        if total <= DIALOG_PROMPT_MAX_CHOICES {
            return 0;
        }
        let max_offset = total - DIALOG_PROMPT_MAX_CHOICES;
        let mut offset = self.choice_scroll.min(max_offset);
        let cursor = self.choice_cursor.min(total - 1);
        if cursor < offset {
            offset = cursor;
        } else if cursor >= offset + DIALOG_PROMPT_MAX_CHOICES {
            offset = cursor + 1 - DIALOG_PROMPT_MAX_CHOICES;
        }
        offset
    }
}

/// Split `inner` into a body rect and an optional one-row hint rect
/// along the bottom edge. Returns `None` for the hint when the inner
/// rect is too short to spare a row.
///
/// Extracted as a free function (not a method) so both line-pumping
/// and choice modes share the exact same split arithmetic — the hint
/// row width must match between modes or a player flicking through a
/// dialog will see the bottom row jump by a cell when the screen
/// transitions from line to choice.
fn split_body_and_hint(inner: Rect) -> (Rect, Option<Rect>) {
    let hint_h = if inner.height > 1 { 1 } else { 0 };
    let body_h = inner.height.saturating_sub(hint_h);
    let body = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: body_h,
    };
    let hint = if hint_h > 0 {
        Some(Rect {
            x: inner.x,
            y: inner.y + body_h,
            width: inner.width,
            height: hint_h,
        })
    } else {
        None
    };
    (body, hint)
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

    // ---------- Task 2c render tests ----------
    //
    // The render impl picks one of three body modes (line-pumping,
    // finished, choice) and one of two layouts (modal, compact). We
    // pin each mode/layout combination through a `TestBackend` rather
    // than via `insta` snapshots: the assertions below check shape
    // (border glyphs in modal, no border in compact, choice text
    // present, line text present) which is the contract real games
    // rely on. Pixel-perfect snapshots would over-fit the tests to
    // ratatui's internal layout choices and force churn on every
    // upstream bump.

    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
    use crate::screen::{GameContext, Screen};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Minimal config fixture — every field is required by the
    /// constructor, but render does not read any of them. Mirrors the
    /// shape used in `prompt_screen.rs` tests so future readers can
    /// hop between the two parallel suites without re-learning.
    fn fixture_config() -> GameConfig {
        GameConfig {
            game: GameSection {
                title: "Test".into(),
                slug: "test".into(),
                description: "fixture".into(),
                min_width: 80,
                min_height: 24,
                start_map: "lobby".into(),
                start_x: 1,
                start_y: 1,
            },
            save: SaveSection {
                strategy: SaveStrategy::PerFogletUser,
            },
            manifest: ManifestSection {
                timeout_ms: 1_800_000,
                idle_timeout_ms: 300_000,
                visibility: "members".into(),
                auth_scope: "site".into(),
            },
            world: Default::default(),
            turns: None,
            leaderboards: Vec::new(),
            multiplayer: None,
            factions: Default::default(),
        }
    }

    fn fixture_context() -> FogletContext {
        FogletContext {
            door_id: "test-door".into(),
            user_id: Some("test-user".into()),
            username: Some("tester".into()),
            role: None,
            session_id: Some("sess-1".into()),
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::LocalDev,
        }
    }

    /// Slightly richer dialog than [`fixture_dialog_yaml`]: greeting
    /// node has a script line and three branches, one of which is
    /// gated by `heard_rumor`. Lets the choice-mode render and gating
    /// tests share a single fixture.
    fn fixture_branching_yaml() -> &'static str {
        r#"
start: greeting
nodes:
  greeting:
    lines: ["The clerk eyes you."]
    choices:
      - text: "Just checking in."
        goto: end
      - text: "I heard about the murder."
        set: [heard_rumor]
        goto: end
      - text: "Got a master key?"
        requires: heard_rumor
        goto: end
  end: {}
"#
    }

    /// Render the buffer to a single string for substring assertions.
    /// A direct port of the technique used in `scenes/dialog.rs`
    /// tests; keeping the pattern local avoids a test-only crate
    /// dep just for pretty-printing.
    fn buffer_to_string(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn render_modal_paints_border_and_first_line() {
        // Default layout is `Modal`. Render the fixture at the start
        // (cursor on `greeting`, line 0) and pin two things: the
        // top-left border glyph proves the bordered block was drawn,
        // and the rendered buffer contains the line text.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let mut screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None);

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (40, 12));
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();

        assert_eq!(
            buf[(0, 0)].symbol(),
            "┌",
            "modal layout must draw a top-left border corner"
        );
        let dump = buffer_to_string(&buf);
        assert!(
            dump.contains("Hello, traveller."),
            "expected greeting line in modal body; got:\n{dump}"
        );
    }

    #[test]
    fn render_compact_skips_border() {
        // `compact()` builder doesn't exist yet (Task 2e), so we
        // toggle the layout via the field that the test module can
        // see. The post-Task-2e tests will switch to the builder.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let mut screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None).compact();

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (40, 12));
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        // Compact layout MUST NOT draw a border glyph at (0, 0). The
        // body text starts in the top-left, so the cell is either a
        // letter from the line or a blank — never a corner glyph.
        assert_ne!(
            buf[(0, 0)].symbol(),
            "┌",
            "compact layout must not draw a bordered block"
        );
        let dump = buffer_to_string(&buf);
        assert!(
            dump.contains("Hello, traveller."),
            "expected greeting line in compact body; got:\n{dump}"
        );
    }

    #[test]
    fn render_choices_show_filtered_visible_text() {
        // Walk past the single greeting line so the choice list is
        // visible, then render. The gated branch (`requires: heard_rumor`)
        // must not appear; the two ungated branches must.
        let dialog = load_dialog(fixture_branching_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let mut start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        // Manual advance past the script line so `available_choices`
        // returns the branches — Task 2d will own the input-driven
        // version of this walk.
        {
            let mut borrowed = flags.borrow_mut();
            start.advance(&dialog, &mut borrowed).expect("advance");
        }

        let mut screen = DialogScreen::new(dialog, start, flags.clone(), |_| ScreenCommand::None);
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (60, 12));
        let mut term = Terminal::new(TestBackend::new(60, 12)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let dump = buffer_to_string(term.backend().buffer());

        assert!(
            dump.contains("Just checking in."),
            "ungated choice should render; got:\n{dump}"
        );
        assert!(
            dump.contains("I heard about the murder."),
            "ungated choice should render; got:\n{dump}"
        );
        assert!(
            !dump.contains("master key"),
            "gated choice must hide while `heard_rumor` is unset; got:\n{dump}"
        );
    }

    #[test]
    fn render_finished_state_shows_leave_hint() {
        // Drive the fixture to its terminal node — `greeting` has a
        // line and a goto to `end` (an empty terminal). Two advances
        // walk us off the line and into `end`; render then shows the
        // "(They turn away.)" hint rather than panicking on an empty
        // body.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let mut start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        {
            // `advance` chases the linear `goto` in the same step
            // when the next line lands on a node with no available
            // choices, so a single advance is enough to reach `end`
            // and finish the dialog.
            let mut borrowed = flags.borrow_mut();
            start.advance(&dialog, &mut borrowed).expect("advance line");
        }
        assert!(start.is_finished(), "fixture should be finished");

        let mut screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None);
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (40, 12));
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let dump = buffer_to_string(term.backend().buffer());
        assert!(
            dump.contains("turn away"),
            "finished dialog must render the leave hint; got:\n{dump}"
        );
    }

    // ---------- Task 2d handle_input tests ----------
    //
    // The handle_input impl has four routing rules (Esc, finished,
    // line-pumping, choice). The tests below pin each — and in the
    // choice case, both navigation and selection — so a refactor that
    // collapses the body modes can be caught by a single test run
    // rather than only surfacing in the example's behavioural-parity
    // suite (Tasks 5–8). `GameContext` is required by the trait method
    // signature even though `handle_input` ignores it; we build the
    // same fixture the render tests use.

    /// Drive a single `Input` through the screen with a fresh
    /// `GameContext` and return the resulting [`ScreenCommand`].
    /// Wrapping the boilerplate keeps each test focused on the rule it
    /// is pinning rather than on context construction.
    fn dispatch(screen: &mut DialogScreen, input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (60, 12));
        screen.handle_input(&mut ctx, input)
    }

    /// Build a `(dialog, flags, state)` triple from the branching
    /// fixture, advanced past the script line so the choice list is
    /// the live body mode. Returns the triple without constructing the
    /// screen — tests that want a custom callback build it themselves.
    fn fixture_in_choice_mode() -> (Dialog, Rc<RefCell<FlagSet>>, DialogState) {
        let dialog = load_dialog(fixture_branching_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let mut state = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        {
            let mut borrowed = flags.borrow_mut();
            state.advance(&dialog, &mut borrowed).expect("advance line");
        }
        (dialog, flags, state)
    }

    #[test]
    fn handle_input_up_down_moves_choice_cursor() {
        // Two visible choices in the branching fixture (the third is
        // gated by `heard_rumor`). Down should walk 0 → 1, then Down
        // again should clamp at 1 rather than walking off the end.
        // Up should walk back 1 → 0, then clamp at 0.
        let (dialog, flags, state) = fixture_in_choice_mode();
        let mut screen = DialogScreen::new(dialog, state, flags, |_| ScreenCommand::None);
        assert_eq!(screen.choice_cursor, 0, "starts at first choice");

        let cmd = dispatch(&mut screen, Input::Down);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.choice_cursor, 1);

        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(
            screen.choice_cursor, 1,
            "cursor must clamp at last visible choice, not wrap"
        );

        let _ = dispatch(&mut screen, Input::Up);
        assert_eq!(screen.choice_cursor, 0);

        let _ = dispatch(&mut screen, Input::Up);
        assert_eq!(screen.choice_cursor, 0, "cursor must clamp at zero");
    }

    #[test]
    fn handle_input_enter_picks_current_choice_and_fires_callback() {
        // Move cursor to the second choice ("I heard about the
        // murder.") then press Enter. The callback must receive
        // `ChoicePicked { choice_index: 1, target_node: "end" }` and
        // the `set: [heard_rumor]` flag application must be visible
        // through the shared `Rc<RefCell<FlagSet>>`.
        let (dialog, flags, state) = fixture_in_choice_mode();
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let mut screen = DialogScreen::new(dialog, state, flags.clone(), move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::Pop
        });

        let _ = dispatch(&mut screen, Input::Down);
        let cmd = dispatch(&mut screen, Input::Enter);
        // The callback's return value flows out as the
        // ScreenCommand — pin it so a future refactor that swallowed
        // the value would surface here.
        assert!(matches!(cmd, ScreenCommand::Pop));

        let actions = recorded.borrow();
        assert_eq!(actions.len(), 1, "exactly one ChoicePicked emitted");
        match &actions[0] {
            DialogAction::ChoicePicked {
                choice_index,
                target_node,
            } => {
                assert_eq!(*choice_index, 1);
                assert_eq!(target_node, "end");
            }
            other => panic!("expected ChoicePicked, got {other:?}"),
        }

        // Branch flag application — the second choice's `set:` list
        // adds `heard_rumor`. The shared flag handle must reflect that.
        assert!(
            flags.borrow().contains("heard_rumor"),
            "picking the second choice must apply its `set:` flags"
        );
    }

    #[test]
    fn handle_input_filters_gated_choices_via_predicates() {
        // The third branch (`requires: heard_rumor`) is hidden until
        // the flag is set. Pressing Down twice from the start should
        // still leave the cursor at index 1 because `visible` only
        // sees two choices. After the flag is set externally, the
        // gated branch becomes pickable and Down can walk to index 2.
        let (dialog, flags, state) = fixture_in_choice_mode();
        let mut screen = DialogScreen::new(dialog, state, flags.clone(), |_| ScreenCommand::None);

        // Pre-flag: only two choices visible, cursor clamps at 1.
        let _ = dispatch(&mut screen, Input::Down);
        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(
            screen.choice_cursor, 1,
            "gated third choice must not be reachable via cursor"
        );

        // Flip the gate flag and walk again — now the third choice
        // is visible, so Down can reach index 2.
        flags.borrow_mut().insert("heard_rumor".to_string());
        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(
            screen.choice_cursor, 2,
            "flag flip must unlock the gated choice for cursor navigation"
        );
    }

    #[test]
    fn handle_input_esc_emits_cancelled() {
        // Esc must always emit `Cancelled`, regardless of body mode.
        // Test it from the choice-mode fixture; the other modes route
        // through the same first-line check.
        let (dialog, flags, state) = fixture_in_choice_mode();
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let mut screen = DialogScreen::new(dialog, state, flags, move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::Pop
        });

        let cmd = dispatch(&mut screen, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(recorded.borrow().len(), 1);
        assert_eq!(recorded.borrow()[0], DialogAction::Cancelled);
    }

    #[test]
    fn handle_input_enter_advances_lines() {
        // Line-pumping mode: Enter walks the line cursor one step.
        // Use the simple fixture (one line, then `goto: end`); after
        // a single Enter the dialog should be finished, which fires
        // `Finished` automatically (no second Enter needed) — pinning
        // the SPEC §4.2 "advance through terminals on the same press"
        // contract that the v2 example relied on.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let mut screen = DialogScreen::new(dialog, start, flags, move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::None
        });

        let _ = dispatch(&mut screen, Input::Enter);
        // Dialog should now be finished and Finished must have fired.
        assert!(screen.state.is_finished());
        assert_eq!(recorded.borrow().len(), 1);
        assert_eq!(recorded.borrow()[0], DialogAction::Finished);
    }

    #[test]
    fn handle_input_numeric_hotkey_picks_choice_directly() {
        // Pressing `2` should pick the second visible choice without
        // touching the cursor. Mirrors the SPEC §4.1 numeric-hotkey
        // path that the helper already supports.
        let (dialog, flags, state) = fixture_in_choice_mode();
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let mut screen = DialogScreen::new(dialog, state, flags.clone(), move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::Pop
        });

        let cmd = dispatch(&mut screen, Input::Char('2'));
        assert!(matches!(cmd, ScreenCommand::Pop));
        let actions = recorded.borrow();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            DialogAction::ChoicePicked { choice_index, .. } => {
                assert_eq!(*choice_index, 1, "digit `2` picks index 1");
            }
            other => panic!("expected ChoicePicked, got {other:?}"),
        }
        assert!(flags.borrow().contains("heard_rumor"));
    }

    #[test]
    fn handle_input_finished_state_enter_emits_finished() {
        // A dialog that is already finished should emit Finished on
        // the next Enter. Drive the simple fixture to its terminal
        // (the helper's auto-chase advances through the goto on the
        // same call) and pin the rule.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let mut start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        {
            let mut borrowed = flags.borrow_mut();
            start.advance(&dialog, &mut borrowed).expect("advance");
        }
        assert!(start.is_finished());
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let mut screen = DialogScreen::new(dialog, start, flags, move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::None
        });

        let _ = dispatch(&mut screen, Input::Enter);
        assert_eq!(recorded.borrow().len(), 1);
        assert_eq!(recorded.borrow()[0], DialogAction::Finished);
    }

    // ---------- Task 2e accessor tests ----------
    //
    // The accessors are tiny — they hand back references or rebind the
    // layout field — so the tests only need to confirm each one routes
    // to the right field without side effects. They also pin builder
    // chaining (`new(..).modal().compact()` lands in `Compact`) so a
    // future contributor cannot inadvertently break the
    // [`PromptScreen`] symmetry that motivated Task 2e.

    #[test]
    fn dialog_accessor_returns_loaded_graph() {
        // The accessor must hand back the *same* graph the constructor
        // received. Comparing to a freshly-loaded copy of the fixture
        // verifies equality without needing the screen to expose
        // pointer identity.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let screen = DialogScreen::new(dialog.clone(), start, flags, |_| ScreenCommand::None);
        assert_eq!(screen.dialog(), &dialog);
    }

    #[test]
    fn state_accessor_reflects_cursor_advances() {
        // Drive the state forward via `handle_input` (the public path)
        // and confirm `state()` reflects the new cursor. The simple
        // fixture has one line then a goto to the terminal — one Enter
        // walks both, so the post-input state must be `is_finished`.
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let mut screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None);
        assert!(
            !screen.state().is_finished(),
            "fresh state must not be finished"
        );
        let _ = dispatch(&mut screen, Input::Enter);
        assert!(
            screen.state().is_finished(),
            "Enter on the simple fixture must walk to the terminal"
        );
    }

    #[test]
    fn modal_and_compact_builders_set_layout() {
        // Cover the builder-chain shape — `.modal()` and `.compact()`
        // must each set the corresponding variant, and the last call
        // wins. This is the test that catches an accidental swap of
        // the two methods (a copy-paste hazard given how similar they
        // are to [`PromptScreen`]'s pair).
        let dialog = load_dialog(fixture_dialog_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let modal_screen = DialogScreen::new(dialog.clone(), start.clone(), flags.clone(), |_| {
            ScreenCommand::None
        })
        .modal();
        assert_eq!(modal_screen.layout(), DialogLayout::Modal);

        let compact_screen =
            DialogScreen::new(dialog.clone(), start.clone(), flags.clone(), |_| {
                ScreenCommand::None
            })
            .compact();
        assert_eq!(compact_screen.layout(), DialogLayout::Compact);

        // Last call wins — chaining `.modal().compact()` lands in
        // Compact, mirroring [`PromptScreen`]'s builder semantics.
        let chained = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None)
            .modal()
            .compact();
        assert_eq!(chained.layout(), DialogLayout::Compact);
    }

    #[test]
    fn render_clamps_choice_cursor_against_live_count() {
        // A stale `choice_cursor` (e.g. set by a prior render against
        // a longer choice list) must not point past the end of the
        // current list. We seed the cursor to 99, render, and rely on
        // the absence of a panic plus the presence of the first
        // choice's hotkey marker in the buffer to prove the clamp
        // happened. (`render` would panic via `Some(99)` on a 2-choice
        // prompt without the clamp; `ChoicePrompt::render` indexes
        // into `choices` for the highlighted-row marker.)
        let dialog = load_dialog(fixture_branching_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let mut start = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        {
            let mut borrowed = flags.borrow_mut();
            start.advance(&dialog, &mut borrowed).expect("advance");
        }
        let mut screen = DialogScreen::new(dialog, start, flags, |_| ScreenCommand::None);
        screen.choice_cursor = 99;

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (60, 12));
        let mut term = Terminal::new(TestBackend::new(60, 12)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw should not panic on out-of-range cursor");
        let dump = buffer_to_string(term.backend().buffer());
        assert!(
            dump.contains("Just checking in."),
            "first choice should still render after clamp; got:\n{dump}"
        );
    }

    // ---------- Task 2f scroll-on-overflow tests ----------
    //
    // SPEC_v2_1.md §4.2 requires deterministic scrolling when an
    // available-choice list exceeds [`DIALOG_PROMPT_MAX_CHOICES`].
    // The fixture below has 12 ungated choices on a single node; the
    // tests pin both navigation (cursor + window invariants) and
    // observable rendering (which choice texts are on screen as the
    // window slides).

    /// 12-choices fixture. All branches are ungated and lead to the
    /// terminal node so the test focus stays on cursor/scroll
    /// behaviour rather than flag mechanics.
    fn fixture_twelve_choices_yaml() -> &'static str {
        r#"
start: pick
nodes:
  pick:
    choices:
      - text: "Choice 01"
        goto: end
      - text: "Choice 02"
        goto: end
      - text: "Choice 03"
        goto: end
      - text: "Choice 04"
        goto: end
      - text: "Choice 05"
        goto: end
      - text: "Choice 06"
        goto: end
      - text: "Choice 07"
        goto: end
      - text: "Choice 08"
        goto: end
      - text: "Choice 09"
        goto: end
      - text: "Choice 10"
        goto: end
      - text: "Choice 11"
        goto: end
      - text: "Choice 12"
        goto: end
  end: {}
"#
    }

    fn fixture_twelve_choice_screen<F>(on_action: F) -> (DialogScreen, Rc<RefCell<FlagSet>>)
    where
        F: FnMut(DialogAction) -> ScreenCommand + 'static,
    {
        let dialog = load_dialog(fixture_twelve_choices_yaml()).expect("fixture parses");
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let state = {
            let mut borrowed = flags.borrow_mut();
            DialogState::start(&dialog, &mut borrowed)
        };
        let screen = DialogScreen::new(dialog, state, flags.clone(), on_action);
        (screen, flags)
    }

    #[test]
    fn handle_input_scrolls_window_when_cursor_walks_past_cap() {
        // Twelve visible choices, page size nine. Walking Down past
        // index 8 must advance `choice_scroll` so the cursor row
        // stays inside the rendered window. After eleven Downs the
        // cursor sits on the last choice (index 11) and the window
        // is anchored at scroll=3 so rows 3..=11 are visible.
        let (mut screen, _flags) = fixture_twelve_choice_screen(|_| ScreenCommand::None);
        assert_eq!(screen.choice_cursor, 0);
        assert_eq!(screen.choice_scroll, 0);

        // First eight Downs stay inside the initial page.
        for _ in 0..8 {
            let _ = dispatch(&mut screen, Input::Down);
        }
        assert_eq!(screen.choice_cursor, 8);
        assert_eq!(
            screen.choice_scroll, 0,
            "cursor still on the first page — no scroll yet"
        );

        // Ninth Down crosses the page boundary; the window slides
        // by one so the cursor row remains the bottom-of-page entry.
        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(screen.choice_cursor, 9);
        assert_eq!(
            screen.choice_scroll, 1,
            "scroll must advance once the cursor crosses MAX_CHOICES"
        );

        // Two more Downs walk to the last choice and scroll to the
        // last possible window (scroll=3 → rows 3..=11 visible).
        let _ = dispatch(&mut screen, Input::Down);
        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(screen.choice_cursor, 11);
        assert_eq!(screen.choice_scroll, 3);

        // Down at the bottom must clamp — neither cursor nor scroll
        // can advance past the live list.
        let _ = dispatch(&mut screen, Input::Down);
        assert_eq!(screen.choice_cursor, 11);
        assert_eq!(screen.choice_scroll, 3);
    }

    #[test]
    fn handle_input_scrolls_window_back_on_up() {
        // Mirror of the Down test: walk to the last choice, then Up
        // until the cursor is on the first page again. The window
        // must retreat with the cursor so the highlighted row is
        // always visible.
        let (mut screen, _flags) = fixture_twelve_choice_screen(|_| ScreenCommand::None);
        for _ in 0..11 {
            let _ = dispatch(&mut screen, Input::Down);
        }
        assert_eq!((screen.choice_cursor, screen.choice_scroll), (11, 3));

        // Two Ups stay inside the bottom window (cursor 9 still
        // visible at scroll=3 → rows 3..=11).
        let _ = dispatch(&mut screen, Input::Up);
        let _ = dispatch(&mut screen, Input::Up);
        assert_eq!((screen.choice_cursor, screen.choice_scroll), (9, 3));

        // Walking past the top of the window pulls scroll back so
        // the cursor row stays on screen.
        let _ = dispatch(&mut screen, Input::Up);
        assert_eq!((screen.choice_cursor, screen.choice_scroll), (8, 3));
        // Continue Up until the cursor lands at index 2 — by that
        // point the window must have retreated to scroll=2.
        for _ in 0..6 {
            let _ = dispatch(&mut screen, Input::Up);
        }
        assert_eq!((screen.choice_cursor, screen.choice_scroll), (2, 2));
    }

    #[test]
    fn render_paginated_choices_show_only_visible_window() {
        // With twelve choices and the cursor on index 11, only the
        // last nine rows ("Choice 04" through "Choice 12") must
        // render — the first three are scrolled off the top.
        let (mut screen, _flags) = fixture_twelve_choice_screen(|_| ScreenCommand::None);
        for _ in 0..11 {
            let _ = dispatch(&mut screen, Input::Down);
        }
        assert_eq!(screen.choice_scroll, 3);

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (60, 18));
        let mut term = Terminal::new(TestBackend::new(60, 18)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let dump = buffer_to_string(term.backend().buffer());

        // Off-page rows MUST NOT appear.
        for label in ["Choice 01", "Choice 02", "Choice 03"] {
            assert!(
                !dump.contains(label),
                "row {label} is above the visible window; got:\n{dump}"
            );
        }
        // On-page rows MUST appear.
        for label in [
            "Choice 04",
            "Choice 05",
            "Choice 06",
            "Choice 07",
            "Choice 08",
            "Choice 09",
            "Choice 10",
            "Choice 11",
            "Choice 12",
        ] {
            assert!(
                dump.contains(label),
                "row {label} should be visible at scroll=3; got:\n{dump}"
            );
        }
    }

    #[test]
    fn handle_input_enter_picks_offscreen_choice_via_window_relative_hotkey() {
        // Walk the cursor to the 11th choice (full index 10, page
        // index 7 once scroll lands at 3) and press Enter. The
        // adapter must synthesise a *window-relative* digit (`'8'`)
        // and route it through the windowed helper so the picked
        // full-list index is 10, not 7.
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let (mut screen, _flags) = fixture_twelve_choice_screen(move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::Pop
        });
        for _ in 0..10 {
            let _ = dispatch(&mut screen, Input::Down);
        }
        assert_eq!((screen.choice_cursor, screen.choice_scroll), (10, 2));

        let cmd = dispatch(&mut screen, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Pop));
        let actions = recorded.borrow();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            DialogAction::ChoicePicked { choice_index, .. } => {
                assert_eq!(
                    *choice_index, 10,
                    "Enter on cursor 10 must surface the full-list index, not the page-relative one"
                );
            }
            other => panic!("expected ChoicePicked, got {other:?}"),
        }
        // Cursor and scroll must reset for the next dialog frame —
        // the next conversation should start from the top.
        assert_eq!(screen.choice_cursor, 0);
        assert_eq!(screen.choice_scroll, 0);
    }

    #[test]
    fn handle_input_numeric_hotkey_is_window_relative() {
        // Scroll to the bottom page, then press `1` — the player
        // should pick the *first visible* row (full index 3), not
        // the absolute first choice in the list.
        let recorded: Rc<RefCell<Vec<DialogAction>>> = Rc::new(RefCell::new(Vec::new()));
        let recorded_writer = recorded.clone();
        let (mut screen, _flags) = fixture_twelve_choice_screen(move |a| {
            recorded_writer.borrow_mut().push(a);
            ScreenCommand::Pop
        });
        for _ in 0..11 {
            let _ = dispatch(&mut screen, Input::Down);
        }
        assert_eq!(screen.choice_scroll, 3);

        let _ = dispatch(&mut screen, Input::Char('1'));
        let actions = recorded.borrow();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            DialogAction::ChoicePicked { choice_index, .. } => {
                assert_eq!(
                    *choice_index, 3,
                    "digit `1` on the second page picks the first *visible* row (full index 3)"
                );
            }
            other => panic!("expected ChoicePicked, got {other:?}"),
        }
    }
}
