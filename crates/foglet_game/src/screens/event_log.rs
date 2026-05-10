//! `EventLogScreen` — reusable read-only v2 event log/news surface.

use std::fmt;
use std::sync::Arc;

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::config::EventLogScreenSection;
use crate::events::{EventError, EventRecord};
use crate::input::Input;
use crate::screen::{GameContext, Screen, ScreenCommand};
use crate::world_db::WorldDb;

type EventFormatter = Arc<dyn Fn(&EventRecord) -> EventLogLine + 'static>;
type EventFilter = Arc<dyn Fn(&EventRecord) -> bool + 'static>;

/// Scope used to select events for [`EventLogScreen`].
#[derive(Clone)]
pub enum EventLogScope {
    /// Include all events.
    Global,
    /// Include events attributed to one player.
    Player(i64),
    /// Include events matching one kind key.
    Kind(String),
    /// Include events accepted by a game-supplied predicate.
    Custom(EventFilter),
}

impl fmt::Debug for EventLogScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => f.write_str("Global"),
            Self::Player(player_id) => f.debug_tuple("Player").field(player_id).finish(),
            Self::Kind(kind) => f.debug_tuple("Kind").field(kind).finish(),
            Self::Custom(_) => f.write_str("Custom(<callback>)"),
        }
    }
}

/// Timestamp rendering mode for event rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampStyle {
    /// Render the stored SQLite timestamp as-is.
    Absolute,
    /// Render a compact relative placeholder.
    Relative,
    /// Hide timestamps.
    Hidden,
}

/// Rendered line returned by a game-supplied event formatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogLine {
    /// Primary rendered line.
    pub primary: String,
    /// Optional secondary rendered line.
    pub secondary: Option<String>,
}

/// Read-only screen over v2 `world_events`.
#[derive(Clone)]
pub struct EventLogScreen {
    title: String,
    events: Vec<EventRecord>,
    scope: EventLogScope,
    page_size: usize,
    timestamp_style: TimestampStyle,
    empty_state_text: String,
    formatter: EventFormatter,
    page_start: usize,
}

impl fmt::Debug for EventLogScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventLogScreen")
            .field("title", &self.title)
            .field("events", &self.events)
            .field("scope", &self.scope)
            .field("page_size", &self.page_size)
            .field("timestamp_style", &self.timestamp_style)
            .field("empty_state_text", &self.empty_state_text)
            .field("page_start", &self.page_start)
            .finish()
    }
}

impl EventLogScreen {
    /// Default screen title.
    pub const DEFAULT_TITLE: &str = "Event Log";
    /// Default empty-state copy.
    pub const DEFAULT_EMPTY_STATE_TEXT: &str = "(no events yet)";

    /// Load events from the world DB using config defaults.
    pub fn from_world_db(
        world: &WorldDb,
        config: &EventLogScreenSection,
    ) -> Result<Self, EventError> {
        Self::from_world_db_with_scope(world, config, EventLogScope::Global)
    }

    /// Load events from the world DB using config defaults and a scope.
    pub fn from_world_db_with_scope(
        world: &WorldDb,
        config: &EventLogScreenSection,
        scope: EventLogScope,
    ) -> Result<Self, EventError> {
        let events = query_events(world, &scope)?;
        Ok(Self::new(events, config.default_page_size as usize).with_scope(scope))
    }

    /// Build a screen from an event snapshot.
    #[must_use]
    pub fn new(events: Vec<EventRecord>, page_size: usize) -> Self {
        Self {
            title: Self::DEFAULT_TITLE.to_string(),
            events,
            scope: EventLogScope::Global,
            page_size: page_size.max(1),
            timestamp_style: TimestampStyle::Absolute,
            empty_state_text: Self::DEFAULT_EMPTY_STATE_TEXT.to_string(),
            formatter: Arc::new(|event| EventLogLine {
                primary: event.message.clone(),
                secondary: None,
            }),
            page_start: 0,
        }
    }

    /// Override the screen title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Override the event scope metadata stored on this screen.
    #[must_use]
    pub fn with_scope(mut self, scope: EventLogScope) -> Self {
        self.scope = scope;
        self
    }

    /// Override timestamp rendering.
    #[must_use]
    pub fn with_timestamp_style(mut self, style: TimestampStyle) -> Self {
        self.timestamp_style = style;
        self
    }

    /// Override empty-state text.
    #[must_use]
    pub fn with_empty_state_text(mut self, text: impl Into<String>) -> Self {
        self.empty_state_text = text.into();
        self
    }

