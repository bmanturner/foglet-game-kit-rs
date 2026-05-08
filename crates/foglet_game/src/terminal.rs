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
//! # Panic-hook integration (Task 5c)
//!
//! SPEC §7.3 / §13.1 require that a panic mid-loop restore the
//! terminal *before* the default panic handler prints — otherwise the
//! panic message lands inside the alternate screen and is wiped out
//! the instant the process exits, leaving the operator with both a
//! corrupted terminal and no diagnostic.
//!
//! The integration is process-global because [`std::panic::set_hook`]
//! is process-global. We expose four pieces:
//!
//! - [`install_panic_hook`] (and [`install_panic_hook_with`] for
//!   tests) registers a hook that runs a configured restoration
//!   callback then chains to the previously-installed hook.
//!   Idempotent — the underlying [`std::sync::Once`] guarantees the
//!   hook is set exactly once even if multiple guards are constructed
//!   over a process's lifetime (e.g. a test binary that constructs
//!   several).
//! - [`arm_panic_hook`] / [`disarm_panic_hook`] toggle whether the
//!   hook actually runs restoration. The guard arms on construction
//!   (production path only) and disarms on `cleanup()` / Drop, so a
//!   panic *outside* a TUI session is left to the default hook.
//! - [`is_panic_hook_armed`] is exposed mainly for tests asserting
//!   the arm/disarm contract.
//!
//! The hook itself does *not* hold a reference to a [`TerminalGuard`].
//! The terminal is process-global state; the guard is just one
//! supervisor of it. Passing a function pointer instead of a closure
//! lets the registry stay `Sync` without unsafe code and keeps the
//! panic-hook path allocation-free at the moment it matters most.

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

/// Function-pointer signature for the restoration callback the panic
/// hook invokes when armed.
///
/// We require a `fn()` (not a closure) on purpose: function pointers
/// are `Copy + Send + Sync` and so live inside a `Mutex<Option<_>>`
/// without any `Box<dyn ...>` ceremony, and they keep the hook path
/// — which runs while the runtime is already mid-collapse —
/// allocation-free. The production restorer (`default_panic_restore`)
/// is the only `fn()` the runtime registers; tests register their own
/// to assert the arm/disarm contract without touching a real
/// terminal.
pub type PanicRestoreFn = fn();

/// One-shot guard for the [`std::panic::set_hook`] call.
///
/// `set_hook` replaces the current process-wide hook; calling it more
/// than once would either drop earlier restorers (data race for any
/// running guard) or stack hooks unbounded across test runs. The
/// `Once` ensures we install exactly once and treat subsequent
/// `install_panic_hook*` calls as "update the restorer, leave the
/// hook itself alone."
static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Process-wide armed flag.
///
/// `true` while a [`TerminalGuard`] is responsible for the terminal
/// and a panic should restore it. The hook flips it back to `false`
/// the first time it fires so a chained panic (rare, but possible if
/// a destructor panics during unwind) does not re-run restoration on
/// an already-restored terminal.
static PANIC_HOOK_ARMED: AtomicBool = AtomicBool::new(false);

/// The restorer that the installed hook invokes when armed.
///
/// Stored behind a mutex so [`install_panic_hook_with`] (the test
/// seam) can swap the production crossterm restorer for a recording
/// stub without ripping out the hook itself.
static PANIC_RESTORE_FN: Mutex<Option<PanicRestoreFn>> = Mutex::new(None);

/// Install the production panic hook.
///
/// Idempotent. Safe to call from every [`TerminalGuard::new`]; only
/// the first call registers with the standard library, subsequent
/// calls just refresh the restorer to `default_panic_restore`
/// (which is what production always wants — the single shared
/// terminal is restored the same way regardless of which guard
/// armed the hook).
///
/// Call before constructing the guard so the hook is in place even
/// if `enter()` itself triggers a panic (rare, but the cost of being
/// defensive here is one `Once` check).
pub fn install_panic_hook() {
    install_panic_hook_with(default_panic_restore);
}

