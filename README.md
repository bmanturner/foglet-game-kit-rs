# foglet-game-kit-rs

A Rust authoring kit for [Foglet](https://github.com/bmanturner/foglet-bbs)
door games. It ships a library crate (`foglet_game`) for building the
game itself and a CLI (`fgk`) for scaffolding, packaging, and emitting
the Foglet operator manifest.

The kit targets Foglet's `:external_pty` runtime: each game compiles to
a single binary that Foglet launches under a PTY, with a documented
context handed in via `FOGLET_DOOR_CONTEXT` and `FOGLET_*` env vars.

## What you get

- **`foglet_game`** — terminal guard with panic-safe restoration, a
  screen-stack runtime, input normalization, save manager with atomic
  writes, ASCII map and YAML dialog primitives, and ratatui widgets.
  Includes higher-level authoring ergonomics: `SaveSlot<T>` (typed
  shared save handle with a dirty flag, `load_or_default`, and a
  `save_handler` closure for runtime persistence), `DialogScreen` (a
  `Screen` adapter over the YAML dialog runner that mirrors
  `PromptScreen<T>` and supports modal/compact layouts), modal layout
  helpers (`centred_rect`, `render_modal`, `render_hint_line`), and
  `Game::with_save_handler` so persistence runs inside the terminal
  guard's lifetime instead of a manual tail in `main`. See
  [`docs/save-and-state.md`](docs/save-and-state.md) and
  [`docs/dialog-screens.md`](docs/dialog-screens.md) for when to reach
  for each.

  The library also ships BBS-native async multiplayer primitives built
  on a per-game shared SQLite world DB: durable notices/mail (`Notice`,
  `WorldDb::send_notice` / `inbox` / `mark_read` / `archive_notice`),
  challenge lifecycles (`Challenge` with create/accept/decline/resolve/expire
  transitions), shared market listings (`MarketListing` with atomic,
  callback-rolled-back `buy_listing`), factions and shared goals
  (`Faction`, `SharedGoal`, `contribute_to_goal` with auto-completion
  at target), and bounty boards (`Bounty` with post/claim/complete/expire).
  Each primitive is opt-in via `[multiplayer]` in `game.toml` and stores
  state in the SQLite world DB — no live sockets, no background pollers,
  refresh-on-navigation only. See
  [`docs/async-multiplayer.md`](docs/async-multiplayer.md) for the
  mailbox-multiplayer model and the explicit no-real-time scope.

  Spatial graph primitives let games represent durable place graphs:
  directed place graphs (`Place`, `Route`), per-player presence and
  recall (`Presence`, `PlaceRecall`), owner-keyed inventory slots with
  atomic transfer (`InventorySlot`, `transfer`), and durable world
  ticks (`WorldTickTask`, `register_tick`, `run_due_ticks`). See
  [`docs/spatial.md`](docs/spatial.md),
  [`docs/presence-and-recall.md`](docs/presence-and-recall.md),
  [`docs/inventory.md`](docs/inventory.md), and
  [`docs/world-ticks.md`](docs/world-ticks.md).

  Workflow composition primitives handle higher-level game mechanics:
  generic Contracts, a unified Job Board, transactional Travel,
  policy-driven Inventory Capacity, an Event Log screen, and a
  feature-gated multi-user local test harness. See
  [`docs/contracts.md`](docs/contracts.md),
  [`docs/job-board.md`](docs/job-board.md),
  [`docs/travel.md`](docs/travel.md),
  [`docs/inventory-capacity.md`](docs/inventory-capacity.md),
  [`docs/event-log-screen.md`](docs/event-log-screen.md), and
  [`docs/test-support-multi-user.md`](docs/test-support-multi-user.md).
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
and the game slug, so it must be lowercase ASCII alphanumeric plus `-`.
The scaffold includes `Cargo.toml`, `src/main.rs`, an `assets/game.toml`,
a starter map, `.gitignore`, and a per-project README.

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

`emit-manifest` reads `assets/game.toml` and writes the operator manifest
JSON to stdout. Pipe it into Foglet's manifest directory. The
`--install-dir` MUST be the absolute path the door will live at on the
Foglet host.

```bash
fgk emit-manifest \
  --install-dir /srv/foglet/doors/murder-motel-smoke \
  > /tmp/murder-motel-smoke.manifest.json
```

### 5. Package the bundle

`fgk package` runs `cargo build --release`, then assembles the
deployable directory:

```text
dist/
  murder-motel-smoke   # release binary
  run.sh               # boring wrapper, executable
  manifest.json        # operator manifest
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

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
```

The library forbids `unwrap()` outside tests and routes every TUI exit
through the terminal guard (normal quit, error, panic, Ctrl-C, resize).
Read [`docs/terminal-safety.md`](docs/terminal-safety.md) before
touching anything that owns the alternate screen.

## Limitations and known constraints

These are documented rather than worked around. They affect the
*verification* surface, not the runtime contract.

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
- **TUI smoke is manual.** The terminal-guard contract (raw mode +
  alternate screen + panic-hook restoration) requires a real TTY that
  CI cannot drive. The recipe and evidence trail live in
  [`docs/terminal-safety.md`](docs/terminal-safety.md).

## License

Dual-licensed under MIT or Apache-2.0, matching the wider Rust
ecosystem. See `LICENSE-MIT` / `LICENSE-APACHE` once added.
