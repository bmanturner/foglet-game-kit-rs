//! Persisted and runtime state shared across screens.
//!
//! [`SaveState`] is both the on-disk shape (via a flat wire helper) and
//! the live runtime bundle. Each persisted field lives behind an
//! `Rc<RefCell<T>>` so every screen sharing the slot observes the same
//! mutations. The canonical handle is the [`foglet_game::SaveSlot`] in
//! [`SharedSlots::save`]; the per-field Rcs in [`SharedSlots`] are
//! aliases cloned from the slot's inner state, so screen code that
//! holds the alias and the runtime save handler see the same cells.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{DateProvider, FeedbackLine, FlagSet, SaveSlot};
use serde::{Deserialize, Serialize};

use crate::clock::SystemDateProvider;
use crate::world::PendingClueEvent;

/// Persisted state for the player's save slot.
///
/// Each field lives behind an `Rc<RefCell<T>>` so [`SharedSlots`] and
/// the runtime [`SaveSlot<SaveState>`] share one set of cells.
/// Serialisation goes through [`SaveStateWire`] to pin a flat on-disk
/// JSON shape compatible with v1/v1.1/v2 saves.
#[derive(Debug, Default)]
pub struct SaveState {
    pub player: Rc<RefCell<PlayerSlot>>,
    /// Narrative flags. Dialog screens clone this Rc so flag writes
    /// inside a branch are visible to later conversations.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Inventory item IDs from the [`crate::map::MapScreen::ITEMS`]
    /// catalog. Unknown ids render silently as gone.
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Map the player is currently standing on, for dispatching the
    /// title-screen Continue path. Defaults to [`default_map_name`].
    pub map_name: Rc<RefCell<String>>,
}

impl Clone for SaveState {
    /// Deep-clone: each Rc gets a fresh inner copy so
    /// `SaveSlot::snapshot` cannot bleed mutations back into live state.
    fn clone(&self) -> Self {
        Self {
            player: Rc::new(RefCell::new(*self.player.borrow())),
            flags: Rc::new(RefCell::new(self.flags.borrow().clone())),
            inventory: Rc::new(RefCell::new(self.inventory.borrow().clone())),
            map_name: Rc::new(RefCell::new(self.map_name.borrow().clone())),
        }
    }
}

impl PartialEq for SaveState {
    /// Content-based equality: reach through `RefCell` so a snapshot
    /// compares equal to its origin even with different Rc identities.
    fn eq(&self, other: &Self) -> bool {
        *self.player.borrow() == *other.player.borrow()
            && *self.flags.borrow() == *other.flags.borrow()
            && *self.inventory.borrow() == *other.inventory.borrow()
            && *self.map_name.borrow() == *other.map_name.borrow()
    }
}

impl Eq for SaveState {}

/// Flat on-disk JSON shape for [`SaveState`]. Hand-pinned (rather than
/// derive-on-`SaveState`) so the runtime can keep field-level Rcs
/// without leaking that nesting into the JSON. v1/v1.1/v2 saves keep
/// round-tripping.
#[derive(Default, Debug, Serialize, Deserialize)]
struct SaveStateWire {
    player_x: u16,
    player_y: u16,
    won: bool,
    flags: BTreeSet<String>,
    inventory: BTreeSet<String>,
    /// `#[serde(default)]` keeps v1 saves (no `cash` field) parsing.
    #[serde(default)]
    cash: u32,
    /// `#[serde(default)]` keeps pre-Room-7 saves resuming in the lobby.
    #[serde(default = "default_map_name")]
    map_name: String,
}

