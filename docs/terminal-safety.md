# Terminal safety contract

Terminal safety is release-critical for `foglet_game`. A door that
leaves the operator's shell in raw mode, stuck inside the alternate
screen, or echoing nothing is worse than a door that crashes — the
operator has no obvious recovery beyond `reset` or closing the session.
This document is the concrete evidence that the kit honours the
terminal-safety contract.

## 1. The contract

The terminal MUST be restored on every termination path:

- Normal quit.
- `Ctrl-C` interrupt.
- Controlled error (`anyhow::Error` bubbling out of `Game::run`).
- Panic mid-TUI ownership.
- A cramped-terminal refusal — the size check fails *without* having
  entered raw mode or the alternate screen, so there is nothing to
  restore.

"Restored" means: the alternate screen is exited, raw mode is disabled,
the cursor is shown, and any pending output is flushed. After that, the
shell prompt is usable, typing echoes normally, and panic / error
messages land on the user's real terminal where they can be read.

## 2. Startup ordering

`foglet_game::Game::run` performs startup in this order. The
size-check-before-raw-mode rule is the load-bearing piece — if the
terminal is too small we refuse cleanly, with no alt-screen residue
to undo:

1. Parse CLI args.
2. Load game config.
3. Load Foglet context (`FOGLET_DOOR_CONTEXT` → env fallback →
   local-dev synthesis).
4. Resolve assets path.
5. Resolve per-user save path.
6. Determine terminal size.
7. **Check minimum terminal size.** A failure here returns an error
   *before* any terminal mode change, so the operator's shell is
   untouched.
8. Only after the size check, enter raw mode and the alternate screen
   via `TerminalGuard::new`.
9. Install the panic hook (`install_panic_hook` + `arm_panic_hook`)
   so that a panic from this point onward restores the terminal
   before the default hook prints.
10. Push the initial screen.
11. Enter the runtime loop.

The implementation lives in `crates/foglet_game/src/runtime.rs` and
`crates/foglet_game/src/terminal.rs`. The contract is enforced by
`Game::run` itself; per-game code never reaches into raw-mode setup
directly.

## 3. The guard

`TerminalGuard` owns the raw-mode + alt-screen state and runs the
restoration sequence in two places:

- **`Drop`.** Engaged unconditionally; safe to double-drop because the
  guard tracks a `cleaned` flag.
- **Explicit `cleanup()`.** Called by the runtime loop on the normal
  exit path so the operator sees their shell *before* any "save
  flushed" or error message prints. After `cleanup()`, the subsequent
  `Drop` is a no-op.

Idempotency tests cover explicit-cleanup-then-drop and
drop-without-explicit-cleanup. See
`crates/foglet_game/src/terminal.rs`.

## 4. The panic hook

`install_panic_hook` captures the previous hook (typically the default
backtrace printer) and replaces it with one that:

1. Runs the guard's restoration sequence first.
2. Chains to the previous hook so panic output and `RUST_BACKTRACE`
   behaviour are preserved.

`arm_panic_hook` flips the hook from "no-op" to "restore the live
guard" once the guard exists, so a panic during construction itself
cannot fire the restorer against an uninitialised terminal.

The runtime loop installs and arms the hook *after* the guard is
constructed and *before* dispatching the first event. The `Drop` order
of locals in `Game::run` ensures the hook is disarmed before the guard
itself is dropped.

## 5. Logging discipline

While the TUI owns the terminal, the kit MUST NOT write to stdout.
Concretely:

- `tracing` is configured to a file appender (when the host opts in)
  or to a no-op subscriber. There is no `tracing_subscriber::fmt()`
  default that would target stdout.
- Errors meant for the operator are returned from `Game::run` and
  printed by the `fgk`-generated `main` *after* the guard has been
  dropped or `cleanup()`-ed.
- `println!` / `eprintln!` are not used in any TUI-active path.
  Reviewers should reject PRs that add them inside the runtime loop.

## 6. Manual smoke recipe

CI cannot drive a real TTY. The recipe lives next to the tests so the
maintainer re-running them sees it alongside the unit suite:

- [`crates/foglet_game/tests/manual-smoke.md`](../crates/foglet_game/tests/manual-smoke.md)

It covers, at minimum:

- **Scenario 1 — Clean quit.** Quit key restores the shell.
- **Scenario 2 — `Ctrl-C` interrupt.** Mapped to a controlled
  shutdown by the input mapper; guard `cleanup()` runs.
- **Scenario 3 — Panic during TUI ownership.** Forced panic; alt
  screen exits, raw mode disables, the panic message and (with
  `RUST_BACKTRACE=1`) the backtrace land on the real shell.
- **Scenario 4 — Resize mid-session.** Cramped resize surfaces a
  notice inside the alt screen without exiting raw mode.
- **Scenario 5 — SSH disconnect.** Best-effort: SIGHUP-without-handler
  cannot run `Drop`; document any regression in the commit body.
- **Scenario 6 — Save persistence round trip.**

Re-run the recipe before any release that touches `terminal.rs`,
`runtime.rs`, or the panic-hook plumbing, and reference this file in
the commit body so the evidence trail stays auditable.

## 7. What the unit tests do cover

`cargo test --workspace` is not a substitute for the manual recipe,
but it does cover the parts that do not need a real TTY:

- `TerminalGuard` setup/teardown ordering, idempotency, and the
  `cleanup()`-then-`Drop` no-op contract.
- Panic-hook arm / disarm bookkeeping using a recording backend that
  observes the restorer being called before the chained hook.
- The runtime loop driven by a scripted `EventSource`, asserting that
  `ScreenCommand::Quit` runs `cleanup()` and flushes saves before
  return.
- Input mapping for arrows, Enter, Esc, chars, `Ctrl-C`, and
  `Resize` — the events the safety contract relies on.

The combination — automated coverage of the orchestration plus the
manual recipe for the real-TTY paths — is what the terminal-safety
contract requires.
