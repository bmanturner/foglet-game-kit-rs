//! Persisted and runtime state shared across screens.
//!
//! [`SaveState`] is the on-disk JSON shape; [`SharedSlots`] is the
//! `Rc<RefCell<_>>` bundle every screen mutates while the game is
//! running. [`SharedSlots::snapshot`] / [`Self::apply`] are the bridge
//! between the two — extending the save schema is "add a field, add a
//! line to each, run tests".

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{FeedbackLine, FlagSet};
use serde::{Deserialize, Serialize};

/// Persisted state for the player's save slot (Task 13h).
///
/// JSON-serialised through the kit's `write_atomic` helper, deserialised
/// back via `read_save`. The shape matches [`SharedSlots::snapshot`] one
/// field at a time so adding a new save field is "extend the struct,
/// extend the snapshot/apply pair, run tests".
///
/// Field naming sticks to the shared-state vocabulary (`player_x`,
/// `player_y`, `won`) rather than nesting a `PlayerSlot` so an operator
/// reading `save.json` can map every key back to a screen field at a
/// glance.
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveState {
    /// Player column at save time. Restored verbatim into
    /// [`SharedSlots::player`] on `apply`.
    pub player_x: u16,
    /// Player row at save time.
    pub player_y: u16,
    /// Whether the win condition has fired in this save slot. A `true`
    /// value preserves the post-win map render (no `*` marker) on
    /// resume.
    pub won: bool,
    /// Narrative flags set during gameplay (`heard_rumor`, etc.).
    pub flags: BTreeSet<String>,
    /// Inventory item IDs the player has collected. The ID namespace is
    /// the [`crate::map::MapScreen::ITEMS`] catalog; ids without a
    /// catalog match are rendered silently as gone — see
    /// [`crate::modals::InventoryScreen::current_labels`].
    pub inventory: BTreeSet<String>,
    /// Coin balance carried into v1.1 for the night-clerk vendor scene
    /// (SPEC §9). `#[serde(default)]` so a v1 save written before the
    /// field existed deserialises cleanly with `cash == 0`; the New
    /// Game / load paths reset to [`PlayerSlot::STARTING_CASH`].
    #[serde(default)]
    pub cash: u32,
    /// Identifier of the map the player was on at save time. Lets the
    /// title-screen Continue path dispatch to the right map screen
    /// (lobby vs. Room 7) instead of always pushing the lobby. The
    /// `#[serde(default = "default_map_name")]` keeps v1 saves
    /// deserialising as the lobby — the only map the example shipped
    /// with before the Room 7 addition.
    #[serde(default = "default_map_name")]
    pub map_name: String,
}

/// Default map identifier baked into [`SaveState::map_name`] when a
/// pre-Room-7 save file is read. Centralised so the spelling matches
/// the value [`crate::map::MapScreen`] writes into the slots on
/// construction.
pub fn default_map_name() -> String {
    "lobby".to_string()
}

/// Mutable runtime fields that need to survive a quit/launch cycle and
/// are therefore shared between the screens that mutate them and the
/// `main` scope that writes the save on exit.
///
/// Cloned freely (each clone is three `Rc::clone` calls) so every
/// screen that needs read or write access holds its own handle. The
/// canonical handle lives in `main`, which uses [`Self::snapshot`] /
/// [`Self::apply`] to bridge to and from on-disk [`SaveState`].
#[derive(Clone, Debug)]
pub struct SharedSlots {
    /// Narrative-flag store. Same Rc the [`crate::scenes::dialog::DialogScreen`]
    /// borrows for `requires`-gated branches.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Inventory id set. Same Rc the
    /// [`crate::modals::InventoryScreen`] reads to draw the player's
    /// pockets.
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Player position + win latch. Pulled into a single Rc so a single
    /// `borrow_mut()` swap is enough to apply a loaded save without
    /// briefly observing a half-restored position.
    pub player: Rc<RefCell<PlayerSlot>>,
    /// Most recent player-facing feedback line — written by prompt
    /// callbacks (e.g. the Lost-and-Found Drawer in Task 10f) and read
    /// by the [`crate::map::MapScreen`] renderer below the movement
    /// hint. Ephemeral state: not part of [`SaveState`], cleared on
    /// `reset` so a fresh run never opens with stale narration from a
    /// prior session.
    pub feedback: Rc<RefCell<Option<FeedbackLine>>>,
    /// Identifier of the map the player is currently standing on.
    /// Each map screen (lobby, Room 7, …) writes its own identifier
    /// here on construction; [`Self::snapshot`] reads it into the
    /// on-disk save and [`Self::apply`] writes it back so the title
    /// screen's Continue path can dispatch the resumed run onto the
    /// correct screen. Defaults to [`default_map_name`] on a fresh
    /// `SharedSlots` so a New Game starting in the lobby never has a
    /// blank map identifier.
    pub map_name: Rc<RefCell<String>>,
}