impl Serialize for SaveState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let player = *self.player.borrow();
        let wire = SaveStateWire {
            player_x: player.x,
            player_y: player.y,
            won: player.won,
            flags: self.flags.borrow().clone(),
            inventory: self.inventory.borrow().clone(),
            cash: player.cash,
            map_name: self.map_name.borrow().clone(),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SaveState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SaveStateWire::deserialize(deserializer)?;
        Ok(Self {
            player: Rc::new(RefCell::new(PlayerSlot {
                x: wire.player_x,
                y: wire.player_y,
                won: wire.won,
                cash: wire.cash,
            })),
            flags: Rc::new(RefCell::new(wire.flags)),
            inventory: Rc::new(RefCell::new(wire.inventory)),
            map_name: Rc::new(RefCell::new(wire.map_name)),
        })
    }
}

/// Default map identifier — must match the value
/// [`crate::map::MapScreen`] writes into the slots.
pub fn default_map_name() -> String {
    "lobby".to_string()
}

/// Runtime state shared across screens. The persistence handle is
/// [`Self::save`]; the per-field Rcs alias the slot's inner state so
/// existing screen code mutates the same cells the save handler reads.
///
/// `Debug` is hand-written because [`Self::date_provider`] is
/// `Rc<dyn DateProvider>` (no `Debug` bound).
#[derive(Clone)]
pub struct SharedSlots {
    pub save: SaveSlot<SaveState>,
    /// Aliased from `save.borrow().flags`. Dialog screens clone this
    /// Rc for `requires`-gated branches.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Aliased from `save.borrow().inventory`.
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Aliased from `save.borrow().player`.
    pub player: Rc<RefCell<PlayerSlot>>,
    /// Most recent player-facing feedback line. Ephemeral; cleared on
    /// [`Self::reset`] so a New Game opens without stale narration.
    pub feedback: Rc<RefCell<Option<FeedbackLine>>>,
    /// Aliased from `save.borrow().map_name`.
    pub map_name: Rc<RefCell<String>>,
    /// Date provider for the clue-inspection turn ledger. Runtime
    /// service; never persisted, so a save loaded on day M observes
    /// day M's allowance via the freshly-constructed provider.
    pub date_provider: Rc<dyn DateProvider>,
    /// One-frame mailbox for `clue_found` events. Prompt callbacks are
    /// `'static` and cannot touch `WorldDb`; they push here, the
    /// lobby tick drains. Not persisted: a flush failure silently
    /// drops the entry rather than panic out of `handle_input`.
    pub pending_clue_events: Rc<RefCell<Vec<PendingClueEvent>>>,
}

impl std::fmt::Debug for SharedSlots {
    // `save` is omitted: aliases below print the same Rcs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSlots")
            .field("flags", &self.flags)
            .field("inventory", &self.inventory)
            .field("player", &self.player)
            .field("feedback", &self.feedback)
            .field("map_name", &self.map_name)
            .field("date_provider", &"<dyn DateProvider>")
            .field("pending_clue_events", &self.pending_clue_events)
            .finish()
    }
}

/// Small POD bundle inside [`SharedSlots::player`].
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSlot {
    pub x: u16,
    pub y: u16,
    pub won: bool,
    /// Coin balance in "g" units (SPEC §9 vendor scene).
    pub cash: u32,
}

impl PlayerSlot {
    /// Tuned to SPEC §9: enough for 25g coffee, not enough for a 50g
    /// rumor tip, so the disabled-state branch fires on a clean run.
    pub const STARTING_CASH: u32 = 40;
}

