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

/// Semantic style role for prompt text and choices (SPEC_v1_1.md §4.7).
///
/// Prompts attach roles, not raw ANSI. The render layer (Task 5f) maps
/// roles to a Ratatui `Style`, which keeps two properties that the
/// SPEC requires:
///
/// 1. **Color is never the only carrier of meaning** — disabled rows
///    still render visibly different in monochrome because the
///    renderer adds a marker/prefix when it sees `StyleRole::Disabled`,
///    not just because the text is dim.
/// 2. **Themes are overridable** — a game (or a future operator config)
///    can swap the role-to-`Style` mapping without touching the prompt
///    data, because the data only carries the role enum.
///
/// The variants are the SPEC-listed initial roles. Adding a role is a
/// SPEC-level change; do not extend this enum to carry per-prompt
/// styling — use `hint` / `disabled_reason` text instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StyleRole {
    /// Default text style. Body lines, footers, plain choice labels.
    Normal,
    /// Title text — modal headers, section banners.
    Title,
    /// Emphasised inline text (warnings, callouts) without escalating
    /// to `Error`.
    Emphasis,
    /// De-emphasised text — secondary lines, footnote-style hints.
    Muted,
    /// Inline hint text attached to a choice (e.g. `"need 50g"`).
    Hint,
    /// Error feedback text — selecting a disabled choice, validation
    /// messages.
    Error,
    /// Success feedback text — `"Moved Room 7 key to inventory."`.
    Success,
    /// Numeric currency / cost annotations.
    Currency,
    /// Disabled choice rows. Renderer pairs this with a monochrome
    /// marker (Task 5c) so the disabled state survives a black/white
    /// terminal.
    Disabled,
    /// Highlighted hotkey character inside a choice marker like `(E)`.
    Hotkey,
}

/// One selectable option inside a `ChoicePrompt` (SPEC_v1_1.md §4.2).
///
/// `T` is the game-facing **stable id** the prompt returns when the
/// player selects this choice — usually a small `enum` defined by the
/// game. Stable ids decouple the *display* of a prompt (which changes
/// when localisation, copy edits, or dynamic labels arrive) from the
/// *semantics* of a selection (which the game's state machine wants
/// to stay constant). Compare with returning the label string: a copy
/// tweak from `"Take"` to `"Pick up"` would silently break a `match`.
///
/// # Building one
///
/// Construction goes through [`PromptChoice::new`] so the common path
/// (an enabled choice with a hotkey, a label, and an id) stays a
/// one-liner. Optional fields — `disabled_reason`, `hint`, `style` —
/// are set with `with_*` chain methods. The richer
/// `disabled_if`/`.choice(...)` builder helpers land in Task 2d on top
/// of this same data.
///
/// # Disabled choices
///
/// `enabled` defaults to `true`. Setting `enabled = false` SHOULD be
/// paired with [`PromptChoice::with_disabled_reason`] so the renderer
/// (Task 5c) can show *why* the option is unavailable in monochrome —
/// e.g. `- [M] Mana potions ... (full)`. The reducer (Task 3c) returns
/// the same reason string in `PromptAction::Disabled`, so the game can
/// surface it as feedback without re-deriving the cause.
///
/// # What this type intentionally does NOT do
///
/// - It does not hold game state (gold, inventory). The author wires
///   `enabled = gold >= price` at construction time; the prompt does
///   not re-evaluate predicates on its own.
/// - It does not validate hotkey uniqueness. Duplicate-key detection
///   is a `ChoicePrompt`-level concern (Task 2c) because it depends on
///   the *set* of active choices.
/// - It does not own a confirmation policy yet. SPEC §4.2 reserves a
///   `confirm` field; it lands when `ConfirmPrompt` does (Task 6a) so
///   the data and the reducer arrive together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptChoice<T> {
    /// Direct-input hotkey. Stored normalised — see [`PromptKey::char`].
    pub key: PromptKey,
    /// Display label. Renderer wraps and styles this; MUST NOT contain
    /// raw terminal control sequences (SPEC §4.2).
    pub label: String,
    /// Stable game-facing id returned by the reducer on selection.
    /// Typically a small game-defined enum.
    pub value: T,
    /// `true` (default) means the player can select this choice.
    /// `false` makes the choice render as disabled and selecting it
    /// returns `PromptAction::Disabled` rather than `Selected`.
    pub enabled: bool,
    /// Optional human-readable reason shown next to a disabled choice
    /// and echoed back through `PromptAction::Disabled`. Empty when
    /// the choice is enabled.
    pub disabled_reason: Option<String>,
    /// Optional inline hint shown after the label — e.g. cost or
    /// capacity (`"50g"`, `"0/26"`). Independent of `disabled_reason`
    /// because an enabled choice can still want a hint.
    pub hint: Option<String>,
    /// Optional semantic style role override. `None` means the
    /// renderer picks the default for the choice's enabled/disabled
    /// state.
    pub style: Option<StyleRole>,
}

