//! Persisted and runtime state shared across screens.
//!
//! [`SaveState`] is both the on-disk shape (serialised through a flat
//! wire helper) **and** the live runtime bundle: each persisted field
//! is held as an `Rc<RefCell<T>>` so every screen sharing the slot sees
//! the same mutations. v2.1 §Task 5a moves the canonical handle behind
//! [`foglet_game::SaveSlot`] — `SharedSlots::save` is the typed slot
//! the runtime save handler reads, and the per-field Rcs in
//! [`SharedSlots`] are aliases cloned from the slot's inner state so
//! existing screen code keeps compiling unchanged. v2.1 §Task 5b
//! deletes the hand-written `SharedSlots::snapshot` / `apply` glue —
//! callers now go through [`SaveSlot::snapshot`] directly, and a new
//! [`SharedSlots::with_save_state`] constructor handles the
//! load-then-resume path so aliases never get orphaned. Tasks 5c–5d
//! migrate the remaining consumers off the per-field aliases.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{DateProvider, FeedbackLine, FlagSet, SaveSlot};
use serde::{Deserialize, Serialize};

use crate::clock::SystemDateProvider;
use crate::world::PendingClueEvent;

/// Persisted state for the player's save slot (Task 13h, restructured
/// in SPEC_v2_1 §Task 5a).
///
/// Each persisted field lives behind an `Rc<RefCell<T>>` so the
/// [`SharedSlots`] bundle and the runtime [`SaveSlot<SaveState>`]
/// share *one* set of cells — mutating through any handle is observed
/// everywhere. Serialisation goes through [`SaveStateWire`], a flat
/// helper that pins the on-disk JSON shape (`player_x`, `player_y`,
/// `won`, `flags`, `inventory`, `cash`, `map_name`) so v1 / v1.1 / v2
/// save files keep round-tripping.
///
/// Field naming sticks to the shared-state vocabulary so an operator
/// reading `save.json` can map every key back to a screen field at a
/// glance.
#[derive(Debug, Default)]
pub struct SaveState {
    /// Player position + win latch + cash, behind a single
    /// `Rc<RefCell<_>>` so a screen swapping `(x, y)` in one
    /// borrow_mut never lets a concurrent reader observe a half-applied
    /// position.
    pub player: Rc<RefCell<PlayerSlot>>,
    /// Narrative flags set during gameplay (`heard_rumor`, etc.).
    /// Cloned into a screen-local handle when a [`foglet_game::DialogScreen`]
    /// needs `Rc<RefCell<FlagSet>>` access — the inner refcell is
    /// shared, so flag writes inside a dialog branch are visible to
    /// later conversations and to the win-condition check.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Inventory item IDs the player has collected. The ID namespace is
    /// the [`crate::map::MapScreen::ITEMS`] catalog; ids without a
    /// catalog match are rendered silently as gone — see
    /// [`crate::modals::InventoryScreen::current_labels`].
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Identifier of the map the player is currently standing on. Lets
    /// the title-screen Continue path dispatch to the right map screen
    /// (lobby vs. Room 7) instead of always pushing the lobby. Defaults
    /// to [`default_map_name`] so a New Game starting in the lobby
    /// never has a blank map identifier.
    pub map_name: Rc<RefCell<String>>,
}

impl Clone for SaveState {
    /// Deep-clone: each field gets a fresh `Rc<RefCell<_>>` whose inner
    /// value is a clone of the original's. `SaveSlot::snapshot` relies
    /// on this — a snapshot mutated by the caller MUST NOT bleed back
    /// into the live state, which a refcount-bump `Clone` would not
    /// guarantee.
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
    /// Content-based equality. The Rc identities differ between a
    /// snapshot and its origin, but the on-disk shape is what tests
    /// compare — so equality reaches through the `RefCell` and asks
    /// "do the bytes match?".
    fn eq(&self, other: &Self) -> bool {
        *self.player.borrow() == *other.player.borrow()
            && *self.flags.borrow() == *other.flags.borrow()
            && *self.inventory.borrow() == *other.inventory.borrow()
            && *self.map_name.borrow() == *other.map_name.borrow()
    }
}

impl Eq for SaveState {}

