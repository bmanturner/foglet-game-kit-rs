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