impl<T> PromptChoice<T> {
    /// Build an enabled choice with the three always-required fields.
    ///
    /// `key` accepts anything that converts into a [`PromptKey`] —
    /// most commonly a `char`, which goes through `PromptKey::char`
    /// and lowercases ASCII letters automatically. `label` accepts any
    /// `Into<String>` so callers can pass `&str` or `String`.
    pub fn new(key: impl Into<PromptKey>, label: impl Into<String>, value: T) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            value,
            // SPEC §4.2: "`enabled` bool, default `true`". Centralising
            // the default here means the rest of the codebase never has
            // to remember it.
            enabled: true,
            disabled_reason: None,
            hint: None,
            style: None,
        }
    }

    /// Mark this choice disabled and attach the reason. Pairs with
    /// SPEC §4.2's "disabled choices MUST render visibly distinct" by
    /// giving the renderer a non-empty string to surface.
    pub fn with_disabled_reason(mut self, reason: impl Into<String>) -> Self {
        self.enabled = false;
        self.disabled_reason = Some(reason.into());
        self
    }

    /// Attach an inline hint (cost, capacity, side note). Does not
    /// touch `enabled` — disabled hints are valid (`"need 50g"`).
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Override the semantic style role. Use sparingly — the renderer
    /// already picks sensible defaults from `enabled` and the choice's
    /// position; manual overrides are for emphasis/error/success rows.
    pub fn with_style(mut self, role: StyleRole) -> Self {
        self.style = Some(role);
        self
    }
}

/// Construction-time error surfaced when a prompt's choice list violates
/// an invariant the renderer / reducer depend on.
///
/// Returned from validators like [`validate_choices`] (Task 2c) and,
/// later, builder finalisation (Task 2d) and `ChoicePrompt::new`
/// (Task 3a). Authoring errors are intentionally separate from runtime
/// `PromptAction` outcomes — a prompt with duplicate hotkeys is never a
/// player-facing condition, it is a programmer bug that should fail loud
/// the moment the prompt is built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromptError {
    /// Two choices in the same prompt share a hotkey after
    /// case-normalisation. SPEC_v1_1.md §4.1 makes this a hard error
    /// because disabled choices still *reserve* their hotkey: a
    /// disabled `(E)` row plus an enabled `(e)` row would either route
    /// the press to the disabled handler (player gets a "can't do that"
    /// message for an action they expected to work) or to the enabled
    /// one (the disabled label silently lies). Refusing to construct
    /// such a prompt is the only safe option.
    #[error(
        "duplicate prompt hotkey {key:?}: choices {first_label:?} and {second_label:?} both bind it"
    )]
    DuplicateHotkey {
        /// The shared, normalised key.
        key: PromptKey,
        /// Label of the first choice that registered this key (in
        /// declaration order).
        first_label: String,
        /// Label of the second choice attempting to bind the same key.
        second_label: String,
    },
}

/// Reject a choice list that violates SPEC_v1_1.md §4.1 hotkey rules.
///
/// Specifically: **every** choice in the list reserves its hotkey,
/// regardless of `enabled`. The SPEC's exact wording is "Disabled
/// choices still reserve their hotkey by default so a disabled option
/// cannot accidentally trigger another action." That sentence is the
/// reason this validator does not filter by `enabled` — both
/// `(E) Equip` (enabled) and `(E) Equip — full` (disabled) compete for
/// the same `e` press, and the resolution can only ever be "fail at
/// construction" because either runtime resolution surprises the
/// player.
///
/// Keys are compared after the lowercase normalisation that
/// [`PromptKey::char`] applies, so `'e'` and `'E'` collide as expected.
/// `Enter` and `Esc` variants are also compared structurally — a prompt
/// declaring two `Enter`-keyed choices is rejected for the same reason
/// even though that combination is exotic.
///
/// Returns the *first* duplicate found in declaration order so the
/// error message points at a deterministic pair, which makes
/// regression-style tests easy to write.
pub fn validate_choices<T>(choices: &[PromptChoice<T>]) -> Result<(), PromptError> {
    // Linear scan with a small Vec — choice lists are short (single
    // digits in practice; SPEC §4.4 talks about ~5 typical), so the
    // O(n²) cost is dwarfed by the constant factor of any HashMap and
    // keeps the implementation allocation-free for the common path of
    // 2-4 choices that pass validation.
    for (i, choice) in choices.iter().enumerate() {
        for prior in &choices[..i] {
            if prior.key == choice.key {
                return Err(PromptError::DuplicateHotkey {
                    key: choice.key,
                    first_label: prior.label.clone(),
                    second_label: choice.label.clone(),
                });
            }
        }
    }
    Ok(())
}

