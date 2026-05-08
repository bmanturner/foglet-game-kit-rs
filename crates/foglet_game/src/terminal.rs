//! Terminal lifecycle guard.
//!
//! SPEC §13.1 declares terminal safety release-critical: every path that
//! enters raw mode + the alternate screen MUST restore the terminal —
//! on normal quit, controlled error, panic, Ctrl-C, and resize. Per
//! SPEC §7.1 the size check runs *before* the guard engages, so by the
//! time we touch raw mode we already know the terminal is large enough
//! to use safely.
//!
//! This module owns the *mechanics* of that contract. Higher layers
//! (the runtime in Task 7) own the *ordering* — they call the size
//! check, construct the guard, install the panic hook (Task 5c), and
//! drive the runtime loop. The guard's job is narrow: turn raw-mode +
//! alt-screen on at construction, turn them back off exactly once at
//! teardown, and never panic in the cleanup path.
//!
//! # Design — backend trait
//!
//! Real-terminal calls (`crossterm::terminal::enable_raw_mode` and the
//! `EnterAlternateScreen` queueing on stdout) require a TTY and so are
//! impossible to drive from `cargo test` running under a pipe. We
//! abstract them behind [`TerminalBackend`] for two reasons:
//!
//! 1. **Testability.** The Drop ordering, the `cleaned` idempotency
//!    flag, and (in Task 5c) the panic-hook integration are pure
//!    orchestration logic. A test backend that just records calls
//!    lets us assert that orchestration is correct without owning a
//!    real terminal — which is the part most likely to regress.
//! 2. **A single seam for future hosts.** If we ever need to drive
//!    something other than `Stdout` (a captured PTY in tests, an
//!    SSH-attached buffer), the seam is already here.
//!
//! The production backend is [`CrosstermBackend`] and is what the
//! runtime constructs in real use; it is intentionally *not*
//! unit-tested — its correctness rests on `crossterm`'s own coverage
//! and the manual-smoke recipe documented for SPEC §13.1.
//!
//! # What this module does NOT do (yet)
//!
//! - **No panic-hook installation.** Task 5c installs a panic hook
//!   that runs the same restoration *before* the default panic
//!   handler prints, per SPEC §7.3 / §13.1. The hook will reuse the
//!   `cleaned` flag below — by setting it to `true` after running
//!   restoration itself, it makes the eventual Drop a no-op without
//!   needing a back-channel to the guard instance.

use std::io::{self, Stdout, Write};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

