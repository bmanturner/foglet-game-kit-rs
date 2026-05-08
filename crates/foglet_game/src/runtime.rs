//! Top-level [`Game`] builder and runtime entry point per SPEC §8.1.
//!
//! Task 7c ships the **builder shape and validation** that authors will
//! call from their `main.rs`:
//!
//! ```no_run
//! use foglet_game::{Game, GameResult, SavePolicy, Screen, ScreenCommand, GameContext, Input};
//!
//! struct TitleScreen;
//! impl Screen for TitleScreen {
//!     fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
//! }
//!
//! fn main() -> GameResult<()> {
//!     Game::new("Murder Motel")
//!         .min_size(80, 24)
//!         .save_policy(SavePolicy::PerFogletUser)
//!         .push_screen(Box::new(TitleScreen))
//!         .run()
//! }
//! ```
//!
//! The runtime *loop* (poll events → normalize input → dispatch →
//! `apply_command` → render → tick) is Task 7d's responsibility. To keep
//! the split clean and testable, this module exposes:
//!
//! - [`Game`] — the fluent builder.
//! - [`Game::build`] — pure validation that returns a [`BuiltGame`]
//!   value. Tests can drive every validation branch without ever
//!   touching a terminal.
//! - [`Game::run`] — the public entry point SPEC §8.1 advertises. Today
//!   it calls `build()` and then delegates to [`run_built`], whose body
//!   is a documented stub waiting for 7d. When 7d lands the body of
//!   `run_built` is replaced; the public surface here does not change.
//!
//! ## Why split `build` from `run`
//!
//! Validation (no empty title, at least one starting screen, sane min
//! size) is pure and trivially unit-testable. The runtime loop is
//! decidedly not — it pulls in the terminal guard, a `crossterm` event
//! source, and `ratatui` rendering. Splitting them lets 7c land with
//! full coverage of the contract authors actually rely on (the builder
//! API surface) without faking an event source just to assert "the
//! builder rejects an empty screen list".

use crate::screen::{Screen, ScreenStack};

/// Where save files should be written. Mirrors the policy options in
/// SPEC §12 and the `[save] strategy` field from SPEC §9.1.
///
/// Kept as a small enum (rather than e.g. a `PathBuf`) so the runtime
/// can resolve the *concrete* directory at startup using the loaded
/// [`crate::FogletContext`]. Directly handing in a path here would let
/// authors hard-code per-host assumptions, which SPEC §12 explicitly
/// forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePolicy {
    /// Default: `<install_dir>/saves/<FOGLET_USER_ID>/save.json` in
    /// production, `.fgk/saves/local-dev/save.json` for local-dev (per
    /// SPEC §12). The runtime resolves which root applies based on the
    /// context source — authors do not pick.
    PerFogletUser,
    /// Disable on-disk saves entirely. Useful for screens-only demos
    /// and the eventual `fgk new` smoke harness, neither of which has
    /// durable state worth persisting.
    None,
}

impl Default for SavePolicy {
    /// Mirrors SPEC §9.1's default `strategy = "per_foglet_user"`. An
    /// author who does not call [`Game::save_policy`] gets the
    /// production-shaped behaviour.
    fn default() -> Self {
        Self::PerFogletUser
    }
}

/// Errors produced by the [`Game`] builder and (eventually) the
/// runtime loop. Library-internal — uses `thiserror` per the PROMPT.md
/// "thiserror inside libraries" rule. The `fgk` binary and example
/// games can convert this into `anyhow::Error` at the process boundary.
#[derive(Debug, thiserror::Error)]
pub enum GameError {
    /// `Game::new("")` or any whitespace-only title. SPEC §5.2 lists
    /// `title` as a required `GameConfig` field; the runtime carries
    /// the same invariant for ad-hoc construction.
    #[error("game title must not be empty or whitespace")]
    EmptyTitle,

    /// `min_size(0, _)` or `min_size(_, 0)`. A zero-cell minimum would
    /// degenerate the SPEC §7.1 "size check" into a tautology and is
    /// almost certainly a typo.
    #[error("min_size must be at least 1x1, got {width}x{height}")]
    InvalidMinSize {
        /// Width passed to [`Game::min_size`].
        width: u16,
        /// Height passed to [`Game::min_size`].
        height: u16,
    },

    /// `run()` was called without a starting screen. The runtime needs
    /// a top-of-stack screen to dispatch the first input to; without
    /// one there is nothing to do.
    #[error("at least one screen must be pushed before run()")]
    NoStartingScreen,