    /// Register a formatter callback for event rows.
    #[must_use]
    pub fn with_formatter<F>(mut self, formatter: F) -> Self
    where
        F: Fn(&EventRecord) -> EventLogLine + 'static,
    {
        self.formatter = Arc::new(formatter);
        self
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

        if self.events.is_empty() {
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

        let visible = self.page_size.min(inner.height as usize);
        let events = self.events.iter().skip(self.page_start).take(visible);
        for (idx, event) in events.enumerate() {
            let line = (self.formatter)(event);
            let row = self.format_event_line(event, &line);
            write_row(
                frame.buffer_mut(),
                inner,
                idx as u16,
                &row,
                Style::default(),
            );
        }
    }

    fn format_event_line(&self, event: &EventRecord, line: &EventLogLine) -> String {
        match self.timestamp_style {
            TimestampStyle::Absolute => {
                format!("{} {} {}", event.created_at, event.kind, line.primary)
            }
            TimestampStyle::Relative => format!("now {} {}", event.kind, line.primary),
            TimestampStyle::Hidden => format!("{} {}", event.kind, line.primary),
        }
    }

    fn next_page(&mut self) {
        if self.events.is_empty() {
            self.page_start = 0;
            return;
        }
        let next = self.page_start.saturating_add(self.page_size);
        self.page_start = next.min(self.events.len().saturating_sub(1));
    }

    fn previous_page(&mut self) {
        self.page_start = self.page_start.saturating_sub(self.page_size);
    }
}

impl Screen for EventLogScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut ratatui::Frame<'_>) {
        self.render_inner(frame, frame.area());
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match input {
            Input::Right | Input::Down | Input::Char('n' | 'N') => {
                self.next_page();
                ScreenCommand::None
            }
            Input::Left | Input::Up | Input::Char('p' | 'P') => {
                self.previous_page();
                ScreenCommand::None
            }
            Input::Char('q' | 'Q') | Input::Esc => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        }
    }
}

fn query_events(world: &WorldDb, scope: &EventLogScope) -> Result<Vec<EventRecord>, EventError> {
    let mut statement = world
        .connection()
        .prepare(
            "SELECT id, created_at, kind, player_id, message, metadata \
             FROM world_events \
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(|source| EventError::Sqlite { source })?;
    let rows = statement
        .query_map([], row_to_event)
        .map_err(|source| EventError::Sqlite { source })?;
    let events = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| EventError::Sqlite { source })?;
    Ok(events
        .into_iter()
        .filter(|event| scope_includes(scope, event))
        .collect())
}

fn scope_includes(scope: &EventLogScope, event: &EventRecord) -> bool {
    match scope {
        EventLogScope::Global => true,
        EventLogScope::Player(player_id) => event.player_id == Some(*player_id),
        EventLogScope::Kind(kind) => event.kind == *kind,
        EventLogScope::Custom(filter) => filter(event),
    }
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        id: row.get(0)?,
        created_at: row.get(1)?,
        kind: row.get(2)?,
        player_id: row.get(3)?,
        message: row.get(4)?,
        metadata: row.get(5)?,
    })
}

