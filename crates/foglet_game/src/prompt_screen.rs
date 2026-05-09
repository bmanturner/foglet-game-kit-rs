//! `PromptScreen<T>` — optional [`Screen`] adapter that renders a
//! [`ChoicePrompt`] and routes its [`PromptAction`] outcomes back to the
//! runtime as [`ScreenCommand`] values (SPEC_v1_1.md §4 + §5.5).
//!
//! # When to reach for this
//!
//! Use `PromptScreen` when the screen is *just* a prompt: a loot drawer,
//! a yes/no confirmation, an "are you sure" gate, an any-key pause, the
//! Murder Motel night-clerk vendor (Task 11). The adapter wires the
//! standard prompt loop — direct-key match, navigation movement, render
//! into the full frame — so authors don't have to repeat the boilerplate
//! every time.
//!
//! Compose [`ChoicePrompt`] manually inside a custom [`Screen`] when the
//! prompt is one panel among several (e.g. a map plus an inline action
//! menu) or when the screen owns mutable state the prompt has to consult
//! across frames (e.g. shop labels that recompute from live cash). The
//! callback signature here intentionally does not see the per-frame
//! [`GameContext`] — it runs *after* `handle_input`, so games that need
//! to peek at runtime data while choosing their `ScreenCommand` should
//! drop one layer down to `ChoicePrompt::handle` directly.
//!
//! See `docs/prompt-screens.md` for the full authoring rule of thumb on
//! choosing between this adapter and a hand-rolled [`Screen`] that
//! composes [`ChoicePrompt`] manually.
//!
//! # Why a callback, not a return-value-only design
//!
//! [`ScreenCommand`] is the SPEC §5.5 vocabulary the runtime understands;
//! [`PromptAction`] is the prompt's narrower vocabulary. The mapping
//! between them is game-specific (one game's `Selected(LeaveDrawer)`
//! becomes `Pop`; another's becomes `Replace`), so the adapter must
//! defer to the author. A boxed `FnMut` keeps the type concrete (no
//! extra generic parameter on `PromptScreen`) and lets games close over
//! their own state — the most common case is a captured `Rc<RefCell<_>>`
//! holding inventory or cash that the callback updates before returning
//! [`ScreenCommand::Pop`].

use crate::input::Input;
use crate::prompt::{ChoicePrompt, PromptAction};
use crate::screen::{GameContext, Screen, ScreenCommand};

/// Layout mode for [`PromptScreen`] rendering.
///
/// Mirrors the two render entry points on [`ChoicePrompt`]:
/// [`ChoicePrompt::render`] for the SPEC §4.3 compact unboxed shape, and
/// [`ChoicePrompt::render_modal`] for the SPEC §4.8 bordered modal. The
/// adapter keeps both available rather than picking one because real
/// games mix them — title-screen menus tend to be compact, mid-game
/// confirmations tend to be modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptLayout {
    /// SPEC §4.3 compact unboxed layout: no border, body and choices
    /// flow top-down inside the frame area. Default for `PromptScreen`.
    Compact,
    /// SPEC §4.8 bordered modal layout: a [`ratatui::widgets::Block`]
    /// frame with the prompt title in the top border and the prompt
    /// content rendered into the inner rect.
    Modal,
}

/// Boxed callback type for translating a [`PromptAction`] outcome into a
/// [`ScreenCommand`]. Extracted into an alias so the field type stays
/// readable in `Debug` impls and so the trait object bound (`'static` +
/// [`FnMut`]) lives in one place.
type ActionCallback<T> = Box<dyn FnMut(PromptAction<T>) -> ScreenCommand + 'static>;

/// `Screen` adapter wrapping a [`ChoicePrompt`] and a callback that
/// converts each [`PromptAction`] into a [`ScreenCommand`].
///
/// # Lifecycle
///
/// 1. `render` paints the prompt into the frame using the configured
///    [`PromptLayout`]. The prompt owns its own state (cursor, body,
///    choices), so `render` is read-only with respect to the screen's
///    own fields beyond updating the buffer.
/// 2. `handle_input` first asks the prompt to consume navigation keys
///    via [`ChoicePrompt::step_from_input`]. If the input moved the
///    cursor, no [`ScreenCommand`] is produced — the next frame just
///    redraws the new highlight. Otherwise the input flows through
///    [`ChoicePrompt::handle`] and the resulting [`PromptAction`] is
///    routed through the callback.
/// 3. `tick` and `on_resize` fall back to the `Screen` defaults
///    ([`ScreenCommand::None`]). Prompts have no animation or layout
///    cache to refresh, and resize is already covered by the runtime
///    re-rendering the next frame against the new size.
///
/// # Generic bounds
///
/// `T: Clone + 'static` matches what [`ChoicePrompt::handle`] requires
/// (it clones the chosen choice's id into [`PromptAction`]) and what
/// `Box<dyn Screen>` requires (no borrowed data leaks through the trait
/// object). Real games use small `Copy` enums for `T`, so neither bound
/// is a practical constraint.
pub struct PromptScreen<T> {
    prompt: ChoicePrompt<T>,
    on_action: ActionCallback<T>,
    layout: PromptLayout,
}

