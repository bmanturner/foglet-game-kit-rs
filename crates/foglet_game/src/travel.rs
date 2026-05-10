//! `travel` — transactional movement request and result contracts for v5.
//!
//! This module defines the shape of one travel attempt before the
//! transaction helper itself lands:
//!
//! - In a **space exploration** game, the request can represent a
//!   captain moving between two docks with a fuel-cost callback.
//! - In a **dungeon crawler**, the same request can represent a player
//!   crossing from one chamber to another with a trap-validation callback.
//!
//! The primitive is intentionally genre-neutral and does not prescribe
//! turn costs, currency, hazards, or encounter policy.

use std::fmt;

use thiserror::Error;

use crate::events;
use crate::inventory::InventoryError;
use crate::presence::PresenceRecord;
use crate::spatial::Route;
use crate::world_db::WorldDb;

/// Callback type used to validate a route before movement mutates presence.
///
/// The callback runs during travel orchestration and can reject movement
/// using a typed [`TravelError`].
pub type TravelValidateCallback<'a> =
    Box<dyn FnMut(&PresenceRecord, &Route) -> Result<(), TravelError> + 'a>;

/// Callback type used to charge game-defined movement costs.
///
/// This keeps economics out of the kit while still letting travel
/// orchestration enforce rollback-on-error semantics in a later task.
pub type TravelChargeCostCallback<'a> =
    Box<dyn FnMut(&PresenceRecord, &Route) -> Result<(), TravelError> + 'a>;

/// Callback type that optionally produces an event payload for a movement.
///
/// Returning `None` means "do not append an event row for this move".
pub type TravelAppendEventCallback<'a> =
    Box<dyn FnMut(&PresenceRecord, &Route) -> Option<TravelEventDraft> + 'a>;

/// Event payload shape emitted by an optional travel `append_event` callback.
///
/// The structure intentionally mirrors v2 event append fields without
/// prescribing message text:
///
/// - A **space exploration** game can emit `"dock_arrival"` events.
/// - A **town simulation** game can emit `"district_entered"` events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TravelEventDraft {
    /// Event kind key written to `world_events.kind`.
    pub kind: String,
    /// Event message text written to `world_events.message`.
    pub message: String,
    /// Optional opaque metadata payload for game-defined event details.
    pub metadata_json: Option<String>,
}

/// Input payload for one travel attempt.
///
/// `TravelRequest` bundles identity, destination, route resolution hints,
/// and caller-owned callbacks into one explicit unit so travel orchestration
/// can execute the documented step order atomically.
pub struct TravelRequest<'a> {
    /// Player id whose current presence will be moved.
    pub player_id: i64,
    /// Destination place id to move into.
    pub dest_place_id: i64,
    /// Optional explicit route id. When `None`, the helper resolves one
    /// unique outbound route to `dest_place_id`.
    pub route_id: Option<i64>,
    /// Callback run before cost charging to validate caller-defined rules.
    pub validate: TravelValidateCallback<'a>,
    /// Callback run after validation to charge caller-defined travel cost.
    pub charge_cost: TravelChargeCostCallback<'a>,
    /// Whether travel should touch place-recall for the destination.
    pub touch_recall: bool,
    /// Optional callback that can synthesize one event payload.
    pub append_event: Option<TravelAppendEventCallback<'a>>,
}

impl<'a> TravelRequest<'a> {
    /// Construct a request with no explicit route and no-op cost/validation.
    ///
    /// Defaults:
    ///
    /// - `route_id = None` (resolve uniquely by adjacency),
    /// - `touch_recall = true`,
    /// - `append_event = None`.
    #[must_use]
    pub fn new(player_id: i64, dest_place_id: i64) -> Self {
        Self {
            player_id,
            dest_place_id,
            route_id: None,
            validate: Box::new(|_, _| Ok(())),
            charge_cost: Box::new(|_, _| Ok(())),
            touch_recall: true,
            append_event: None,
        }
    }

    /// Set an explicit route id for this request.
    #[must_use]
    pub fn with_route_id(mut self, route_id: i64) -> Self {
        self.route_id = Some(route_id);
        self
    }

