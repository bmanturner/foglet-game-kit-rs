//! Lightweight service-layer context helpers.
//!
//! Screens receive [`crate::GameContext`] from the runtime. Game
//! services often need a smaller, test-friendly bundle: immutable game
//! config, Foglet session identity, and an injected [`DateProvider`].
//! `ServiceContext` provides that bundle without owning the world DB,
//! save state, or any game-specific service object.

use crate::config::{GameConfig, TurnsSection};
use crate::foglet::FogletContext;
use crate::turns::{DateProvider, LocalDate};

/// Borrowed context for game-authored service functions.
///
/// The type is intentionally lightweight. It composes the values that
/// make service calls deterministic and contextual:
///
/// - [`GameConfig`] for policy such as daily turn allowance.
/// - [`FogletContext`] for door/session/player identity.
/// - [`DateProvider`] for the service's definition of "today".
///
/// It does not replace the existing simple APIs. A small game can keep
/// passing `&FixedDateProvider` or another `DateProvider` directly to
/// `WorldDb::spend_turns`; this wrapper is for projects whose service
/// layer was otherwise growing three repeated parameters per method.
pub struct ServiceContext<'a, D: DateProvider + ?Sized> {
    /// Parsed game config.
    pub config: &'a GameConfig,
    /// Current Foglet session context.
    pub foglet: &'a FogletContext,
    /// Injected source of the local calendar date.
    pub date_provider: &'a D,
}

impl<'a, D: DateProvider + ?Sized> ServiceContext<'a, D> {
    /// Borrow config, Foglet context, and date provider into one
    /// service-call context.
    pub fn new(config: &'a GameConfig, foglet: &'a FogletContext, date_provider: &'a D) -> Self {
        Self {
            config,
            foglet,
            date_provider,
        }
    }

    /// Return the injected local date for this service call.
    ///
    /// Service code should prefer this over reading the host clock
    /// directly, especially when turn spending, travel costs, world
    /// ticks, or scan cooldowns need deterministic tests.
    pub fn today(&self) -> LocalDate {
        self.date_provider.today()
    }

    /// Borrow the configured turn policy, if the game opted into daily
    /// turns.
    pub fn turns(&self) -> Option<&'a TurnsSection> {
        self.config.turns.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceContext;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::players::PLAYERS_MIGRATION;
    use crate::turns::{FixedDateProvider, LocalDate, TURN_LEDGER_MIGRATION};
    use crate::world_db::WorldDb;

    #[test]
    fn service_context_threads_fixed_date_into_turn_spending() {
        let config = toml::from_str(
            r#"
[game]
title = "Service Test"
slug = "service-test"
description = "Service test fixture"
min_width = 80
min_height = 24
start_map = "start"
start_x = 1
start_y = 1

[turns]
daily_allowance = 10
carryover_max = 2
"#,
        )
        .expect("fixture config parses");
        let foglet = FogletContext {
            door_id: "service-test".to_string(),
            user_id: Some("user-42".to_string()),
            username: Some("Tester".to_string()),
            role: Some("user".to_string()),
            session_id: Some("session-1".to_string()),
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::LocalDev,
        };
        let date_provider =
            FixedDateProvider::new(LocalDate::parse("2026-05-13").expect("date parses"));
        let services = ServiceContext::new(&config, &foglet, &date_provider);

        let dir = tempfile::tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&TURN_LEDGER_MIGRATION)
            .expect("turn ledger migration applies");
        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["Tester"],
                |row| row.get(0),
            )
            .expect("fixture player inserts");
        let turns = services.turns().expect("fixture config enables turns");

        let row = world
            .spend_turns(
                player_id,
                3,
                turns.daily_allowance,
                turns.carryover_max,
                services.date_provider,
            )
            .expect("fixed-date spend succeeds");

        assert_eq!(services.today().as_str(), "2026-05-13");
        assert_eq!(row.local_date.as_str(), "2026-05-13");
        assert_eq!(row.balance, 7);
        assert_eq!(services.foglet.user_id.as_deref(), Some("user-42"));
    }
}
