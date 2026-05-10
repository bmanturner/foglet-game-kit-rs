//! `JobBoardScreen` — reusable v5 Job Board list surface.
//!
//! This screen renders aggregated [`crate::job_board::JobBoardEntry`]
//! rows into a terminal-friendly table so games can present one list
//! even when opportunities come from different primitives.
//!
//! The screen is deliberately genre-neutral:
//!
//! - A **space exploration** game can render freight contracts and
//!   challenge races from multiple orbital stations.
//! - A **dungeon crawler** can render guild commissions and rival
//!   bounty postings from multiple town districts.
//!
//! Task 6a lands the render baseline, while Task 6b adds keyboard
//! navigation, pagination, and a quit hotkey. Detail modals and
//! accept/claim callbacks arrive in follow-up tasks.

use std::fmt;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::input::Input;
use crate::job_board::JobBoardEntry;
use crate::screen::{GameContext, Screen, ScreenCommand};
use crate::widgets::{centred_rect, render_modal};

/// Callback signature used by [`JobBoardScreen`] to render detail-body
/// text for one selected [`JobBoardEntry`].
///
/// The callback keeps gameplay semantics in game code:
///
/// - A **space exploration** game can return a cargo manifest and dock
///   contact instructions.
/// - A **dungeon crawler** can return a patron note plus hazard hints.
type DetailBodyCallback = Arc<dyn Fn(&JobBoardEntry) -> String + 'static>;

/// Captured detail modal snapshot for the currently selected entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailModal {
    title: String,
    body: String,
}

/// Screen rendering an aggregated Job Board table.
///
/// The screen owns a snapshot of entries so callers can render exactly
/// the data they queried in that frame:
///
/// - In a **space exploration** game the caller can refresh this list
///   each time a station terminal is opened.
/// - In a **town simulation** game the caller can refresh after a town
///   board clerk posts new work orders.
///
/// This render-only baseline paints a fixed table shape with four
/// visible columns (`kind_label`, `state_label`, `title`, `expires_at`)
/// inside a bordered 80×24-safe layout.
#[derive(Clone)]
pub struct JobBoardScreen {
    title: String,
    entries: Vec<JobBoardEntry>,
    empty_state_text: String,
    detail_body_callback: Option<DetailBodyCallback>,
    detail_modal: Option<DetailModal>,
    selected_index: usize,
    page_start: usize,
}

impl fmt::Debug for JobBoardScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobBoardScreen")
            .field("title", &self.title)
            .field("entries", &self.entries)
            .field("empty_state_text", &self.empty_state_text)
            .field(
                "detail_body_callback_registered",
                &self.detail_body_callback.is_some(),
            )
            .field("detail_modal", &self.detail_modal)
            .field("selected_index", &self.selected_index)
            .field("page_start", &self.page_start)
            .finish()
    }
}

impl JobBoardScreen {
    /// Default title shown in the top border.
    const DEFAULT_TITLE: &str = "Job Board";
    /// Default message shown when `entries` is empty.
    const DEFAULT_EMPTY_STATE_TEXT: &str = "(no opportunities right now)";
    /// Canonical width for the "kind" column.
    const KIND_COL_WIDTH: usize = 12;
    /// Canonical width for the "state" column.
    const STATE_COL_WIDTH: usize = 12;
    /// Canonical width for the "expires" column.
    const EXPIRES_COL_WIDTH: usize = 20;
    /// Single-cell spacer between logical columns.
    const COL_GAP: usize = 1;
    /// In the canonical 80x24 frame, border + header + divider leaves
    /// exactly 20 visible rows for entries.
    const CANONICAL_PAGE_SIZE: usize = 20;
    /// Modal title shown when a detail pane is open.
    const DETAIL_MODAL_TITLE: &str = "Opportunity Details";
    /// Preferred modal width; clamped by [`centred_rect`] when the
    /// frame is smaller than 72 columns.
    const DETAIL_MODAL_WIDTH: u16 = 72;
    /// Preferred modal height; clamped by [`centred_rect`] when the
    /// frame is smaller than 16 rows.
    const DETAIL_MODAL_HEIGHT: u16 = 16;

