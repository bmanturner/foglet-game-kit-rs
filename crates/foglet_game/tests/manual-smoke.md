# Terminal-safety manual smoke recipe

SPEC §13.1 declares terminal restoration release-critical and §14
explicitly accepts manual evidence for the cases CI cannot drive
(real TTY, real `Ctrl-C`, real panic-during-TUI). This file documents
those checks. Re-run before any release that touches `terminal.rs` or
the runtime loop, and reference this file from the commit body.

The expected "good" outcome for every scenario below is **the same**:
after the test ends, the operator's shell prompt re-appears, typing
echoes normally, line-editing works, and the panic message (when
applicable) is visible *outside* the alternate screen.

## Prerequisites

- A real terminal emulator (iTerm2, Alacritty, GNOME Terminal,
  `tmux` pane, etc.). Piped stdout will short-circuit the
  alternate-screen sequences and invalidate the test.
- A scratch binary that exercises the guard — once Task 7 lands the
  `murder_motel` example serves; in the meantime, write a throwaway
  binary that calls `TerminalGuard::new()` and then sleeps or loops
  on input.

## Scenario 1 — Clean quit

1. Launch the binary in a terminal.
2. Press the documented quit key (eventually `q`; before Task 7,
   exit the loop via whatever mechanism the throwaway binary
   provides).
3. **Expected:** alt-screen exits, raw mode disabled, shell prompt
   restored, terminal echoes typed input normally.

## Scenario 2 — Ctrl-C interrupt

1. Launch the binary.
2. Press `Ctrl-C` mid-session.
3. **Expected:** the runtime treats `Ctrl-C` as a controlled
   shutdown (Task 6 + Task 7d wire the input mapping); the guard's
   `cleanup()` runs and the terminal is restored.

## Scenario 3 — Panic during TUI ownership (Task 5c)

1. Launch a build of the binary that deliberately panics after
   engaging the guard. For example:

   ```rust
   let _guard = foglet_game::TerminalGuard::new()?;
   foglet_game::install_panic_hook();
   foglet_game::arm_panic_hook();
   panic!("smoke: forced panic with TUI engaged");
   ```

2. **Expected:**
   - Alt-screen exits **before** the panic backtrace prints.
   - Raw mode is disabled before the panic message reaches the
     terminal — the message lands at the operator's shell prompt
     and is readable, not buried inside the alt screen.
   - The shell prompt is usable immediately after the process
     exits (no `stty sane` needed).

3. Repeat with `RUST_BACKTRACE=1` to confirm the chained default
   hook still produces the backtrace — the kit's hook calls the
   restorer first and then chains, so cargo's / std's panic output
   is preserved.

## Scenario 4 — Resize mid-session

1. Launch the binary with a window large enough to satisfy the
   minimum-size check.
2. Resize the terminal smaller than the configured minimum.
3. **Expected:** the runtime surfaces a "terminal too small" notice
   inside the alt screen (Task 7d) without exiting raw mode mid-
   render. Restoring the window restores normal rendering.
4. Quit (Scenario 1). The terminal is restored cleanly.

## Scenario 5 — SSH disconnect (best-effort)

1. SSH into a remote shell and launch the binary on the remote host.
2. Forcibly close the local SSH client window (do not type `q`).
3. Reconnect.
4. **Expected:** the remote shell is usable on reconnect. The guard's
   `Drop` cannot run if the process is killed by SIGHUP-without-
   handler, but the next shell will reset terminal state on
   attachment, so a `reset` is the worst case. Document any
   regression here in `DECISIONS.md`.

## What this file is not

- Not a substitute for `cargo test` — the unit tests cover the
  arm/disarm contract, the idempotency invariants, and the recording-
  backend orchestration. This file covers the parts that need a real
  TTY.
- Not run by CI. SPEC §14 explicitly lists the manual checks as
  "implementation-defined" QA, and the maintainer is responsible
  for re-running them before tagging a release.
