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

#[cfg(test)]
mod tests {
    //! Pin the v3 multiplayer config shape. Murder Motel is the SPEC §13
    //! acceptance fixture, so a regression here (toggle drift, agency
    //! slug typo, missing notice cap) silently breaks the example's
    //! contract with the rest of v3. Loading via [`fixture_config`]
    //! exercises the same path every other test uses, so we catch it.
    use super::fixture_config;

    #[test]
    fn game_toml_enables_every_v3_multiplayer_primitive() {
        let cfg = fixture_config();
        let mp = cfg
            .multiplayer
            .as_ref()
            .expect("scaffold opts into [multiplayer]");
        assert!(mp.notices, "notices toggle on for guestbook content");
        assert!(mp.challenges, "challenges toggle on for rival flow");
        assert!(mp.market, "market toggle on for lost-and-found");
        assert!(mp.factions, "factions toggle on for detective agencies");
        assert!(mp.bounties, "bounties toggle on for clue board");
        assert_eq!(
            mp.max_notice_body_chars, 1_000,
            "scaffold pins SPEC v3 §5.2 default cap"
        );
    }

    #[test]
    fn game_toml_seeds_both_detective_agencies() {
        let cfg = fixture_config();
        let slugs: Vec<&str> = cfg.factions.seed.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slugs, vec!["blue-desk", "red-room"]);
        let blue = &cfg.factions.seed[0];
        assert_eq!(blue.display_name, "Blue Desk Agency");
        assert!(!blue.description.is_empty());
        let red = &cfg.factions.seed[1];
        assert_eq!(red.display_name, "Red Room Agency");
        assert!(!red.description.is_empty());
    }
}
