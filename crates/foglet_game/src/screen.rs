//! Screen trait, screen commands, and the per-frame [`GameContext`]
//! handed to screens during render/input/tick.
//!
//! Task 7a deliberately ships **types only**. The pure stack
//! reducer (`apply_command`) lives in Task 7b, the `Game` builder in
//! 7c, and the runtime loop wiring (event poll → render → tick) in
//! 7d. Splitting it this way keeps each commit small enough
//! to review and lets the reducer be unit-tested without any terminal
//! plumbing.
//!
//! ## Why `GameContext` is a borrow-bag, not the runtime
//!
//! SPEC §5.3 gives the runtime ownership of `config`,
//! `foglet_context`, `terminal_size`, the screen stack, the save
//! manager, a `running` flag, and `last_tick`. Screens only need a
//! read view of some of those — passing them the whole `GameRuntime`
//! would let a screen reach in and pop itself off the stack mid-
//! render, which is precisely what [`ScreenCommand`] exists to
//! prevent.
//!
//! [`GameContext`] is therefore a thin struct of borrows that the
//! runtime reconstructs each iteration of the loop. It is the *only*
//! handle screens have on runtime data. Adding a field here is a
//! deliberate widening of the screen API surface and should be
//! justified the same way a new SPEC §5.5 command would be.
//!
//! ## Why `ScreenCommand::Push` boxes the screen
//!
//! `Screen` is object-safe (every method takes `&mut self` and uses
//! only concrete arguments) so the screen stack stores
//! `Box<dyn Screen>`. `Push` and `Replace` therefore carry a
//! `Box<dyn Screen>` rather than a generic `S: Screen`, matching how
//! the runtime will store them in 7b.

use crate::config::GameConfig;
use crate::foglet::FogletContext;
use crate::input::Input;

/// Per-frame view of the runtime state passed to every [`Screen`]
/// callback.
///
/// **Lifetime.** A fresh [`GameContext`] is constructed by the
/// runtime each iteration of the loop and dropped before the next
/// poll, so screens cannot stash it across frames — and they
/// shouldn't try, because the underlying values are owned by the
/// runtime. If you need to remember something between frames, store
/// it on `self`.
///
/// **Mutability.** The wrapper is `&mut`-handed to screens (per
/// SPEC §8.2), but the borrowed sub-values are intentionally `&` for
/// the immutable game-wide data (`config`, `foglet`). `terminal_size`
/// is a `Copy` pair the runtime overwrites on resize before the next
/// frame; screens may read it but mutating it from a screen has no
/// effect beyond the current callback.
#[derive(Debug)]
pub struct GameContext<'a> {
    /// Parsed `assets/game.toml`. Title, slug, min size, etc. (SPEC
    /// §5.2). Immutable for the lifetime of the run.
    pub config: &'a GameConfig,
    /// Foglet door context for this session — door id, user id,
    /// terminal size hint, and friends (SPEC §5.1). Immutable for the
    /// lifetime of the run; the runtime is the single source of
    /// truth.
    pub foglet: &'a FogletContext,
    /// Current terminal size in cells, updated by the runtime when a
    /// `Resize` input is dispatched. Screens that need to lay out
    /// against the live size (e.g. centring widgets) should read this
    /// rather than re-querying `crossterm`.
    pub terminal_size: (u16, u16),
}

impl<'a> GameContext<'a> {
    /// Convenience constructor used by the runtime (and by tests) to
    /// pack the borrowed runtime references into a [`GameContext`].
    ///
    /// Kept inherent rather than `pub(crate)` so example games and
    /// downstream tests can build a context without standing up the
    /// full runtime.
    pub fn new(
        config: &'a GameConfig,
        foglet: &'a FogletContext,
        terminal_size: (u16, u16),
    ) -> Self {
        Self {
            config,
            foglet,
            terminal_size,
        }
    }
}

