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

#[cfg(test)]
mod tests {
    //! Type-level coverage for 7a. Stack operations and runtime wiring
    //! are tested in 7b/7d respectively.

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
