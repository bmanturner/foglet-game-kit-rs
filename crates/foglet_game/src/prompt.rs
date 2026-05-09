//! Text-interface prompt primitives — the v1.1 layer between
//! `Screen`/`Input` and door-game prompt flows.
//!
//! # Why this module exists
//!
//! v1 lets a game move a player around an ASCII map and run scripted
//! dialog graphs. v1.1 adds the missing piece for classic BBS/RPG
//! flows: loot prompts, vendor transactions, confirmation gates,
//! "press any key" pauses, and post-action feedback. SPEC_v1_1.md §1
//! and §3 establish the contract; this module is the home for that
//! contract's implementation.
//!
//! # Architecture in one sentence
//!
//! Prompts are **pure reducers plus Ratatui render helpers**. A
//! `ChoicePrompt` value is data; sending an [`crate::Input`] through
//! it returns a typed `PromptAction`; rendering is a separate function
//! that draws into a `Frame` or buffer. Game state lives outside the
//! prompt — the prompt routes choices, it does not own gold,
//! inventory, or flags.
//!
//! ## Why not a scripting VM, an "engine", or a widget with hidden state?
//!
//! Considered and rejected during Task 1:
//!
//! - **Scripting VM** (Lua/Rhai/custom) — SPEC_v1_1.md §2.2 forbids
//!   it, and door-game prompts need live Rust state (inventory, gold,
//!   capacity, flag sets) that is awkward to thread through an
//!   interpreter. Builder-driven Rust prompts also test under
//!   `TestBackend` without spinning up a runtime.
//! - **Stateful widget that owns selection internally** (the obvious
//!   "Ratatui-flavored" route) — hides cursor state behind a `&mut`
//!   handle, makes input handling order-dependent, and resists
//!   table-driven reducer tests. The v1 `DialogState` already
//!   established the pure-reducer pattern; v1.1 stays consistent.
//! - **Forcing every prompt through `DialogState` YAML** — works for
//!   static NPC chatter but cannot express dynamic labels like
//!   `"Mana potions: 174g each | You have: 0/26"`. SPEC_v1_1.md §8
//!   explicitly keeps Rust-authored prompts first-class.
//!
//! Pure reducers + render helpers won because they (1) test cleanly
//! under `ratatui::backend::TestBackend`, (2) compose with the
//! existing `Screen` trait without a new runtime, and (3) leave game
//! semantics in game code where `disabled_if(!can_buy, ...)` reads
//! naturally next to the `gold`/`inventory` fields it inspects. The
//! optional [`crate::screen::Screen`] adapter (Task 7) is a thin
//! convenience over the same primitives.
//!
//! # Module map (target — populated across Tasks 2–8)
//!
//! - `PromptKey` — normalized direct-input key (Task 2a).
//! - `PromptChoice<T>` — one selectable option with stable id and
//!   optional disabled reason/hint/style role (Task 2b).
//! - `ChoicePrompt<T>` + `PromptAction<T>` — the core reducer
//!   (Tasks 3–4).
//! - `ConfirmPrompt`, `AnyKeyPrompt` — small specialised reducers
//!   layered over `ChoicePrompt` (Task 6).
//! - `TextBlock`, semantic style roles, prompt rendering helpers
//!   (Task 5).
//! - `PromptScreen` — optional `Screen` adapter (Task 7).
//!
//! Task 1 only stands the module up so `lib.rs` re-exports compile;
//! the types above land alongside the tasks that exercise them.

use crate::Input;

/// Normalized direct-input key for prompts (SPEC_v1_1.md §4.1).
///
/// Prompts care about exactly three things: a printable hotkey, a
/// confirmation press (`Enter`), and a cancel press (`Esc`). Everything
/// else — arrow keys, resize, mouse, modifiers — either belongs to the
/// arrow/Enter navigation mode handled inside [`crate::prompt`] or is
/// not a prompt-relevant signal at all.
///
/// Character hotkeys are stored **lowercase** so direct-key matching is
/// case-insensitive without per-comparison `to_ascii_lowercase` calls.
/// Construct via [`PromptKey::char`] or [`PromptKey::from_input`]; the
/// `Char` variant's tuple field is intentionally not pub-exposed
/// through a constructor that skips normalization.
///
/// # Why a separate type, not just `Input`?
///
/// `Input` is the runtime's full event vocabulary (resize, ctrl, arrow,
/// unknown). Prompts only consume a small subset, and the reducer's
/// match arms are easier to read — and harder to bug — when the type
/// system says "this is a prompt key, not any old input". Conversion
/// happens once at the prompt edge via [`PromptKey::from_input`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptKey {
    /// A printable character hotkey, normalized to lowercase ASCII when
    /// the source was an ASCII letter. Non-ASCII characters are kept
    /// as-is — prompt authors using non-ASCII hotkeys are responsible
    /// for their own case handling.
    Char(char),
    /// Enter / Return — used to confirm the highlighted choice in the
    /// optional arrow/Enter navigation mode (Task 4).
    Enter,
    /// Escape — used to cancel the prompt when cancellation is
    /// configured (Task 3d).
    Esc,
}

