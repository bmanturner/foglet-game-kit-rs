//! Lobby map screen, NPC + item catalogs, and the embedded map/dialog
//! assets they reference.
//!
//! [`MapScreen`] is the gameplay hub: it owns the parsed lobby tile
//! grid and routes input into the dialog, inventory, and SPEC §9 proof
//! scenes. The static [`Self::NPCS`] and [`Self::ITEMS`] catalogs along
//! with the locked-door / win-tile / drawer constants live here so a
//! single file describes the whole lobby.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{
    parse_map, FeedbackLine, FlagSet, GameContext, Input, Map, Screen, ScreenCommand, TileLegend,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::layout::centred_rect;
use crate::modals::{InventoryScreen, WinScreen};
use crate::scenes::dialog::DialogScreen;
use crate::scenes::lost_and_found::lost_and_found_drawer_screen;
use crate::scenes::night_clerk::night_clerk_vendor_screen;
use crate::state::SharedSlots;

/// Lobby map source, embedded at compile time.
///
/// `include_str!` keeps the example self-contained — the binary needs
/// no runtime asset lookup to render its first map, which sidesteps
/// the "where am I being run from?" issue that bit the title screen's
/// `assets/game.toml` lookup before [`crate::GAME_TOML_PATH`] was
/// introduced. `fgk new`-generated projects load their starter map
/// from disk via `assets/maps/<name>.txt`; switch to
/// `std::fs::read_to_string` if you copy this scaffold and want
/// hot-editable maps in dev.
const LOBBY_MAP_TEXT: &str = include_str!("../assets/maps/lobby.txt");

/// Dialog scripts for each NPC, embedded at compile time.
///
/// Same rationale as [`LOBBY_MAP_TEXT`] — keeping the YAML inside the
/// binary means `cargo run --example` works no matter the CWD, and
/// `fgk new`-generated scaffolds can swap to `std::fs::read_to_string`
/// when authors want hot-edit iteration in dev. The Night Clerk's
/// script carries the SPEC §13 branching requirement; the other two
/// are short linear flavour beats so the map feels populated rather
/// than decorated with one talkable NPC and two scenery glyphs.
const NIGHT_CLERK_DIALOG: &str = include_str!("../assets/dialog/night_clerk.yaml");
const BELLHOP_DIALOG: &str = include_str!("../assets/dialog/bellhop.yaml");
const MAID_DIALOG: &str = include_str!("../assets/dialog/maid.yaml");

/// A non-player character pinned to a fixed cell on the lobby map.
///
/// `Npc` is intentionally `Copy` and built from `&'static str` slices
/// so the whole NPC roster lives as a `const` array on
/// [`MapScreen::NPCS`]. The conversation YAML is also `&'static str`
/// (via `include_str!`); the dialog graph is parsed lazily when the
/// player actually engages, which keeps the example's startup cost
/// at zero NPC-related allocations.
#[derive(Debug, Clone, Copy)]
pub struct Npc {
    /// Display name shown in the dialog header. Stable across runs.
    pub name: &'static str,
    /// Map glyph painted at `(x, y)`. Single-cell ASCII — the render
    /// path treats it the same way it treats the player glyph.
    pub glyph: char,
    /// Map column. Must reference a walkable floor cell in the lobby
    /// legend; the constructor does not validate this because the
    /// roster is hand-authored and any drift will surface as a
    /// failing test rather than a runtime panic.
    pub x: u16,
    /// Map row. Same constraint as [`Self::x`].
    pub y: u16,
    /// Embedded dialog YAML this NPC opens with. Loaded by
    /// [`DialogScreen::new`] on first interaction; parse failures are
    /// build-time bugs (the YAML ships in the binary) so the dialog
    /// constructor uses `expect`.
    pub dialog_yaml: &'static str,
}

/// A collectable item pinned to a fixed cell on the lobby map.
///
/// Like [`Npc`], `Item` is `Copy` and built from `&'static str` slices
/// so the entire item catalog lives as a `const` array on
/// [`MapScreen::ITEMS`]. The collected set is tracked separately on the
/// map screen as a [`BTreeSet`] of item IDs; the catalog is the
/// authoritative source for an item's display name and glyph regardless
/// of whether the player has picked it up.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// Stable identifier used as the inventory key. Chosen from a
    /// constrained ASCII namespace so it round-trips through the save
    /// file (Task 13h) without serialisation surprises.
    pub id: &'static str,
    /// Display name shown on the inventory screen. Stored separately
    /// from `id` so we can rename the player-facing label without
    /// invalidating saves.
    pub name: &'static str,
    /// Map glyph painted at `(x, y)` while the item is uncollected.
    /// Single-cell ASCII to match the player and NPC overlays.
    pub glyph: char,
    /// Map column. Must reference a walkable floor cell that is *not*
    /// occupied by an NPC; the test
    /// `map_items_sit_on_walkable_non_npc_cells` enforces this.
    pub x: u16,
    /// Map row. Same constraint as [`Self::x`].
    pub y: u16,
}

/// Lobby map screen — the SPEC §13 "5 rooms with player movement"
/// fixture (Task 13c).
///
/// Owns a parsed [`Map`] plus the player's `(x, y)` coordinates. The
/// map itself is immutable for this iteration: 13d/13e/13f layer NPCs,
/// items, and locked-door state on top, but those concerns live on
/// the screen (or future sibling screens) rather than mutating the
/// underlying tile grid.
///
/// ## Why a fixed-size map and label overlay
///
/// The map is 46 cells wide and 7 cells tall, well inside the
/// 80×24 minimum from SPEC §7.1. Five rooms are laid out in a
/// horizontal strip — Room 1, Room 2, Lobby, Room 3, Room 4 — with
/// `+` doors at `y == 3` connecting each room to its neighbour. The
/// rooms are anonymous in the ASCII source (just `#`/`+`/space) and
/// labels are overlaid at render time, which keeps the legend
/// minimal and avoids the trap of maps becoming illegible if the
/// labelling convention ever changes.
///
/// ## Walkability and bounds
///
/// Movement consults [`Map::is_walkable`]. Walls block, floors and
/// doors pass. Out-of-bounds is treated as a wall so the input
/// handler does not need a separate bounds check before stepping.
pub struct MapScreen {
    /// Parsed lobby map (immutable for this screen's lifetime). Held
    /// directly rather than re-parsing each frame so render is
    /// allocation-light.
    map: Map,
    /// Save-bearing runtime state — player position, narrative flags,
    /// inventory, win latch. Shared with `main` (which writes the save
    /// on exit), with [`DialogScreen`] (`flags`), and with
    /// [`InventoryScreen`] (`inventory`). The single Rc bundle replaces
    /// what used to be three separate fields so Task 13h's save/load
    /// path manipulates the same handles the screens read.
    slots: SharedSlots,
    /// SPEC_v2 §Task 13b cache for the lobby's "remaining turns" status
    /// line. Populated lazily from [`Self::tick`] (which is allowed to
    /// touch SQLite under the kit's contract) and updated in place by
    /// the X-press handler when a clue inspection lands. Stays `None`
    /// until the first `tick` runs against a [`GameContext`] that has a
    /// world DB attached *and* a `[turns]` section configured — which is
    /// precisely the set of runs that should display a status line, so
    /// no separate "should we show this?" flag is needed alongside the
    /// option.
    ///
    /// Held in a `RefCell` because [`Screen::render`] takes `&mut self`
    /// while [`Self::hint_line`] takes `&self`; the cell keeps both
    /// paths consulting the same value without forcing a wider mutable
    /// borrow through the ratatui draw path.
    turns_status: RefCell<Option<crate::world::RemainingTurns>>,
}

impl MapScreen {
    /// Title rendered on the bordered block surrounding the map.
    pub const TITLE: &'static str = "Murder Motel — Lobby";

    /// Identifier this screen writes into [`SharedSlots::map_name`] on
    /// construction. Read by the title screen's Continue path to
    /// dispatch a resumed run back to the lobby. Symmetric with
    /// [`crate::room_7::Room7Screen::MAP_NAME`].
    pub const MAP_NAME: &'static str = "lobby";

    /// Cell the player lands on when arriving from Room 7. One step
    /// west of [`Self::STAIRS_UP_POS`] so the first directional input
    /// after arrival doesn't bump the player straight back upstairs.
    pub const LOBBY_ARRIVAL_POS: (u16, u16) = (43, 1);

    /// Glyph used to draw the player on top of the underlying floor.
    /// Pulled from [`foglet_game::PLAYER_GLYPH`] so the convention
    /// stays in lockstep with the kit-wide constant.
    pub const PLAYER_GLYPH: char = foglet_game::PLAYER_GLYPH;