impl<T: Clone + 'static> PromptScreen<T> {
    /// Build a new `PromptScreen` from a [`ChoicePrompt`] and a callback
    /// mapping [`PromptAction`] outcomes to [`ScreenCommand`] values.
    ///
    /// Defaults to [`PromptLayout::Compact`]; call [`PromptScreen::modal`]
    /// to switch to the bordered modal layout.
    ///
    /// The callback is stored as a boxed `FnMut`, so it can mutate
    /// captured state across calls — typical use is closing over a
    /// shared `Rc<RefCell<GameState>>` to mutate inventory/cash before
    /// emitting [`ScreenCommand::Pop`].
    pub fn new<F>(prompt: ChoicePrompt<T>, on_action: F) -> Self
    where
        F: FnMut(PromptAction<T>) -> ScreenCommand + 'static,
    {
        Self {
            prompt,
            on_action: Box::new(on_action),
            layout: PromptLayout::Compact,
        }
    }

    /// Switch the layout to [`PromptLayout::Modal`] (SPEC §4.8 bordered
    /// modal). Builder-style so the call site reads
    /// `PromptScreen::new(...).modal()` for the common confirmation case.
    pub fn modal(mut self) -> Self {
        self.layout = PromptLayout::Modal;
        self
    }

    /// Switch the layout to [`PromptLayout::Compact`] (SPEC §4.3). The
    /// adapter starts in compact mode so this method exists primarily
    /// for symmetry with [`PromptScreen::modal`] and for explicit
    /// callers that want to spell out their layout choice.
    pub fn compact(mut self) -> Self {
        self.layout = PromptLayout::Compact;
        self
    }

    /// Borrow the underlying [`ChoicePrompt`] — useful for tests that
    /// want to inspect the prompt's state without driving it through the
    /// `Screen` trait.
    pub fn prompt(&self) -> &ChoicePrompt<T> {
        &self.prompt
    }

    /// Mutably borrow the underlying [`ChoicePrompt`]. Provided for the
    /// rare case where a game wants to mutate the prompt between frames
    /// without recreating the whole screen (e.g. updating a dynamic
    /// label). Most authors should rebuild the prompt instead — keeping
    /// the prompt as immutable post-construction makes the reducer
    /// easier to reason about.
    pub fn prompt_mut(&mut self) -> &mut ChoicePrompt<T> {
        &mut self.prompt
    }

    /// Inspect the configured [`PromptLayout`]. Exposed for tests; runtime
    /// callers have no reason to read it (the layout choice is internal).
    pub fn layout(&self) -> PromptLayout {
        self.layout
    }
}