    /// Build a renderable Job Board list from pre-aggregated entries.
    ///
    /// The screen does not query providers directly. Games keep control
    /// over timing so they can cache rows, debounce expensive queries,
    /// or gate refreshes behind explicit user actions.
    #[must_use]
    pub fn new(entries: Vec<JobBoardEntry>) -> Self {
        Self {
            title: Self::DEFAULT_TITLE.to_string(),
            entries,
            empty_state_text: Self::DEFAULT_EMPTY_STATE_TEXT.to_string(),
            detail_body_callback: None,
            detail_modal: None,
            selected_index: 0,
            page_start: 0,
        }
    }

    /// Override the screen title displayed in the surrounding border.
    ///
    /// Useful when one game exposes multiple boards and wants each to
    /// carry domain-specific copy while keeping the same layout.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Override the empty-state copy shown when the board has no rows.
    ///
    /// This lets each game set tone-appropriate guidance ("check back
    /// after market reset", "guild desk opens at dawn", etc.) without
    /// changing screen behavior.
    #[must_use]
    pub fn with_empty_state_text(mut self, text: impl Into<String>) -> Self {
        self.empty_state_text = text.into();
        self
    }

    /// Register a game callback used to build the detail modal body.
    ///
    /// The callback is invoked when the player presses `Enter` on the
    /// selected row.
    ///
    /// Genre-neutral examples:
    ///
    /// - A **space exploration** board can render cargo tonnage, drop
    ///   point, and radio channel details for a freight listing.
    /// - A **town simulation** board can render district notes and
    ///   bulletin clerk instructions for a municipal work order.
    ///
    /// Calling this replaces any previously registered callback.
    #[must_use]
    pub fn with_detail_body_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&JobBoardEntry) -> String + 'static,
    {
        self.detail_body_callback = Some(Arc::new(callback));
        self
    }

    /// Replace the screen's current entry snapshot.
    ///
    /// Callers use this to refresh the rendered list after they re-run
    /// aggregation logic elsewhere.
    pub fn set_entries(&mut self, entries: Vec<JobBoardEntry>) {
        self.entries = entries;
        // New data invalidates any previously opened modal because its
        // body snapshot describes a row from the old list.
        self.detail_modal = None;
        self.normalize_page_state(Self::CANONICAL_PAGE_SIZE);
    }

    /// Borrow the entries currently scheduled for rendering.
    #[must_use]
    pub fn entries(&self) -> &[JobBoardEntry] {
        &self.entries
    }

    fn page_size_for_height(height: u16) -> usize {
        // Outer border consumes two rows, then the table header and
        // divider consume two more.
        let inner_height = height.saturating_sub(2) as usize;
        inner_height.saturating_sub(2).max(1)
    }

    fn normalize_page_state(&mut self, page_size: usize) {
        if self.entries.is_empty() {
            self.selected_index = 0;
            self.page_start = 0;
            return;
        }

        self.selected_index = self.selected_index.min(self.entries.len() - 1);
        self.page_start = self.page_start.min(self.last_page_start(page_size));
        if self.selected_index < self.page_start
            || self.selected_index >= self.page_start + page_size
        {
            self.page_start = (self.selected_index / page_size) * page_size;
        }
    }

    fn last_page_start(&self, page_size: usize) -> usize {
        if self.entries.is_empty() {
            return 0;
        }
        ((self.entries.len() - 1) / page_size) * page_size
    }

    fn move_cursor_down(&mut self, page_size: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected_index = (self.selected_index + 1) % self.entries.len();
        self.normalize_page_state(page_size);
    }

    fn move_cursor_up(&mut self, page_size: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected_index = if self.selected_index == 0 {
            self.entries.len() - 1
        } else {
            self.selected_index - 1
        };
        self.normalize_page_state(page_size);
    }

    fn next_page(&mut self, page_size: usize) {
        if self.entries.is_empty() {
            return;
        }
        let old_start = self.page_start;
        let offset = self.selected_index.saturating_sub(old_start);
        let last_start = self.last_page_start(page_size);
        self.page_start = if self.page_start + page_size > last_start {
            0
        } else {
            self.page_start + page_size
        };
        self.selected_index = (self.page_start + offset).min(self.entries.len() - 1);
        self.normalize_page_state(page_size);
    }

    fn previous_page(&mut self, page_size: usize) {
        if self.entries.is_empty() {
            return;
        }
        let old_start = self.page_start;
        let offset = self.selected_index.saturating_sub(old_start);
        let last_start = self.last_page_start(page_size);
        self.page_start = if self.page_start == 0 {
            last_start
        } else {
            self.page_start.saturating_sub(page_size)
        };
        self.selected_index = (self.page_start + offset).min(self.entries.len() - 1);
        self.normalize_page_state(page_size);
    }

    fn open_detail_modal(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let Some(callback) = self.detail_body_callback.as_ref() else {
            return;
        };
        let Some(entry) = self.entries.get(self.selected_index) else {
            return;
        };
        self.detail_modal = Some(DetailModal {
            title: Self::DETAIL_MODAL_TITLE.to_string(),
            body: callback(entry),
        });
    }

    fn close_detail_modal(&mut self) {
        self.detail_modal = None;
    }

    fn render_inner(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title.as_str());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.entries.is_empty() {
            // Keeping empty-state rendering in 6a makes the screen
            // usable in manual smoke tests before 6d adds explicit
            // behavior assertions around this path.
            let empty = Paragraph::new(self.empty_state_text.as_str())
                .alignment(Alignment::Center)
                .style(Style::default().add_modifier(Modifier::DIM));
            let band = Rect {
                x: inner.x,
                y: inner.y + inner.height / 2,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(empty, band);
            return;
        }

        let buf = frame.buffer_mut();
        let table_width = inner.width as usize;

        let header = Self::format_row(table_width, "KIND", "STATE", "TITLE", "EXPIRES (UTC)");
        Self::write_row(
            buf,
            inner,
            0,
            &header,
            Style::default().add_modifier(Modifier::BOLD),
        );

        if inner.height <= 1 {
            return;
        }

        let divider = "-".repeat(table_width);
        Self::write_row(buf, inner, 1, &divider, Style::default());

        if inner.height <= 2 {
            return;
        }

        let visible = (inner.height as usize).saturating_sub(2);
        let start = self.page_start.min(self.entries.len());
        for (idx, entry) in self.entries.iter().skip(start).take(visible).enumerate() {
            let expires = entry.expires_at.as_deref().unwrap_or("-");
            let row = Self::format_row(
                table_width,
                &entry.kind_label,
                &entry.state_label,
                &entry.title,
                expires,
            );
            let absolute_index = start + idx;
            let style = if absolute_index == self.selected_index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Self::write_row(buf, inner, (idx + 2) as u16, &row, style);
        }
    }

    fn render_detail_modal(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let Some(detail) = self.detail_modal.as_ref() else {
            return;
        };
        let modal = centred_rect(Self::DETAIL_MODAL_WIDTH, Self::DETAIL_MODAL_HEIGHT, area);
        render_modal(
            frame,
            modal,
            Some(detail.title.as_str()),
            detail.body.as_str(),
        );
    }

    fn write_row(buf: &mut Buffer, area: Rect, row: u16, line: &str, style: Style) {
        if row >= area.height || area.width == 0 {
            return;
        }
        let y = area.y + row;
        // `set_stringn` hard-truncates to `area.width`, which enforces
        // the "single-row table line, no wrap overflow" requirement.
        buf.set_stringn(area.x, y, line, area.width as usize, style);
    }

    fn format_row(width: usize, kind: &str, state: &str, title: &str, expires: &str) -> String {
        if width == 0 {
            return String::new();
        }

        let fixed = Self::KIND_COL_WIDTH
            + Self::STATE_COL_WIDTH
            + Self::EXPIRES_COL_WIDTH
            + (Self::COL_GAP * 3);
        let title_width = width.saturating_sub(fixed);

        let kind_cell = Self::fit_cell(kind, Self::KIND_COL_WIDTH);
        let state_cell = Self::fit_cell(state, Self::STATE_COL_WIDTH);
        let title_cell = Self::fit_cell(title, title_width);
        let expires_cell = Self::fit_cell(expires, Self::EXPIRES_COL_WIDTH);

        let mut line = String::with_capacity(width);
        line.push_str(&kind_cell);
        line.push(' ');
        line.push_str(&state_cell);
        line.push(' ');
        line.push_str(&title_cell);
        line.push(' ');
        line.push_str(&expires_cell);
        if line.chars().count() < width {
            line.push_str(&" ".repeat(width - line.chars().count()));
        }
        line
    }

    fn fit_cell(value: &str, width: usize) -> String {
        if width == 0 {
            return String::new();
        }

        let mut out = String::with_capacity(width);
        for ch in value.chars().take(width) {
            out.push(ch);
        }
        if out.chars().count() < width {
            out.push_str(&" ".repeat(width - out.chars().count()));
        }
        out
    }
}