/// Install the panic hook with a caller-supplied restorer.
///
/// Public-but-test-leaning. The runtime always uses
/// [`install_panic_hook`]; this entry point exists so the unit tests
/// can register a recording function pointer and assert that it
/// fires (or does not) under the arm/disarm contract — without
/// driving a real TTY or relying on the default crossterm calls
/// silently no-op'ing on a piped stdout.
pub fn install_panic_hook_with(restore: PanicRestoreFn) {
    // Update the restorer first so the hook installation below sees
    // a populated slot if anything in the std-lib `set_hook` path
    // were to fire spuriously (it does not in practice; the order
    // here is for invariant clarity, not to defend against real
    // races).
    *PANIC_RESTORE_FN
        .lock()
        .expect("panic restore mutex poisoned") = Some(restore);
    PANIC_HOOK_INSTALLED.call_once(|| {
        // Capture and chain the previous hook. This matters in two
        // realistic environments: (a) `cargo test` installs its own
        // hook to capture per-test panic output — chaining preserves
        // that diagnostic; (b) a host application embedding the
        // runtime may have already installed its own hook for
        // crash reporting. Replacing instead of chaining would
        // silently break either.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            run_panic_restoration();
            prev(info);
        }));
    });
}

/// Mark the panic hook as armed.
///
/// Idempotent. Called by the runtime after a [`TerminalGuard`] is in
/// place; until then a panic should fall through to the default
/// (chained) hook with no terminal touching.
pub fn arm_panic_hook() {
    PANIC_HOOK_ARMED.store(true, Ordering::SeqCst);
}

/// Mark the panic hook as disarmed.
///
/// Called by [`TerminalGuard::cleanup`] and [`TerminalGuard`]'s Drop
/// impl. After disarming, a subsequent panic runs the chained
/// previous hook without first invoking the restorer — correct,
/// because by definition the terminal is no longer in raw mode.
pub fn disarm_panic_hook() {
    PANIC_HOOK_ARMED.store(false, Ordering::SeqCst);
}

/// Read the armed flag.
///
/// Primarily a test affordance; the runtime never branches on this.
pub fn is_panic_hook_armed() -> bool {
    PANIC_HOOK_ARMED.load(Ordering::SeqCst)
}

/// Body of the installed panic hook, factored out so tests can call
/// it directly (driving a real panic from a unit test would tear
/// down the test binary).
///
/// Performs an atomic disarm (`swap`) so a re-entrant panic during
/// the restorer cannot run restoration twice — once is the contract,
/// twice on an already-restored terminal is the regression we are
/// guarding against.
pub(crate) fn run_panic_restoration() {
    if PANIC_HOOK_ARMED.swap(false, Ordering::SeqCst) {
        // Copy the function pointer out before releasing the lock —
        // we do not want to hold the mutex across the call, both for
        // panic-during-panic safety and because the production
        // restorer touches stdout which can in pathological cases
        // block.
        let restore = PANIC_RESTORE_FN
            .lock()
            .expect("panic restore mutex poisoned")
            .as_ref()
            .copied();
        if let Some(f) = restore {
            f();
        }
    }
}

/// Production restorer used by [`install_panic_hook`].
///
/// Best-effort: every step is wrapped in `let _ =` because we are
/// already on the panic path and the only thing worse than a borked
/// terminal is a panic-during-panic abort that hides the original
/// diagnostic. Matches the order in [`CrosstermBackend::leave`]
/// (alt-screen exit before raw-mode disable) so the user-visible
/// behaviour of a panic is the same as a clean shutdown.
fn default_panic_restore() {
    let mut out = io::stdout();
    let _ = execute!(out, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

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
        // Install the panic hook *before* attempting `enter()`. If
        // `enter()` itself panics (vanishingly rare — it would mean
        // crossterm panicked rather than returning Err) we still want
        // restoration to fire. Arming happens after the guard is
        // built so a setup failure leaves the hook un-armed.
        install_panic_hook();
        let guard = Self::with_backend(CrosstermBackend::new())?;
        arm_panic_hook();
        Ok(guard)
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
        let res = self.backend.leave();
        // Disarm regardless of success: by this point we have made
        // our one teardown attempt, and a future panic must not
        // re-toggle a half-restored terminal. Arming is the
        // runtime's signal that "I own raw mode right now"; that
        // signal is no longer true.
        disarm_panic_hook();
        res
    }
}

