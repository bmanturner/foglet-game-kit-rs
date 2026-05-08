//! `murder_motel` — the SPEC §13 acceptance-fixture sample game.
//!
//! Each Task 13 sub-iteration grows this binary one screen at a time.
//! Task 13a stood up the project scaffold and a title screen with a
//! menu hint; Task 13b (this commit) adds a real main menu and a
//! controls/help screen behind it. Later sub-tasks layer the map,
//! NPCs, dialog, inventory, locked door, win condition, and save flow
//! on top of it.
//!
//! ## Running
//!
//! ```bash
//! cargo run --example murder_motel
//! ```
//!
//! The example target lives in `crates/foglet_game/Cargo.toml`; its
//! source path points at this directory so the project also reads as
//! a self-contained scaffold an author can copy as a starting point.
//!
//! ## Asset loading
//!
//! `cargo run --example` sets the current working directory to the
//! workspace root, but tests, IDEs, and downstream callers can run the
//! binary from anywhere. We resolve `assets/` against
//! [`CARGO_MANIFEST_DIR`] (the example's host package — `foglet_game`)
//! so the path is stable regardless of CWD. If you copy this scaffold
//! into a standalone Cargo project, swap the constant for a plain
//! `"assets/game.toml"` — `fgk new` already does that for the
//! generated template.
//!
//! [`CARGO_MANIFEST_DIR`]: https://doc.rust-lang.org/cargo/reference/environment-variables.html

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{
    load_context, load_dialog, parse_map, process_env, render_inventory_list, render_menu_list,
    Dialog, DialogState, FlagSet, Game, GameConfig, GameContext, Input, InventoryList, Map,
    MenuList, Screen, ScreenCommand, TileLegend,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

/// Absolute path to `examples/murder_motel/assets/game.toml`.
///
/// Computed at compile time from the host package's manifest dir so
/// the example runs the same regardless of where `cargo` was invoked.
const GAME_TOML_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/murder_motel/assets/game.toml"
);

/// Lobby map source, embedded at compile time.
///
/// `include_str!` keeps the example self-contained — the binary needs
/// no runtime asset lookup to render its first map, which sidesteps
/// the "where am I being run from?" issue that bit the title screen's
/// `assets/game.toml` lookup before [`GAME_TOML_PATH`] was introduced.
/// `fgk new`-generated projects load their starter map from disk via
/// `assets/maps/<name>.txt`; switch to `std::fs::read_to_string` if you
/// copy this scaffold and want hot-editable maps in dev.
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

/// First screen the player sees on launch.
///
/// Shows the configured title, a one-line tagline, and a "Press Enter"
/// prompt that leads into the main menu added in Task 13b. Esc / Q /
/// Ctrl-C still quit directly so a player who lands on the title screen
/// with no patience for menus has an immediate exit.
#[derive(Debug, Default)]
pub struct TitleScreen;

impl TitleScreen {
    /// The label rendered in the bordered title block. Pulled out as a
    /// constant so the integration test can assert it without touching
    /// the runtime.
    pub const HEADING: &'static str = "Murder Motel";
    /// Single-line tagline shown directly under the heading.
    pub const TAGLINE: &'static str = "A noir whodunit at the edge of town.";
    /// Prompt rendered beneath the tagline. The title screen is now a
    /// pure splash gate — Enter advances to the real menu.
    pub const ENTER_HINT: &'static str = "Press Enter to begin   ([Q] Quit)";
}

impl Screen for TitleScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Three centred lines: heading, tagline, prompt. Plain
        // `Paragraph` is enough for a splash; the live menu lives one
        // screen deeper in `MainMenuScreen`.
        let lines = vec![
            Line::from(Span::styled(
                Self::HEADING,
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Self::TAGLINE),
            Line::from(""),
            Line::from(Self::ENTER_HINT),
        ];
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(Self::HEADING));
        frame.render_widget(widget, frame.area());
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Enter advances into the main menu. We push rather than
            // replace so a future "back to title" affordance from the
            // menu is a one-line `Pop` away if we ever want it.
            Input::Enter => ScreenCommand::Push(Box::new(MainMenuScreen::new())),
            // Direct quit affordances. Esc + Q + Ctrl-C match the
            // controls every other screen in the kit honors.
            Input::Esc | Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Identifies a row in the main menu by intent rather than by index, so
/// activation logic doesn't read like "if selected == 2 then push help".
///
/// Stays in lockstep with [`MainMenuScreen::ITEMS`]; the order of those
/// labels and the order of these variants is the same on purpose so the
/// screen can map between them with a plain `as usize` cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainMenuItem {
    /// Start a fresh game. Pushes the lobby [`MapScreen`] with the
    /// player at the spawn coordinates from `assets/game.toml`.
    NewGame,
    /// Resume a saved game. Until Task 13h lands save persistence,
    /// "continue" is identical to [`Self::NewGame`] — push a fresh
    /// lobby. The placeholder is honest about its limits: the screen
    /// is reachable, but the state is not yet restored from disk.
    Continue,
    /// Push the help/controls screen.
    Help,
    /// Exit the runtime.
    Quit,
}

impl MainMenuItem {
    /// Convert a row index into a [`MainMenuItem`]. Out-of-range values
    /// are treated as no-op so a stray activation never panics.
    fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::NewGame),
            1 => Some(Self::Continue),
            2 => Some(Self::Help),
            3 => Some(Self::Quit),
            _ => None,
        }
    }

    /// The [`ScreenCommand`] this menu item should produce when
    /// activated. Centralised here so [`MainMenuScreen`] can react to
    /// both Enter-on-selection and the per-item shortcut keys with one
    /// call.
    ///
    /// `ctx` is borrowed so New Game / Continue can read the spawn
    /// coordinates from `assets/game.toml` rather than hard-coding
    /// them — the map screen pushes with whatever `start_x` /
    /// `start_y` the config carries, which keeps the example honest
    /// if those values are ever retuned.
    fn activate(self, ctx: &GameContext<'_>) -> ScreenCommand {
        match self {
            // Both New Game and Continue land on a fresh lobby map for
            // now. Task 13h replaces Continue with a save-restoring
            // path; until then both options take the player to the
            // same starting state, which is the truthful behaviour for
            // a game that has no persistence yet.
            Self::NewGame | Self::Continue => {
                let (sx, sy) = (ctx.config.game.start_x, ctx.config.game.start_y);
                ScreenCommand::Push(Box::new(MapScreen::new_lobby(sx, sy)))
            }
            Self::Help => ScreenCommand::Push(Box::new(HelpScreen)),
            Self::Quit => ScreenCommand::Quit,
        }
    }
}

