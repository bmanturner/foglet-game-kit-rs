//! `map_projection` — player-scoped place and route visibility views.
//!
//! This module composes durable spatial, presence, and recall primitives
//! into a render-friendly read model without assuming a grid, genre, or
//! objective system. Games provide the visibility policy; the kit handles
//! player scoping, current-location detection, and route availability.

use std::collections::HashMap;

use thiserror::Error;

use crate::place_recall::{PlaceRecallError, PlaceRecallRecord};
use crate::presence::{PresenceError, PresenceRecord};
use crate::spatial::{Place, Route};
use crate::world_db::WorldDb;

/// Player-facing visibility bucket for one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceVisibility {
    /// The player's current presence location.
    Current,
    /// A place the player has personally recalled or visited.
    Seen,
    /// A policy-visible place that is safe to reveal.
    Known,
    /// A policy-visible place that should be presented as uncertain.
    Rumored,
    /// A place the policy does not want in the default projection.
    Hidden,
}

/// Game-authored place visibility policy.
///
/// The kit passes the durable place row, this player's recall row for
/// that place if one exists, and the current place id. The policy should
/// return `Hidden` for places that should not appear by default. The
/// projection helper always upgrades the current place to
/// [`PlaceVisibility::Current`] before consulting the policy so the
/// player cannot hide their own location accidentally.
pub trait PlaceVisibilityPolicy {
    /// Decide the visibility bucket for one place.
    fn visibility_for_place(
        &self,
        place: &Place,
        recall: Option<&PlaceRecallRecord>,
        current_place_id: i64,
    ) -> PlaceVisibility;
}

/// Projection options for hidden rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapProjectionOptions {
    /// Include hidden places in `places`.
    pub include_hidden_places: bool,
    /// Include policy-hidden routes in `routes`.
    pub include_hidden_routes: bool,
}

impl Default for MapProjectionOptions {
    fn default() -> Self {
        Self {
            include_hidden_places: false,
            include_hidden_routes: false,
        }
    }
}

/// Render-ready place row for map, atlas, star-chart, or room-list UIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceProjection {
    /// Durable place row.
    pub place: Place,
    /// Visibility bucket assigned for this player.
    pub visibility: PlaceVisibility,
    /// This player's recall row for the place, if any.
    pub recall: Option<PlaceRecallRecord>,
}

/// Player-facing route availability in a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteAvailability {
    /// The route leaves the player's current place.
    Outbound,
    /// The route is visible but does not leave the current place.
    Unavailable,
    /// At least one endpoint is hidden by policy.
    Hidden,
}

/// Render-ready directed route row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProjection {
    /// Durable route row.
    pub route: Route,
    /// Visibility assigned to the source place.
    pub from_visibility: PlaceVisibility,
    /// Visibility assigned to the destination place.
    pub to_visibility: PlaceVisibility,
    /// Availability relative to the player's current presence.
    pub availability: RouteAvailability,
}

/// Full player-scoped map projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerMapProjection {
    /// Current presence used as the route origin.
    pub current: PresenceRecord,
    /// Place rows visible under the selected options.
    pub places: Vec<PlaceProjection>,
    /// Route rows visible under the selected options.
    pub routes: Vec<RouteProjection>,
}

