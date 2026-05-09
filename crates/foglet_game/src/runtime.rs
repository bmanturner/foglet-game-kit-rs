//! Top-level [`Game`] builder and runtime entry point per SPEC §8.1.
//!
//! Task 7d wires the runtime loop on top of the type machinery shipped in
//! 7a/7b/7c. The loop's responsibilities (SPEC §7.2) are:
//!
//! 1. Render the top screen.
//! 2. Poll a normalized [`Input`] from the event source (timing out at
//!    the tick interval so [`crate::Screen::tick`] still fires when the
//!    user is idle).
//! 3. Dispatch the input (or a tick) to the top screen, receive a
//!    [`crate::ScreenCommand`].
//! 4. Apply that command via [`crate::apply_command`]; act on the
//!    returned [`crate::SideEffect`] (save flush, exit).
//! 5. Loop.
//!
//! ## Why two entry points
//!
//! - [`Game::run`] / [`run_built`] is the *production* surface. It owns
//!   the [`TerminalGuard`] (raw mode + alt screen + panic hook), builds a
//!   `ratatui::Terminal` over `crossterm`, and wraps a [`CrosstermEventSource`].
//!   It is what authors call from `main.rs`.
//! - [`run_with_io`] is the *testable* surface. It takes already-built
//!   I/O dependencies (a `Terminal<B>`, an [`EventSource`]) so unit tests
//!   can inject a `ratatui::backend::TestBackend` plus a scripted event
//!   queue and assert the loop's behaviour without a TTY.
//!
//! Splitting them this way keeps the production wiring narrow (a few
//! lines of orchestration) while letting the loop body itself — the part
//! most likely to regress — live behind a fully-deterministic test
//! harness. SPEC §13.1 forbids us from running the live loop in CI; this
//! split is how we still get coverage.
//!
//! ## Tick policy
//!
//! Per SPEC §7.2 the tick interval is "implementation-defined". We hold
//! it at [`TICK_INTERVAL`] (50 ms, ~20 fps for animation/cooldowns).
//! Authors who need a different cadence today should override behaviour
//! inside `tick`; widening this to a builder knob waits for a real
//! authoring need.
//!
//! ## What the loop does **not** do (yet)
//!
//! - Save persistence: today we surface [`SideEffect::Save`] to a
//!   caller-supplied callback. Wiring it to the actual on-disk save
//!   manager is Task 8b.
//! - Message / error UI: [`SideEffect::Message`] and
//!   [`SideEffect::Error`] are routed to a no-op stub. The real status
//!   line lands with the widget primitives in Task 9c.

use std::time::{Duration, Instant};

use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::config::GameConfig;
use crate::foglet::FogletContext;
use crate::input::{self, Input};
use crate::screen::{apply_command, ExitReason, GameContext, Screen, ScreenStack, SideEffect};
use crate::terminal::{
    arm_panic_hook, install_panic_hook, CrosstermBackend as TermCrosstermBackend, TerminalGuard,
};
use crate::world_db::{WorldDb, WorldDbError, WorldDbOptions};

/// How long the runtime is willing to wait on the event source before
/// firing a [`Screen::tick`].
///
/// 50 ms ≈ 20 frames per second, which is the cadence the SPEC §9.4
/// example animations were sized against. Game logic that needs a
/// finer-grained timer should drive its own `Instant` arithmetic from
/// inside `tick` rather than ask the runtime to tick faster.
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

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

