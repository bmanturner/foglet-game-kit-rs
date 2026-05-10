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
//! Task 6a intentionally lands the render-only baseline. Navigation,
//! pagination, modal details, and accept/claim callbacks arrive in
//! follow-up tasks.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::job_board::JobBoardEntry;
use crate::screen::{GameContext, Screen};

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
#[derive(Debug, Clone)]
pub struct JobBoardScreen {
    title: String,
    entries: Vec<JobBoardEntry>,
    empty_state_text: String,
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

    /// Replace the screen's current entry snapshot.
    ///
    /// Callers use this to refresh the rendered list after they re-run
    /// aggregation logic elsewhere.
    pub fn set_entries(&mut self, entries: Vec<JobBoardEntry>) {
        self.entries = entries;
    }

    /// Borrow the entries currently scheduled for rendering.
    #[must_use]
    pub fn entries(&self) -> &[JobBoardEntry] {
        &self.entries
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
        for (idx, entry) in self.entries.iter().take(visible).enumerate() {
            let expires = entry.expires_at.as_deref().unwrap_or("-");
            let row = Self::format_row(
                table_width,
                &entry.kind_label,
                &entry.state_label,
                &entry.title,
                expires,
            );
            Self::write_row(buf, inner, (idx + 2) as u16, &row, Style::default());
        }
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
        self.render_inner(frame, frame.area());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy};
    use crate::foglet::{ContextSource, FogletContext};
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
}