/// Errors produced while building map projections.
#[derive(Debug, Error)]
pub enum MapProjectionError {
    /// The player has no current presence row.
    #[error("player `{player_id}` has no current presence row")]
    MissingPresence {
        /// Player id being projected.
        player_id: i64,
    },
    /// Reading presence failed.
    #[error("presence lookup failed: {source}")]
    Presence {
        /// Underlying presence error.
        #[source]
        source: PresenceError,
    },
    /// Reading recall failed.
    #[error("place recall lookup failed: {source}")]
    Recall {
        /// Underlying recall error.
        #[source]
        source: PlaceRecallError,
    },
    /// Reading places or routes failed.
    #[error("map projection query failed: {source}")]
    Sqlite {
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
}

/// Build a player-scoped map projection with hidden rows omitted.
pub fn project_player_map(
    world: &WorldDb,
    player_id: i64,
    policy: &impl PlaceVisibilityPolicy,
) -> Result<PlayerMapProjection, MapProjectionError> {
    project_player_map_with_options(world, player_id, policy, MapProjectionOptions::default())
}

/// Build a player-scoped map projection using explicit hidden-row
/// options.
pub fn project_player_map_with_options(
    world: &WorldDb,
    player_id: i64,
    policy: &impl PlaceVisibilityPolicy,
    options: MapProjectionOptions,
) -> Result<PlayerMapProjection, MapProjectionError> {
    let current = world
        .get_presence(player_id)
        .map_err(|source| MapProjectionError::Presence { source })?
        .ok_or(MapProjectionError::MissingPresence { player_id })?;
    let recalls = world
        .recall_for_player(player_id)
        .map_err(|source| MapProjectionError::Recall { source })?;
    let recall_by_place: HashMap<i64, PlaceRecallRecord> = recalls
        .into_iter()
        .map(|recall| (recall.place_id, recall))
        .collect();

    let all_places = load_places(world.connection())?;
    let mut visibility_by_place = HashMap::new();
    let mut places = Vec::new();

    for place in all_places {
        let recall = recall_by_place.get(&place.id);
        let visibility = if place.id == current.place_id {
            PlaceVisibility::Current
        } else {
            policy.visibility_for_place(&place, recall, current.place_id)
        };
        visibility_by_place.insert(place.id, visibility);
        if visibility != PlaceVisibility::Hidden || options.include_hidden_places {
            places.push(PlaceProjection {
                place,
                visibility,
                recall: recall.cloned(),
            });
        }
    }

    let routes = load_routes(world.connection())?
        .into_iter()
        .filter_map(|route| {
            let from_visibility = *visibility_by_place
                .get(&route.from_place_id)
                .unwrap_or(&PlaceVisibility::Hidden);
            let to_visibility = *visibility_by_place
                .get(&route.to_place_id)
                .unwrap_or(&PlaceVisibility::Hidden);
            let endpoint_hidden = from_visibility == PlaceVisibility::Hidden
                || to_visibility == PlaceVisibility::Hidden;
            if endpoint_hidden && !options.include_hidden_routes {
                return None;
            }
            let availability = if endpoint_hidden {
                RouteAvailability::Hidden
            } else if route.from_place_id == current.place_id {
                RouteAvailability::Outbound
            } else {
                RouteAvailability::Unavailable
            };
            Some(RouteProjection {
                route,
                from_visibility,
                to_visibility,
                availability,
            })
        })
        .collect();

    Ok(PlayerMapProjection {
        current,
        places,
        routes,
    })
}

fn load_places(conn: &rusqlite::Connection) -> Result<Vec<Place>, MapProjectionError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, key, display_name, kind, metadata_json, created_at\n\
             FROM places\n\
             ORDER BY id ASC",
        )
        .map_err(|source| MapProjectionError::Sqlite { source })?;
    let places = stmt
        .query_map([], row_to_place)
        .map_err(|source| MapProjectionError::Sqlite { source })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| MapProjectionError::Sqlite { source })?;
    Ok(places)
}

fn load_routes(conn: &rusqlite::Connection) -> Result<Vec<Route>, MapProjectionError> {
    let mut stmt = conn
        .prepare(
            "SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at\n\
             FROM routes\n\
             ORDER BY id ASC",
        )
        .map_err(|source| MapProjectionError::Sqlite { source })?;
    let routes = stmt
        .query_map([], row_to_route)
        .map_err(|source| MapProjectionError::Sqlite { source })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| MapProjectionError::Sqlite { source })?;
    Ok(routes)
}

