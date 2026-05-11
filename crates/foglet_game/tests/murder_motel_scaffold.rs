//! Integration test for the murder motel example scaffold.
//!
//! The example's own `#[cfg(test)] mod tests` covers the `TitleScreen`
//! keyboard contract, but those run only under `cargo test --examples`
//! / `--all-targets`. This test sits inside the library crate's
//! `tests/` directory so it executes under the bare
//! `cargo test --workspace` gate, guaranteeing the scaffold's on-disk
//! assets stay parseable as the example grows.
//!
//! What we assert:
//! 1. `assets/game.toml` parses through `GameConfig::load` (the same
//!    code path the example's `main` uses).
//! 2. The configured slug, title, and minimum size satisfy the
//!    acceptance criteria (named "Murder Motel", `min_width >= 80`,
//!    `min_height >= 24`).
//! 3. The save strategy is `per_foglet_user` so the per-user
//!    persistence path has the right scaffold from day one.
//! 4. The example target is registered on this crate so
//!    `cargo run --example murder_motel` resolves.
//!
//! Item 4 is the easy regression to miss when reorganising the
//! workspace, so we assert it indirectly: read this crate's
//! `Cargo.toml` and look for the `[[example]]` entry. Reading manifest
//! text via `env!("CARGO_MANIFEST_DIR")` keeps the test independent of
//! the working directory `cargo test` was invoked from.

use std::path::PathBuf;

use foglet_game::{GameConfig, LeaderboardSort, SaveStrategy, TurnReset};

/// Resolve a path relative to this crate's manifest dir. Examples and
/// the test runner can be invoked from anywhere; this keeps every disk
/// lookup deterministic.
fn workspace_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

#[test]
fn scaffold_game_toml_parses_into_game_config() {
    let path = workspace_path("../../examples/murder_motel/assets/game.toml");
    let cfg = GameConfig::load(&path)
        .unwrap_or_else(|err| panic!("scaffold game.toml at {path:?} failed to load: {err}"));

    assert_eq!(
        cfg.game.slug, "murder-motel",
        "slug feeds the Foglet manifest path; door must be named `murder-motel`"
    );
    assert_eq!(
        cfg.game.title, "Murder Motel",
        "title shown on the Title screen; matches the example's HEADING constant"
    );
    assert!(
        cfg.game.min_width >= 80 && cfg.game.min_height >= 24,
        "minimum terminal size must be at least 80x24 \
         (got {}x{})",
        cfg.game.min_width,
        cfg.game.min_height
    );
    assert_eq!(
        cfg.save.strategy,
        SaveStrategy::PerFogletUser,
        "save strategy must be per-user; the scaffold must opt in from day one"
    );
}

#[test]
fn scaffold_game_toml_enables_shared_world_sections() {
    // We assert the *intent* of each section (world enabled, turn
    // ledger present, `investigators` board registered) rather than
    // every default value — the config layer's own tests cover
    // defaulting, and re-asserting them here would just couple the
    // fixture to schema details.
    let path = workspace_path("../../examples/murder_motel/assets/game.toml");
    let cfg = GameConfig::load(&path)
        .unwrap_or_else(|err| panic!("scaffold game.toml at {path:?} failed to load: {err}"));

    assert!(
        cfg.world.enabled,
        "[world].enabled must be true so the shared-world DB can be opened"
    );

    let turns = cfg
        .turns
        .as_ref()
        .expect("[turns] section is required so the daily ledger has an allowance");
    assert!(
        turns.daily_allowance > 0,
        "[turns].daily_allowance must be > 0; the config validator rejects zero, \
         but we restate it here so a future edit that lowers the value is caught \
         by this fixture-level assertion before it reaches the runtime"
    );
    assert_eq!(
        turns.reset,
        TurnReset::LocalMidnight,
        "pinning `local_midnight` here flags any silent reset change while the \
         closed enum is still narrow"
    );

    let investigators = cfg
        .leaderboards
        .iter()
        .find(|board| board.name == "investigators")
        .expect("the `investigators` leaderboard must be registered in the fixture");
    assert_eq!(
        investigators.sort,
        LeaderboardSort::Desc,
        "Murder Motel ranks highest score first; pinning `desc` guards against a \
         future edit flipping the direction"
    );
}

#[test]
fn example_target_is_registered_on_this_crate() {
    // Reading our own Cargo.toml as text rather than parsing keeps this
    // test free of `toml` features the rest of the test suite doesn't
    // pull in. The exact assertions below are deliberately structural,
    // not stringly-typed: we look for the example name and the path
    // that points at the workspace-root scaffold.
    let manifest = std::fs::read_to_string(workspace_path("Cargo.toml"))
        .expect("foglet_game/Cargo.toml is readable from its own manifest dir");
    assert!(
        manifest.contains("[[example]]"),
        "expected an [[example]] entry registering murder_motel; \
         missing from foglet_game/Cargo.toml"
    );
    assert!(
        manifest.contains(r#"name = "murder_motel""#),
        "example name must be exactly `murder_motel` so \
         `cargo run --example murder_motel` resolves"
    );
    assert!(
        manifest.contains("examples/murder_motel/src/main.rs"),
        "example path must point at the workspace-root scaffold so the \
         example and the on-disk project share one source of truth"
    );
}