/// Flat on-disk JSON shape for [`SaveState`] (SPEC_v2_1 §Task 5a).
///
/// Kept private and used only by the [`Serialize`] / [`Deserialize`]
/// impls below. Pinning the wire shape here — instead of derive-on-
/// `SaveState` — lets the runtime carry `Rc<RefCell<_>>` field-level
/// handles (so screens can clone an `Rc<RefCell<FlagSet>>` for
/// `DialogScreen`) without leaking that nesting into the JSON. v1 /
/// v1.1 / v2 saves continue to deserialise unchanged.
#[derive(Default, Debug, Serialize, Deserialize)]
struct SaveStateWire {
    player_x: u16,
    player_y: u16,
    won: bool,
    flags: BTreeSet<String>,
    inventory: BTreeSet<String>,
    /// Coin balance carried into v1.1 for the night-clerk vendor scene
    /// (SPEC §9). `#[serde(default)]` so a v1 save written before the
    /// field existed deserialises cleanly with `cash == 0`.
    #[serde(default)]
    cash: u32,
    /// Identifier of the map the player was on at save time. Defaults
    /// to `"lobby"` so any pre-Room-7 save resumes on the lobby map.
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
/// Cloned freely (each clone is a handful of `Rc::clone` calls) so
/// every screen that needs read or write access holds its own handle.
/// The canonical persistence handle is [`Self::save`] — a typed
/// [`SaveSlot<SaveState>`] whose inner [`SaveState`] holds the same
/// `Rc<RefCell<_>>` field handles aliased into [`Self::flags`] /
/// [`Self::inventory`] / [`Self::player`] / [`Self::map_name`]. SPEC_v2_1
/// §Task 5a keeps the per-field aliases so existing screen code
/// (`slots.flags.borrow_mut()`, …) compiles unchanged while migrations
/// land in 5b–5d.
///
/// `Debug` is hand-written rather than derived because
/// [`Self::date_provider`] holds a `dyn DateProvider` trait object
/// that does not require `Debug`. The manual impl prints a stable
/// placeholder for that one field and forwards the rest verbatim.
#[derive(Clone)]
pub struct SharedSlots {
    /// Typed save handle wrapping the canonical [`SaveState`]. The
    /// runtime save handler (SPEC_v2_1 §Task 4) reads from this slot
    /// directly; the per-field aliases below clone the slot's inner
    /// `Rc<RefCell<_>>`s so mutations through any handle land in the
    /// same cells the save handler will serialise.
    pub save: SaveSlot<SaveState>,
    /// Narrative-flag store. Cloned from `save.borrow().flags` so
    /// `slots.flags.borrow_mut().insert(...)` is observed by any other
    /// handle reading the same flag store, including the runtime save
    /// path. Same Rc the [`crate::scenes::dialog::DialogScreen`]
    /// borrows for `requires`-gated branches.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Inventory id set, aliased from `save.borrow().inventory`. Same
    /// Rc the [`crate::modals::InventoryScreen`] reads to draw the
    /// player's pockets.
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Player position + win latch + cash, aliased from
    /// `save.borrow().player`. Pulled into a single Rc so a single
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
    /// Source of "today's local date" used by the SPEC_v2 §Task 13a
    /// clue-inspection turn-spend helper. Held as an `Rc<dyn _>` so
    /// the live game can plug in [`SystemDateProvider`] while tests
    /// inject [`foglet_game::FixedDateProvider`] (or any other impl)
    /// without recompiling.
    ///
    /// Not part of [`SaveState`] — the date provider is a runtime
    /// service, not persisted state, and a save written on day N
    /// reloaded on day M MUST observe the new day's allowance via
    /// the freshly-constructed provider, not via a stale value
    /// frozen into the JSON.
    pub date_provider: Rc<dyn DateProvider>,
    /// Mailbox of `clue_found` world events queued by prompt callbacks
    /// (SPEC_v2 §Task 13c-ii). Prompt-screen callbacks are `'static` and
    /// never see [`foglet_game::GameContext`], so they cannot touch the
    /// world DB themselves. Instead they push a [`PendingClueEvent`]
    /// here when a *new* major clue lands in inventory; the lobby's
    /// per-frame `tick` (which already holds `&WorldDb` via `ctx`)
    /// drains the queue and writes the rows.
    ///
    /// Not part of [`SaveState`]: the mailbox is purely an in-memory
    /// hand-off across one frame boundary. A pending entry that fails
    /// to flush (e.g. `world_db` is `None`) is intentionally silent —
    /// the bulletin missing one row is preferable to the prompt
    /// callback panicking out of `handle_input`.
    pub pending_clue_events: Rc<RefCell<Vec<PendingClueEvent>>>,
}

impl std::fmt::Debug for SharedSlots {
    /// Hand-written so the `Rc<dyn DateProvider>` field doesn't force
    /// the trait to require `Debug`. The `save` slot is omitted —
    /// every persisted field is already printed via its alias below,
    /// and re-printing the same Rcs through the slot would just
    /// duplicate the output.
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
    /// Hand-written so the [`Self::map_name`] alias starts at the
    /// canonical lobby identifier and so the per-field Rcs alias
    /// **into** the [`Self::save`] slot's inner [`SaveState`]. The
    /// SaveState is constructed first via [`SaveState::default`]
    /// (zeroed player, empty flags/inventory, lobby map name) and the
    /// aliases clone its field-level Rcs — bumping refcounts, not
    /// duplicating cells — so every consumer observes the same
    /// mutations regardless of which handle they read or write.
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
            // Live games walk wall-clock time via `SystemDateProvider`;
            // tests overwrite this slot through `with_date_provider`
            // to drive the SPEC §Task 6f deterministic-reset story.
            date_provider: Rc::new(SystemDateProvider),
            pending_clue_events: Rc::new(RefCell::new(Vec::new())),
        }
    }
}

