//! Shared test fixtures used by every module's `#[cfg(test)] mod tests`.
//!
//! Pulled into a single module so a future tweak to the scaffold's
//! `assets/game.toml` or to the [`FogletContext`] schema only needs one
//! edit. Each domain test module imports the helpers it needs via
//! `use crate::test_support::*;`.

use foglet_game::{ContextSource, FogletContext, GameConfig};

use crate::GAME_TOML_PATH;

/// Load the scaffold's shipped `assets/game.toml`. Every test that needs
/// a [`GameConfig`] funnels through here so a config schema change
/// surfaces in one place.
pub fn fixture_config() -> GameConfig {
    GameConfig::load(GAME_TOML_PATH).expect("scaffold game.toml parses")
}

/// Build a deterministic [`FogletContext`] for tests that need to drive a
/// [`GameContext`](foglet_game::GameContext). The values mimic a local
/// dev session at the SPEC §7.1 floor (80×24).
pub fn fixture_context() -> FogletContext {
    FogletContext {
        door_id: "murder-motel".into(),
        user_id: Some("u-test".into()),
        username: Some("tester".into()),
        role: None,
        session_id: Some("s-test".into()),
        terminal_width: 80,
        terminal_height: 24,
        source: ContextSource::LocalDev,
    }
}