    /// Override whether recall should be touched on successful movement.
    #[must_use]
    pub fn with_touch_recall(mut self, touch_recall: bool) -> Self {
        self.touch_recall = touch_recall;
        self
    }

    /// Install a custom validation callback.
    #[must_use]
    pub fn with_validate<F>(mut self, validate: F) -> Self
    where
        F: FnMut(&PresenceRecord, &Route) -> Result<(), TravelError> + 'a,
    {
        self.validate = Box::new(validate);
        self
    }

    /// Install a custom cost callback.
    #[must_use]
    pub fn with_charge_cost<F>(mut self, charge_cost: F) -> Self
    where
        F: FnMut(&PresenceRecord, &Route) -> Result<(), TravelError> + 'a,
    {
        self.charge_cost = Box::new(charge_cost);
        self
    }

    /// Install an optional event-payload callback.
    #[must_use]
    pub fn with_append_event<F>(mut self, append_event: F) -> Self
    where
        F: FnMut(&PresenceRecord, &Route) -> Option<TravelEventDraft> + 'a,
    {
        self.append_event = Some(Box::new(append_event));
        self
    }
}

impl fmt::Debug for TravelRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TravelRequest")
            .field("player_id", &self.player_id)
            .field("dest_place_id", &self.dest_place_id)
            .field("route_id", &self.route_id)
            .field("touch_recall", &self.touch_recall)
            .field("has_append_event", &self.append_event.is_some())
            .finish()
    }
}

/// Output payload from a successful travel transaction.
///
/// This row-shaped result captures the committed transition details so
/// callers can update UI and follow-up gameplay state without re-querying
/// multiple tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TravelResult {
    /// Previous place id before movement.
    pub from_place_id: i64,
    /// Destination place id after movement.
    pub to_place_id: i64,
    /// Route id used for this movement.
    pub route_id: i64,
    /// SQLite timestamp written by the resulting presence mutation.
    pub entered_at: String,
    /// Optional `world_events.id` inserted by travel event append.
    pub event_id: Option<i64>,
}

/// Typed failures for travel orchestration.
///
/// Variants stay explicit so callers can branch on user-facing causes
/// (`NoRoute`, `AmbiguousRoute`, `ValidationFailed`, etc.) rather than
/// parsing stringly SQL diagnostics.
#[derive(Debug, Error)]
pub enum TravelError {
    /// The target player has no current presence row to move from.
    #[error("player `{player_id}` has no presence row")]
    NoPresence {
        /// Caller-supplied player id.
        player_id: i64,
    },
    /// No matching outbound route exists for this request.
    #[error(
        "no route for player `{player_id}` from place `{from_place_id}` to `{to_place_id}`{route_suffix}"
    )]
    NoRoute {
        /// Caller-supplied player id.
        player_id: i64,
        /// Origin place id read from current presence.
        from_place_id: i64,
        /// Requested destination place id.
        to_place_id: i64,
        /// Optional explicit route id that failed to resolve.
        route_id: Option<i64>,
        /// Render helper included in the error string.
        route_suffix: String,
    },
    /// More than one outbound route matched while `route_id` was omitted.
    #[error(
        "ambiguous route for player `{player_id}` from place `{from_place_id}` to `{to_place_id}`; matching route ids: {matching_route_ids:?}"
    )]
    AmbiguousRoute {
        /// Caller-supplied player id.
        player_id: i64,
        /// Origin place id read from current presence.
        from_place_id: i64,
        /// Requested destination place id.
        to_place_id: i64,
        /// Route ids that matched this movement request.
        matching_route_ids: Vec<i64>,
    },
    /// Game-supplied validation callback rejected movement.
    #[error("travel validation rejected player `{player_id}` on route `{route_id}`: {reason}")]
    ValidationFailed {
        /// Caller-supplied player id.
        player_id: i64,
        /// Route id that was being validated.
        route_id: i64,
        /// Game-authored explanation surfaced to callers.
        reason: String,
    },
    /// Game-supplied cost callback rejected movement.
    #[error("travel cost charge failed for player `{player_id}` on route `{route_id}`: {reason}")]
    CostFailed {
        /// Caller-supplied player id.
        player_id: i64,
        /// Route id whose cost charge failed.
        route_id: i64,
        /// Game-authored explanation surfaced to callers.
        reason: String,
    },
    /// Underlying v4 inventory transfer/debit call failed.
    #[error("travel inventory mutation failed: {source}")]
    InventoryError {
        /// Wrapped inventory error source.
        #[source]
        source: Box<InventoryError>,
    },
    /// Underlying SQLite work failed while reading or mutating travel state.
    #[error("travel database operation failed during {step}: {source}")]
    Database {
        /// Human-readable phase label for the failed SQL step.
        step: &'static str,
        /// Underlying SQLite error.
        #[source]
        source: rusqlite::Error,
    },
}