/// Command returned by a [`Screen`] callback to instruct the runtime
/// what to do next. Mirrors SPEC §5.5.
///
/// Screens never mutate the screen stack directly; they emit one of
/// these and the runtime applies it in `apply_command` (Task 7b). This keeps screen logic pure and unit-testable, and
/// guarantees the runtime gets a chance to (e.g.) flush the save or
/// drop the terminal guard before exiting.
///
/// `PartialEq`/`Eq` are intentionally **not** derived: `Push` and
/// `Replace` carry a trait object that has no meaningful equality.
/// Tests that need to inspect a returned command should pattern-match
/// on the variant.
pub enum ScreenCommand {
    /// Do nothing. The default return for `handle_input` / `tick` /
    /// `on_resize` when the screen has no transition to request.
    None,
    /// Push a new screen on top of the current one. The current
    /// screen stays on the stack and resumes when `Pop` is applied.
    Push(Box<dyn Screen>),
    /// Pop the top screen. If the stack would become empty the
    /// runtime treats this as a clean exit (decision deferred to
    /// 7b/7d).
    Pop,
    /// Replace the top screen with a new one. The previous top is
    /// dropped immediately; no transition back is possible.
    Replace(Box<dyn Screen>),
    /// Exit the runtime cleanly. Equivalent to popping every screen
    /// off the stack.
    Quit,
    /// Trigger a save flush via the runtime's save manager. The
    /// runtime decides what to write; this command just nominates
    /// "now is a good time".
    Save,
    /// Show a transient message to the player. The runtime decides
    /// where (status line, modal, log) — screens just describe the
    /// intent.
    Message(String),
    /// A screen-level error that the runtime should surface to the
    /// player and/or operator log. Plain `String` rather than a typed
    /// error keeps `ScreenCommand` free of `thiserror` / `anyhow`
    /// coupling; richer typing can wait for a real use case.
    Error(String),
}

impl std::fmt::Debug for ScreenCommand {
    /// Hand-written so `Push`/`Replace` print as e.g.
    /// `Push(<dyn Screen>)` instead of failing to derive `Debug` on
    /// the trait object.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Push(_) => f.write_str("Push(<dyn Screen>)"),
            Self::Pop => f.write_str("Pop"),
            Self::Replace(_) => f.write_str("Replace(<dyn Screen>)"),
            Self::Quit => f.write_str("Quit"),
            Self::Save => f.write_str("Save"),
            Self::Message(s) => f.debug_tuple("Message").field(s).finish(),
            Self::Error(s) => f.debug_tuple("Error").field(s).finish(),
        }
    }
}

/// A logical game screen — title, menu, map, dialog, inventory, etc.
///
/// Mirrors SPEC §8.2. Screens render themselves into a Ratatui frame,
/// receive normalized [`Input`] events, and respond by emitting a
/// [`ScreenCommand`] rather than mutating the runtime directly.
///
/// ## Default impls
///
/// `handle_input`, `tick`, and `on_resize` default to
/// [`ScreenCommand::None`] so a render-only screen (splash, credits)
/// can implement just `render` and ignore the rest. `render` has no
/// default because every screen must put *something* on the frame —
/// silently rendering nothing is almost always a bug worth surfacing
/// at compile time.
///
/// ## Object safety
///
/// All methods take `&mut self` and use only concrete argument types,
/// so `Box<dyn Screen>` is well-formed. The screen stack and the
/// `Push`/`Replace` variants of [`ScreenCommand`] both rely on this.
pub trait Screen {
    /// Draw the screen into the given Ratatui frame.
    ///
    /// Implementations should be **pure with respect to the frame**:
    /// no side effects beyond writing widgets. Game-state mutations
    /// belong in `tick` or in response to `handle_input`.
    fn render(&mut self, ctx: &mut GameContext<'_>, frame: &mut ratatui::Frame<'_>);