/// Top-level main menu shown after the title splash.
///
/// Owns the item label vector and the selection cursor. Rendering goes
/// through the shared [`MenuList`] widget so the visual baseline stays
/// consistent with later screens (dialog choices, inventory selection)
/// that will reuse the same widget.
#[derive(Debug)]
pub struct MainMenuScreen {
    /// Cached item labels owned by the screen. The widget borrows this
    /// each frame; storing it on the screen avoids re-allocating four
    /// `String`s per render.
    items: Vec<String>,
    /// Index of the currently highlighted row. `MenuList` clamps for us
    /// so out-of-range values are safe, but we still maintain it
    /// faithfully so keyboard navigation feels correct.
    selected: usize,
}

impl MainMenuScreen {
    /// Title rendered on the menu's bordered block.
    pub const TITLE: &'static str = "Main Menu";
    /// Order of menu items. Kept aligned with [`MainMenuItem`] so the
    /// row index can be cast straight to an item.
    pub const ITEMS: [&'static str; 4] = ["New Game", "Continue", "Help", "Quit"];

    /// Build a menu with the cursor on `New Game`.
    pub fn new() -> Self {
        Self {
            items: Self::ITEMS.iter().map(|s| s.to_string()).collect(),
            selected: 0,
        }
    }

    /// Move the selection cursor by `delta`, clamping at both ends.
    /// Pulled out of `handle_input` so tests can exercise the cursor
    /// behaviour directly.
    fn move_cursor(&mut self, delta: i32) {
        let len = self.items.len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }
}

impl Default for MainMenuScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen for MainMenuScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre a fixed-size menu inside the frame so the look is the
        // same regardless of terminal dimensions (within the SPEC §7.1
        // 80x24 floor).
        let area = centred_rect(40, 8, frame.area());
        let menu = MenuList {
            title: Some(Self::TITLE),
            items: &self.items,
            selected: self.selected,
        };
        render_menu_list(frame, area, &menu);
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Vertical navigation. We deliberately accept both arrows
            // and the classic `j`/`k` so vi-flavoured BBS callers feel
            // at home — same as we'll do on the map screen later.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.move_cursor(-1);
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.move_cursor(1);
                ScreenCommand::None
            }
            // Activate the highlighted item.
            Input::Enter => MainMenuItem::from_index(self.selected)
                .map(|item| item.activate(ctx))
                .unwrap_or(ScreenCommand::None),
            // Per-item shortcuts mirror the first letter of each label.
            // `Q` doubles as a generic quit affordance — both meanings
            // resolve to `ScreenCommand::Quit` so there's no ambiguity.
            Input::Char('n') | Input::Char('N') => MainMenuItem::NewGame.activate(ctx),
            Input::Char('c') | Input::Char('C') => MainMenuItem::Continue.activate(ctx),
            Input::Char('h') | Input::Char('H') => MainMenuItem::Help.activate(ctx),
            Input::Char('q') | Input::Char('Q') | Input::Esc | Input::Ctrl('c') => {
                ScreenCommand::Quit
            }
            _ => ScreenCommand::None,
        }
    }
}

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
    /// Player column. Updated only by [`Self::try_move`] so the
    /// invariant "player position is always walkable" is maintained
    /// in one place.
    player_x: u16,
    /// Player row. Same invariant as `player_x`.
    player_y: u16,
    /// Narrative-flag store shared with [`DialogScreen`] so flags set
    /// during a conversation persist across the dialog stack push/pop
    /// boundary. The dialog state machine takes a `&mut FlagSet`; an
    /// `Rc<RefCell<_>>` is the lightest way to lend the same
    /// collection to two screens without the runtime needing a
    /// flag-store concept yet (Task 13h will move this onto the save
    /// manager so flags survive process exit).
    flags: Rc<RefCell<FlagSet>>,
    /// Set of collected item IDs, shared with the [`InventoryScreen`]
    /// the same way `flags` is shared with the [`DialogScreen`]. A
    /// `BTreeSet<String>` mirrors the [`FlagSet`] shape so Task 13h
    /// can serialise both stores through the same code path.
    inventory: Rc<RefCell<BTreeSet<String>>>,
}

impl MapScreen {
    /// Title rendered on the bordered block surrounding the map.
    pub const TITLE: &'static str = "Murder Motel — Lobby";

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

    /// Build the lobby map screen with the player at `(start_x,
    /// start_y)`. The constructor parses [`LOBBY_MAP_TEXT`] against
    /// the lobby legend; the parse can only fail if the embedded
    /// asset diverges from the legend, which is a build-time bug —
    /// hence the `expect`.
    pub fn new_lobby(start_x: u16, start_y: u16) -> Self {
        let legend = lobby_legend();
        let map =
            parse_map(LOBBY_MAP_TEXT, &legend).expect("embedded lobby map parses against legend");
        let mut screen = Self {
            map,
            player_x: 0,
            player_y: 0,
            flags: Rc::new(RefCell::new(FlagSet::new())),
            inventory: Rc::new(RefCell::new(BTreeSet::new())),
        };
        // Clamp the spawn against the map bounds and walkability so a
        // misconfigured `start_x` / `start_y` in `assets/game.toml`
        // can't put the player inside a wall. If the requested cell
        // is unwalkable we walk a small spiral outward looking for a
        // floor; if even that fails (a totally hostile map) we fall
        // back to (1, 1) which the lobby legend guarantees is floor.
        screen.player_x = start_x;
        screen.player_y = start_y;
        if !screen.map.is_walkable(start_x, start_y) {
            if let Some((fx, fy)) = screen.find_nearest_walkable(start_x, start_y) {
                screen.player_x = fx;
                screen.player_y = fy;
            } else {
                screen.player_x = 1;
                screen.player_y = 1;
            }
        }
        screen
    }