impl Default for SharedSlots {
    // Per-field Rcs alias *into* the SaveSlot's inner SaveState — the
    // aliases are refcount bumps, not new cells, so every handle
    // observes the same mutations.
    fn default() -> Self {
        let save_state = SaveState {
            player: Rc::new(RefCell::new(PlayerSlot::default())),
            flags: Rc::new(RefCell::new(FlagSet::new())),
            inventory: Rc::new(RefCell::new(BTreeSet::new())),
            map_name: Rc::new(RefCell::new(default_map_name())),
        };
        let flags = Rc::clone(&save_state.flags);
        let inventory = Rc::clone(&save_state.inventory);
        let player = Rc::clone(&save_state.player);
        let map_name = Rc::clone(&save_state.map_name);
        Self {
            save: SaveSlot::new(save_state),
            flags,
            inventory,
            player,
            feedback: Rc::new(RefCell::new(None)),
            map_name,
            date_provider: Rc::new(SystemDateProvider),
            pending_clue_events: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl SharedSlots {
    /// Build slots whose per-field aliases point at `save`'s Rcs from
    /// frame zero. Avoids the "load → apply" path that would orphan
    /// the aliases. `SaveSlot::load_or_default` is the right tool when
    /// `T` stays opaque; this example needs the field aliases.
    pub fn with_save_state(save: SaveState) -> Self {
        let flags = Rc::clone(&save.flags);
        let inventory = Rc::clone(&save.inventory);
        let player = Rc::clone(&save.player);
        let map_name = Rc::clone(&save.map_name);
        Self {
            save: SaveSlot::new(save),
            flags,
            inventory,
            player,
            feedback: Rc::new(RefCell::new(None)),
            map_name,
            date_provider: Rc::new(SystemDateProvider),
            pending_clue_events: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Override [`Self::date_provider`] for tests that want a fixed
    /// clock independent of wall-clock time.
    #[must_use]
    pub fn with_date_provider(mut self, provider: Rc<dyn DateProvider>) -> Self {
        self.date_provider = provider;
        self
    }
}

impl SharedSlots {
    /// Reset every slot to a fresh-game baseline at `(start_x, start_y)`.
    /// Mutating through `save.borrow_mut()` flips the SaveSlot's dirty
    /// flag so a "save iff dirty" hook treats New Game as worth
    /// persisting.
    pub fn reset(&self, start_x: u16, start_y: u16) {
        let state = self.save.borrow_mut();
        state.flags.borrow_mut().clear();
        state.inventory.borrow_mut().clear();
        {
            let mut p = state.player.borrow_mut();
            p.x = start_x;
            p.y = start_y;
            p.won = false;
            p.cash = PlayerSlot::STARTING_CASH;
        }
        *state.map_name.borrow_mut() = default_map_name();
        drop(state);

        *self.feedback.borrow_mut() = None;
        self.pending_clue_events.borrow_mut().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MapScreen;
    use foglet_game::FeedbackLine;

    #[test]
    fn shared_slots_snapshot_round_trips_through_apply() {
        let original = SharedSlots::default();
        original.flags.borrow_mut().insert("heard_rumor".into());
        original.flags.borrow_mut().insert("has_key".into());
        original.inventory.borrow_mut().insert("brass_key".into());
        original.inventory.borrow_mut().insert("matchbook".into());
        {
            let mut p = original.player.borrow_mut();
            p.x = 43;
            p.y = 4;
            p.won = true;
        }
        let snap = original.save.snapshot();

        let restored = SharedSlots::with_save_state(snap);
        let p = restored.player.borrow();
        assert_eq!((p.x, p.y, p.won), (43, 4, true));
        drop(p);
        assert_eq!(*original.flags.borrow(), *restored.flags.borrow());
        assert_eq!(*original.inventory.borrow(), *restored.inventory.borrow());
    }

    #[test]
    fn shared_slots_reset_clears_all_state() {
        let slots = SharedSlots::default();
        slots.flags.borrow_mut().insert("heard_rumor".into());
        slots.inventory.borrow_mut().insert("brass_key".into());
        slots.player.borrow_mut().won = true;
        slots.player.borrow_mut().x = 99;
        slots.player.borrow_mut().cash = 0;

        slots.reset(22, 4);

        assert!(slots.flags.borrow().is_empty(), "flags must be cleared");
        assert!(
            slots.inventory.borrow().is_empty(),
            "inventory must be cleared"
        );
        let p = slots.player.borrow();
        assert_eq!((p.x, p.y, p.won), (22, 4, false));
        assert_eq!(
            p.cash,
            PlayerSlot::STARTING_CASH,
            "reset must restore the v1.1 vendor-scene starting balance"
        );
    }

    #[test]
    fn fresh_run_state_matches_v1_1_proof_scene_baseline() {
        // SPEC §9 baseline: empty inventory, no receipt-read flag, 40g.
        let slots = SharedSlots::default();
        slots.reset(22, 4);

        let inv = slots.inventory.borrow();
        assert!(
            !inv.contains(MapScreen::ROOM_7_KEY_ID),
            "fresh run must not start with the Room 7 key"
        );
        assert!(
            !inv.contains(MapScreen::MATCHBOOK_ID),
            "fresh run must not start with the matchbook"
        );
        drop(inv);

        assert!(
            !slots.flags.borrow().contains(MapScreen::RECEIPT_READ_FLAG),
            "fresh run must not have the receipt-read flag set"
        );

        assert_eq!(
            slots.player.borrow().cash,
            PlayerSlot::STARTING_CASH,
            "fresh run must hand the player the documented starting cash"
        );
    }

    #[test]
    fn shared_slots_round_trip_preserves_cash() {
        // v1.1 field — pinned separately so a regression points here.
        let original = SharedSlots::default();
        original.player.borrow_mut().cash = 137;
        let restored = SharedSlots::with_save_state(original.save.snapshot());
        assert_eq!(restored.player.borrow().cash, 137);
    }

    #[test]
    fn save_state_serializes_to_json_and_back() {
        // Catches accidental serde rename / skip drift on the wire shape.
        let state = SaveState::default();
        {
            let mut p = state.player.borrow_mut();
            p.x = 43;
            p.y = 4;
            p.won = true;
        }
        state.flags.borrow_mut().insert("heard_rumor".into());
        state.inventory.borrow_mut().insert("brass_key".into());
        let json = serde_json::to_string(&state).expect("serialise");
        let parsed: SaveState = serde_json::from_str(&json).expect("parse");
        assert_eq!(state, parsed);
    }

    #[test]
    fn shared_slots_reset_clears_feedback() {
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        *slots.feedback.borrow_mut() = Some(FeedbackLine::info("stale"));
        slots.reset(0, 0);
        assert!(slots.feedback.borrow().is_none());
    }

    #[test]
    fn shared_slots_round_trip_preserves_map_name() {
        let original = SharedSlots::default();
        *original.map_name.borrow_mut() = "room_7".to_string();
        let restored = SharedSlots::with_save_state(original.save.snapshot());
        assert_eq!(restored.map_name.borrow().as_str(), "room_7");
    }

    #[test]
    fn shared_slots_default_map_name_is_lobby() {
        let slots = SharedSlots::default();
        assert_eq!(slots.map_name.borrow().as_str(), "lobby");
    }

    #[test]
    fn shared_slots_reset_returns_map_name_to_lobby() {
        let slots = SharedSlots::default();
        *slots.map_name.borrow_mut() = "room_7".to_string();
        slots.reset(22, 4);
        assert_eq!(slots.map_name.borrow().as_str(), "lobby");
    }

    #[test]
    fn save_state_v1_without_map_name_deserialises_as_lobby() {
        // Pre-Room-7 saves lack `map_name` and `cash`; both must default.
        let v1_json = r#"{
            "player_x": 22,
            "player_y": 4,
            "won": false,
            "flags": [],
            "inventory": []
        }"#;
        let parsed: SaveState = serde_json::from_str(v1_json).expect("v1 save parses");
        assert_eq!(parsed.map_name.borrow().as_str(), "lobby");
        assert_eq!(
            parsed.player.borrow().cash,
            0,
            "missing cash field also defaults"
        );
    }

    #[test]
    fn save_state_round_trips_map_name_through_json() {
        let state = SaveState::default();
        *state.map_name.borrow_mut() = "room_7".to_string();
        let json = serde_json::to_string(&state).expect("serialise");
        let parsed: SaveState = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.map_name.borrow().as_str(), "room_7");
    }
}