impl SharedSlots {
    /// Build a [`SharedSlots`] whose persistence slot starts at `save`.
    ///
    /// Used by `main` when a previous run wrote a save file: we want
    /// the slot's inner [`SaveState`] (and the per-field aliases that
    /// clone its Rcs) to point at the loaded data from the start, so
    /// no `apply` step is needed and no aliases are ever orphaned.
    /// Tests use the same constructor to set up "restored" slots
    /// in the snapshot/apply round-trip checks.
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

    /// Replace [`Self::date_provider`] with a caller-supplied handle.
    /// Returns `self` so the call composes with [`Self::default`] for
    /// tests that want a fixed clock without writing into the field
    /// post-hoc:
    ///
    /// ```ignore
    /// let slots = SharedSlots::default()
    ///     .with_date_provider(Rc::new(FixedDateProvider::new(date)));
    /// ```
    ///
    /// Production code uses the [`SystemDateProvider`] default; tests
    /// reach for this builder so the date the turn ledger sees is
    /// independent of wall-clock time when the test runs.
    #[must_use]
    pub fn with_date_provider(mut self, provider: Rc<dyn DateProvider>) -> Self {
        self.date_provider = provider;
        self
    }
}

impl SharedSlots {
    /// Reset every slot to a fresh-game baseline at `(start_x, start_y)`.
    /// Called when the main menu activates "New Game" so leftover state
    /// from a previously loaded save does not bleed into the new run.
    ///
    /// SPEC_v2_1 §Task 5c routes the persisted mutations through
    /// [`SaveSlot::borrow_mut`] on [`Self::save`]. That gives us two
    /// things in one move:
    ///
    /// * The SaveSlot's dirty flag flips as a side effect of legitimate
    ///   writes — a "save iff dirty" hook (Task 4) treats a New Game as
    ///   a meaningful state change worth persisting.
    /// * The persisted half of the reset is grouped under one borrow,
    ///   communicating "this block is the on-disk state" without a
    ///   trailing `let _ = self.save.borrow_mut()` no-op.
    ///
    /// The per-field aliases ([`Self::flags`], [`Self::inventory`],
    /// [`Self::player`], [`Self::map_name`]) share the same per-field
    /// `Rc<RefCell<_>>` cells as the slot's inner [`SaveState`], so
    /// every screen handle keeps observing the cleared values without
    /// re-aliasing. SPEC_v2_1 §Task 5d will migrate those screen
    /// consumers off the per-field aliases.
    pub fn reset(&self, start_x: u16, start_y: u16) {
        // Persisted half — one borrow on the SaveSlot covers every
        // on-disk field.
        let state = self.save.borrow_mut();
        state.flags.borrow_mut().clear();
        state.inventory.borrow_mut().clear();
        {
            let mut p = state.player.borrow_mut();
            p.x = start_x;
            p.y = start_y;
            p.won = false;
            // Restore the v1.1 vendor-scene starting balance so a New
            // Game hands the player the documented 40g regardless of
            // what the previous run spent. Centralised so a future
            // re-tune touches one constant.
            p.cash = PlayerSlot::STARTING_CASH;
        }
        // A fresh game always starts in the lobby; clobber any leftover
        // identifier from a previously-loaded save so the snapshot
        // taken on the New Game's first quit lands on the correct map.
        *state.map_name.borrow_mut() = default_map_name();
        drop(state);

        // Ephemeral half — never persisted, lives outside the SaveSlot.
        // Clearing here keeps the New Game flow free of stale narration
        // from a prior session (`feedback`) and stale mailbox entries
        // from a previous run that never reached the lobby tick
        // (`pending_clue_events`).
        *self.feedback.borrow_mut() = None;
        self.pending_clue_events.borrow_mut().clear();
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
        let restored = SharedSlots::with_save_state(original.save.snapshot());
        assert_eq!(restored.player.borrow().cash, 137);
    }