    /// Handle a normalized [`Input`] event.
    ///
    /// Returns the runtime's next instruction. Default:
    /// [`ScreenCommand::None`] so screens that ignore input (or only
    /// react to `tick`) need not override this.
    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
        ScreenCommand::None
    }

    /// Per-frame tick hook for animation, cooldowns, AI, etc.
    ///
    /// The runtime calls this once per frame *after* `handle_input`
    /// (decision finalized in 7d). Default: [`ScreenCommand::None`].
    fn tick(&mut self, _ctx: &mut GameContext<'_>) -> ScreenCommand {
        ScreenCommand::None
    }

    /// Notify the screen that the terminal was resized.
    ///
    /// The runtime also updates [`GameContext::terminal_size`] before
    /// the next render, so most screens won't need to override this.
    /// Provided for screens that want to e.g. re-cache a layout.
    /// Default: [`ScreenCommand::None`].
    fn on_resize(
        &mut self,
        _ctx: &mut GameContext<'_>,
        _width: u16,
        _height: u16,
    ) -> ScreenCommand {
        ScreenCommand::None
    }
}

/// The screen stack the runtime owns and reducers operate over.
///
/// A type alias rather than a newtype because the stack is just a
/// `Vec<Box<dyn Screen>>` — wrapping it would force callers through an
/// inherent-method API for trivial pushes/pops the runtime already
/// drives via [`apply_command`]. Keeping it transparent also lets tests
/// build a stack with `vec![Box::new(MyScreen)]` and inspect `.len()`
/// directly.
pub type ScreenStack = Vec<Box<dyn Screen>>;

/// Reason the runtime should leave its main loop, returned by
/// [`apply_command`] when a command transitions the stack into a
/// terminal state.
///
/// The runtime translates this into the SPEC §7.3 exit path: restore
/// terminal, flush dirty saves, return an exit code. Keeping the
/// reason explicit (rather than collapsing both into a single bool)
/// lets the runtime distinguish "the player asked to quit" from "the
/// last screen popped itself" — useful for logging and for future
/// behaviours like "auto-save on Quit but not on EmptyStack".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// A screen returned [`ScreenCommand::Quit`].
    Quit,
    /// A [`ScreenCommand::Pop`] left the stack empty. SPEC §5.5 says
    /// the runtime treats this as a clean exit.
    EmptyStack,
}

/// Side effect produced by [`apply_command`] that the runtime must act
/// on *after* the stack has been mutated.
///
/// This is deliberately a plain enum — the reducer stays pure
/// (mutates only the stack it was handed) and the runtime is the one
/// that actually calls into the save manager, the message line, the
/// terminal guard, etc. That split is what makes 7b unit-testable
/// without any `ratatui`/`crossterm` plumbing.
///
/// `Push` / `Replace` are absent on purpose: they only mutate the
/// stack and have no out-of-band effect for the runtime to perform.
#[derive(Debug, PartialEq, Eq)]
pub enum SideEffect {
    /// Nothing for the runtime to do beyond rendering the (possibly
    /// updated) stack on the next frame.
    None,
    /// Runtime should ask the save manager to flush.
    Save,
    /// Runtime should surface a transient message to the player.
    Message(String),
    /// Runtime should surface an error to the player and/or operator
    /// log. Plain `String` to match [`ScreenCommand::Error`].
    Error(String),
    /// Runtime should leave the loop and run the SPEC §7.3 shutdown.
    Exit(ExitReason),
}