impl PromptKey {
    /// Construct a character prompt key, lowercasing ASCII letters so
    /// direct-key matching is case-insensitive.
    ///
    /// Use this when defining choice hotkeys in a `PromptChoice` (or
    /// equivalent builder, landing in Task 2b); the reducer will
    /// compare incoming [`PromptKey::Char`] values for equality with no
    /// further casing.
    pub fn char(c: char) -> Self {
        PromptKey::Char(c.to_ascii_lowercase())
    }

    /// Translate an [`Input`] into a [`PromptKey`] when the input is
    /// prompt-relevant; return `None` otherwise.
    ///
    /// `None` covers the inputs prompts deliberately ignore: resize
    /// (SPEC_v1_1.md §7 — "resize ignored by prompt selection logic"),
    /// arrow/direction keys (consumed by the navigation reducer in Task
    /// 4, not the direct-key path), backspace, ctrl-modified keys, and
    /// `Input::Unknown`. Returning an `Option` rather than a default
    /// variant forces callers to make an explicit choice about
    /// non-prompt input instead of silently funneling resize through.
    pub fn from_input(input: Input) -> Option<Self> {
        match input {
            Input::Char(c) => Some(PromptKey::char(c)),
            Input::Enter => Some(PromptKey::Enter),
            Input::Esc => Some(PromptKey::Esc),
            // Arrow keys belong to the optional navigation reducer
            // (Task 4); the direct-key reducer treats them as not its
            // event. Same for backspace, ctrl combos, resize, unknown.
            Input::Up
            | Input::Down
            | Input::Left
            | Input::Right
            | Input::Backspace
            | Input::Ctrl(_)
            | Input::Resize { .. }
            | Input::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_lowercases_ascii_letters() {
        // SPEC_v1_1.md §4.1: "Character hotkeys MUST match
        // case-insensitively by default."
        assert_eq!(PromptKey::char('e'), PromptKey::Char('e'));
        assert_eq!(PromptKey::char('E'), PromptKey::Char('e'));
        assert_eq!(PromptKey::char('e'), PromptKey::char('E'));
    }

    #[test]
    fn char_preserves_non_letter_ascii_and_digits() {
        // Digits and punctuation have no case; passthrough verbatim so
        // numeric hotkeys (SPEC_v1_1.md §7) work as authored.
        assert_eq!(PromptKey::char('1'), PromptKey::Char('1'));
        assert_eq!(PromptKey::char('?'), PromptKey::Char('?'));
    }

    #[test]
    fn from_input_maps_chars_lowercased() {
        assert_eq!(
            PromptKey::from_input(Input::Char('e')),
            Some(PromptKey::Char('e'))
        );
        assert_eq!(
            PromptKey::from_input(Input::Char('E')),
            Some(PromptKey::Char('e'))
        );
        // Both cases collapse to the same prompt key — direct equality
        // is enough for the reducer (Task 3) to match either input.
        assert_eq!(
            PromptKey::from_input(Input::Char('e')),
            PromptKey::from_input(Input::Char('E'))
        );
    }

    #[test]
    fn from_input_maps_enter_and_esc() {
        assert_eq!(PromptKey::from_input(Input::Enter), Some(PromptKey::Enter));
        assert_eq!(PromptKey::from_input(Input::Esc), Some(PromptKey::Esc));
    }

    #[test]
    fn resize_is_not_a_prompt_key() {
        // SPEC_v1_1.md §7: "resize ignored by prompt selection logic".
        // Translating resize to `None` lets the reducer treat it as a
        // no-op without a special case in every match arm.
        assert_eq!(
            PromptKey::from_input(Input::Resize {
                width: 120,
                height: 40
            }),
            None
        );
    }

    #[test]
    fn arrows_backspace_ctrl_unknown_are_not_prompt_keys() {
        // The direct-key reducer (Task 3) rejects these; the optional
        // navigation reducer (Task 4) consumes arrows separately and
        // does not go through `from_input`.
        for input in [
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Backspace,
            Input::Ctrl('c'),
            Input::Unknown,
        ] {
            assert_eq!(PromptKey::from_input(input), None, "{input:?}");
        }
    }
}
