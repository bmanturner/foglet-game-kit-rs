//! Simple shared widgets (SPEC §9 — Game Primitives).
//!
//! Three widgets every door game ends up needing — pulled into the
//! library so authors don't reinvent them and so the visual baseline
//! stays consistent across games shipped with the kit:
//!
//! - [`MenuList`]   — vertical list of choices with a single selection
//!   marker. Used for title menus, dialog choices, and any "pick one"
//!   prompt.
//! - [`InventoryList`] — vertical list of player-held items, with an
//!   explicit empty-state hint so an empty inventory doesn't render as
//!   a blank box.
//! - [`MessageLine`] — single-line status / hint / error display,
//!   styled by [`MessageKind`].
//!
//! # Why widgets, not screens
//!
//! Per SPEC §8.2 a `Screen` owns input handling. These widgets are the
//! opposite: pure render helpers that take a `ratatui::Frame` and a
//! state struct, write into a `Rect`, and return. They never read
//! input, never push or pop screens, and never mutate game state.
//! Authors compose them inside their own `Screen::render` methods,
//! drive selection / contents from their screen state, and dispatch
//! input through the screen trait.
//!
//! # Why no internal state
//!
//! Widget state lives in the calling screen so the screen can persist
//! it across frames, save it into the save file if it wants to, and
//! react to input by mutating it directly. Storing the selected index
//! inside the widget would force every screen to thread a `&mut
//! Widget` through render and input paths for no benefit.
//!
//! # Test strategy
//!
//! Each widget renders into a `ratatui::backend::TestBackend` and the
//! resulting buffer is asserted line-by-line. This is closer to a
//! pixel test than a snapshot — it catches the things that actually
//! matter (selection marker is on the right row; empty state is shown;
//! truncation works) without locking in incidental whitespace.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;

/// Marker drawn next to the selected row in a [`MenuList`] or
/// [`InventoryList`]. Two characters wide so the "selected" row stays
/// visually distinct without needing colour support — important for
/// the BBS audience where colour rendering varies per client.
const SELECTION_MARKER: &str = "> ";

/// Same width as [`SELECTION_MARKER`] so unselected rows align
/// vertically with the selected one.
const SELECTION_GUTTER: &str = "  ";

/// A vertical list of selectable options, e.g. the title-screen
/// "New game / Continue / Quit" menu.
///
/// State is owned by the caller — typically a `Screen` storing the
/// items vector and a `selected: usize` cursor. Render is pure: pass
/// the borrowed state to [`render_menu_list`] each frame.
///
/// `selected` is clamped to `items.len().saturating_sub(1)` at render
/// time, so callers don't need to defensively bounds-check before
/// rendering. An empty `items` vector is allowed and renders to just
/// the bordered title.
#[derive(Debug, Clone)]
pub struct MenuList<'a> {
    /// Title rendered as the block's top-border label. `None` hides
    /// the title row but keeps the border.
    pub title: Option<&'a str>,
    /// Choice labels, top-to-bottom. Each entry occupies one row
    /// (truncation at right edge if wider than the available area).
    pub items: &'a [String],
    /// Index of the currently highlighted row. Out-of-range values
    /// are clamped — callers don't have to guard against an empty
    /// list.
    pub selected: usize,
}

/// A list of player-held items with an empty-state hint.
///
/// Distinct from [`MenuList`] only in its empty-state behaviour: an
/// empty inventory renders the configured `empty_hint` string instead
/// of a blank box, so players never see "did the screen break?"
/// ambiguity.
#[derive(Debug, Clone)]
pub struct InventoryList<'a> {
    /// Title rendered as the block's top-border label.
    pub title: Option<&'a str>,
    /// Visible item labels (game decides whether to include
    /// quantities, descriptions, etc.).
    pub items: &'a [String],
    /// Index of the currently highlighted row. Ignored when `items`
    /// is empty.
    pub selected: usize,
    /// Centered text shown when `items` is empty, e.g.
    /// `"(nothing in your pockets)"`. The whole point of distinguishing
    /// inventory from a generic menu — empty rendering shouldn't be
    /// silent.
    pub empty_hint: &'a str,
}

/// Severity classification for [`MessageLine`]. Drives the foreground
/// colour and the text prefix so monochrome BBS clients can still tell
/// the kinds apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Neutral status update. No prefix; default terminal colour.
    Info,
    /// User-facing nudge ("press ? for help"). Cyan + `[?]` prefix.
    Hint,
    /// Recoverable error visible to the player (e.g. "door is locked").
    /// Red + `[!]` prefix. Distinct from `ScreenCommand::Error`, which
    /// is the engine's fatal-exit channel.
    Error,
}

