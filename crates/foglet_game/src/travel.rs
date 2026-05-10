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

use crate::inventory::InventoryError;
use crate::presence::PresenceRecord;
use crate::spatial::Route;

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

#[cfg(test)]
mod tests {
    use super::{TravelError, TravelRequest};
    use crate::inventory::InventoryError;
    use crate::presence::PresenceRecord;
    use crate::spatial::Route;

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
}