    /// Player coordinates, exposed so tests and future siblings (the
    /// inventory screen, the save manager) can read the position
    /// without reaching into private state.
    pub fn player(&self) -> (u16, u16) {
        (self.player_x, self.player_y)
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
        let target_x = match (self.player_x as i32).checked_add(dx) {
            Some(v) if v >= 0 => v as u16,
            _ => return false,
        };
        let target_y = match (self.player_y as i32).checked_add(dy) {
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
        // NPCs are solid: walking into one is converted to "stand
        // adjacent". The talk affordance (Enter while next to an
        // NPC) handles interaction; without this guard the player
        // would have to step *off* an NPC's cell to address them,
        // which is the wrong feel for a top-down game.
        if Self::npc_at(target_x, target_y).is_some() {
            return false;
        }
        self.player_x = target_x;
        self.player_y = target_y;
        // Items pick up on step. The render path filters collected
        // items out of the overlay, so the cell visually clears the
        // same frame the inventory grows. Items aren't blocking — a
        // stranded item under foot would otherwise trap the player
        // until they pressed Enter, which is the wrong feel here.
        if let Some(item) = Self::item_at(target_x, target_y) {
            self.inventory.borrow_mut().insert(item.id.to_string());
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
        for (dx, dy) in OFFSETS {
            let cx = self.player_x as i32 + dx;
            let cy = self.player_y as i32 + dy;
            if cx < 0 || cy < 0 {
                continue;
            }
            if let Some(npc) = Self::npc_at(cx as u16, cy as u16) {
                return Some(npc);
            }
        }
        None
    }

    /// Shared handle to the narrative-flag store. Cloned so the
    /// dialog screen and the map screen mutate the same `RefCell`.
    pub fn flags(&self) -> Rc<RefCell<FlagSet>> {
        Rc::clone(&self.flags)
    }

    /// Shared handle to the inventory store. Cloned so the
    /// [`InventoryScreen`] reads the same set the [`MapScreen`] writes
    /// when the player walks over an item. Task 13h will lift this
    /// onto the save manager so contents survive process exit; until
    /// then it lives on the map screen for the play session.
    pub fn inventory(&self) -> Rc<RefCell<BTreeSet<String>>> {
        Rc::clone(&self.inventory)
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
        self.inventory.borrow().contains(id)
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
        // Stamp the player glyph by replacing the byte at the player's
        // column with `@`. The lobby legend uses ASCII space/`#`/`+`
        // (all 1-byte UTF-8) and the player glyph is also ASCII, so
        // byte-level replacement is safe here. If the legend ever
        // grows multi-byte glyphs this needs to switch to a
        // char-aware splice.
        if (self.player_y as usize) < rows.len() {
            let row = &mut rows[self.player_y as usize];
            let col = self.player_x as usize;
            if col < row.len() {
                let mut chars: Vec<char> = row.chars().collect();
                chars[col] = Self::PLAYER_GLYPH;
                *row = chars.into_iter().collect();
            }
        }
        rows
    }
}

/// Legend used to parse the lobby ASCII map.
///
/// Pulled out of [`MapScreen::new_lobby`] so the integration tests can
/// build the same legend the screen uses without coupling to the
/// constructor's internals. Glyphs:
///
/// - `#` → wall (blocking)
/// - ` ` → floor (walkable)
/// - `+` → door (walkable)
/// - `L` → locked door (parsed as a `Custom` kind so the kit's tile
///   model stays minimal; [`MapScreen::try_move`] gates passage on the
///   brass key being in inventory, while render styles the cell red
///   until unlocked)
fn lobby_legend() -> TileLegend {
    TileLegend::from_pairs([
        ("#", "wall"),
        (" ", "floor"),
        ("+", "door"),
        ("L", "locked_door"),
    ])
    .expect("static lobby legend parses")
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
                if (y as u16) == self.player_y && (x as u16) == self.player_x {
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
            let hint = Paragraph::new(Self::HINT_LINE).alignment(Alignment::Center);
            frame.render_widget(hint, hint_area);
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Cardinal movement, both arrow keys and vi-style aliases.
            // The screen does not announce a "blocked" state when a
            // move fails — the lack of motion is the feedback.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.try_move(0, -1);
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.try_move(0, 1);
                ScreenCommand::None
            }
            Input::Left | Input::Char('h') => {
                self.try_move(-1, 0);
                ScreenCommand::None
            }
            Input::Right | Input::Char('l') | Input::Char('L') => {
                self.try_move(1, 0);
                ScreenCommand::None
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
            // Esc / Backspace pop back to the main menu so a curious
            // player can return to the splash flow without quitting.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,
            // Q and Ctrl-C remain hard-quit affordances everywhere.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            _ => ScreenCommand::None,
        }
    }
}

impl MapScreen {
    /// One-line movement hint shown directly below the map block.
    /// Pulled out as a constant so tests can assert it appears in the
    /// rendered buffer without binding to the precise wording.
    pub const HINT_LINE: &'static str =
        "Move: arrows/hjkl    Talk: Enter    Inv: I    Back: Esc    Quit: Q";
}

/// Modal dialog screen pushed when the player talks to an NPC.
///
/// Owns a parsed [`Dialog`] graph and a [`DialogState`] cursor walking
/// it. The shared [`FlagSet`] is borrowed from the [`MapScreen`] beneath
/// us via an `Rc<RefCell<_>>`, so flags set during this conversation
/// (the Night Clerk's `heard_rumor`, `has_key`, etc.) survive after the
/// screen pops and are visible to later conversations and to future
/// game logic (locked doors in 13f, win conditions in 13g).
///
/// ## Render contract
///
/// While the cursor sits on a line, the body shows the speaker's name
/// and the line text plus a "[Enter] continue" hint. Once the lines on
/// the current node are exhausted, the body shows the available
/// choices through the shared [`MenuList`] widget. When the dialog has
/// finished (no more lines, no more choices, no goto) the body shows
/// a "[Esc] leave" hint.
///
/// ## Input contract
///
/// - `Up` / `Down` (and `j`/`k`) move the choice cursor when choices
///   are visible.
/// - `Enter` advances the next line, picks the highlighted choice, or
///   pops the screen when the dialog is finished.
/// - `Esc` / `Backspace` pops at any time — the player can always walk
///   away mid-conversation.
/// - `Q` / `Ctrl-C` still hard-quit, matching the rest of the kit.
pub struct DialogScreen {
    /// Speaker label rendered in the dialog block's title. Owned as
    /// `&'static str` because [`Npc`] is `Copy` and lives in
    /// [`MapScreen::NPCS`].
    speaker: &'static str,
    /// Parsed dialog graph. Held by value so `DialogState` can borrow
    /// it across multiple input dispatches without lifetime gymnastics.
    dialog: Dialog,
    /// Cursor walking [`Self::dialog`].
    state: DialogState,
    /// Shared narrative-flag store. Cloned from [`MapScreen::flags`]
    /// at construction time.
    flags: Rc<RefCell<FlagSet>>,
    /// Highlighted choice when choices are visible. Clamped against
    /// `available_choices().len()` at render time, so it's safe to
    /// keep around even when choices change between frames.
    selected_choice: usize,
}

impl DialogScreen {
    /// Build a dialog screen for the given NPC, sharing the supplied
    /// flag store. The dialog YAML is parsed eagerly here so any
    /// schema error surfaces at the moment the player presses Enter
    /// rather than on the first render.
    ///
    /// `expect` is acceptable because the YAML ships in the binary
    /// (`include_str!`); a parse failure is a build-time bug, not a
    /// runtime input.
    pub fn new(npc: &Npc, flags: Rc<RefCell<FlagSet>>) -> Self {
        let dialog = load_dialog(npc.dialog_yaml).expect("embedded NPC dialog parses");
        let state = {
            let mut fs = flags.borrow_mut();
            DialogState::start(&dialog, &mut fs)
        };
        Self {
            speaker: npc.name,
            dialog,
            state,
            flags,
            selected_choice: 0,
        }
    }