    #[test]
    fn save_state_serializes_to_json_and_back() {
        // Round-trip through the same `serde_json` path
        // `write_atomic`/`read_save` use. Catches accidental
        // `#[serde(skip)]` / rename drift before it ships. SPEC_v2_1
        // §Task 5a moved the field-level handles behind `Rc<RefCell<_>>`
        // and a flat `SaveStateWire` adapter; this test keeps the
        // behavioural assertion (round-trip equal) but writes through
        // the new shape.
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
        // `feedback` is ephemeral — a New Game restart must wipe a stale
        // "Moved Room 7 key to inventory." line so the splash flow does
        // not open under prior-run narration.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        *slots.feedback.borrow_mut() = Some(FeedbackLine::info("stale"));
        slots.reset(0, 0);
        assert!(slots.feedback.borrow().is_none());
    }

    #[test]
    fn shared_slots_round_trip_preserves_map_name() {
        // `map_name` joined SaveState alongside the Room 7 addition; the
        // snapshot/apply pair must round-trip the field or a player who
        // saves inside Room 7 resumes in the lobby. Covered separately
        // from the broader round-trip test so a future failure points at
        // exactly the new field.
        let original = SharedSlots::default();
        *original.map_name.borrow_mut() = "room_7".to_string();
        let restored = SharedSlots::with_save_state(original.save.snapshot());
        assert_eq!(restored.map_name.borrow().as_str(), "room_7");
    }

    #[test]
    fn shared_slots_default_map_name_is_lobby() {
        // The default constructor must seed `map_name` with the lobby
        // identifier so a snapshot taken before any map screen has run
        // (e.g. a player who quit straight from the title menu) still
        // dispatches Continue onto the lobby instead of an unknown map.
        let slots = SharedSlots::default();
        assert_eq!(slots.map_name.borrow().as_str(), "lobby");
    }

    #[test]
    fn shared_slots_reset_returns_map_name_to_lobby() {
        // New Game always begins in the lobby; if a previously loaded
        // save left `map_name = "room_7"` in the slots, `reset` must
        // clobber it so the snapshot taken after the New Game's first
        // quit doesn't mis-route a future Continue back into Room 7.
        let slots = SharedSlots::default();
        *slots.map_name.borrow_mut() = "room_7".to_string();
        slots.reset(22, 4);
        assert_eq!(slots.map_name.borrow().as_str(), "lobby");
    }

    #[test]
    fn save_state_v1_without_map_name_deserialises_as_lobby() {
        // Any save written before the Room 7 addition lacks the
        // `map_name` field. `#[serde(default = "default_map_name")]`
        // must populate it with `"lobby"` so existing saves keep
        // working and Continue dispatches to the lobby.
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
        // Pin the on-disk shape of the field — the same path
        // `write_atomic`/`read_save` use — so a future serde rename
        // surfaces as a failed round-trip rather than a silent reset
        // to the lobby.
        let state = SaveState::default();
        *state.map_name.borrow_mut() = "room_7".to_string();
        let json = serde_json::to_string(&state).expect("serialise");
        let parsed: SaveState = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.map_name.borrow().as_str(), "room_7");
    }
}