impl From<char> for PromptKey {
    /// Lift a `char` straight into a [`PromptKey`] via
    /// [`PromptKey::char`] so call sites like
    /// `PromptChoice::new('e', "Equip", Action::Equip)` stay terse.
    fn from(c: char) -> Self {
        PromptKey::char(c)
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

    /// Game-defined stable id used in the choice-model tests. Mirrors
    /// the shape an actual game would use — small enum, `Eq`, copy.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LootAction {
        Equip,
        Take,
        Pass,
    }

    #[test]
    fn choice_new_defaults_enabled_true_with_no_metadata() {
        // SPEC §4.2: `enabled` defaults to true; metadata fields stay
        // unset until the author opts in. This is the path 99% of
        // prompt choices take, so it has to stay a one-liner.
        let choice = PromptChoice::new('e', "Equip immediately", LootAction::Equip);

        assert_eq!(choice.key, PromptKey::Char('e'));
        assert_eq!(choice.label, "Equip immediately");
        assert_eq!(choice.value, LootAction::Equip);
        assert!(choice.enabled);
        assert_eq!(choice.disabled_reason, None);
        assert_eq!(choice.hint, None);
        assert_eq!(choice.style, None);
    }

    #[test]
    fn choice_new_lowercases_ascii_hotkey_via_from_char() {
        // The `From<char> for PromptKey` impl funnels through
        // `PromptKey::char`, so `'E'` and `'e'` produce the same key
        // without callers thinking about it.
        let upper = PromptChoice::new('E', "Equip", LootAction::Equip);
        let lower = PromptChoice::new('e', "Equip", LootAction::Equip);
        assert_eq!(upper.key, lower.key);
    }

    #[test]
    fn with_disabled_reason_preserves_reason_and_flips_enabled() {
        // SPEC §4.2: disabled choices carry a reason that the renderer
        // and reducer both surface. Setting the reason MUST also flip
        // `enabled = false` — otherwise authors would have to remember
        // both calls and an inconsistent state could ship.
        let choice =
            PromptChoice::new('m', "Mana potions", LootAction::Take).with_disabled_reason("full");

        assert!(!choice.enabled);
        assert_eq!(choice.disabled_reason.as_deref(), Some("full"));
    }

    #[test]
    fn with_hint_does_not_disable_choice() {
        // Hints (cost, capacity) are independent of disabled state —
        // `"Coffee — 25g"` is an enabled choice with a hint, not a
        // disabled one.
        let choice = PromptChoice::new('b', "Black coffee", LootAction::Take).with_hint("25g");

        assert!(choice.enabled);
        assert_eq!(choice.hint.as_deref(), Some("25g"));
        assert_eq!(choice.disabled_reason, None);
    }

    #[test]
    fn with_style_overrides_role() {
        let choice = PromptChoice::new('p', "Pass", LootAction::Pass).with_style(StyleRole::Muted);
        assert_eq!(choice.style, Some(StyleRole::Muted));
    }

    #[test]
    fn validate_choices_accepts_unique_keys() {
        // Baseline: a normal loot prompt with three distinct hotkeys
        // passes validation. Empty lists also pass (a prompt with zero
        // choices is degenerate but not malformed at this layer).
        let choices = [
            PromptChoice::new('e', "Equip", LootAction::Equip),
            PromptChoice::new('t', "Take", LootAction::Take),
            PromptChoice::new('p', "Pass", LootAction::Pass),
        ];
        assert_eq!(validate_choices(&choices), Ok(()));

        let empty: [PromptChoice<LootAction>; 0] = [];
        assert_eq!(validate_choices(&empty), Ok(()));
    }