    /// Room labels rendered as overlays at fixed columns. The order
    /// matches the left-to-right room order in the map; `Lobby` sits
    /// in the middle. Stored as `(label, column)` pairs so the test
    /// suite can assert each label paints into the expected room
    /// without re-deriving the layout.
    ///
    /// Columns are computed from the map's room geometry: the first
    /// room starts at column 1, each room is 8 cells wide, and the
    /// dividing wall is one cell. So room `i` (0-based) starts at
    /// column `1 + 9 * i`. We pick the third interior column of each
    /// room (`start + 1`) so a six-character label fits without
    /// trampling the room's own walls.
    pub const ROOM_LABELS: &'static [(&'static str, u16)] = &[
        ("Room 1", 2),
        ("Room 2", 11),
        ("Lobby", 21),
        ("Room 3", 29),
        ("Room 4", 38),
    ];

    /// Non-player characters placed on the lobby map (Task 13d).
    ///
    /// Three NPCs satisfies the SPEC §13 acceptance criterion. Two of
    /// them — Bellhop and Maid — exist as flavour with short linear
    /// scripts; the Night Clerk carries the branching dialog the
    /// acceptance fixture requires (a `requires`-gated choice that
    /// only unlocks after the player asks about the murder).
    ///
    /// Coordinates are expressed in map cells. Each NPC sits on a
    /// floor cell inside its room — see the lobby ASCII source for
    /// the room geometry. Glyphs are single uppercase ASCII letters
    /// so even monochrome BBS clients can tell them apart from the
    /// player's `@`.
    pub const NPCS: [Npc; 3] = [
        Npc {
            name: "Night Clerk",
            glyph: 'C',
            x: 24,
            y: 2,
            dialog_yaml: NIGHT_CLERK_DIALOG,
        },
        Npc {
            name: "Bellhop",
            glyph: 'B',
            x: 4,
            y: 2,
            dialog_yaml: BELLHOP_DIALOG,
        },
        Npc {
            name: "Maid",
            glyph: 'M',
            x: 41,
            y: 2,
            dialog_yaml: MAID_DIALOG,
        },
    ];

    /// Catalog of collectable items distributed across the lobby's
    /// five rooms (Task 13e).
    ///
    /// Exactly five items, exactly one per room, satisfies the
    /// SPEC §13 acceptance criterion. Coordinates land on floor cells
    /// that are not occupied by an NPC; the
    /// `map_items_sit_on_walkable_non_npc_cells` test enforces both
    /// invariants so accidental drift surfaces as a failed test rather
    /// than a confusing render. Glyphs are single lowercase ASCII
    /// letters so they're visually distinguishable from the player's
    /// `@` and the uppercase NPC glyphs even on monochrome BBS clients.
    pub const ITEMS: [Item; 5] = [
        // The brass key sits in Room 1 (left of every door) so the
        // player can collect it without first traversing the locked
        // door at x=36 introduced in Task 13f. The matchbook moved to
        // Room 4 so the locked door actually gates progress: there is
        // exactly one collectable behind it, and it is reachable only
        // after the brass key is in the inventory.
        Item {
            id: "brass_key",
            name: "Brass key",
            glyph: 'k',
            x: 6,
            y: 5,
        },
        Item {
            id: "cigarette_case",
            name: "Cigarette case",
            glyph: 'c',
            x: 13,
            y: 2,
        },
        Item {
            id: "newspaper",
            name: "Newspaper clipping",
            glyph: 'n',
            x: 20,
            y: 5,
        },
        Item {
            id: "lipstick",
            name: "Lipstick tube",
            glyph: 'l',
            x: 32,
            y: 2,
        },
        Item {
            id: "matchbook",
            name: "Matchbook",
            glyph: 'm',
            x: 39,
            y: 5,
        },
    ];

    /// Inventory ID required to pass the locked door at
    /// [`Self::LOCKED_DOOR_POS`]. Must match the `id` of the brass-key
    /// catalog entry above; the test
    /// `locked_door_key_id_matches_catalog` asserts this invariant so
    /// a future rename of either field surfaces immediately rather than
    /// silently un-locking the door.
    pub const LOCKED_DOOR_KEY_ID: &'static str = "brass_key";

    /// Position of the lobby's one locked door. The cell is rendered
    /// from the lobby ASCII map's `L` glyph (parsed as a `Custom`
    /// tile kind via the lobby legend) so render code stays a thin
    /// styling pass and the geometry lives in the asset file.
    pub const LOCKED_DOOR_POS: (u16, u16) = (36, 3);

    /// Glyph painted on the locked door cell while it is still locked.
    /// Pulled out so render and tests share one source of truth.
    pub const LOCKED_DOOR_GLYPH: char = 'L';

    /// Glyph painted on the locked door cell once unlocked. Matches the
    /// open-door glyph elsewhere on the map so a player who unlocks the
    /// door visually understands the cell is now equivalent to its
    /// unlocked siblings.
    pub const UNLOCKED_DOOR_GLYPH: char = '+';

    /// Position of the stairs that lead up to Room 7. Inside Room 4,
    /// behind the brass-key gate, so the natural play chain is: brass
    /// key → walk into Room 4 → climb the stairs (only possible after
    /// the Lost-and-Found Drawer hands the player the
    /// [`Self::ROOM_7_KEY_ID`]). A step onto this cell with the room
    /// key in inventory swaps the screen for a fresh
    /// [`crate::room_7::Room7Screen`] via [`ScreenCommand::Replace`].
    pub const STAIRS_UP_POS: (u16, u16) = (44, 1);

    /// Glyph painted on [`Self::STAIRS_UP_POS`]. `>` matches the
    /// roguelike convention for "stairs going up away from this level."
    pub const STAIRS_UP_GLYPH: char = '>';

    /// Position of the win tile — the spot the player must stand on,
    /// after asking the Night Clerk about the murder, to solve the
    /// case (Task 13g). Inside Room 4, which is itself only reachable
    /// once the brass key has unlocked the door, so the natural play
    /// chain is: talk to clerk → ask about the murder → grab the key →
    /// walk into Room 4 → stand here.
    pub const WIN_TILE_POS: (u16, u16) = (43, 4);

    /// Glyph painted on [`Self::WIN_TILE_POS`] while the case is still
    /// open. Stays put after winning: the player has already triggered
    /// the modal, so re-painting an `*` would be misleading. Render
    /// uses [`MapScreen::has_won`] to suppress the glyph post-win.
    pub const WIN_TILE_GLYPH: char = '*';

    /// Narrative flag that must be set for a step onto
    /// [`Self::WIN_TILE_POS`] to win the game. Set by the Night Clerk's
    /// `heard_rumor` branch (see `assets/dialog/night_clerk.yaml`).
    /// Centralised as a constant so the tests can reference the exact
    /// string the runtime checks without re-typing it.
    pub const WIN_FLAG: &'static str = "heard_rumor";

    /// Inventory id for the SPEC §9 Lost-and-Found Drawer key. Distinct
    /// from [`Self::LOCKED_DOOR_KEY_ID`] (the lobby brass key) so the
    /// two collectables can co-exist in `SharedSlots::inventory` and
    /// be queried independently. Lives outside [`Self::ITEMS`] because
    /// the v1.1 proof scene grants it through a prompt rather than a
    /// map cell — there is no glyph or coordinate to record. Surfaced
    /// in the inventory display via [`Self::EXTRA_INVENTORY_ITEMS`].
    pub const ROOM_7_KEY_ID: &'static str = "room_7_key";

    /// `(id, display name)` pairs for inventory entries that are NOT
    /// part of [`Self::ITEMS`] — i.e. items granted by prompts rather
    /// than picked up off the map. The Lost-and-Found Drawer hands the
    /// player [`Self::ROOM_7_KEY_ID`] without a corresponding map
    /// cell; without an entry here it would land in the inventory set
    /// but render as nothing, which is exactly the bug a player sees
    /// when they take the key from the drawer and the `Inventory`
    /// modal stays empty. Sequenced after the map-pickup catalog so a
    /// drawer-granted key sorts beneath the cigarette case the player
    /// already picked up earlier.
    pub const EXTRA_INVENTORY_ITEMS: &'static [(&'static str, &'static str)] =
        &[(Self::ROOM_7_KEY_ID, "Room 7 key")];

    /// Inventory id for the cracked matchbook offered by the
    /// Lost-and-Found Drawer prompt. Re-uses the existing catalog id
    /// so the prompt and the lobby map item never duplicate the same
    /// keepsake in the player's pockets — picking it up either way
    /// flips the same `BTreeSet` entry.
    pub const MATCHBOOK_ID: &'static str = "matchbook";

    /// Narrative flag set when the player reads the receipt at the
    /// Lost-and-Found Drawer (SPEC §9 step 3). Flag rather than item
    /// because the receipt isn't carried; reading it unlocks branches
    /// downstream of the drawer scene without occupying an inventory
    /// slot.
    pub const RECEIPT_READ_FLAG: &'static str = "receipt_read";

    /// Map cell that hosts the Lost-and-Found Drawer (Task 10f). Sits
    /// next to the Night Clerk at column 24 so the prompt's "behind the
    /// desk" framing is geographically honest: the player walks up to
    /// the front desk and finds the drawer beside the clerk. The cell
    /// itself is treated as furniture — non-walkable, blocking via the
    /// same gate that protects NPC tiles — so a press of the search key
    /// from any orthogonally adjacent floor cell opens the prompt.
    pub const LOST_AND_FOUND_POS: (u16, u16) = (25, 2);

    /// Glyph painted at [`Self::LOST_AND_FOUND_POS`]. A single uppercase
    /// `D` (for Drawer) keeps the affordance legible on monochrome BBS
    /// clients without colliding with the existing NPC glyphs (`B`/`C`/
    /// `M`) or item glyphs (lowercase). Pulled out as a constant so the
    /// renderer and the headless `rendered_rows` test consult one
    /// source of truth.
    pub const LOST_AND_FOUND_GLYPH: char = 'D';

    /// Always-visible movement and navigation cues. Concatenated with
    /// any contextual verbs by [`Self::hint_line`] to form the full
    /// hint shown beneath the map.
    const HINT_BASE_PREFIX: &'static str = "Move: arrows/hjkl";
    const HINT_BASE_SUFFIX: &'static str = "Inv: I  Back: Esc  Quit: Q";

    /// Build a lobby map screen with fresh, unshared slots. Used by
    /// tests that want an isolated screen instance and by callers that
    /// don't need to participate in the Task 13h save/load handshake.
    pub fn new_lobby(start_x: u16, start_y: u16) -> Self {
        let slots = SharedSlots::default();
        slots.reset(start_x, start_y);
        Self::with_shared(start_x, start_y, slots)
    }

    /// Build a lobby map screen against caller-supplied [`SharedSlots`].
    ///
    /// The slots' `player.x/y` are used as the spawn unless the menu
    /// has already populated them from a loaded save — in which case
    /// the saved coordinates take precedence over `(start_x, start_y)`.
    /// Walkability is still checked: if the saved cell has been re-
    /// authored into a wall the spiral fallback steers the player onto
    /// the nearest floor, mirroring the misconfigured-config path.
    pub fn with_shared(start_x: u16, start_y: u16, slots: SharedSlots) -> Self {
        let legend = lobby_legend();
        let map =
            parse_map(LOBBY_MAP_TEXT, &legend).expect("embedded lobby map parses against legend");
        let screen = Self {
            map,
            slots,
            turns_status: RefCell::new(None),
        };
        // Mark the slots as "we are now on the lobby" so a save
        // snapshot taken before the next transition records the right
        // map identifier. Symmetric with
        // [`crate::room_7::Room7Screen::with_shared`].
        *screen.slots.map_name.borrow_mut() = Self::MAP_NAME.to_string();
        // Decide the spawn cell. If the slots arrived empty (e.g. a
        // brand-new game) we honour the caller's `(start_x, start_y)`;
        // otherwise the slots already carry the loaded player position
        // and we trust that. Either way we then pass the chosen cell
        // through the walkability gate so a re-authored map can never
        // strand the player inside a wall.
        let initial = {
            let p = screen.slots.player.borrow();
            if p.x == 0 && p.y == 0 {
                (start_x, start_y)
            } else {
                (p.x, p.y)
            }
        };
        let (chosen_x, chosen_y) = if screen.map.is_walkable(initial.0, initial.1) {
            initial
        } else {
            screen
                .find_nearest_walkable(initial.0, initial.1)
                .unwrap_or((1, 1))
        };
        let mut p = screen.slots.player.borrow_mut();
        p.x = chosen_x;
        p.y = chosen_y;
        drop(p);
        screen
    }

    /// Player coordinates, exposed so tests and future siblings (the
    /// inventory screen, the save manager) can read the position
    /// without reaching into private state.
    pub fn player(&self) -> (u16, u16) {
        let p = self.slots.player.borrow();
        (p.x, p.y)
    }

    /// Reference to the parsed map. Useful for tests asserting that
    /// the lobby has the expected room shape.
    pub fn map(&self) -> &Map {
        &self.map
    }

    /// Attempt to move the player by `(dx, dy)` (each ±1).
    ///
    /// Returns `true` if the step succeeded. The walkability check
    /// short-circuits when the target would underflow the unsigned
    /// coordinate space, so stepping left from column 0 silently
    /// no-ops instead of wrapping to `u16::MAX`.
    pub fn try_move(&mut self, dx: i32, dy: i32) -> bool {
        let (cur_x, cur_y) = {
            let p = self.slots.player.borrow();
            (p.x, p.y)
        };
        let target_x = match (cur_x as i32).checked_add(dx) {
            Some(v) if v >= 0 => v as u16,
            _ => return false,
        };
        let target_y = match (cur_y as i32).checked_add(dy) {
            Some(v) if v >= 0 => v as u16,
            _ => return false,
        };
        if !self.map.is_walkable(target_x, target_y) {
            return false;
        }
        // Locked-door gate (Task 13f). The lobby ASCII tags one cell
        // with the `L` legend kind; passage is rejected unless the
        // matching key item sits in the player's inventory. The map
        // model itself treats `Custom` tiles as walkable — the lock
        // is a screen-level concern, parallel to NPC blocking below.
        if self.is_locked_door_blocking(target_x, target_y) {
            return false;
        }
        // Stairs-up gate. Symmetric with the locked-door gate above —
        // a player without the Room 7 key bumps the cell as if it
        // were a wall. Once the key is in inventory the cell is
        // walkable; the post-move handler emits Replace as soon as
        // the player completes a step onto it.
        if self.is_stairs_blocking(target_x, target_y) {
            return false;
        }
        // NPCs are solid: walking into one is converted to "stand
        // adjacent". The talk affordance (Enter while next to an
        // NPC) handles interaction; without this guard the player
        // would have to step *off* an NPC's cell to address them,
        // which is the wrong feel for a top-down game.
        if Self::npc_at(target_x, target_y).is_some() {
            return false;
        }
        // Lost-and-Found Drawer (Task 10f) is furniture. Blocking the
        // step keeps the desk's framing consistent — the player stands
        // beside the drawer and presses the search key, instead of
        // standing *on* the drawer to interact with it.
        if Self::is_lost_and_found_at(target_x, target_y) {
            return false;
        }
        {
            let mut p = self.slots.player.borrow_mut();
            p.x = target_x;
            p.y = target_y;
        }
        // Items pick up on step. The render path filters collected
        // items out of the overlay, so the cell visually clears the
        // same frame the inventory grows. Items aren't blocking — a
        // stranded item under foot would otherwise trap the player
        // until they pressed Enter, which is the wrong feel here.
        //
        // `BTreeSet::insert` returns `true` only when the value was
        // newly added; gating the feedback line on it means a step
        // back through an already-collected cell (the glyph is gone
        // but the cell still walkable) does NOT re-narrate the pick-
        // up — only the moment the inventory actually grew.
        if let Some(item) = Self::item_at(target_x, target_y) {
            let newly_added = self
                .slots
                .inventory
                .borrow_mut()
                .insert(item.id.to_string());
            if newly_added {
                *self.slots.feedback.borrow_mut() = Some(foglet_game::FeedbackLine::success(
                    format!("Picked up {}.", item.name),
                ));
            }
        }
        true
    }

    /// Find the NPC standing on the given cell, if any. Pulled out
    /// so both [`Self::try_move`] (blocking) and the talk handler
    /// (adjacency probe) consult the same roster.
    pub fn npc_at(x: u16, y: u16) -> Option<&'static Npc> {
        Self::NPCS.iter().find(|n| n.x == x && n.y == y)
    }

    /// NPC the player can talk to right now, or `None`. Returns the
    /// first NPC whose cell is the player's own cell or one of the
    /// four cardinal neighbours — diagonal "talk through the wall"
    /// would be ambiguous when an NPC is in a different room and
    /// also feels wrong on a square grid.
    pub fn nearby_npc(&self) -> Option<&'static Npc> {
        const OFFSETS: &[(i32, i32)] = &[(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)];
        let (px, py) = self.player();
        for (dx, dy) in OFFSETS {
            let cx = px as i32 + dx;
            let cy = py as i32 + dy;
            if cx < 0 || cy < 0 {
                continue;
            }
            if let Some(npc) = Self::npc_at(cx as u16, cy as u16) {
                return Some(npc);
            }
        }
        None
    }

    /// Whether the cell at `(x, y)` is the Lost-and-Found Drawer
    /// (Task 10f). Centralised so the renderer, movement gate, and
    /// adjacency probe consult the same answer instead of re-deriving
    /// the comparison.
    pub fn is_lost_and_found_at(x: u16, y: u16) -> bool {
        (x, y) == Self::LOST_AND_FOUND_POS
    }

    /// Whether the player can act on the Lost-and-Found Drawer right
    /// now. Mirrors [`Self::nearby_npc`]: the player must stand on the
    /// drawer cell or one of the four cardinal neighbours. Diagonals
    /// are excluded so a player two rooms away never accidentally
    /// reaches across the wall to a drawer they cannot see.
    pub fn nearby_lost_and_found(&self) -> bool {
        const OFFSETS: &[(i32, i32)] = &[(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)];
        let (px, py) = self.player();
        OFFSETS.iter().any(|(dx, dy)| {
            let cx = px as i32 + dx;
            let cy = py as i32 + dy;
            if cx < 0 || cy < 0 {
                return false;
            }
            Self::is_lost_and_found_at(cx as u16, cy as u16)
        })
    }

    /// Whether the player is positioned to interact with the Night
    /// Clerk's vendor menu (Task 11g). Mirrors [`Self::nearby_npc`]
    /// but filters by name so a future second NPC at an adjacent
    /// cell — say, a passing maid — can never accidentally route the
    /// `b`/`B` "buy" affordance into the wrong dialog. Diagonals are
    /// excluded for the same reason `nearby_npc` rejects them: a
    /// cardinal-only adjacency check matches the player's mental model
    /// of "I'm standing next to the counter".
    pub fn nearby_clerk(&self) -> bool {
        self.nearby_npc()
            .is_some_and(|npc| npc.name == "Night Clerk")
    }

    /// Build the one-line hint shown beneath the map for the current
    /// player position. Movement, inventory, and navigation cues are
    /// always visible; verb hints (`Talk`, `Buy`, `Search`) only appear
    /// when the player stands adjacent to a tile that accepts them, so
    /// the hint never advertises an action the keypress would silently
    /// reject.
    pub fn hint_line(&self) -> String {
        // Owned `String` segments rather than `&str` because the Task
        // 13b status segment (`"Turns: N/D"`) is formatted on the fly
        // and would otherwise be a dangling reference. The static
        // prefix/suffix get a one-time `.to_string()` to share the
        // segment vector's element type.
        let mut segments: Vec<String> = vec![Self::HINT_BASE_PREFIX.to_string()];
        if self.nearby_npc().is_some() {
            segments.push("Talk: Enter".to_string());
        }
        if self.nearby_clerk() {
            segments.push("Buy: B".to_string());
        }
        if self.nearby_lost_and_found() {
            segments.push("Search: X".to_string());
        }
        // SPEC_v2 §Task 13b: paint today's clue-turn balance into the
        // hint line whenever the cache is populated. The cache stays
        // empty when `[turns]` is absent (single-player runs / pre-v2
        // games), so the segment naturally drops out for those games
        // without a separate feature flag.
        if let Some(status) = self.turns_status.borrow().as_ref() {
            segments.push(format!(
                "Turns: {}/{}",
                status.remaining, status.daily_allowance
            ));
        }
        segments.push(Self::HINT_BASE_SUFFIX.to_string());
        segments.join("  ")
    }

    /// Shared handle to the narrative-flag store. Cloned so the
    /// dialog screen and the map screen mutate the same `RefCell`.
    pub fn flags(&self) -> Rc<RefCell<FlagSet>> {
        Rc::clone(&self.slots.flags)
    }

    /// Borrow the screen's [`SharedSlots`]. Used by the menu screen so
    /// "New Game" can reset all slots without reaching into private
    /// fields, and by tests asserting save-related invariants.
    pub fn slots(&self) -> &SharedSlots {
        &self.slots
    }

    /// Shared handle to the inventory store. Cloned so the
    /// [`InventoryScreen`] reads the same set the [`MapScreen`] writes
    /// when the player walks over an item. Task 13h will lift this
    /// onto the save manager so contents survive process exit; until
    /// then it lives on the map screen for the play session.
    pub fn inventory(&self) -> Rc<RefCell<BTreeSet<String>>> {
        Rc::clone(&self.slots.inventory)
    }

    /// The catalog item sitting on `(x, y)`, or `None`. Walks the
    /// static [`Self::ITEMS`] array; the catalog is small enough that
    /// a linear scan matches how [`Self::npc_at`] queries the NPC
    /// roster and avoids per-frame map allocations.
    pub fn item_at(x: u16, y: u16) -> Option<&'static Item> {
        Self::ITEMS.iter().find(|i| i.x == x && i.y == y)
    }

    /// Whether the given item ID has already been collected. Render
    /// and `try_move` both consult this so an item disappears from
    /// the map atomically with its appearance in the inventory.
    pub fn is_collected(&self, id: &str) -> bool {
        self.slots.inventory.borrow().contains(id)
    }

    /// Whether the cell at `(x, y)` is the lobby's locked door. The
    /// position lives in [`Self::LOCKED_DOOR_POS`]; centralising the
    /// check means the renderer and movement gate consult the same
    /// answer instead of re-deriving the comparison.
    pub fn is_locked_door_at(x: u16, y: u16) -> bool {
        (x, y) == Self::LOCKED_DOOR_POS
    }

    /// Whether the player currently holds the locked-door key item.
    /// Used by both [`Self::is_locked_door_blocking`] and the renderer
    /// (so an unlocked door is painted in the open-door glyph).
    pub fn has_locked_door_key(&self) -> bool {
        self.is_collected(Self::LOCKED_DOOR_KEY_ID)
    }

    /// Whether a step into `(x, y)` should be blocked by the locked
    /// door. True only for the locked-door cell while the player is
    /// missing the matching key item — every other case (a non-locked
    /// cell, or the locked cell with the key in hand) returns false.
    pub fn is_locked_door_blocking(&self, x: u16, y: u16) -> bool {
        Self::is_locked_door_at(x, y) && !self.has_locked_door_key()
    }

    /// Whether the cell at `(x, y)` is the lobby's stairs-up cell.
    /// Mirror of [`Self::is_locked_door_at`] — same one-call shape so
    /// every read site (renderer, movement gate, post-move handler)
    /// consults a single source of truth.
    pub fn is_stairs_at(x: u16, y: u16) -> bool {
        (x, y) == Self::STAIRS_UP_POS
    }

    /// Whether the player currently holds the Room 7 key — the gate
    /// keeping the stairs up to Room 7 closed. Pulled out so render
    /// (which paints the `>` glyph in different styles depending on
    /// whether the player has earned the climb) and the movement
    /// gate share one predicate.
    pub fn has_room_7_key(&self) -> bool {
        self.is_collected(Self::ROOM_7_KEY_ID)
    }

    /// Whether a step into `(x, y)` should be blocked by the stairs
    /// gate. True only for the stairs cell while the player is
    /// missing the Room 7 key — every other case returns false.
    pub fn is_stairs_blocking(&self, x: u16, y: u16) -> bool {
        Self::is_stairs_at(x, y) && !self.has_room_7_key()
    }

    /// Whether the win condition has already fired. Exposed so tests
    /// can assert the latch without poking at private state and so the
    /// renderer can drop the win-tile glyph after the modal triggers.
    pub fn has_won(&self) -> bool {
        self.slots.player.borrow().won
    }

    /// Whether stepping onto the win tile right now would solve the
    /// case. True only when the player stands on [`Self::WIN_TILE_POS`]
    /// with [`Self::WIN_FLAG`] set and has not already triggered the
    /// modal. Centralising the predicate keeps render and
    /// [`Self::handle_input`] reading the same answer.
    fn should_trigger_win(&self) -> bool {
        let p = self.slots.player.borrow();
        if p.won {
            return false;
        }
        if (p.x, p.y) != Self::WIN_TILE_POS {
            return false;
        }
        drop(p);
        self.slots.flags.borrow().contains(Self::WIN_FLAG)
    }

    /// Latch the win flag and return the screen command that should be
    /// emitted in response to the player's last move. Called after
    /// every successful or attempted movement so a step onto the win
    /// tile fires the modal in the same frame the move resolves.
    fn maybe_win_command(&mut self) -> ScreenCommand {
        if self.should_trigger_win() {
            self.slots.player.borrow_mut().won = true;
            ScreenCommand::Push(Box::new(WinScreen))
        } else {
            ScreenCommand::None
        }
    }

    /// If the player just completed a step onto [`Self::STAIRS_UP_POS`],
    /// build the [`ScreenCommand::Replace`] that swaps this lobby
    /// screen for a fresh [`crate::room_7::Room7Screen`] sharing the
    /// same slots. Sets the player position to
    /// [`crate::room_7::Room7Screen::ARRIVAL_POS`] before constructing
    /// the new screen so Room 7's `with_shared` constructor — which
    /// honours the slots' saved coordinates when they're walkable on
    /// its map — drops the player at the canonical arrival cell
    /// instead of wherever the lobby left them. The transition is
    /// gated by [`Self::is_stairs_blocking`] in `try_move`, so this
    /// helper only fires when the player legitimately stood on the
    /// stairs cell with the Room 7 key in hand.
    fn maybe_take_stairs(&mut self, ctx: &mut GameContext<'_>) -> ScreenCommand {
        let on_stairs = {
            let p = self.slots.player.borrow();
            (p.x, p.y) == Self::STAIRS_UP_POS
        };
        if !on_stairs {
            return ScreenCommand::None;
        }
        // Defensive: the gate in `try_move` should already prevent
        // ever standing here without the key, but we re-check rather
        // than trust callers.
        if !self.has_room_7_key() {
            return ScreenCommand::None;
        }
        // SPEC_v2 §Task 12b: stamp the shared-world record of "who
        // opened Room 7, and when". Done before the screen swap so a
        // freshly-loaded Room 7 screen (Task 12c onward) can read the
        // canonical pair on its first frame. Failures are logged and
        // swallowed: the player still transitions, the bulletin just
        // misses an entry. The kit's terminal-safety contract forbids
        // bubbling DB errors out of `handle_input` because the screen
        // stack is mid-transition.
        // SPEC_v2 §Task 12c: derive the arrival banner *while* recording
        // the opening. We compute `arrival_feedback` here (rather than
        // inside `Room7Screen::with_shared`) so that the world-DB
        // borrow stays scoped to the current `handle_input` tick — the
        // forthcoming Room 7 screen does not hold a `&WorldDb`.
        let mut arrival_feedback: Option<FeedbackLine> = None;
        if let Some(world) = ctx.world_db {
            // Errors are swallowed on purpose: the screen stack is
            // mid-transition, the kit has no logging facility wired
            // into the runtime yet (the workspace's `tracing` dep is
            // not pulled into `foglet_game` — see Cargo.toml), and
            // the transition itself is not gated on the recording
            // landing. A failed write means the bulletin (Task 13d)
            // misses one entry; the player still arrives in Room 7.
            // Production diagnostics will land alongside the
            // forthcoming kit-side `tracing` integration; until then
            // a `let _` keeps the call paths honest about which
            // failures are intentionally non-fatal.
            if let Ok(player) = world.upsert_player(ctx.foglet) {
                if let Ok(opening) = crate::world::record_room_7_opening(world, player.id) {
                    arrival_feedback =
                        crate::world::shared_room_7_arrival_feedback(&opening, player.id);
                    // SPEC_v2 §Task 13c: log the Room 7 opening into
                    // `world_events` so the upcoming bulletin (Task
                    // 13d) can render "Room 7 was unlocked." in the
                    // lobby ledger. The helper internally short-
                    // circuits to a no-op when this player isn't the
                    // first opener and swallows DB errors for the same
                    // mid-transition terminal-safety reasons the
                    // surrounding `let _` branches cite.
                    let _ = crate::world::append_room_7_opened_event(world, &opening);
                }
            }
        }
        {
            let mut p = self.slots.player.borrow_mut();
            p.x = crate::room_7::Room7Screen::ARRIVAL_POS.0;
            p.y = crate::room_7::Room7Screen::ARRIVAL_POS.1;
        }
        // Replace any stale lobby feedback. Either the Task 12c "you
        // are not the first" banner takes the slot (later opener), or
        // we clear it so the body's flavour line is the only thing
        // that appears until the player triggers another prompt
        // (current player is the opener / world DB unavailable).
        *self.slots.feedback.borrow_mut() = arrival_feedback;
        ScreenCommand::Replace(Box::new(crate::room_7::Room7Screen::with_shared(
            self.slots.clone(),
        )))
    }

    /// Combine the post-move transition checks: stairs first (Replace
    /// the screen), then the win-tile latch (Push a modal). Order
    /// matters — taking the stairs is a hard transition and shouldn't
    /// be silently overridden by a stale win-tile read.
    fn after_move(&mut self, ctx: &mut GameContext<'_>) -> ScreenCommand {
        match self.maybe_take_stairs(ctx) {
            ScreenCommand::None => self.maybe_win_command(),
            cmd => cmd,
        }
    }

    /// Locate a walkable cell near `(x, y)` by widening rings.
    ///
    /// Used as a self-correcting safety net for misconfigured spawn
    /// coordinates. Capped at a small radius — searching forever on a
    /// pathologically hostile map is worse than the (1, 1) fallback.
    fn find_nearest_walkable(&self, x: u16, y: u16) -> Option<(u16, u16)> {
        for radius in 1i32..=8 {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs() != radius && dy.abs() != radius {
                        continue;
                    }
                    let cx = x as i32 + dx;
                    let cy = y as i32 + dy;
                    if cx < 0 || cy < 0 {
                        continue;
                    }
                    let cx = cx as u16;
                    let cy = cy as u16;
                    if self.map.is_walkable(cx, cy) {
                        return Some((cx, cy));
                    }
                }
            }
        }
        None
    }

    /// Build the rendered lines for the current map state.
    ///
    /// Pulled out of the `render` body so `cargo test` can assert the
    /// painted glyphs without going through `ratatui::Terminal`. The
    /// returned `Vec<String>` is one entry per map row; the player is
    /// stamped on top of the floor cell using [`Self::PLAYER_GLYPH`].
    pub fn rendered_rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .map
            .cells
            .iter()
            .map(|row| row.iter().map(|tile| tile.glyph).collect::<String>())
            .collect();
        // Locked-door glyph swap mirrors the styled render path: once
        // unlocked, paint the cell as a regular `+` so headless tests
        // see the same character the player does.
        if self.has_locked_door_key() {
            let (lx, ly) = Self::LOCKED_DOOR_POS;
            if (ly as usize) < rows.len() {
                let row = &mut rows[ly as usize];
                let col = lx as usize;
                if col < row.len() {
                    let mut chars: Vec<char> = row.chars().collect();
                    chars[col] = Self::UNLOCKED_DOOR_GLYPH;
                    *row = chars.into_iter().collect();
                }
            }
        }
        // Win-tile glyph stamp (Task 13g). Mirrors the styled render
        // path's behaviour: paint `*` until the player has won, then
        // fall back to the underlying floor glyph. Stamped *before* the
        // player overlay below so standing on the tile still shows the
        // player's `@` rather than the marker.
        let (px, py, won) = {
            let p = self.slots.player.borrow();
            (p.x, p.y, p.won)
        };
        if !won {
            let (wx, wy) = Self::WIN_TILE_POS;
            if (wy as usize) < rows.len() {
                let row = &mut rows[wy as usize];
                let col = wx as usize;
                if col < row.len() {
                    let mut chars: Vec<char> = row.chars().collect();
                    chars[col] = Self::WIN_TILE_GLYPH;
                    *row = chars.into_iter().collect();
                }
            }
        }
        // Lost-and-Found Drawer glyph (Task 10f). Painted unconditionally
        // — there is no "drawer consumed" state because the prompt's own
        // disabled-(K) branch (Task 10d) handles the only "already
        // taken" case. Stamped before the player overlay below so
        // standing adjacent to the drawer never hides its glyph.
        let (lx, ly) = Self::LOST_AND_FOUND_POS;
        if (ly as usize) < rows.len() {
            let row = &mut rows[ly as usize];
            let col = lx as usize;
            if col < row.len() {
                let mut chars: Vec<char> = row.chars().collect();
                chars[col] = Self::LOST_AND_FOUND_GLYPH;
                *row = chars.into_iter().collect();
            }
        }
        // Stamp the player glyph by replacing the byte at the player's
        // column with `@`. The lobby legend uses ASCII space/`#`/`+`
        // (all 1-byte UTF-8) and the player glyph is also ASCII, so
        // byte-level replacement is safe here. If the legend ever
        // grows multi-byte glyphs this needs to switch to a
        // char-aware splice.
        if (py as usize) < rows.len() {
            let row = &mut rows[py as usize];
            let col = px as usize;
            if col < row.len() {
                let mut chars: Vec<char> = row.chars().collect();
                chars[col] = Self::PLAYER_GLYPH;
                *row = chars.into_iter().collect();
            }
        }
        rows
    }
}

