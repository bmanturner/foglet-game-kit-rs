//! Input normalization — `crossterm::event::Event` → [`Input`].
//!
//! SPEC §8.3 defines a small, terminal-agnostic [`Input`] enum that
//! game `Screen`s consume. The runtime loop (Task 7) polls
//! `crossterm` events and pipes them through [`from_event`] so screens
//! never touch `crossterm` types directly. Keeping the mapping pure —
//! no terminal state, no globals — means it is fully unit-testable
//! without a TTY and stays trivially swappable if the runtime ever
//! grows a different event source.
//!
//! ## Mapping rules
//!
//! - Arrow keys → `Up` / `Down` / `Left` / `Right`.
//! - `Enter`, `Esc`, `Backspace` → their named variants.
//! - `Resize(w, h)` → `Input::Resize { width: w, height: h }`.
//! - `Char(c)` with no modifiers (or only `SHIFT`) → `Input::Char(c)`.
//!   `SHIFT` is intentionally not stripped from the `char` itself —
//!   `crossterm` already delivers the shifted character (e.g. `'A'`),
//!   so consumers see what the user typed.
//! - `Char(c)` with `CTRL` → `Input::Ctrl(c)`. `CTRL+SHIFT+c` still
//!   maps to `Ctrl(c)` because the `CTRL` intent dominates; SPEC §8.3
//!   does not distinguish further. **Ctrl-C is reported as
//!   `Input::Ctrl('c')`** rather than swallowed, so the runtime loop
//!   can treat it as a quit signal *after* terminal restoration.
//! - Anything else (mouse events, focus events, paste, F-keys, Tab,
//!   media keys, …) → [`Input::Unknown`]. SPEC §8.3 lists the variants
//!   v1 cares about; other inputs are deliberately collapsed so games
//!   don't grow ad-hoc per-key handling that won't survive a future
//!   crossterm bump.
//!
//! Press/release distinction: on platforms where `crossterm` reports
//! both `Press` and `Release` events, only `Press` produces a non-
//! [`Input::Unknown`] result. Repeats map the same as presses; SPEC
//! §8.3 has no held-key concept.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Terminal-agnostic input variants consumed by `Screen`s.
///
/// Mirrors SPEC §8.3 verbatim. Adding a variant is a SPEC change and
/// must be reflected there first; removing or renaming one is a
/// breaking API change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Enter / Return.
    Enter,
    /// Escape.
    Esc,
    /// Backspace.
    Backspace,
    /// A printable character (already shifted by the terminal).
    ///
    /// Modifiers other than `SHIFT` route to other variants (e.g.
    /// `Ctrl(c)`); they never reach this case.
    Char(char),
    /// `Ctrl` + a printable character. The carried `char` is the
    /// lowercase form `crossterm` reports.
    Ctrl(char),
    /// Terminal resize. Width/height are in cells, matching
    /// `crossterm::event::Event::Resize`.
    Resize {
        /// New width in cells.
        width: u16,
        /// New height in cells.
        height: u16,
    },
    /// Anything the runtime does not currently translate. Screens
    /// generally ignore these; they exist so the mapping is total.
    Unknown,
}

/// Translate a single `crossterm` event into the SPEC §8.3 [`Input`].
///
/// This is the single seam the runtime loop calls on every polled
/// event. Pure: same input always yields the same output, no I/O, no
/// terminal state required.
pub fn from_event(event: Event) -> Input {
    match event {
        Event::Key(key) => from_key_event(key),
        Event::Resize(width, height) => Input::Resize { width, height },
        // Mouse, focus, paste are out-of-scope for v1 (SPEC §8.3 lists
        // exactly what we map). Collapsing keeps screens immune to
        // crossterm growing new event variants.
        _ => Input::Unknown,
    }
}