impl<T: Clone + 'static> Screen for PromptScreen<T> {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        let buf = frame.buffer_mut();
        match self.layout {
            PromptLayout::Compact => {
                self.prompt.render(area, buf);
            }
            PromptLayout::Modal => {
                self.prompt.render_modal(area, buf);
            }
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        // Navigation movement (Up/Down/j/k) mutates the cursor without
        // emitting a `PromptAction`; the next render simply redraws the
        // new highlight. Returning `None` here matches what every other
        // screen does for "input absorbed, no transition" — and crucially
        // prevents the same press from also being interpreted as a
        // direct-key hotkey by the fall-through path below.
        if self.prompt.step_from_input(input) {
            return ScreenCommand::None;
        }
        let action = self.prompt.handle(input);
        // Callback drives the SPEC §5.5 mapping. We never inspect
        // `action` past handing it over: the game owns the policy of
        // "Pop on Selected? Replace on Cancelled? Message on Disabled?"
        // and the adapter exists precisely so it can.
        (self.on_action)(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
    use crate::prompt::{ChoicePrompt, PromptAction};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Stable choice id used across the tests below — small `Copy` enum
    /// matches the shape real games use for `T`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DrawerChoice {
        TakeKey,
        Leave,
    }

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

    fn drawer_prompt() -> ChoicePrompt<DrawerChoice> {
        ChoicePrompt::new()
            .title("Lost-and-Found Drawer")
            .body("A drawer of forgotten things.")
            .choice('k', DrawerChoice::TakeKey, "Take Room 7 key")
            .choice('l', DrawerChoice::Leave, "Leave it")
            .cancellable(true)
    }

    #[test]
    fn defaults_to_compact_layout() {
        let screen = PromptScreen::new(drawer_prompt(), |_| ScreenCommand::None);
        assert_eq!(screen.layout(), PromptLayout::Compact);
    }

    #[test]
    fn modal_builder_switches_layout() {
        let screen = PromptScreen::new(drawer_prompt(), |_| ScreenCommand::None).modal();
        assert_eq!(screen.layout(), PromptLayout::Modal);
    }

    #[test]
    fn handle_input_routes_selected_through_callback() {
        // Captured via `Rc<RefCell<_>>` so the assertion can read the
        // last action the callback saw, mirroring how a real game would
        // close over its own mutable state.
        let last: Rc<RefCell<Option<PromptAction<DrawerChoice>>>> = Rc::new(RefCell::new(None));
        let last_writer = last.clone();
        let mut screen = PromptScreen::new(drawer_prompt(), move |action| {
            *last_writer.borrow_mut() = Some(action.clone());
            match action {
                PromptAction::Selected(DrawerChoice::Leave) => ScreenCommand::Pop,
                PromptAction::Selected(_) => ScreenCommand::None,
                _ => ScreenCommand::None,
            }
        });

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        // Pressing the lowercase hotkey for `Leave` must reach the
        // callback as `Selected(Leave)` and the callback's `Pop` must
        // bubble out unchanged — the adapter is *just* a bridge, not a
        // policy layer.
        let cmd = screen.handle_input(&mut ctx, Input::Char('l'));
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(
            last.borrow().clone(),
            Some(PromptAction::Selected(DrawerChoice::Leave))
        );
    }

    #[test]
    fn handle_input_routes_cancelled_when_cancellable() {
        let mut screen = PromptScreen::new(drawer_prompt(), |action| match action {
            PromptAction::Cancelled => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        });
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn handle_input_uppercase_hotkey_matches_lowercase_choice() {
        // SPEC §4.1 case-insensitive direct keys: pressing `K` must hit
        // the same choice the lowercase `k` would. The adapter doesn't
        // do any case work itself; this test pins that we haven't
        // accidentally lost normalization in the routing path.
        let seen: Rc<RefCell<Vec<DrawerChoice>>> = Rc::new(RefCell::new(Vec::new()));
        let seen_writer = seen.clone();
        let mut screen = PromptScreen::new(drawer_prompt(), move |action| {
            if let PromptAction::Selected(c) = action {
                seen_writer.borrow_mut().push(c);
            }
            ScreenCommand::None
        });
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let _ = screen.handle_input(&mut ctx, Input::Char('K'));
        assert_eq!(seen.borrow().clone(), vec![DrawerChoice::TakeKey]);
    }

    #[test]
    fn navigation_input_does_not_invoke_callback() {
        // In navigation mode (`.navigable(true)`), Up/Down should mutate
        // the cursor and return `ScreenCommand::None` *without* the
        // callback ever firing — otherwise the same press would also be
        // interpreted as a direct-key hotkey, double-dispatching.
        let invoked = Rc::new(RefCell::new(0u32));
        let invoked_writer = invoked.clone();
        let prompt = drawer_prompt().navigable(true);
        let mut screen = PromptScreen::new(prompt, move |_| {
            *invoked_writer.borrow_mut() += 1;
            ScreenCommand::None
        });
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Down);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(*invoked.borrow(), 0);
        // Cursor advanced — this is the user-visible side-effect that
        // proves the navigation arm fired rather than a quiet no-op.
        assert_eq!(screen.prompt().selected, Some(1));
    }

    #[test]
    fn render_writes_choice_label_in_compact_layout() {
        let mut screen = PromptScreen::new(drawer_prompt(), |_| ScreenCommand::None);
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        // Look for the `(K) Take Room 7 key` row anywhere in the
        // rendered buffer. We don't pin the exact y-coordinate because
        // the prompt's body wrapping and blank-line policy is the
        // contract under test in the prompt module — here we only need
        // proof that rendering ran end-to-end.
        let mut found = false;
        for y in 0..buf.area.height {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            if row.contains("(K) Take Room 7 key") {
                found = true;
                break;
            }
        }
        assert!(found, "compact render should include the choice row");
    }

    #[test]
    fn render_draws_border_in_modal_layout() {
        let mut screen = PromptScreen::new(drawer_prompt(), |_| ScreenCommand::None).modal();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (40, 12));
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        // Top-left corner glyph proves the modal block was drawn.
        // Ratatui 0.29 uses `┌` for the default `Borders::ALL` corner.
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(39, 0)].symbol(), "┐");
        assert_eq!(buf[(0, 11)].symbol(), "└");
    }

    /// Minimal in-memory event source mimicking the runtime loop's
    /// `poll → handle_input` cadence without any terminal I/O. SPEC §13
    /// requires runtime tests stay off the live TUI; a `VecDeque` of
    /// `Input` values is the simplest "fake event source" that still
    /// exercises the same code path the production loop uses (one
    /// `handle_input` call per polled event).
    struct FakeEventSource {
        events: std::collections::VecDeque<Input>,
    }

    impl FakeEventSource {
        fn new<I: IntoIterator<Item = Input>>(events: I) -> Self {
            Self {
                events: events.into_iter().collect(),
            }
        }

        fn next(&mut self) -> Option<Input> {
            self.events.pop_front()
        }
    }

    #[test]
    fn fake_event_source_drives_screen_to_selection() {
        // End-to-end check that mirrors how the runtime would feed a
        // `PromptScreen` from its event poll. We script a short input
        // sequence (an ignored `Resize`, a navigation `Down` on a
        // navigable prompt, a final lowercase hotkey) and assert both
        // the captured `PromptAction`s *and* the `ScreenCommand`
        // emitted at each step. This is the contract Task 7b is meant
        // to lock in: a fake event source can drive `PromptScreen`
        // through `Screen::handle_input` and observe the same outcomes
        // the real runtime would.
        let captured: Rc<RefCell<Vec<PromptAction<DrawerChoice>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let captured_writer = captured.clone();
        let prompt = drawer_prompt().navigable(true);
        let mut screen = PromptScreen::new(prompt, move |action| {
            captured_writer.borrow_mut().push(action.clone());
            match action {
                PromptAction::Selected(DrawerChoice::TakeKey) => ScreenCommand::Pop,
                PromptAction::Selected(DrawerChoice::Leave) => ScreenCommand::None,
                PromptAction::Cancelled => ScreenCommand::Pop,
                _ => ScreenCommand::None,
            }
        });

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        let mut events = FakeEventSource::new([
            Input::Resize {
                width: 80,
                height: 24,
            },
            Input::Down,
            Input::Char('k'),
        ]);

        let mut commands = Vec::new();
        while let Some(ev) = events.next() {
            commands.push(screen.handle_input(&mut ctx, ev));
        }

        // Three events in → three `ScreenCommand`s out. Resize and
        // navigation produce `None`; only the final hotkey transitions.
        assert_eq!(commands.len(), 3);
        assert!(matches!(commands[0], ScreenCommand::None));
        assert!(matches!(commands[1], ScreenCommand::None));
        assert!(matches!(commands[2], ScreenCommand::Pop));

        // The callback must only have fired for events the prompt
        // actually translated into a `PromptAction` — not for the
        // navigation step (consumed by `step_from_input`) and not for
        // the `Resize` (returns `PromptAction::None`, but still routed
        // through the callback per `handle_input`'s contract). We pin
        // both: exactly one `Selected(TakeKey)` at the end, preceded by
        // one `None` from the resize.
        let actions = captured.borrow().clone();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0], PromptAction::None);
        assert_eq!(actions[1], PromptAction::Selected(DrawerChoice::TakeKey));
    }

    #[test]
    fn fake_event_source_routes_cancellation_through_screen_command() {
        // Pairs with the test above: a fake source can also drive the
        // cancellation arm to a `ScreenCommand`. Using `Esc` here is
        // the canonical "back out of the prompt" gesture (SPEC §4.4).
        let mut screen = PromptScreen::new(drawer_prompt(), |action| match action {
            PromptAction::Cancelled => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        });
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        let mut events = FakeEventSource::new([Input::Esc]);
        let cmd = screen.handle_input(&mut ctx, events.next().expect("scripted event"));
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert!(events.next().is_none());
    }

    #[test]
    fn screen_is_object_safe_via_box_dyn() {
        // PromptScreen has to fit through `Box<dyn Screen>` for it to be
        // usable with `ScreenCommand::Push`/`Replace`. This test would
        // fail to compile if a future change accidentally added a
        // non-object-safe method to `Screen` or to the adapter.
        let screen = PromptScreen::new(drawer_prompt(), |_| ScreenCommand::None);
        let _boxed: Box<dyn Screen> = Box::new(screen);
    }
}