    #[test]
    fn validate_choices_rejects_duplicate_active_keys() {
        // SPEC §4.1: "Duplicate active hotkeys in one prompt MUST be
        // rejected or produce a clear construction error." The error
        // points at both labels so the failing test (or the panicking
        // builder, in Task 2d) names the exact culprits.
        let choices = [
            PromptChoice::new('e', "Equip", LootAction::Equip),
            PromptChoice::new('t', "Take", LootAction::Take),
            PromptChoice::new('e', "Eat", LootAction::Pass),
        ];
        assert_eq!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Char('e'),
                first_label: "Equip".to_string(),
                second_label: "Eat".to_string(),
            }),
        );
    }

    #[test]
    fn validate_choices_normalises_case_before_comparing() {
        // `'E'` and `'e'` collapse to the same `PromptKey::Char('e')`,
        // so this is a duplicate even though the source `char` literals
        // differ. Authors who try to "split" a hotkey across cases get
        // a hard fail instead of a runtime ambiguity.
        let choices = [
            PromptChoice::new('E', "Equip", LootAction::Equip),
            PromptChoice::new('e', "Eat", LootAction::Pass),
        ];
        assert!(matches!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Char('e'),
                ..
            })
        ));
    }

    #[test]
    fn validate_choices_rejects_active_disabled_collision() {
        // SPEC §4.1: "Disabled choices still reserve their hotkey by
        // default." A disabled row paired with an enabled row sharing
        // the same key is exactly the surprise the SPEC forbids — the
        // disabled label would either swallow the press (and the player
        // sees a misleading "can't do that") or be ignored (and the
        // visible disabled marker lies). Reject at construction.
        let choices = [
            PromptChoice::new('m', "Mana potions", LootAction::Take).with_disabled_reason("full"),
            PromptChoice::new('m', "Mead", LootAction::Equip),
        ];
        assert!(matches!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Char('m'),
                ..
            })
        ));
    }

    #[test]
    fn validate_choices_rejects_two_disabled_with_same_key() {
        // Even two disabled rows sharing a key is malformed: the
        // renderer would print two `(M)` markers, and the moment one is
        // re-enabled the prompt becomes ambiguous. Same rule, same
        // error.
        let choices = [
            PromptChoice::new('m', "Mana potions", LootAction::Take).with_disabled_reason("full"),
            PromptChoice::new('M', "Mead", LootAction::Equip).with_disabled_reason("none left"),
        ];
        assert!(matches!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey { .. })
        ));
    }

    #[test]
    fn validate_choices_reports_first_duplicate_in_declaration_order() {
        // When several pairs collide, the validator returns the first
        // pair encountered scanning forward. Determinism makes failing
        // tests stable — otherwise refactors that reorder choices would
        // flip which label landed in the error.
        let choices = [
            PromptChoice::new('a', "Alpha", LootAction::Equip),
            PromptChoice::new('b', "Bravo", LootAction::Take),
            PromptChoice::new('a', "Apple", LootAction::Pass),
            PromptChoice::new('b', "Banana", LootAction::Pass),
        ];
        assert_eq!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Char('a'),
                first_label: "Alpha".to_string(),
                second_label: "Apple".to_string(),
            }),
        );
    }

    #[test]
    fn validate_choices_treats_enter_and_esc_as_keys_too() {
        // Choices keyed on Enter/Esc are exotic but legal at the data
        // level. Two `Enter`-keyed choices still collide — same rule,
        // no special case. Ensures the validator does not silently skip
        // non-`Char` variants.
        let choices = [
            PromptChoice::new(PromptKey::Enter, "Confirm", LootAction::Equip),
            PromptChoice::new(PromptKey::Enter, "Also confirm", LootAction::Take),
        ];
        assert_eq!(
            validate_choices(&choices),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Enter,
                first_label: "Confirm".to_string(),
                second_label: "Also confirm".to_string(),
            }),
        );
    }

    #[test]
    fn builder_chain_combines_disabled_and_hint() {
        // Realistic vendor case: tip-for-rumor priced at 50g while the
        // player has 40g — disabled, with both a `disabled_reason`
        // explaining why and a `hint` carrying the cost.
        let choice = PromptChoice::new('t', "Tip for rumor", LootAction::Take)
            .with_hint("50g")
            .with_disabled_reason("need 50g");

        assert!(!choice.enabled);
        assert_eq!(choice.hint.as_deref(), Some("50g"));
        assert_eq!(choice.disabled_reason.as_deref(), Some("need 50g"));
    }
}
