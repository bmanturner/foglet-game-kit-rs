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
    load_context, load_dialog, parse_map, process_env, read_save, render_inventory_list,
    render_menu_list, resolve_save_path, write_atomic, ChoicePrompt, Dialog, DialogState,
    FeedbackLine, FlagSet, Game, GameConfig, GameContext, Input, InventoryList, Map, MenuList,
    PromptAction, PromptScreen, SavePathInputs, Screen, ScreenCommand, TileLegend,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

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
    /// the [`MapScreen::ITEMS`] catalog; ids without a catalog match are
    /// rendered silently as gone — see [`InventoryScreen::current_labels`].
    pub inventory: BTreeSet<String>,
    /// Coin balance carried into v1.1 for the night-clerk vendor scene
    /// (SPEC §9). `#[serde(default)]` so a v1 save written before the
    /// field existed deserialises cleanly with `cash == 0`; the New
    /// Game / load paths reset to [`PlayerSlot::STARTING_CASH`].
    #[serde(default)]
    pub cash: u32,
}

/// Mutable runtime fields that need to survive a quit/launch cycle and
/// are therefore shared between the screens that mutate them and the
/// `main` scope that writes the save on exit.
///
/// Cloned freely (each clone is three `Rc::clone` calls) so every
/// screen that needs read or write access holds its own handle. The
/// canonical handle lives in `main`, which uses [`Self::snapshot`] /
/// [`Self::apply`] to bridge to and from on-disk [`SaveState`].
#[derive(Clone, Default, Debug)]
pub struct SharedSlots {
    /// Narrative-flag store. Same Rc the [`DialogScreen`] borrows for
    /// `requires`-gated branches.
    pub flags: Rc<RefCell<FlagSet>>,
    /// Inventory id set. Same Rc the [`InventoryScreen`] reads to draw
    /// the player's pockets.
    pub inventory: Rc<RefCell<BTreeSet<String>>>,
    /// Player position + win latch. Pulled into a single Rc so a single
    /// `borrow_mut()` swap is enough to apply a loaded save without
    /// briefly observing a half-restored position.
    pub player: Rc<RefCell<PlayerSlot>>,
    /// Most recent player-facing feedback line — written by prompt
    /// callbacks (e.g. the Lost-and-Found Drawer in Task 10f) and read
    /// by the [`MapScreen`] renderer below the movement hint. Ephemeral
    /// state: not part of [`SaveState`], cleared on `reset` so a fresh
    /// run never opens with stale narration from a prior session.
    pub feedback: Rc<RefCell<Option<FeedbackLine>>>,
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
    }
}