/// Errors the terminal guard can surface during setup or teardown.
///
/// Both variants wrap the underlying [`io::Error`] from `crossterm`.
/// The two-variant split (rather than a single `Io(io::Error)`) is
/// intentional: callers handling setup failure typically want to
/// surface a clear-error message *before* anything has been changed,
/// while teardown failures happen during cleanup paths where the only
/// reasonable response is to log and move on (we are usually already
/// exiting). Distinguishing them up front keeps both call sites
/// honest.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// Raw mode + alternate screen could not be entered.
    ///
    /// In practice this means the process is not attached to a TTY,
    /// or the controlling terminal does not support the requested
    /// mode. The runtime should surface this as a controlled error
    /// per SPEC §7.3 and exit non-zero — no terminal state has been
    /// changed, so no restoration is required.
    #[error("failed to enter raw mode + alternate screen: {0}")]
    Setup(#[source] io::Error),

    /// Restoration failed during teardown.
    ///
    /// The guard tries both `LeaveAlternateScreen` and
    /// `disable_raw_mode` regardless; this variant captures whichever
    /// of the underlying calls reported an error. We surface it for
    /// observability — by the time it fires the process is usually
    /// already on the way out and there is no useful recovery.
    #[error("failed to restore terminal: {0}")]
    Teardown(#[source] io::Error),
}

/// Pluggable backend that performs the actual raw-mode + alternate-
/// screen toggling.
///
/// Implementors are expected to be small adapters over the real
/// terminal API (production) or recording stubs (tests). The trait
/// is deliberately minimal — `enter` then later `leave`, both
/// fallible — to keep the surface tractable and the orchestration
/// logic in [`TerminalGuard`] the load-bearing piece.
pub trait TerminalBackend {
    /// Switch the terminal into raw mode + the alternate screen.
    ///
    /// Called exactly once, when [`TerminalGuard::new`] succeeds.
    /// Implementations MUST leave the terminal in a "ready for the
    /// runtime loop" state on `Ok(())` and MUST NOT have changed any
    /// terminal state on `Err(...)` (so the caller can surface the
    /// error without first restoring).
    fn enter(&mut self) -> Result<(), TerminalError>;

    /// Restore the terminal to its original mode.
    ///
    /// Called exactly once per successful `enter`, whether through
    /// [`TerminalGuard`]'s Drop impl (the safety net) or — once Task
    /// 5b lands — an explicit `cleanup()`. Implementations MUST
    /// attempt every restoration step even if an earlier step fails,
    /// returning the *first* error observed. Skipping later steps on
    /// the first failure is the path that historically leaves users
    /// staring at a borked shell.
    fn leave(&mut self) -> Result<(), TerminalError>;
}

/// Production [`TerminalBackend`] that drives `crossterm` against
/// stdout.
///
/// Construct this when the runtime engages on a real TTY; the
/// `Stdout` handle is captured at construction so the same writer is
/// used for both `EnterAlternateScreen` and `LeaveAlternateScreen`.
/// Buffering is intentionally avoided — `crossterm::execute!` flushes
/// on every call, and the alt-screen sequences must reach the
/// terminal immediately or the user sees nothing happen.
pub struct CrosstermBackend {
    out: Stdout,
}

impl CrosstermBackend {
    /// Build a backend bound to the process's stdout.
    ///
    /// Capturing stdout once (rather than re-acquiring it each call)
    /// avoids interleaving raw-mode toggles with another part of the
    /// program writing to a freshly-locked stdout. There is exactly
    /// one TUI per process; one stdout handle per guard is correct.
    pub fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Default for CrosstermBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalBackend for CrosstermBackend {
    fn enter(&mut self) -> Result<(), TerminalError> {
        // Order matters: `enable_raw_mode` first means a failure here
        // leaves the alternate screen untouched, so there is nothing
        // to restore. If we entered the alt screen first and then
        // raw-mode failed, we would have to back out — and back-out
        // paths are precisely where terminal corruption originates.
        enable_raw_mode().map_err(TerminalError::Setup)?;
        execute!(self.out, EnterAlternateScreen).map_err(|e| {
            // Best-effort rollback: if alt-screen entry fails, undo
            // raw mode so the caller is genuinely back to the state
            // we found the terminal in. We deliberately swallow the
            // rollback error — the *original* failure is what the
            // caller needs to see.
            let _ = disable_raw_mode();
            TerminalError::Setup(e)
        })?;
        Ok(())
    }

    fn leave(&mut self) -> Result<(), TerminalError> {
        // Run BOTH steps even if the first fails. The motivating
        // failure mode: `LeaveAlternateScreen` errors (rare, but
        // possible on a half-disconnected SSH session) and we still
        // need raw mode disabled or the user's shell will eat the
        // next keystroke. We surface the first error to the caller
        // for observability but never short-circuit cleanup.
        let alt = execute!(self.out, LeaveAlternateScreen);
        let raw = disable_raw_mode();
        match (alt, raw) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(e), _) => Err(TerminalError::Teardown(e)),
            (_, Err(e)) => Err(TerminalError::Teardown(e)),
        }
    }
}