impl Screen for MapScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre the map inside the frame. The +2 accounts for the
        // bordered block; without it the right wall would be clipped
        // by the border on terminals exactly at the 80-column floor.
        let area = centred_rect(self.map.width + 2, self.map.height + 2, frame.area());

        let player_glyph_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let label_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let npc_style = Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD);
        let item_style = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        // Locked door is painted red+bold while still locked so the
        // player has an unmissable visual cue that the cell is gating
        // them. Once the brass key is in inventory the cell falls back
        // to the open-door glyph in default style — no separate
        // "unlocked but special" state to maintain.
        let locked_door_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
        // Win tile is painted in the same red+bold register as the
        // locked door so the player reads "important" from the colour
        // alone. Drops once the case has already been solved (see
        // `has_won`) so a returning player does not see a stale marker.
        let win_tile_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);

        let (px, py, won) = {
            let p = self.slots.player.borrow();
            (p.x, p.y, p.won)
        };
        let mut lines: Vec<Line<'_>> = Vec::with_capacity(self.map.cells.len());
        for (y, row) in self.map.cells.iter().enumerate() {
            // Build each row as a sequence of spans: most cells are a
            // single styleless character, but the player's cell gets
            // the bold-yellow `@` overlay, and row 1 has the room
            // labels stamped over the floor cells.
            let mut spans: Vec<Span<'_>> = Vec::with_capacity(row.len());
            let row_string: String = row.iter().map(|t| t.glyph).collect();
            // Compute a per-column override lookup for this row so the
            // span loop stays a flat O(width) walk.
            let labels_for_row = if y == 1 {
                Self::ROOM_LABELS.to_vec()
            } else {
                Vec::new()
            };

            let mut x = 0usize;
            while x < row_string.len() {
                // Player glyph wins over labels and base cells.
                if (y as u16) == py && (x as u16) == px {
                    spans.push(Span::styled(
                        Self::PLAYER_GLYPH.to_string(),
                        player_glyph_style,
                    ));
                    x += 1;
                    continue;
                }
                // NPCs win over labels but lose to the player. The
                // player and NPCs never share a cell because
                // `try_move` blocks the player from stepping onto
                // an NPC, so the priority here is purely a tiebreak
                // against the room-label overlay.
                if let Some(npc) = Self::npc_at(x as u16, y as u16) {
                    spans.push(Span::styled(npc.glyph.to_string(), npc_style));
                    x += 1;
                    continue;
                }
                // Items appear under the player and NPCs but over
                // labels and plain floor — once collected they vanish
                // from the map until a future replay/save reset.
                if let Some(item) = Self::item_at(x as u16, y as u16) {
                    if !self.is_collected(item.id) {
                        spans.push(Span::styled(item.glyph.to_string(), item_style));
                        x += 1;
                        continue;
                    }
                }
                // Locked door (Task 13f). The lobby map's `L` cell
                // paints red+bold while the player is missing the
                // brass key, then collapses to the regular `+` glyph
                // once the key is in inventory. No items or labels
                // overlap this cell, so this branch is unconditional.
                if Self::is_locked_door_at(x as u16, y as u16) {
                    if self.has_locked_door_key() {
                        spans.push(Span::raw(Self::UNLOCKED_DOOR_GLYPH.to_string()));
                    } else {
                        spans.push(Span::styled(
                            Self::LOCKED_DOOR_GLYPH.to_string(),
                            locked_door_style,
                        ));
                    }
                    x += 1;
                    continue;
                }
                // Win-tile marker (Task 13g). Painted only while the
                // case is open; once the player has triggered the
                // modal the cell falls back to its underlying floor
                // glyph so a return visit reads as "ordinary room".
                if !won && (x as u16, y as u16) == Self::WIN_TILE_POS {
                    spans.push(Span::styled(
                        Self::WIN_TILE_GLYPH.to_string(),
                        win_tile_style,
                    ));
                    x += 1;
                    continue;
                }
                // Lost-and-Found Drawer glyph (Task 10f). Painted in the
                // same cyan+bold register as the room labels so the
                // affordance reads as "important UI furniture" without
                // getting confused with the locked door's red warning
                // colour. The drawer cell is non-walkable so the player
                // glyph never collides with it on this branch.
                if Self::is_lost_and_found_at(x as u16, y as u16) {
                    spans.push(Span::styled(
                        Self::LOST_AND_FOUND_GLYPH.to_string(),
                        label_style,
                    ));
                    x += 1;
                    continue;
                }
                // Stairs-up cell. Same cyan+bold register as the
                // drawer and room labels so the affordance reads as
                // "important UI furniture." Painted unconditionally —
                // a player without the Room 7 key bumps the cell like
                // a wall, the same feedback model the locked door
                // uses.
                if Self::is_stairs_at(x as u16, y as u16) {
                    spans.push(Span::styled(Self::STAIRS_UP_GLYPH.to_string(), label_style));
                    x += 1;
                    continue;
                }
                // Room label?
                if let Some((label, _)) = labels_for_row.iter().find(|(_, lx)| (*lx as usize) == x)
                {
                    spans.push(Span::styled((*label).to_string(), label_style));
                    x += label.len();
                    continue;
                }
                // Plain cell.
                let ch = row_string.as_bytes()[x] as char;
                spans.push(Span::raw(ch.to_string()));
                x += 1;
            }
            lines.push(Line::from(spans));
        }

        // Append a one-line hint under the map so a fresh player knows
        // how to move and exit. Kept as a separate line so the map
        // grid stays a clean rectangle aligned with its block borders.
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);

        // Hint line directly below the map block. We size it as one
        // row tall and the same width as the map block so it looks
        // anchored to the map rather than floating in the middle of
        // the screen.
        let hint_area = Rect {
            x: area.x,
            y: area.y.saturating_add(area.height),
            width: area.width,
            height: 1,
        };
        if hint_area.y < frame.area().height {
            let hint = Paragraph::new(self.hint_line()).alignment(Alignment::Center);
            frame.render_widget(hint, hint_area);
        }

        // Feedback line (Task 10f). Sits one row below the hint so the
        // post-action narration from prompts (e.g. "Moved Room 7 key
        // to inventory.") lands in a consistent spot regardless of map
        // size. We borrow read-only and clone the line because
        // `FeedbackLine::render` takes `&self` and writes directly into
        // the frame's buffer.
        let feedback_y = hint_area.y.saturating_add(1);
        if feedback_y < frame.area().height {
            if let Some(line) = self.slots.feedback.borrow().clone() {
                let feedback_area = Rect {
                    x: area.x,
                    y: feedback_y,
                    width: area.width,
                    height: 1,
                };
                line.render(feedback_area, frame.buffer_mut());
            }
        }
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Cardinal movement, both arrow keys and vi-style aliases.
            // The screen does not announce a "blocked" state when a
            // move fails — the lack of motion is the feedback.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.try_move(0, -1);
                self.after_move(ctx)
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.try_move(0, 1);
                self.after_move(ctx)
            }
            Input::Left | Input::Char('h') => {
                self.try_move(-1, 0);
                self.after_move(ctx)
            }
            Input::Right | Input::Char('l') | Input::Char('L') => {
                self.try_move(1, 0);
                self.after_move(ctx)
            }
            // Talk affordance: Enter (or `t`) when the player is
            // adjacent to an NPC opens that NPC's dialog. With no
            // one nearby the key is inert — silent rejection rather
            // than an error message, the same feedback model as
            // walking into a wall.
            Input::Enter | Input::Char('t') | Input::Char('T') => match self.nearby_npc() {
                Some(npc) => ScreenCommand::Push(Box::new(DialogScreen::new(npc, self.flags()))),
                None => ScreenCommand::None,
            },
            // Open the inventory modal. `i` is the canonical RPG key
            // for this affordance and the help screen documents it
            // explicitly so a player can find their pockets without
            // hunting for the right keystroke.
            Input::Char('i') | Input::Char('I') => {
                ScreenCommand::Push(Box::new(InventoryScreen::new(self.inventory())))
            }
            // Vendor affordance (Task 11g). When the player stands
            // next to the Night Clerk, `b`/`B` ("buy") opens the SPEC
            // §9 step 4 vendor prompt; everywhere else the key is
            // inert. Mirrors the drawer's `x`/`X` route so both proof
            // scenes share an interaction grammar — adjacency + verb
            // key — and neither leaks into a debug menu.
            Input::Char('b') | Input::Char('B') => {
                if self.nearby_clerk() {
                    *self.slots.feedback.borrow_mut() = None;
                    ScreenCommand::Push(Box::new(night_clerk_vendor_screen(self.slots.clone())))
                } else {
                    ScreenCommand::None
                }
            }
            // Search affordance (Task 10f). When the player stands next
            // to the Lost-and-Found Drawer, `x`/`X` opens the SPEC §9
            // loot prompt; everywhere else the key is inert (silent
            // rejection, same model as bumping a wall). Keeps the prompt
            // reachable from normal lobby play without bolting it onto
            // a debug menu.
            Input::Char('x') | Input::Char('X') => {
                if self.nearby_lost_and_found() {
                    // SPEC_v2 §Task 13a: examining clue hotspots spends
                    // one daily turn. Route through the helper so the
                    // upsert + spend + result-mapping live in one place
                    // (see `crate::world::spend_clue_inspection_turn`
                    // for the full rationale).
                    //
                    // Outcome routing:
                    // - `Spent` / `NotConfigured` / `Failed`: open the
                    //   prompt. A `Failed` SQLite hiccup must not
                    //   soft-lock the player; the kit's terminal-safety
                    //   contract forbids panicking out of `handle_input`.
                    // - `InsufficientTurns`: surface the SPEC §Task 13a
                    //   feedback line and DO NOT push the prompt — the
                    //   player needs to come back tomorrow.
                    use crate::world::{
                        spend_clue_inspection_turn, ClueInspectionOutcome, NO_CLUE_TURNS_FEEDBACK,
                    };
                    let outcome = match ctx.world_db {
                        Some(world) => spend_clue_inspection_turn(
                            world,
                            ctx.foglet,
                            ctx.config,
                            &*self.slots.date_provider,
                        ),
                        // No world DB attached (single-player or
                        // headless tests that don't stand one up):
                        // treat as if `[turns]` was opted out of so
                        // the affordance remains reachable.
                        None => ClueInspectionOutcome::NotConfigured,
                    };
                    match outcome {
                        ClueInspectionOutcome::InsufficientTurns { balance } => {
                            *self.slots.feedback.borrow_mut() =
                                Some(FeedbackLine::error(NO_CLUE_TURNS_FEEDBACK));
                            // Sync the status cache to the rejected
                            // balance so the hint line shows "Turns:
                            // 0/D" the very next frame — without this
                            // a stale "1/D" from before the day's
                            // final spend would linger until tick re-
                            // queried (SPEC_v2 §Task 13b).
                            if let Some(turns) = ctx.config.turns.as_ref() {
                                *self.turns_status.borrow_mut() =
                                    Some(crate::world::RemainingTurns {
                                        remaining: balance,
                                        daily_allowance: turns.daily_allowance,
                                    });
                            }
                            return ScreenCommand::None;
                        }
                        ClueInspectionOutcome::Spent { remaining } => {
                            // Update the status cache from the spend's
                            // post-decrement balance — the screen does
                            // not re-query SQLite for the hint line, so
                            // this is the canonical refresh point for a
                            // successful inspection (SPEC_v2 §Task 13b).
                            if let Some(turns) = ctx.config.turns.as_ref() {
                                *self.turns_status.borrow_mut() =
                                    Some(crate::world::RemainingTurns {
                                        remaining,
                                        daily_allowance: turns.daily_allowance,
                                    });
                            }
                        }
                        ClueInspectionOutcome::NotConfigured | ClueInspectionOutcome::Failed(_) => {
                        }
                    }
                    // Clear any prior feedback so a fresh interaction
                    // never pops in under stale narration from the last
                    // action — the prompt will write its own line on
                    // dismissal.
                    *self.slots.feedback.borrow_mut() = None;
                    ScreenCommand::Push(Box::new(lost_and_found_drawer_screen(self.slots.clone())))
                } else {
                    ScreenCommand::None
                }
            }
            // Esc / Backspace pop back to the main menu so a curious
            // player can return to the splash flow without quitting.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,
            // Q and Ctrl-C remain hard-quit affordances everywhere.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            _ => ScreenCommand::None,
        }
    }

    /// Per-frame hook: when the SPEC_v2 §Task 13b status cache is
    /// empty and a world DB is attached, lazily populate it via
    /// [`crate::world::read_remaining_turns`]. Subsequent frames
    /// short-circuit on the `Some` check, and clue-spend handlers
    /// keep the cache fresh in place — so this hook is effectively a
    /// one-shot loader plus a "world DB attached after launch" safety
    /// net.
    ///
    /// Why `tick` and not `render`: SPEC §Task 10d / `Screen::render`
    /// docs forbid blocking world queries on the draw path. `tick`
    /// runs once per frame *before* render and explicitly tolerates
    /// SQLite latency; `read_remaining_turns` performs an upsert plus
    /// an ensure-today-row write, both of which fall under that
    /// tolerance.
    fn tick(&mut self, ctx: &mut GameContext<'_>) -> ScreenCommand {
        // SPEC_v2 §Task 13c-ii: drain any `clue_found` events the
        // Lost-and-Found Drawer's prompt callback queued on the
        // previous frame. Done first so a successful drawer interaction
        // in frame N produces a bulletin row before the next render in
        // frame N+1, even if the turns-status cache is already warm.
        if let Some(world) = ctx.world_db {
            if !self.slots.pending_clue_events.borrow().is_empty() {
                let _ = crate::world::flush_pending_clue_events(world, ctx.foglet, &self.slots);
            }
        }
        if self.turns_status.borrow().is_some() {
            return ScreenCommand::None;
        }
        if let Some(world) = ctx.world_db {
            if let Some(status) = crate::world::read_remaining_turns(
                world,
                ctx.foglet,
                ctx.config,
                &*self.slots.date_provider,
            ) {
                *self.turns_status.borrow_mut() = Some(status);
            }
        }
        ScreenCommand::None
    }
}

