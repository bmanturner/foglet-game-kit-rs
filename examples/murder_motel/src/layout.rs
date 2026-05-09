//! Small layout helpers shared across the screens.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Centre a `width × height` rectangle inside `outer`, clamping the
/// inner size if `outer` is smaller than requested.
///
/// Pulled out so every modal lays out the same way; SPEC §7.1 already
/// guarantees an 80x24 floor so the clamping path only matters for the
/// unit tests that hand in tiny `TestBackend` frames.
pub fn centred_rect(width: u16, height: u16, outer: Rect) -> Rect {
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