/// Owner of the terminal-mode lifecycle.
///
/// Construct one with [`TerminalGuard::new`] *after* the SPEC §7.1
/// minimum-size check has passed. While the guard is alive the
/// terminal is in raw mode + alt screen. When it is dropped the
/// terminal is restored — exactly once, even if Drop runs after an
/// explicit cleanup (Task 5b) or alongside the panic hook (Task 5c).
///
/// The guard is intentionally `!Send + !Sync` in spirit — there is
/// one terminal per process and the runtime owns it on a single
/// thread. We do not impl those traits explicitly because `Stdout`
/// is already `Send`, but constructing more than one guard at a
/// time is a logic error the caller is responsible for avoiding;
/// SPEC §7.1 ordering puts that responsibility on the runtime.
pub struct TerminalGuard<B: TerminalBackend = CrosstermBackend> {
    backend: B,
    /// `true` once the restoration sequence has run successfully.
    ///
    /// Drop checks this flag and skips `leave` if it is already set,
    /// so an explicit `cleanup()` (Task 5b) followed by Drop will
    /// only restore once. For Task 5a the flag has one observable
    /// consumer — the Drop impl — but it is part of the public
    /// invariant and gets exercised by 5b/5c, hence the early seat.
    cleaned: bool,
}

impl TerminalGuard<CrosstermBackend> {
    /// Construct a guard backed by the real `crossterm` adapter.
    ///
    /// This is the production constructor — the runtime wires it up
    /// in the SPEC §7.1 startup sequence. Tests prefer
    /// [`TerminalGuard::with_backend`] so they can assert the
    /// orchestration without a TTY.
    ///
    /// Returns `Err(TerminalError::Setup)` if raw mode or the
    /// alternate screen could not be entered; the terminal is left
    /// untouched in that case so the caller can surface a clear
    /// error per SPEC §7.3.
    pub fn new() -> Result<Self, TerminalError> {
        Self::with_backend(CrosstermBackend::new())
    }
}

impl<B: TerminalBackend> TerminalGuard<B> {
    /// Construct a guard with a caller-supplied backend.
    ///
    /// Primarily a test seam: pair this with a recording backend to
    /// assert that `enter` runs exactly once, that Drop calls `leave`
    /// exactly once, and (in 5b) that an explicit `cleanup()`
    /// followed by Drop does not double-restore.
    pub fn with_backend(mut backend: B) -> Result<Self, TerminalError> {
        backend.enter()?;
        Ok(Self {
            backend,
            cleaned: false,
        })
    }

    /// `true` once the restoration sequence has run.
    ///
    /// Exposed for tests; the runtime never needs to ask. The bool
    /// is the canonical signal that this guard is "spent" — Drop
    /// uses it to make itself a no-op after an explicit cleanup
    /// (Task 5b) or after the panic hook has already restored
    /// (Task 5c).
    pub fn is_cleaned(&self) -> bool {
        self.cleaned
    }

    /// Explicitly restore the terminal *now* and mark the guard spent.
    ///
    /// The motivating use case is SPEC §7.3's "controlled error" path:
    /// the runtime hits a recoverable failure inside the loop and
    /// wants to restore the terminal *before* printing the error
    /// message — otherwise the message lands inside the alternate
    /// screen and disappears the moment the process exits. Drop is
    /// the safety net; this is the eager path callers reach for when
    /// they need the error text to actually be visible.
    ///
    /// After this returns (success or failure) the guard is marked
    /// cleaned, so the eventual Drop is a no-op. Calling `cleanup`
    /// twice is safe and a no-op the second time — the surfaced
    /// teardown error is whatever the *first* call observed.
    ///
    /// Unlike Drop, this returns the teardown error so callers can
    /// log it. By the time the error fires the terminal is in an
    /// undefined state and there is no useful recovery beyond
    /// reporting it; the runtime's response should be to log via the
    /// file appender (SPEC §13.2 — never stdout while we may have
    /// been in raw mode) and exit.
    pub fn cleanup(&mut self) -> Result<(), TerminalError> {
        if self.cleaned {
            return Ok(());
        }
        // Mark cleaned *before* attempting `leave`. Even on failure
        // the terminal has been partially poked — running `leave`
        // again from Drop would compound the corruption rather than
        // recover from it. The flag's invariant is "we have made our
        // one teardown attempt," not "teardown succeeded."
        self.cleaned = true;
        self.backend.leave()
    }
}