impl TravelError {
    /// Helper for constructing [`Self::NoRoute`] with the standard optional
    /// route suffix.
    #[must_use]
    pub fn no_route(
        player_id: i64,
        from_place_id: i64,
        to_place_id: i64,
        route_id: Option<i64>,
    ) -> Self {
        let route_suffix = route_id
            .map(|id| format!(" (requested route `{id}`)"))
            .unwrap_or_default();
        Self::NoRoute {
            player_id,
            from_place_id,
            to_place_id,
            route_id,
            route_suffix,
        }
    }
}

impl From<InventoryError> for TravelError {
    fn from(source: InventoryError) -> Self {
        Self::InventoryError {
            source: Box::new(source),
        }
    }
}

impl WorldDb {
    /// Execute one travel request as a single SQLite transaction.
    ///
    /// The helper follows the v5 order exactly: read current presence,
    /// resolve the route, run validation, charge cost, move presence,
    /// optionally touch recall, and optionally append a world event.
    /// Any error returns before commit, so SQLite rolls back every
    /// mutation performed earlier in the helper.
    pub fn travel(&mut self, mut req: TravelRequest<'_>) -> Result<TravelResult, TravelError> {
        let tx = self
            .connection_mut()
            .transaction()
            .map_err(|source| TravelError::Database {
                step: "begin transaction",
                source,
            })?;

        let presence = current_presence(&tx, req.player_id)?;
        let route = resolve_route(
            &tx,
            req.player_id,
            presence.place_id,
            req.dest_place_id,
            req.route_id,
        )?;

        (req.validate)(&presence, &route).map_err(|error| match error {
            TravelError::ValidationFailed { .. } => error,
            other => TravelError::ValidationFailed {
                player_id: req.player_id,
                route_id: route.id,
                reason: other.to_string(),
            },
        })?;

        (req.charge_cost)(&presence, &route).map_err(|error| match error {
            TravelError::CostFailed { .. } | TravelError::InventoryError { .. } => error,
            other => TravelError::CostFailed {
                player_id: req.player_id,
                route_id: route.id,
                reason: other.to_string(),
            },
        })?;

        let moved = move_presence(&tx, req.player_id, req.dest_place_id)?;

        if req.touch_recall && table_exists(&tx, "place_recall")? {
            touch_recall(&tx, req.player_id, req.dest_place_id)?;
        }

        let event_id = match req.append_event.as_mut() {
            Some(append_event) if table_exists(&tx, "world_events")? => {
                append_event(&presence, &route)
                    .map(|draft| {
                        events::append_event_on(
                            &tx,
                            &draft.kind,
                            Some(req.player_id),
                            &draft.message,
                            draft.metadata_json.as_deref(),
                        )
                        .map(|event| event.id)
                        .map_err(|source| TravelError::Database {
                            step: "append travel event",
                            source: event_error_to_sqlite(source),
                        })
                    })
                    .transpose()?
            }
            _ => None,
        };

        let result = TravelResult {
            from_place_id: presence.place_id,
            to_place_id: moved.place_id,
            route_id: route.id,
            entered_at: moved.entered_at,
            event_id,
        };

        tx.commit().map_err(|source| TravelError::Database {
            step: "commit travel",
            source,
        })?;

        Ok(result)
    }
}

