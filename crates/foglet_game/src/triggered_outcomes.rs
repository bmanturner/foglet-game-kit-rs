//! `triggered_outcomes` — generic trigger and outcome contracts.
//!
//! This module gives games a small, genre-neutral vocabulary for
//! deterministic consequences such as "a player arrived at a place" while
//! leaving authored rules, proof tables, rewards, and notices in game code.
//!
//! The kit intentionally separates output channels:
//!
//! - immediate feedback is returned to the caller/UI;
//! - durable Event Log rows are represented as event drafts and can be
//!   appended with [`crate::events::append_event_on`] inside the active
//!   transaction;
//! - future channels such as Notices, Daily Intel, or world-state changes
//!   should remain separate game-owned writes unless a later kit primitive
//!   explicitly adopts them.
//!
//! Idempotency is game-owned. Use [`TriggeredOutcome::idempotency_key`] or
//! [`TriggerContext::stable_key`] to build a uniqueness guard in a game proof
//! table, insert that proof in the transaction callback, and only emit side
//! effects when the insert wins. The kit does not require a universal proofs
//! table.

/// The kind of deterministic trigger being evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    /// A player's presence moved into a place.
    PlaceArrived,
}

impl TriggerKind {
    /// Stable string key for storage, logs, and idempotency keys.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlaceArrived => "place_arrived",
        }
    }
}

/// Context passed to game-authored trigger rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerContext {
    /// Trigger family.
    pub trigger_kind: TriggerKind,
    /// Player affected by the trigger.
    pub player_id: i64,
    /// Place id associated with the trigger, when one exists.
    pub place_id: Option<i64>,
    /// Route id associated with the trigger, when one exists.
    pub route_id: Option<i64>,
    /// Optional source row id, such as an authored rule id.
    pub source_id: Option<i64>,
    /// Optional game-authored source key.
    pub source_key: Option<String>,
}

impl TriggerContext {
    /// Build a context for arrival at a place through a route.
    #[must_use]
    pub fn place_arrived(player_id: i64, place_id: i64, route_id: Option<i64>) -> Self {
        Self {
            trigger_kind: TriggerKind::PlaceArrived,
            player_id,
            place_id: Some(place_id),
            route_id,
            source_id: None,
            source_key: None,
        }
    }

    /// Attach an authored source row id.
    #[must_use]
    pub fn with_source_id(mut self, source_id: i64) -> Self {
        self.source_id = Some(source_id);
        self
    }

    /// Attach an authored source key.
    #[must_use]
    pub fn with_source_key(mut self, source_key: impl Into<String>) -> Self {
        self.source_key = Some(source_key.into());
        self
    }

    /// Deterministic key suitable as a prefix for game-owned proof rows.
    #[must_use]
    pub fn stable_key(&self) -> String {
        format!(
            "{}:player:{}:place:{}:route:{}:source:{}:{}",
            self.trigger_kind.as_str(),
            self.player_id,
            self.place_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.route_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.source_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            self.source_key.as_deref().unwrap_or("-"),
        )
    }
}

/// One immediate UI feedback line produced by a trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeFeedback {
    /// Stable feedback key for UI grouping or deduplication.
    pub key: String,
    /// Player-facing message text.
    pub message: String,
    /// Optional genre-neutral severity/style hint.
    pub tone: Option<String>,
}

impl OutcomeFeedback {
    /// Construct a plain feedback line.
    #[must_use]
    pub fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            message: message.into(),
            tone: None,
        }
    }

    /// Attach a style/severity hint.
    #[must_use]
    pub fn with_tone(mut self, tone: impl Into<String>) -> Self {
        self.tone = Some(tone.into());
        self
    }
}

/// Optional durable Event Log output associated with a triggered outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeredEventDraft {
    /// Event kind key to write to `world_events.kind`.
    pub kind: String,
    /// Event message text to write to `world_events.message`.
    pub message: String,
    /// Optional opaque metadata payload.
    pub metadata_json: Option<String>,
}

