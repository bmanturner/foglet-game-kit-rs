//! Room 7 — the second map of the Murder Motel example.
//!
//! Reached only by climbing the stairs at
//! [`crate::map::MapScreen::STAIRS_UP_POS`] with
//! [`crate::map::MapScreen::ROOM_7_KEY_ID`] in inventory. Demonstrates
//! two patterns the lobby on its own can't:
//!
//! 1. **Map-to-map travel** via [`ScreenCommand::Replace`] — stepping
//!    onto the stairs-down cell swaps this screen for a fresh
//!    [`MapScreen`] handed the same [`SharedSlots`], so flags,
//!    inventory, and player coordinates persist across the transition.
//! 2. **Save-state map awareness** — the constructor writes
//!    [`Self::MAP_NAME`] into [`SharedSlots::map_name`] so a snapshot
//!    taken from this screen records the right identifier; the title
//!    screen's Continue path reads it back to dispatch a resumed run
//!    onto Room 7 instead of always pushing the lobby.
//!
//! Room 7 is intentionally small: a single bordered room with a
//! stairs-down tile back to the lobby and a body tile that emits a
//! flavour feedback line. No NPCs, items, or dialogs — the lesson is
//! the transition mechanism, not the roster surface.
use std::rc::Rc;

use foglet_game::{
    parse_map, FeedbackLine, GameContext, Input, Map, Screen, ScreenCommand, TileLegend,
};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::map::MapScreen;
use crate::modals::InventoryScreen;
use crate::state::SharedSlots;

/// Room 7 ASCII map embedded at compile time. Same rationale as
/// [`crate::map::LOBBY_MAP_TEXT`]: keeping the asset inside the binary
/// means `cargo run --example` works regardless of CWD.
const ROOM_7_MAP_TEXT: &str = include_str!("../assets/maps/room_7.txt");

/// Map screen for Room 7. Mirrors the shape of
/// [`crate::map::MapScreen`] but with a smaller surface — no NPC
/// roster, item catalog, or dialog plumbing because the room is a
/// flavour stop, not a content hub.
pub struct Room7Screen {
    /// Parsed Room 7 map, immutable for the screen's lifetime.
    map: Map,
    /// Same shared slots the lobby holds — the two screens borrow the
    /// identical `Rc`s, which is why every flag, item, and the player
    /// position survive the [`ScreenCommand::Replace`] transition.
    slots: SharedSlots,
}

impl Room7Screen {
    /// Title rendered on the bordered block surrounding the map.
    pub const TITLE: &'static str = "Murder Motel — Room 7";

    /// Glyph used to draw the player. Pulled from the kit-wide constant
    /// so the convention stays in lockstep with the lobby's player
    /// glyph and any future map screens.
    pub const PLAYER_GLYPH: char = foglet_game::PLAYER_GLYPH;

    /// Identifier this screen writes into [`SharedSlots::map_name`] on
    /// construction. Read by [`crate::title_menu`] to dispatch a
    /// Continue load back onto Room 7 when the saved run was here.
    pub const MAP_NAME: &'static str = "room_7";

    /// Cell the player spawns into when arriving from the lobby. One
    /// step east of the stairs-down so the first directional input
    /// after arrival doesn't immediately re-trigger the transition.
    pub const ARRIVAL_POS: (u16, u16) = (5, 2);

    /// Stairs-down cell — stepping onto it swaps this screen for a
    /// fresh [`MapScreen`] that resumes from the same shared slots.
    pub const STAIRS_DOWN_POS: (u16, u16) = (4, 2);

    /// Glyph for the stairs-down cell. `<` matches the roguelike
    /// convention for "stairs going down to the level you came from."
    pub const STAIRS_DOWN_GLYPH: char = '<';

    /// Body tile — stepping on it emits a one-line flavour message
    /// through the shared feedback slot. Pure narration; no flag is
    /// set and the win path is unaffected.
    pub const BODY_POS: (u16, u16) = (9, 4);

    /// Glyph for the body tile.
    pub const BODY_GLYPH: char = 'b';

    /// Flavour line written to [`SharedSlots::feedback`] when the
    /// player first steps onto [`Self::BODY_POS`]. Lives as a constant
    /// so tests can assert on the message without re-typing it.
    pub const BODY_FEEDBACK: &'static str =
        "The body is sprawled across the bed. The matchbook in your pocket suddenly feels heavier.";