    /// Speaker name. Exposed for tests asserting which NPC is on
    /// screen without poking at private fields.
    pub fn speaker(&self) -> &str {
        self.speaker
    }

    /// Whether the dialog cursor reports finished. Read-only: tests
    /// assert end-state without driving the screen through input.
    pub fn is_finished(&self) -> bool {
        self.state.is_finished()
    }

    /// Borrow the underlying dialog state. Mostly for tests; gameplay
    /// code routes through `handle_input`.
    pub fn state(&self) -> &DialogState {
        &self.state
    }

    /// Snapshot of the choice labels currently presented. Useful for
    /// tests that want to assert "the unlocked branch appears after
    /// the rumor flag is set" without owning a `Frame`.
    pub fn current_choice_labels(&self) -> Vec<String> {
        let flags = self.flags.borrow();
        self.state
            .available_choices(&self.dialog, &flags)
            .into_iter()
            .map(|c| c.text.clone())
            .collect()
    }
}

impl Screen for DialogScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Dialog modal sits across the bottom of the screen — high
        // enough to fit the longest two-line node in the night clerk's
        // script plus a hint row, and wide enough to fit the longest
        // choice label without truncation. We centre it horizontally
        // so the player's eye returns to roughly where the map sat.
        let outer = frame.area();
        let modal_w = outer.width.min(60);
        let modal_h = outer.height.min(10);
        let area = Rect {
            x: outer.x + outer.width.saturating_sub(modal_w) / 2,
            y: outer.y + outer.height.saturating_sub(modal_h) / 2,
            width: modal_w,
            height: modal_h,
        };