    /// Terminal guard reported a setup or teardown failure during the
    /// runtime loop. Wrapping this here (rather than re-exporting
    /// [`crate::TerminalError`]) keeps `?` at the runtime call site
    /// ergonomic.
    #[error(transparent)]
    Terminal(#[from] crate::terminal::TerminalError),
}

/// Convenience alias matching SPEC §8.1's `GameResult<T>`.
///
/// Re-exported from the crate root so authors only need to bring
/// `foglet_game::GameResult` into scope.
pub type GameResult<T> = std::result::Result<T, GameError>;

/// Fluent builder for a Foglet door game.
///
/// All methods take `self` by value and return `Self`, so the builder
/// chains naturally as in SPEC §8.1's example. `push_screen` may be
/// called multiple times — screens are pushed in call order, which
/// means the *last* `push_screen` becomes the top-of-stack the runtime
/// dispatches input to first. Authors typically push one screen and
/// let the screen logic itself drive further pushes via
/// [`crate::ScreenCommand::Push`].
///
/// `Debug` is hand-written (rather than derived) because `Box<dyn
/// Screen>` is not `Debug`. We print the count of pushed screens
/// instead, which is what tests and operators actually care about.
pub struct Game {
    /// Display title from [`Game::new`]. Validated on `build()`.
    title: String,
    /// Minimum supported terminal size. Defaults to the SPEC §2.2
    /// baseline of 80x24 when the author does not override it. The
    /// runtime checks the live size against this value *before*
    /// entering raw mode (SPEC §7.1 step 7).
    min_size: (u16, u16),
    /// Selected save policy; defaults to
    /// [`SavePolicy::PerFogletUser`].
    save_policy: SavePolicy,
    /// Screens accumulated in builder order. The last entry is the top
    /// of the stack the runtime renders/dispatches first.
    screens: ScreenStack,
}

impl std::fmt::Debug for Game {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Game")
            .field("title", &self.title)
            .field("min_size", &self.min_size)
            .field("save_policy", &self.save_policy)
            .field(
                "screens",
                &format_args!("[{} screen(s)]", self.screens.len()),
            )
            .finish()
    }
}

impl Game {
    /// Start a new builder with the given display title. Whitespace is
    /// preserved as written; trimming is the author's responsibility
    /// (mirroring how [`crate::GameConfig`] treats `title`).
    ///
    /// The title becomes the window/header label various screens
    /// surface to the player. Empty or whitespace-only titles are
    /// rejected at `build()` time so authors get a clear error
    /// regardless of whether they call `Game::new("")` directly or end
    /// up with an empty title via interpolation.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            min_size: (80, 24),
            save_policy: SavePolicy::default(),
            screens: ScreenStack::new(),
        }
    }

    /// Override the minimum supported terminal size. The runtime
    /// refuses to enter raw mode if the live terminal is smaller than
    /// this (SPEC §7.1).
    ///
    /// Defaults to `(80, 24)` per the SPEC §2.2 Foglet baseline. Most
    /// authors should leave this at the default; tightening it
    /// (e.g. `min_size(64, 22)`) is fine if the game has been
    /// explicitly designed for a cramped layout.
    #[must_use]
    pub fn min_size(mut self, width: u16, height: u16) -> Self {
        self.min_size = (width, height);
        self
    }

    /// Select where save files are written. See [`SavePolicy`].
    #[must_use]
    pub fn save_policy(mut self, policy: SavePolicy) -> Self {
        self.save_policy = policy;
        self
    }

    /// Push a screen onto the initial screen stack.
    ///
    /// Call order matters: the **last** `push_screen` sits on top and
    /// receives input first. The vast majority of games push exactly
    /// one screen (the title screen) and rely on
    /// [`crate::ScreenCommand::Push`] from inside that screen to
    /// transition further. Repeated calls are accepted because they
    /// make the testing API ergonomic, not because they are the
    /// expected authoring pattern.
    #[must_use]
    pub fn push_screen(mut self, screen: Box<dyn Screen>) -> Self {
        self.screens.push(screen);
        self
    }

    /// Validate the builder and produce a [`BuiltGame`] without
    /// entering the runtime loop.
    ///
    /// Exposed publicly so `fgk` and integration tests can assert that
    /// a configuration is well-formed before committing to a TUI
    /// session. The runtime path ([`Game::run`]) calls this internally.
    pub fn build(self) -> GameResult<BuiltGame> {
        if self.title.trim().is_empty() {
            return Err(GameError::EmptyTitle);
        }
        let (w, h) = self.min_size;
        if w == 0 || h == 0 {
            return Err(GameError::InvalidMinSize {
                width: w,
                height: h,
            });
        }
        if self.screens.is_empty() {
            return Err(GameError::NoStartingScreen);
        }
        Ok(BuiltGame {
            title: self.title,
            min_size: self.min_size,
            save_policy: self.save_policy,
            screens: self.screens,
        })
    }

    /// Run the game.
    ///
    /// SPEC §8.1's advertised entry point. Today this validates the
    /// builder via [`Game::build`] and then delegates to [`run_built`].
    /// The runtime loop body lands in Task 7d; until then `run_built`
    /// is a documented stub that returns `Ok(())` after validation
    /// succeeds, so the example in SPEC §8.1 type-checks and the public
    /// surface 7d will fill in is already nailed down.
    pub fn run(self) -> GameResult<()> {
        let built = self.build()?;
        run_built(built)
    }
}