/// Translate a `KeyEvent` into [`Input`]. Public so tests and future
/// alternative event sources (e.g. a fake terminal driver in Task 7d)
/// can reuse the key-level rules without synthesising whole `Event`s.
pub fn from_key_event(key: KeyEvent) -> Input {
    // Some platforms (Windows, kitty protocol, …) deliver Release and
    // Repeat in addition to Press. v1 only cares about Press +
    // Repeat-as-press; Release is collapsed to Unknown so a single
    // physical keystroke doesn't fire a screen handler twice.
    match key.kind {
        KeyEventKind::Press | KeyEventKind::Repeat => {}
        KeyEventKind::Release => return Input::Unknown,
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Up => Input::Up,
        KeyCode::Down => Input::Down,
        KeyCode::Left => Input::Left,
        KeyCode::Right => Input::Right,
        KeyCode::Enter => Input::Enter,
        KeyCode::Esc => Input::Esc,
        KeyCode::Backspace => Input::Backspace,
        KeyCode::Char(c) if ctrl => Input::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) => Input::Char(c),
        _ => Input::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn press(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn arrows_map_to_directional_inputs() {
        assert_eq!(
            from_event(press(KeyCode::Up, KeyModifiers::NONE)),
            Input::Up
        );
        assert_eq!(
            from_event(press(KeyCode::Down, KeyModifiers::NONE)),
            Input::Down
        );
        assert_eq!(
            from_event(press(KeyCode::Left, KeyModifiers::NONE)),
            Input::Left
        );
        assert_eq!(
            from_event(press(KeyCode::Right, KeyModifiers::NONE)),
            Input::Right
        );
    }

    #[test]
    fn enter_esc_backspace_map_to_named_variants() {
        assert_eq!(
            from_event(press(KeyCode::Enter, KeyModifiers::NONE)),
            Input::Enter
        );
        assert_eq!(
            from_event(press(KeyCode::Esc, KeyModifiers::NONE)),
            Input::Esc
        );
        assert_eq!(
            from_event(press(KeyCode::Backspace, KeyModifiers::NONE)),
            Input::Backspace
        );
    }

    #[test]
    fn plain_chars_map_to_char_variant() {
        assert_eq!(
            from_event(press(KeyCode::Char('a'), KeyModifiers::NONE)),
            Input::Char('a')
        );
        // `crossterm` already delivers the shifted character; we
        // forward it verbatim rather than down-casing.
        assert_eq!(
            from_event(press(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Input::Char('A')
        );
        assert_eq!(
            from_event(press(KeyCode::Char(' '), KeyModifiers::NONE)),
            Input::Char(' ')
        );
    }

    #[test]
    fn ctrl_c_is_reported_not_swallowed() {
        // SPEC: Ctrl-C reaches the runtime so it can run the terminal
        // guard's restore path before the process tears down.
        assert_eq!(
            from_event(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Input::Ctrl('c')
        );
    }

    #[test]
    fn ctrl_shift_char_still_maps_to_ctrl_lowercase() {
        // CTRL dominates SHIFT; the carried char is the lowercase
        // form so consumers can match `Ctrl('c')` regardless of how
        // the terminal reports the modifier mix.
        let ev = press(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(from_event(ev), Input::Ctrl('c'));
    }

    #[test]
    fn resize_passes_dimensions_through() {
        assert_eq!(
            from_event(Event::Resize(120, 40)),
            Input::Resize {
                width: 120,
                height: 40,
            }
        );
    }

    #[test]
    fn unmapped_keys_become_unknown() {
        // F-keys, Tab, Insert, Home, etc. are intentionally collapsed
        // to Unknown — adding them is a SPEC change.
        assert_eq!(
            from_event(press(KeyCode::F(5), KeyModifiers::NONE)),
            Input::Unknown
        );
        assert_eq!(
            from_event(press(KeyCode::Tab, KeyModifiers::NONE)),
            Input::Unknown
        );
        assert_eq!(
            from_event(press(KeyCode::Home, KeyModifiers::NONE)),
            Input::Unknown
        );
    }

    #[test]
    fn non_key_non_resize_events_are_unknown() {
        // Mouse/focus/paste are out-of-scope; ensure they don't leak
        // through as some other variant.
        assert_eq!(from_event(Event::FocusGained), Input::Unknown);
        assert_eq!(from_event(Event::FocusLost), Input::Unknown);
        assert_eq!(from_event(Event::Paste("hi".into())), Input::Unknown);
    }

    #[test]
    fn key_release_events_are_dropped_to_unknown() {
        // Without this, every keystroke would fire a screen handler
        // twice on platforms that report Release events.
        let ev = Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(from_event(ev), Input::Unknown);
    }

    #[test]
    fn key_repeat_events_map_like_press() {
        // Held keys (auto-repeat) feel like a stream of presses to
        // games — same mapping, no special handling.
        let ev = Event::Key(KeyEvent {
            code: KeyCode::Down,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        });
        assert_eq!(from_event(ev), Input::Down);
    }

    #[test]
    fn ctrl_alpha_round_trip_for_common_signals() {
        // Spot-check a handful of Ctrl combos game authors are likely
        // to bind (quit, save, etc.). Lowercasing keeps match arms
        // simple regardless of caps-lock or shift state.
        for (raw, expected) in [('q', 'q'), ('s', 's'), ('D', 'd')] {
            assert_eq!(
                from_event(press(KeyCode::Char(raw), KeyModifiers::CONTROL)),
                Input::Ctrl(expected),
                "ctrl+{raw} should map to Ctrl('{expected}')"
            );
        }
    }
}
