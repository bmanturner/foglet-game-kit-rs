# foglet-game-kit-rs

A Rust authoring kit for [Foglet](https://github.com/bmanturner/foglet-bbs)
door games. It ships a library crate (`foglet_game`) for building the
game itself and a CLI (`fgk`) for scaffolding, packaging, and emitting
the Foglet operator manifest.

The kit targets Foglet's `:external_pty` runtime: each game compiles to
a single binary that Foglet launches under a PTY, with a documented
context handed in via `FOGLET_DOOR_CONTEXT` and `FOGLET_*` env vars.
See [`SPEC.md`](SPEC.md) for the full contract.

## What you get

- **`foglet_game`** — terminal guard with panic-safe restoration, a
  screen-stack runtime, input normalization, save manager with atomic
  writes, ASCII map and YAML dialog primitives, and ratatui widgets.
- **`fgk`** — `new` (scaffolder), `emit-manifest` (Foglet operator
  JSON), and `package` (deployable bundle with a boring auditable
  `run.sh` wrapper).
- **`murder_motel`** — a complete sample game that exercises every
  primitive, runnable as `cargo run --example murder_motel`.

## Quickstart — from zero to a working Foglet door

The walkthrough below produces a packaged door bundle on disk plus the
matching manifest JSON. Step 5 hands them to Foglet.

### 1. Install the CLI

Clone the kit and install `fgk` from source. Cargo drops the binary in
`~/.cargo/bin`, which should already be on your `PATH`.

```bash
git clone https://github.com/foglet/foglet-game-kit-rs.git
cd foglet-game-kit-rs
cargo install --path crates/fgk
fgk --version
```

### 2. Scaffold a new game

```bash
fgk new ~/code/murder-motel-smoke
cd ~/code/murder-motel-smoke
```

The project name (last path segment) becomes both the Cargo crate name
and the SPEC §9.1 game slug, so it must be lowercase ASCII alphanumeric
plus `-`. The scaffold includes `Cargo.toml`, `src/main.rs`, an
`assets/game.toml`, a starter map, `.gitignore`, and a per-project
README.

### 3. Run locally

```bash
cargo run
```

The kit synthesizes a local-dev Foglet context (`FOGLET_USER_ID=local-dev`,
terminal size from your TTY) when no `FOGLET_DOOR_CONTEXT` is present.
Saves land in `.fgk/saves/local-dev/save.json` so they don't collide
with packaged installs. Quit with `q` from the title screen; resize and
Ctrl-C are handled by the terminal guard.

To exercise the bundled sample game instead:

```bash
cargo run --example murder_motel
```

### 4. Emit the Foglet manifest

`emit-manifest` reads `assets/game.toml` and writes the SPEC §10.3 JSON
to stdout. Pipe it into Foglet's manifest directory. The
`--install-dir` MUST be the absolute path the door will live at on the
Foglet host.

```bash
fgk emit-manifest \
  --install-dir /srv/foglet/doors/murder-motel-smoke \
  > /tmp/murder-motel-smoke.manifest.json
```

### 5. Package the bundle

`fgk package` runs `cargo build --release`, then assembles the
deployable directory described in SPEC §10.4:

```text
dist/
  murder-motel-smoke   # release binary
  run.sh               # boring wrapper, executable
  manifest.json        # operator manifest (SPEC §10.3)
  assets/              # game.toml, maps, dialog, ...
```

```bash
fgk package --out dist/
```

`run.sh` is auditable by design — it `cd`s next to the binary, picks
the per-user save dir from `FOGLET_USER_ID` (with `FGK_SAVE_DIR`
override), and `exec`s the binary. It never interpolates user input
into a shell command.

### 6. Install as a Foglet door

On the Foglet host:

```bash
# Copy the bundle into the operator door tree (path must match
# whatever you passed to --install-dir in step 4).
sudo rsync -a dist/ /srv/foglet/doors/murder-motel-smoke/
sudo chmod +x /srv/foglet/doors/murder-motel-smoke/run.sh

# Install the manifest where the Foglet runner looks it up.
sudo install -m 0644 /tmp/murder-motel-smoke.manifest.json \
  /etc/foglet/manifests/murder-motel-smoke.json
```

Restart Foglet (or whatever it documents for manifest reloads) and
verify the door appears in the BBS Door Games list. The full
operator-side walkthrough — manifest placement, permissions, Foglet QA
expectations — lives in [`docs/foglet-install.md`](docs/foglet-install.md).

## Murder Motel v2 — two local users sharing a world

The Murder Motel example exercises every v2 shared-world primitive:
the player registry, daily turn ledger, append-only event log, the
`investigators` leaderboard, and the `motel_world_state` table that
records who first opened Room 7. The walkthrough below drives it as
two distinct local users so you can watch the shared world react.

> **Why two terminals.** A single `cargo run --example murder_motel`
> synthesises one anonymous local-dev player (`FogletContext.user_id =
> None`). To prove "another caller changed the world", v2 needs a
> second stable identity. The `FOGLET_*` env-fallback path (SPEC §5.1)
> lets you stand one up without standing up Foglet itself.

### 1. Pick a shared workspace

The example's world DB resolves to `world/world.sqlite` relative to
the *current working directory*. Both users must launch from the same
directory so they hit the same SQLite file. The repo root works:

```bash
cd path/to/foglet-game-kit-rs
ls world/ 2>/dev/null || mkdir world  # created automatically on first launch
```

If a stale `world.sqlite` from earlier experiments is present, delete
it (along with any `-wal` / `-shm` siblings) before starting — the
walkthrough assumes a fresh database.

### 2. Launch as Alice (terminal 1)

```bash
FOGLET_DOOR_ID=local-dev-motel \
FOGLET_USER_ID=alice \
FOGLET_USERNAME=Alice \
FOGLET_TERMINAL_WIDTH=$(tput cols) \
FOGLET_TERMINAL_HEIGHT=$(tput lines) \
cargo run --example murder_motel
```

`FOGLET_DOOR_ID` is the signal that opts into env-fallback mode —
without it the loader would synthesise the anonymous local-dev context
and Alice/Bob would land on the same player row. The `tput` calls
hand the runtime your real terminal size so the SPEC §7.1 size check
passes.

In Alice's session:

1. From the title screen press **Enter** → main menu → **New Game**.
2. Walk to the lobby's Room 7 door and unlock it (follow the on-screen
   hints). This writes `room_7_opened_at` and Alice's player id into
   `motel_world_state`, appends a `room_7_opened` event, and credits
   Alice on the `investigators` leaderboard when she takes a clue.