/// Errors produced by the [`Game`] builder and runtime loop.
///
/// Library-internal — uses `thiserror` per the PROMPT.md "thiserror
/// inside libraries" rule. The `fgk` binary and example games can
/// convert this into `anyhow::Error` at the process boundary.
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

    /// `run()` was called without a [`GameConfig`]. The runtime needs
    /// the parsed `assets/game.toml` to populate [`GameContext`] —
    /// authors must call [`Game::with_config`] before [`Game::run`].
    /// (The testable seam [`run_with_io`] takes the config as a
    /// parameter and so cannot hit this path.)
    #[error("Game::run() requires a GameConfig — call .with_config() before .run()")]
    MissingConfig,

    /// `run()` was called without a [`FogletContext`]. Same shape as
    /// [`GameError::MissingConfig`]: load via [`crate::load_context`]
    /// (or supply your own) and pass it through [`Game::with_foglet_context`].
    #[error("Game::run() requires a FogletContext — call .with_foglet_context() before .run()")]
    MissingContext,

    /// The configured terminal is smaller than the game's declared
    /// minimum (SPEC §7.1 step 7). The runtime refuses to enter raw
    /// mode rather than render a corrupted layout. Sizes are reported
    /// so the operator's error message is actionable.
    #[error(
        "terminal too small: have {actual_width}x{actual_height}, need at least {required_width}x{required_height}"
    )]
    TerminalTooSmall {
        /// Live terminal width in cells.
        actual_width: u16,
        /// Live terminal height in cells.
        actual_height: u16,
        /// Configured minimum width.
        required_width: u16,
        /// Configured minimum height.
        required_height: u16,
    },

    /// The event source returned an I/O error while polling or
    /// reading. Rendered as a string because the underlying source —
    /// [`crossterm`] in production, anything in tests — is generic.
    /// Authors typically surface this and exit non-zero per SPEC §7.3.
    #[error("event source I/O error: {0}")]
    EventIo(String),

    /// `ratatui::Terminal::draw` reported an error. Same string-based
    /// shape as [`GameError::EventIo`] for the same reason: the
    /// underlying backend is generic (CrosstermBackend in production,
    /// `TestBackend` in tests).
    #[error("render error: {0}")]
    Render(String),

    /// A save callback returned an error. The loop converts whatever
    /// the callback produced into this variant; the on-disk save
    /// manager itself (Task 8) carries its own error type and
    /// translates at the runtime boundary.
    #[error("save error: {0}")]
    Save(String),

    /// Terminal guard reported a setup or teardown failure during the
    /// runtime loop. Wrapping this here (rather than re-exporting
    /// [`crate::TerminalError`]) keeps `?` at the runtime call site
    /// ergonomic.
    #[error(transparent)]
    Terminal(#[from] crate::terminal::TerminalError),

    /// Opening or bootstrapping the shared-world SQLite database failed
    /// during startup. Wraps [`WorldDbError`] so the runtime can
    /// `?`-propagate the underlying world-db error without reaching for
    /// `anyhow` inside the library. Task 10c will arrange for this
    /// failure to surface *after* terminal restoration; today the open
    /// happens before the guard arms, so a clean error message reaches
    /// the operator without any raw-mode side effects.
    #[error(transparent)]
    WorldOpen(#[from] WorldDbError),
}

/// Convenience alias matching SPEC §8.1's `GameResult<T>`.
///
/// Re-exported from the crate root so authors only need to bring
/// `foglet_game::GameResult` into scope.
pub type GameResult<T> = std::result::Result<T, GameError>;

/// Boxed save closure type used by `Game::with_save_handler` (Task 4)
/// and produced by [`crate::save::SaveSlot::save_handler`] (Task 1e).
///
/// SPEC_v2_1 §4.4 pins this exact signature so the runtime, the
/// `SaveSlot` typed wrapper, and any author-written closure share one
/// vocabulary. The type lives here (rather than in `save.rs`) because
/// it references [`GameError`] — the failure mode the runtime turns
/// handler errors into via `GameError::Save(_)`.
///
/// `FnMut` (not `Fn`) so handlers may carry mutable bookkeeping —
/// e.g. a "writes-since-last-flush" counter — without resorting to
/// interior mutability. `'static` because the runtime stores the box
/// for the lifetime of the [`Game`] instance.
///
/// Defined eagerly in Task 1e so [`crate::save::SaveSlot::save_handler`]
/// has a concrete return type to point at; Task 4 wires the runtime
/// plumbing that consumes values of this type.
pub type SaveHandler = Box<dyn FnMut() -> Result<(), GameError>>;

/// Source of normalized [`Input`] events for the runtime loop.
///
/// Production uses [`CrosstermEventSource`] (which polls
/// `crossterm::event::poll` + `read` and pipes through
/// [`crate::input::from_event`]). Tests use a scripted queue. Either
/// way the loop never touches `crossterm` types directly — that is the
/// whole point of normalising in [`crate::input`].
///
/// ## Contract
///
/// - `next_input(timeout)` MUST block at most `timeout` and return
///   `Ok(None)` if the timeout elapsed without any input. The runtime
///   treats `Ok(None)` as the cue to fire a [`Screen::tick`].
/// - Returning `Ok(Some(_))` MUST consume exactly one input. The loop
///   processes one input per iteration; queueing two would let the
///   second skip rendering.
/// - `Err(...)` is treated as a fatal runtime error per SPEC §7.3.
pub trait EventSource {
    /// Wait up to `timeout` for the next normalized input.
    ///
    /// Returns:
    /// - `Ok(Some(input))` — input ready before the deadline.
    /// - `Ok(None)` — the timeout elapsed; runtime should tick.
    /// - `Err(_)` — fatal I/O error.
    fn next_input(&mut self, timeout: Duration) -> std::io::Result<Option<Input>>;
}

/// Production [`EventSource`] backed by `crossterm::event`.
///
/// Holds no state — every call reads fresh from the global event
/// queue. Constructed by [`run_built`] in the production wiring; tests
/// never use this directly because it requires a TTY.
pub struct CrosstermEventSource;

impl CrosstermEventSource {
    /// Construct the production event source. Does not touch the
    /// terminal until [`EventSource::next_input`] is called.
    pub fn new() -> Self {
        Self
    }
}

impl Default for CrosstermEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSource for CrosstermEventSource {
    fn next_input(&mut self, timeout: Duration) -> std::io::Result<Option<Input>> {
        // `event::poll` blocks up to `timeout` waiting for any event.
        // It returns Ok(false) on timeout — that's our "fire a tick"
        // signal. On Ok(true) the next `event::read()` call is
        // guaranteed not to block.
        if crossterm::event::poll(timeout)? {
            let ev = crossterm::event::read()?;
            Ok(Some(input::from_event(ev)))
        } else {
            Ok(None)
        }
    }
}

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
    /// Optional parsed `assets/game.toml`. Required by [`Game::run`]
    /// (so the runtime can populate [`GameContext::config`]) but left
    /// optional on the builder so tests that only exercise validation
    /// don't need to construct one.
    config: Option<GameConfig>,
    /// Optional Foglet context. Same shape as `config`: required by
    /// [`Game::run`], optional on the builder.
    foglet: Option<FogletContext>,
    /// Optional save handler installed via [`Game::with_save_handler`]
    /// (Task 4b). The runtime invokes this on every
    /// [`crate::ScreenCommand::Save`] emission and once on the Quit
    /// drain (Tasks 4c–4d). Defaults to `None` so existing v2 games
    /// keep their stub-callback behaviour until the author opts in.
    ///
    /// Stored as `Option<SaveHandler>` rather than `SaveHandler` so a
    /// game built without persistence stays cheap (no allocation for
    /// an unused boxed closure) and so `with_save_handler` can be a
    /// "set or replace" rather than a "stack" — calling it twice
    /// overwrites, matching the SPEC_v2_1 §4.4 contract that exactly
    /// one handler is in scope at runtime.
    save_handler: Option<SaveHandler>,
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
            .field("config", &self.config.is_some())
            .field("foglet", &self.foglet.is_some())
            .field("save_handler", &self.save_handler.is_some())
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
            config: None,
            foglet: None,
            save_handler: None,
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

    /// Attach the parsed `assets/game.toml`. Required for [`Game::run`].
    ///
    /// Kept as a separate setter (rather than an argument to
    /// [`Game::new`]) so screens-only validation tests can build a
    /// `Game` without standing up a config fixture.
    #[must_use]
    pub fn with_config(mut self, config: GameConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Attach the loaded Foglet context. Required for [`Game::run`].
    ///
    /// Authors typically call [`crate::load_context`] in `main()` and
    /// thread the result here. Bypassing the loader (e.g. constructing
    /// a [`FogletContext`] from internal sources) is fine for tests
    /// and embedded usage but goes against SPEC §2.4's "trust only
    /// `FOGLET_DOOR_CONTEXT`" guidance for production doors.
    #[must_use]
    pub fn with_foglet_context(mut self, foglet: FogletContext) -> Self {
        self.foglet = Some(foglet);
        self
    }

    /// Install a save handler that the runtime will invoke whenever a
    /// screen emits [`crate::ScreenCommand::Save`] and once on the
    /// Quit drain (SPEC_v2_1 §4.4 / Tasks 4c–4d).
    ///
    /// The canonical producer is [`crate::SaveSlot::save_handler`]:
    ///
    /// ```ignore
    /// let slot: SaveSlot<MyState> = SaveSlot::load_or_default(&path)?;
    /// Game::new("…")
    ///     .with_config(cfg)
    ///     .with_foglet_context(ctx)
    ///     .with_save_handler(slot.save_handler(path.clone()))
    ///     .push_screen(Box::new(TitleScreen::new(slot.handle())))
    ///     .run()?;
    /// ```
    ///
    /// Authors can also write a closure by hand — anything that is
    /// `FnMut() -> Result<(), GameError> + 'static` is accepted, so a
    /// game persisting to e.g. an HTTP endpoint can wire its own
    /// effect here without going through `SaveSlot`.
    ///
    /// ## Replacement, not stacking
    ///
    /// Calling `with_save_handler` twice replaces the previous
    /// handler. SPEC_v2_1 §4.4 pins this: the runtime owns exactly
    /// one save effect at a time so the Quit drain can call it
    /// idempotently. If a game needs multiple persistence effects,
    /// the author composes them inside a single closure (e.g. write
    /// the JSON save *and* publish a metric) — the runtime stays
    /// agnostic to that fan-out.
    ///
    /// ## Contract
    ///
    /// - Handlers run **inside** the [`crate::TerminalGuard`]'s
    ///   lifetime. They MUST NOT print to stdout (it would corrupt
    ///   the alternate screen). Log to a file appender or
    ///   `tracing`'s no-op subscriber instead.
    /// - Returning `Err(_)` aborts the runtime with
    ///   [`GameError::Save`]; terminal restoration still runs because
    ///   the loop result threads back through the guard's `cleanup()`
    ///   call. (See [`run_built_with_opener`].)
    /// - The closure may carry mutable state (`FnMut`) — useful for a
    ///   "writes since last flush" counter or for retrying a transient
    ///   I/O error inside the handler before surfacing it.
    #[must_use]
    pub fn with_save_handler<F>(mut self, handler: F) -> Self
    where
        F: FnMut() -> Result<(), GameError> + 'static,
    {
        self.save_handler = Some(Box::new(handler));
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
            config: self.config,
            foglet: self.foglet,
            save_handler: self.save_handler,
        })
    }

    /// Run the game.
    ///
    /// SPEC §8.1's advertised entry point. Validates the builder via
    /// [`Game::build`], then delegates to [`run_built`] for the
    /// production wiring (terminal guard, ratatui terminal, crossterm
    /// event source, runtime loop).
    pub fn run(self) -> GameResult<()> {
        let built = self.build()?;
        run_built(built)
    }
}