/// First screen the player sees on launch.
///
/// Shows the configured title, a one-line tagline, and a "Press Enter"
/// prompt that leads into the main menu added in Task 13b. Esc / Q /
/// Ctrl-C still quit directly so a player who lands on the title screen
/// with no patience for menus has an immediate exit.
#[derive(Default)]
pub struct TitleScreen {
    /// Shared runtime state propagated into the [`MainMenuScreen`] when
    /// the player presses Enter. Held on the title so the slots loaded
    /// by `main` (Task 13h) survive the title → menu transition without
    /// a static.
    slots: SharedSlots,
}

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

    /// Build a title screen wired to the supplied shared slots.
    pub fn with_slots(slots: SharedSlots) -> Self {
        Self { slots }
    }
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
            Input::Enter => {
                ScreenCommand::Push(Box::new(MainMenuScreen::with_slots(self.slots.clone())))
            }
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
    fn activate(self, ctx: &GameContext<'_>, slots: &SharedSlots) -> ScreenCommand {
        let (sx, sy) = (ctx.config.game.start_x, ctx.config.game.start_y);
        match self {
            // New Game wipes the shared slots so a leftover loaded save
            // does not bleed in, then pushes a fresh map at the
            // configured spawn.
            Self::NewGame => {
                slots.reset(sx, sy);
                ScreenCommand::Push(Box::new(MapScreen::with_shared(sx, sy, slots.clone())))
            }
            // Continue keeps whatever state the slots already carry —
            // either the loaded save (if `main` populated them at
            // startup) or the same default-zero state New Game would
            // otherwise have built. The walkability fallback inside
            // `MapScreen::with_shared` keeps an empty-default Continue
            // safe even when no save was loaded.
            Self::Continue => {
                ScreenCommand::Push(Box::new(MapScreen::with_shared(sx, sy, slots.clone())))
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
    /// Shared runtime state. Routed into `MainMenuItem::activate` so
    /// "New Game" can reset the slots and "Continue" can pass the
    /// already-populated slots straight to a fresh [`MapScreen`].
    slots: SharedSlots,
}

impl MainMenuScreen {
    /// Title rendered on the menu's bordered block.
    pub const TITLE: &'static str = "Main Menu";
    /// Order of menu items. Kept aligned with [`MainMenuItem`] so the
    /// row index can be cast straight to an item.
    pub const ITEMS: [&'static str; 4] = ["New Game", "Continue", "Help", "Quit"];

    /// Build a menu with the cursor on `New Game` and freshly defaulted
    /// shared slots. Used by tests and by callers that don't need to
    /// participate in the save/load handshake. The non-test entry point
    /// uses [`Self::with_slots`] so the slots threaded through `main`
    /// reach the activate path.
    pub fn new() -> Self {
        Self::with_slots(SharedSlots::default())
    }

    /// Build a menu wired to the supplied shared slots. The slots are
    /// the bridge between the disk save (loaded by `main`) and the map
    /// screen the menu pushes — see [`MainMenuItem::activate`].
    pub fn with_slots(slots: SharedSlots) -> Self {
        Self {
            items: Self::ITEMS.iter().map(|s| s.to_string()).collect(),
            selected: 0,
            slots,
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
                .map(|item| item.activate(ctx, &self.slots))
                .unwrap_or(ScreenCommand::None),
            // Per-item shortcuts mirror the first letter of each label.
            // `Q` doubles as a generic quit affordance — both meanings
            // resolve to `ScreenCommand::Quit` so there's no ambiguity.
            Input::Char('n') | Input::Char('N') => MainMenuItem::NewGame.activate(ctx, &self.slots),
            Input::Char('c') | Input::Char('C') => {
                MainMenuItem::Continue.activate(ctx, &self.slots)
            }
            Input::Char('h') | Input::Char('H') => MainMenuItem::Help.activate(ctx, &self.slots),
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
    /// Save-bearing runtime state — player position, narrative flags,
    /// inventory, win latch. Shared with `main` (which writes the save
    /// on exit), with [`DialogScreen`] (`flags`), and with
    /// [`InventoryScreen`] (`inventory`). The single Rc bundle replaces
    /// what used to be three separate fields so Task 13h's save/load
    /// path manipulates the same handles the screens read.
    slots: SharedSlots,
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
    /// map cell — there is no glyph or coordinate to record.
    pub const ROOM_7_KEY_ID: &'static str = "room_7_key";

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
        let screen = Self { map, slots };
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
        if let Some(item) = Self::item_at(target_x, target_y) {
            self.slots
                .inventory
                .borrow_mut()
                .insert(item.id.to_string());
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

/// Player-facing narration shown above the Lost-and-Found Drawer loot
/// prompt (SPEC_v1_1.md §9 step 1).
///
/// Stored as two body lines because `ChoicePrompt::body` appends one
/// logical line per call and treats them as separate paragraphs the
/// renderer wraps independently. Authoring the text as a constant keeps
/// it close to the prompt builder and lets the test assert the exact
/// string the player sees without re-typing it.
pub const LOST_AND_FOUND_BODY: [&str; 2] = [
    "Behind the desk, the lost-and-found drawer sticks halfway open.",
    "Inside: a tarnished room key tagged \"7\", a cracked matchbook, and a receipt from last night.",
];

/// Stable id returned by [`lost_and_found_drawer_prompt`] when the
/// player picks one of its four options (SPEC_v1_1.md §9 step 2).
///
/// Carries semantics, not display strings: copy edits to the prompt
/// labels MUST NOT cascade into the action handler. Task 10c wires
/// each variant into the corresponding state mutation; Task 10d adds
/// the disabled-`(K)` branch on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAndFoundChoice {
    /// `(K)` — move the Room 7 key into the player's inventory.
    TakeRoom7Key,
    /// `(M)` — pocket the cracked matchbook (shares the catalog id with
    /// the lobby map's matchbook so the two pickup paths can't duplicate
    /// the same keepsake).
    PocketMatchbook,
    /// `(R)` — set the `receipt_read` narrative flag and surface the
    /// receipt body as feedback.
    ReadReceipt,
    /// `(L)` — leave the drawer untouched and exit the prompt.
    Leave,
}

/// Disabled-reason string surfaced when the player presses `(K)` after
/// the Room 7 key is already in their inventory (SPEC_v1_1.md §9 step
/// 3). Centralised so the prompt builder, the runtime feedback line,
/// and the test that pins the contract all reference one source — copy
/// edits to the player-facing reason are a one-line change here.
pub const ROOM_7_KEY_ALREADY_HELD_REASON: &str = "already in inventory";

/// Build the Lost-and-Found Drawer loot prompt with every choice
/// enabled (SPEC_v1_1.md §9 step 2).
///
/// Convenience wrapper around [`lost_and_found_drawer_prompt_with_state`]
/// for the common "fresh-state" case used by Task 10b's data tests and
/// Task 10c's action-handler tests, where the player has not yet picked
/// anything up. Production scene wiring (Task 10f) goes through the
/// state-aware builder so `(K)` greys out automatically once the key is
/// in inventory.
pub fn lost_and_found_drawer_prompt() -> ChoicePrompt<LostAndFoundChoice> {
    lost_and_found_drawer_prompt_with_state(false)
}

/// Build the Lost-and-Found Drawer loot prompt with the disabled-`(K)`
/// branch wired in (SPEC_v1_1.md §9 step 3, Task 10d).
///
/// `has_room_7_key` is a plain bool rather than a `&SharedSlots` borrow
/// so the function stays trivially pure: the caller computes the
/// inventory predicate once at scene-entry and passes it in. That keeps
/// the prompt builder testable without spinning up the runtime, and
/// avoids tangling the renderer with `RefCell` borrow scheduling when
/// the same scene later wants to redraw after a successful pickup
/// flips the predicate.
///
/// `disabled_if` attaches to the **most recently added choice**
/// (`crates/foglet_game/src/prompt.rs:651`), so the call sits
/// immediately after `(K)`. The reducer in `foglet_game` then surfaces
/// the press as [`foglet_game::PromptAction::Disabled`] with the
/// SPEC-mandated reason, ensuring the disabled branch can never reach
/// [`apply_lost_and_found_choice`] and double-insert the key.
pub fn lost_and_found_drawer_prompt_with_state(
    has_room_7_key: bool,
) -> ChoicePrompt<LostAndFoundChoice> {
    let mut prompt = ChoicePrompt::new();
    for line in LOST_AND_FOUND_BODY {
        prompt = prompt.body(line);
    }
    prompt
        .choice('K', LostAndFoundChoice::TakeRoom7Key, "Take the Room 7 key")
        .disabled_if(has_room_7_key, ROOM_7_KEY_ALREADY_HELD_REASON)
        .choice(
            'M',
            LostAndFoundChoice::PocketMatchbook,
            "Pocket the matchbook",
        )
        .choice('R', LostAndFoundChoice::ReadReceipt, "Read the receipt")
        .choice('L', LostAndFoundChoice::Leave, "Leave it alone")
}

/// Result of applying a [`LostAndFoundChoice`] against shared player
/// state (SPEC_v1_1.md §9 step 5).
///
/// Mirrors the prompt's variants one-for-one so callers can branch on
/// what just happened without re-deriving it from the choice. Task 10e
/// pairs each variant with the player-facing feedback line; Task 10d
/// will introduce a separate disabled-`(K)` branch (the prompt itself
/// will return [`foglet_game::PromptAction::Disabled`] in that case, so
/// this enum stays narrowly scoped to *successful* applications).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAndFoundOutcome {
    /// Room 7 key was added to the player's inventory.
    TookRoom7Key,
    /// The cracked matchbook was added to the player's inventory.
    /// Inventory is a `BTreeSet`, so pocketing twice is idempotent —
    /// the player never ends the scene with two matchbooks even if the
    /// lobby pickup also fired.
    PocketedMatchbook,
    /// The `receipt_read` narrative flag is now set.
    ReadReceipt,
    /// Player walked away; no state mutated.
    Left,
}

/// Apply the player's [`LostAndFoundChoice`] to shared runtime state
/// (SPEC_v1_1.md §9 steps 2–3, 5).
///
/// Mutates `slots` directly because every consumer in the example
/// already holds an [`Rc<RefCell<_>>`] handle, so threading a `&mut`
/// view through the call site would just shadow the existing
/// borrow-checked sharing. Returns the typed [`LostAndFoundOutcome`]
/// so the caller (a future scene controller) can drive the post-action
/// feedback line in Task 10e without re-matching on the input choice.
///
/// Task 10c keeps this handler unconditional: pressing `(K)` always
/// inserts the Room 7 key. Task 10d wraps the prompt with the
/// `disabled_if(...)` branch so the disabled press never reaches this
/// function in the first place.
pub fn apply_lost_and_found_choice(
    slots: &SharedSlots,
    choice: LostAndFoundChoice,
) -> LostAndFoundOutcome {
    match choice {
        LostAndFoundChoice::TakeRoom7Key => {
            slots
                .inventory
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            LostAndFoundOutcome::TookRoom7Key
        }
        LostAndFoundChoice::PocketMatchbook => {
            slots
                .inventory
                .borrow_mut()
                .insert(MapScreen::MATCHBOOK_ID.to_string());
            LostAndFoundOutcome::PocketedMatchbook
        }
        LostAndFoundChoice::ReadReceipt => {
            slots
                .flags
                .borrow_mut()
                .insert(MapScreen::RECEIPT_READ_FLAG.to_string());
            LostAndFoundOutcome::ReadReceipt
        }
        LostAndFoundChoice::Leave => LostAndFoundOutcome::Left,
    }
}

/// Player-facing line emitted after a successful `(K)` press
/// (SPEC_v1_1.md §9 step 5). Centralised so the renderer, the test that
/// pins the contract, and any future transcript log share one source.
pub const TOOK_ROOM_7_KEY_FEEDBACK: &str = "Moved Room 7 key to inventory.";

/// Player-facing line emitted after pocketing the cracked matchbook.
/// Mirrors the SPEC §9 step 5 sample format ("Moved X to inventory.")
/// while staying narratively distinct so the player can tell which
/// `(M)` press just registered.
pub const POCKETED_MATCHBOOK_FEEDBACK: &str = "Pocketed the cracked matchbook.";

/// Player-facing line emitted when the player reads the receipt.
///
/// SPEC §9 step 5 only pins the post-action *shape*, not the receipt's
/// wording, so the body lives here as a deliberate copy hook: the
/// receipt is what unlocks the `receipt_read` flag's downstream value
/// (J.M. initials, 23:47 timestamp, cash payment) without forcing the
/// player to memorise it from the prompt.
pub const READ_RECEIPT_FEEDBACK: &str =
    "Receipt: Room 7, paid cash at 23:47 last night. Signed \"J.M.\"";

/// Map a [`LostAndFoundOutcome`] to the player-facing feedback line the
/// scene should display next (SPEC_v1_1.md §9 step 5, Task 10e).
///
/// Returns `None` for [`LostAndFoundOutcome::Left`] because walking away
/// is a deliberate "no narration" path: we do not want a confirmation
/// message implying the drawer remembered the player's hesitation.
/// Every other variant emits a `FeedbackLine` so the caller can drop it
/// straight into the runtime feedback slot without re-matching the
/// outcome.
///
/// All three messages use [`FeedbackLine::info`] rather than `success`:
/// the loot prompt is descriptive narration, not a transactional win,
/// and the SPEC §9 step 5 sample shows no leading marker. The
/// `success` style is reserved for the night-clerk vendor (Task 11)
/// where a coffee purchase reads as an unambiguous positive outcome.
/// Build the [`PromptScreen`] the lobby pushes when the player searches
/// the Lost-and-Found Drawer (SPEC §9, Task 10f).
///
/// Centralising the wiring here keeps the [`MapScreen`] input handler a
/// one-liner and lets tests exercise the same factory the runtime uses,
/// so a future regression in the callback's outcome→`ScreenCommand`
/// mapping cannot hide behind a private closure literal.
///
/// The closure clones the supplied [`SharedSlots`] handle so it can
/// outlive the synchronous `handle_input` call: `PromptScreen` keeps the
/// callback alive across frames, and the captured slots reach into the
/// same `RefCell`s the map screen reads — there is only ever one logical
/// inventory/feedback pair.
///
/// Outcome routing:
///
/// - `Selected(_)` applies the choice via [`apply_lost_and_found_choice`],
///   stores the matching [`FeedbackLine`] (if any) in
///   [`SharedSlots::feedback`], and pops back to the lobby. The disabled
///   branch is `(K)`-specific and never reaches `Selected`.
/// - `Disabled { reason, .. }` surfaces the reason as an error feedback
///   line *without* popping — the player is told why the press was
///   rejected and stays in the prompt to make a different choice.
/// - `Cancelled` (Esc) and any-key fall-through (`None`) follow the
///   SPEC §4.4 cancellation contract: pop the prompt, leave state
///   untouched.
pub fn lost_and_found_drawer_screen(slots: SharedSlots) -> PromptScreen<LostAndFoundChoice> {
    let has_room_7_key = slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID);
    let prompt = lost_and_found_drawer_prompt_with_state(has_room_7_key).cancellable(true);
    let callback_slots = slots;
    PromptScreen::new(prompt, move |action| match action {
        PromptAction::Selected(choice) => {
            let outcome = apply_lost_and_found_choice(&callback_slots, choice);
            if let Some(line) = lost_and_found_feedback(outcome) {
                *callback_slots.feedback.borrow_mut() = Some(line);
            }
            ScreenCommand::Pop
        }
        PromptAction::Disabled { reason, .. } => {
            // Disabled hotkeys (currently only `(K)` once the Room 7
            // key is held) keep the prompt open — the player gets an
            // error line explaining the rejection and can pick a
            // different choice without re-opening the drawer.
            if let Some(reason) = reason {
                *callback_slots.feedback.borrow_mut() = Some(FeedbackLine::error(reason));
            }
            ScreenCommand::None
        }
        PromptAction::Cancelled => ScreenCommand::Pop,
        PromptAction::None | PromptAction::ConfirmRequested(_) => ScreenCommand::None,
    })
    .modal()
}

pub fn lost_and_found_feedback(outcome: LostAndFoundOutcome) -> Option<FeedbackLine> {
    match outcome {
        LostAndFoundOutcome::TookRoom7Key => Some(FeedbackLine::info(TOOK_ROOM_7_KEY_FEEDBACK)),
        LostAndFoundOutcome::PocketedMatchbook => {
            Some(FeedbackLine::info(POCKETED_MATCHBOOK_FEEDBACK))
        }
        LostAndFoundOutcome::ReadReceipt => Some(FeedbackLine::info(READ_RECEIPT_FEEDBACK)),
        LostAndFoundOutcome::Left => None,
    }
}

/// Player-facing narration shown above the night-clerk vendor prompt
/// (SPEC_v1_1.md §9 step 4).
///
/// Two body lines mirror the SPEC's two-paragraph framing so the
/// renderer can wrap them independently — the first sets the scene,
/// the second drops the pricing-fueled barb. Authoring the lines as a
/// constant keeps the wording version-controlled and lets the data
/// test below pin it byte-for-byte without re-typing the SPEC quote.
pub const NIGHT_CLERK_VENDOR_BODY: [&str; 2] = [
    "The night clerk drums his fingers beside a locked cigar box.",
    "\"Evidence costs extra after midnight.\"",
];

/// Stable id returned by [`night_clerk_vendor_prompt`] when the player
/// picks one of its three options (SPEC_v1_1.md §9 step 4).
///
/// Carries semantics, not display strings, so Task 11b's dynamic
/// label refactor (price/cash interpolation) and Task 11c's transaction
/// handler can both branch on the variant without re-parsing the
/// rendered label. The enum mirrors SPEC §9 step 4's three-row layout
/// one-for-one — there is intentionally no separate "leave" variant
/// because [`Self::NoThanks`] already serves that role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightClerkVendorChoice {
    /// `(B)` — pay the coffee price (Task 11c will decrement cash by
    /// [`COFFEE_PRICE`] gold and emit a success feedback line).
    BuyCoffee,
    /// `(T)` — tip for a rumor; Task 11d disables this row when the
    /// player has fewer than [`RUMOR_TIP_PRICE`] gold and surfaces the
    /// `need 50g` reason.
    TipForRumor,
    /// `(N)` — close the prompt without spending anything.
    NoThanks,
}

/// Cost of the night clerk's black coffee, in gold pieces
/// (SPEC_v1_1.md §9 step 4). Centralised so Task 11b's dynamic label
/// and Task 11c's transaction handler share one source of truth — a
/// future copy-edit to "30g" only changes the constant, never the
/// branching logic.
pub const COFFEE_PRICE: u32 = 25;

/// Cost of the rumor tip, in gold pieces (SPEC_v1_1.md §9 step 4).
/// Pairs with [`PlayerSlot::STARTING_CASH`] (40g) so the proof scene
/// always boots into the disabled-`(T)` branch Task 11d wires up.
pub const RUMOR_TIP_PRICE: u32 = 50;

/// Disabled-row reason shown next to `(T) Tip the clerk for a rumor`
/// when the player's wallet is below [`RUMOR_TIP_PRICE`]
/// (SPEC_v1_1.md §9 step 4: "Tipping is disabled when the player lacks
/// enough cash and shows `need 50g`"). Centralised so the prompt
/// builder, the disabled-press regression test, and any future copy
/// edit share one source of truth — the SPEC pins the wording verbatim,
/// so a future bump of `RUMOR_TIP_PRICE` deliberately invalidates this
/// constant rather than silently desyncing the player-facing string.
pub const NEED_RUMOR_TIP_REASON: &str = "need 50g";

/// Format the priced-row label for the night-clerk vendor prompt
/// (SPEC_v1_1.md §9 step 4, Task 11b).
///
/// The SPEC pins the shape `"<action>: <price>g | You have: <cash>g"`
/// — a single source of truth keeps `Buy a black coffee` and
/// `Tip the clerk for a rumor` in lockstep so a copy-edit to the
/// separator (`|`) only touches one place. Returning `String` (rather
/// than `Cow<'static, str>`) is intentional: the cash component is
/// dynamic on every render, so the allocation is unavoidable.
fn priced_vendor_label(action: &str, price: u32, cash: u32) -> String {
    format!("{action}: {price}g | You have: {cash}g")
}

/// Build the night-clerk vendor prompt with dynamic price/cash hints
/// (SPEC_v1_1.md §9 step 4, Task 11b).
///
/// `cash` is the player's current gold balance, which the priced rows
/// (`(B)` and `(T)`) splice into their labels in the SPEC's
/// `"<action>: <price>g | You have: <cash>g"` shape. Re-calling this
/// function after a transaction is the supported path for refreshing
/// the labels — the prompt itself does not retain a reference to the
/// player's wallet, which keeps the data model a value type and lets
/// every reducer test pin a deterministic balance without plumbing
/// shared state through `Rc<RefCell<_>>`.
///
/// `(N) No thanks` keeps a static label because the SPEC's reference
/// block deliberately omits a price annotation for the leave path —
/// adding one would imply a cost the row does not actually charge.
///
/// Task 11d gates the `(T)` row with `disabled_if(cash < RUMOR_TIP_PRICE,
/// NEED_RUMOR_TIP_REASON)` so the disabled-`(T)` branch fires whenever
/// the player can't actually afford the tip. The reducer then surfaces
/// the press as [`foglet_game::PromptAction::Disabled`], which means a
/// disabled tip can never reach [`apply_night_clerk_vendor_choice`] and
/// silently decrement cash. `(B)` stays unconditionally enabled — the
/// 25g coffee always sits within reach of the 40g
/// [`PlayerSlot::STARTING_CASH`] floor, so a separate gate would just be
/// unreachable code.
///
/// The choice ordering matches SPEC §9 step 4 verbatim (`B`, `T`, `N`)
/// so the rendered prompt reads top-to-bottom in the same order an
/// operator scanning the SPEC's reference block would expect.
pub fn night_clerk_vendor_prompt(cash: u32) -> ChoicePrompt<NightClerkVendorChoice> {
    let mut prompt = ChoicePrompt::new();
    for line in NIGHT_CLERK_VENDOR_BODY {
        prompt = prompt.body(line);
    }
    prompt
        .choice(
            'B',
            NightClerkVendorChoice::BuyCoffee,
            priced_vendor_label("Buy a black coffee", COFFEE_PRICE, cash),
        )
        .choice(
            'T',
            NightClerkVendorChoice::TipForRumor,
            priced_vendor_label("Tip the clerk for a rumor", RUMOR_TIP_PRICE, cash),
        )
        // `disabled_if` attaches to the most-recently-added choice, so
        // this call must sit immediately after the `(T)` row. The
        // reducer in `foglet_game` then routes the disabled press to
        // `PromptAction::Disabled` with NEED_RUMOR_TIP_REASON — the
        // apply handler never sees a TipForRumor it couldn't afford.
        .disabled_if(cash < RUMOR_TIP_PRICE, NEED_RUMOR_TIP_REASON)
        .choice('N', NightClerkVendorChoice::NoThanks, "No thanks")
}

/// Result of applying a [`NightClerkVendorChoice`] against shared
/// player state (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Mirrors the prompt's variants one-for-one *only as they get wired
/// up* — Task 11c lands [`Self::BoughtCoffee`], Task 11e adds the
/// no-thanks path, and Task 11d will route the `(T)` row through a
/// disabled-prompt branch (so it never reaches this enum). Keeping the
/// enum narrowly scoped to *successful, state-mutating* outcomes mirrors
/// [`LostAndFoundOutcome`] and lets the feedback helper return a single
/// [`FeedbackLine`] without juggling `Option<Option<…>>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightClerkVendorOutcome {
    /// `(B)` succeeded: the coffee price was deducted from the player's
    /// wallet and the success feedback line should be surfaced. Task
    /// 11c is the *only* path that produces this variant — Task 11f's
    /// any-key continuation pipes it through unchanged.
    BoughtCoffee,
}

/// Player-facing line emitted after a successful `(B)` press
/// (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Centralised so the renderer, the test that pins the contract, and
/// any future transcript log share one source. Wording bakes in the
/// price so the player sees the deduction confirmed in the same line
/// — handy when the wallet field is off-screen during the prompt
/// dismissal.
pub const BOUGHT_COFFEE_FEEDBACK: &str = "Bought a black coffee for 25g.";

/// Apply the player's [`NightClerkVendorChoice`] to shared runtime state
/// (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Returns `Some(outcome)` for the *implemented* mutating paths and
/// `None` for the still-unscoped rows so future tasks (11d, 11e) can
/// fill in their behaviour without rewriting this signature. Task 11c
/// lands `BuyCoffee`: cash decrements by [`COFFEE_PRICE`] and the caller
/// surfaces the success feedback. The other variants intentionally
/// return `None` for now — Task 11d disables `(T)` upstream (so it
/// never reaches this handler) and Task 11e wires `(N)` to a
/// no-state-change exit.
///
/// Mutates `slots` directly to mirror [`apply_lost_and_found_choice`]'s
/// shape — every consumer already holds [`SharedSlots`] handles, so a
/// `&mut`-style API would just shadow the existing `RefCell` sharing.
///
/// Cash is decremented with plain `-=` (not `saturating_sub`) so an
/// unexpected underflow is a loud panic in debug builds rather than a
/// silent wrap to zero. Task 11d's disabled-`(T)` branch is the
/// gatekeeper for tip affordability; Task 11c's coffee at 25g is
/// always within reach of the 40g [`PlayerSlot::STARTING_CASH`] floor,
/// so no separate `(B)` gate exists.
pub fn apply_night_clerk_vendor_choice(
    slots: &SharedSlots,
    choice: NightClerkVendorChoice,
) -> Option<NightClerkVendorOutcome> {
    match choice {
        NightClerkVendorChoice::BuyCoffee => {
            let mut player = slots.player.borrow_mut();
            player.cash -= COFFEE_PRICE;
            Some(NightClerkVendorOutcome::BoughtCoffee)
        }
        // Task 11d gates `(T)` at the prompt layer (disabled row), so
        // it should never reach the apply handler. Task 11e will add
        // the `(N)` no-state-change exit. Returning `None` keeps the
        // match exhaustive without pretending to mutate state.
        NightClerkVendorChoice::TipForRumor | NightClerkVendorChoice::NoThanks => None,
    }
}

/// Map a [`NightClerkVendorOutcome`] to the player-facing feedback line
/// the scene should display next (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Uses [`FeedbackLine::success`] (not `info`) because the coffee
/// purchase is an unambiguous transactional win — the SPEC explicitly
/// reserves the success register for the vendor scene to distinguish
/// it from the Lost-and-Found Drawer's descriptive narration. Returns
/// `Option` so the type stays parallel with
/// [`lost_and_found_feedback`], leaving room for a future no-narration
/// variant without re-shaping the call sites.
pub fn night_clerk_vendor_feedback(outcome: NightClerkVendorOutcome) -> Option<FeedbackLine> {
    match outcome {
        NightClerkVendorOutcome::BoughtCoffee => {
            Some(FeedbackLine::success(BOUGHT_COFFEE_FEEDBACK))
        }
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

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Cardinal movement, both arrow keys and vi-style aliases.
            // The screen does not announce a "blocked" state when a
            // move fails — the lack of motion is the feedback.
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.try_move(0, -1);
                self.maybe_win_command()
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.try_move(0, 1);
                self.maybe_win_command()
            }
            Input::Left | Input::Char('h') => {
                self.try_move(-1, 0);
                self.maybe_win_command()
            }
            Input::Right | Input::Char('l') | Input::Char('L') => {
                self.try_move(1, 0);
                self.maybe_win_command()
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
            // Search affordance (Task 10f). When the player stands next
            // to the Lost-and-Found Drawer, `x`/`X` opens the SPEC §9
            // loot prompt; everywhere else the key is inert (silent
            // rejection, same model as bumping a wall). Keeps the prompt
            // reachable from normal lobby play without bolting it onto
            // a debug menu.
            Input::Char('x') | Input::Char('X') => {
                if self.nearby_lost_and_found() {
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
}

impl MapScreen {
    /// One-line movement hint shown directly below the map block.
    /// Pulled out as a constant so tests can assert it appears in the
    /// rendered buffer without binding to the precise wording.
    pub const HINT_LINE: &'static str =
        "Move: arrows/hjkl  Talk: Enter  Search: X  Inv: I  Back: Esc  Quit: Q";
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

/// Terminal "you win" modal pushed when the player solves the case
/// (Task 13g).
///
/// Reaching the win modal requires the player to have collected the
/// brass key (otherwise Room 4 is unreachable) and to have asked the
/// Night Clerk about the murder (otherwise the win flag is unset). In
/// other words: the modal can only appear if the player has actually
/// played through the SPEC §13 acceptance loop.
///
/// The screen is intentionally terminal — any keystroke quits the
/// runtime. A future Task 13h save loop can introduce a "play again"
/// affordance; for the 13g acceptance fixture, "Press any key to quit"
/// is the simplest faithful end-state.
#[derive(Debug, Default)]
pub struct WinScreen;

impl WinScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Case Closed";
    /// Body lines for the modal. Stored as `&'static str` so the screen
    /// is allocation-free per frame and tests can assert exact strings.
    pub const LINES: &'static [&'static str] = &[
        "You found the smoking gun. Lipstick on the wall spells out a name.",
        "",
        "The Night Clerk shrugs. 'Knew it'd be one of 'em. Lock up on your way out.'",
        "",
        "[Any key] quit",
    ];
}

impl Screen for WinScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre the modal. Width matches the longest line plus border
        // padding; height is the line count plus two for the border so
        // every line is visible on an exactly-80x24 terminal.
        let area = centred_rect(72, (Self::LINES.len() as u16) + 2, frame.area());
        let lines: Vec<Line<'_>> = Self::LINES.iter().map(|s| Line::from(*s)).collect();
        let widget = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
        // Any key quits — the modal is the end of the game.
        ScreenCommand::Quit
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

    // Resolve where this user's save lives and (if a previous run wrote
    // one) load it before the runtime takes over the terminal. Doing
    // both before `Game::run` means a malformed save surfaces as a
    // clean-error exit on stderr instead of a corrupted post-TUI scroll.
    let save_path = resolve_save_path(
        &SavePathInputs {
            slug: &config.game.slug,
            strategy: config.save.strategy,
            context: &foglet,
            cli_override: None,
        },
        process_env,
    )?;
    let slots = SharedSlots::default();
    if let Some(path) = save_path.as_deref() {
        if let Some(loaded) = read_save::<SaveState>(path)? {
            slots.apply(loaded);
        }
    }

    Game::new(config.game.title.clone())
        .min_size(config.game.min_width, config.game.min_height)
        .with_config(config)
        .with_foglet_context(foglet)
        .push_screen(Box::new(TitleScreen::with_slots(slots.clone())))
        .run()?;

    // Persist on clean exit. The runtime returns Ok only when a screen
    // emitted Quit (or popped to empty), so reaching this line means
    // the player has finished a session and the slot mutations on
    // `slots` are the canonical state worth keeping. Errors propagate
    // *after* the terminal has been restored — `Game::run` already
    // tore the guard down — so the operator sees a clean message
    // rather than a scrambled one.
    if let Some(path) = save_path {
        write_atomic(&path, &slots.snapshot())?;
    }
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
        TitleScreen::default().handle_input(&mut ctx, input)
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

    // ---- Win condition (Task 13g) -------------------------------------

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

    #[test]
    fn win_flag_matches_dialog_yaml() {
        // Renaming the flag in either place would silently un-gate the
        // win condition. Drive the Night Clerk's rumor branch and
        // confirm the flag the dialog sets is the same one the map
        // checks.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let (mut screen, flags) = fresh_clerk_dialog();
        screen.handle_input(&mut ctx, Input::Enter);
        screen.handle_input(&mut ctx, Input::Enter);
        // Highlight "I heard about the murder." (index 1) and pick it.
        screen.handle_input(&mut ctx, Input::Down);
        screen.handle_input(&mut ctx, Input::Enter);
        assert!(
            flags.borrow().contains(MapScreen::WIN_FLAG),
            "Night Clerk rumor branch must set the WIN_FLAG; saw {:?}",
            flags.borrow()
        );
    }

    #[test]
    fn win_screen_quits_on_any_key() {
        // The terminal modal must quit on every reasonable keystroke.
        // Iterating a representative cross-section is enough to lock
        // in the contract without enumerating every Input variant.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for key in [
            Input::Enter,
            Input::Esc,
            Input::Char('q'),
            Input::Char('x'),
            Input::Up,
            Input::Backspace,
        ] {
            let mut screen = WinScreen;
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} must quit the win screen"
            );
        }
    }

    #[test]
    fn win_screen_renders_into_test_backend() {
        // Sanity-check that the modal actually paints. We assert the
        // title and a sentinel substring of the body so a future
        // regression that drops the paragraph entirely surfaces here.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = WinScreen;
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
        assert!(found.contains(WinScreen::TITLE));
        assert!(
            found.contains("smoking gun"),
            "expected win body in render; buffer was:\n{found}"
        );
    }

    // ---- SaveState / SharedSlots (Task 13h) ----------------------------

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

    #[test]
    fn main_menu_new_game_resets_slots_before_pushing_map() {
        // "New Game" wipes whatever was in the slots so a stale loaded
        // save cannot leak items or flags into the new run.
        let slots = SharedSlots::default();
        slots.flags.borrow_mut().insert("heard_rumor".into());
        slots.inventory.borrow_mut().insert("brass_key".into());
        let mut menu = MainMenuScreen::with_slots(slots.clone());
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = menu.handle_input(&mut ctx, Input::Char('n'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        assert!(slots.flags.borrow().is_empty());
        assert!(slots.inventory.borrow().is_empty());
        let p = slots.player.borrow();
        assert_eq!((p.x, p.y), (cfg.game.start_x, cfg.game.start_y));
    }

    #[test]
    fn main_menu_continue_preserves_slots() {
        // "Continue" must not touch the slots — they came from the
        // loaded save and the map screen reads them on construction.
        let slots = SharedSlots::default();
        slots.flags.borrow_mut().insert("heard_rumor".into());
        slots.inventory.borrow_mut().insert("brass_key".into());
        slots.player.borrow_mut().x = 39;
        slots.player.borrow_mut().y = 5;
        let mut menu = MainMenuScreen::with_slots(slots.clone());
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = menu.handle_input(&mut ctx, Input::Char('c'));
        assert!(matches!(cmd, ScreenCommand::Push(_)));
        assert!(slots.flags.borrow().contains("heard_rumor"));
        assert!(slots.inventory.borrow().contains("brass_key"));
        assert_eq!(slots.player.borrow().x, 39);
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

    // ---- Lost-and-Found Drawer prompt (SPEC §9 Task 10b) -------------

    #[test]
    fn lost_and_found_prompt_exposes_spec_choices_and_body() {
        // SPEC §9 step 2 nails down the prompt's four hotkeys, their
        // labels, and the narration above them. Asserting against the
        // typed prompt (rather than the rendered buffer) catches drift
        // in the data the action handler will read in Task 10c, even
        // before the renderer hooks the prompt into a Screen.
        use foglet_game::PromptKey;

        let prompt = lost_and_found_drawer_prompt();

        assert_eq!(
            prompt.body.as_slice(),
            &LOST_AND_FOUND_BODY[..],
            "narration must match SPEC §9 step 1 verbatim"
        );

        let expected: &[(char, &str, LostAndFoundChoice)] = &[
            ('k', "Take the Room 7 key", LostAndFoundChoice::TakeRoom7Key),
            (
                'm',
                "Pocket the matchbook",
                LostAndFoundChoice::PocketMatchbook,
            ),
            ('r', "Read the receipt", LostAndFoundChoice::ReadReceipt),
            ('l', "Leave it alone", LostAndFoundChoice::Leave),
        ];
        assert_eq!(
            prompt.choices.len(),
            expected.len(),
            "Lost-and-Found Drawer must expose the four SPEC §9 choices"
        );
        for (choice, (key, label, value)) in prompt.choices.iter().zip(expected) {
            assert_eq!(
                choice.key,
                PromptKey::char(*key),
                "hotkey for {label:?} drifted from SPEC §9"
            );
            assert_eq!(choice.label, *label, "label for {key} drifted from SPEC §9");
            assert_eq!(choice.value, *value, "value for {label:?} drifted");
            assert!(
                choice.enabled,
                "Task 10b ships every choice enabled; Task 10d adds the disabled-(K) branch"
            );
        }
    }

    #[test]
    fn lost_and_found_prompt_renders_body_and_hotkeys() {
        // Drive the prompt through Ratatui's `TestBackend` so the test
        // asserts against the same buffer semantics the live runtime
        // uses (SPEC §6 deterministic-render contract). A 70x10 area
        // gives the body two rows and leaves room for the four choice
        // rows + label without forcing the renderer into modal mode.
        let prompt = lost_and_found_drawer_prompt();
        let backend = TestBackend::new(70, 10);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");

        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..10 {
            for x in 0..70 {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        // SPEC §9 step 1 narration — the wrapped fragment "lost-and-found
        // drawer" survives the renderer's word-wrap regardless of where
        // the second body line breaks at 70 columns.
        assert!(
            rendered.contains("lost-and-found drawer"),
            "rendered prompt missing SPEC §9 narration; got:\n{rendered}"
        );
        // Each choice row must surface its `(X)` hotkey marker plus the
        // SPEC §9 label so monochrome terminals stay legible.
        for marker in ["(K)", "(M)", "(R)", "(L)"] {
            assert!(
                rendered.contains(marker),
                "missing hotkey marker {marker} in rendered prompt:\n{rendered}"
            );
        }
        for label in [
            "Take the Room 7 key",
            "Pocket the matchbook",
            "Read the receipt",
            "Leave it alone",
        ] {
            assert!(
                rendered.contains(label),
                "missing label {label:?} in rendered prompt:\n{rendered}"
            );
        }
    }

    // ---- Night-clerk vendor prompt data (SPEC §9 Task 11a) -----------

    #[test]
    fn night_clerk_vendor_prompt_exposes_spec_choices_and_body() {
        // SPEC §9 step 4 pins the prompt's three hotkeys, their labels,
        // and the two-line narration above them. Asserting against the
        // typed prompt (rather than the rendered buffer) catches drift
        // in the data Task 11b will read for dynamic labels and Task
        // 11c for the transaction handler, before the renderer hooks
        // the prompt into a Screen.
        use foglet_game::PromptKey;

        // Pin the prompt to the SPEC's reference balance (40g) so the
        // priced labels match the example block byte-for-byte. The
        // dynamic-label coverage lives in the dedicated test below.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);

        assert_eq!(
            prompt.body.as_slice(),
            &NIGHT_CLERK_VENDOR_BODY[..],
            "narration must match SPEC §9 step 4 verbatim"
        );

        // SPEC §9 step 4 pins the priced-row layout. After Task 11d the
        // `(T)` row is disabled at STARTING_CASH=40g (40 < 50g rumor
        // price), so the expectation table tracks the post-11d enabled
        // flag per row instead of asserting blanket enabled-ness.
        let expected: &[(char, &str, NightClerkVendorChoice, bool)] = &[
            (
                'b',
                "Buy a black coffee: 25g | You have: 40g",
                NightClerkVendorChoice::BuyCoffee,
                true,
            ),
            (
                't',
                "Tip the clerk for a rumor: 50g | You have: 40g",
                NightClerkVendorChoice::TipForRumor,
                false,
            ),
            ('n', "No thanks", NightClerkVendorChoice::NoThanks, true),
        ];
        assert_eq!(
            prompt.choices.len(),
            expected.len(),
            "Night-clerk vendor must expose the three SPEC §9 choices"
        );
        for (choice, (key, label, value, enabled)) in prompt.choices.iter().zip(expected) {
            assert_eq!(
                choice.key,
                PromptKey::char(*key),
                "hotkey for {label:?} drifted from SPEC §9"
            );
            assert_eq!(choice.label, *label, "label for {key} drifted from SPEC §9");
            assert_eq!(choice.value, *value, "value for {label:?} drifted");
            assert_eq!(
                choice.enabled, *enabled,
                "enabled flag for {key} drifted from SPEC §9 step 4 / Task 11d"
            );
        }
    }

    #[test]
    fn night_clerk_vendor_prompt_renders_body_and_hotkeys() {
        // Drive the prompt through Ratatui's `TestBackend` so the test
        // asserts against the same buffer semantics the live runtime
        // uses (SPEC §6 deterministic-render contract). 70x10 gives the
        // body two rows plus the three choice rows + label without
        // forcing the renderer into modal mode.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);
        let backend = TestBackend::new(70, 10);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");

        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..10 {
            for x in 0..70 {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        // SPEC §9 step 4 narration — both body fragments must survive
        // the renderer's word-wrap regardless of where the lines break
        // at 70 columns.
        for fragment in ["night clerk drums", "Evidence costs extra"] {
            assert!(
                rendered.contains(fragment),
                "rendered prompt missing SPEC §9 step 4 fragment {fragment:?}; got:\n{rendered}"
            );
        }
        // Each choice row must surface its hotkey marker plus the SPEC
        // §9 label so monochrome terminals stay legible. After Task 11d
        // the `(T)` row is disabled at STARTING_CASH=40g, so it renders
        // with the SPEC §4.2 disabled marker `- [T]` and the
        // `(need 50g)` reason instead of the enabled `(T)` form.
        for marker in ["(B)", "- [T]", "(N)"] {
            assert!(
                rendered.contains(marker),
                "missing hotkey marker {marker} in rendered prompt:\n{rendered}"
            );
        }
        for label in [
            "Buy a black coffee: 25g | You have: 40g",
            "Tip the clerk for a rumor: 50g | You have: 40g",
            "No thanks",
        ] {
            assert!(
                rendered.contains(label),
                "missing label {label:?} in rendered prompt:\n{rendered}"
            );
        }
        // SPEC §9 step 4 pins the disabled-tip reason verbatim.
        assert!(
            rendered.contains("(need 50g)"),
            "disabled (T) row must surface NEED_RUMOR_TIP_REASON; got:\n{rendered}"
        );
    }

    #[test]
    fn night_clerk_vendor_prompt_labels_track_current_cash() {
        // SPEC §9 step 4: priced rows render `<action>: <price>g | You
        // have: <cash>g`, where the cash component reflects the
        // *current* balance — not the starting balance. Building the
        // prompt with two distinct cash values and asserting the
        // priced labels move in lockstep with the input is the
        // cheapest way to prove the dynamic-label refactor (Task 11b)
        // didn't accidentally hardcode 40g. The leave-row label must
        // stay static across both balances because the SPEC reference
        // block deliberately omits a price annotation for `(N)`.
        let lean = night_clerk_vendor_prompt(0);
        let flush = night_clerk_vendor_prompt(123);

        let lean_labels: Vec<&str> = lean.choices.iter().map(|c| c.label.as_str()).collect();
        let flush_labels: Vec<&str> = flush.choices.iter().map(|c| c.label.as_str()).collect();

        assert_eq!(
            lean_labels,
            vec![
                "Buy a black coffee: 25g | You have: 0g",
                "Tip the clerk for a rumor: 50g | You have: 0g",
                "No thanks",
            ],
            "lean wallet should render `You have: 0g` on both priced rows",
        );
        assert_eq!(
            flush_labels,
            vec![
                "Buy a black coffee: 25g | You have: 123g",
                "Tip the clerk for a rumor: 50g | You have: 123g",
                "No thanks",
            ],
            "flush wallet should render `You have: 123g` on both priced rows",
        );
    }

    // ---- Night-clerk vendor: buy coffee (SPEC §9 Task 11c) -----------

    #[test]
    fn night_clerk_buy_coffee_decrements_cash_and_emits_success_feedback() {
        // SPEC §9 step 4 / CHECKLIST Task 11c: "Implement buying coffee
        // for 25g. Test: cash decrements and success feedback renders."
        // Run the apply handler against a freshly reset slot bundle so
        // the assertion pins the *delta* (one COFFEE_PRICE deduction)
        // rather than a hardcoded post-balance. The success-style
        // feedback contract is the second half of the SPEC's note that
        // the vendor scene reads as a transactional win — pinning the
        // style role here guards against a regression that swaps it
        // back to `info` (the Lost-and-Found register).
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let starting_cash = slots.player.borrow().cash;
        assert_eq!(
            starting_cash,
            PlayerSlot::STARTING_CASH,
            "fresh reset should boot at the documented starting balance"
        );

        let outcome = apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::BuyCoffee);

        assert_eq!(outcome, Some(NightClerkVendorOutcome::BoughtCoffee));
        assert_eq!(
            slots.player.borrow().cash,
            starting_cash - COFFEE_PRICE,
            "BuyCoffee must deduct exactly COFFEE_PRICE from the player's wallet"
        );

        let line = night_clerk_vendor_feedback(NightClerkVendorOutcome::BoughtCoffee)
            .expect("BoughtCoffee must surface a feedback line");
        assert_eq!(line.text(), BOUGHT_COFFEE_FEEDBACK);
        assert_eq!(
            line.kind(),
            foglet_game::FeedbackKind::Success,
            "coffee purchase must use the success style register"
        );
    }

    #[test]
    fn night_clerk_buy_coffee_lowercase_and_uppercase_match() {
        // SPEC §9 step 7 (mirrored from the Lost-and-Found contract):
        // direct-key prompts must treat lowercase/uppercase identically.
        // Drive the prompt itself — not just the apply handler — so the
        // test pins both halves of the case-folding chain end-to-end.
        for upper in [false, true] {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let starting = slots.player.borrow().cash;
            let prompt = night_clerk_vendor_prompt(starting);
            let key = if upper { 'B' } else { 'b' };
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Selected(NightClerkVendorChoice::BuyCoffee) => {}
                other => panic!("expected BuyCoffee for `{key}`, got {other:?}"),
            }
            apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::BuyCoffee);
            assert_eq!(
                slots.player.borrow().cash,
                starting - COFFEE_PRICE,
                "case-folded `{key}` must produce the same wallet delta",
            );
        }
    }

    // ---- Night-clerk vendor: disabled tip branch (SPEC §9 Task 11d) ----

    #[test]
    fn night_clerk_vendor_disables_tip_when_cash_below_rumor_price() {
        // SPEC §9 step 4: "Tipping is disabled when the player lacks
        // enough cash and shows `need 50g`." Pin both halves — the
        // `enabled` flag and the verbatim reason — at the documented
        // 40g starting balance so a future copy-edit to either has to
        // come through here. Inspecting the typed prompt (rather than
        // the rendered buffer) keeps the test cheap; the renderer
        // surface is already covered by
        // `night_clerk_vendor_prompt_renders_body_and_hotkeys` above.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);

        let tip = prompt
            .choices
            .iter()
            .find(|c| c.value == NightClerkVendorChoice::TipForRumor)
            .expect("(T) choice must remain present so the player sees the gated row");
        assert!(
            !tip.enabled,
            "(T) must be disabled when cash < RUMOR_TIP_PRICE"
        );
        assert_eq!(
            tip.disabled_reason.as_deref(),
            Some(NEED_RUMOR_TIP_REASON),
            "disabled-tip reason must match SPEC §9 step 4 verbatim"
        );

        // Sibling rows must stay enabled — Task 11d only gates `(T)`.
        for value in [
            NightClerkVendorChoice::BuyCoffee,
            NightClerkVendorChoice::NoThanks,
        ] {
            let choice = prompt
                .choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value:?} missing from vendor prompt"));
            assert!(choice.enabled, "{value:?} must remain enabled at 40g");
            assert!(choice.disabled_reason.is_none());
        }
    }

    #[test]
    fn night_clerk_vendor_enables_tip_when_cash_meets_rumor_price() {
        // Mirror of the disabled case so a regression that flips the
        // comparison (`<=` vs `<`, or hardcoding 40g) lights up here
        // rather than masquerading as a manifest-level bug. RUMOR_TIP_PRICE
        // is the boundary — the row should be enabled at exactly 50g
        // (the player can afford the tip) and at any larger balance.
        for cash in [RUMOR_TIP_PRICE, RUMOR_TIP_PRICE + 25, 999] {
            let prompt = night_clerk_vendor_prompt(cash);
            let tip = prompt
                .choices
                .iter()
                .find(|c| c.value == NightClerkVendorChoice::TipForRumor)
                .expect("(T) choice present");
            assert!(
                tip.enabled,
                "(T) must be enabled at cash={cash} (>= RUMOR_TIP_PRICE)"
            );
            assert!(tip.disabled_reason.is_none());
        }
    }

    #[test]
    fn night_clerk_vendor_pressing_disabled_tip_does_not_decrement_cash() {
        // SPEC §9 step 4 + CHECKLIST Task 11d: "starting 40g disables
        // tip and selecting `t` does not decrement cash." Drive the
        // reducer with both lowercase and uppercase to mirror the SPEC
        // §9 step 7 case-folding contract — a disabled hotkey must
        // route through `PromptAction::Disabled` regardless of case so a
        // future regression that only folds the enabled path lights up
        // here. The post-press snapshot equality is the guarantee that
        // the disabled branch never reaches `apply_night_clerk_vendor_choice`
        // (which would `-=` COFFEE_PRICE — wrong amount, but still a
        // mutation) or some hypothetical tip-applier that hasn't been
        // gated yet.
        use foglet_game::PromptAction;

        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let starting_cash = slots.player.borrow().cash;
        assert!(
            starting_cash < RUMOR_TIP_PRICE,
            "test invariant: STARTING_CASH must trigger the disabled-(T) branch"
        );
        let prompt = night_clerk_vendor_prompt(starting_cash);

        for key in ['t', 'T'] {
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Disabled { reason, .. } => assert_eq!(
                    reason.as_deref(),
                    Some(NEED_RUMOR_TIP_REASON),
                    "disabled reason for {key} drifted from SPEC §9 step 4"
                ),
                other => panic!("expected PromptAction::Disabled for `{key}`, got {other:?}"),
            }
        }

        // Reducer never reached the apply handler → state is byte-
        // identical to the pre-press snapshot. Guards against a future
        // regression that adds a `TipForRumor` apply branch without
        // also re-checking the prompt-layer gate.
        assert_eq!(
            slots.snapshot(),
            before,
            "disabled (T) press must not mutate any slot"
        );
        assert_eq!(
            slots.player.borrow().cash,
            starting_cash,
            "cash must be unchanged after a disabled (T) press"
        );
    }

    // ---- Night-clerk vendor: no-thanks exit (SPEC §9 Task 11e) -------

    #[test]
    fn night_clerk_vendor_no_thanks_exits_without_state_change() {
        // SPEC §9 step 4: "No thanks exits cleanly." CHECKLIST Task 11e
        // pins the contract: pressing `n` resolves to a clean prompt
        // exit with zero state mutations. The test drives both halves
        // of the chain — the prompt reducer's case-folded selection
        // (lowercase + uppercase per SPEC §9 step 7) and the apply
        // handler's `None` return — and snapshots the slot bundle on
        // either side of the press to prove no slot moved. A future
        // regression that adds a `NoThanks` apply branch (e.g. a stray
        // morale tick) would flip the snapshot equality and surface
        // here, not deep in a downstream save-replay test.
        use foglet_game::PromptAction;

        for key in ['n', 'N'] {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let before = slots.snapshot();
            let starting_cash = slots.player.borrow().cash;
            let prompt = night_clerk_vendor_prompt(starting_cash);

            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Selected(NightClerkVendorChoice::NoThanks) => {}
                other => panic!("expected NoThanks for `{key}`, got {other:?}"),
            }

            // Apply handler must return `None` for the no-thanks path —
            // there is no `NightClerkVendorOutcome` variant for a clean
            // exit, which is what keeps the feedback helper from
            // accidentally surfacing a misleading success line.
            let outcome = apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::NoThanks);
            assert!(
                outcome.is_none(),
                "NoThanks must produce no outcome (got {outcome:?}) for `{key}`"
            );

            // Byte-identical snapshot proves nothing moved: not cash,
            // not inventory, not flags, not coordinates. Pinning the
            // full snapshot (rather than just `cash`) guards against a
            // future no-thanks side-effect (e.g. an `npc_visited` flag)
            // sneaking in without an explicit SPEC update.
            assert_eq!(
                slots.snapshot(),
                before,
                "NoThanks press for `{key}` must not mutate any slot"
            );
            assert_eq!(
                slots.player.borrow().cash,
                starting_cash,
                "cash must be unchanged after a NoThanks press for `{key}`"
            );
        }
    }

    // ---- Lost-and-Found Drawer action handler (SPEC §9 Task 10c) -----

    #[test]
    fn lost_and_found_lowercase_and_uppercase_hotkeys_match() {
        // SPEC §9 step 7: "lowercase and uppercase hotkeys select the
        // same action". Drive the prompt's reducer with each case and
        // assert the typed `Selected(...)` payload is identical, then
        // apply both through `apply_lost_and_found_choice` against fresh
        // slots and assert the resulting state matches.
        use foglet_game::PromptAction;

        let prompt = lost_and_found_drawer_prompt();
        for (lower, upper, expected) in [
            ('k', 'K', LostAndFoundChoice::TakeRoom7Key),
            ('m', 'M', LostAndFoundChoice::PocketMatchbook),
            ('r', 'R', LostAndFoundChoice::ReadReceipt),
            ('l', 'L', LostAndFoundChoice::Leave),
        ] {
            let lower_action = prompt.handle(Input::Char(lower));
            let upper_action = prompt.handle(Input::Char(upper));
            assert!(
                matches!(lower_action, PromptAction::Selected(v) if v == expected),
                "lowercase {lower} must select {expected:?}, got {lower_action:?}"
            );
            assert!(
                matches!(upper_action, PromptAction::Selected(v) if v == expected),
                "uppercase {upper} must select {expected:?}, got {upper_action:?}"
            );

            // Apply both through the action handler from independent
            // slots and snapshot the result. Equal snapshots prove the
            // case folding is end-to-end, not just a `PromptKey`
            // cosmetic match.
            let lower_slots = SharedSlots::default();
            lower_slots.reset(0, 0);
            let upper_slots = SharedSlots::default();
            upper_slots.reset(0, 0);
            let lower_outcome = apply_lost_and_found_choice(&lower_slots, expected);
            let upper_outcome = apply_lost_and_found_choice(&upper_slots, expected);
            assert_eq!(
                lower_outcome, upper_outcome,
                "outcome must not depend on hotkey case"
            );
            assert_eq!(
                lower_slots.snapshot(),
                upper_slots.snapshot(),
                "post-apply state must not depend on hotkey case for {expected:?}"
            );
        }
    }

    #[test]
    fn lost_and_found_apply_take_key_inserts_room_7_key() {
        // Direct unit on the action handler so a regression in
        // `TakeRoom7Key` points here, not at the higher-level
        // case-folding test. Starts from a fresh `reset` baseline so
        // the assertion isolates the mutation.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        assert!(!slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID));

        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::TakeRoom7Key);

        assert_eq!(outcome, LostAndFoundOutcome::TookRoom7Key);
        assert!(
            slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID),
            "TakeRoom7Key must add ROOM_7_KEY_ID to inventory"
        );
        // Other slots stay untouched — the handler is single-purpose.
        assert!(!slots.inventory.borrow().contains(MapScreen::MATCHBOOK_ID));
        assert!(!slots.flags.borrow().contains(MapScreen::RECEIPT_READ_FLAG));
    }

    #[test]
    fn lost_and_found_apply_pocket_matchbook_inserts_matchbook() {
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::PocketMatchbook);
        assert_eq!(outcome, LostAndFoundOutcome::PocketedMatchbook);
        assert!(
            slots.inventory.borrow().contains(MapScreen::MATCHBOOK_ID),
            "PocketMatchbook must add MATCHBOOK_ID to inventory"
        );
    }

    #[test]
    fn lost_and_found_apply_pocket_matchbook_is_idempotent() {
        // SPEC §9 comment on `LostAndFoundChoice::PocketMatchbook`: the
        // drawer and the lobby map share `MATCHBOOK_ID`, so pocketing
        // twice (or pocketing after lobby pickup) MUST NOT duplicate
        // the keepsake. `BTreeSet::insert` enforces this; the test
        // pins the contract so a future `Vec`-based refactor cannot
        // silently break it.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::MATCHBOOK_ID.to_string());
        apply_lost_and_found_choice(&slots, LostAndFoundChoice::PocketMatchbook);
        let count = slots
            .inventory
            .borrow()
            .iter()
            .filter(|id| id.as_str() == MapScreen::MATCHBOOK_ID)
            .count();
        assert_eq!(count, 1, "matchbook must remain unique in inventory");
    }

    #[test]
    fn lost_and_found_apply_read_receipt_sets_flag() {
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::ReadReceipt);
        assert_eq!(outcome, LostAndFoundOutcome::ReadReceipt);
        assert!(
            slots.flags.borrow().contains(MapScreen::RECEIPT_READ_FLAG),
            "ReadReceipt must set RECEIPT_READ_FLAG"
        );
        // Reading the receipt is purely a flag mutation; inventory
        // stays empty so a future scene can't conflate "read" with
        // "took the receipt as an item".
        assert!(slots.inventory.borrow().is_empty());
    }

    // ---- Lost-and-Found Drawer disabled-(K) branch (SPEC §9 Task 10d) -

    #[test]
    fn lost_and_found_prompt_disables_take_key_when_already_held() {
        // SPEC §9 step 3: "If the player already has the Room 7 key,
        // `(K)` renders disabled with reason `already in inventory`."
        // Inspect the typed prompt rather than the buffer so the test
        // pins the data contract (the renderer test on line 3776
        // already covers visual surfacing of disabled rows via the
        // shared `ChoicePrompt::render` path in `foglet_game`).
        let prompt = lost_and_found_drawer_prompt_with_state(true);

        let take_key = prompt
            .choices
            .iter()
            .find(|c| c.value == LostAndFoundChoice::TakeRoom7Key)
            .expect("(K) choice must still be present so the player sees why it's locked");
        assert!(
            !take_key.enabled,
            "(K) must be disabled when the Room 7 key is already in inventory"
        );
        assert_eq!(
            take_key.disabled_reason.as_deref(),
            Some(ROOM_7_KEY_ALREADY_HELD_REASON),
            "disabled reason must match SPEC §9 step 3 verbatim"
        );

        // Sibling choices stay enabled — Task 10d only gates `(K)`, not
        // the rest of the drawer's affordances.
        for value in [
            LostAndFoundChoice::PocketMatchbook,
            LostAndFoundChoice::ReadReceipt,
            LostAndFoundChoice::Leave,
        ] {
            let choice = prompt
                .choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value:?} choice missing"));
            assert!(
                choice.enabled,
                "{value:?} must remain enabled when only (K) is gated"
            );
            assert!(choice.disabled_reason.is_none());
        }
    }

    #[test]
    fn lost_and_found_prompt_take_key_stays_enabled_without_room_7_key() {
        // Mirror of the disabled case so a future regression that
        // accidentally inverts the condition lights up here, not in a
        // downstream Murder Motel smoke test.
        let prompt = lost_and_found_drawer_prompt_with_state(false);
        let take_key = prompt
            .choices
            .iter()
            .find(|c| c.value == LostAndFoundChoice::TakeRoom7Key)
            .expect("(K) choice present");
        assert!(take_key.enabled);
        assert!(take_key.disabled_reason.is_none());
    }

    #[test]
    fn lost_and_found_pressing_disabled_take_key_does_not_duplicate_item() {
        // SPEC §9 step 3 + Task 10d test directive: "pressing `k` when
        // disabled returns disabled message and does not duplicate the
        // item." We stage a slot with the key already present, build
        // the state-aware prompt, drive both lowercase and uppercase
        // hotkeys through the reducer, assert the typed
        // `PromptAction::Disabled` payload, and confirm the inventory
        // count stays at one.
        use foglet_game::PromptAction;

        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let before = slots.snapshot();

        let prompt = lost_and_found_drawer_prompt_with_state(true);

        for key in ['k', 'K'] {
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Disabled { reason, .. } => assert_eq!(
                    reason.as_deref(),
                    Some(ROOM_7_KEY_ALREADY_HELD_REASON),
                    "disabled reason for {key} drifted from SPEC §9"
                ),
                other => panic!("expected PromptAction::Disabled, got {other:?} for {key}"),
            }
        }

        // The reducer never reached `apply_lost_and_found_choice`, so
        // state must be byte-identical to the pre-press snapshot — no
        // ghost matchbook, no flag flips, and crucially no second
        // ROOM_7_KEY_ID inserted (BTreeSet would dedupe anyway, but a
        // future Vec-based inventory must not regress this).
        assert_eq!(
            slots.snapshot(),
            before,
            "disabled (K) press must not mutate any slot"
        );
        let count = slots
            .inventory
            .borrow()
            .iter()
            .filter(|id| id.as_str() == MapScreen::ROOM_7_KEY_ID)
            .count();
        assert_eq!(
            count, 1,
            "Room 7 key must not be duplicated by a disabled press"
        );
    }

    // ---- Lost-and-Found Drawer feedback messages (SPEC §9 Task 10e) ----

    #[test]
    fn lost_and_found_feedback_messages_match_spec_strings() {
        // SPEC §9 step 5 pins the take-key wording verbatim. The
        // matchbook and receipt strings live in the example, but Task
        // 10e calls them out by name in the checklist; pin all three so
        // a copy edit forces an explicit checklist update.
        assert_eq!(TOOK_ROOM_7_KEY_FEEDBACK, "Moved Room 7 key to inventory.");
        assert_eq!(
            POCKETED_MATCHBOOK_FEEDBACK,
            "Pocketed the cracked matchbook."
        );
        assert!(
            READ_RECEIPT_FEEDBACK.contains("Room 7"),
            "receipt feedback should mention Room 7 to reward the read"
        );
    }

    #[test]
    fn lost_and_found_feedback_take_key_emits_take_message() {
        // Action handler returns `TookRoom7Key`; the feedback mapper
        // MUST surface the SPEC §9 step 5 line. Using `rendered_text`
        // also pins the absence of a leading marker (Info kind).
        let line = lost_and_found_feedback(LostAndFoundOutcome::TookRoom7Key)
            .expect("TookRoom7Key must emit a feedback line");
        assert_eq!(line.text(), TOOK_ROOM_7_KEY_FEEDBACK);
        assert_eq!(line.rendered_text(), TOOK_ROOM_7_KEY_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_pocket_matchbook_emits_pocket_message() {
        let line = lost_and_found_feedback(LostAndFoundOutcome::PocketedMatchbook)
            .expect("PocketedMatchbook must emit a feedback line");
        assert_eq!(line.text(), POCKETED_MATCHBOOK_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_read_receipt_emits_receipt_text() {
        let line = lost_and_found_feedback(LostAndFoundOutcome::ReadReceipt)
            .expect("ReadReceipt must emit a feedback line");
        assert_eq!(line.text(), READ_RECEIPT_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_leave_emits_no_message() {
        // Walking away is intentionally silent; emitting a "You walked
        // away." line would imply state mutation the (L) branch
        // explicitly avoids (Task 10e doc + Task 10c noop guarantee).
        assert!(lost_and_found_feedback(LostAndFoundOutcome::Left).is_none());
    }

    #[test]
    fn lost_and_found_feedback_pairs_with_apply_outcome_end_to_end() {
        // End-to-end pin: apply each enabled choice against a fresh
        // SharedSlots, feed the outcome through the feedback mapper,
        // and assert the resulting text. Catches regressions where
        // `apply_lost_and_found_choice` and `lost_and_found_feedback`
        // drift apart on the variant->message contract.
        let cases = [
            (LostAndFoundChoice::TakeRoom7Key, TOOK_ROOM_7_KEY_FEEDBACK),
            (
                LostAndFoundChoice::PocketMatchbook,
                POCKETED_MATCHBOOK_FEEDBACK,
            ),
            (LostAndFoundChoice::ReadReceipt, READ_RECEIPT_FEEDBACK),
        ];
        for (choice, expected) in cases {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let outcome = apply_lost_and_found_choice(&slots, choice);
            let line = lost_and_found_feedback(outcome)
                .unwrap_or_else(|| panic!("{choice:?} should produce feedback"));
            assert_eq!(line.text(), expected, "feedback drift for {choice:?}");
        }
    }

    #[test]
    fn lost_and_found_apply_leave_is_a_noop() {
        // SPEC §9 step 2: "(L) Leave it alone" exits the prompt without
        // mutating state. Snapshot before/after equality is the
        // strongest possible assertion that no slot was touched.
        let slots = SharedSlots::default();
        slots.reset(7, 3);
        let before = slots.snapshot();
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::Leave);
        assert_eq!(outcome, LostAndFoundOutcome::Left);
        assert_eq!(
            slots.snapshot(),
            before,
            "Leave must not mutate inventory, flags, or player state"
        );
    }

    // ---- Lost-and-Found Drawer lobby integration (SPEC §9 Task 10f) ---

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
    fn drawer_screen_callback_take_key_writes_feedback_and_pops() {
        // Drive the same `PromptScreen` the runtime uses, scripting a
        // `(K)` press through `handle_input` and asserting the captured
        // side effects: the Room 7 key lands in inventory, the feedback
        // slot carries the SPEC §9 step 5 line, and the screen returns
        // `Pop` so the lobby comes back into focus.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('k'));
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert!(
            slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID),
            "(K) must move the Room 7 key into the inventory"
        );
        let feedback = slots.feedback.borrow().clone().expect("feedback set");
        assert_eq!(feedback.text(), TOOK_ROOM_7_KEY_FEEDBACK);
    }

    #[test]
    fn drawer_screen_callback_disabled_take_key_writes_error_and_stays_open() {
        // Pre-load the key so the prompt's `(K)` row builds disabled,
        // then verify the callback surfaces the SPEC §9 reason as an
        // error feedback line and emits `None` so the prompt stays on
        // screen for a different choice.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('k'));
        assert!(
            matches!(cmd, ScreenCommand::None),
            "disabled press must keep the prompt open"
        );
        let feedback = slots
            .feedback
            .borrow()
            .clone()
            .expect("disabled press must surface a reason");
        assert_eq!(feedback.text(), ROOM_7_KEY_ALREADY_HELD_REASON);
    }

    #[test]
    fn drawer_screen_callback_cancel_pops_without_mutating_state() {
        // Esc on a cancellable prompt is the SPEC §4.4 "back out" gesture.
        // The lobby must come back unchanged — no key, no flag, no
        // matchbook bonus from a hesitant press.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(slots.snapshot(), before);
        assert!(slots.feedback.borrow().is_none());
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
    fn map_hint_advertises_search_affordance() {
        // The hint line is the only place a player learns the search
        // affordance exists. If a future copy edit drops "Search: X" the
        // drawer becomes unreachable except by accident.
        assert!(
            MapScreen::HINT_LINE.contains("Search: X"),
            "hint line must advertise the search key; got {:?}",
            MapScreen::HINT_LINE
        );
    }
}