fn row_to_place(row: &rusqlite::Row<'_>) -> rusqlite::Result<Place> {
    Ok(Place {
        id: row.get(0)?,
        key: row.get(1)?,
        display_name: row.get(2)?,
        kind: row.get(3)?,
        metadata_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

fn row_to_route(row: &rusqlite::Row<'_>) -> rusqlite::Result<Route> {
    Ok(Route {
        from_place_id: row.get(0)?,
        to_place_id: row.get(1)?,
        id: row.get(2)?,
        kind: row.get(3)?,
        requirements_json: row.get(4)?,
        metadata_json: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        project_player_map, project_player_map_with_options, MapProjectionOptions, PlaceVisibility,
        PlaceVisibilityPolicy, RouteAvailability,
    };
    use crate::place_recall::PLACE_RECALL_MIGRATION;
    use crate::players::PLAYERS_MIGRATION;
    use crate::presence::PRESENCE_MIGRATION;
    use crate::spatial::{Place, PLACES_MIGRATION, ROUTES_MIGRATION};
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    struct TestPolicy;

    impl PlaceVisibilityPolicy for TestPolicy {
        fn visibility_for_place(
            &self,
            place: &Place,
            recall: Option<&crate::PlaceRecallRecord>,
            _current_place_id: i64,
        ) -> PlaceVisibility {
            if recall.is_some() {
                return PlaceVisibility::Seen;
            }
            if place.kind == "market" {
                return PlaceVisibility::Known;
            }
            if place.metadata_json.as_deref() == Some(r#"{"rumored":true}"#) {
                return PlaceVisibility::Rumored;
            }
            PlaceVisibility::Hidden
        }
    }

    fn world_with_map() -> WorldDb {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&PLACES_MIGRATION)
            .expect("places migration applies");
        world
            .apply_migration(&ROUTES_MIGRATION)
            .expect("routes migration applies");
        world
            .apply_migration(&PRESENCE_MIGRATION)
            .expect("presence migration applies");
        world
            .apply_migration(&PLACE_RECALL_MIGRATION)
            .expect("place recall migration applies");
        world
    }

    #[test]
    fn projection_combines_current_recalled_known_rumored_and_hidden_places() {
        let world = world_with_map();
        world
            .connection()
            .execute(
                "INSERT INTO players (id, handle) VALUES (?1, ?2)",
                rusqlite::params![1_i64, "alice"],
            )
            .expect("player inserts");
        let hub = world
            .insert_place("hub", "Hub", "room", None)
            .expect("hub inserts");
        let seen = world
            .insert_place("seen", "Seen Room", "room", None)
            .expect("seen place inserts");
        let _market = world
            .insert_place("market", "Market", "market", None)
            .expect("market inserts");
        let _rumor = world
            .insert_place("rumor", "Rumor", "room", Some(r#"{"rumored":true}"#))
            .expect("rumor inserts");
        let hidden = world
            .insert_place("hidden", "Hidden", "vault", None)
            .expect("hidden inserts");

        world
            .set_presence(1, hub.id, None)
            .expect("presence inserts");
        world
            .touch_recall(1, seen.id, Some(r#"{"visited":true}"#))
            .expect("alice recall inserts");
        world
            .connection()
            .execute(
                "INSERT INTO players (id, handle) VALUES (?1, ?2)",
                rusqlite::params![2_i64, "bob"],
            )
            .expect("second player inserts");
        world
            .touch_recall(2, hidden.id, Some(r#"{"visited":true}"#))
            .expect("bob recall inserts");

        let projection = project_player_map(&world, 1, &TestPolicy).expect("projection succeeds");

        assert_eq!(projection.current.place_id, hub.id);
        let visible: Vec<_> = projection
            .places
            .iter()
            .map(|place| (place.place.key.as_str(), place.visibility))
            .collect();
        assert_eq!(
            visible,
            vec![
                ("hub", PlaceVisibility::Current),
                ("seen", PlaceVisibility::Seen),
                ("market", PlaceVisibility::Known),
                ("rumor", PlaceVisibility::Rumored),
            ]
        );
        assert!(
            projection
                .places
                .iter()
                .all(|place| place.place.id != hidden.id),
            "hidden places and another player's recall should not leak"
        );
    }

    #[test]
    fn projection_marks_routes_outbound_unavailable_and_policy_hidden() {
        let world = world_with_map();
        world
            .connection()
            .execute(
                "INSERT INTO players (id, handle) VALUES (?1, ?2)",
                rusqlite::params![1_i64, "alice"],
            )
            .expect("player inserts");
        let hub = world.insert_place("hub", "Hub", "room", None).expect("hub");
        let seen = world
            .insert_place("seen", "Seen Room", "room", None)
            .expect("seen");
        let market = world
            .insert_place("market", "Market", "market", None)
            .expect("market");
        let hidden = world
            .insert_place("hidden", "Hidden", "vault", None)
            .expect("hidden");
        let outbound = world
            .create_route(hub.id, market.id, "road", None, None)
            .expect("outbound route inserts");
        let unavailable = world
            .create_route(seen.id, hub.id, "return", None, None)
            .expect("unavailable route inserts");
        let hidden_route = world
            .create_route(hub.id, hidden.id, "secret", None, None)
            .expect("hidden route inserts");

        world
            .set_presence(1, hub.id, None)
            .expect("presence inserts");
        world
            .touch_recall(1, seen.id, None)
            .expect("recall inserts");

        let default_projection =
            project_player_map(&world, 1, &TestPolicy).expect("default projection succeeds");
        let default_routes: Vec<_> = default_projection
            .routes
            .iter()
            .map(|route| (route.route.id, route.availability))
            .collect();
        assert_eq!(
            default_routes,
            vec![
                (outbound.id, RouteAvailability::Outbound),
                (unavailable.id, RouteAvailability::Unavailable),
            ]
        );

        let with_hidden = project_player_map_with_options(
            &world,
            1,
            &TestPolicy,
            MapProjectionOptions {
                include_hidden_places: false,
                include_hidden_routes: true,
            },
        )
        .expect("hidden-route projection succeeds");
        let hidden_projection = with_hidden
            .routes
            .iter()
            .find(|route| route.route.id == hidden_route.id)
            .expect("hidden route included by option");
        assert_eq!(hidden_projection.availability, RouteAvailability::Hidden);
        assert_eq!(hidden_projection.to_visibility, PlaceVisibility::Hidden);
    }
}
