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

## Scenario 6 — Save persistence round trip (Task 13h)

The save manager writes per-user JSON via `write_atomic` and reloads it
on the next launch. CI cannot drive a real Foglet door, so the
quit/launch cycle is verified by hand:

1. Pick a fresh save scratch directory: `export FGK_SAVE_DIR=$(mktemp -d)`.
2. Launch: `cargo run --example murder_motel`. Press Enter on the title,
   choose **New Game**, walk a few steps, talk to the Night Clerk and
   pick the rumor branch (so `heard_rumor` is set), pick up the brass
   key, then quit cleanly with `q`.
3. **Expected:** `$FGK_SAVE_DIR/save.json` exists. Inspecting it shows
   non-default `player_x` / `player_y`, `flags` containing
   `heard_rumor`, `inventory` containing `brass_key`, and either
   `won: true` or `won: false` depending on whether the player stepped
   onto the win tile before quitting.
4. Re-launch: `cargo run --example murder_motel`. Press Enter, choose
   **Continue**.
5. **Expected:** the player stands on the saved cell (not the configured
   spawn), the inventory screen still lists the previously collected
   items, and the locked door is rendered as already unlocked if the
   brass key was held at save time.
6. Choose **New Game** instead and confirm the slots reset: the player
   is back at `(start_x, start_y)`, no items, no flags.

Cleanup: `rm -rf "$FGK_SAVE_DIR"`. The save is JSON pretty-printed via
`serde_json::to_writer_pretty` per `crates/foglet_game/src/save.rs`, so
an operator inspecting `save.json` can read every field at a glance.

## Scenario 7 — Lost-and-Found Drawer prompt (v1.1 Task 10)

The drawer is the v1.1 proof scene for direct-key choice prompts with
a disabled choice and feedback messages.

1. Launch: `cargo run --example murder_motel`. From the title press
   Enter, choose **New Game**.
2. Walk west off the spawn until you reach the lobby's lost-and-found
   tile (the `X` "Search" affordance lights up in the hint bar when
   you are cardinally adjacent).
3. Press `x` (or `X`). The Lost-and-Found Drawer prompt appears with
   `(K) take Room 7 key`, `(M) pocket cracked matchbook`,
   `(R) read receipt`, `(L) leave drawer`.
4. **Expected:**
   - Press `k` → `Moved Room 7 key to inventory.` feedback line; the
     prompt re-renders with `(K)` greyed/disabled and the disabled
     reason visible. Pressing `k` again surfaces the disabled reason
     and does not duplicate the item.
   - Press `M` → `Pocketed the cracked matchbook.` feedback. Case
     insensitivity confirmed.
   - Press `r` → receipt body text renders.
   - Press `l` (or Esc) → drawer pops, returning to the map.
5. Quit with `q`. Terminal restores cleanly.

The keyboard reducer for these outcomes is exercised non-interactively
by the unit suite (`lost_and_found_*` tests in
`examples/murder_motel/src/main.rs`); this manual recipe confirms the
same behaviour through a real TTY.

## Scenario 8 — Night-clerk vendor prompt (v1.1 Task 11)

The night-clerk vendor is the v1.1 proof scene for dynamic labels,
disabled-by-state choices, and any-key continuation.

1. Launch: `cargo run --example murder_motel`. Title → **New Game**.
2. Walk to the front desk until you stand cardinally adjacent to the
   night clerk; the hint bar advertises `Buy: B`.
3. Press `b`. The vendor prompt renders with three choices whose
   labels include the live cash total (e.g. `[B] Buy black coffee — 25g
   (you have 40g)` and `[T] Tip for rumor — 50g (need 50g)`).
4. **Expected:**
   - With starting cash (40g), `(T)` is disabled and shows `need 50g`.
     Pressing `t` surfaces the disabled reason; cash is unchanged.
   - Press `b` → `Brewed a black coffee. -25g.` feedback, cash drops to
     15g, prompt is replaced with the any-key continuation screen
     (`Press any key to continue...`).
   - Press any meaningful key → returns to the map. Terminal resize
     while on the continuation screen is ignored (does not close it).
   - Re-engage the clerk with `b`, press `n` (no thanks) → vendor pops
     immediately with no state change.
   - Re-engage with `b`, press Esc → vendor cancels with no state
     change.
5. Quit with `q`. Terminal restores cleanly.

The state transitions are exercised by the `night_clerk_vendor_*`
tests; this recipe confirms the live-TTY rendering and key path.

## What this file is not

- Not a substitute for `cargo test` — the unit tests cover the
  arm/disarm contract, the idempotency invariants, and the recording-
  backend orchestration. This file covers the parts that need a real
  TTY.
- Not run by CI. SPEC §14 explicitly lists the manual checks as
  "implementation-defined" QA, and the maintainer is responsible
  for re-running them before tagging a release.