/// Small POD bundle inside [`SharedSlots::player`].
///
/// Held in a `RefCell` so movement, win-latch, and save-restore can all
/// mutate it through the same Rc. `Copy` because every field is a
/// primitive — cheaper than re-borrowing for every read.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSlot {
    /// Player column.
    pub x: u16,
    /// Player row.
    pub y: u16,
    /// Whether the win condition has been triggered this run.
    pub won: bool,
    /// Player's coin balance, in the abstract "g" unit the SPEC §9
    /// vendor prompt advertises ("You have: 40g"). The v1.1 proof scene
    /// is the only consumer; gameplay paths that don't touch the
    /// vendor leave this unchanged.
    pub cash: u32,
}

impl PlayerSlot {
    /// Coins the player starts a fresh run with. Tuned to the SPEC §9
    /// vendor example: 40g is enough to buy the 25g coffee but not
    /// enough to tip for a rumor (50g), so the disabled-state branch
    /// fires on a clean New Game.
    pub const STARTING_CASH: u32 = 40;
}

impl Default for SharedSlots {
    /// Hand-written so the [`Self::map_name`] slot starts at the
    /// canonical lobby identifier instead of an empty string. Every
    /// other slot still uses its derived default — the only deviation
    /// is the map name, which downstream code (snapshot, Continue
    /// dispatch) relies on being non-empty.
    fn default() -> Self {
        Self {
            flags: Rc::new(RefCell::new(FlagSet::new())),
            inventory: Rc::new(RefCell::new(BTreeSet::new())),
            player: Rc::new(RefCell::new(PlayerSlot::default())),
            feedback: Rc::new(RefCell::new(None)),
            map_name: Rc::new(RefCell::new(default_map_name())),
        }
    }
}

impl SharedSlots {
    /// Reset every slot to a fresh-game baseline at `(start_x, start_y)`.
    /// Called when the main menu activates "New Game" so leftover state
    /// from a previously loaded save does not bleed into the new run.
    pub fn reset(&self, start_x: u16, start_y: u16) {
        self.flags.borrow_mut().clear();
        self.inventory.borrow_mut().clear();
        let mut p = self.player.borrow_mut();
        p.x = start_x;
        p.y = start_y;
        p.won = false;
        // Restore the v1.1 vendor-scene starting balance so a New Game
        // hands the player the documented 40g regardless of what the
        // previous run spent. The field is re-initialised in one place
        // so a future re-tune touches a single constant.
        p.cash = PlayerSlot::STARTING_CASH;
        // Clear any leftover feedback so a new run never opens under a
        // stale "Moved Room 7 key to inventory." line from a previous
        // session.
        *self.feedback.borrow_mut() = None;
        // A fresh game always starts in the lobby; clobber any leftover
        // identifier from a previously-loaded save so the snapshot
        // taken on the New Game's first quit lands on the correct map.
        *self.map_name.borrow_mut() = default_map_name();
    }