/// Pure reducer: apply a [`ScreenCommand`] to a [`ScreenStack`] and
/// return the resulting [`SideEffect`].
///
/// ## Why a reducer rather than a method on the runtime
///
/// SPEC §7.2 says the runtime "applies returned `ScreenCommand`
/// values" — but that step is entirely about manipulating the stack
/// and choosing the next action. Pulling it out of the runtime means:
///
/// - The mapping from command → stack change is exhaustively
///   specified by the type system (every variant is matched here).
/// - Tests can drive scripted command sequences against a `Vec` and
///   assert the final stack shape without mocking a terminal.
/// - The runtime in 7d is left with a thin job: poll → normalize →
///   dispatch → apply → render.
///
/// ## Behaviour matrix
///
/// | Command           | Stack effect                                   | SideEffect                  |
/// |-------------------|------------------------------------------------|-----------------------------|
/// | `None`            | none                                           | `None`                      |
/// | `Push(s)`         | push `s` on top                                | `None`                      |
/// | `Pop`             | pop top; empty afterwards → exit               | `None` or `Exit(EmptyStack)`|
/// | `Replace(s)`      | pop top (if any), then push `s`                | `None`                      |
/// | `Quit`            | clear the stack                                | `Exit(Quit)`                |
/// | `Save`            | none                                           | `Save`                      |
/// | `Message(s)`      | none                                           | `Message(s)`                |
/// | `Error(s)`        | none                                           | `Error(s)`                  |
///
/// Notes:
///
/// - `Pop` on an already-empty stack is treated identically to popping
///   the last screen: `Exit(EmptyStack)`. The runtime should never see
///   this in practice (it stops looping the moment the first
///   `EmptyStack` is reported), but defining the behaviour here keeps
///   the function total.
/// - `Replace` on an empty stack just pushes — no error. The runtime
///   only reaches this codepath if a screen returned `Replace` while
///   it was the top, and by then the stack is non-empty; the empty
///   branch is defined only so the reducer is total.
/// - `Quit` clears the stack so the runtime sees a consistent "no
///   more screens" state on its way out, matching the SPEC §5.5
///   description that `Quit` is "equivalent to popping every screen".
pub fn apply_command(stack: &mut ScreenStack, command: ScreenCommand) -> SideEffect {
    match command {
        ScreenCommand::None => SideEffect::None,
        ScreenCommand::Push(screen) => {
            stack.push(screen);
            SideEffect::None
        }
        ScreenCommand::Pop => {
            stack.pop();
            if stack.is_empty() {
                SideEffect::Exit(ExitReason::EmptyStack)
            } else {
                SideEffect::None
            }
        }
        ScreenCommand::Replace(screen) => {
            stack.pop();
            stack.push(screen);
            SideEffect::None
        }
        ScreenCommand::Quit => {
            stack.clear();
            SideEffect::Exit(ExitReason::Quit)
        }
        ScreenCommand::Save => SideEffect::Save,
        ScreenCommand::Message(s) => SideEffect::Message(s),
        ScreenCommand::Error(s) => SideEffect::Error(s),
    }
}

#[cfg(test)]
mod tests {
    //! Type-level coverage for 7a plus reducer coverage for 7b.
    //! Runtime wiring (event poll, render, terminal guard integration)
    //! is tested in 7d.

    use super::*;
    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
    use crate::input::Input;
    use ratatui::{backend::TestBackend, Terminal};

    /// Minimal `GameConfig` factory — keeps the tests focused on the
    /// `screen` module rather than reproducing config plumbing.
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