impl<B: TerminalBackend> Drop for TerminalGuard<B> {
    fn drop(&mut self) {
        // Idempotency: if cleanup already ran (5b/5c) we MUST NOT
        // toggle raw mode again — doing so on a terminal that is
        // already restored corrupts state on some hosts. The flag
        // is the one source of truth for "guard is spent."
        if self.cleaned {
            return;
        }
        // Drop is the safety net: every termination path eventually
        // gets here. We deliberately discard the error — there is no
        // Result to thread out of Drop, and by definition we are on
        // the way out. The guard's Setup-vs-Teardown error split
        // exists so explicit `cleanup()` callers in Task 5b can
        // observe teardown failures; Drop just does its best.
        let _ = self.backend.leave();
        self.cleaned = true;
    }
}

/// Best-effort flush helper used by the runtime when it wants to be
/// sure pending sequences have hit the terminal before a
/// soon-to-fail operation.
///
/// Kept here, alongside the guard, because it operates on the same
/// stdout handle the production backend uses. Logging — per SPEC
/// §13.2 — must not target stdout while the guard is engaged; this
/// helper is for *terminal escape sequences* the runtime layer has
/// already buffered, not for diagnostics.
pub fn flush_stdout() -> io::Result<()> {
    io::stdout().flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Recording backend used to assert orchestration without a TTY.
    ///
    /// Each method appends an entry to a shared log; tests then
    /// compare the log against expected sequences. The backend can
    /// be told to fail a specific call to exercise the error
    /// branches without changing the call shape.
    #[derive(Clone, Default)]
    struct RecordingBackend {
        log: Rc<RefCell<Vec<&'static str>>>,
        fail_enter: bool,
        fail_leave: bool,
    }

    impl TerminalBackend for RecordingBackend {
        fn enter(&mut self) -> Result<(), TerminalError> {
            self.log.borrow_mut().push("enter");
            if self.fail_enter {
                return Err(TerminalError::Setup(io::Error::other("boom")));
            }
            Ok(())
        }

        fn leave(&mut self) -> Result<(), TerminalError> {
            self.log.borrow_mut().push("leave");
            if self.fail_leave {
                return Err(TerminalError::Teardown(io::Error::other("boom")));
            }
            Ok(())
        }
    }

    #[test]
    fn new_calls_enter_once_and_marks_unclean() {
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        let guard = TerminalGuard::with_backend(backend).expect("setup ok");
        assert_eq!(*log.borrow(), vec!["enter"]);
        assert!(!guard.is_cleaned(), "fresh guard is not yet cleaned");
        // Drop happens at end of scope; checked in the next test.
        drop(guard);
    }

    #[test]
    fn drop_calls_leave_exactly_once() {
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        {
            let _guard = TerminalGuard::with_backend(backend).expect("setup ok");
        }
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn drop_runs_on_early_return() {
        // Simulates the SPEC §7.3 "controlled error" path: a function
        // owns the guard, hits an early `?`, and Drop must still
        // restore the terminal. We exercise this by constructing the
        // guard inside a closure that returns Err early.
        let backend = RecordingBackend::default();
        let log = backend.log.clone();

        fn run<B: TerminalBackend>(backend: B) -> Result<(), &'static str> {
            let _guard = TerminalGuard::with_backend(backend).map_err(|_| "setup")?;
            // Pretend something inside the runtime loop fails.
            Err("boom")
        }

        let res = run(backend);
        assert!(res.is_err());
        assert_eq!(
            *log.borrow(),
            vec!["enter", "leave"],
            "Drop must run on early return"
        );
    }

    #[test]
    fn setup_failure_returns_error_and_skips_leave() {
        let backend = RecordingBackend {
            fail_enter: true,
            ..RecordingBackend::default()
        };
        let log = backend.log.clone();
        let res = TerminalGuard::with_backend(backend);
        assert!(matches!(res, Err(TerminalError::Setup(_))));
        // No guard was constructed → Drop never runs → leave never
        // called. Critical invariant: a setup failure leaves the
        // terminal untouched, so callers can surface a clear error
        // without restoring something that was never engaged.
        assert_eq!(*log.borrow(), vec!["enter"]);
    }

    #[test]
    fn teardown_failure_in_drop_is_swallowed() {
        // Drop has no Result to surface; a failing teardown must
        // not panic. We assert that Drop completes and leaves the
        // log shape we expect (single leave attempted).
        let backend = RecordingBackend {
            fail_leave: true,
            ..RecordingBackend::default()
        };
        let log = backend.log.clone();
        {
            let _guard = TerminalGuard::with_backend(backend).expect("setup ok");
        }
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn manual_clean_flag_skips_leave_in_drop() {
        // Pre-condition for Task 5b: setting `cleaned` from outside
        // (which `cleanup()` will do) must turn Drop into a no-op so
        // the restoration runs exactly once.
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        {
            let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");
            // Stand-in for Task 5b's `cleanup()`.
            guard.backend.leave().expect("manual leave ok");
            guard.cleaned = true;
        }
        // Exactly one leave despite the guard going through Drop.
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn explicit_cleanup_then_drop_runs_leave_exactly_once() {
        // SPEC §7.3 controlled-error path: the runtime restores the
        // terminal *before* printing the error, then the guard is
        // dropped on the way out. Drop must observe `cleaned = true`
        // and skip its own `leave` call.
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        {
            let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");
            guard.cleanup().expect("cleanup ok");
            assert!(guard.is_cleaned(), "cleanup must mark guard spent");
        }
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn drop_without_explicit_cleanup_still_restores() {
        // The dual of the previous test: callers who never reach
        // `cleanup()` (panic mid-loop, normal Quit path before 5c
        // lands, etc.) must still get a one-shot Drop teardown.
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        {
            let _guard = TerminalGuard::with_backend(backend).expect("setup ok");
            // No explicit cleanup; just fall out of scope.
        }
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn double_cleanup_is_a_noop() {
        // Idempotency: a defensive caller (or a future runtime that
        // calls cleanup from both the controlled-error path *and* a
        // shutdown hook) must not double-restore.
        let backend = RecordingBackend::default();
        let log = backend.log.clone();
        let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");
        guard.cleanup().expect("first cleanup ok");
        guard.cleanup().expect("second cleanup is a noop");
        drop(guard);
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn cleanup_surfaces_teardown_error_and_marks_spent() {
        // Failed teardown is observable through `cleanup()` — that is
        // its whole reason for existing on top of Drop. The guard
        // still ends up marked spent so a subsequent Drop will not
        // re-attempt teardown on a half-restored terminal.
        let backend = RecordingBackend {
            fail_leave: true,
            ..RecordingBackend::default()
        };
        let log = backend.log.clone();
        let mut guard = TerminalGuard::with_backend(backend).expect("setup ok");
        let res = guard.cleanup();
        assert!(matches!(res, Err(TerminalError::Teardown(_))));
        assert!(guard.is_cleaned(), "failed cleanup still marks guard spent");
        drop(guard);
        // Exactly one leave attempt despite the failure + Drop.
        assert_eq!(*log.borrow(), vec!["enter", "leave"]);
    }

    #[test]
    fn crossterm_backend_constructs() {
        // Smoke check: constructing the production backend must not
        // touch the terminal. (`enter()` would; `new()` only
        // captures the stdout handle.)
        let _ = CrosstermBackend::new();
        let _ = CrosstermBackend::default();
    }
}