/// Legend used to parse the lobby ASCII map.
///
/// Pulled out of [`MapScreen::with_shared`] so the integration tests
/// can build the same legend the screen uses without coupling to the
/// constructor's internals. Glyphs:
///
/// - `#` → wall (blocking)
/// - ` ` → floor (walkable)
/// - `+` → door (walkable)
/// - `L` → locked door (parsed as a `Custom` kind so the kit's tile
///   model stays minimal; [`MapScreen::try_move`] gates passage on the
///   brass key being in inventory, while render styles the cell red
///   until unlocked)
/// - `>` → stairs up to Room 7 (Custom; gated on the
///   [`MapScreen::ROOM_7_KEY_ID`] inventory item, then a step onto the
///   cell emits [`ScreenCommand::Replace`] with a fresh
///   [`crate::room_7::Room7Screen`])
fn lobby_legend() -> TileLegend {
    TileLegend::from_pairs([
        ("#", "wall"),
        (" ", "floor"),
        ("+", "door"),
        ("L", "locked_door"),
        (">", "stairs_up"),
    ])
    .expect("static lobby legend parses")
}

#[cfg(test)]
pub(crate) mod tests {
    //! Tests for the lobby map: spawn, movement, NPC/item catalogs,
    //! locked-door gating, win-tile latch, and the SPEC §9 hint /
    //! adjacency contracts the proof scenes hang off of.
    //!
    //! `pub(crate)` so the cross-cutting fixtures (`fresh_map_screen`,
    //! `walk_to`) are reachable from sibling test modules — the dialog
    //! and lost-and-found scene tests need them to drive an integration
    //! flow without re-deriving the helpers locally.

    use super::*;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::GameContext;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Build a [`MapScreen`] using the same spawn coordinates the
    /// scaffold's `assets/game.toml` ships with. Centralised so a
    /// future spawn retune updates one place instead of every test.
    pub(crate) fn fresh_map_screen() -> MapScreen {
        let cfg = fixture_config();
        MapScreen::new_lobby(cfg.game.start_x, cfg.game.start_y)
    }

    /// Dispatch one input to a fresh map screen and return the screen
    /// (so tests can assert position) plus the command emitted.
    fn dispatch_map(input: Input) -> (MapScreen, ScreenCommand) {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let cmd = map.handle_input(&mut ctx, input);
        (map, cmd)
    }

