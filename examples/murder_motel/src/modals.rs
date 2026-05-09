//! Static / single-purpose modal screens: help, win, inventory.
//!
//! Grouped because each one is small enough not to warrant its own
//! file but they share no state: the help screen is a fixed reference
//! card, the win screen is the terminal "case closed" beat, and the
//! inventory screen reads from the shared inventory store the map
//! screen writes to.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use foglet_game::{
    centred_rect, render_hint_line, render_inventory_list, render_modal, EventRecord,
    FogletContext, GameContext, Input, InventoryList, LeaderboardSort, ScoreRecord, Screen,
    ScreenCommand, WorldDb,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::map::MapScreen;

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
        "Bulletin      E   (lobby bulletin / recent events ledger)",
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
        // SPEC_v2_1 §4.3 modal: bordered, titled, left-aligned body.
        // `render_modal` matches the prior shape (one-cell horizontal
        // padding inside the border) so the help body and Task 13b
        // substring assertions still hit.
        let body = Self::LINES.join("\n");
        render_modal(frame, area, Some(Self::TITLE), &body);
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
        // SPEC_v2_1 §4.3 modal: bordered, titled, left-aligned body.
        // Width matches the longest line plus border padding; height is
        // the line count plus two for the border so every line is
        // visible on an exactly-80x24 terminal.
        let area = centred_rect(72, (Self::LINES.len() as u16) + 2, frame.area());
        let body = Self::LINES.join("\n");
        render_modal(frame, area, Some(Self::TITLE), &body);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, _input: Input) -> ScreenCommand {
        // Any key quits — the modal is the end of the game.
        ScreenCommand::Quit
    }
}

/// Read-only "Profile" modal showing the player's Foglet role and
/// security level (SPEC_v2 §Task 13h).
///
/// SPEC §4.5 is explicit that role/security values are **advisory** —
/// Foglet is the source of truth for door authorization, and the kit
/// surfaces these only for in-game flavor (sysop/mod affordances,
/// dropfile-compatible permission checks). The Profile modal exists so
/// a player on a sysop or mod context can *see* that the game knows
/// who they are, while the body copy spells out that the label has no
/// authorization weight inside the door.
///
/// The screen captures the displayed values at construction time from
/// [`GameContext::foglet`] so the render path does no
/// [`FogletContext`] lookups (mirrors how the leaderboard and bulletin
/// modals snapshot their data — keeps `Screen::render` allocation- and
/// query-free per SPEC §Task 10d, which only formally applies to world
/// queries but is the right shape for any read).
///
/// ## Why a dedicated screen
///
/// Task 13h calls for "sysop/mod/user synthetic contexts show distinct
/// labels/security levels". Embedding the proof on the help screen
/// would couple the controls reference to runtime state; a dedicated
/// modal is the simplest path that keeps every other screen unchanged
/// and lets the test load three synthetic contexts in isolation.
///
/// ## Input contract
///
/// - `Esc` / `Backspace` / `p` / `P` pop back to the menu beneath us.
/// - `Q` / `Ctrl-C` still hard-quit, matching every other modal.
#[derive(Debug)]
pub struct ProfileScreen {
    /// Display label for the role (e.g. `"sysop"`, `"mod"`, `"user"`,
    /// or the raw payload from [`FogletRole::Other`]). Captured at
    /// construction so render doesn't re-derive it per frame.
    role_label: String,
    /// Security level integer (50 / 90 / 100 today). Stored alongside
    /// the label so the modal renders the SPEC §4.5 mapping next to the
    /// role string the player would otherwise see in isolation.
    security_level: i64,
    /// Display handle for the player. Snapshot of
    /// [`FogletContext::username`] (or `"(local dev)"` when absent) so
    /// the operator-facing identity matches what the player typed at
    /// the BBS prompt rather than the opaque `user_id`.
    handle: String,
}

impl ProfileScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Profile";
    /// Hint band shown at the bottom of the modal. Mirrors the close
    /// affordances exposed by [`Self::handle_input`].
    pub const HINT: &'static str = "[Esc/P/Backspace] close    [Q] quit";
    /// Fallback display when the Foglet context didn't carry a username
    /// (local-dev sessions, anonymous-access doors). Constant so tests
    /// can pin the literal.
    pub const ANONYMOUS_HANDLE: &'static str = "(local dev)";

    /// Build a profile modal from a borrowed [`FogletContext`].
    ///
    /// Used by [`Self::from_context`] and by tests that want to assert
    /// the captured fields without standing up a [`GameContext`]. The
    /// inherent constructor takes a borrow so callers don't need to
    /// clone their session-scoped context.
    pub fn from_foglet(foglet: &FogletContext) -> Self {
        let role = foglet.foglet_role();
        Self {
            role_label: role.as_token().to_string(),
            security_level: foglet.security_level(),
            handle: foglet
                .username
                .clone()
                .unwrap_or_else(|| Self::ANONYMOUS_HANDLE.to_string()),
        }
    }

    /// Convenience used from the menu activation path: build the modal
    /// from a `&GameContext`. The borrow-bag context keeps the
    /// `FogletContext` reference alive for as long as the runtime owns
    /// it, so the snapshot we take here is always self-consistent.
    pub fn from_context(ctx: &GameContext<'_>) -> Self {
        Self::from_foglet(ctx.foglet)
    }

    /// Captured role label, exposed for tests.
    pub fn role_label(&self) -> &str {
        &self.role_label
    }

    /// Captured security level, exposed for tests.
    pub fn security_level(&self) -> i64 {
        self.security_level
    }

    /// Captured handle string, exposed for tests.
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// The body lines rendered inside the modal. Pulled into a method
    /// so render and the test exercising "all three labels appear"
    /// share one source of truth.
    fn body_lines(&self) -> Vec<String> {
        vec![
            format!("Handle           {}", self.handle),
            format!("Role             {}", self.role_label),
            format!("Security level   {}", self.security_level),
            String::new(),
            // Two-line wrap so the advisory fits a 56-column modal even
            // when the role label is the longest variant.
            "Advisory only — Foglet decides who launches the door.".to_string(),
            "These values are in-game flavor, not authorization.".to_string(),
        ]
    }
}