    /// A no-op screen used purely to verify the trait's default
    /// methods compile to the expected `ScreenCommand::None` values.
    struct NoopScreen;
    impl Screen for NoopScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
    }

    #[test]
    fn game_context_borrows_runtime_data() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let ctx = GameContext::new(&cfg, &fc, (100, 30));
        assert_eq!(ctx.config.game.slug, "test");
        assert_eq!(ctx.foglet.user_id.as_deref(), Some("test-user"));
        assert_eq!(ctx.terminal_size, (100, 30));
    }

    #[test]
    fn default_handle_input_is_none() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = NoopScreen;
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Enter),
            ScreenCommand::None
        ));
    }

    #[test]
    fn default_tick_is_none() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = NoopScreen;
        assert!(matches!(screen.tick(&mut ctx), ScreenCommand::None));
    }

    #[test]
    fn default_on_resize_is_none() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = NoopScreen;
        assert!(matches!(
            screen.on_resize(&mut ctx, 100, 40),
            ScreenCommand::None
        ));
    }

    #[test]
    fn screen_is_object_safe() {
        // If `Screen` were not object-safe this would fail to compile,
        // which would in turn break `Box<dyn Screen>` in
        // `ScreenCommand::Push`/`Replace`.
        let _boxed: Box<dyn Screen> = Box::new(NoopScreen);
    }

    #[test]
    fn screen_command_push_carries_screen() {
        let cmd = ScreenCommand::Push(Box::new(NoopScreen));
        match cmd {
            ScreenCommand::Push(_) => {}
            other => panic!("expected Push, got {other:?}"),
        }
    }

    #[test]
    fn screen_command_replace_carries_screen() {
        let cmd = ScreenCommand::Replace(Box::new(NoopScreen));
        assert!(matches!(cmd, ScreenCommand::Replace(_)));
    }

    #[test]
    fn screen_command_message_round_trips_payload() {
        let cmd = ScreenCommand::Message("hello".into());
        match cmd {
            ScreenCommand::Message(s) => assert_eq!(s, "hello"),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn screen_command_error_round_trips_payload() {
        let cmd = ScreenCommand::Error("boom".into());
        match cmd {
            ScreenCommand::Error(s) => assert_eq!(s, "boom"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn screen_command_debug_does_not_panic_for_trait_objects() {
        // Hand-rolled Debug impl: confirm Push/Replace render without
        // exploding (a derive would have failed to compile).
        let push = ScreenCommand::Push(Box::new(NoopScreen));
        let replace = ScreenCommand::Replace(Box::new(NoopScreen));
        assert_eq!(format!("{push:?}"), "Push(<dyn Screen>)");
        assert_eq!(format!("{replace:?}"), "Replace(<dyn Screen>)");
        assert_eq!(format!("{:?}", ScreenCommand::None), "None");
        assert_eq!(format!("{:?}", ScreenCommand::Pop), "Pop");
        assert_eq!(format!("{:?}", ScreenCommand::Quit), "Quit");
        assert_eq!(format!("{:?}", ScreenCommand::Save), "Save");
    }

    /// A render-counting screen used to prove the trait wires through
    /// to a real `ratatui::Frame` against a `TestBackend` — i.e. the
    /// signature isn't only theoretically correct.
    struct CountingScreen {
        renders: u32,
    }
    impl Screen for CountingScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {
            self.renders += 1;
        }
    }

    /// A screen that records every input it receives — tagged with its
    /// own identity into a shared log — and returns a pre-programmed
    /// [`ScreenCommand`] from each `handle_input` call. Reducer tests
    /// use the shared log to assert dispatch order without needing to
    /// downcast trait objects.
    struct ScriptedScreen {
        tag: &'static str,
        /// Shared dispatch log: each entry is the `tag` of the screen
        /// whose `handle_input` was invoked. `Rc<RefCell<...>>` keeps
        /// the test single-threaded and ergonomic; production code
        /// never sees this.
        log: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
        /// Queue of commands to return from `handle_input`. If
        /// exhausted, `handle_input` returns [`ScreenCommand::None`] so
        /// the test can keep dispatching without panicking.
        script: std::collections::VecDeque<ScreenCommand>,
    }

    impl ScriptedScreen {
        fn new(
            tag: &'static str,
            log: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
            script: Vec<ScreenCommand>,
        ) -> Self {
            Self {
                tag,
                log,
                script: script.into(),
            }
        }
    }

    impl Screen for ScriptedScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
        fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
            self.log.borrow_mut().push(self.tag);
            self.script.pop_front().unwrap_or(ScreenCommand::None)
        }
    }

    /// Run a scripted dispatch loop: for each input, call the top
    /// screen's `handle_input` and apply the returned command to the
    /// stack. Returns the side effects produced. Dispatch order is
    /// recovered from the shared log on the [`ScriptedScreen`]s. This
    /// is the closest unit-level analogue to the runtime loop without
    /// any terminal coupling.
    fn scripted_run(
        stack: &mut ScreenStack,
        ctx: &mut GameContext<'_>,
        inputs: &[Input],
    ) -> Vec<SideEffect> {
        let mut effects = Vec::new();
        for input in inputs {
            let cmd = match stack.last_mut() {
                Some(top) => top.handle_input(ctx, *input),
                None => break,
            };
            let effect = apply_command(stack, cmd);
            let exited = matches!(effect, SideEffect::Exit(_));
            effects.push(effect);
            if exited {
                break;
            }
        }
        effects
    }

    #[test]
    fn apply_none_is_noop() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::None);
        assert_eq!(effect, SideEffect::None);
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn apply_push_grows_stack() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Push(Box::new(NoopScreen)));
        assert_eq!(effect, SideEffect::None);
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn apply_pop_shrinks_stack() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen), Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Pop);
        assert_eq!(effect, SideEffect::None);
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn apply_pop_last_screen_signals_empty_stack_exit() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Pop);
        assert_eq!(effect, SideEffect::Exit(ExitReason::EmptyStack));
        assert!(stack.is_empty());
    }

    #[test]
    fn apply_pop_on_empty_stack_signals_empty_stack_exit() {
        let mut stack: ScreenStack = Vec::new();
        let effect = apply_command(&mut stack, ScreenCommand::Pop);
        assert_eq!(effect, SideEffect::Exit(ExitReason::EmptyStack));
        assert!(stack.is_empty());
    }

    #[test]
    fn apply_replace_swaps_top() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen), Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Replace(Box::new(NoopScreen)));
        assert_eq!(effect, SideEffect::None);
        // Replace pops + pushes — length unchanged.
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn apply_replace_on_empty_just_pushes() {
        // Defined behaviour even though the runtime never reaches this
        // branch in practice (a screen has to be on top to return
        // `Replace`). Documented in the function's behaviour matrix.
        let mut stack: ScreenStack = Vec::new();
        let effect = apply_command(&mut stack, ScreenCommand::Replace(Box::new(NoopScreen)));
        assert_eq!(effect, SideEffect::None);
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn apply_quit_clears_stack_and_exits() {
        let mut stack: ScreenStack = vec![
            Box::new(NoopScreen),
            Box::new(NoopScreen),
            Box::new(NoopScreen),
        ];
        let effect = apply_command(&mut stack, ScreenCommand::Quit);
        assert_eq!(effect, SideEffect::Exit(ExitReason::Quit));
        assert!(stack.is_empty());
    }

    #[test]
    fn apply_save_leaves_stack_alone() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Save);
        assert_eq!(effect, SideEffect::Save);
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn apply_message_passes_through() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Message("hi".into()));
        assert_eq!(effect, SideEffect::Message("hi".into()));
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn apply_error_passes_through() {
        let mut stack: ScreenStack = vec![Box::new(NoopScreen)];
        let effect = apply_command(&mut stack, ScreenCommand::Error("boom".into()));
        assert_eq!(effect, SideEffect::Error("boom".into()));
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn scripted_input_drives_push_then_pop_then_quit() {
        // Title screen pushes a Menu on Enter; Menu pops on Esc; the
        // exposed Title then quits on 'q'. This is the canonical round
        // trip for the reducer wiring 7d will rely on.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let menu = ScriptedScreen::new("menu", log.clone(), vec![ScreenCommand::Pop]);
        let title = ScriptedScreen::new(
            "title",
            log.clone(),
            vec![ScreenCommand::Push(Box::new(menu)), ScreenCommand::Quit],
        );
        let mut stack: ScreenStack = vec![Box::new(title)];

        let inputs = [Input::Enter, Input::Esc, Input::Char('q')];
        let effects = scripted_run(&mut stack, &mut ctx, &inputs);

        // First input dispatches to title (push menu); second to menu
        // (pop back to title); third to title again (quit).
        assert_eq!(log.borrow().clone(), vec!["title", "menu", "title"]);
        assert_eq!(
            effects,
            vec![
                SideEffect::None,
                SideEffect::None,
                SideEffect::Exit(ExitReason::Quit),
            ]
        );
        assert!(stack.is_empty(), "Quit clears the stack");
    }

    #[test]
    fn scripted_save_does_not_disturb_stack_or_top() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::<&'static str>::new()));
        let s = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![ScreenCommand::Save, ScreenCommand::Message("ok".into())],
        );
        let mut stack: ScreenStack = vec![Box::new(s)];
        let effects = scripted_run(&mut stack, &mut ctx, &[Input::Enter, Input::Enter]);

        assert_eq!(log.borrow().clone(), vec!["only", "only"]);
        assert_eq!(
            effects,
            vec![SideEffect::Save, SideEffect::Message("ok".into())]
        );
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn render_runs_against_test_backend() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (40, 10));
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("test backend");
        let mut screen = CountingScreen { renders: 0 };
        terminal
            .draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        assert_eq!(screen.renders, 1);
    }
}