/// A validated [`Game`] ready to be handed to the runtime loop.
///
/// Exposed because Task 7d will write a `runtime::run(BuiltGame)`
/// equivalent, and because `fgk` may want to inspect the validated
/// shape (e.g. to print the resolved `min_size` in a `--dry-run` mode)
/// without reimplementing validation.
///
/// Fields are `pub(crate)` for now: external callers should treat this
/// as opaque, but the runtime module needs to read them when 7d wires
/// the loop. Widen visibility deliberately if a real authoring need
/// shows up.
pub struct BuiltGame {
    pub(crate) title: String,
    pub(crate) min_size: (u16, u16),
    pub(crate) save_policy: SavePolicy,
    pub(crate) screens: ScreenStack,
}

impl std::fmt::Debug for BuiltGame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltGame")
            .field("title", &self.title)
            .field("min_size", &self.min_size)
            .field("save_policy", &self.save_policy)
            .field(
                "screens",
                &format_args!("[{} screen(s)]", self.screens.len()),
            )
            .finish()
    }
}

impl BuiltGame {
    /// Validated title (non-empty after trimming).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Validated minimum terminal size, in cells.
    pub fn min_size(&self) -> (u16, u16) {
        self.min_size
    }

    /// Resolved [`SavePolicy`].
    pub fn save_policy(&self) -> SavePolicy {
        self.save_policy
    }

    /// Number of screens initially pushed onto the stack. Useful for
    /// tests and `fgk` diagnostics; the actual screens are intentionally
    /// not exposed because exposing `Box<dyn Screen>` outside the
    /// runtime would invite shenanigans.
    pub fn screen_count(&self) -> usize {
        self.screens.len()
    }
}