impl Screen for ProfileScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let lines = self.body_lines();
        // Reserve the bottom row for the hint band so close affordances
        // are always visible regardless of body length.
        let height = (lines.len() as u16) + 3; // +2 border, +1 hint
        let area = centred_rect(60, height, frame.area());
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

        // SPEC_v2_1 §4.3 modal + hint line: shared shape between the
        // profile read-out and the kit's other left-aligned modals
        // (HelpScreen). `render_hint_line` swaps the hand-rolled
        // DarkGray paragraph for the kit-standard `StyleRole::Hint`
        // (DIM) styling — visually similar but consistent across screens.
        let body_text = lines.join("\n");
        render_modal(frame, body, Some(Self::TITLE), &body_text);
        if hint_h > 0 {
            render_hint_line(frame, hint, Self::HINT);
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Hard-quit affordances — Q / Ctrl-C never get trapped.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // The same key that opens the modal also closes it (`p`/`P`),
            // mirroring the `i` toggle on the inventory screen.
            Input::Esc | Input::Backspace | Input::Char('p') | Input::Char('P') => {
                ScreenCommand::Pop
            }
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
    /// Walks two sources, in this order:
    ///
    /// 1. [`MapScreen::ITEMS`] — the on-map collectables picked up via
    ///    `try_move`.
    /// 2. [`MapScreen::EXTRA_INVENTORY_ITEMS`] — items granted by
    ///    prompts (the Lost-and-Found Drawer's Room 7 key) that have
    ///    no map cell and so wouldn't appear on the first list.
    ///
    /// Both sources are walked in declaration order so the UI listing
    /// is stable and independent of insertion order. IDs in the
    /// inventory set without a match in either catalog are silently
    /// skipped — a forward-compat no-op for stale save IDs.
    pub fn current_labels(&self) -> Vec<String> {
        let held = self.inventory.borrow();
        let from_map = MapScreen::ITEMS
            .iter()
            .filter(|item| held.contains(item.id))
            .map(|item| item.name.to_string());
        let from_extras = MapScreen::EXTRA_INVENTORY_ITEMS
            .iter()
            .filter(|(id, _)| held.contains(*id))
            .map(|(_, name)| name.to_string());
        from_map.chain(from_extras).collect()
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
            render_hint_line(frame, hint, "[Up/Down] choose    [Esc/I] close");
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

/// Lobby bulletin / recent-events ledger (SPEC_v2 §Task 13d).
///
/// Snapshots the newest [`BulletinScreen::EVENT_LIMIT`] rows from the
/// kit's `world_events` table at the moment the player presses `E` and
/// renders them as a scrollable list. The query happens at construction
/// rather than per-frame because [`Screen::render`] forbids blocking
/// world queries (SPEC §Task 10d) — the modal is push-on-demand, so a
/// single read on construction matches what the player just asked for.
///
/// "System" events (rows with `NULL` `player_id`) are included on
/// purpose: the lobby bulletin is the global feed, and Murder Motel
/// uses player-attributed rows today. If the future bulletin grows a
/// per-player view it can call [`WorldDb::player_events`] from a
/// sibling screen.
///
/// ## Render contract
///
/// - One row per event: `[HH:MM:SS] kind — message`. The timestamp is
///   the raw SQLite text (UTC `YYYY-MM-DD HH:MM:SS`) trimmed to the
///   `HH:MM:SS` slice; the kit deliberately stores ISO text so the
///   operator-facing `sqlite3` story matches the runtime view, and the
///   modal mirrors that.
/// - When the world DB is absent or returns no events, the screen
///   renders the [`Self::EMPTY_HINT`] line so the affordance never
///   feels broken. Callers that pass `None` (no `[world]` configured,
///   tests that don't stand a DB up) reach the same code path.
///
/// ## Input contract
///
/// - `Up` / `Down` (and `j`/`k`) move the highlight cursor.
/// - `Esc` / `Backspace` / `e` / `E` pop back to the map.
/// - `Q` / `Ctrl-C` still hard-quit, matching every other screen.
pub struct BulletinScreen {
    /// Snapshot of recent events, newest first. Owned by the screen so
    /// the world DB can be released after construction — the bulletin
    /// is a frozen read of "what was true when you opened it" rather
    /// than a live tail.
    events: Vec<EventRecord>,
    /// Highlighted row. Clamped at render time against the live event
    /// count so an empty bulletin never points the cursor past zero.
    selected: usize,
}

impl BulletinScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Bulletin";
    /// Empty-state hint shown when the bulletin holds no rows. Pulled
    /// out as a constant so tests can assert it without binding to
    /// incidental wording.
    pub const EMPTY_HINT: &'static str = "(no bulletin entries yet)";
    /// Maximum rows fetched from `world_events` on open. 50 is enough
    /// to cover several days of Murder Motel play on an 80x24 terminal
    /// and well under SPEC §Task 7c's "newest-N" intent — the modal
    /// re-queries each time the player opens it, so a hard cap is the
    /// right tradeoff against pulling thousands of rows on a long-
    /// running door.
    pub const EVENT_LIMIT: u32 = 50;

    /// Build a bulletin screen by querying `world` for the newest
    /// [`Self::EVENT_LIMIT`] events. `None` (or a DB that returns an
    /// error) yields an empty bulletin — the screen renders the
    /// empty-state hint and the player can still close it cleanly.
    ///
    /// Errors from `recent_events` are deliberately swallowed: the
    /// SPEC §13.x terminal-safety contract forbids panicking out of
    /// the input handler that pushes us, and a transient SQLite error
    /// shouldn't soft-lock the player at the lobby. The kit logs the
    /// underlying error through `tracing` already (Task 7); the modal
    /// just shows nothing rather than the error message.
    pub fn from_world_db(world: Option<&WorldDb>) -> Self {
        let events = world
            .and_then(|w| w.recent_events(Self::EVENT_LIMIT).ok())
            .unwrap_or_default();
        Self {
            events,
            selected: 0,
        }
    }

    /// Construct directly from an event vector. Used by the tests so
    /// they can drive the screen without standing up a `WorldDb`; also
    /// the building block [`Self::from_world_db`] funnels through.
    pub fn from_events(events: Vec<EventRecord>) -> Self {
        Self {
            events,
            selected: 0,
        }
    }

    /// Format one event row for display. Pulled out of `render` so the
    /// formatting contract is unit-testable (SPEC §Task 13d's
    /// "deterministic tie ordering" lives in the kit's
    /// `recent_events`; this helper just renders what we got).
    ///
    /// Format: `[HH:MM:SS] kind — message`. If the SQLite timestamp
    /// doesn't contain a space (corrupt or hand-edited row) the whole
    /// stored value is shown so an operator can still see what's there.
    pub fn format_row(event: &EventRecord) -> String {
        let time = event
            .created_at
            .split(' ')
            .nth(1)
            .unwrap_or(event.created_at.as_str());
        format!("[{time}] {} — {}", event.kind, event.message)
    }
}

impl Screen for BulletinScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre a tall modal so the list can hold the full snapshot on
        // a 24-row terminal without scrolling: 18 body rows + 2 border
        // + 1 hint row fits comfortably under SPEC §13.1's 80x24 floor.
        let outer = frame.area();
        let area = centred_rect(72, 21, outer);

        // Reserve a one-row hint band at the bottom of the modal so
        // the controls are always visible — same pattern as
        // `InventoryScreen`.
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

        if self.events.is_empty() {
            // Empty bulletin: render the hint inside the bordered
            // block so the screen doesn't just look like a blank box.
            // Players who arrive before any events fire (fresh world
            // DB, single-player runs) see "(no bulletin entries yet)"
            // rather than a void.
            let widget = Paragraph::new(Self::EMPTY_HINT)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
            frame.render_widget(widget, body);
        } else {
            // Clamp the cursor against the live count so a row removed
            // between renders (impossible today — events are append-
            // only — but cheap insurance) never points off the end.
            let selected = self.selected.min(self.events.len() - 1);
            let items: Vec<ListItem<'_>> = self
                .events
                .iter()
                .enumerate()
                .map(|(idx, event)| {
                    let mut line = Line::from(Self::format_row(event));
                    if idx == selected {
                        line = line.style(Style::default().fg(Color::Black).bg(Color::White));
                    }
                    ListItem::new(line)
                })
                .collect();
            let list =
                List::new(items).block(Block::default().borders(Borders::ALL).title(Self::TITLE));
            frame.render_widget(list, body);
        }
        if hint_h > 0 {
            render_hint_line(frame, hint, "[Up/Down] scroll    [Esc/E] close");
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on hard-quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Esc / Backspace / `e` close the modal — `e` toggles so
            // the same keystroke that opens the bulletin also closes
            // it, matching the inventory's `i` toggle pattern.
            Input::Esc | Input::Backspace | Input::Char('e') | Input::Char('E') => {
                ScreenCommand::Pop
            }
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                if !self.events.is_empty() && self.selected + 1 < self.events.len() {
                    self.selected += 1;
                }
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

/// Investigators leaderboard screen reachable from the main menu
/// (SPEC_v2 §Task 13f).
///
/// Snapshots the top [`LeaderboardScreen::TOP_N`] rows of the
/// `investigators` board (SPEC_v2 §Task 13e) at construction time and
/// renders them as a numbered list `<rank>. <handle>  <score>`. The
/// query happens on push, never per-frame, because SPEC §Task 10d
/// forbids blocking world queries on the render path. Mirrors
/// [`BulletinScreen`]: same modal shape, same close affordances, same
/// log-and-swallow behaviour around transient SQLite errors.
///
/// ## Render contract
///
/// - One row per score, prefixed with the 1-based rank.
/// - Handles are looked up via [`WorldDb::player_handle`]; an unknown
///   id (deleted out from under us by an operator cleanup) renders as
///   [`Self::UNKNOWN_HANDLE`] so the row stays useful instead of
///   silently disappearing.
/// - When the world DB is absent or the board has no rows, the screen
///   shows [`Self::EMPTY_HINT`] — same affordance as the bulletin.
///
/// ## Input contract
///
/// - `Up`/`Down` (and `j`/`k`) move the highlight cursor.
/// - `Esc` / `Backspace` / `l` / `L` pop back to the main menu.
/// - `Q` / `Ctrl-C` still hard-quit, matching every other screen.
pub struct LeaderboardScreen {
    /// Pre-resolved rows: each top-scores entry is paired with the
    /// handle string we'll render, so the render path makes no DB
    /// calls. Empty when the board is empty *or* the world DB is
    /// absent — both reach the same empty-state hint.
    rows: Vec<LeaderboardRow>,
    /// Highlighted row. Clamped at render time against the live row
    /// count so an empty board never points the cursor past zero.
    selected: usize,
}

/// One rendered leaderboard row: the kit-side score plus the handle
/// the screen will display. Resolving the handle once at construction
/// keeps the render path free of DB calls and lets tests build screens
/// from synthetic data without standing up a `players` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardRow {
    /// The score record as returned by [`WorldDb::top_scores`]. Stored
    /// verbatim so future render tweaks (e.g. show `updated_at`) can
    /// reach the original SQLite columns without re-querying.
    pub score: ScoreRecord,
    /// Display string for the player. Resolved via
    /// [`WorldDb::player_handle`] at construction; falls back to
    /// [`LeaderboardScreen::UNKNOWN_HANDLE`] when the lookup misses.
    pub handle: String,
}

impl LeaderboardScreen {
    /// Block title rendered on the bordered modal.
    pub const TITLE: &'static str = "Investigators Leaderboard";
    /// Empty-state hint shown when the leaderboard holds no rows.
    pub const EMPTY_HINT: &'static str = "(no investigators ranked yet)";
    /// Placeholder rendered when [`WorldDb::player_handle`] returns
    /// `Ok(None)` for an id pulled out of `top_scores`. Pulled out as a
    /// constant so tests can pin the wording.
    pub const UNKNOWN_HANDLE: &'static str = "unknown";
    /// Maximum rows fetched from `top_scores` on open. 10 fits an 80x24
    /// modal comfortably and matches the typical "top ten" leaderboard
    /// idiom; the kit's `top_scores` accepts any `u32` so a future tweak
    /// only edits this constant.
    pub const TOP_N: u32 = 10;

    /// Build a leaderboard screen by querying `world` for the top
    /// [`Self::TOP_N`] rows of the `investigators` board. `None` (or a
    /// DB that returns an error) yields an empty leaderboard — the
    /// screen renders the empty-state hint and the player can still
    /// close it cleanly.
    ///
    /// Errors from `top_scores` / `player_handle` are deliberately
    /// swallowed for the same terminal-safety reason
    /// [`BulletinScreen::from_world_db`] swallows its read errors: a
    /// transient SQLite hiccup must not soft-lock the player at the
    /// menu, and `tracing` already records the underlying error.
    pub fn from_world_db(world: Option<&WorldDb>) -> Self {
        let rows = world
            .and_then(|w| {
                let scores = w
                    .top_scores(
                        crate::world::INVESTIGATORS_LEADERBOARD_NAME,
                        LeaderboardSort::Desc,
                        Self::TOP_N,
                    )
                    .ok()?;
                Some(
                    scores
                        .into_iter()
                        .map(|score| {
                            // `Ok(None)` and any error path collapse to
                            // the placeholder: the leaderboard row stays
                            // visible either way, and operators can use
                            // the rank/score to chase down the missing
                            // player record offline if they need to.
                            let handle = w
                                .player_handle(score.player_id)
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| Self::UNKNOWN_HANDLE.to_string());
                            LeaderboardRow { score, handle }
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        Self { rows, selected: 0 }
    }

    /// Construct directly from a row vector. Used by the tests so they
    /// can drive the screen without standing up a `WorldDb`; also the
    /// building block [`Self::from_world_db`] funnels through.
    pub fn from_rows(rows: Vec<LeaderboardRow>) -> Self {
        Self { rows, selected: 0 }
    }

    /// Format one leaderboard row for display. Pulled out of `render`
    /// so the format contract is unit-testable. Format is
    /// `<rank>. <handle>  <score>` with a 2-space gutter so the score
    /// column lines up regardless of handle width.
    pub fn format_row(rank: usize, row: &LeaderboardRow) -> String {
        format!("{:>2}. {}  {}", rank, row.handle, row.score.score)
    }
}

impl Screen for LeaderboardScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Same modal shape as [`BulletinScreen`]: a centred 60x14 box
        // with a one-row hint band at the bottom. 14 rows hold the top
        // ten plus borders + hint comfortably under the SPEC §13.1
        // 80x24 floor.
        let outer = frame.area();
        let area = centred_rect(60, 14, outer);

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

        if self.rows.is_empty() {
            let widget = Paragraph::new(Self::EMPTY_HINT)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
            frame.render_widget(widget, body);
        } else {
            let selected = self.selected.min(self.rows.len() - 1);
            let items: Vec<ListItem<'_>> = self
                .rows
                .iter()
                .enumerate()
                .map(|(idx, row)| {
                    let mut line = Line::from(Self::format_row(idx + 1, row));
                    if idx == selected {
                        line = line.style(Style::default().fg(Color::Black).bg(Color::White));
                    }
                    ListItem::new(line)
                })
                .collect();
            let list =
                List::new(items).block(Block::default().borders(Borders::ALL).title(Self::TITLE));
            frame.render_widget(list, body);
        }
        if hint_h > 0 {
            render_hint_line(frame, hint, "[Up/Down] scroll    [Esc/L] close");
        }
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            // Always-on hard-quit affordances.
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            // Esc / Backspace / `l` close the modal — `l` toggles so
            // the same key that opens the leaderboard from the menu
            // also closes it from inside, matching the bulletin's `e`
            // and inventory's `i` toggle pattern.
            Input::Esc | Input::Backspace | Input::Char('l') | Input::Char('L') => {
                ScreenCommand::Pop
            }
            Input::Up | Input::Char('k') | Input::Char('K') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                if !self.rows.is_empty() && self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
                ScreenCommand::None
            }
            _ => ScreenCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the static help, terminal win, and modal inventory
    //! screens. Each test stands the screen up directly and dispatches
    //! one input — no map / dialog state is required.

    use super::*;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::GameContext;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Dispatch one input to a fresh help screen.
    fn dispatch_help(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        HelpScreen.handle_input(&mut ctx, input)
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

    // ---- WinScreen -----------------------------------------------------

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

    // ---- ProfileScreen ------------------------------------------------

    /// Build a [`FogletContext`] for the role-display tests. Local-dev
    /// source so we don't depend on Foglet wire JSON, and a deterministic
    /// handle so the buffer assertion below can pin the rendered string.
    fn ctx_for_role(role: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "murder-motel".into(),
            user_id: Some("u-test".into()),
            username: Some("tester".into()),
            role: role.map(|r| r.to_string()),
            session_id: Some("s-test".into()),
            terminal_width: 80,
            terminal_height: 24,
            source: foglet_game::ContextSource::LocalDev,
        }
    }

    /// Render a [`ProfileScreen`] into a [`TestBackend`] and dump the
    /// cell grid as a single string so callers can assert substrings.
    fn render_profile_to_string(role: Option<&str>) -> String {
        let cfg = fixture_config();
        let fc = ctx_for_role(role);
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = ProfileScreen::from_foglet(&fc);
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
        found
    }

    #[test]
    fn profile_sysop_context_shows_distinct_label_and_security() {
        // `"sysop"` must surface the canonical label and the SPEC §4.5
        // security level (100). The two values together prove the
        // typed `FogletRole` mapping reached the modal.
        let screen = ProfileScreen::from_foglet(&ctx_for_role(Some("sysop")));
        assert_eq!(screen.role_label(), "sysop");
        assert_eq!(screen.security_level(), 100);
        let buf = render_profile_to_string(Some("sysop"));
        assert!(buf.contains("sysop"), "missing sysop label; buffer:\n{buf}");
        assert!(buf.contains("100"), "missing sysop level; buffer:\n{buf}");
    }

    #[test]
    fn profile_mod_context_shows_distinct_label_and_security() {
        let screen = ProfileScreen::from_foglet(&ctx_for_role(Some("mod")));
        assert_eq!(screen.role_label(), "mod");
        assert_eq!(screen.security_level(), 90);
        let buf = render_profile_to_string(Some("mod"));
        assert!(buf.contains("mod"), "missing mod label; buffer:\n{buf}");
        assert!(buf.contains("90"), "missing mod level; buffer:\n{buf}");
    }

    #[test]
    fn profile_user_context_shows_distinct_label_and_security() {
        // Both an explicit `"user"` and a missing role must collapse to
        // the user-level mapping (50). We test both spellings here so a
        // future tweak to `FogletRole::parse` that forgets one branch
        // surfaces against the same modal.
        for role in [Some("user"), None] {
            let screen = ProfileScreen::from_foglet(&ctx_for_role(role));
            assert_eq!(screen.role_label(), "user", "role={role:?}");
            assert_eq!(screen.security_level(), 50, "role={role:?}");
        }
        let buf = render_profile_to_string(Some("user"));
        assert!(buf.contains("user"), "missing user label; buffer:\n{buf}");
        assert!(buf.contains("50"), "missing user level; buffer:\n{buf}");
    }

    #[test]
    fn profile_distinguishes_all_three_roles() {
        // The three canonical roles must produce three distinct
        // (label, level) pairs. This is the SPEC_v2 §Task 13h
        // "synthetic contexts show distinct labels/security levels"
        // proof, asserted at the level-pair granularity rather than
        // through three independent buffers.
        let triples: Vec<(String, i64)> = ["sysop", "mod", "user"]
            .iter()
            .map(|r| {
                let s = ProfileScreen::from_foglet(&ctx_for_role(Some(r)));
                (s.role_label().to_string(), s.security_level())
            })
            .collect();
        assert_eq!(
            triples,
            vec![
                ("sysop".into(), 100),
                ("mod".into(), 90),
                ("user".into(), 50),
            ],
        );
    }

    #[test]
    fn profile_render_includes_advisory_disclaimer() {
        // The screen MUST surface the in-game/advisory framing — the
        // proof would be misleading otherwise (a sysop label rendered
        // without the disclaimer reads as a real authorization
        // signal). Check both halves of the wrapped sentence to lock
        // in the framing without binding to incidental whitespace.
        let buf = render_profile_to_string(Some("sysop"));
        assert!(
            buf.contains("Advisory only") && buf.contains("Foglet decides"),
            "missing advisory framing; buffer was:\n{buf}"
        );
        assert!(
            buf.contains("in-game flavor"),
            "missing in-game flavor framing; buffer was:\n{buf}"
        );
    }

    #[test]
    fn profile_unknown_role_falls_back_to_label_with_user_security() {
        // `FogletRole::Other` keeps the raw string for the label but
        // collapses to user-level (50) for security — the modal must
        // surface both faithfully (otherwise an unknown payload would
        // silently appear sysop-coloured by accident).
        let screen = ProfileScreen::from_foglet(&ctx_for_role(Some("ops")));
        assert_eq!(screen.role_label(), "ops");
        assert_eq!(screen.security_level(), 50);
    }

    #[test]
    fn profile_anonymous_handle_falls_back_to_local_dev_label() {
        // No Foglet username → the modal shows the documented sentinel
        // rather than rendering a bare colon. The test pins the literal
        // so a future refactor can't silently change what the player
        // sees.
        let mut fc = ctx_for_role(Some("user"));
        fc.username = None;
        let screen = ProfileScreen::from_foglet(&fc);
        assert_eq!(screen.handle(), ProfileScreen::ANONYMOUS_HANDLE);
    }

    #[test]
    fn profile_close_keys_pop() {
        // Esc / Backspace / `p` / `P` all close the modal — the
        // open-key-toggles-closed pattern shared with the inventory
        // screen.
        let cfg = fixture_config();
        let fc = ctx_for_role(Some("sysop"));
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for key in [
            Input::Esc,
            Input::Backspace,
            Input::Char('p'),
            Input::Char('P'),
        ] {
            let mut screen = ProfileScreen::from_foglet(&fc);
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Pop),
                "{key:?} should pop the profile screen"
            );
        }
    }

    #[test]
    fn profile_quit_keys_quit() {
        let cfg = fixture_config();
        let fc = ctx_for_role(Some("sysop"));
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for key in [Input::Char('q'), Input::Char('Q'), Input::Ctrl('c')] {
            let mut screen = ProfileScreen::from_foglet(&fc);
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} should quit the profile screen"
            );
        }
    }

    // ---- InventoryScreen ----------------------------------------------

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
    fn inventory_lists_room_7_key_from_extras_catalog() {
        // Regression for the user-reported bug "took the room 7 key
        // from the drawer but it never appeared in inventory." The
        // ID lives in `EXTRA_INVENTORY_ITEMS`, not the map-cell
        // catalog, so the screen has to walk both sources.
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let screen = InventoryScreen::new(Rc::clone(&inv));
        assert_eq!(
            screen.current_labels(),
            vec!["Room 7 key".to_string()],
            "drawer-granted Room 7 key must surface in the inventory list"
        );
    }

    #[test]
    fn inventory_orders_map_items_before_extras() {
        // Ordering contract: map-cell items appear in catalog order
        // first, then the extras. Prevents a future re-author of the
        // drawer key's position in `EXTRA_INVENTORY_ITEMS` from
        // accidentally pushing the brass key down the list.
        let inv = Rc::new(RefCell::new(BTreeSet::new()));
        inv.borrow_mut().insert("brass_key".to_string());
        inv.borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let screen = InventoryScreen::new(Rc::clone(&inv));
        assert_eq!(
            screen.current_labels(),
            vec!["Brass key".to_string(), "Room 7 key".to_string()],
            "map-cell items must list before extras-catalog entries"
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

    // ---- BulletinScreen ----------------------------------------------

    /// Build a synthetic [`EventRecord`] for the bulletin tests. Pulled
    /// out as a helper so we can assert on real fields without standing
    /// up a `WorldDb` for each case.
    fn synthetic_event(id: i64, created_at: &str, kind: &str, message: &str) -> EventRecord {
        EventRecord {
            id,
            created_at: created_at.to_string(),
            kind: kind.to_string(),
            player_id: Some(1),
            message: message.to_string(),
            metadata: None,
        }
    }

    #[test]
    fn bulletin_format_row_extracts_time_slice() {
        // The kit stores `YYYY-MM-DD HH:MM:SS` text; the bulletin pulls
        // the time slice for compactness on an 80-column modal.
        let event = synthetic_event(
            1,
            "2026-05-09 12:34:56",
            "room_7_opened",
            "alice opened Room 7",
        );
        assert_eq!(
            BulletinScreen::format_row(&event),
            "[12:34:56] room_7_opened — alice opened Room 7"
        );
    }

    #[test]
    fn bulletin_format_row_falls_back_when_timestamp_lacks_space() {
        // Defensive: a hand-edited or future-format timestamp without
        // the canonical date/time split should still surface in the
        // modal so an operator can see what's there.
        let event = synthetic_event(1, "2026-05-09T12:34:56Z", "kind", "msg");
        assert_eq!(
            BulletinScreen::format_row(&event),
            "[2026-05-09T12:34:56Z] kind — msg"
        );
    }

    #[test]
    fn bulletin_empty_no_world_db_renders_hint() {
        // `from_world_db(None)` is the no-`[world]` / single-player
        // path. The screen must still render cleanly, with the
        // documented empty hint.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_world_db(None);
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
            found.contains(BulletinScreen::TITLE),
            "bulletin must paint its title; buffer was:\n{found}"
        );
        assert!(
            found.contains(BulletinScreen::EMPTY_HINT),
            "empty bulletin must surface the empty-state hint; buffer was:\n{found}"
        );
    }

    #[test]
    fn bulletin_renders_event_rows_into_test_backend() {
        // Two synthetic events, newest-first as `recent_events` would
        // return them. Both messages must appear in the modal.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(vec![
            synthetic_event(
                2,
                "2026-05-09 12:35:00",
                "clue_found",
                "alice took the matchbook",
            ),
            synthetic_event(
                1,
                "2026-05-09 12:34:56",
                "room_7_opened",
                "alice opened Room 7",
            ),
        ]);
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
        assert!(found.contains("clue_found"), "first row missing: {found}");
        assert!(
            found.contains("alice took the matchbook"),
            "msg missing: {found}"
        );
        assert!(
            found.contains("room_7_opened"),
            "second row missing: {found}"
        );
        assert!(
            !found.contains(BulletinScreen::EMPTY_HINT),
            "non-empty bulletin must not paint the empty hint: {found}"
        );
    }

    #[test]
    fn bulletin_esc_pops() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(Vec::new());
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Esc),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn bulletin_e_toggles_closed() {
        // Same muscle-memory contract as the inventory `i` toggle: the
        // key that opens the bulletin also closes it.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(Vec::new());
        for key in [Input::Char('e'), Input::Char('E'), Input::Backspace] {
            let mut s = BulletinScreen::from_events(Vec::new());
            assert!(
                matches!(s.handle_input(&mut ctx, key), ScreenCommand::Pop),
                "{key:?} should pop the bulletin"
            );
        }
        // Sanity: the loop above shadowed `screen` per-iteration; we
        // also need the original `screen` handle to still respond.
        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Char('e')),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn bulletin_quit_keys_quit() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(Vec::new());
        for key in [Input::Char('q'), Input::Char('Q'), Input::Ctrl('c')] {
            assert!(
                matches!(screen.handle_input(&mut ctx, key), ScreenCommand::Quit),
                "{key:?} should quit the bulletin"
            );
        }
    }

    #[test]
    fn bulletin_cursor_clamps() {
        // Selection cursor must not advance past the live row count
        // and must not underflow at zero — same contract as the
        // inventory clamp test.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(vec![
            synthetic_event(2, "2026-05-09 12:35:00", "k", "two"),
            synthetic_event(1, "2026-05-09 12:34:56", "k", "one"),
        ]);
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(screen.selected, 1, "cursor must clamp at last row");
        for _ in 0..10 {
            screen.handle_input(&mut ctx, Input::Up);
        }
        assert_eq!(screen.selected, 0, "cursor must clamp at top row");
    }

    #[test]
    fn bulletin_cursor_is_inert_when_empty() {
        // Down on an empty bulletin must not advance past zero — the
        // empty-state branch in render relies on `selected == 0`.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = BulletinScreen::from_events(Vec::new());
        screen.handle_input(&mut ctx, Input::Down);
        assert_eq!(screen.selected, 0);
    }

    #[test]
    fn bulletin_from_world_db_reads_recent_events() {
        // Stand up a real WorldDb, append two events, and confirm the
        // bulletin snapshot matches `recent_events`'s newest-first
        // ordering. This is the integration touch-point that proves
        // SPEC §Task 13d's "lists recent events" contract.
        use foglet_game::WorldDb;
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open world db");
        world
            .apply_migration(&foglet_game::PLAYERS_MIGRATION)
            .expect("apply players migration");
        world
            .apply_migration(&foglet_game::WORLD_EVENTS_MIGRATION)
            .expect("apply world_events migration");
        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) \
                 VALUES (NULL, 'alice', 'user', 50, \
                         CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed alice");
        let alice_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle = 'alice'", [], |r| {
                r.get(0)
            })
            .expect("read alice id");
        world
            .append_event("room_7_opened", Some(alice_id), "alice opened Room 7", None)
            .expect("append first event");
        world
            .append_event(
                "clue_found",
                Some(alice_id),
                "alice found a matchbook",
                None,
            )
            .expect("append second event");

        let screen = BulletinScreen::from_world_db(Some(&world));
        assert_eq!(
            screen.events.len(),
            2,
            "bulletin must surface both appended events"
        );
        // recent_events orders newest-first; the matchbook event was
        // appended second, so it should land at index 0.
        assert_eq!(screen.events[0].kind, "clue_found");
        assert_eq!(screen.events[1].kind, "room_7_opened");
    }

    // ---- LeaderboardScreen (SPEC_v2 §Task 13f) -------------------------

    /// Build a synthetic leaderboard row vector for tests that don't
    /// need to stand up a real `WorldDb`.
    fn leaderboard_rows(rows: &[(&str, i64)]) -> Vec<LeaderboardRow> {
        rows.iter()
            .enumerate()
            .map(|(idx, (handle, score))| LeaderboardRow {
                score: ScoreRecord {
                    board: "investigators".to_string(),
                    player_id: idx as i64 + 1,
                    score: *score,
                    updated_at: "2026-05-09 00:00:00".to_string(),
                },
                handle: (*handle).to_string(),
            })
            .collect()
    }

    /// Dispatch one input to a fresh leaderboard screen with no rows.
    fn dispatch_leaderboard(input: Input) -> ScreenCommand {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        LeaderboardScreen::from_rows(Vec::new()).handle_input(&mut ctx, input)
    }

    #[test]
    fn leaderboard_esc_pops() {
        // Esc closes the modal back to the menu beneath it — same Pop
        // contract every other modal honors so an accidental Esc never
        // drops the player out of the program.
        assert!(matches!(
            dispatch_leaderboard(Input::Esc),
            ScreenCommand::Pop
        ));
    }

    #[test]
    fn leaderboard_l_toggles_close() {
        // `l` opens the leaderboard from the menu; the same key closes
        // it from inside, mirroring the bulletin's `e` toggle.
        for input in [Input::Char('l'), Input::Char('L'), Input::Backspace] {
            assert!(
                matches!(dispatch_leaderboard(input), ScreenCommand::Pop),
                "{input:?} should pop the leaderboard"
            );
        }
    }

    #[test]
    fn leaderboard_q_still_quits() {
        // Hard-quit affordances stay live even inside the modal so a
        // player who lands here by accident can always get out.
        assert!(matches!(
            dispatch_leaderboard(Input::Char('q')),
            ScreenCommand::Quit
        ));
        assert!(matches!(
            dispatch_leaderboard(Input::Ctrl('c')),
            ScreenCommand::Quit
        ));
    }

    #[test]
    fn leaderboard_empty_renders_hint() {
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = LeaderboardScreen::from_rows(Vec::new());
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
            found.contains(LeaderboardScreen::EMPTY_HINT),
            "empty leaderboard must render the hint; buffer was:\n{found}"
        );
    }

    #[test]
    fn leaderboard_renders_rows_with_handles_and_scores() {
        let rows = leaderboard_rows(&[("alice", 5), ("bob", 3)]);
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = LeaderboardScreen::from_rows(rows);
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
        assert!(found.contains(LeaderboardScreen::TITLE));
        // Format is "<rank>. <handle>  <score>" — checking the rank
        // prefix together with the handle prevents a future regression
        // that decoupled the two.
        assert!(
            found.contains(" 1. alice"),
            "alice must render at rank 1; buffer was:\n{found}"
        );
        assert!(
            found.contains(" 2. bob"),
            "bob must render at rank 2; buffer was:\n{found}"
        );
        assert!(found.contains('5'));
    }

    #[test]
    fn leaderboard_format_row_pads_rank_and_separates_columns() {
        let rows = leaderboard_rows(&[("alice", 7)]);
        let line = LeaderboardScreen::format_row(1, &rows[0]);
        // Right-aligned 2-wide rank keeps the score column flush even
        // when the leaderboard exceeds nine entries — pinning the
        // format here catches a future tweak that flips alignment.
        assert_eq!(line, " 1. alice  7");
    }

    #[test]
    fn leaderboard_down_advances_cursor() {
        let rows = leaderboard_rows(&[("alice", 5), ("bob", 3), ("cleo", 1)]);
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = LeaderboardScreen::from_rows(rows);
        screen.handle_input(&mut ctx, Input::Down);
        assert_eq!(screen.selected, 1);
        screen.handle_input(&mut ctx, Input::Char('j'));
        assert_eq!(screen.selected, 2);
        // Past the end clamps rather than wraps — same contract as the
        // bulletin so muscle memory transfers.
        screen.handle_input(&mut ctx, Input::Down);
        assert_eq!(screen.selected, 2);
    }

    #[test]
    fn leaderboard_from_world_db_reads_top_scores_with_handles() {
        // End-to-end: seed two players + scores into a real `WorldDb`,
        // then prove the screen surfaces them with the right handles
        // and the right rank order. Locks in the `top_scores` →
        // `player_handle` plumbing and the descending-sort default.
        use foglet_game::{LEADERBOARD_SCORES_MIGRATION, PLAYERS_MIGRATION};
        let dir = tempfile::tempdir().expect("tempdir");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard migration");
        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) VALUES \
                 ('u1','alice','user',50,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP),\
                 ('u2','bob','user',50,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed players");
        let (alice_id, bob_id): (i64, i64) = world
            .connection()
            .query_row(
                "SELECT \
                 (SELECT id FROM players WHERE handle='alice'), \
                 (SELECT id FROM players WHERE handle='bob')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read ids");
        world
            .increment_score(crate::world::INVESTIGATORS_LEADERBOARD_NAME, alice_id, 5)
            .expect("score alice");
        world
            .increment_score(crate::world::INVESTIGATORS_LEADERBOARD_NAME, bob_id, 3)
            .expect("score bob");

        let screen = LeaderboardScreen::from_world_db(Some(&world));
        assert_eq!(
            screen.rows.len(),
            2,
            "both players should land on the board"
        );
        assert_eq!(screen.rows[0].handle, "alice");
        assert_eq!(screen.rows[0].score.score, 5);
        assert_eq!(screen.rows[1].handle, "bob");
        assert_eq!(screen.rows[1].score.score, 3);
    }

    #[test]
    fn leaderboard_from_world_db_with_none_yields_empty_screen() {
        // No `[world]` section, no DB — the screen must still construct
        // and surface the empty-state hint instead of panicking.
        let screen = LeaderboardScreen::from_world_db(None);
        assert!(screen.rows.is_empty());
    }

    #[test]
    fn leaderboard_from_world_db_uses_unknown_handle_for_orphan_player_id() {
        // If a leaderboard row references a player id whose row was
        // wiped (operator cleanup, partial restore), the screen should
        // still render the rank — handle just falls back to the
        // documented "unknown" placeholder.
        use foglet_game::{LEADERBOARD_SCORES_MIGRATION, PLAYERS_MIGRATION};
        let dir = tempfile::tempdir().expect("tempdir");
        let mut world = WorldDb::open(dir.path().join("world.sqlite")).expect("open");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration");
        world
            .apply_migration(&LEADERBOARD_SCORES_MIGRATION)
            .expect("leaderboard migration");
        // Seed a player + score, then drop the player row directly.
        // The kit configures `PRAGMA foreign_keys = ON`, so we toggle it
        // off for the manual delete to simulate the "operator wiped the
        // players row out from under us" scenario the screen has to
        // tolerate. This is a test-only escape hatch — production code
        // never bypasses the FK.
        world
            .connection()
            .execute(
                "INSERT INTO players (foglet_user_id, handle, role, security_level, \
                 first_seen_at, last_seen_at) VALUES \
                 ('orphan','ghost','user',50,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
                [],
            )
            .expect("seed orphan player");
        let orphan_id: i64 = world
            .connection()
            .query_row("SELECT id FROM players WHERE handle='ghost'", [], |r| {
                r.get(0)
            })
            .expect("read orphan id");
        world
            .increment_score(crate::world::INVESTIGATORS_LEADERBOARD_NAME, orphan_id, 4)
            .expect("score orphan");
        world
            .connection()
            .execute("PRAGMA foreign_keys = OFF", [])
            .expect("disable fk for orphan test");
        world
            .connection()
            .execute("DELETE FROM players WHERE id = ?1", [orphan_id])
            .expect("delete orphan player");

        let screen = LeaderboardScreen::from_world_db(Some(&world));
        assert_eq!(screen.rows.len(), 1);
        assert_eq!(screen.rows[0].handle, LeaderboardScreen::UNKNOWN_HANDLE);
        assert_eq!(screen.rows[0].score.score, 4);
    }
}