/// Structured consequence returned by game-authored trigger evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeredOutcome {
    /// Stable authored outcome key.
    pub key: String,
    /// Trigger that produced the outcome.
    pub context: TriggerContext,
    /// Optional idempotency key used by game-owned proof/progress tables.
    pub idempotency_key: Option<String>,
    /// Immediate caller/UI feedback. This is not a durable event log row.
    pub feedback: Vec<OutcomeFeedback>,
    /// Optional event draft. Games can append this with
    /// [`crate::events::append_event_on`] when the surrounding transaction
    /// decides the outcome should become durable history.
    pub event: Option<TriggeredEventDraft>,
    /// Optional opaque metadata payload for game-defined details.
    pub metadata_json: Option<String>,
}

impl TriggeredOutcome {
    /// Construct an outcome with no feedback or durable event draft.
    #[must_use]
    pub fn new(key: impl Into<String>, context: TriggerContext) -> Self {
        Self {
            key: key.into(),
            context,
            idempotency_key: None,
            feedback: Vec::new(),
            event: None,
            metadata_json: None,
        }
    }

    /// Attach a game-owned idempotency key.
    #[must_use]
    pub fn with_idempotency_key(mut self, idempotency_key: impl Into<String>) -> Self {
        self.idempotency_key = Some(idempotency_key.into());
        self
    }

    /// Append one immediate feedback line.
    #[must_use]
    pub fn with_feedback(mut self, feedback: OutcomeFeedback) -> Self {
        self.feedback.push(feedback);
        self
    }

    /// Attach an optional durable event draft.
    #[must_use]
    pub fn with_event(mut self, event: TriggeredEventDraft) -> Self {
        self.event = Some(event);
        self
    }

    /// Attach opaque game-owned metadata.
    #[must_use]
    pub fn with_metadata_json(mut self, metadata_json: impl Into<String>) -> Self {
        self.metadata_json = Some(metadata_json.into());
        self
    }
}

/// Transaction-local application callback for game-owned outcome effects.
///
/// Games can use this trait when they want to package authored rule
/// evaluation behind a reusable type. The implementor should check
/// idempotency/proof state, apply game-owned mutations, optionally append
/// Event Log rows, and return the outcomes that should be surfaced to the UI.
pub trait OutcomeApplication {
    /// Apply outcomes for `context` inside `conn`'s active transaction.
    fn apply(
        &mut self,
        conn: &rusqlite::Connection,
        context: &TriggerContext,
    ) -> rusqlite::Result<Vec<TriggeredOutcome>>;
}

impl<F> OutcomeApplication for F
where
    F: FnMut(&rusqlite::Connection, &TriggerContext) -> rusqlite::Result<Vec<TriggeredOutcome>>,
{
    fn apply(
        &mut self,
        conn: &rusqlite::Connection,
        context: &TriggerContext,
    ) -> rusqlite::Result<Vec<TriggeredOutcome>> {
        self(conn, context)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutcomeFeedback, TriggerContext, TriggerKind, TriggeredEventDraft, TriggeredOutcome,
    };

    #[test]
    fn place_arrival_context_has_stable_genre_neutral_key() {
        let context = TriggerContext::place_arrived(7, 12, Some(44))
            .with_source_id(3)
            .with_source_key("arrival.rule");

        assert_eq!(context.trigger_kind, TriggerKind::PlaceArrived);
        assert_eq!(
            context.stable_key(),
            "place_arrived:player:7:place:12:route:44:source:3:arrival.rule"
        );
    }

    #[test]
    fn outcome_separates_feedback_from_event_draft_and_idempotency() {
        let context = TriggerContext::place_arrived(7, 12, None);
        let outcome = TriggeredOutcome::new("rule.arrive", context.clone())
            .with_idempotency_key(format!("{}:rule.arrive", context.stable_key()))
            .with_feedback(OutcomeFeedback::new("proof", "Survey proof recorded."))
            .with_event(TriggeredEventDraft {
                kind: "survey_proof".to_string(),
                message: "Survey proof recorded at place 12.".to_string(),
                metadata_json: Some(r#"{"place_id":12}"#.to_string()),
            });

        assert_eq!(
            outcome.idempotency_key.as_deref(),
            Some("place_arrived:player:7:place:12:route:-:source:-:-:rule.arrive")
        );
        assert_eq!(outcome.feedback[0].message, "Survey proof recorded.");
        assert_eq!(
            outcome.event.as_ref().map(|event| event.kind.as_str()),
            Some("survey_proof")
        );
    }
}