impl<B: TerminalBackend> Drop for TerminalGuard<B> {
    fn drop(&mut self) {
        // Idempotency: if cleanup already ran (5b/5c) we MUST NOT
        // toggle raw mode again — doing so on a terminal that is
        // already restored corrupts state on some hosts. The flag
        // is the one source of truth for "guard is spent."
        if self.cleaned {
            // Even if `leave` already ran (via `cleanup()` or the
            // panic hook), the armed flag may still be set if the
            // guard skipped its disarm path — defend in depth.
            disarm_panic_hook();
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
        disarm_panic_hook();
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

    // -----------------------------------------------------------------
    // Panic hook tests (Task 5c)
    //
    // These touch process-global state (the armed flag and the
    // registered restorer). Cargo runs tests in parallel by default,
    // so we serialize the panic-hook tests behind a single mutex.
    // The mutex is `'static` and recovered from poisoning because a
    // failed assertion in one test must not block the rest.
    // -----------------------------------------------------------------

    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex as StdMutex;

    static PANIC_HOOK_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock_panic_hook_tests() -> std::sync::MutexGuard<'static, ()> {
        // Recover from a previously-poisoned guard: a single failing
        // assertion should not cascade into "all subsequent panic-hook
        // tests fail to acquire the lock."
        match PANIC_HOOK_TEST_LOCK.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// Registered restorer counter used by the panic-hook tests.
    static TEST_RESTORE_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn test_restore_fn() {
        TEST_RESTORE_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn reset_panic_hook_state() {
        // Always end a test with the hook *disarmed* and the counter
        // cleared, regardless of how the previous test left things.
        disarm_panic_hook();
        TEST_RESTORE_CALLS.store(0, Ordering::SeqCst);
        install_panic_hook_with(test_restore_fn);
    }

    #[test]
    fn install_panic_hook_is_idempotent() {
        let _guard = lock_panic_hook_tests();
        // Calling install twice must not panic and must leave a
        // restorer registered. We cannot directly observe the std-lib
        // hook count, but a working arm + run_panic_restoration cycle
        // proves the hook chained through.
        install_panic_hook();
        install_panic_hook_with(test_restore_fn);
        TEST_RESTORE_CALLS.store(0, Ordering::SeqCst);
        arm_panic_hook();
        run_panic_restoration();
        assert_eq!(TEST_RESTORE_CALLS.load(Ordering::SeqCst), 1);
        reset_panic_hook_state();
    }

    #[test]
    fn run_panic_restoration_invokes_restorer_when_armed() {
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        arm_panic_hook();
        assert!(is_panic_hook_armed(), "arm sets the flag");
        run_panic_restoration();
        assert_eq!(
            TEST_RESTORE_CALLS.load(Ordering::SeqCst),
            1,
            "armed hook fires the restorer"
        );
        assert!(
            !is_panic_hook_armed(),
            "hook must disarm itself after firing — guards against re-entrant panics"
        );
        reset_panic_hook_state();
    }

    #[test]
    fn run_panic_restoration_is_noop_when_disarmed() {
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        // Never arm, or explicitly disarm.
        disarm_panic_hook();
        run_panic_restoration();
        assert_eq!(
            TEST_RESTORE_CALLS.load(Ordering::SeqCst),
            0,
            "disarmed hook must not invoke the restorer"
        );
        reset_panic_hook_state();
    }

    #[test]
    fn arm_then_disarm_prevents_restoration() {
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        arm_panic_hook();
        disarm_panic_hook();
        run_panic_restoration();
        assert_eq!(TEST_RESTORE_CALLS.load(Ordering::SeqCst), 0);
        reset_panic_hook_state();
    }

    #[test]
    fn second_run_panic_restoration_is_noop_after_firing() {
        // Re-entrant panic guard: if a destructor panics during the
        // unwind triggered by the first panic, the hook fires again.
        // The second invocation must not double-call the restorer on
        // an already-restored terminal.
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        arm_panic_hook();
        run_panic_restoration();
        run_panic_restoration();
        assert_eq!(TEST_RESTORE_CALLS.load(Ordering::SeqCst), 1);
        reset_panic_hook_state();
    }

    #[test]
    fn guard_cleanup_disarms_panic_hook() {
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        let backend = RecordingBackend::default();
        let mut g = TerminalGuard::with_backend(backend).expect("setup ok");
        // `with_backend` does not arm — that is the runtime's job.
        // Simulate the runtime arming the hook after constructing
        // the guard, then assert cleanup disarms it.
        arm_panic_hook();
        assert!(is_panic_hook_armed());
        g.cleanup().expect("cleanup ok");
        assert!(
            !is_panic_hook_armed(),
            "cleanup() must disarm the panic hook so post-TUI panics use the default handler"
        );
        reset_panic_hook_state();
    }

    #[test]
    fn guard_drop_disarms_panic_hook() {
        let _guard = lock_panic_hook_tests();
        reset_panic_hook_state();
        let backend = RecordingBackend::default();
        {
            let _g = TerminalGuard::with_backend(backend).expect("setup ok");
            arm_panic_hook();
            assert!(is_panic_hook_armed());
            // Fall out of scope without calling cleanup — Drop must
            // still disarm.
        }
        assert!(
            !is_panic_hook_armed(),
            "Drop must disarm the panic hook even when cleanup() was not called"
        );
        reset_panic_hook_state();
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