3. Quit with **q** to persist the save and close cleanly.

### 3. Launch as Bob (terminal 2)

In a second terminal, from the same directory:

```bash
FOGLET_DOOR_ID=local-dev-motel \
FOGLET_USER_ID=bob \
FOGLET_USERNAME=Bob \
FOGLET_TERMINAL_WIDTH=$(tput cols) \
FOGLET_TERMINAL_HEIGHT=$(tput lines) \
cargo run --example murder_motel
```

Bob will see:

- An **arrival banner** announcing that Alice opened Room 7 first
  (Task 12c). Room 7 itself is already unlocked because the world
  state survives across sessions.
- The **lobby bulletin / ledger screen** (Task 13d) listing Alice's
  recent `room_7_opened` and `clue_found` events.
- The **investigators leaderboard** (Task 13f) with Alice's score on
  it. Inspecting clues as Bob spends his own daily turns and adds
  Bob's row.

Quit Bob's session with **q**. Re-launch either user and the prior
state is still there — the SQLite file at `world/world.sqlite` is the
single source of truth.

### 4. Inspect the shared world (optional)

```bash
sqlite3 world/world.sqlite '
  SELECT handle, role, security_level FROM players;
  SELECT board_name, score FROM leaderboard_scores ORDER BY score DESC;
  SELECT kind, message FROM world_events ORDER BY id DESC LIMIT 5;
'
```

The `players` rows show that Alice and Bob landed on distinct stable
identities (SPEC_v2 §4.4). The leaderboard and event tables are the
same data the in-game screens read. See
[`docs/shared-world.md`](docs/shared-world.md) for the full schema,
backup procedure, and lock-recovery guidance.

> **Role/security display proof.** Role is carried only through the
> `FOGLET_DOOR_CONTEXT` JSON file (SPEC §5.1) — there is no `FOGLET_ROLE`
> env var. To drive the Task 13h advisory display, write a small JSON
> file with `{"door_id": "...", "user_id": "alice", "role": "sysop",
> "terminal_width": 80, "terminal_height": 24}` and export
> `FOGLET_DOOR_CONTEXT=/path/to/that.json` instead of the discrete
> `FOGLET_*` vars. The badge is for in-game flavour only — the kit
> never uses it for launch authorization (SPEC_v2 §4.5).

## Repository layout

```text
crates/
  foglet_game/   # library: terminal guard, runtime, screens, save, primitives
  fgk/           # CLI: new, emit-manifest, package
examples/
  murder_motel/  # acceptance fixture; runs as `cargo run --example murder_motel`
SPEC.md          # immutable contract — read this before changing the kit
CHECKLIST.md     # task ledger driven by the Ralph loop
DECISIONS.md     # ADRs for crate-budget and architecture deviations
docs/
  foglet-install.md     # operator-facing install walkthrough
  terminal-safety.md    # SPEC §13.1 evidence and recipes
```

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

The library forbids `unwrap()` outside tests and routes every TUI exit
through the terminal guard (normal quit, error, panic, Ctrl-C, resize).
Read SPEC §13.1 and [`docs/terminal-safety.md`](docs/terminal-safety.md)
before touching anything that owns the alternate screen.

## Limitations and known constraints

Per SPEC §15 these are documented rather than worked around. They
affect the *verification* surface, not the runtime contract.

- **Scaffolded projects depend on an unpublished crate.** `fgk new`
  emits a `Cargo.toml` with `foglet_game = "0.1"`, matching the kit's
  pre-1.0 plan. Until `foglet_game` is published to crates.io, point
  the dependency at a checkout — e.g. `foglet_game = { path =
  "../foglet-game-kit-rs/crates/foglet_game" }` — to compile a
  freshly-scaffolded project. The scaffold smoke test in
  `crates/fgk/tests/cli_new.rs` verifies file generation; running
  `cargo test` inside the scaffold is gated on the publish step.
- **`murder_motel` is a workspace example, not a standalone project.**
  Sources live at `examples/murder_motel/` and compile via
  `cargo run --example murder_motel`. To package it with `fgk
  package`, build the example first and pass `--binary` so the CLI
  skips its own `cargo build --release`:

  ```bash
  cargo build --release --example murder_motel
  fgk package \
    --project examples/murder_motel \
    --out dist/ \
    --binary target/release/examples/murder_motel
  ```

  Projects produced by `fgk new` build a regular release binary and
  do not need the `--binary` flag.
- **TUI smoke is manual.** SPEC §13.1 explicitly accepts manual
  evidence for the terminal-guard contract (raw mode + alternate
  screen + panic-hook restoration) because CI cannot drive a real
  terminal. The recipe and evidence trail live in
  [`docs/terminal-safety.md`](docs/terminal-safety.md).

## License

Dual-licensed under MIT or Apache-2.0, matching the wider Rust
ecosystem. See `LICENSE-MIT` / `LICENSE-APACHE` once added.