/// A validated [`Game`] ready to be handed to the runtime loop.
///
/// Exposed so `fgk` can inspect the validated shape (e.g. to print the
/// resolved `min_size` in a `--dry-run` mode) without reimplementing
/// validation, and so [`run_built`] / [`run_with_io`] can take an
/// already-validated value rather than re-running checks.
///
/// Fields are `pub(crate)` for now: external callers should treat this
/// as opaque, but the runtime module needs to read them. Widen
/// visibility deliberately if a real authoring need shows up.
pub struct BuiltGame {
    pub(crate) title: String,
    pub(crate) min_size: (u16, u16),
    pub(crate) save_policy: SavePolicy,
    pub(crate) screens: ScreenStack,
    pub(crate) config: Option<GameConfig>,
    pub(crate) foglet: Option<FogletContext>,
    /// Save handler threaded through from [`Game::with_save_handler`]
    /// (Task 4b). [`run_built_with_opener`] takes this out of the
    /// validated game and feeds it to [`run_with_io`] as `on_save`,
    /// replacing the v2 stub. `None` means the author opted out of
    /// persistence — the loop falls back to a no-op closure so a
    /// stray [`crate::ScreenCommand::Save`] is silently ignored
    /// rather than aborting the run.
    pub(crate) save_handler: Option<SaveHandler>,
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
            .field("config", &self.config.is_some())
            .field("foglet", &self.foglet.is_some())
            .field("save_handler", &self.save_handler.is_some())
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

/// Open the shared-world SQLite DB for this run, *only* when the
/// loaded config opted in via `[world].enabled = true`.
///
/// This is the Task 10b seam. Splitting it out from [`run_built`] gives
/// us:
///
/// - a unit-testable function that exercises both branches (enabled
///   vs. disabled) without spinning up a TUI, and
/// - a clean place for Task 10c to swap in an injectable opener so a
///   simulated DB-open failure can verify the post-guard error path
///   restores the terminal first.
///
/// Returns `Ok(None)` when the world layer is disabled or the section
/// is missing entirely (`WorldSection::default().enabled == false`),
/// so a v1 game with no `[world]` block keeps booting unchanged.
///
/// Errors propagate as [`GameError::WorldOpen`] — see SPEC §5.1
/// "clear-error" path: the runtime never silently degrades a
/// world-enabled config to a no-op.
pub fn open_world_db_if_enabled(config: &GameConfig) -> GameResult<Option<WorldDb>> {
    if !config.world.enabled {
        return Ok(None);
    }
    let options = WorldDbOptions::from(&config.world);
    let db = WorldDb::open_with_options(&config.world.path, options)?;
    Ok(Some(db))
}

/// Run the validated game against production I/O — terminal guard, a
/// ratatui terminal over crossterm, and the crossterm event source.
///
/// SPEC §7.1 startup ordering enforced here:
///
/// 1. Verify config + foglet context are present.
/// 2. Determine live terminal size (via `crossterm::terminal::size`).
/// 3. Check live size ≥ configured `min_size`. If not, error *before*
///    entering raw mode so the operator sees a clean message.
/// 4. Construct the [`TerminalGuard`] (which arms the panic hook).
/// 5. Build the ratatui terminal + event source and call
///    [`run_with_io`].
/// 6. On the way out — success or error — call `guard.cleanup()` so
///    teardown errors are observed (Drop is the safety net but does
///    not surface errors).
///
/// Save persistence is currently a no-op stub: when the loop emits
/// [`SideEffect::Save`] we ignore it. Task 8 wires the real save
/// manager in. The signature change will be a breaking one (the stub
/// here returns `Ok(())` whereas the real path returns the save
/// manager's result) but isolating it now keeps Task 7d focused on
/// the loop wiring.
pub fn run_built(built: BuiltGame) -> GameResult<()> {
    // Production wiring uses the canonical opener. The injectable
    // [`run_built_with_opener`] variant exists so Task 10c can
    // simulate a DB-open failure under test without standing up a
    // real broken SQLite path.
    run_built_with_opener(built, &open_world_db_if_enabled)
}

/// Type alias for the world-DB opener seam consumed by
/// [`run_built_with_opener`].
///
/// A `&dyn Fn(...)` (rather than a generic) keeps the runtime API
/// monomorphic — the opener only needs to be invoked once and the
/// indirection cost is irrelevant against terminal I/O. Tests box a
/// closure that returns a synthetic [`GameError::WorldOpen`] to
/// exercise the failure ordering described by SPEC §7.1 / Task 10c.
pub type WorldOpenerFn = dyn Fn(&GameConfig) -> GameResult<Option<WorldDb>>;

/// Like [`run_built`] but with the world-DB opener supplied by the
/// caller.
///
/// Task 10c: locks in the SPEC §7.1 invariant that DB-open MUST run
/// **before** any terminal raw-mode toggle, so a DB-open failure
/// trivially leaves the terminal in its original state — there is
/// nothing to restore because nothing was changed. The injectable
/// opener lets a unit test simulate a failing opener and assert both
/// the propagated [`GameError::WorldOpen`] and that raw mode was
/// never engaged, without needing a broken SQLite path on disk.
///
/// Production callers should use [`run_built`]; this variant is
/// public so future work (e.g. wiring a process-wide opener override
/// for staging environments) can plug in without re-implementing the
/// SPEC §7.1 startup sequence.
pub fn run_built_with_opener(mut built: BuiltGame, open_world: &WorldOpenerFn) -> GameResult<()> {
    // Required deps — checked before touching the terminal so a
    // misconfigured author gets a clear-error exit with no terminal
    // state changed. We `take()` the optionals out of `built` so the
    // remaining `BuiltGame` can be moved into `run_with_io` while the
    // borrowed config + foglet outlive the call.
    let config = built.config.take().ok_or(GameError::MissingConfig)?;
    let foglet = built.foglet.take().ok_or(GameError::MissingContext)?;

    // Open the shared-world DB *before* engaging the terminal guard.
    // SPEC §7.1 / Task 10c: this ordering is the load-bearing
    // invariant — a DB-open failure here returns `Err` while the
    // terminal is still in its untouched, cooked-mode state, so no
    // restoration is required and any operator-facing error message
    // prints to a normal terminal. Moving this call after the guard
    // is constructed would re-introduce the "error message lost
    // inside the alternate screen" hazard that SPEC §7.3 explicitly
    // forbids. The opener is injected so tests can substitute a
    // failing implementation without needing a real broken path.
    let world_db = open_world(&config)?;

    // Live terminal size. crossterm::terminal::size works on a TTY
    // before raw mode is engaged; using it here keeps the size check
    // ahead of the guard per SPEC §7.1 step 7.
    let (w, h) = crossterm::terminal::size().map_err(|e| GameError::EventIo(e.to_string()))?;
    let (mw, mh) = built.min_size;
    if w < mw || h < mh {
        return Err(GameError::TerminalTooSmall {
            actual_width: w,
            actual_height: h,
            required_width: mw,
            required_height: mh,
        });
    }

    // Install the panic hook *before* the guard arms so a panic during
    // guard construction itself still has a restorer registered. The
    // guard's own `new()` does this internally, but doing it explicitly
    // here documents the SPEC §7.1 ordering.
    install_panic_hook();
    let mut guard: TerminalGuard<TermCrosstermBackend> = TerminalGuard::new()?;
    arm_panic_hook();

    // Take ownership of stdout for ratatui — ratatui's CrosstermBackend
    // wraps a writer; we use a fresh handle rather than reusing the
    // guard's so the guard's draining flush at teardown does not race
    // ratatui's own buffered writes.
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| GameError::Render(e.to_string()))?;
    let mut events = CrosstermEventSource::new();