    /// Walk the player from spawn onto the cell at `(target_x,
    /// target_y)` using a simple axis-aligned route. Used by the
    /// pickup tests to land on an item without re-deriving the
    /// movement sequence each time. Returns the live screen so the
    /// caller can keep driving it.
    pub(crate) fn walk_to(map: &mut MapScreen, ctx: &mut GameContext<'_>, tx: u16, ty: u16) {
        // Doors live on y=3 — that's the only row connecting rooms.
        // Route there first, traverse horizontally, then settle on
        // the target row. A break-on-no-progress guard prevents the
        // helper from looping forever if a wall ever blocks the
        // route.
        let step = |map: &mut MapScreen, ctx: &mut GameContext<'_>, key: Input| -> bool {
            let before = map.player();
            map.handle_input(ctx, key);
            map.player() != before
        };
        while map.player().1 != 3 {
            let key = if map.player().1 < 3 {
                Input::Down
            } else {
                Input::Up
            };
            if !step(map, ctx, key) {
                break;
            }
        }
        while map.player().0 != tx {
            let key = if map.player().0 < tx {
                Input::Right
            } else {
                Input::Left
            };
            if !step(map, ctx, key) {
                break;
            }
        }
        while map.player().1 != ty {
            let key = if map.player().1 < ty {
                Input::Down
            } else {
                Input::Up
            };
            if !step(map, ctx, key) {
                break;
            }
        }
    }

    // ---- Spawn / movement ---------------------------------------------

    #[test]
    fn map_spawn_uses_config_coordinates() {
        let cfg = fixture_config();
        let map = fresh_map_screen();
        // `assets/game.toml` carries (22, 4) — the lobby's mid-room
        // floor cell. If this assertion fails the spawn config and
        // the test fixture have drifted; update both together.
        assert_eq!(map.player(), (cfg.game.start_x, cfg.game.start_y));
        assert!(
            map.map().is_walkable(map.player().0, map.player().1),
            "spawn cell must be walkable"
        );
    }

    #[test]
    fn map_lobby_has_five_rooms_in_a_row() {
        // Sanity-check the geometry the rest of the screen relies on.
        // Five rooms means six wall columns dividing them, including
        // the outer walls. Walking row 1 from left to right we expect
        // exactly six `#` cells.
        let map = fresh_map_screen();
        let row = &map.map().cells[1];
        let wall_cells = row
            .iter()
            .filter(|t| matches!(t.kind, foglet_game::TileKind::Wall))
            .count();
        assert_eq!(
            wall_cells,
            6,
            "expected six wall columns (5 rooms + 2 outer walls — minus 1 since outer walls double as room edges)"
        );
    }

    #[test]
    fn map_arrow_keys_move_player() {
        // Right then left should land on the original cell. The lobby
        // layout guarantees both steps are walkable from the spawn.
        let (mut map, cmd) = dispatch_map(Input::Right);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(map.player(), (23, 4));

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        map.handle_input(&mut ctx, Input::Left);
        assert_eq!(map.player(), (22, 4));
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(map.player(), (22, 3));
        map.handle_input(&mut ctx, Input::Down);
        assert_eq!(map.player(), (22, 4));
    }

    #[test]
    fn map_vi_keys_move_player() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        for key in [
            Input::Char('h'),
            Input::Char('j'),
            Input::Char('k'),
            Input::Char('l'),
        ] {
            map.handle_input(&mut ctx, key);
        }
        // Net displacement: -1 +1 -1 +1 in (x,y,y,x) → back to spawn
        // because every step succeeds (lobby's mid-room is fully open).
        assert_eq!(map.player(), (cfg.game.start_x, cfg.game.start_y));
    }

    #[test]
    fn map_walls_block_movement() {
        // Walk left repeatedly until we hit Room 1's left wall, then
        // try one more step. The position must clamp at the column
        // immediately right of the wall instead of advancing into it.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        for _ in 0..50 {
            map.handle_input(&mut ctx, Input::Left);
        }
        let (x, y) = map.player();
        // Spawn row (y=4) has no doors — only `#` at the column-9, 18,
        // 27, 36 walls — so leftward motion clamps at the lobby's
        // first interior column (x=19, immediately right of the wall
        // at x=18). The test exercises both bounds-clamping and
        // walkability invariants in one shot.
        assert_eq!(
            (x, y),
            (19, 4),
            "spawn-row left walks must clamp at the lobby's west wall"
        );
        assert!(map.map().is_walkable(x, y));
    }

    #[test]
    fn map_player_can_traverse_doors_into_each_room() {
        // Walk far enough left to enter Room 1 (cross doors at x=18
        // and x=9), assert the player is past both door columns.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Step up into the door row so doors are aligned with the
        // player's y. Spawn is (22, 4); doors are on y=3.
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(map.player(), (22, 3));
        for _ in 0..15 {
            map.handle_input(&mut ctx, Input::Left);
        }
        let (x, y) = map.player();
        assert_eq!(y, 3, "player must stay on the door row");
        assert!(
            x < 9,
            "expected to have entered Room 1 (left of the x=9 door); ended at x={x}"
        );
    }

    #[test]
    fn map_esc_pops_back_to_menu() {
        let (_, cmd) = dispatch_map(Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn map_quit_keys_quit() {
        for key in [Input::Char('q'), Input::Char('Q'), Input::Ctrl('c')] {
            let (_, cmd) = dispatch_map(key);
            assert!(
                matches!(cmd, ScreenCommand::Quit),
                "{key:?} should quit the map screen"
            );
        }
    }

    #[test]
    fn map_unwalkable_spawn_falls_back_to_floor() {
        // Spawning on a wall is a config bug, not a panic. The
        // constructor finds the nearest walkable cell instead of
        // entering an invariant-violating state.
        let map = MapScreen::new_lobby(0, 0); // outer corner wall
        let (x, y) = map.player();
        assert!(
            map.map().is_walkable(x, y),
            "expected spawn to be relocated to a floor cell, got ({x}, {y}) which is not walkable"
        );
    }

    #[test]
    fn map_renders_into_test_backend() {
        // Paint into a TestBackend and confirm the player glyph and
        // every room label show up in the buffer. The map block title
        // is also asserted so a future regression that strips the
        // border is caught here.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(
            found.contains(MapScreen::TITLE),
            "map block title missing; buffer was:\n{found}"
        );
        assert!(
            found.contains("@"),
            "expected player glyph in rendered map; buffer was:\n{found}"
        );
        for (label, _) in MapScreen::ROOM_LABELS {
            assert!(
                found.contains(label),
                "missing room label {label:?}; buffer was:\n{found}"
            );
        }
        assert!(
            found.contains("Move:"),
            "expected hint line below map; buffer was:\n{found}"
        );
    }

    #[test]
    fn map_rendered_rows_stamp_player() {
        // `rendered_rows` is the headless-renderable surface tests use
        // when they don't need a `TestBackend`. Confirm the player
        // glyph lands at the expected column on the spawn row.
        let map = fresh_map_screen();
        let rows = map.rendered_rows();
        let (px, py) = map.player();
        let stamped: Vec<char> = rows[py as usize].chars().collect();
        assert_eq!(
            stamped[px as usize],
            MapScreen::PLAYER_GLYPH,
            "expected `@` at player column; row was {:?}",
            rows[py as usize]
        );
    }

    // ---- NPCs ---------------------------------------------------------

    #[test]
    fn map_has_three_npcs_on_walkable_floors() {
        // SPEC §13 says exactly three NPCs. Each must stand on a
        // walkable floor cell — placing one on a wall would silently
        // erase that wall in the renderer and confuse navigation.
        let map = fresh_map_screen();
        assert_eq!(MapScreen::NPCS.len(), 3, "expected exactly three NPCs");
        for npc in MapScreen::NPCS.iter() {
            assert!(
                map.map().is_walkable(npc.x, npc.y),
                "NPC {:?} stands on an unwalkable cell ({}, {})",
                npc.name,
                npc.x,
                npc.y
            );
        }
    }

    #[test]
    fn map_npc_glyphs_render_into_buffer() {
        // Paint the lobby and confirm every NPC's glyph appears. Locks
        // in the contract that the NPC overlay actually fires from
        // `render`.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        for npc in MapScreen::NPCS.iter() {
            assert!(
                found.contains(npc.glyph),
                "missing NPC glyph {:?} for {:?}; buffer was:\n{}",
                npc.glyph,
                npc.name,
                found
            );
        }
    }

    #[test]
    fn map_npcs_block_player_movement() {
        // Stepping into an NPC must be a no-op. Drive the player up
        // to a cell adjacent to the Night Clerk and confirm the next
        // Up does not enter the clerk's cell.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Spawn (22, 4) → Up to (22, 3) → Right twice to (24, 3).
        // Night Clerk sits on (24, 2).
        map.handle_input(&mut ctx, Input::Up);
        map.handle_input(&mut ctx, Input::Right);
        map.handle_input(&mut ctx, Input::Right);
        assert_eq!(map.player(), (24, 3));
        // Up would step onto the clerk; movement must be rejected.
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(
            map.player(),
            (24, 3),
            "player must not be able to walk onto an NPC's cell"
        );
    }

    #[test]
    fn map_enter_with_no_npc_nearby_is_inert() {
        // From spawn the nearest NPC (Night Clerk at (24, 2)) is two
        // cells diagonal away. Enter should not push a dialog.
        let (_, cmd) = dispatch_map(Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::None),
            "Enter with no nearby NPC must be inert, got {cmd:?}"
        );
    }

    #[test]
    fn map_enter_adjacent_to_npc_pushes_dialog() {
        // Walk to the cell directly south of the Night Clerk and
        // press Enter; the screen must push a DialogScreen.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        map.handle_input(&mut ctx, Input::Up); // (22, 3)
        map.handle_input(&mut ctx, Input::Right); // (23, 3)
        map.handle_input(&mut ctx, Input::Right); // (24, 3)
        let cmd = map.handle_input(&mut ctx, Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "Enter adjacent to Night Clerk must push a dialog, got {cmd:?}"
        );
    }

    // ---- Items + i-key wiring -----------------------------------------

    #[test]
    fn map_has_five_items_in_distinct_rooms() {
        // SPEC §13 calls for exactly five collectables. We further
        // enforce one per room — the rooms are 8 cells wide, so
        // bucketing items by `x / 9` gives 0,1,2,3,4 with no
        // duplicates if the catalog is correctly distributed.
        assert_eq!(MapScreen::ITEMS.len(), 5, "expected exactly five items");
        let mut buckets: Vec<u16> = MapScreen::ITEMS.iter().map(|i| i.x / 9).collect();
        buckets.sort();
        assert_eq!(
            buckets,
            vec![0, 1, 2, 3, 4],
            "expected one item per room (x/9 bucket)"
        );
    }

    #[test]
    fn map_items_sit_on_walkable_non_npc_cells() {
        // Catalog invariant: every item is on a floor cell with no
        // NPC standing on it. Drift (an item placed on a wall, or on
        // top of the Night Clerk) silently breaks render and pickup.
        let map = fresh_map_screen();
        for item in MapScreen::ITEMS.iter() {
            assert!(
                map.map().is_walkable(item.x, item.y),
                "item {:?} at ({}, {}) sits on an unwalkable cell",
                item.name,
                item.x,
                item.y
            );
            assert!(
                MapScreen::npc_at(item.x, item.y).is_none(),
                "item {:?} at ({}, {}) overlaps an NPC",
                item.name,
                item.x,
                item.y
            );
        }
    }

    #[test]
    fn map_item_glyphs_render_into_buffer_until_collected() {
        // Each catalog glyph appears on a fresh map; once an item is
        // marked collected, its glyph drops from the next render.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        for item in MapScreen::ITEMS.iter() {
            assert!(
                found.contains(item.glyph),
                "missing item glyph {:?} for {:?}; buffer was:\n{}",
                item.glyph,
                item.name,
                found
            );
        }
        // Mark the matchbook collected and re-render; the `m` glyph
        // should disappear from the painted map.
        map.inventory().borrow_mut().insert("matchbook".to_string());
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("redraw");
        let buf = term.backend().buffer().clone();
        // Inspect the matchbook's exact cell rather than the whole
        // buffer; the lowercase `m` could plausibly recur elsewhere
        // (it doesn't today, but a future label change shouldn't
        // weaken the assertion).
        let item = MapScreen::ITEMS
            .iter()
            .find(|i| i.id == "matchbook")
            .expect("matchbook in catalog");
        let cell = buf.cell((item.x, item.y)).expect("cell").symbol();
        assert_ne!(
            cell, "m",
            "collected item must not paint its glyph; cell was {cell:?}"
        );
    }

    #[test]
    fn map_walking_onto_item_collects_it() {
        // Drive the player onto the brass-key cell (6, 5) (Room 1,
        // since Task 13f swapped the key into Room 1 so it is
        // collectable without first traversing the locked door) and
        // confirm the inventory grows by exactly that ID.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        assert_eq!(map.player(), (6, 5), "should land on brass-key cell");
        assert!(
            map.is_collected("brass_key"),
            "stepping onto an item must add it to the inventory"
        );
    }

    #[test]
    fn map_i_key_pushes_inventory_screen() {
        let (_, cmd) = dispatch_map(Input::Char('i'));
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "`i` must push the inventory screen, got {cmd:?}"
        );
    }

    #[test]
    fn map_pickup_emits_feedback_line() {
        // Without a feedback line on map-item pickup the player has no
        // signal that the brass key landed in their pocket — the glyph
        // disappears under the player's `@` and silence follows. Pin
        // the feedback contract so a future try_move refactor doesn't
        // silently drop the cue.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Brass key sits at (6, 5) — the only collectable in Room 1.
        walk_to(&mut map, &mut ctx, 6, 5);
        let feedback = map
            .slots()
            .feedback
            .borrow()
            .clone()
            .expect("pickup must populate the feedback slot");
        assert_eq!(
            feedback.rendered_text(),
            "+ Picked up Brass key.",
            "feedback line must name the item just collected (FeedbackLine::success renders with a `+ ` marker)"
        );
    }

    #[test]
    fn map_pickup_does_not_re_emit_feedback_on_revisit() {
        // Feedback fires only when the inventory actually grows. A
        // player who walks onto the cell, leaves, and walks back must
        // not re-narrate the pickup — the cell is empty and the line
        // would be a lie.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        // Stomp the feedback slot to a sentinel so we can detect
        // whether the second pass overwrites it.
        *map.slots().feedback.borrow_mut() = Some(foglet_game::FeedbackLine::info("sentinel"));
        // Step off then back onto the now-empty cell.
        map.handle_input(&mut ctx, Input::Up);
        map.handle_input(&mut ctx, Input::Down);
        let feedback = map
            .slots()
            .feedback
            .borrow()
            .clone()
            .expect("sentinel should still be present");
        assert_eq!(
            feedback.rendered_text(),
            "sentinel",
            "revisiting an emptied cell must not overwrite feedback"
        );
    }

    #[test]
    fn end_to_end_brass_key_pickup_unlocks_door_and_renders_in_inventory() {
        // Reproduction for the user-reported bug "I picked up the
        // brass key but the door didn't unlock and the inventory
        // screen was empty." Drives the full pickup → inventory
        // round-trip through the same input pump real gameplay uses,
        // so any regression that bypasses `walk_to`'s direct insertion
        // would surface here.
        use crate::modals::InventoryScreen;
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();

        // Walk to the brass-key cell at (6, 5) using only the public
        // input dispatcher. `walk_to` routes through `handle_input`,
        // matching the runtime path.
        walk_to(&mut map, &mut ctx, 6, 5);
        assert_eq!(map.player(), (6, 5), "player should reach the key cell");
        assert!(
            map.is_collected(MapScreen::LOCKED_DOOR_KEY_ID),
            "stepping onto the brass-key cell must add it to inventory"
        );
        assert!(
            map.has_locked_door_key(),
            "has_locked_door_key() must reflect the inventory state"
        );

        // Open the inventory screen with the same Rc handle the lobby
        // uses and confirm the label list includes the brass key.
        let inv_screen = InventoryScreen::new(map.inventory());
        let labels = inv_screen.current_labels();
        assert!(
            labels.iter().any(|l| l == "Brass key"),
            "Brass key must appear in the inventory list; got {labels:?}"
        );

        // Walk back east through the locked door and confirm the gate
        // dropped — the player must reach a cell east of x=36.
        walk_to(&mut map, &mut ctx, 39, 5);
        assert_eq!(
            map.player(),
            (39, 5),
            "with brass key in hand the locked door must let the player through"
        );
    }

    // ---- Locked door --------------------------------------------------

    #[test]
    fn locked_door_key_id_matches_catalog() {
        // Renaming the brass-key item or the locked-door key constant
        // would silently un-lock the door — assert they stay in sync.
        assert!(MapScreen::ITEMS
            .iter()
            .any(|i| i.id == MapScreen::LOCKED_DOOR_KEY_ID));
    }

    #[test]
    fn locked_door_blocks_player_without_key() {
        // Walk to (36, 3) — the cell directly south of nothing but
        // the locked door's own column — and try to step into it.
        // Without the brass key in the inventory the step must be
        // rejected and the player must remain on (35, 3).
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Spawn (22, 4) → Up to door row, then walk east toward Room
        // 4. The door at x=36 should stop the player at x=35.
        map.handle_input(&mut ctx, Input::Up);
        for _ in 0..30 {
            map.handle_input(&mut ctx, Input::Right);
        }
        let (x, y) = map.player();
        assert_eq!(
            (x, y),
            (35, 3),
            "locked door must clamp the eastward walk at x=35 (one cell west of the lock)"
        );
        assert!(
            !map.has_locked_door_key(),
            "test precondition: key must not yet be in inventory"
        );
    }

    #[test]
    fn locked_door_passes_player_with_key() {
        // Pick up the brass key from Room 1 first, then walk back
        // east across the locked door. The player must end up east
        // of x=36 (Room 4).
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        assert!(
            map.has_locked_door_key(),
            "expected brass key to be collected at (6, 5)"
        );
        // Now route to Room 4 via the door row.
        walk_to(&mut map, &mut ctx, 39, 5);
        assert_eq!(
            map.player(),
            (39, 5),
            "player should reach the matchbook cell once the door is unlocked"
        );
        assert!(
            map.is_collected("matchbook"),
            "matchbook in Room 4 should be picked up after passing the unlocked door"
        );
    }

    #[test]
    fn locked_door_renders_with_locked_glyph_until_unlocked() {
        // Paint the lobby with no items collected and confirm the
        // locked-door glyph (`L`) appears at its cell. Then mark the
        // brass key as collected and re-render; the cell must paint
        // the open-door glyph (`+`) instead.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let (lx, ly) = MapScreen::LOCKED_DOOR_POS;
        // The map block adds a one-cell border, and `centred_rect`
        // offsets the map inside the frame — `rendered_rows` keeps the
        // contract simpler, so use it for a direct cell check.
        let rows = map.rendered_rows();
        let locked_cell = rows[ly as usize].chars().nth(lx as usize).unwrap();
        assert_eq!(
            locked_cell,
            MapScreen::LOCKED_DOOR_GLYPH,
            "locked door must paint as `L` while still locked"
        );
        // The styled render path (TestBackend) must also include `L`
        // somewhere visible.
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(
            found.contains('L'),
            "expected locked-door glyph in render; buffer was:\n{found}"
        );

        // Unlock and re-check.
        map.inventory().borrow_mut().insert("brass_key".into());
        let rows = map.rendered_rows();
        let cell = rows[ly as usize].chars().nth(lx as usize).unwrap();
        assert_eq!(
            cell,
            MapScreen::UNLOCKED_DOOR_GLYPH,
            "unlocked door must paint as `+`"
        );
    }

    // ---- Stairs / Room 7 transition ----------------------------------

    #[test]
    fn lobby_with_shared_writes_lobby_into_map_name_slot() {
        // Symmetric with the Room 7 constructor: every map screen
        // tags the slots with its identifier so a quit-then-Continue
        // round-trip lands the player back on the right map.
        let slots = SharedSlots::default();
        let _screen = MapScreen::with_shared(22, 4, slots.clone());
        assert_eq!(slots.map_name.borrow().as_str(), MapScreen::MAP_NAME);
    }

    #[test]
    fn stairs_block_player_without_room_7_key() {
        // The stairs-up cell is gated by the Room 7 key, just like the
        // locked door is gated by the brass key. Walking eastward
        // across Room 4 toward (44, 1) must clamp the player at (43, 1)
        // — one cell west of the stairs — when the inventory is empty.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Brass key first so the locked door doesn't pre-empt the
        // stairs gate; the test is about the *stairs* gate.
        walk_to(&mut map, &mut ctx, 6, 5);
        assert!(map.has_locked_door_key());
        // Now route to (43, 1) and try to step further east.
        walk_to(&mut map, &mut ctx, 43, 1);
        assert_eq!(map.player(), (43, 1));
        assert!(
            !map.has_room_7_key(),
            "test precondition: room 7 key must not yet be in inventory"
        );
        map.handle_input(&mut ctx, Input::Right);
        assert_eq!(
            map.player(),
            (43, 1),
            "stairs gate must clamp the eastward step at x=43"
        );
    }

    #[test]
    fn stairs_emit_replace_when_player_has_room_7_key() {
        // With the Room 7 key in inventory, stepping onto the stairs
        // cell must emit ScreenCommand::Replace so the runtime swaps
        // the lobby for Room 7. The post-move handler also re-points
        // the slots' player position at Room 7's ARRIVAL_POS *before*
        // returning Replace, so by the time the caller reads
        // `map.player()` the slots already reflect the destination's
        // arrival cell — that's the contract the destination's
        // `with_shared` constructor relies on to honour
        // saved-coords-as-spawn correctly.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        // Grant the Room 7 key directly to keep the test focused on
        // the transition rather than driving the drawer prompt that
        // normally sets it.
        map.inventory()
            .borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        walk_to(&mut map, &mut ctx, 43, 1);
        assert_eq!(map.player(), (43, 1));
        let cmd = map.handle_input(&mut ctx, Input::Right);
        assert!(
            matches!(cmd, ScreenCommand::Replace(_)),
            "stairs step with Room 7 key must emit Replace; got {cmd:?}"
        );
        assert_eq!(
            map.player(),
            crate::room_7::Room7Screen::ARRIVAL_POS,
            "post-transition slots must point at Room 7's arrival cell"
        );
    }

    /// SPEC_v2 §Task 12b — stepping onto the stairs with the Room 7
    /// key in hand AND a `world_db` attached to the GameContext must
    /// stamp `motel_world_state` with the opener's `players.id` and
    /// the SQLite `CURRENT_TIMESTAMP`. Exercises the same path
    /// `run_with_io` drives at runtime, but headless: build the map
    /// screen, walk to the stairs, dispatch the final step with
    /// `ctx.with_world_db(&world)` attached, then read the canonical
    /// pair back via [`crate::world::room_7_opening`] and assert.
    #[test]
    fn stairs_step_records_room_7_opening_in_world_db() {
        use crate::world::{record_room_7_opening, room_7_opening, MOTEL_WORLD_STATE_MIGRATION};
        use foglet_game::WorldDb;
        use tempfile::tempdir;

        // Stand up a real on-disk world DB with the kit's players
        // migration plus the motel migration. Going through the
        // `WorldDb::open + apply_migration` path (rather than poking
        // tables directly) is what the runtime does, so the test
        // captures the production schema state at the moment the
        // first opening lands.
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        // Sanity: no opener recorded before the stairs step.
        assert!(
            room_7_opening(&world).expect("read").is_none(),
            "precondition: no opener row before the player steps onto stairs"
        );

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut map = fresh_map_screen();
        // Drive to the stairs the same way `stairs_emit_replace_when_player_has_room_7_key`
        // does, but every input dispatched through this `ctx` carries
        // the world DB so the post-move handler can stamp the record.
        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to(&mut map, &mut ctx, 6, 5);
            map.inventory()
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            walk_to(&mut map, &mut ctx, 43, 1);
            assert_eq!(map.player(), (43, 1));
            let cmd = map.handle_input(&mut ctx, Input::Right);
            assert!(
                matches!(cmd, ScreenCommand::Replace(_)),
                "stairs step must still emit Replace when world_db is attached"
            );
        }

        // Read back through the same helper Tasks 12c/13d will use.
        let opening = room_7_opening(&world)
            .expect("read succeeds")
            .expect("first opening must be recorded after the stairs step");
        assert!(
            !opening.opened_at.is_empty(),
            "opened_at must be populated by CURRENT_TIMESTAMP"
        );
        assert!(
            opening.opened_by_player_id > 0,
            "opener id must be a positive player row id, got {}",
            opening.opened_by_player_id
        );

        // Re-running the recorder with a different player must NOT
        // overwrite the opener: this is the cross-player invariant
        // Task 12c will hang the "someone got here first" surface
        // off of, so pinning it at the integration level here
        // catches regressions in the screen path that the helper-
        // level `record_room_7_opening_is_first_writer_wins` test
        // alone would miss.
        let later = record_room_7_opening(&world, opening.opened_by_player_id + 999)
            .expect("later call succeeds");
        assert!(
            !later.first_opening,
            "subsequent call must report the row already existed"
        );
        assert_eq!(
            later.opened_by_player_id, opening.opened_by_player_id,
            "later call must observe the original opener id"
        );
    }

    /// SPEC_v2 §Task 12c — when another player has already opened
    /// Room 7, the *current* player's stairs step must populate
    /// `SharedSlots::feedback` with the "another investigator already
    /// unlocked Room 7" banner. Strategy: pre-seed the world DB with
    /// an opening row for player id 999 (a synthetic opener that is
    /// *not* the current local-dev player), drive the stairs step, and
    /// inspect the slots.
    #[test]
    fn stairs_step_sets_arrival_feedback_when_someone_else_opened() {
        use crate::world::{record_room_7_opening, MOTEL_WORLD_STATE_MIGRATION};
        use foglet_game::WorldDb;
        use tempfile::tempdir;

        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        // Synthetic prior opener: a player id we know cannot collide
        // with the local-dev fixture player (which gets a fresh
        // autoincrement id starting at 1) by jumping the id well past
        // any single test run could generate.
        record_room_7_opening(&world, 999).expect("seed prior opening");

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut map = fresh_map_screen();
        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to(&mut map, &mut ctx, 6, 5);
            map.inventory()
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            walk_to(&mut map, &mut ctx, 43, 1);
            let cmd = map.handle_input(&mut ctx, Input::Right);
            assert!(
                matches!(cmd, ScreenCommand::Replace(_)),
                "stairs step must still emit Replace; got {cmd:?}"
            );
        }

        // Read the feedback the post-move handler stamped into the
        // shared slots. The Room 7 screen will paint this on its first
        // frame because it borrows the same `Rc<RefCell<...>>`.
        let line = map
            .slots()
            .feedback
            .borrow()
            .clone()
            .expect("Task 12c: arrival feedback must be set when someone else opened first");
        let text = line.rendered_text();
        assert!(
            text.contains("Another investigator"),
            "feedback must call out the prior opening: {text}"
        );
    }

    /// SPEC_v2 §Task 12c — when the *current* player is the first
    /// opener, the arrival slot must be cleared (no "someone else got
    /// here" banner) so Room 7's body line owns the feedback row on
    /// the first frame after arrival.
    #[test]
    fn stairs_step_clears_arrival_feedback_for_first_opener() {
        use crate::world::MOTEL_WORLD_STATE_MIGRATION;
        use foglet_game::WorldDb;
        use tempfile::tempdir;

        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut map = fresh_map_screen();
        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to(&mut map, &mut ctx, 6, 5);
            map.inventory()
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            walk_to(&mut map, &mut ctx, 43, 1);
            let _ = map.handle_input(&mut ctx, Input::Right);
        }

        assert!(
            map.slots().feedback.borrow().is_none(),
            "first opener must arrive in Room 7 with a clean feedback slot"
        );
    }

    /// SPEC_v2 §Task 12d — two-player end-to-end proof of the shared
    /// Room 7 record. Two distinct Foglet contexts (Alice and Bob),
    /// distinguished by `user_id`, walk fresh `MapScreen` instances to
    /// the stairs while sharing one on-disk world DB. Alice goes first
    /// and lands in Room 7 with no arrival banner; Bob follows with the
    /// same key and must (a) observe the arrival banner naming a prior
    /// opening, and (b) leave the canonical `motel_world_state` row
    /// pointing at Alice's `players.id` rather than his own.
    ///
    /// This is the integration counterpart to the helper-level
    /// `record_room_7_opening_is_first_writer_wins` test: it drives the
    /// production `handle_input` path twice with two real upserted
    /// players, so any future regression that swaps the post-move
    /// recorder for a "always overwrite" or "current player wins"
    /// implementation surfaces here.
    #[test]
    fn two_players_share_room_7_evidence() {
        use crate::world::{room_7_opening, MOTEL_WORLD_STATE_MIGRATION};
        use foglet_game::{ContextSource, FogletContext, WorldDb};
        use tempfile::tempdir;

        // One on-disk world DB shared across both players, exactly the
        // way two Foglet sessions hitting the same install would see it.
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");

        // Build two FogletContexts that differ only in identity. Going
        // through the `Some(user_id)` branch of `upsert_player` (rather
        // than the local-dev fallback) makes the test independent of
        // the `synthesize_local_dev_key` hash — it directly mirrors a
        // real Foglet handoff where each user has a stable id.
        let make_ctx = |user_id: &str, username: &str| FogletContext {
            door_id: "murder-motel".into(),
            user_id: Some(user_id.into()),
            username: Some(username.into()),
            role: None,
            session_id: Some("s-test".into()),
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::LocalDev,
        };
        let alice_ctx = make_ctx("u-alice", "alice");
        let bob_ctx = make_ctx("u-bob", "bob");

        let cfg = fixture_config();

        // --- Alice: first opener -----------------------------------
        let alice_player_id;
        let alice_opened_at;
        {
            let mut alice_map = fresh_map_screen();
            let mut ctx = GameContext::new(&cfg, &alice_ctx, (80, 24)).with_world_db(&world);
            walk_to(&mut alice_map, &mut ctx, 6, 5);
            alice_map
                .inventory()
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            walk_to(&mut alice_map, &mut ctx, 43, 1);
            let cmd = alice_map.handle_input(&mut ctx, Input::Right);
            assert!(
                matches!(cmd, ScreenCommand::Replace(_)),
                "alice's stairs step must transition into Room 7; got {cmd:?}"
            );
            // Alice is the first opener — her arrival slot is cleared
            // so Room 7's body line owns the feedback row.
            assert!(
                alice_map.slots().feedback.borrow().is_none(),
                "first opener (alice) must arrive with a clean feedback slot"
            );
            // Capture the recorded opener id so the post-Bob assertion
            // can prove the row still belongs to her. We read through
            // the same helper the production lobby UI will use.
            let opening = room_7_opening(&world)
                .expect("read after alice's step")
                .expect("alice's stairs step must have recorded an opening");
            alice_player_id = opening.opened_by_player_id;
            alice_opened_at = opening.opened_at;
        }

        // --- Bob: later opener -------------------------------------
        // Fresh `MapScreen`, fresh inventory, fresh `GameContext` —
        // exactly what a second Foglet session would build. The world
        // DB handle is the only thing shared.
        let mut bob_map = fresh_map_screen();
        {
            let mut ctx = GameContext::new(&cfg, &bob_ctx, (80, 24)).with_world_db(&world);
            walk_to(&mut bob_map, &mut ctx, 6, 5);
            bob_map
                .inventory()
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            walk_to(&mut bob_map, &mut ctx, 43, 1);
            let cmd = bob_map.handle_input(&mut ctx, Input::Right);
            assert!(
                matches!(cmd, ScreenCommand::Replace(_)),
                "bob's stairs step must still transition into Room 7; got {cmd:?}"
            );
        }

        // Bob is a later opener — his arrival slot must carry the
        // Task 12c "another investigator" banner with Alice's
        // timestamp. The handle is intentionally not surfaced (the
        // kit has no id→handle lookup; identities are advisory).
        let banner = bob_map
            .slots()
            .feedback
            .borrow()
            .clone()
            .expect("bob (later opener) must see the shared-evidence banner");
        let text = banner.rendered_text();
        assert!(
            text.contains("Another investigator"),
            "banner must call out the prior opening: {text}"
        );
        assert!(
            text.contains(&alice_opened_at),
            "banner must echo alice's opening timestamp ({alice_opened_at}): {text}"
        );

        // The canonical record must still point at Alice. This is the
        // shared-world invariant: "who opened Room 7" is one answer
        // across all players, no matter who walks in afterwards.
        let final_opening = room_7_opening(&world)
            .expect("read after bob's step")
            .expect("opening row must persist after bob's step");
        assert_eq!(
            final_opening.opened_by_player_id, alice_player_id,
            "shared record must still credit alice ({alice_player_id}), not bob"
        );
        assert_eq!(
            final_opening.opened_at, alice_opened_at,
            "shared record must keep alice's original timestamp"
        );

        // Sanity: alice and bob did upsert as distinct players. If a
        // future regression made the upsert collide on, say, `handle`
        // alone, the "alice still owns the row" check above would
        // succeed vacuously (because both contexts would map to the
        // same id). Pin the distinctness explicitly here.
        let bob_player = world.upsert_player(&bob_ctx).expect("bob upsert succeeds");
        assert_ne!(
            bob_player.id, alice_player_id,
            "alice and bob must resolve to distinct players.id rows"
        );
    }

    #[test]
    fn stairs_render_paints_glyph_at_stairs_pos() {
        // Pin the rendered glyph so a future copy edit (or a glyph
        // collision with another tile) surfaces as a failed test.
        let map = fresh_map_screen();
        let rows = map.rendered_rows();
        let (sx, sy) = MapScreen::STAIRS_UP_POS;
        let cell = rows[sy as usize]
            .chars()
            .nth(sx as usize)
            .expect("stairs cell renders");
        assert_eq!(
            cell,
            MapScreen::STAIRS_UP_GLYPH,
            "stairs cell must paint the documented glyph"
        );
    }

    // ---- Win condition ------------------------------------------------

    #[test]
    fn win_tile_sits_on_walkable_floor_inside_room_four() {
        // The win cell must be reachable. We assert: walkable, no NPC
        // overlap, no item overlap, and column inside Room 4 (x/9 == 4).
        let map = fresh_map_screen();
        let (wx, wy) = MapScreen::WIN_TILE_POS;
        assert!(
            map.map().is_walkable(wx, wy),
            "win tile must be a walkable cell"
        );
        assert!(
            MapScreen::npc_at(wx, wy).is_none(),
            "win tile must not overlap an NPC"
        );
        assert!(
            MapScreen::item_at(wx, wy).is_none(),
            "win tile must not overlap a collectable item"
        );
        assert_eq!(wx / 9, 4, "win tile must live in Room 4 (x/9 bucket)");
    }

    #[test]
    fn win_step_with_flag_pushes_win_screen() {
        // Walk to a cell adjacent to the win tile (with the brass key
        // in inventory so the door is open), set the rumor flag, then
        // step onto the win tile. The handle_input call must emit a
        // Push and `has_won` must latch.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        // Pick up the brass key first so the locked door opens.
        walk_to(&mut map, &mut ctx, 6, 5);
        assert!(map.has_locked_door_key());
        // Set the flag the dialog would set.
        map.flags()
            .borrow_mut()
            .insert(MapScreen::WIN_FLAG.to_string());
        // Walk to (42, 4) — the cell directly west of the win tile.
        // Approaching from the west avoids crossing the win tile mid-
        // route: `walk_to` traverses doors on y=3, then descends to
        // (42, 4), so the player never steps on (43, 4) until the
        // explicit final move below.
        walk_to(&mut map, &mut ctx, 42, 4);
        assert_eq!(map.player(), (42, 4), "should land west of the win tile");
        assert!(!map.has_won(), "win latch must still be open");
        // Final step onto (43, 4).
        let cmd = map.handle_input(&mut ctx, Input::Right);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "step onto win tile with flag set must push WinScreen, got {cmd:?}"
        );
        assert!(map.has_won(), "win latch must close after firing");
        assert_eq!(map.player(), MapScreen::WIN_TILE_POS);
    }

    #[test]
    fn win_step_without_flag_is_inert() {
        // Same path as above but with no flag set. The step must
        // succeed (the cell is walkable) but emit None.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        walk_to(&mut map, &mut ctx, 42, 4);
        assert!(
            !map.flags().borrow().contains(MapScreen::WIN_FLAG),
            "test precondition: rumor flag must not be set"
        );
        let cmd = map.handle_input(&mut ctx, Input::Right);
        assert!(
            matches!(cmd, ScreenCommand::None),
            "step onto win tile without flag must be inert, got {cmd:?}"
        );
        assert!(!map.has_won());
        assert_eq!(map.player(), MapScreen::WIN_TILE_POS);
    }

    #[test]
    fn win_latch_is_one_shot() {
        // Once the latch fires, walking off and back onto the tile
        // must not push WinScreen a second time. Without the latch a
        // post-modal exit path could produce a stack of WinScreens.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        walk_to(&mut map, &mut ctx, 6, 5);
        map.flags()
            .borrow_mut()
            .insert(MapScreen::WIN_FLAG.to_string());
        walk_to(&mut map, &mut ctx, 42, 4);
        // Trigger the modal once.
        let _ = map.handle_input(&mut ctx, Input::Right);
        assert!(map.has_won());
        // Step off (west back to (42, 4)) then back on (east to
        // (43, 4)). The second step must NOT push another WinScreen.
        let cmd_off = map.handle_input(&mut ctx, Input::Left);
        assert!(matches!(cmd_off, ScreenCommand::None));
        let cmd_back = map.handle_input(&mut ctx, Input::Right);
        assert!(
            matches!(cmd_back, ScreenCommand::None),
            "win modal must not re-fire after the latch closes; got {cmd_back:?}"
        );
    }

    #[test]
    fn win_tile_renders_until_won() {
        // The `*` glyph must paint into both the headless rendered_rows
        // surface and the styled TestBackend buffer while `has_won` is
        // false. After winning the cell falls back to a plain floor.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut map = fresh_map_screen();
        let (wx, wy) = MapScreen::WIN_TILE_POS;
        let rows = map.rendered_rows();
        let cell = rows[wy as usize].chars().nth(wx as usize).unwrap();
        assert_eq!(
            cell,
            MapScreen::WIN_TILE_GLYPH,
            "win tile must paint as `*` while the case is open"
        );
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| map.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(
            found.contains('*'),
            "expected win-tile glyph in styled render; buffer was:\n{found}"
        );

        // Latch the win and re-render — the `*` must be gone from the
        // win tile (we check that exact cell to avoid false positives
        // from any future glyph reuse).
        map.slots().player.borrow_mut().won = true;
        let rows = map.rendered_rows();
        let cell = rows[wy as usize].chars().nth(wx as usize).unwrap();
        assert_ne!(
            cell,
            MapScreen::WIN_TILE_GLYPH,
            "post-win render must drop the `*` from the win tile"
        );
    }

    // ---- Loaded-save / save-bridge surface ----------------------------

    #[test]
    fn map_screen_with_shared_honours_loaded_player_position() {
        // `with_shared` must honour the player position the slots
        // already carry (the loaded-save case) instead of resetting to
        // the configured spawn. Without this, "Continue" would teleport
        // the player back to the lobby door every launch.
        let slots = SharedSlots::default();
        {
            let mut p = slots.player.borrow_mut();
            p.x = 39;
            p.y = 5;
            p.won = false;
        }
        let map = MapScreen::with_shared(22, 4, slots);
        assert_eq!(map.player(), (39, 5));
    }

    // ---- Drawer / clerk geometry the proof scenes hang off of ---------

    #[test]
    fn drawer_position_is_a_lobby_floor_cell_next_to_the_clerk() {
        // The prompt's "behind the desk" framing is meaningful only if
        // the drawer actually sits beside the Night Clerk. Pin both:
        // the cell is walkable in the underlying map (so the renderer
        // does not silently erase a wall) and the clerk is one step
        // away, matching SPEC §9 step 1's narrative geometry.
        let map = fresh_map_screen();
        let (dx, dy) = MapScreen::LOST_AND_FOUND_POS;
        assert!(
            map.map().is_walkable(dx, dy),
            "drawer cell ({dx}, {dy}) must be a walkable floor in the lobby ASCII"
        );
        let clerk = MapScreen::NPCS
            .iter()
            .find(|n| n.name == "Night Clerk")
            .expect("Night Clerk must exist on the lobby roster");
        let manhattan = (clerk.x as i32 - dx as i32).abs() + (clerk.y as i32 - dy as i32).abs();
        assert_eq!(
            manhattan, 1,
            "drawer must be one orthogonal step from the clerk"
        );
    }

    #[test]
    fn drawer_glyph_appears_in_rendered_rows() {
        // The headless render surface is what BBS clients ultimately see.
        // Stamping `D` proves the affordance is visible in normal play
        // — not just reachable through a hidden hotkey.
        let map = fresh_map_screen();
        let rows = map.rendered_rows();
        let (dx, dy) = MapScreen::LOST_AND_FOUND_POS;
        let stamped: Vec<char> = rows[dy as usize].chars().collect();
        assert_eq!(
            stamped[dx as usize],
            MapScreen::LOST_AND_FOUND_GLYPH,
            "expected `D` at drawer column; row was {:?}",
            rows[dy as usize]
        );
    }

    #[test]
    fn drawer_cell_blocks_player_movement() {
        // The drawer is furniture — the player must stand *beside* it,
        // not on it. Otherwise the SPEC §9 prompt's framing breaks down
        // and a wandering player could end up perched on the desk.
        // Approach from below: spawn (22, 4) → step right three times
        // to (25, 4) → step up to (25, 3), the floor cell directly
        // below the drawer at (25, 2).
        let mut map = fresh_map_screen();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for _ in 0..3 {
            map.handle_input(&mut ctx, Input::Right);
        }
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(
            map.player(),
            (25, 3),
            "expected approach-from-below path to land at (25, 3)"
        );
        assert!(
            map.nearby_lost_and_found(),
            "(25, 3) is the orthogonal neighbour directly below the drawer"
        );

        // Stepping up would land on the drawer cell — must be rejected.
        let cmd = map.handle_input(&mut ctx, Input::Up);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(
            map.player(),
            (25, 3),
            "drawer cell must reject the player's step"
        );
    }

    #[test]
    fn search_key_is_inert_when_not_adjacent_to_drawer() {
        // From the spawn (22, 4) the drawer is well out of reach.
        // Pressing `x` must be a no-op — silent, like bumping a wall —
        // so the affordance does not leak into rooms where the prompt
        // would be narratively wrong.
        let (mut map, cmd) = dispatch_map(Input::Char('x'));
        assert!(
            matches!(cmd, ScreenCommand::None),
            "search key without nearby drawer must produce no command"
        );
        // And again with uppercase so a Caps-Lock player isn't punished
        // by triggering the prompt from anywhere on the map.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = map.handle_input(&mut ctx, Input::Char('X'));
        assert!(matches!(cmd, ScreenCommand::None));
    }

    #[test]
    fn search_key_pushes_prompt_when_adjacent_to_drawer() {
        // Approach the drawer from the right (the only adjacency that
        // does not cross the Night Clerk's blocking cell) and verify
        // both `x` and `X` open the prompt.
        for key in [Input::Char('x'), Input::Char('X')] {
            let mut map = fresh_map_screen();
            let cfg = fixture_config();
            let fc = fixture_context();
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
            // Spawn (22, 4) → right thrice → up once = (25, 3),
            // the cell directly below the drawer at (25, 2).
            for _ in 0..3 {
                map.handle_input(&mut ctx, Input::Right);
            }
            map.handle_input(&mut ctx, Input::Up);
            assert_eq!(map.player(), (25, 3));
            assert!(map.nearby_lost_and_found());
            let cmd = map.handle_input(&mut ctx, key);
            assert!(
                matches!(cmd, ScreenCommand::Push(_)),
                "{key:?} adjacent to drawer must push the prompt screen"
            );
        }
    }

    #[test]
    fn map_hint_advertises_search_affordance_only_when_adjacent() {
        // The hint line is the only place a player learns the search
        // affordance exists. The contextual rule: it must hide when the
        // player is not next to the drawer (so a stray `X` press is
        // never advertised) and reveal itself the moment the player
        // steps adjacent.
        let mut map = fresh_map_screen();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        // Spawn (22, 4): not adjacent to the drawer at (25, 2).
        assert!(
            !map.hint_line().contains("Search: X"),
            "search hint must hide when not adjacent to the drawer; got {:?}",
            map.hint_line()
        );

        // Walk to (26, 2) — adjacent to the drawer but not the clerk.
        // From spawn: right four times, up twice. (24, 4) and (25, 4)
        // are floor cells in the same room as spawn; (26, 3) crosses
        // the door row at col 26 (floor between the two `+` doors).
        for _ in 0..4 {
            map.handle_input(&mut ctx, Input::Right);
        }
        for _ in 0..2 {
            map.handle_input(&mut ctx, Input::Up);
        }
        assert_eq!(map.player(), (26, 2));
        assert!(map.nearby_lost_and_found());
        assert!(
            map.hint_line().contains("Search: X"),
            "search hint must advertise the key when adjacent to the drawer; got {:?}",
            map.hint_line()
        );
    }

    #[test]
    fn map_hint_advertises_buy_affordance_only_when_adjacent() {
        // Same shape as the search-affordance test, scoped to the
        // vendor prompt: `b`/`B` is the only route to the SPEC §9
        // step 4 vendor screen, and the hint must surface that key
        // exactly when the player stands next to the Night Clerk.
        let mut map = fresh_map_screen();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));

        // Spawn (22, 4): not adjacent to the Night Clerk at (24, 2).
        assert!(
            !map.hint_line().contains("Buy: B"),
            "buy hint must hide when not adjacent to the clerk; got {:?}",
            map.hint_line()
        );

        // Walk to (24, 3) — adjacent only to the clerk; (24, 3) is on
        // the door row but col 24 is floor between the room-3 / room-4
        // doors at cols 18 and 27.
        for _ in 0..2 {
            map.handle_input(&mut ctx, Input::Right);
        }
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(map.player(), (24, 3));
        assert!(map.nearby_clerk());
        assert!(
            map.hint_line().contains("Buy: B"),
            "buy hint must advertise the key when adjacent to the clerk; got {:?}",
            map.hint_line()
        );
    }

    #[test]
    fn nearby_clerk_only_true_when_adjacent_to_clerk() {
        // The vendor affordance is gated by `nearby_clerk` rather than
        // `nearby_npc` so an adjacent Bellhop/Maid never accidentally
        // routes a `b` press into a vendor flow they do not own. Pin
        // the contract: spawn returns false (no NPC at the four
        // cardinals), one step right of the clerk returns true.
        let map = fresh_map_screen();
        assert!(
            !map.nearby_clerk(),
            "spawn must not be adjacent to the Night Clerk"
        );

        // Walk to (24, 3) — the cell directly south of the Night Clerk
        // at (24, 2). Spawn is (22, 4); right twice + up once lands on
        // it without crossing the clerk's blocking cell or the drawer
        // at (25, 2).
        let mut map = fresh_map_screen();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for _ in 0..2 {
            map.handle_input(&mut ctx, Input::Right);
        }
        map.handle_input(&mut ctx, Input::Up);
        assert_eq!(map.player(), (24, 3));
        assert!(
            map.nearby_clerk(),
            "cell south of clerk at (24, 3) must satisfy nearby_clerk"
        );
    }

    #[test]
    fn buy_key_pushes_vendor_prompt_when_adjacent_to_clerk() {
        // `b` and `B` must both push the vendor screen — SPEC §9 step 7
        // requires case-folded hotkeys, and the integration must mirror
        // it. `B` is also the Bellhop's glyph; that is *not* a hotkey
        // collision because input mapping happens at the screen level
        // before any glyph lookup.
        for key in [Input::Char('b'), Input::Char('B')] {
            let mut map = fresh_map_screen();
            let cfg = fixture_config();
            let fc = fixture_context();
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
            for _ in 0..2 {
                map.handle_input(&mut ctx, Input::Right);
            }
            map.handle_input(&mut ctx, Input::Up);
            assert_eq!(map.player(), (24, 3));
            assert!(map.nearby_clerk());
            let cmd = map.handle_input(&mut ctx, key);
            assert!(
                matches!(cmd, ScreenCommand::Push(_)),
                "{key:?} adjacent to Night Clerk must push the vendor screen"
            );
        }
    }

    #[test]
    fn buy_key_inert_when_not_adjacent_to_clerk() {
        // From spawn the player is two rows below the clerk (no
        // cardinal neighbour). Pressing `b` here must be silent — the
        // SPEC §4.1 inert-key contract — so a player who taps the
        // wrong key cannot open a vendor prompt mid-corridor.
        let mut map = fresh_map_screen();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        assert!(!map.nearby_clerk());
        let cmd = map.handle_input(&mut ctx, Input::Char('b'));
        assert!(
            matches!(cmd, ScreenCommand::None),
            "buy key must be inert away from the clerk"
        );
    }

    // ---- SPEC_v2 §Task 13a clue-inspection turn spend ---------------

    /// Walk a fresh map screen up to the cell directly south of the
    /// drawer (25, 3). Pulled out so each Task 13a integration test
    /// reads "set up, drive search key, assert" rather than repeating
    /// the four-step approach. Returns the live screen and a context
    /// already pointed at the supplied world DB so the caller can
    /// keep dispatching `Input::Char('x')` against the same borrow.
    fn walk_to_drawer(map: &mut MapScreen, ctx: &mut GameContext<'_>) {
        for _ in 0..3 {
            map.handle_input(ctx, Input::Right);
        }
        map.handle_input(ctx, Input::Up);
        assert_eq!(map.player(), (25, 3), "approach must land south of drawer");
        assert!(map.nearby_lost_and_found());
    }

    /// Set up the kit + Murder Motel migrations a real install would
    /// have applied by the time the lobby's X-press fires. Mirrors
    /// the helper used by the `world.rs` Task 13a tests so a future
    /// schema add lands in one place.
    fn world_with_full_stack(dir: &tempfile::TempDir) -> foglet_game::WorldDb {
        use crate::world::MOTEL_WORLD_STATE_MIGRATION;
        let db_path = dir.path().join("world.sqlite");
        let mut world = foglet_game::WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&foglet_game::TURN_LEDGER_MIGRATION)
            .expect("apply turn_ledger migration");
        world
            .apply_migration(&MOTEL_WORLD_STATE_MIGRATION)
            .expect("apply motel_world_state migration");
        world
    }

    /// Build [`SharedSlots`] with a [`FixedDateProvider`] pinned to
    /// "today" so the turn ledger's date math is independent of the
    /// wall clock. Without this the Task 13a tests would have to walk
    /// the date forward to hit the carryover branch — covered by the
    /// kit-level reset tests, not by these integration ones.
    fn fixed_date_slots() -> SharedSlots {
        use foglet_game::{FixedDateProvider, LocalDate};
        SharedSlots::default().with_date_provider(Rc::new(FixedDateProvider::new(
            LocalDate::parse("2026-05-09").expect("valid date"),
        )))
    }

    fn fresh_map_screen_with_slots(slots: SharedSlots) -> MapScreen {
        let cfg = fixture_config();
        slots.reset(cfg.game.start_x, cfg.game.start_y);
        // After `reset` the fixed date provider survives because
        // `SharedSlots::reset` only touches the persisted slots, not
        // the runtime services. Construct via `with_shared` so the
        // map keys off the same handle.
        MapScreen::with_shared(cfg.game.start_x, cfg.game.start_y, slots)
    }

    /// SPEC_v2 §Task 13a: pressing the search key adjacent to the
    /// drawer with a live world DB attached must spend one daily turn
    /// AND still push the prompt. Drives the screen end-to-end (no
    /// helper short-circuit) so a regression that disconnected the
    /// X-press from the turn helper would surface here.
    #[test]
    fn search_key_spends_a_turn_when_world_db_attached() {
        let dir = tempfile::tempdir().expect("tempdir");
        let world = world_with_full_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots.clone());

        let cmd = {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to_drawer(&mut map, &mut ctx);
            map.handle_input(&mut ctx, Input::Char('x'))
        };

        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "search key with attached world DB must still push the prompt; got {cmd:?}"
        );

        // The scaffold's daily_allowance is 3 — a single press leaves
        // 2 turns. We read straight out of the ledger via the helper
        // because the screen has no remaining-turns surface yet (Task
        // 13b will add one).
        use crate::world::{spend_clue_inspection_turn, ClueInspectionOutcome};
        let next = spend_clue_inspection_turn(&world, &fc, &cfg, &*slots.date_provider);
        assert_eq!(
            next,
            ClueInspectionOutcome::Spent { remaining: 1 },
            "second spend must observe the post-X-press balance (3 - 1 - 1 = 1)"
        );
    }

    /// Once the day's allowance is exhausted, the X-press must NOT
    /// push the prompt — the player gets a feedback line instead and
    /// stays on the lobby map. Mirrors the SPEC_v2 §Task 13a "examine
    /// hotspots spends turns" contract: out of turns means out of
    /// inspections.
    #[test]
    fn search_key_blocks_prompt_when_balance_exhausted() {
        use crate::world::{spend_clue_inspection_turn, NO_CLUE_TURNS_FEEDBACK};

        let dir = tempfile::tempdir().expect("tempdir");
        let world = world_with_full_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots.clone());

        // Drain the allowance directly through the helper so the test
        // isolates the *blocked* X-press. Three spends at
        // daily_allowance=3 ⇒ balance 0.
        for _ in 0..3 {
            let _ = spend_clue_inspection_turn(&world, &fc, &cfg, &*slots.date_provider);
        }

        let cmd = {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to_drawer(&mut map, &mut ctx);
            map.handle_input(&mut ctx, Input::Char('x'))
        };

        assert!(
            matches!(cmd, ScreenCommand::None),
            "exhausted balance must keep the prompt closed; got {cmd:?}"
        );
        let feedback = slots
            .feedback
            .borrow()
            .clone()
            .expect("rejected X-press must surface the no-turns feedback line");
        assert_eq!(
            feedback.text(),
            NO_CLUE_TURNS_FEEDBACK,
            "feedback text must match the SPEC §Task 13a constant"
        );
    }

    /// Without a world DB attached, the X-press still pushes the
    /// prompt — the helper short-circuits to `NotConfigured` and the
    /// screen treats that exactly like `Spent`. Pre-v2 single-player
    /// games and headless tests that don't stand up a world DB keep
    /// working unchanged.
    #[test]
    fn search_key_without_world_db_still_pushes_prompt() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots);

        let cmd = {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
            walk_to_drawer(&mut map, &mut ctx);
            map.handle_input(&mut ctx, Input::Char('x'))
        };

        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "no world DB attached must fall back to the unconditional push; got {cmd:?}"
        );
    }

    // ---- SPEC_v2 §Task 13b remaining-turns status line --------------

    /// Before any tick fires (or with no world DB attached), the
    /// hint line must NOT carry a "Turns:" segment — single-player
    /// runs keep the lobby's status row terse.
    #[test]
    fn hint_line_omits_turns_segment_until_cache_populated() {
        let slots = fixed_date_slots();
        let map = fresh_map_screen_with_slots(slots);
        let hint = map.hint_line();
        assert!(
            !hint.contains("Turns:"),
            "fresh map screen must not advertise a turn balance: {hint}"
        );
    }

    /// First tick against a context with a world DB attached must
    /// populate the Task 13b cache so the next render's hint line
    /// includes the full daily allowance. Drives the public `Screen`
    /// surface (not the `read_remaining_turns` helper directly) so a
    /// regression in the tick wiring fails here.
    #[test]
    fn tick_populates_turns_status_from_world_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let world = world_with_full_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots);

        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            let cmd = map.tick(&mut ctx);
            assert!(
                matches!(cmd, ScreenCommand::None),
                "tick should not emit a screen transition; got {cmd:?}"
            );
        }

        let hint = map.hint_line();
        assert!(
            hint.contains("Turns: 3/3"),
            "tick must populate the status cache to the full daily_allowance: {hint}"
        );
    }

    /// After a successful clue inspection the X-press handler updates
    /// the cache in place — without re-querying SQLite — so the next
    /// frame's hint line reflects the post-spend balance. Asserts the
    /// SPEC_v2 §Task 13b refresh-on-spend wiring.
    #[test]
    fn x_press_updates_turns_status_on_successful_spend() {
        let dir = tempfile::tempdir().expect("tempdir");
        let world = world_with_full_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots);

        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to_drawer(&mut map, &mut ctx);
            let cmd = map.handle_input(&mut ctx, Input::Char('x'));
            assert!(matches!(cmd, ScreenCommand::Push(_)));
        }

        let hint = map.hint_line();
        assert!(
            hint.contains("Turns: 2/3"),
            "X-press must drop remaining from 3 to 2 in the status line: {hint}"
        );
    }

    /// When the day's allowance is exhausted, the rejected X-press
    /// still syncs the cache to the canonical zero balance — the
    /// previous frame might have shown "1/3" right before the final
    /// spend, and the rejected press is the right place to land on
    /// "0/3" without waiting for tick to re-query.
    #[test]
    fn rejected_x_press_syncs_turns_status_to_zero() {
        use crate::world::spend_clue_inspection_turn;

        let dir = tempfile::tempdir().expect("tempdir");
        let world = world_with_full_stack(&dir);
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots.clone());

        // Drain the allowance through the helper so the X-press tested
        // below exercises only the rejection path.
        for _ in 0..3 {
            let _ = spend_clue_inspection_turn(&world, &fc, &cfg, &*slots.date_provider);
        }

        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            walk_to_drawer(&mut map, &mut ctx);
            // Pre-seed the cache to a stale "1/3" so the test fails if
            // the rejection path forgets to overwrite it.
            *map.turns_status.borrow_mut() = Some(crate::world::RemainingTurns {
                remaining: 1,
                daily_allowance: 3,
            });
            let cmd = map.handle_input(&mut ctx, Input::Char('x'));
            assert!(matches!(cmd, ScreenCommand::None));
        }

        let hint = map.hint_line();
        assert!(
            hint.contains("Turns: 0/3"),
            "rejected X-press must overwrite stale cache with the true zero balance: {hint}"
        );
    }

    /// SPEC_v2 §Task 13c-ii: events queued by the Lost-and-Found
    /// Drawer callback drain into `world_events` on the next lobby
    /// tick. Drives the public `Screen::tick` surface so a regression
    /// in the wiring (e.g. forgetting to call `flush_pending_clue_events`)
    /// surfaces here, not in a downstream Murder Motel smoke test.
    #[test]
    fn tick_drains_pending_clue_events_into_world_events() {
        use crate::world::{
            PendingClueEvent, CLUE_FOUND_EVENT_KIND, CLUE_FOUND_ROOM_7_KEY_MESSAGE,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        // world_with_full_stack covers players + turn_ledger +
        // motel_world_state but not world_events; layer the events
        // migration on top so the drain has a destination table.
        let mut world = world_with_full_stack(&dir);
        world
            .apply_migration(&foglet_game::WORLD_EVENTS_MIGRATION)
            .expect("apply world_events migration");
        let cfg = fixture_config();
        let fc = fixture_context();
        let slots = fixed_date_slots();
        let mut map = fresh_map_screen_with_slots(slots.clone());

        // Pre-seed the mailbox the way the drawer callback would, so
        // this test isolates the tick-drain wiring from the prompt.
        slots
            .pending_clue_events
            .borrow_mut()
            .push(PendingClueEvent {
                item_id: MapScreen::ROOM_7_KEY_ID.to_string(),
                message: CLUE_FOUND_ROOM_7_KEY_MESSAGE.to_string(),
            });

        {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24)).with_world_db(&world);
            let cmd = map.tick(&mut ctx);
            assert!(matches!(cmd, ScreenCommand::None));
        }

        assert!(
            slots.pending_clue_events.borrow().is_empty(),
            "tick must drain the mailbox so a stale row never re-flushes"
        );
        let events = world.recent_events(10).expect("read events");
        assert_eq!(events.len(), 1, "tick must write exactly one row");
        assert_eq!(events[0].kind, CLUE_FOUND_EVENT_KIND);
        assert_eq!(events[0].message, CLUE_FOUND_ROOM_7_KEY_MESSAGE);
        assert!(
            events[0].player_id.is_some(),
            "tick must attribute the event to the current foglet user"
        );
    }
}