/// Runtime entry point — Task 7d will fill in the body.
///
/// Today this is a deliberate stub: the builder API contract from
/// 7c is fully specified (and tested) without coupling to a terminal,
/// and 7d replaces this body with the actual loop wiring (terminal
/// guard, event poll, dispatch via [`crate::apply_command`], render).
/// Returning `Ok(())` here is honest given that no loop has been wired
/// yet — there is literally nothing for the runtime to fail at, because
/// there is no runtime.
///
/// Public so 7d's tests (and any future binary that wants to skip the
/// builder ceremony) can call it directly with a [`BuiltGame`].
pub fn run_built(_built: BuiltGame) -> GameResult<()> {
    // TODO(task-7d): poll terminal events → `Input::from_event` →
    // dispatch to `built.screens.last_mut()` → `apply_command` →
    // render via `ratatui::Terminal`. Until 7d lands, succeed
    // silently after validation so SPEC §8.1's example compiles.
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Builder validation coverage for 7c. The runtime loop itself is
    //! covered by 7d; the stub `run_built` is exercised here only to
    //! prove the call path from `Game::run` is wired.

    use super::*;
    use crate::screen::{GameContext, Screen};

    /// A do-nothing screen for builder tests. Render is required by
    /// the trait; nothing else needs to be implemented.
    struct DummyScreen;
    impl Screen for DummyScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
    }

    fn dummy() -> Box<dyn Screen> {
        Box::new(DummyScreen)
    }

    #[test]
    fn defaults_match_spec_baseline() {
        // Confirms the unstated-but-load-bearing defaults: 80x24 min
        // size and PerFogletUser save policy. Changing either default
        // is a SPEC §2.2 / §9.1 deviation worth catching here.
        let g = Game::new("Test");
        assert_eq!(g.min_size, (80, 24));
        assert_eq!(g.save_policy, SavePolicy::PerFogletUser);
    }

    #[test]
    fn save_policy_default_is_per_foglet_user() {
        assert_eq!(SavePolicy::default(), SavePolicy::PerFogletUser);
    }

    #[test]
    fn builder_chains_in_spec_8_1_order() {
        // The exact chain SPEC §8.1 advertises. If this stops compiling
        // the public API has drifted from the SPEC and 7c needs to
        // re-justify the change.
        let built = Game::new("Murder Motel")
            .min_size(80, 24)
            .save_policy(SavePolicy::PerFogletUser)
            .push_screen(dummy())
            .build()
            .expect("valid builder");
        assert_eq!(built.title(), "Murder Motel");
        assert_eq!(built.min_size(), (80, 24));
        assert_eq!(built.save_policy(), SavePolicy::PerFogletUser);
        assert_eq!(built.screen_count(), 1);
    }

    #[test]
    fn build_rejects_empty_title() {
        let err = Game::new("").push_screen(dummy()).build().unwrap_err();
        assert!(matches!(err, GameError::EmptyTitle));
    }

    #[test]
    fn build_rejects_whitespace_only_title() {
        // Empty after trimming is just as broken as literal "".
        let err = Game::new("   \t\n")
            .push_screen(dummy())
            .build()
            .unwrap_err();
        assert!(matches!(err, GameError::EmptyTitle));
    }

    #[test]
    fn build_rejects_zero_width_min_size() {
        let err = Game::new("T")
            .min_size(0, 24)
            .push_screen(dummy())
            .build()
            .unwrap_err();
        assert!(matches!(
            err,
            GameError::InvalidMinSize {
                width: 0,
                height: 24
            }
        ));
    }

    #[test]
    fn build_rejects_zero_height_min_size() {
        let err = Game::new("T")
            .min_size(80, 0)
            .push_screen(dummy())
            .build()
            .unwrap_err();
        assert!(matches!(
            err,
            GameError::InvalidMinSize {
                width: 80,
                height: 0
            }
        ));
    }

    #[test]
    fn build_rejects_no_screens() {
        let err = Game::new("T").build().unwrap_err();
        assert!(matches!(err, GameError::NoStartingScreen));
    }

    #[test]
    fn push_screen_preserves_order() {
        // The builder appends; tests assert that a multi-push results
        // in `screen_count` matching the number of pushes (the runtime
        // treats the last push as top-of-stack — covered in 7d).
        let built = Game::new("T")
            .push_screen(dummy())
            .push_screen(dummy())
            .push_screen(dummy())
            .build()
            .expect("valid");
        assert_eq!(built.screen_count(), 3);
    }

    #[test]
    fn min_size_overrides_default() {
        let built = Game::new("T")
            .min_size(64, 22)
            .push_screen(dummy())
            .build()
            .expect("valid");
        assert_eq!(built.min_size(), (64, 22));
    }

    #[test]
    fn save_policy_override_round_trips() {
        let built = Game::new("T")
            .save_policy(SavePolicy::None)
            .push_screen(dummy())
            .build()
            .expect("valid");
        assert_eq!(built.save_policy(), SavePolicy::None);
    }

    #[test]
    fn run_succeeds_for_valid_builder() {
        // Today `run_built` is a stub; this test pins the behaviour so
        // 7d's replacement does not silently regress to "always errors".
        // When 7d lands it should update this test to assert against
        // the scripted-input runtime path instead.
        let result = Game::new("T").push_screen(dummy()).run();
        assert!(result.is_ok(), "stubbed run should succeed: {result:?}");
    }

    #[test]
    fn run_propagates_validation_errors() {
        let result = Game::new("").push_screen(dummy()).run();
        assert!(matches!(result, Err(GameError::EmptyTitle)));
    }

    #[test]
    fn debug_for_game_does_not_leak_screen_internals() {
        // We deliberately avoid printing `Box<dyn Screen>` (it has no
        // useful Debug). Instead we print a count. Lock that in.
        let g = Game::new("Pretty")
            .push_screen(dummy())
            .push_screen(dummy());
        let s = format!("{g:?}");
        assert!(s.contains("title: \"Pretty\""), "got: {s}");
        assert!(s.contains("2 screen(s)"), "got: {s}");
    }

    #[test]
    fn game_error_is_send_and_sync() {
        // anyhow::Error wants Send + Sync at the process boundary.
        // Catch any future variant that breaks that contract.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GameError>();
    }
}