    /// One-line movement hint shown below the map. Room 7 has no Talk,
    /// Buy, or Search affordances — the only verbs are Move, Inv,
    /// Back, and Quit — so the hint is a static string instead of the
    /// contextual builder the lobby uses.
    pub const HINT_LINE: &'static str = "Move: arrows/hjkl  Inv: I  Back: Esc  Quit: Q";

    /// Build a Room 7 screen sharing the supplied [`SharedSlots`]. The
    /// player position is honoured from the slots if it lands on a
    /// walkable Room 7 cell (resumed save), otherwise the player is
    /// dropped at [`Self::ARRIVAL_POS`] (fresh transition from the
    /// lobby). Either way the slots' `map_name` is updated to
    /// [`Self::MAP_NAME`] so the next snapshot records this screen.
    pub fn with_shared(slots: SharedSlots) -> Self {
        let legend = room_7_legend();
        let map =
            parse_map(ROOM_7_MAP_TEXT, &legend).expect("embedded room_7 map parses against legend");
        let screen = Self { map, slots };
        // Mark the slots as "we are now on Room 7" so a save snapshot
        // taken before the next transition lands on the right map.
        *screen.slots.map_name.borrow_mut() = Self::MAP_NAME.to_string();
        // Decide spawn cell. If the slots' saved position lands on a
        // walkable Room 7 cell we honour it (resumed save); otherwise
        // we drop the player at the canonical arrival cell. The
        // walkability check is a self-correcting safety net — a
        // player whose saved coords landed on a wall after a re-author
        // ends up at the arrival cell instead of stranded.
        let saved = {
            let p = screen.slots.player.borrow();
            (p.x, p.y)
        };
        let (cx, cy) = if screen.map.is_walkable(saved.0, saved.1) {
            saved
        } else {
            Self::ARRIVAL_POS
        };
        let mut p = screen.slots.player.borrow_mut();
        p.x = cx;
        p.y = cy;
        drop(p);
        screen
    }

    /// Player coordinates. Exposed so tests can read the position
    /// without poking at private state.
    pub fn player(&self) -> (u16, u16) {
        let p = self.slots.player.borrow();
        (p.x, p.y)
    }

    /// Attempt to move the player by `(dx, dy)` (each ±1). Returns
    /// `true` if the step succeeded. Walkability gates the move; Room
    /// 7 has no NPC, item, or locked-door layers so the body of this
    /// function is much shorter than the lobby's analogue.
    pub fn try_move(&mut self, dx: i32, dy: i32) -> bool {
        let (cur_x, cur_y) = self.player();
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
        let mut p = self.slots.player.borrow_mut();
        p.x = target_x;
        p.y = target_y;
        true
    }

    /// Whether the player is standing on the body cell. Centralised so
    /// render and the feedback emitter consult one source of truth.
    pub fn standing_on_body(&self) -> bool {
        self.player() == Self::BODY_POS
    }

    /// Whether the player is standing on the stairs-down cell.
    pub fn standing_on_stairs(&self) -> bool {
        self.player() == Self::STAIRS_DOWN_POS
    }

    /// Update the shared feedback slot if the player just stepped onto
    /// the body cell. Safe to call after every move — when the player
    /// is elsewhere it does nothing, so a later wall bump does not
    /// erase the line.
    fn maybe_emit_body_feedback(&self) {
        if !self.standing_on_body() {
            return;
        }
        *self.slots.feedback.borrow_mut() = Some(FeedbackLine::info(Self::BODY_FEEDBACK));
    }

    /// If the player just stepped onto the stairs-down cell, build the
    /// [`ScreenCommand::Replace`] that swaps Room 7 for a fresh
    /// [`MapScreen`] sharing the same slots. Sets the player position
    /// to [`MapScreen::LOBBY_ARRIVAL_POS`] before constructing the
    /// lobby — its `with_shared` honours saved coordinates that land
    /// on a walkable cell, and Room 7's stairs-down coordinate (4, 2)
    /// happens to be a walkable lobby cell too (the Bellhop's spawn).
    /// Without this nudge the player would arrive in the lobby on
    /// top of the Bellhop. Also clears the feedback slot so the lobby
    /// opens with a clean narration line instead of stale Room 7
    /// flavor text.
    fn maybe_take_stairs(&self) -> Option<ScreenCommand> {
        if !self.standing_on_stairs() {
            return None;
        }
        {
            let mut p = self.slots.player.borrow_mut();
            p.x = MapScreen::LOBBY_ARRIVAL_POS.0;
            p.y = MapScreen::LOBBY_ARRIVAL_POS.1;
        }
        *self.slots.feedback.borrow_mut() = None;
        // The `(0, 0)` start args are ignored when the slots carry
        // non-zero coordinates (which we just set above).
        Some(ScreenCommand::Replace(Box::new(MapScreen::with_shared(
            0,
            0,
            self.slots.clone(),
        ))))
    }

