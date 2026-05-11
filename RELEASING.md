# Releasing foglet-game-kit-rs

## Scope

This workspace publishes two crates:

- `foglet_game`
- `fgk`

The `murder_motel` sample game is repository-only. It is a local
fixture and example checkout, not part of the published crates.

## Preflight

1. Confirm you are logged in for crates.io publishing:
   `cargo login`
2. Verify the working tree only contains intentional release changes.
3. Choose the release version and update workspace/package versions if
   needed.

## Verification

Run these from the workspace root:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
cargo package -p foglet_game
cargo package -p fgk
```

## Publish order

Publish the library first, then the CLI:

```sh
cargo publish -p foglet_game
cargo publish -p fgk
```

If crates.io has not indexed `foglet_game` yet, wait a minute and retry
the `fgk` publish.

## Post-publish smoke

```sh
cargo install fgk
fgk --version
fgk new /tmp/fgk-smoke
cd /tmp/fgk-smoke
cargo run
```

Optional local packaging smoke:

```sh
fgk package --out dist/
```