fn current_presence(
    conn: &rusqlite::Connection,
    player_id: i64,
) -> Result<PresenceRecord, TravelError> {
    const SQL: &str = "\
SELECT player_id, place_id, entered_at, metadata_json\n\
FROM presence\n\
WHERE player_id = ?1";

    match conn.query_row(SQL, rusqlite::params![player_id], row_to_presence) {
        Ok(presence) => Ok(presence),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(TravelError::NoPresence { player_id }),
        Err(source) => Err(TravelError::Database {
            step: "read current presence",
            source,
        }),
    }
}

fn resolve_route(
    conn: &rusqlite::Connection,
    player_id: i64,
    from_place_id: i64,
    to_place_id: i64,
    route_id: Option<i64>,
) -> Result<Route, TravelError> {
    match route_id {
        Some(id) => explicit_route(conn, player_id, from_place_id, to_place_id, id),
        None => unique_route(conn, player_id, from_place_id, to_place_id),
    }
}

fn explicit_route(
    conn: &rusqlite::Connection,
    player_id: i64,
    from_place_id: i64,
    to_place_id: i64,
    route_id: i64,
) -> Result<Route, TravelError> {
    const SQL: &str = "\
SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at\n\
FROM routes\n\
WHERE id = ?1 AND from_place_id = ?2 AND to_place_id = ?3";

    match conn.query_row(
        SQL,
        rusqlite::params![route_id, from_place_id, to_place_id],
        row_to_route,
    ) {
        Ok(route) => Ok(route),
        Err(rusqlite::Error::QueryReturnedNoRows) => Err(TravelError::no_route(
            player_id,
            from_place_id,
            to_place_id,
            Some(route_id),
        )),
        Err(source) => Err(TravelError::Database {
            step: "resolve explicit route",
            source,
        }),
    }
}

fn unique_route(
    conn: &rusqlite::Connection,
    player_id: i64,
    from_place_id: i64,
    to_place_id: i64,
) -> Result<Route, TravelError> {
    const SQL: &str = "\
SELECT from_place_id, to_place_id, id, kind, requirements_json, metadata_json, created_at\n\
FROM routes\n\
WHERE from_place_id = ?1 AND to_place_id = ?2\n\
ORDER BY id ASC";

    let mut stmt = conn.prepare(SQL).map_err(|source| TravelError::Database {
        step: "resolve unique route",
        source,
    })?;
    let routes = stmt
        .query_map(rusqlite::params![from_place_id, to_place_id], row_to_route)
        .map_err(|source| TravelError::Database {
            step: "resolve unique route",
            source,
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| TravelError::Database {
            step: "resolve unique route",
            source,
        })?;

    match routes.as_slice() {
        [] => Err(TravelError::no_route(
            player_id,
            from_place_id,
            to_place_id,
            None,
        )),
        [route] => Ok(route.clone()),
        routes => Err(TravelError::AmbiguousRoute {
            player_id,
            from_place_id,
            to_place_id,
            matching_route_ids: routes.iter().map(|route| route.id).collect(),
        }),
    }
}

fn move_presence(
    conn: &rusqlite::Connection,
    player_id: i64,
    dest_place_id: i64,
) -> Result<PresenceRecord, TravelError> {
    const SQL: &str = "\
UPDATE presence\n\
SET place_id = ?2,\n\
    entered_at = CURRENT_TIMESTAMP\n\
WHERE player_id = ?1\n\
RETURNING player_id, place_id, entered_at, metadata_json";

    conn.query_row(
        SQL,
        rusqlite::params![player_id, dest_place_id],
        row_to_presence,
    )
    .map_err(|source| TravelError::Database {
        step: "move presence",
        source,
    })
}

fn touch_recall(
    conn: &rusqlite::Connection,
    player_id: i64,
    place_id: i64,
) -> Result<(), TravelError> {
    const SQL: &str = "\
INSERT INTO place_recall (player_id, place_id, first_seen_at, last_seen_at, snapshot_json)\n\
VALUES (?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, NULL)\n\
ON CONFLICT(player_id, place_id) DO UPDATE\n\
SET last_seen_at = CURRENT_TIMESTAMP";

    conn.execute(SQL, rusqlite::params![player_id, place_id])
        .map(|_| ())
        .map_err(|source| TravelError::Database {
            step: "touch place recall",
            source,
        })
}

fn table_exists(conn: &rusqlite::Connection, table_name: &str) -> Result<bool, TravelError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        rusqlite::params![table_name],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|source| TravelError::Database {
        step: "check optional table",
        source,
    })
}