    /// Run the post-move handlers. Order matters: emit feedback first
    /// (so the next render shows the line even if the screen is about
    /// to be replaced and never gets a frame), then check for the
    /// transition.
    fn after_move(&mut self) -> ScreenCommand {
        self.maybe_emit_body_feedback();
        self.maybe_take_stairs().unwrap_or(ScreenCommand::None)
    }
}

impl Screen for Room7Screen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Centre the map inside the frame, same rule the lobby follows.
        let area =
            crate::layout::centred_rect(self.map.width + 2, self.map.height + 2, frame.area());

        let player_glyph_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let stairs_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);

        let (px, py) = self.player();
        let mut lines: Vec<Line<'_>> = Vec::with_capacity(self.map.cells.len());
        for (y, row) in self.map.cells.iter().enumerate() {
            let row_string: String = row.iter().map(|t| t.glyph).collect();
            let mut spans: Vec<Span<'_>> = Vec::with_capacity(row.len());
            for (x, ch) in row_string.chars().enumerate() {
                if (y as u16) == py && (x as u16) == px {
                    spans.push(Span::styled(
                        Self::PLAYER_GLYPH.to_string(),
                        player_glyph_style,
                    ));
                    continue;
                }
                if (x as u16, y as u16) == Self::STAIRS_DOWN_POS {
                    spans.push(Span::styled(
                        Self::STAIRS_DOWN_GLYPH.to_string(),
                        stairs_style,
                    ));
                    continue;
                }
                if (x as u16, y as u16) == Self::BODY_POS {
                    spans.push(Span::styled(Self::BODY_GLYPH.to_string(), body_style));
                    continue;
                }
                spans.push(Span::raw(ch.to_string()));
            }
            lines.push(Line::from(spans));
        }

        let widget = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title(Self::TITLE));
        frame.render_widget(widget, area);

        // Hint line directly below the map block.
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

        // Feedback line one row below the hint, mirroring the lobby's
        // layout so the body's flavour message lands in a consistent
        // visual slot regardless of which map the player is on.
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
            Input::Char('q') | Input::Char('Q') | Input::Ctrl('c') => ScreenCommand::Quit,
            Input::Up | Input::Char('k') | Input::Char('K') => {
                self.try_move(0, -1);
                self.after_move()
            }
            Input::Down | Input::Char('j') | Input::Char('J') => {
                self.try_move(0, 1);
                self.after_move()
            }
            Input::Left | Input::Char('h') => {
                self.try_move(-1, 0);
                self.after_move()
            }
            Input::Right | Input::Char('l') | Input::Char('L') => {
                self.try_move(1, 0);
                self.after_move()
            }
            Input::Char('i') | Input::Char('I') => ScreenCommand::Push(Box::new(
                InventoryScreen::new(Rc::clone(&self.slots.inventory)),
            )),
            _ => ScreenCommand::None,
        }
    }
}

/// Tile legend for Room 7. Custom tiles default to walkable in the kit
/// (`TileKind::Custom`), so `<` and `b` both pass [`Map::is_walkable`]
/// and the screen layers its own behaviour on top via the post-move
/// handlers.
fn room_7_legend() -> TileLegend {
    TileLegend::from_pairs([
        ("#", "wall"),
        (" ", "floor"),
        ("<", "stairs_down"),
        ("b", "body"),
    ])
    .expect("static room_7 legend parses")
}

#[cfg(test)]
mod tests {
    //! Tests for Room 7's spawn, movement, body feedback, and the
    //! transition back to the lobby.

    use super::*;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::GameContext;

    fn fresh_room_7() -> Room7Screen {
        Room7Screen::with_shared(SharedSlots::default())
    }