fn write_row(buf: &mut ratatui::buffer::Buffer, area: Rect, row: u16, text: &str, style: Style) {
    if row >= area.height {
        return;
    }
    let y = area.y + row;
    for (offset, ch) in text.chars().take(area.width as usize).enumerate() {
        let x = area.x + offset as u16;
        buf[(x, y)].set_symbol(&ch.to_string()).set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::{EventLogLine, EventLogScope, EventLogScreen, TimestampStyle};
    use crate::config::{
        EventLogScreenSection, GameConfig, GameSection, ManifestSection, SaveSection, SaveStrategy,
    };
    use crate::events::EventRecord;
    use crate::events::WORLD_EVENTS_MIGRATION;
    use crate::foglet::{ContextSource, FogletContext};
    use crate::input::Input;
    use crate::players::PLAYERS_MIGRATION;
    use crate::screen::{GameContext, Screen, ScreenCommand};
    use crate::world_db::WorldDb;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use std::cell::Cell;
    use std::rc::Rc;
    use tempfile::tempdir;

    #[test]
    fn event_log_screen_reads_configured_events_and_renders_within_80x24() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("events migration applies");
        world
            .append_event("door_opened", None, "The door opened.", None)
            .expect("event appends");

        let section = EventLogScreenSection {
            enabled: true,
            default_page_size: 20,
        };
        let mut screen = EventLogScreen::from_world_db(&world, &section)
            .expect("screen loads events")
            .with_timestamp_style(TimestampStyle::Hidden);
        let buffer = draw_screen(&mut screen);

        assert_eq!(buffer.area.width, 80);
        assert_eq!(buffer.area.height, 24);
        assert!(contains_text(&buffer, "Event Log"));
        assert!(contains_text(&buffer, "door_opened The door opened."));
    }

    #[test]
    fn event_log_screen_empty_result_uses_empty_state_text() {
        let mut screen =
            EventLogScreen::new(Vec::new(), 20).with_empty_state_text("(case ledger is empty)");
        let buffer = draw_screen(&mut screen);

        assert!(contains_text(&buffer, "(case ledger is empty)"));
    }

    #[test]
    fn event_log_screen_pages_forward_and_backward_across_100_events() {
        let mut screen = EventLogScreen::new(event_fixture(100), 10)
            .with_timestamp_style(TimestampStyle::Hidden);
        let (config, foglet) = fixture_context();
        let mut ctx = GameContext::new(&config, &foglet, (80, 24));

        let first_page = draw_screen(&mut screen);
        assert!(contains_text(&first_page, "kind event-000"));
        assert!(!contains_text(&first_page, "kind event-010"));

        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Right),
            ScreenCommand::None
        ));
        let second_page = draw_screen(&mut screen);
        assert!(contains_text(&second_page, "kind event-010"));
        assert!(!contains_text(&second_page, "kind event-000"));

        assert!(matches!(
            screen.handle_input(&mut ctx, Input::Left),
            ScreenCommand::None
        ));
        let back_to_first = draw_screen(&mut screen);
        assert!(contains_text(&back_to_first, "kind event-000"));
    }

    #[test]
    fn event_log_screen_player_scope_excludes_other_players_events() {
        let dir = tempdir().expect("tempdir creates");
        let db_path = dir.path().join("world.sqlite");
        let mut world = WorldDb::open(&db_path).expect("open succeeds");
        world
            .apply_migration(&PLAYERS_MIGRATION)
            .expect("players migration applies");
        world
            .apply_migration(&WORLD_EVENTS_MIGRATION)
            .expect("events migration applies");
        world
            .connection()
            .execute(
                "INSERT INTO players (id, handle, role, security_level) VALUES (?1, ?2, 'user', 0)",
                rusqlite::params![1, "alice"],
            )
            .expect("alice player inserts");
        world
            .connection()
            .execute(
                "INSERT INTO players (id, handle, role, security_level) VALUES (?1, ?2, 'user', 0)",
                rusqlite::params![2, "bob"],
            )
            .expect("bob player inserts");
        world
            .append_event("move", Some(1), "alice moved", None)
            .expect("alice event appends");
        world
            .append_event("move", Some(2), "bob moved", None)
            .expect("bob event appends");

        let section = EventLogScreenSection {
            enabled: true,
            default_page_size: 20,
        };
        let mut screen =
            EventLogScreen::from_world_db_with_scope(&world, &section, EventLogScope::Player(1))
                .expect("screen loads player-scoped events")
                .with_timestamp_style(TimestampStyle::Hidden);
        let buffer = draw_screen(&mut screen);

        assert!(contains_text(&buffer, "alice moved"));
        assert!(!contains_text(&buffer, "bob moved"));
    }

    #[test]
    fn event_log_screen_formatter_runs_once_per_visible_event_and_drives_rendering() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_formatter = Rc::clone(&calls);
        let mut screen = EventLogScreen::new(event_fixture(3), 2)
            .with_timestamp_style(TimestampStyle::Hidden)
            .with_formatter(move |event| {
                calls_for_formatter.set(calls_for_formatter.get() + 1);
                EventLogLine {
                    primary: format!("formatted {}", event.message),
                    secondary: None,
                }
            });

        let buffer = draw_screen(&mut screen);

        assert_eq!(calls.get(), 2);
        assert!(contains_text(&buffer, "kind formatted event-000"));
        assert!(contains_text(&buffer, "kind formatted event-001"));
        assert!(!contains_text(&buffer, "event-002"));
    }

    fn draw_screen(screen: &mut EventLogScreen) -> Buffer {
        let (config, foglet) = fixture_context();
        let mut ctx = GameContext::new(&config, &foglet, (80, 24));
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        term.draw(|frame| screen.render(&mut ctx, frame))
            .expect("draw screen");
        term.backend().buffer().clone()
    }

    fn contains_text(buf: &Buffer, needle: &str) -> bool {
        (0..buf.area.height).any(|y| {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf.cell((x, y)).expect("cell in bounds").symbol());
            }
            line.contains(needle)
        })
    }

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

    fn event_fixture(count: usize) -> Vec<EventRecord> {
        (0..count)
            .map(|idx| EventRecord {
                id: idx as i64,
                created_at: "2030-01-01 00:00:00".to_string(),
                kind: "kind".to_string(),
                player_id: None,
                message: format!("event-{idx:03}"),
                metadata: None,
            })
            .collect()
    }
}