fn row_to_presence(row: &rusqlite::Row<'_>) -> rusqlite::Result<PresenceRecord> {
    Ok(PresenceRecord {
        player_id: row.get(0)?,
        place_id: row.get(1)?,
        entered_at: row.get(2)?,
        metadata_json: row.get(3)?,
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

fn event_error_to_sqlite(error: events::EventError) -> rusqlite::Error {
    match error {
        events::EventError::Sqlite { source } => source,
        other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::{TravelError, TravelEventDraft, TravelRequest};
    use crate::events::WORLD_EVENTS_MIGRATION;
    use crate::inventory::InventoryError;
    use crate::place_recall::PLACE_RECALL_MIGRATION;
    use crate::players::PLAYERS_MIGRATION;
    use crate::presence::PresenceRecord;
    use crate::presence::PRESENCE_MIGRATION;
    use crate::spatial::{Route, PLACES_MIGRATION, ROUTES_MIGRATION};
    use crate::world_db::WorldDb;
    use tempfile::tempdir;

    fn sample_presence(place_id: i64) -> PresenceRecord {
        PresenceRecord {
            player_id: 9,
            place_id,
            entered_at: "2030-01-01T00:00:00Z".to_string(),
            metadata_json: None,
        }
    }

    fn sample_route() -> Route {
        Route {
            id: 41,
            from_place_id: 5,
            to_place_id: 8,
            kind: "passage".to_string(),
            requirements_json: None,
            metadata_json: None,
            created_at: "2030-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn request_defaults_to_unique_route_resolution_and_touch_recall() {
        let mut request = TravelRequest::new(7, 12);
        assert_eq!(request.player_id, 7);
        assert_eq!(request.dest_place_id, 12);
        assert_eq!(request.route_id, None);
        assert!(request.touch_recall);
        assert!(request.append_event.is_none());

        let presence = sample_presence(5);
        let route = sample_route();
        (request.validate)(&presence, &route).expect("default validation should succeed");
        (request.charge_cost)(&presence, &route).expect("default cost callback should succeed");
    }

    #[test]
    fn request_builder_overrides_route_and_touch_recall() {
        let request = TravelRequest::new(3, 4)
            .with_route_id(99)
            .with_touch_recall(false);
        assert_eq!(request.route_id, Some(99));
        assert!(!request.touch_recall);
    }

    #[test]
    fn travel_error_variants_cover_specified_failure_buckets() {
        let errors = [
            TravelError::NoPresence { player_id: 1 },
            TravelError::no_route(1, 10, 20, Some(44)),
            TravelError::AmbiguousRoute {
                player_id: 1,
                from_place_id: 10,
                to_place_id: 20,
                matching_route_ids: vec![44, 45],
            },
            TravelError::ValidationFailed {
                player_id: 1,
                route_id: 44,
                reason: "locked gate".to_string(),
            },
            TravelError::CostFailed {
                player_id: 1,
                route_id: 44,
                reason: "not enough rations".to_string(),
            },
            TravelError::InventoryError {
                source: Box::new(InventoryError::MissingSourceSlot {
                    owner_kind: "player".to_string(),
                    owner_id: 1,
                    item_key: "ration".to_string(),
                }),
            },
        ];

        assert_eq!(errors.len(), 6);
    }

    #[test]
    fn travel_success_updates_presence_touches_recall_and_appends_event() {
        let TravelFixture {
            mut world,
            player_id,
            origin_id,
            destination_id,
            route,
            ..
        } = setup_travel_fixture();

        let result = world
            .travel(
                TravelRequest::new(player_id, destination_id).with_append_event(
                    |presence, route| {
                        Some(TravelEventDraft {
                            kind: "arrived".to_string(),
                            message: format!(
                                "player {} moved from {} to {}",
                                presence.player_id, route.from_place_id, route.to_place_id
                            ),
                            metadata_json: Some(r#"{"via":"hall"}"#.to_string()),
                        })
                    },
                ),
            )
            .expect("travel succeeds");

        let loaded_presence = world
            .get_presence(player_id)
            .expect("presence reads")
            .expect("presence row exists");
        let recall = world
            .recall_for_player(player_id)
            .expect("recall reads after travel");
        let events = world
            .player_events(player_id, 10)
            .expect("player events read after travel");

        assert_eq!(result.from_place_id, origin_id);
        assert_eq!(result.to_place_id, destination_id);
        assert_eq!(result.route_id, route.id);
        assert_eq!(result.event_id, events.first().map(|event| event.id));
        assert_eq!(loaded_presence.place_id, destination_id);
        assert!(
            recall
                .iter()
                .any(|row| row.player_id == player_id && row.place_id == destination_id),
            "successful travel should touch recall for the destination"
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "arrived");
        assert_eq!(events[0].metadata, Some(r#"{"via":"hall"}"#.to_string()));
    }

    #[test]
    fn travel_validation_failure_rolls_back_presence_recall_and_events() {
        let TravelFixture {
            mut world,
            player_id,
            origin_id,
            destination_id,
            ..
        } = setup_travel_fixture();

        let rejected = world.travel(
            TravelRequest::new(player_id, destination_id)
                .with_validate(|_, route| {
                    Err(TravelError::ValidationFailed {
                        player_id,
                        route_id: route.id,
                        reason: "locked passage".to_string(),
                    })
                })
                .with_append_event(|_, _| {
                    Some(TravelEventDraft {
                        kind: "arrived".to_string(),
                        message: "should not persist".to_string(),
                        metadata_json: None,
                    })
                }),
        );

        match rejected {
            Err(TravelError::ValidationFailed { reason, .. }) => {
                assert_eq!(reason, "locked passage");
            }
            other => panic!("expected validation failure, got {other:?}"),
        }

        let loaded_presence = world
            .get_presence(player_id)
            .expect("presence reads")
            .expect("presence row exists");
        let recall = world
            .recall_for_player(player_id)
            .expect("recall reads after rollback");
        let events = world
            .player_events(player_id, 10)
            .expect("player events read after rollback");

        assert_eq!(loaded_presence.place_id, origin_id);
        assert!(
            !recall
                .iter()
                .any(|row| row.player_id == player_id && row.place_id == destination_id),
            "validation failure must not touch destination recall"
        );
        assert!(
            events.is_empty(),
            "validation failure must not append events"
        );
    }

    struct TravelFixture {
        world: WorldDb,
        player_id: i64,
        origin_id: i64,
        destination_id: i64,
        route: Route,
        _tempdir: tempfile::TempDir,
    }

    fn setup_travel_fixture() -> TravelFixture {
        let tempdir = tempdir().expect("tempdir creates");
        let db_path = tempdir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        apply_travel_migrations(&mut world);

        let player_id: i64 = world
            .connection()
            .query_row(
                "INSERT INTO players (handle) VALUES (?1) RETURNING id",
                rusqlite::params!["traveler"],
                |row| row.get(0),
            )
            .expect("player insert works");
        let origin = world
            .insert_place("origin", "Origin", "room", None)
            .expect("origin place inserts");
        let destination = world
            .insert_place("destination", "Destination", "room", None)
            .expect("destination place inserts");
        let route = world
            .create_route(origin.id, destination.id, "hall", None, None)
            .expect("route inserts");
        world
            .set_presence(player_id, origin.id, None)
            .expect("initial presence inserts");

        TravelFixture {
            world,
            player_id,
            origin_id: origin.id,
            destination_id: destination.id,
            route,
            _tempdir: tempdir,
        }
    }

    fn apply_travel_migrations(world: &mut WorldDb) {
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
            .expect("recall migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("events migration applies");
    }
}