    #[test]
    fn room_7_spawn_lands_at_arrival_pos_when_slots_are_default() {
        // Default slots carry (0, 0) for the player — not a walkable
        // Room 7 cell — so the constructor must drop the player at
        // ARRIVAL_POS instead of leaving them on a wall.
        let screen = fresh_room_7();
        assert_eq!(screen.player(), Room7Screen::ARRIVAL_POS);
    }

    #[test]
    fn room_7_constructor_writes_map_name_into_slots() {
        // Save snapshots taken from this screen must record "room_7"
        // — the title screen's Continue path keys on this string to
        // dispatch a resumed run.
        let slots = SharedSlots::default();
        let _screen = Room7Screen::with_shared(slots.clone());
        assert_eq!(slots.map_name.borrow().as_str(), Room7Screen::MAP_NAME);
    }

    #[test]
    fn room_7_walls_block_movement() {
        // The room is a 20x7 bordered rectangle. From spawn (5, 2) we
        // route DOWN first (away from the stairs cell at (4, 2) so the
        // step never re-triggers the transition), then west, and pin
        // the left wall at col 0 by attempting to step into it.
        let mut screen = fresh_room_7();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        // Drop to row 3 (a floor row with no special tiles) so left-
        // ward steps don't bump the stairs at (4, 2).
        screen.handle_input(&mut ctx, Input::Down);
        assert_eq!(screen.player(), (5, 3));
        // Walk west until the wall stops us. The interior runs from
        // col 1 to col 18; col 0 is the wall. Five Lefts from x=5
        // put us at x=0... no — first Left is (4, 3), then (3, 3),
        // (2, 3), (1, 3), and the fifth attempt (0, 3) is the wall.
        for _ in 0..5 {
            screen.handle_input(&mut ctx, Input::Left);
        }
        assert_eq!(
            screen.player(),
            (1, 3),
            "left wall at col 0 must clamp the player at col 1"
        );
        // One more attempted step left is also blocked.
        screen.handle_input(&mut ctx, Input::Left);
        assert_eq!(screen.player(), (1, 3));
    }

    #[test]
    fn step_onto_stairs_emits_replace_back_to_lobby() {
        // Walk west from spawn (5, 2) to (4, 2) — the stairs cell.
        // The handle_input for the move must return Replace; the
        // post-move handler also re-points the slots' player position
        // at MapScreen::LOBBY_ARRIVAL_POS *before* returning, so by
        // the time the caller reads `screen.player()` the slots
        // already reflect the lobby's arrival cell, not the stairs.
        let mut screen = fresh_room_7();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Left);
        assert!(
            matches!(cmd, ScreenCommand::Replace(_)),
            "stepping onto stairs-down must emit Replace; got {cmd:?}"
        );
        assert_eq!(
            screen.player(),
            MapScreen::LOBBY_ARRIVAL_POS,
            "post-transition slots must point at the lobby's arrival cell"
        );
    }

    #[test]
    fn step_onto_body_writes_feedback_line() {
        // Walk from spawn (5, 2) to the body at (9, 4): right four
        // times, down twice. After the final step the shared feedback
        // slot must hold BODY_FEEDBACK.
        let mut screen = fresh_room_7();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        for _ in 0..4 {
            screen.handle_input(&mut ctx, Input::Right);
        }
        for _ in 0..2 {
            screen.handle_input(&mut ctx, Input::Down);
        }
        assert_eq!(screen.player(), Room7Screen::BODY_POS);
        let feedback = screen
            .slots
            .feedback
            .borrow()
            .clone()
            .expect("body step must populate feedback slot");
        assert_eq!(
            feedback.rendered_text(),
            Room7Screen::BODY_FEEDBACK,
            "body feedback must match the documented constant"
        );
    }

    #[test]
    fn resumed_save_with_room_7_position_lands_on_saved_cell() {
        // A save taken while the player stood on (8, 3) of Room 7
        // must restore the player to (8, 3), not the arrival cell.
        let slots = SharedSlots::default();
        {
            let mut p = slots.player.borrow_mut();
            p.x = 8;
            p.y = 3;
        }
        let screen = Room7Screen::with_shared(slots);
        assert_eq!(screen.player(), (8, 3));
    }

    #[test]
    fn quit_key_exits_runtime() {
        let mut screen = fresh_room_7();
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('q'));
        assert!(matches!(cmd, ScreenCommand::Quit));
    }
}