impl Screen for JobBoardScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut ratatui::Frame<'_>) {
        let page_size = Self::page_size_for_height(frame.area().height);
        self.normalize_page_state(page_size);
        self.render_inner(frame, frame.area());
        self.render_detail_modal(frame, frame.area());
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        if self.detail_modal.is_some() {
            return match input {
                Input::Esc | Input::Char('q') | Input::Enter => {
                    self.close_detail_modal();
                    ScreenCommand::None
                }
                _ => ScreenCommand::None,
            };
        }

        let page_size = Self::page_size_for_height(ctx.terminal_size.1);
        self.normalize_page_state(page_size);

        match input {
            Input::Enter => {
                self.open_detail_modal();
                ScreenCommand::None
            }
            Input::Up | Input::Char('k') => {
                self.move_cursor_up(page_size);
                ScreenCommand::None
            }
            Input::Down | Input::Char('j') => {
                self.move_cursor_down(page_size);
                ScreenCommand::None
            }
            Input::Left | Input::Char('p') => {
                self.previous_page(page_size);
                ScreenCommand::None
            }
            Input::Right | Input::Char('n') => {
                self.next_page(page_size);
                ScreenCommand::None
            }
            Input::Esc | Input::Char('q') => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
    use crate::input::Input;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn fixture_context() -> (GameConfig, FogletContext) {
        let config = GameConfig {
            game: GameSection {
                title: "Test".into(),
                slug: "test".into(),
                description: "fixture".into(),
                min_width: 80,
                min_height: 24,
                start_map: "lobby".into(),
                start_x: 1,
                start_y: 1,
            },
            save: SaveSection {
                strategy: SaveStrategy::PerFogletUser,
            },
            manifest: ManifestSection {
                timeout_ms: 1_800_000,
                idle_timeout_ms: 300_000,
                visibility: "members".into(),
                auth_scope: "site".into(),
            },
            world: Default::default(),
            turns: None,
            leaderboards: Vec::new(),
            multiplayer: None,
            factions: Default::default(),
            spatial: Default::default(),
            presence: Default::default(),
            place_recall: Default::default(),
            inventory: Default::default(),
            world_ticks: Default::default(),
            contracts: Default::default(),
            job_board: Default::default(),
            travel: Default::default(),
            inventory_capacity: Default::default(),
            screens: Default::default(),
        };

        let foglet = FogletContext {
            door_id: "test-door".into(),
            user_id: Some("test-user".into()),
            username: Some("tester".into()),
            role: None,
            session_id: Some("sess-1".into()),
            terminal_width: 80,
            terminal_height: 24,
            source: ContextSource::LocalDev,
        };

        (config, foglet)
    }

    fn draw_screen(screen: &mut JobBoardScreen) -> Buffer {
        let (config, foglet) = fixture_context();
        let mut ctx = GameContext::new(&config, &foglet, (80, 24));
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw screen");
        term.backend().buffer().clone()
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        let mut out = String::new();
        for x in 0..buf.area.width {
            out.push_str(buf.cell((x, y)).expect("cell in bounds").symbol());
        }
        out
    }

    fn all_rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height).map(|y| row_text(buf, y)).collect()
    }

    fn contains_text(buf: &Buffer, needle: &str) -> bool {
        all_rows(buf).iter().any(|line| line.contains(needle))
    }

    fn entry(kind: &str, state: &str, title: &str, expires_at: Option<&str>) -> JobBoardEntry {
        JobBoardEntry {
            source: crate::job_board::JobBoardSource::Contract,
            source_id: 1,
            kind_label: kind.to_string(),
            state_label: state.to_string(),
            title: title.to_string(),
            summary: "unused in this render test".to_string(),
            reward_preview: None,
            location_preview: None,
            expires_at: expires_at.map(ToOwned::to_owned),
            accept_action: None,
        }
    }

    fn numbered_entries(total: usize) -> Vec<JobBoardEntry> {
        (0..total)
            .map(|idx| {
                entry(
                    "Contract",
                    "available",
                    &format!("Task #{idx:02}"),
                    Some("2026-05-10T10:00:00Z"),
                )
            })
            .collect()
    }

    fn dispatch(screen: &mut JobBoardScreen, input: Input) -> ScreenCommand {
        let (config, foglet) = fixture_context();
        let mut ctx = GameContext::new(&config, &foglet, (80, 24));
        screen.handle_input(&mut ctx, input)
    }

    #[test]
    fn renders_required_columns_in_80x24_layout() {
        let mut screen = JobBoardScreen::new(vec![entry(
            "Contract",
            "available",
            "Deliver ore to moon dock",
            Some("2026-05-10T10:00:00Z"),
        )]);

        let buf = draw_screen(&mut screen);
        let header = row_text(&buf, 1);
        let data = row_text(&buf, 3);

        assert!(header.contains("KIND"));
        assert!(header.contains("STATE"));
        assert!(header.contains("TITLE"));
        assert!(header.contains("EXPIRES (UTC)"));

        assert!(data.contains("Contract"));
        assert!(data.contains("available"));
        assert!(data.contains("Deliver ore to moon dock"));
        assert!(data.contains("2026-05-10T10:00:00Z"));
    }

    #[test]
    fn long_titles_do_not_overflow_into_following_rows() {
        let long_title = "ANCIENT-RUINS-DELIVERY-REQUEST-".repeat(16);
        let mut screen = JobBoardScreen::new(vec![entry(
            "Contract",
            "available",
            &long_title,
            Some("2026-05-10T10:00:00Z"),
        )]);

        let buf = draw_screen(&mut screen);
        let first_row = row_text(&buf, 3);
        let next_row = row_text(&buf, 4);

        assert!(
            first_row.contains("ANCIENT-RUINS-DELIVERY-REQUEST"),
            "expected the long title to appear on the entry row"
        );
        assert!(
            !next_row.contains("ANCIENT-RUINS-DELIVERY-REQUEST"),
            "expected no wrapped overflow into the following row"
        );
    }

    #[test]
    fn cursor_wraps_at_page_boundaries() {
        let mut screen = JobBoardScreen::new(numbered_entries(25));

        // Walk to the bottom row on page 1 (index 19 in an 80x24 view).
        for _ in 0..20 {
            let cmd = dispatch(&mut screen, Input::Down);
            assert!(matches!(cmd, ScreenCommand::None));
        }
        assert_eq!(screen.selected_index, 20);
        assert_eq!(screen.page_start, 20);

        // Moving up from the top of page 2 wraps back onto page 1.
        let cmd = dispatch(&mut screen, Input::Up);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.selected_index, 19);
        assert_eq!(screen.page_start, 0);
    }

    #[test]
    fn page_hotkeys_advance_and_wrap() {
        let mut screen = JobBoardScreen::new(numbered_entries(45));

        let cmd = dispatch(&mut screen, Input::Right);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.page_start, 20);
        assert_eq!(screen.selected_index, 20);

        let cmd = dispatch(&mut screen, Input::Right);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.page_start, 40);
        assert_eq!(screen.selected_index, 40);

        let cmd = dispatch(&mut screen, Input::Right);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.page_start, 0);
        assert_eq!(screen.selected_index, 0);

        let cmd = dispatch(&mut screen, Input::Left);
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(screen.page_start, 40);
        assert_eq!(screen.selected_index, 40);
    }

    #[test]
    fn quit_hotkeys_return_control_to_caller() {
        let mut screen = JobBoardScreen::new(numbered_entries(3));

        let cmd = dispatch(&mut screen, Input::Char('q'));
        assert!(matches!(cmd, ScreenCommand::Pop));

        let cmd = dispatch(&mut screen, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
    }

    #[test]
    fn detail_modal_opens_and_closes_without_disturbing_list_redraw() {
        let mut screen = JobBoardScreen::new(vec![entry(
            "Contract",
            "available",
            "Deliver ore to moon dock",
            Some("2026-05-10T10:00:00Z"),
        )])
        .with_detail_body_callback(|selected| {
            format!(
                "Manifest details for: {}\nDock relay: channel-7",
                selected.title
            )
        });

        let baseline = draw_screen(&mut screen);

        let cmd = dispatch(&mut screen, Input::Enter);
        assert!(matches!(cmd, ScreenCommand::None));

        let modal = draw_screen(&mut screen);
        assert!(
            contains_text(&modal, "Opportunity Details"),
            "expected modal title to render after Enter"
        );
        assert!(
            contains_text(&modal, "Manifest details for: Deliver ore to moon dock"),
            "expected callback-provided detail body to render in modal"
        );

        let cmd = dispatch(&mut screen, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::None));

        let after_close = draw_screen(&mut screen);
        assert_eq!(
            all_rows(&baseline),
            all_rows(&after_close),
            "closing the detail modal must restore the list rendering exactly"
        );
    }
}