        let block = Block::default().borders(Borders::ALL).title(self.speaker);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Body decision: line-pumping mode, choice mode, or finished
        // mode. Each mode renders its own widget into `inner`.
        if let Some(line) = self.state.current_line(&self.dialog) {
            // Reserve a one-row hint band at the bottom of `inner` for
            // the "[Enter] continue" prompt; the rest is the line text
            // wrapped to fit. `Wrap { trim: false }` keeps the
            // author's literal punctuation but still wraps long lines.
            let hint_h = 1.min(inner.height);
            let body_h = inner.height.saturating_sub(hint_h);
            let body = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: body_h,
            };
            let hint = Rect {
                x: inner.x,
                y: inner.y + body_h,
                width: inner.width,
                height: hint_h,
            };
            let body_widget = Paragraph::new(line.to_string())
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false });
            frame.render_widget(body_widget, body);
            if hint_h > 0 {
                frame.render_widget(
                    Paragraph::new("[Enter] continue   [Esc] leave")
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(Color::DarkGray)),
                    hint,
                );
            }
            return;
        }

        if self.is_finished() {
            // Terminal node — the dialog ran off the end. Nothing to
            // render except a leave hint; pressing Esc (or Enter)
            // returns to the map.
            let hint = Paragraph::new("(They turn away.)\n\n[Enter / Esc] leave")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint, inner);
            return;
        }

        // Choice mode. Build owned `String`s for the menu widget and
        // clamp the selection cursor against the live choice count.
        let choice_labels = self.current_choice_labels();
        if choice_labels.is_empty() {
            // Defensive: validator guarantees this shape can only
            // happen if every choice is gated *and* the node has no
            // goto; render a leave hint so the player isn't stuck.
            let hint = Paragraph::new("(There's nothing more to say.)\n\n[Esc] leave")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(hint, inner);
            return;
        }
        let selected = self.selected_choice.min(choice_labels.len() - 1);
        let menu = MenuList {
            title: None,
            items: &choice_labels,
            selected,
        };
        // Reserve a hint band at the bottom of the modal.
        let hint_h = 1.min(inner.height);
        let body_h = inner.height.saturating_sub(hint_h);
        let body = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h,
        };
        let hint = Rect {
            x: inner.x,
            y: inner.y + body_h,
            width: inner.width,
            height: hint_h,
        };
        render_menu_list(frame, body, &menu);
        if hint_h > 0 {
            frame.render_widget(
                Paragraph::new("[Up/Down] choose    [Enter] pick    [Esc] leave")
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::DarkGray)),
                hint,
            );
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Walk away from the conversation.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,

            // Cursor movement applies only while choices are showing;
            // outside that mode we treat it as inert (silent reject)
            // so a stray arrow keystroke doesn't accidentally advance
            // the line cursor.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected_choice > 0 {
                    self.selected_choice -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                let labels = self.current_choice_labels();
                if !labels.is_empty() && self.selected_choice + 1 < labels.len() {
                    self.selected_choice += 1;
                }
                ScreenCommand::None
            }

            Input::Enter => {
                if self.is_finished() {
                    return ScreenCommand::Pop;
                }
                let mut flags = self.flags.borrow_mut();
                if self.state.current_line(&self.dialog).is_some() {
                    // Pump the next line. `advance` only errors if
                    // already finished, which we just checked.
                    let _ = self.state.advance(&self.dialog, &mut flags);
                    return ScreenCommand::None;
                }
                // Choice mode. Use the live count to clamp the index;
                // an out-of-range pick returns NoChoices/OutOfRange
                // and we fall through to a no-op rather than crashing.
                let labels_len = self.state.available_choices(&self.dialog, &flags).len();
                if labels_len == 0 {
                    // Hub with all choices gated + no goto: try
                    // advancing to honour any fallback the validator
                    // permitted. If `advance` finishes the dialog,
                    // the next Enter will Pop.
                    let _ = self.state.advance(&self.dialog, &mut flags);
                    return ScreenCommand::None;
                }
                let idx = self.selected_choice.min(labels_len - 1);
                let _ = self.state.choose(&self.dialog, &mut flags, idx);
                // Reset cursor for the next node so a long previous
                // selection doesn't carry over to a short choice list.
                self.selected_choice = 0;
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Static help / controls reference, pushed from the main menu.
///
/// SPEC §13 calls out a help screen as part of the Task 13b acceptance
/// fixture; it lists the controls every subsequent screen will rely on
/// (movement, selection, save, quit) so a player who lands here knows
/// what to expect across the whole game.
#[derive(Debug, Default)]
pub struct HelpScreen;

impl HelpScreen {
    /// Block title used in render and in tests.
    pub const TITLE: &'static str = "Controls";
    /// Lines shown in the body. Stored as `&'static str` so they're
    /// trivially testable and so the screen does no allocation per
    /// frame.
    pub const LINES: &'static [&'static str] = &[
        "Move          Arrow keys, or h/j/k/l",
        "Talk          Enter (when standing next to an NPC)",
        "Select        Enter",
        "Inventory     I",
        "Back / Cancel Esc or Backspace",
        "Save          S",
        "Quit          Q  (Ctrl-C also works anywhere)",
        "",
        "Press Esc or Backspace to return to the main menu.",
    ];
}

impl Screen for HelpScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let area = centred_rect(60, (Self::LINES.len() as u16) + 2, frame.area());
        let lines: Vec<Line<'_>> = Self::LINES.iter().map(|s| Line::from(*s)).collect();
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Esc / Backspace pop back to the menu beneath us. Q and
            // Ctrl-C still quit outright so the help screen never traps
            // a player who just wants out.
            Input::Esc | Input::Backspace => ScreenCommand::Pop,
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            _ => ScreenCommand::None,
        }
    }
}

/// Modal inventory screen pushed from the [`MapScreen`] via `i`.
///
/// Reads from the same `Rc<RefCell<BTreeSet<String>>>` the map screen
/// writes to, so the screen always reflects the current pocket
/// contents. Looks each ID up in [`MapScreen::ITEMS`] to recover its
/// display name; items in the set without a catalog entry are skipped
/// silently — that shape is unreachable today but a forward-compatible
/// no-op keeps the screen safe across future Task 13h save migrations
/// that might surface a stale ID.
///
/// ## Render contract
///
/// Items show as a [`InventoryList`] with the screen title on the
/// border and a "(nothing in your pockets)" empty hint when the set
/// is empty. A one-row band beneath the list shows close affordances.
///
/// ## Input contract
///
/// - `Up` / `Down` (and `j`/`k`) move the highlight cursor.
/// - `Esc` / `Backspace` / `i` pop back to the map.
/// - `Q` / `Ctrl-C` still hard-quit, matching the rest of the kit.
pub struct InventoryScreen {
    /// Shared store cloned from [`MapScreen::inventory`] at construction.
    inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Highlighted row. Clamped against the visible item count at
    /// render time so an item collected mid-frame never points the
    /// cursor off the end of the list.
    selected: usize,
}

impl InventoryScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Inventory";
    /// Empty-state hint shown when the player's pockets are empty.
    /// Pulled out as a constant so the renderer test can assert it
    /// without binding to incidental wording elsewhere in the code.
    pub const EMPTY_HINT: &'static str = "(nothing in your pockets)";

    /// Build an inventory screen sharing the supplied store.
    pub fn new(inventory: Rc<RefCell<BTreeSet<String>>>) -> Self {
        Self {
            inventory,
            selected: 0,
        }
    }

    /// Display labels for each currently held item, in catalog order.
    ///
    /// Walks the static [`MapScreen::ITEMS`] catalog rather than
    /// iterating the set directly so the UI order is stable and
    /// independent of insertion order. Tests use this to assert
    /// "exactly the picked-up items appear, with their canonical
    /// names" without going through a `Frame`.
    pub fn current_labels(&self) -> Vec<String> {
        let held = self.inventory.borrow();
        MapScreen::ITEMS
            .iter()
            .filter(|item| held.contains(item.id))
            .map(|item| item.name.to_string())
            .collect()
    }
}