    /// Build a [`SaveState`] from the current slot contents. Cloning the
    /// flag/inventory sets keeps the on-disk JSON independent of the
    /// live runtime — the save file is a frozen snapshot, not a mirror.
    pub fn snapshot(&self) -> SaveState {
        let player = *self.player.borrow();
        SaveState {
            player_x: player.x,
            player_y: player.y,
            won: player.won,
            flags: self.flags.borrow().clone(),
            inventory: self.inventory.borrow().clone(),
            cash: player.cash,
            map_name: self.map_name.borrow().clone(),
        }
    }

    /// Overwrite slot contents from a loaded [`SaveState`]. The player
    /// `RefCell` is swapped in one borrow so concurrent screen reads
    /// never observe `(new_x, old_y)`.
    pub fn apply(&self, state: SaveState) {
        {
            let mut p = self.player.borrow_mut();
            p.x = state.player_x;
            p.y = state.player_y;
            p.won = state.won;
            p.cash = state.cash;
        }
        *self.flags.borrow_mut() = state.flags;
        *self.inventory.borrow_mut() = state.inventory;
        *self.map_name.borrow_mut() = state.map_name;
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the persisted save shape and the snapshot/apply
    //! bridge that every screen mutates through.

    use super::*;
    use crate::map::MapScreen;
    use foglet_game::FeedbackLine;

    #[test]
    fn shared_slots_snapshot_round_trips_through_apply() {
        // Snapshot → apply on a fresh `SharedSlots` must reconstruct
        // the same state byte-for-byte. This is the contract `main`
        // relies on: write `snapshot()`, read on next launch, call
        // `apply()`, see the same world.
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
        let snap = original.snapshot();

        let restored = SharedSlots::default();
        restored.apply(snap);
        let p = restored.player.borrow();
        assert_eq!((p.x, p.y, p.won), (43, 4, true));
        drop(p);
        assert_eq!(*original.flags.borrow(), *restored.flags.borrow());
        assert_eq!(*original.inventory.borrow(), *restored.inventory.borrow());
    }

    #[test]
    fn shared_slots_reset_clears_all_state() {
        // "New Game" hands a previously loaded slots into `reset` so
        // resumed flags / inventory / coordinates do not bleed in.
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
        // SPEC §9 Task 10a baseline: a New Game must hand the player
        // an empty inventory (no Room 7 key, no matchbook), an unset
        // receipt-read flag, and the documented 40g starting balance.
        // If any of these drift, the Lost-and-Found Drawer and
        // night-clerk vendor scenes will demo the wrong state.
        let slots = SharedSlots::default();
        // Coordinates here are arbitrary — `reset` always re-sets cash
        // and clears flags/inventory regardless of where the player
        // spawns, so the test is independent of the lobby layout.
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
        // Cash joined SaveState in v1.1; the snapshot/apply pair must
        // round-trip the field or the night-clerk vendor scene loses
        // the player's balance across save/load. Covered separately
        // from `shared_slots_snapshot_round_trips_through_apply` so a
        // future failure points at exactly the new field.
        let original = SharedSlots::default();
        original.player.borrow_mut().cash = 137;
        let restored = SharedSlots::default();
        restored.apply(original.snapshot());
        assert_eq!(restored.player.borrow().cash, 137);
    }

    #[test]
    fn save_state_serializes_to_json_and_back() {
        // Round-trip through the same `serde_json` path
        // `write_atomic`/`read_save` use. Catches accidental
        // `#[serde(skip)]` / rename drift before it ships.
        let mut state = SaveState::default();
        state.player_x = 43;
        state.player_y = 4;
        state.won = true;
        state.flags.insert("heard_rumor".into());
        state.inventory.insert("brass_key".into());
        let json = serde_json::to_string(&state).expect("serialise");
        let parsed: SaveState = serde_json::from_str(&json).expect("parse");
        assert_eq!(state, parsed);
    }

    #[test]
    fn shared_slots_reset_clears_feedback() {
        // `feedback` is ephemeral — a New Game restart must wipe a stale
        // "Moved Room 7 key to inventory." line so the splash flow does
        // not open under prior-run narration.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        *slots.feedback.borrow_mut() = Some(FeedbackLine::info("stale"));
        slots.reset(0, 0);
        assert!(slots.feedback.borrow().is_none());
    }
}