/// Single-line status / hint / error rendered into a one-row area.
///
/// The widget intentionally does not wrap or scroll — the caller picks
/// a one-row `Rect` and the widget truncates if the message overflows.
/// Multi-line messaging is a screen-level concern and should compose
/// `MessageLine` with a `Paragraph`.
#[derive(Debug, Clone)]
pub struct MessageLine<'a> {
    /// Severity tag. Drives the visual style and prefix.
    pub kind: MessageKind,
    /// Message body. Truncated at the right edge if it doesn't fit.
    pub text: &'a str,
}

/// Render a [`MenuList`] into `area`. Pure: state is borrowed,
/// nothing is mutated.
///
/// Rows that don't fit are dropped silently — callers must size
/// `area` to at least `2 + items.len()` rows (top + bottom border +
/// one row per item) to guarantee no truncation. Width is similarly
/// the caller's responsibility; long labels are right-truncated.
pub fn render_menu_list(frame: &mut Frame<'_>, area: Rect, menu: &MenuList<'_>) {
    let block = bordered_block(menu.title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    draw_selectable_rows(frame.buffer_mut(), inner, menu.items, menu.selected);
}

/// Render an [`InventoryList`] into `area`.
///
/// When `items` is empty, the widget centres `empty_hint` inside the
/// inner area (single-row vertical centre, horizontal centre).
/// Otherwise it behaves identically to [`render_menu_list`].
pub fn render_inventory_list(frame: &mut Frame<'_>, area: Rect, inv: &InventoryList<'_>) {
    let block = bordered_block(inv.title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inv.items.is_empty() {
        // Centre the empty-state hint so it's visually distinct from
        // a populated list (which is always left-aligned with a
        // selection gutter).
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let para = Paragraph::new(inv.empty_hint)
            .style(Style::default().add_modifier(Modifier::DIM))
            .alignment(ratatui::layout::Alignment::Center);
        // One-row band in the vertical middle of the inner area.
        let row = inner.y + inner.height / 2;
        let band = Rect {
            x: inner.x,
            y: row,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(para, band);
        return;
    }

    draw_selectable_rows(frame.buffer_mut(), inner, inv.items, inv.selected);
}

/// Render a [`MessageLine`] into `area`. `area.height` should be 1;
/// extra rows are left blank.
pub fn render_message_line(frame: &mut Frame<'_>, area: Rect, msg: &MessageLine<'_>) {
    let (prefix, style) = match msg.kind {
        MessageKind::Info => ("", Style::default()),
        MessageKind::Hint => ("[?] ", Style::default().fg(Color::Cyan)),
        MessageKind::Error => (
            "[!] ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };
    let line = Line::from(vec![
        Span::styled(prefix, style),
        Span::styled(msg.text, style),
    ]);
    // `Paragraph` handles right-truncation when the text overflows
    // the area width, which is what we want for a status line.
    let para = Paragraph::new(line);
    let band = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height.min(1),
    };
    frame.render_widget(para, band);
}

/// Build the title block shared by [`render_menu_list`] and
/// [`render_inventory_list`]. Extracted so the two widgets stay
/// visually consistent without copy-paste drift.
fn bordered_block(title: Option<&str>) -> Block<'_> {
    let mut block = Block::default().borders(Borders::ALL);
    if let Some(t) = title {
        block = block.title(t);
    }
    block
}

/// Draw a selectable list directly into the buffer.
///
/// Inlines the marker / gutter rendering instead of using
/// `ratatui::widgets::List` so:
///
/// - The selection marker is plain ASCII (`> `), readable on any
///   terminal regardless of colour support.
/// - We can clamp `selected` defensively without forcing the caller
///   through `ListState::select(Some(i))`.
fn draw_selectable_rows(buf: &mut Buffer, area: Rect, items: &[String], selected: usize) {
    if area.width == 0 || area.height == 0 || items.is_empty() {
        return;
    }
    // Clamp so an out-of-range `selected` never panics or silently
    // highlights nothing — the call site sees the cursor on the last
    // row, which matches "selection didn't update" UX expectations.
    let clamped = selected.min(items.len() - 1);
    let visible = (area.height as usize).min(items.len());
    for (row, item) in items.iter().take(visible).enumerate() {
        let y = area.y + row as u16;
        let is_selected = row == clamped;
        let (marker, style) = if is_selected {
            (
                SELECTION_MARKER,
                Style::default().add_modifier(Modifier::REVERSED),
            )
        } else {
            (SELECTION_GUTTER, Style::default())
        };
        // Build a single-row line so `ratatui` handles its own
        // right-truncation when the label overflows `area.width`.
        let line = Line::from(vec![
            Span::styled(marker, style),
            Span::styled(item.as_str(), style),
        ]);
        let row_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        Paragraph::new(line).render(row_area, buf);
    }
}

#[cfg(test)]
mod tests {
    //! Buffer-level assertions against `TestBackend`. Each test draws
    //! one widget into a known-size terminal and inspects the rendered
    //! cells — string content for layout, `Style` for colour /
    //! emphasis. Keeps the assertions tight enough to catch regressions
    //! (selection on wrong row, missing empty hint, dropped prefix)
    //! without breaking on incidental whitespace.

    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Collect the printable content of a row as a trimmed string.
    /// Used to assert layout without locking in trailing spaces.
    fn row_text(buf: &Buffer, y: u16) -> String {
        let mut s = String::new();
        for x in 0..buf.area.width {
            s.push_str(buf.cell((x, y)).expect("cell in bounds").symbol());
        }
        s.trim_end().to_string()
    }

    fn full_area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn draw<F: FnOnce(&mut Frame<'_>)>(width: u16, height: u16, f: F) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(width, height)).expect("test backend");
        term.draw(|frame| f(frame)).expect("draw");
        term.backend().buffer().clone()
    }

    // ---- MenuList ---------------------------------------------------

    #[test]
    fn menu_list_renders_title_and_items() {
        let items = vec!["New game".into(), "Continue".into(), "Quit".into()];
        let menu = MenuList {
            title: Some("Main"),
            items: &items,
            selected: 0,
        };
        let buf = draw(20, 5, |f| render_menu_list(f, full_area(20, 5), &menu));
        // Top border carries the title.
        assert!(row_text(&buf, 0).contains("Main"));
        // Inner rows are y=1..=3.
        assert_eq!(row_text(&buf, 1), "│> New game        │");
        assert_eq!(row_text(&buf, 2), "│  Continue        │");
        assert_eq!(row_text(&buf, 3), "│  Quit            │");
    }

    #[test]
    fn menu_list_marks_selected_row() {
        let items = vec!["a".into(), "b".into(), "c".into()];
        let menu = MenuList {
            title: None,
            items: &items,
            selected: 2,
        };
        let buf = draw(10, 5, |f| render_menu_list(f, full_area(10, 5), &menu));
        assert_eq!(row_text(&buf, 1), "│  a     │");
        assert_eq!(row_text(&buf, 2), "│  b     │");
        assert_eq!(row_text(&buf, 3), "│> c     │");
    }

    #[test]
    fn menu_list_clamps_out_of_range_selection() {
        let items = vec!["a".into(), "b".into()];
        let menu = MenuList {
            title: None,
            items: &items,
            selected: 99,
        };
        let buf = draw(10, 4, |f| render_menu_list(f, full_area(10, 4), &menu));
        // Last item gets the marker even though `selected` was
        // out-of-range; this matches "your cursor stuck on the last
        // valid row" UX rather than crashing.
        assert_eq!(row_text(&buf, 1), "│  a     │");
        assert_eq!(row_text(&buf, 2), "│> b     │");
    }

    #[test]
    fn menu_list_handles_empty_items() {
        let items: Vec<String> = vec![];
        let menu = MenuList {
            title: Some("Empty"),
            items: &items,
            selected: 0,
        };
        // Should not panic and should render just the border + title.
        let buf = draw(10, 4, |f| render_menu_list(f, full_area(10, 4), &menu));
        assert!(row_text(&buf, 0).contains("Empty"));
        assert_eq!(row_text(&buf, 1), "│        │");
    }

    #[test]
    fn menu_list_selected_row_uses_reversed_style() {
        let items = vec!["only".into()];
        let menu = MenuList {
            title: None,
            items: &items,
            selected: 0,
        };
        let buf = draw(10, 3, |f| render_menu_list(f, full_area(10, 3), &menu));
        // The marker cell at column 1, row 1 is part of the selected
        // row; check it carries the REVERSED modifier.
        let cell = buf.cell((1, 1)).expect("cell in bounds");
        assert!(
            cell.style().add_modifier.contains(Modifier::REVERSED),
            "selected row should be REVERSED, got {:?}",
            cell.style()
        );
        // Row 0 (top border) should not carry REVERSED.
        let border = buf.cell((1, 0)).expect("cell in bounds");
        assert!(!border.style().add_modifier.contains(Modifier::REVERSED));
    }

    // ---- InventoryList ----------------------------------------------

    #[test]
    fn inventory_list_with_items_renders_like_menu() {
        let items = vec!["key".into(), "lantern".into()];
        let inv = InventoryList {
            title: Some("Pack"),
            items: &items,
            selected: 1,
            empty_hint: "(empty)",
        };
        let buf = draw(15, 5, |f| render_inventory_list(f, full_area(15, 5), &inv));
        assert!(row_text(&buf, 0).contains("Pack"));
        assert_eq!(row_text(&buf, 1), "│  key        │");
        assert_eq!(row_text(&buf, 2), "│> lantern    │");
    }

    #[test]
    fn inventory_list_empty_centers_the_hint() {
        let items: Vec<String> = vec![];
        let inv = InventoryList {
            title: Some("Pack"),
            items: &items,
            selected: 0,
            empty_hint: "(empty)",
        };
        let buf = draw(20, 7, |f| render_inventory_list(f, full_area(20, 7), &inv));
        // Inner area is rows 1..=5, so vertical middle is row 1 + 5/2 = 3.
        let middle = row_text(&buf, 3);
        assert!(
            middle.contains("(empty)"),
            "expected '(empty)' on middle row, got {middle:?}"
        );
        // First inner row should be blank rather than showing the hint.
        let first = row_text(&buf, 1);
        assert!(!first.contains("(empty)"), "hint should not be on row 1");
    }

    #[test]
    fn inventory_list_empty_hint_uses_dim_style() {
        let items: Vec<String> = vec![];
        let inv = InventoryList {
            title: None,
            items: &items,
            selected: 0,
            empty_hint: "x",
        };
        let buf = draw(10, 5, |f| render_inventory_list(f, full_area(10, 5), &inv));
        // Find the cell containing the 'x' and assert DIM.
        let mut found = false;
        for x in 0..buf.area.width {
            for y in 0..buf.area.height {
                let cell = buf.cell((x, y)).expect("cell");
                if cell.symbol() == "x" {
                    assert!(
                        cell.style().add_modifier.contains(Modifier::DIM),
                        "empty hint should be DIM"
                    );
                    found = true;
                }
            }
        }
        assert!(found, "did not find rendered empty hint");
    }

    // ---- MessageLine ------------------------------------------------

    #[test]
    fn message_line_info_has_no_prefix() {
        let msg = MessageLine {
            kind: MessageKind::Info,
            text: "ok",
        };
        let buf = draw(10, 1, |f| render_message_line(f, full_area(10, 1), &msg));
        assert_eq!(row_text(&buf, 0), "ok");
    }

    #[test]
    fn message_line_hint_prefixes_and_colours() {
        let msg = MessageLine {
            kind: MessageKind::Hint,
            text: "press ?",
        };
        let buf = draw(15, 1, |f| render_message_line(f, full_area(15, 1), &msg));
        assert_eq!(row_text(&buf, 0), "[?] press ?");
        let cell = buf.cell((0, 0)).expect("cell");
        assert_eq!(cell.style().fg, Some(Color::Cyan));
    }

    #[test]
    fn message_line_error_prefixes_and_colours_bold() {
        let msg = MessageLine {
            kind: MessageKind::Error,
            text: "locked",
        };
        let buf = draw(15, 1, |f| render_message_line(f, full_area(15, 1), &msg));
        assert_eq!(row_text(&buf, 0), "[!] locked");
        let cell = buf.cell((0, 0)).expect("cell");
        assert_eq!(cell.style().fg, Some(Color::Red));
        assert!(cell.style().add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn message_line_truncates_to_area_width() {
        let msg = MessageLine {
            kind: MessageKind::Info,
            text: "this message is far too long for the area",
        };
        let buf = draw(8, 1, |f| render_message_line(f, full_area(8, 1), &msg));
        let row = row_text(&buf, 0);
        assert_eq!(row.len(), 8, "expected exactly 8 chars, got {row:?}");
        assert!(row.starts_with("this mes"));
    }

    #[test]
    fn message_line_ignores_extra_rows() {
        let msg = MessageLine {
            kind: MessageKind::Info,
            text: "one",
        };
        // Allocate a 3-row area; widget should only paint row 0.
        let buf = draw(10, 3, |f| render_message_line(f, full_area(10, 3), &msg));
        assert_eq!(row_text(&buf, 0), "one");
        assert_eq!(row_text(&buf, 1), "");
        assert_eq!(row_text(&buf, 2), "");
    }
}