impl Screen for InventoryScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let labels = self.current_labels();
        // Modal sized for the longest possible item label plus the
        // selection gutter, with vertical room for all five items
        // and a hint row. Clamps inside `centred_rect` if the
        // terminal is smaller (e.g. `TestBackend` fixtures).
        let outer = frame.area();
        let area = centred_rect(36, (MapScreen::ITEMS.len() as u16) + 4, outer);

        // Reserve a one-row hint band at the bottom of the modal so
        // the controls are always visible regardless of inventory
        // contents.
        let hint_h = 1.min(area.height);
        let body_h = area.height.saturating_sub(hint_h);
        let body = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: body_h,
        };
        let hint = Rect {
            x: area.x,
            y: area.y + body_h,
            width: area.width,
            height: hint_h,
        };

        let selected = if labels.is_empty() {
            0
        } else {
            self.selected.min(labels.len() - 1)
        };
        let inv = InventoryList {
            title: Some(Self::TITLE),
            items: &labels,
            selected,
            empty_hint: Self::EMPTY_HINT,
        };
        render_inventory_list(frame, body, &inv);
        if hint_h > 0 {
            let widget = Paragraph::new("[Up/Down] choose    [Esc/I] close")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(widget, hint);
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on hard-quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Esc / Backspace / `i` close the modal — `i` toggles so
            // the same keystroke that opens the screen also closes
            // it, which is the muscle-memory shortcut every BBS
            // inventory screen has trained players to expect.
            Input::Esc | Input::Backspace | Input::Char('i') | Input::Char('I') => {
                ScreenCommand::Pop
            }
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                let len = self.inventory.borrow().len();
                if len > 0 && self.selected + 1 < len {
                    self.selected += 1;
                }
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Centre a `width × height` rectangle inside `outer`, clamping the
/// inner size if `outer` is smaller than requested.
///
/// Pulled out so both the menu and the help screen lay out the same
/// way; SPEC §7.1 already guarantees an 80x24 floor so the clamping
/// path only matters for the unit tests that hand in tiny `TestBackend`
/// frames.
fn centred_rect(width: u16, height: u16, outer: Rect) -> Rect {
    let w = width.min(outer.width);
    let h = height.min(outer.height);
    let h_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((outer.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(outer);
    let v_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((outer.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(h_layout[1]);
    v_layout[1]
}

fn main() -> anyhow::Result<()> {
    let config = GameConfig::load(GAME_TOML_PATH)?;
    let foglet = load_context(process_env)?;

    Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen))
        .run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Tests exercise the pure-input contract of every screen without
    //! standing up a real terminal. These run under `cargo test
    //! --examples` (and `cargo test --workspace --all-targets`) but not
    //! the bare `cargo test --workspace` gate — the workspace gate is
    //! covered by the integration test at
    //! `crates/foglet_game/tests/murder_motel_scaffold.rs`, which
    //! verifies the assets parse and the example target is registered.

    use super::*;
    use foglet_game::{ContextSource, FogletContext};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn fixture_config() -> GameConfig {
        GameConfig::load(GAME_TOML_PATH).expect("scaffold game.toml parses")
    }

    fn fixture_context() -> FogletContext {
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

    /// Dispatch one input to the title screen.
    fn dispatch_title(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        TitleScreen.handle_input(&mut ctx, input)
    }

    /// Dispatch one input to a fresh main menu.
    fn dispatch_menu(input: Input) -> (MainMenuScreen, ScreenCommand) {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        let cmd = menu.handle_input(&mut ctx, input);
        (menu, cmd)
    }

    /// Dispatch one input to a fresh help screen.
    fn dispatch_help(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        HelpScreen.handle_input(&mut ctx, input)
    }

    // ---- TitleScreen ---------------------------------------------------

    #[test]
    fn title_esc_quits() {
        assert!(matches!(dispatch_title(Input::Esc), ScreenCommand::Quit));
    }

    #[test]
    fn title_lowercase_q_quits() {
        assert!(matches!(
            dispatch_title(Input::Char('q')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn title_ctrl_c_quits() {
        assert!(matches!(
            dispatch_title(Input::Ctrl('c')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn title_enter_pushes_main_menu() {
        // Enter is the only "advance" affordance on the title screen
        // now that the real menu exists. Pushing (rather than replacing)
        // keeps the title splash on the stack so a future "back to
        // title" can be a one-line Pop.
        assert!(matches!(
            dispatch_title(Input::Enter),
            ScreenCommand::Push(_)
        ));
    }

    #[test]
    fn title_unrelated_keys_are_inert() {
        // Arrow keys, random letters — anything not part of the title
        // affordances should not transition state. Keeps the title
        // screen safe against stray keystrokes during launch.
        for input in [
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Char('x'),
            Input::Char('1'),
            Input::Char('n'),
            Input::Char('h'),
        ] {
            assert!(
                matches!(dispatch_title(input), ScreenCommand::None),
                "{input:?} should not transition from the title splash"
            );
        }
    }

    // ---- MainMenuScreen ------------------------------------------------

    #[test]
    fn menu_starts_on_first_item() {
        let menu = MainMenuScreen::new();
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.items.len(), MainMenuScreen::ITEMS.len());
    }

    #[test]
    fn menu_down_advances_cursor() {
        let (menu, cmd) = dispatch_menu(Input::Down);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn menu_up_clamps_at_top() {
        let (menu, cmd) = dispatch_menu(Input::Up);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(menu.selected, 0, "Up at the top must stay at the top");
    }

    #[test]
    fn menu_down_clamps_at_bottom() {
        // Hammer Down past the end of the list and confirm we stop at
        // the last row instead of wrapping or panicking.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        for _ in 0..10 {
            menu.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(menu.selected, MainMenuScreen::ITEMS.len() - 1);
    }

    #[test]
    fn menu_vi_keys_navigate() {
        // `j`/`k` mirror Down/Up. We assert one full round trip so a
        // future regression that wires them to the wrong direction
        // surfaces here.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.handle_input(&mut ctx, Input::Char('j'));
        menu.handle_input(&mut ctx, Input::Char('j'));
        assert_eq!(menu.selected, 2);
        menu.handle_input(&mut ctx, Input::Char('k'));
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn menu_enter_on_help_pushes_help_screen() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 2; // Help
        let cmd = menu.handle_input(&mut ctx, Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "Enter on Help must push HelpScreen, got {cmd:?}"
        );
    }

    #[test]
    fn menu_enter_on_quit_quits() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 3; // Quit
        assert!(matches!(
            menu.handle_input(&mut ctx, Input::Enter),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn menu_enter_on_new_game_pushes_map_screen() {
        // Task 13c wires the map screen behind New Game. Before 13c
        // landed this asserted `Quit`; the contract is now to push a
        // screen so the main menu actually leads somewhere. The
        // companion `n` shortcut is tested separately below.
        let (_, cmd) = dispatch_menu(Input::Enter);
        assert!(
            matches!(cmd, ScreenCommand::Push(_)),
            "Enter on New Game must push the map screen, got {cmd:?}"
        );
    }

    #[test]
    fn menu_n_shortcut_pushes_map_screen() {
        let (_, cmd) = dispatch_menu(Input::Char('n'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
    }

    #[test]
    fn menu_enter_on_continue_pushes_map_screen() {
        // Continue is a 13h-shaped placeholder today: same destination
        // as New Game until persistence lands. The screen is reachable
        // from the menu so the player isn't left at a dead end.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 1; // Continue
        let cmd = menu.handle_input(&mut ctx, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Push(_)));
    }

    #[test]
    fn menu_h_shortcut_pushes_help_regardless_of_cursor() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        menu.selected = 0; // cursor on New Game
        let cmd = menu.handle_input(&mut ctx, Input::Char('h'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        // Cursor stays put — shortcut activation must not move the
        // visual selection.
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn menu_quit_keys_quit() {
        for key in [
            Input::Char('q'),
            Input::Char('Q'),
            Input::Esc,
            Input::Ctrl('c'),
        ] {
            let (_, cmd) = dispatch_menu(key);
            assert!(
                matches!(cmd, ScreenCommand::Quit),
                "{key:?} should quit the main menu, got {cmd:?}"
            );
        }
    }

    #[test]
    fn menu_renders_into_test_backend() {
        // End-to-end render proof: draw the menu into a TestBackend and
        // confirm the title and every item appear in the buffer. Locks
        // in the contract that `MainMenuScreen` actually paints itself.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut menu = MainMenuScreen::new();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| menu.render(&mut ctx, frame))
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
            found.contains(MainMenuScreen::TITLE),
            "menu render missing title; buffer was:\n{found}"
        );
        for item in MainMenuScreen::ITEMS {
            assert!(
                found.contains(item),
                "menu render missing item {item:?}; buffer was:\n{found}"
            );
        }
    }

    // ---- HelpScreen ----------------------------------------------------

    #[test]
    fn help_esc_pops() {
        // Esc takes the player back to the menu beneath us — a Pop, not
        // a Quit. Conflating those would lose the menu state on every
        // accidental Esc.
        assert!(matches!(dispatch_help(Input::Esc), ScreenCommand::Pop));
    }

    #[test]
    fn help_backspace_pops() {
        assert!(matches!(
            dispatch_help(Input::Backspace),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn help_q_still_quits() {
        // Q remains a hard quit even from inside the help screen so the
        // controls reference doesn't trap a player who just wants out.
        assert!(matches!(
            dispatch_help(Input::Char('q')),
            ScreenCommand::Quit
        ));
        assert!(matches!(
            dispatch_help(Input::Ctrl('c')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn help_unrelated_keys_are_inert() {
        for input in [
            Input::Up,
            Input::Down,
            Input::Enter,
            Input::Char('x'),
            Input::Char('1'),
        ] {
            assert!(
                matches!(dispatch_help(input), ScreenCommand::None),
                "{input:?} should not transition from the help screen"
            );
        }
    }

    #[test]
    fn help_renders_into_test_backend() {
        // Prove the help body actually paints. We assert the screen
        // title plus a sentinel substring from one of the body lines —
        // good enough to catch a future regression that drops the
        // paragraph entirely without locking in incidental whitespace.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut help = HelpScreen;
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| help.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(found.contains(HelpScreen::TITLE));
        assert!(
            found.contains("Arrow keys"),
            "expected help body to mention 'Arrow keys'; buffer was:\n{found}"
        );
    }

    // ---- MapScreen -----------------------------------------------------

    /// Build a [`MapScreen`] using the same spawn coordinates the
    /// scaffold's `assets/game.toml` ships with. Centralised so a
    /// future spawn retune updates one place instead of every test.
    fn fresh_map_screen() -> MapScreen {
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

    // ---- NPCs and DialogScreen (Task 13d) ------------------------------

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

    /// Construct a Night Clerk dialog screen with an empty flag store
    /// for tests that need to drive the conversation directly.
    fn fresh_clerk_dialog() -> (DialogScreen, Rc<RefCell<FlagSet>>) {
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let clerk = MapScreen::NPCS
            .iter()
            .find(|n| n.name == "Night Clerk")
            .expect("night clerk in roster");
        let screen = DialogScreen::new(clerk, Rc::clone(&flags));
        (screen, flags)
    }

    #[test]
    fn dialog_starts_on_speaker_and_first_line() {
        let (screen, _flags) = fresh_clerk_dialog();
        assert_eq!(screen.speaker(), "Night Clerk");
        assert!(!screen.is_finished());
        assert_eq!(screen.state().current_node(), "greeting");
    }

    #[test]
    fn dialog_clerk_branch_is_gated_until_rumor_flag_set() {
        // Branching contract: the "Got a master key" choice must NOT
        // appear before the player asks about the murder, and MUST
        // appear after the rumor flag has been set.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        // Advance through both greeting lines so choices are visible.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        let initial = screen.current_choice_labels();
        assert!(
            !initial.iter().any(|t| t.contains("master key")),
            "master-key choice should be gated initially; saw {initial:?}"
        );

        // Set the flag directly to keep the test focused on the gate
        // rather than re-driving the whole conversation; the choose()
        // path is exercised by the round-trip test below.
        flags.borrow_mut().insert("heard_rumor".into());
        let unlocked = screen.current_choice_labels();
        assert!(
            unlocked.iter().any(|t| t.contains("master key")),
            "master-key choice should unlock once `heard_rumor` is set; saw {unlocked:?}"
        );
    }

    #[test]
    fn dialog_clerk_choose_rumor_then_key_sets_flags() {
        // End-to-end: drive the conversation through the rumor →
        // key handover branch and assert both flags persist on the
        // shared store.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        // Pump greeting lines.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // Highlight "I heard about the murder." (index 1) and pick it.
        screen.handle_input(&mut ctx, Input::Down);
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "rumor");
        assert!(flags.borrow().contains("heard_rumor"));
        // Pump rumor lines.
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // First choice on rumor is "What about that key?" — pick it.
        screen.handle_input(&mut ctx, Input::Enter);
        assert_eq!(screen.state().current_node(), "key_handed_over");
        assert!(flags.borrow().contains("has_key"));
    }

    #[test]
    fn dialog_esc_pops() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn dialog_q_quits() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Char('q')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn dialog_finished_then_enter_pops() {
        // Walk the linear Bellhop dialog to its terminal node and
        // confirm one more Enter pops the screen rather than
        // erroring.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let flags = Rc::new(RefCell::new(FlagSet::new()));
        let bellhop = MapScreen::NPCS
            .iter()
            .find(|n| n.name == "Bellhop")
            .expect("bellhop in roster");
        let mut screen = DialogScreen::new(bellhop, Rc::clone(&flags));
        // Two greeting lines, then advance past the last line into
        // the goto, then a final advance to land on the empty `end`
        // node and finish.
        for _ in 0..4 {
            screen.handle_input(&mut ctx, Input::Enter);
        }
        assert!(
            screen.is_finished(),
            "expected linear dialog to finish after 4 advances"
        );
        let cmd = screen.handle_input(&mut ctx, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn dialog_renders_into_test_backend() {
        // Headless render check: the speaker name appears in the
        // border and the first greeting line shows up in the body.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, _flags) = fresh_clerk_dialog();
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
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
            found.contains("Night Clerk"),
            "expected speaker name in dialog frame; buffer was:\n{found}"
        );
        assert!(
            found.contains("slouches"),
            "expected first greeting line in dialog body; buffer was:\n{found}"
        );
    }

    // ---- Items and InventoryScreen (Task 13e) -------------------------

    /// Walk the player from spawn onto the cell at `(target_x,
    /// target_y)` using a simple axis-aligned route. Used by the
    /// pickup tests to land on an item without re-deriving the
    /// movement sequence each time. Returns the live screen so the
    /// caller can keep driving it.
    fn walk_to(map: &mut MapScreen, ctx: &mut GameContext<'_>, tx: u16, ty: u16) {
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
    fn inventory_empty_renders_hint() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
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
            found.contains(InventoryScreen::EMPTY_HINT),
            "empty inventory must surface the empty-hint; buffer was:\n{found}"
        );
    }

    #[test]
    fn inventory_lists_collected_item_names() {
        // Stuff two known IDs into the shared store and confirm the
        // screen renders their canonical names from the catalog.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("matchbook".to_string());
        inv.borrow_mut().insert("brass_key".to_string());
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        let labels = screen.current_labels();
        assert_eq!(
            labels,
            vec!["Brass key".to_string(), "Matchbook".to_string()],
            "labels must follow catalog order, not insertion order"
        );
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw");
        let buf = term.backend().buffer().clone();
        let mut found = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                found.push_str(buf.cell((x, y)).expect("cell").symbol());
            }
            found.push('\n');
        }
        assert!(found.contains(InventoryScreen::TITLE));
        assert!(
            found.contains("Matchbook"),
            "Matchbook must render in the inventory list; buffer was:\n{found}"
        );
        assert!(
            found.contains("Brass key"),
            "Brass key must render in the inventory list; buffer was:\n{found}"
        );
    }

    #[test]
    fn inventory_unknown_id_is_skipped() {
        // Forward-compatibility: a stale ID from a future save must
        // not crash the screen — it just doesn't render.
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("ghost_item".to_string());
        let screen = InventoryScreen::new(Rc::clone(&inv));
        assert!(
            screen.current_labels().is_empty(),
            "unknown IDs must be silently ignored"
        );
    }

    #[test]
    fn inventory_esc_pops() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Esc),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn inventory_i_toggles_closed() {
        // The same key that opens the screen also closes it — muscle
        // memory shortcut every BBS inventory does.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Char('i')),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn inventory_quit_keys_quit() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        let mut screen = InventoryScreen::new(inv);
        for key in [Input::Char('q'), Input::Char('Q'), Input::Ctrl('c')] {
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} should quit the inventory screen"
            );
        }
    }

    // ---- Locked door (Task 13f) ---------------------------------------

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

    #[test]
    fn inventory_cursor_clamps() {
        // Selection cursor must not advance past the live item count
        // and must not underflow at zero.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("matchbook".to_string());
        inv.borrow_mut().insert("brass_key".to_string());
        let mut screen = InventoryScreen::new(Rc::clone(&inv));
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(screen.selected, 1, "cursor must clamp at last row");
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Up);
        }
        assert_eq!(screen.selected, 0, "cursor must clamp at top row");
    }
}