    // Save sink: prefer the handler installed via
    // [`Game::with_save_handler`] (Task 4b). When the author hasn't
    // installed one, fall back to a no-op so a screen that emits
    // [`crate::ScreenCommand::Save`] without configured persistence
    // does not abort the run — SPEC_v2_1 §4.4 explicitly allows games
    // to opt out of save by simply never calling `with_save_handler`.
    // We `take()` the handler out of `built` so `run_with_io` can move
    // `built` while the closure lives outside that ownership.
    let mut on_save: SaveHandler = built
        .save_handler
        .take()
        .unwrap_or_else(|| Box::new(|| Ok(())));

    // Run the loop. We propagate the result through `cleanup_on` so a
    // failure inside the loop still observes teardown errors before
    // returning.
    let result = run_with_io(
        built,
        &config,
        &foglet,
        world_db.as_ref(),
        (w, h),
        &mut terminal,
        &mut events,
        &mut on_save,
    );

    // Explicit cleanup so SPEC §7.3 "controlled error" messages can
    // print *after* terminal restoration. Drop is still the safety
    // net — we don't propagate cleanup errors past the loop's own
    // result, but we do log via the file appender once Task 14c lands.
    let _ = guard.cleanup();

    result.map(|_reason| ())
}

/// Drive the runtime loop against caller-supplied I/O.
///
/// This is the testable seam. All the orchestration lives here; the
/// production wiring in [`run_built`] is just "build deps + call
/// `run_with_io` + cleanup".
///
/// The returned [`ExitReason`] tells the caller *why* the loop ended
/// — useful for both production logging and test assertions. SPEC
/// §7.3 distinguishes "the player asked to quit" from "the last screen
/// popped itself"; preserving the distinction here lets future
/// behaviours like "auto-save on Quit but not on EmptyStack" land
/// without a signature change.
///
/// ## Behaviour summary
///
/// - **Render** the top screen each iteration *before* polling for
///   input. Rendering first means the player sees the new state
///   immediately after a transition rather than after the next event.
/// - **Poll** with a deadline of [`TICK_INTERVAL`] - elapsed-since-tick.
///   `Ok(None)` from the source means the deadline elapsed → fire
///   `tick` and reset the deadline.
/// - **Resize** events update the cached `terminal_size` *before*
///   dispatching to the screen, so `on_resize` and the next render
///   both see the new dimensions.
/// - **Side effects**: `Save` calls the `on_save` callback; `Message`
///   and `Error` are currently swallowed (the status-line widget lives
///   in Task 9c); `Exit(reason)` returns from the loop.
#[allow(clippy::too_many_arguments)]
pub fn run_with_io<B, E>(
    mut built: BuiltGame,
    config: &GameConfig,
    foglet: &FogletContext,
    world_db: Option<&WorldDb>,
    initial_size: (u16, u16),
    terminal: &mut Terminal<B>,
    events: &mut E,
    on_save: &mut dyn FnMut() -> Result<(), GameError>,
) -> GameResult<ExitReason>
where
    B: Backend,
    E: EventSource,
{
    // Defensive: `BuiltGame` invariant says the stack is non-empty,
    // but we re-check here so a future BuiltGame that allows zero
    // screens cannot accidentally drive the loop into a `last_mut()
    // == None` panic on the very first render.
    if built.screens.is_empty() {
        return Err(GameError::NoStartingScreen);
    }

    let mut size = initial_size;
    // The next time a tick *should* fire. Updated to "now + interval"
    // every time we actually fire a tick. Holding a deadline (rather
    // than tracking elapsed-since-last-poll) is the natural way to
    // keep tick cadence steady when input arrives between ticks.
    let mut next_tick = Instant::now() + TICK_INTERVAL;

    loop {
        // ---- Render ----
        // Scoped borrow: `top` exclusively borrows `screens` only
        // long enough to draw. Once `terminal.draw` returns, the
        // borrow ends and the next match arm is free to mutate
        // `screens` via `apply_command`.
        {
            let top = built
                .screens
                .last_mut()
                .expect("non-empty: checked above and after every transition");
            let mut ctx = GameContext::new(config, foglet, size);
            if let Some(db) = world_db {
                ctx = ctx.with_world_db(db);
            }
            terminal
                .draw(|frame| top.render(&mut ctx, frame))
                .map_err(|e| GameError::Render(e.to_string()))?;
        }

        // ---- Poll ----
        let now = Instant::now();
        let timeout = next_tick.saturating_duration_since(now);

        let cmd = match events
            .next_input(timeout)
            .map_err(|e| GameError::EventIo(e.to_string()))?
        {
            Some(Input::Resize { width, height }) => {
                // Update the cached size *before* dispatching, so
                // both `on_resize` and the next render see the new
                // dimensions. The screen still gets the explicit
                // `width, height` arguments per SPEC §8.2.
                size = (width, height);
                let top = built.screens.last_mut().expect("non-empty");
                let mut ctx = GameContext::new(config, foglet, size);
                if let Some(db) = world_db {
                    ctx = ctx.with_world_db(db);
                }
                top.on_resize(&mut ctx, width, height)
            }
            Some(other) => {
                let top = built.screens.last_mut().expect("non-empty");
                let mut ctx = GameContext::new(config, foglet, size);
                if let Some(db) = world_db {
                    ctx = ctx.with_world_db(db);
                }
                top.handle_input(&mut ctx, other)
            }
            None => {
                // Tick. Reset the deadline *before* calling tick so a
                // long-running tick does not collapse subsequent
                // intervals to zero.
                next_tick = Instant::now() + TICK_INTERVAL;
                let top = built.screens.last_mut().expect("non-empty");
                let mut ctx = GameContext::new(config, foglet, size);
                if let Some(db) = world_db {
                    ctx = ctx.with_world_db(db);
                }
                top.tick(&mut ctx)
            }
        };

        // ---- Apply ----
        match apply_command(&mut built.screens, cmd) {
            SideEffect::None => {}
            SideEffect::Save => on_save()?,
            // Message / Error UI lives in Task 9c — for now the
            // status-line slot is unwired and these are swallowed.
            // Errors specifically are not promoted to GameError
            // because a screen-level error is screen-level by
            // construction; the runtime fails only on I/O / render
            // / save problems.
            SideEffect::Message(_) | SideEffect::Error(_) => {}
            SideEffect::Exit(reason) => {
                // Task 4d: fire `on_save` once on the clean-exit drain
                // path so an author who installed a handler via
                // [`Game::with_save_handler`] gets a final flush on
                // both `ExitReason::Quit` (explicit `ScreenCommand::Quit`)
                // and `ExitReason::EmptyStack` (the last screen popped
                // itself). SPEC_v2_1 §4.4 calls this out as the whole
                // point of having a runtime-level save handler — the
                // game shouldn't have to remember to flush on every
                // exit path. Errors short-circuit through `?` above
                // and skip this branch, so a `SideEffect::Save` that
                // fails earlier in this iteration never double-runs
                // here, and a render/IO error still leaves whatever
                // partial state the game last persisted intact rather
                // than half-overwriting it on a panicking exit.
                on_save()?;
                return Ok(reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Builder validation coverage for 7c plus runtime-loop coverage
    //! for 7d. The production `run_built` path is exercised here only
    //! through its validation prefix (missing config / context) — the
    //! full wiring requires a TTY and is covered by the manual-smoke
    //! recipe documented for SPEC §13.1.

    use super::*;
    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
    use crate::input::Input;
    use crate::screen::{GameContext, Screen, ScreenCommand};
    use crate::terminal::{TerminalBackend, TerminalError};
    use ratatui::backend::TestBackend;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// A do-nothing screen for builder tests. Render is required by
    /// the trait; nothing else needs to be implemented.
    struct DummyScreen;
    impl Screen for DummyScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {}
    }

    fn dummy() -> Box<dyn Screen> {
        Box::new(DummyScreen)
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

    // -------------------------------------------------------------
    // Builder validation (7c)
    // -------------------------------------------------------------

    #[test]
    fn defaults_match_spec_baseline() {
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
    fn with_config_threads_config_into_built_game() {
        // Coverage for the new 7d setter: the BuiltGame should carry
        // the config so `run_built` can hand it to the loop.
        let cfg = fixture_config();
        let built = Game::new("T")
            .push_screen(dummy())
            .with_config(cfg.clone())
            .build()
            .expect("valid");
        assert!(built.config.is_some(), "config must round-trip");
        assert_eq!(built.config.as_ref().unwrap().game.slug, cfg.game.slug);
    }

    #[test]
    fn with_foglet_context_threads_context_into_built_game() {
        let fc = fixture_context();
        let built = Game::new("T")
            .push_screen(dummy())
            .with_foglet_context(fc.clone())
            .build()
            .expect("valid");
        assert!(built.foglet.is_some(), "foglet must round-trip");
        assert_eq!(
            built.foglet.as_ref().unwrap().door_id,
            fc.door_id,
            "context round-trips by value"
        );
    }

    #[test]
    fn with_save_handler_stores_handler_on_builder() {
        // Task 4b: the builder must hold onto the handler so a later
        // task (4c/4d) can hand it to the runtime. We verify both that
        // the field flips from `None` to `Some` and that the closure
        // we passed in is the one stored — by invoking it and watching
        // the side-effect counter increment.
        let calls = Rc::new(RefCell::new(0u32));
        let calls_clone = calls.clone();
        let mut g = Game::new("T")
            .push_screen(dummy())
            .with_save_handler(move || {
                *calls_clone.borrow_mut() += 1;
                Ok(())
            });
        assert!(
            g.save_handler.is_some(),
            "with_save_handler must populate the field"
        );

        // Invoke the stored handler. `as_mut()` because `FnMut` needs
        // unique access; the runtime will do the same thing in 4c.
        let handler = g.save_handler.as_mut().expect("just set");
        handler().expect("handler ok");
        assert_eq!(*calls.borrow(), 1, "stored handler is the one we passed");
    }

    #[test]
    fn with_save_handler_replaces_rather_than_stacks() {
        // SPEC_v2_1 §4.4 contract: the runtime owns *one* save effect.
        // Calling `with_save_handler` twice must overwrite — invoking
        // the stored handler once after two installs should fire only
        // the second closure. (If we stacked, both would tick.)
        let first = Rc::new(RefCell::new(0u32));
        let second = Rc::new(RefCell::new(0u32));
        let first_c = first.clone();
        let second_c = second.clone();
        let mut g = Game::new("T")
            .push_screen(dummy())
            .with_save_handler(move || {
                *first_c.borrow_mut() += 1;
                Ok(())
            })
            .with_save_handler(move || {
                *second_c.borrow_mut() += 1;
                Ok(())
            });
        let handler = g.save_handler.as_mut().expect("just set");
        handler().expect("handler ok");
        assert_eq!(*first.borrow(), 0, "first handler must be dropped");
        assert_eq!(*second.borrow(), 1, "second handler is the live one");
    }

    #[test]
    fn debug_for_game_reports_save_handler_presence() {
        // The Debug impl flips `save_handler` between `false` and
        // `true` so operators eyeballing a panic dump can tell whether
        // a save effect is wired without leaking the closure itself.
        let without = format!("{:?}", Game::new("T").push_screen(dummy()));
        assert!(without.contains("save_handler: false"), "got: {without}");
        let with = format!(
            "{:?}",
            Game::new("T")
                .push_screen(dummy())
                .with_save_handler(|| Ok(()))
        );
        assert!(with.contains("save_handler: true"), "got: {with}");
    }

    #[test]
    fn run_built_rejects_missing_config() {
        // Without a config attached, `run_built` must error *before*
        // touching the terminal. We test by calling `run_built`
        // directly so we never reach the size-check / TerminalGuard
        // construction (which would require a TTY).
        let built = Game::new("T")
            .push_screen(dummy())
            .with_foglet_context(fixture_context())
            .build()
            .expect("valid");
        let err = run_built(built).unwrap_err();
        assert!(matches!(err, GameError::MissingConfig), "got {err:?}");
    }

    #[test]
    fn run_built_rejects_missing_context() {
        let built = Game::new("T")
            .push_screen(dummy())
            .with_config(fixture_config())
            .build()
            .expect("valid");
        let err = run_built(built).unwrap_err();
        assert!(matches!(err, GameError::MissingContext), "got {err:?}");
    }

    #[test]
    fn run_propagates_validation_errors() {
        let result = Game::new("").push_screen(dummy()).run();
        assert!(matches!(result, Err(GameError::EmptyTitle)));
    }

    #[test]
    fn debug_for_game_does_not_leak_screen_internals() {
        let g = Game::new("Pretty")
            .push_screen(dummy())
            .push_screen(dummy());
        let s = format!("{g:?}");
        assert!(s.contains("title: \"Pretty\""), "got: {s}");
        assert!(s.contains("2 screen(s)"), "got: {s}");
    }

    #[test]
    fn game_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GameError>();
    }

    // -------------------------------------------------------------
    // Runtime loop (7d)
    // -------------------------------------------------------------

    /// Scripted [`EventSource`] for tests.
    ///
    /// Each entry in the queue is one call to `next_input`. `Some(input)`
    /// hands that input back; `None` simulates a tick timeout. The
    /// requested `timeout` is ignored — we want fully deterministic
    /// dispatch order regardless of wall-clock timing.
    ///
    /// Panics if exhausted, which is a deliberate test-only invariant:
    /// a runaway loop should crash the test rather than hang.
    struct VecEventSource {
        queue: VecDeque<Option<Input>>,
    }

    impl VecEventSource {
        fn new(items: Vec<Option<Input>>) -> Self {
            Self {
                queue: items.into(),
            }
        }
    }

    impl EventSource for VecEventSource {
        fn next_input(&mut self, _timeout: Duration) -> std::io::Result<Option<Input>> {
            Ok(self
                .queue
                .pop_front()
                .expect("VecEventSource exhausted — test scripted too few events"))
        }
    }

    /// A screen that returns a pre-programmed sequence of commands and
    /// records every call into a shared log. Used to drive the loop
    /// through specific transitions and verify dispatch order.
    struct ScriptedScreen {
        tag: &'static str,
        log: Rc<RefCell<Vec<String>>>,
        on_input: VecDeque<ScreenCommand>,
        on_tick: VecDeque<ScreenCommand>,
        on_resize: VecDeque<ScreenCommand>,
    }

    impl ScriptedScreen {
        fn new(
            tag: &'static str,
            log: Rc<RefCell<Vec<String>>>,
            inputs: Vec<ScreenCommand>,
            ticks: Vec<ScreenCommand>,
            resizes: Vec<ScreenCommand>,
        ) -> Self {
            Self {
                tag,
                log,
                on_input: inputs.into(),
                on_tick: ticks.into(),
                on_resize: resizes.into(),
            }
        }
    }

    impl Screen for ScriptedScreen {
        fn render(&mut self, _ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {
            self.log.borrow_mut().push(format!("{}:render", self.tag));
        }

        fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
            self.log
                .borrow_mut()
                .push(format!("{}:input:{input:?}", self.tag));
            self.on_input.pop_front().unwrap_or(ScreenCommand::None)
        }

        fn tick(&mut self, ctx: &mut GameContext<'_>) -> ScreenCommand {
            self.log.borrow_mut().push(format!(
                "{}:tick:{}x{}",
                self.tag, ctx.terminal_size.0, ctx.terminal_size.1
            ));
            self.on_tick.pop_front().unwrap_or(ScreenCommand::None)
        }

        fn on_resize(
            &mut self,
            _ctx: &mut GameContext<'_>,
            width: u16,
            height: u16,
        ) -> ScreenCommand {
            self.log
                .borrow_mut()
                .push(format!("{}:resize:{width}x{height}", self.tag));
            self.on_resize.pop_front().unwrap_or(ScreenCommand::None)
        }
    }

    fn make_built(screen: Box<dyn Screen>) -> BuiltGame {
        Game::new("Test")
            .push_screen(screen)
            .build()
            .expect("valid builder")
    }

    fn make_terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(40, 10)).expect("test backend")
    }

    #[test]
    fn loop_dispatches_input_and_exits_on_quit() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "title",
            log.clone(),
            vec![ScreenCommand::Quit],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('q'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        let log = log.borrow();
        // Render → input(Char('q')) → Quit closes the loop. We render
        // once before the first poll and never again because Quit
        // returns immediately.
        assert_eq!(*log, vec!["title:render", "title:input:Char('q')"]);
    }

    #[test]
    fn loop_pop_to_empty_signals_empty_stack_exit() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![ScreenCommand::Pop],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Esc)]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::EmptyStack);
    }

    #[test]
    fn loop_save_command_invokes_callback_and_continues() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![ScreenCommand::Save, ScreenCommand::Quit],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('s')), Some(Input::Char('q'))]);

        let saves = Rc::new(RefCell::new(0u32));
        let saves_clone = saves.clone();
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(move || {
            *saves_clone.borrow_mut() += 1;
            Ok(())
        });

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        // Task 4c emitted Save → on_save once; Task 4d adds a second
        // call on the Quit drain path. Two invocations total: one
        // explicit, one from clean-exit auto-flush.
        assert_eq!(
            *saves.borrow(),
            2,
            "Save side effect + Quit drain must call on_save twice"
        );
    }

    #[test]
    fn built_game_save_handler_threads_into_runtime_loop() {
        // Task 4c: the handler installed via `Game::with_save_handler`
        // must travel through `Game::build()` onto `BuiltGame`, and
        // from there `run_built` (here: `run_with_io` driven by the
        // same `take()` move that `run_built_with_opener` performs)
        // must observe it as the `on_save` sink. Asserts the handler
        // fires exactly once for a single `ScreenCommand::Save`.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log,
            vec![ScreenCommand::Save, ScreenCommand::Quit],
            vec![],
            vec![],
        );

        let saves = Rc::new(RefCell::new(0u32));
        let saves_clone = saves.clone();

        let mut built = Game::new("Test")
            .push_screen(Box::new(screen))
            .with_save_handler(move || {
                *saves_clone.borrow_mut() += 1;
                Ok(())
            })
            .build()
            .expect("valid builder");

        // Mirror what `run_built_with_opener` does: take the handler
        // off the validated game and feed it to the loop. The fallback
        // closure here matches the production path's "no handler ⇒
        // no-op" behaviour and is never called in this test.
        let mut on_save: SaveHandler = built
            .save_handler
            .take()
            .expect("with_save_handler installs a handler");

        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('s')), Some(Input::Char('q'))]);

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        // One call from the Save side effect (Task 4c) + one from the
        // Quit drain (Task 4d). The "exactly once per Save" semantic
        // for Task 4c is now exercised by `loop_save_only_invokes_handler_once`.
        assert_eq!(
            *saves.borrow(),
            2,
            "Save effect + Quit drain must each invoke the threaded SaveHandler"
        );
    }

    #[test]
    fn loop_quit_drain_invokes_save_handler_once() {
        // Task 4d: a clean exit via `ScreenCommand::Quit` — with no
        // explicit `Save` emitted by any screen — must still invoke
        // the runtime save handler exactly once so games that opt
        // into `Game::with_save_handler` get an unconditional flush
        // on quit.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("only", log, vec![ScreenCommand::Quit], vec![], vec![]);
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('q'))]);

        let saves = Rc::new(RefCell::new(0u32));
        let saves_clone = saves.clone();
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(move || {
            *saves_clone.borrow_mut() += 1;
            Ok(())
        });

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        assert_eq!(
            *saves.borrow(),
            1,
            "Quit drain must invoke on_save exactly once even with no prior Save"
        );
    }

    #[test]
    fn loop_empty_stack_drain_invokes_save_handler_once() {
        // Task 4d: `ExitReason::EmptyStack` is the other clean-exit
        // path — the last screen `Pop`s itself off the stack. SPEC_v2_1
        // §4.4 lists "explicit Quit or empty stack" as the conditions
        // for the drain call, so the handler must fire here too.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("only", log, vec![ScreenCommand::Pop], vec![], vec![]);
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('p'))]);

        let saves = Rc::new(RefCell::new(0u32));
        let saves_clone = saves.clone();
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(move || {
            *saves_clone.borrow_mut() += 1;
            Ok(())
        });

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::EmptyStack);
        assert_eq!(
            *saves.borrow(),
            1,
            "EmptyStack drain must invoke on_save exactly once"
        );
    }

    #[test]
    fn loop_error_exit_does_not_invoke_save_handler() {
        // Task 4d: an error path (e.g. a failing Save callback or a
        // render IO error) must NOT trigger the drain call — the
        // drain is for *clean* exits only. Here we drive a failing
        // Save and assert the handler ran exactly once (for the
        // explicit Save) and not a second time on the way out.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("only", log, vec![ScreenCommand::Save], vec![], vec![]);
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('s'))]);

        let saves = Rc::new(RefCell::new(0u32));
        let saves_clone = saves.clone();
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(move || {
            *saves_clone.borrow_mut() += 1;
            Err(GameError::Save("synthetic".into()))
        });

        let err = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .unwrap_err();
        assert!(matches!(err, GameError::Save(_)), "got {err:?}");
        assert_eq!(
            *saves.borrow(),
            1,
            "error exit must not double-invoke the handler via the drain path"
        );
    }

    #[test]
    fn loop_save_callback_error_propagates() {
        // A failing save must abort the loop. The runtime can't keep
        // running with an unflushed save state across a screen
        // transition without violating SPEC §7.2 ("Save on important
        // durable state changes").
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("only", log, vec![ScreenCommand::Save], vec![], vec![]);
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('s'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> =
            Box::new(|| Err(GameError::Save("disk full".into())));

        let err = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .unwrap_err();
        assert!(matches!(err, GameError::Save(_)), "got {err:?}");
    }

    #[test]
    fn loop_resize_updates_terminal_size_for_next_frame() {
        // After a resize the cached size must be visible to both
        // `on_resize` (current iteration) and the next `tick` /
        // `render` (subsequent iteration). We script a Resize, then
        // a None (tick) which records the cached size, then a Quit.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![ScreenCommand::Quit],
            vec![ScreenCommand::None],
            vec![ScreenCommand::None],
        );
        let built = make_built(Box::new(screen));
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("test backend");
        let mut events = VecEventSource::new(vec![
            Some(Input::Resize {
                width: 100,
                height: 30,
            }),
            None, // tick
            Some(Input::Char('q')),
        ]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        let log = log.borrow();
        // Sequence: render(40x10) → resize:100x30 → render → tick:100x30
        // → render → input:Char('q'). Note we don't assert TestBackend
        // resize itself — that's ratatui's job — only that the cached
        // GameContext.terminal_size reflects the new dimensions.
        assert_eq!(
            *log,
            vec![
                "only:render",
                "only:resize:100x30",
                "only:render",
                "only:tick:100x30",
                "only:render",
                "only:input:Char('q')",
            ]
        );
    }

    #[test]
    fn loop_tick_fires_when_event_source_times_out() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![],
            vec![ScreenCommand::Quit],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        // None = timeout → tick fires → tick returns Quit → loop exits.
        let mut events = VecEventSource::new(vec![None]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        let log = log.borrow();
        assert!(
            log.iter().any(|s| s.starts_with("only:tick:")),
            "tick must have fired; log was {log:?}"
        );
    }

    #[test]
    fn loop_event_source_error_propagates() {
        struct ErroringSource;
        impl EventSource for ErroringSource {
            fn next_input(&mut self, _timeout: Duration) -> std::io::Result<Option<Input>> {
                Err(std::io::Error::other("ev source down"))
            }
        }

        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("only", log, vec![], vec![], vec![]);
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = ErroringSource;
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let err = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .unwrap_err();
        assert!(matches!(err, GameError::EventIo(_)), "got {err:?}");
    }

    #[test]
    fn loop_message_and_error_side_effects_do_not_break_the_loop() {
        // Until Task 9c wires the status-line widget, Message/Error
        // are routed to a no-op. The contract: the loop continues
        // running so subsequent commands still drive transitions.
        let cfg = fixture_config();
        let fc = fixture_context();
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "only",
            log.clone(),
            vec![
                ScreenCommand::Message("hi".into()),
                ScreenCommand::Error("oops".into()),
                ScreenCommand::Quit,
            ],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![
            Some(Input::Char('a')),
            Some(Input::Char('b')),
            Some(Input::Char('c')),
        ]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");
        assert_eq!(reason, ExitReason::Quit);
        let log = log.borrow();
        // Three input dispatches: the loop stayed alive through
        // Message and Error.
        assert_eq!(
            log.iter().filter(|s| s.contains(":input:")).count(),
            3,
            "message/error must not break the loop; log was {log:?}"
        );
    }

    // -------------------------------------------------------------
    // Terminal restoration via TerminalGuard
    //
    // We can't drive the production `run_built` path under `cargo
    // test` (no TTY), but we can prove that *any* runtime path that
    // wraps `run_with_io` between guard construction and `cleanup()`
    // restores the terminal even when the loop returns an error.
    // -------------------------------------------------------------

    #[derive(Default)]
    struct RecordingBackend {
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    impl TerminalBackend for RecordingBackend {
        fn enter(&mut self) -> Result<(), TerminalError> {
            self.log.borrow_mut().push("enter");
            Ok(())
        }
        fn leave(&mut self) -> Result<(), TerminalError> {
            self.log.borrow_mut().push("leave");
            Ok(())
        }
    }

    #[test]
    fn terminal_restoration_runs_when_loop_exits_cleanly() {
        // Mirror the production shape: construct guard → run loop →
        // cleanup. Recording backend asserts leave() fires exactly
        // once.
        let log = Rc::new(RefCell::new(Vec::new()));
        let backend = RecordingBackend { log: log.clone() };
        let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");

        let cfg = fixture_config();
        let fc = fixture_context();
        let screen_log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "title",
            screen_log,
            vec![ScreenCommand::Quit],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('q'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let result = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        );
        guard.cleanup().expect("cleanup ok");

        assert_eq!(result.expect("loop ok"), ExitReason::Quit);
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn terminal_restoration_runs_when_loop_returns_error() {
        // SPEC §7.3 controlled-error path: even if the loop bubbles
        // up an error, the terminal must be restored before the
        // caller prints the diagnostic. Mirrored at the test level.
        let log = Rc::new(RefCell::new(Vec::new()));
        let backend = RecordingBackend { log: log.clone() };
        let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");

        let cfg = fixture_config();
        let fc = fixture_context();
        let screen_log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new(
            "title",
            screen_log,
            vec![ScreenCommand::Save],
            vec![],
            vec![],
        );
        let built = make_built(Box::new(screen));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('s'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> =
            Box::new(|| Err(GameError::Save("nope".into())));

        let result = run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        );
        // Cleanup must run regardless of result.
        guard.cleanup().expect("cleanup ok");

        assert!(matches!(result, Err(GameError::Save(_))));
        assert_eq!(
            *log.borrow(),
            vec!["enter", "leave"],
            "leave must run even on loop error"
        );
    }

    #[test]
    fn crossterm_event_source_constructs() {
        // Smoke check: constructing the production event source is
        // side-effect-free. (`next_input` would block on a real TTY;
        // we don't call it.)
        let _ = CrosstermEventSource::new();
        let _: CrosstermEventSource = Default::default();
    }

    // -------------------------------------------------------------
    // Task 10b — world DB startup wiring
    // -------------------------------------------------------------

    /// A world-disabled config (the `fixture_config` baseline) MUST
    /// short-circuit to `Ok(None)` so v1 games keep booting unchanged.
    #[test]
    fn open_world_db_if_enabled_returns_none_when_disabled() {
        let cfg = fixture_config();
        assert!(!cfg.world.enabled, "fixture baseline is world-off");
        let result = open_world_db_if_enabled(&cfg).expect("disabled path must not error");
        assert!(
            result.is_none(),
            "disabled `[world]` must yield no DB handle"
        );
    }

    /// When `[world].enabled = true` the helper opens (and bootstraps)
    /// a SQLite file at the configured path. Routing `path` through a
    /// `tempdir` here proves the helper honours the configured path
    /// rather than a hard-coded location.
    #[test]
    fn open_world_db_if_enabled_opens_db_when_enabled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = fixture_config();
        cfg.world.enabled = true;
        cfg.world.path = tmp
            .path()
            .join("world.sqlite")
            .to_string_lossy()
            .into_owned();

        let db = open_world_db_if_enabled(&cfg)
            .expect("enabled path must succeed for a valid temp path")
            .expect("Some(db) when enabled");
        // Sanity: the DB is usable end-to-end (busy timeout was applied
        // and journal_mode came back as one of the documented modes).
        let mode = db.journal_mode();
        assert!(
            matches!(
                mode,
                "wal" | "delete" | "memory" | "truncate" | "persist" | "off"
            ),
            "unexpected journal_mode: {mode}"
        );
    }

    /// World-enabled but pointed at a path SQLite cannot create:
    /// resolves to a `WorldOpen` error rather than a panic or a silent
    /// downgrade. The "/" parent for a nested file is what trips up
    /// `create_dir_all` on the macOS sandbox; using a path beneath a
    /// real *file* (not a directory) is a portable way to force the
    /// failure.
    #[test]
    fn open_world_db_if_enabled_propagates_open_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Create a regular file, then ask the helper to use a path
        // that treats it as a directory. `create_dir_all` rejects this
        // on every supported platform.
        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").expect("create blocker file");
        let mut cfg = fixture_config();
        cfg.world.enabled = true;
        cfg.world.path = blocker
            .join("nested")
            .join("world.sqlite")
            .to_string_lossy()
            .into_owned();

        let err = open_world_db_if_enabled(&cfg).expect_err("must surface open failure");
        assert!(
            matches!(err, GameError::WorldOpen(_)),
            "unexpected variant: {err:?}"
        );
    }

    // -------------------------------------------------------------
    // Task 10c — DB-open failure must not engage the terminal
    // -------------------------------------------------------------

    /// SPEC §7.1 / Task 10c invariant: when the world-DB opener
    /// fails, `run_built_with_opener` MUST return the error before
    /// any terminal state changes. The injected opener never touches
    /// SQLite — it just returns a synthetic `WorldOpen` — which lets
    /// us assert the runtime never reached the guard.
    ///
    /// Verification has two prongs:
    ///
    /// 1. The returned error matches `GameError::WorldOpen(_)` —
    ///    proving the opener's error was the cause of exit, not some
    ///    later step (e.g. a missing TTY in CI).
    /// 2. `crossterm::terminal::is_raw_mode_enabled()` is unchanged
    ///    across the call. If a regression ever moves DB-open after
    ///    the guard, the guard's setup would flip raw mode to `true`;
    ///    even if the guard's `Drop` later restored it, we would have
    ///    momentarily owned the terminal. The before/after snapshot
    ///    here is a coarse but durable witness that we never engaged.
    #[test]
    fn run_built_with_opener_does_not_engage_terminal_on_open_failure() {
        let pre_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);

        // A scripted screen so `make_built` succeeds; we never reach
        // this screen in this test, but `BuiltGame` requires at least
        // one to construct.
        let log = Rc::new(RefCell::new(Vec::new()));
        let screen = ScriptedScreen::new("never-reached", log, vec![], vec![], vec![]);
        let mut built = make_built(Box::new(screen));
        // `run_built_with_opener` requires both config and foglet to
        // be present (it `take()`s them up front). The fixtures are
        // the same ones every other 10x test uses.
        built.config = Some(fixture_config());
        built.foglet = Some(fixture_context());

        // Synthetic failure — `InvalidJournalMode` is the cheapest
        // `WorldDbError` to construct (no `rusqlite::Error` round-
        // trip), and the variant is irrelevant to the assertion: we
        // only care that the outer wrapper is `WorldOpen`.
        let opener: Box<WorldOpenerFn> = Box::new(|_cfg| {
            Err(GameError::WorldOpen(WorldDbError::InvalidJournalMode {
                requested: "synthetic-test-failure".into(),
            }))
        });

        let err =
            run_built_with_opener(built, &*opener).expect_err("failing opener must surface as Err");

        assert!(
            matches!(err, GameError::WorldOpen(_)),
            "expected WorldOpen, got: {err:?}"
        );

        // The guard would have flipped raw mode to `true` had we
        // reached it. Equality with the pre-call snapshot is the
        // load-bearing assertion.
        let post_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        assert_eq!(
            pre_raw, post_raw,
            "raw mode state must be unchanged when opener fails before guard construction"
        );
    }

    /// When `run_with_io` is given `Some(world_db)`, every per-frame
    /// `GameContext` must carry the same handle so screens can reach
    /// the world layer via `ctx.world_db`. We verify by recording the
    /// `is_some()` result from inside `render`, `tick`, and
    /// `handle_input` and asserting all three saw the handle.
    #[test]
    fn run_with_io_threads_world_db_into_context() {
        struct Probe {
            sightings: Rc<RefCell<Vec<&'static str>>>,
            inputs: VecDeque<ScreenCommand>,
            ticks: VecDeque<ScreenCommand>,
        }
        impl Screen for Probe {
            fn render(&mut self, ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {
                if ctx.world_db.is_some() {
                    self.sightings.borrow_mut().push("render");
                }
            }
            fn handle_input(&mut self, ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
                if ctx.world_db.is_some() {
                    self.sightings.borrow_mut().push("input");
                }
                self.inputs.pop_front().unwrap_or(ScreenCommand::None)
            }
            fn tick(&mut self, ctx: &mut GameContext<'_>) -> ScreenCommand {
                if ctx.world_db.is_some() {
                    self.sightings.borrow_mut().push("tick");
                }
                self.ticks.pop_front().unwrap_or(ScreenCommand::None)
            }
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let db = WorldDb::open(tmp.path().join("world.sqlite")).expect("open db");

        let cfg = fixture_config();
        let fc = fixture_context();
        let sightings = Rc::new(RefCell::new(Vec::new()));
        let probe = Probe {
            sightings: sightings.clone(),
            inputs: VecDeque::from(vec![ScreenCommand::Quit]),
            ticks: VecDeque::from(vec![ScreenCommand::None]),
        };
        let built = make_built(Box::new(probe));
        let mut term = make_terminal();
        // None = tick, then Char('q') triggers handle_input → Quit.
        let mut events = VecEventSource::new(vec![None, Some(Input::Char('q'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        let reason = run_with_io(
            built,
            &cfg,
            &fc,
            Some(&db),
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert_eq!(reason, ExitReason::Quit);
        let seen = sightings.borrow();
        // All three dispatch sites observed the handle. (Render fires
        // multiple times — once per loop iteration — so we just check
        // each tag appeared at least once.)
        assert!(seen.contains(&"render"), "render saw no world_db: {seen:?}");
        assert!(seen.contains(&"tick"), "tick saw no world_db: {seen:?}");
        assert!(seen.contains(&"input"), "input saw no world_db: {seen:?}");
    }

    /// Symmetric check: when `world_db` is `None`, contexts MUST NOT
    /// fabricate a handle. This guards against an accidental
    /// `unwrap_or(default)` regression in the threading helper.
    #[test]
    fn run_with_io_leaves_world_db_none_when_unset() {
        struct Probe {
            saw_some: Rc<RefCell<bool>>,
        }
        impl Screen for Probe {
            fn render(&mut self, ctx: &mut GameContext<'_>, _frame: &mut ratatui::Frame<'_>) {
                if ctx.world_db.is_some() {
                    *self.saw_some.borrow_mut() = true;
                }
            }
            fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
                ScreenCommand::Quit
            }
        }

        let cfg = fixture_config();
        let fc = fixture_context();
        let saw_some = Rc::new(RefCell::new(false));
        let probe = Probe {
            saw_some: saw_some.clone(),
        };
        let built = make_built(Box::new(probe));
        let mut term = make_terminal();
        let mut events = VecEventSource::new(vec![Some(Input::Char('q'))]);
        let mut on_save: Box<dyn FnMut() -> Result<(), GameError>> = Box::new(|| Ok(()));

        run_with_io(
            built,
            &cfg,
            &fc,
            None,
            (40, 10),
            &mut term,
            &mut events,
            &mut on_save,
        )
        .expect("loop ok");

        assert!(
            !*saw_some.borrow(),
            "ctx.world_db must remain None when run_with_io is given None"
        );
    }
}
