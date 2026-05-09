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

/// A prompt body + choice list + footer, built up via the SPEC §5
/// builder API.
///
/// `ChoicePrompt<T>` is **just data** at this layer (Task 2d). The
/// reducer (`PromptAction<T>`, input-handling, selection) lands in
/// Task 3a on top of the same struct, and rendering (Task 5) reads
/// the same fields. Splitting "shape of the prompt" from "what
/// pressing a key does" keeps the builder testable without a
/// runtime — `ChoicePrompt::new().body(...).choice(...)` is a pure
/// expression that produces an inspectable value.
///
/// # Builder shape
///
/// SPEC §5 fixes the call sites the kit MUST support:
///
/// ```
/// use foglet_game::prompt::ChoicePrompt;
/// # #[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// # enum LootAction { Equip, Take, Pass }
/// let prompt = ChoicePrompt::new()
///     .body("You found this on the Giant Spider's corpse.")
///     .choice('e', LootAction::Equip, "Equip immediately")
///     .choice('t', LootAction::Take, "Take to inventory")
///     .choice('p', LootAction::Pass, "Pass");
/// assert_eq!(prompt.choices.len(), 3);
/// ```
///
/// `.disabled_if(cond, reason)` operates on the **most recently added
/// choice**. That is the form SPEC §5's wandering-monk example uses,
/// and it lets a prompt express "this row is disabled because …" in
/// the same chain as the row's `.choice(...)` call without naming the
/// row separately. Calling `.disabled_if(...)` before any `.choice(...)`
/// is a no-op (rather than a panic) so partial chains stay safe to
/// inspect during construction.
///
/// # Validation policy
///
/// The builder methods themselves never return `Result` — that would
/// break the fluent chain. Duplicate-hotkey detection (SPEC §4.1) runs
/// via [`ChoicePrompt::validate`], and the reducer constructors that
/// arrive in Task 3a will call it during their `try_*` builders. For
/// now, callers can call `prompt.validate()?` at the boundary if they
/// want the same guarantee.
///
/// # What this type intentionally does NOT do (yet)
///
/// - No selected-index state. That belongs to the reducer (Task 4).
/// - No `confirm` per-choice flag. That lands with `ConfirmPrompt`
///   (Task 6a).
/// - No theme/style storage. Per-choice `style: Option<StyleRole>`
///   already lives on `PromptChoice`; whole-prompt theming is a
///   render-layer concern (Task 5f).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoicePrompt<T> {
    /// Body lines, one per `.body(...)` call. Multi-line bodies are
    /// expressed as multiple `.body(...)` calls so the renderer (Task
    /// 5a) can preserve blank-line spacing without parsing embedded
    /// `\n` sequences.
    pub body: Vec<String>,
    /// Active choices in declaration order. The reducer (Task 3a)
    /// scans this list directly; the renderer iterates it for layout.
    pub choices: Vec<PromptChoice<T>>,
    /// Optional footer line — typically dynamic context like
    /// `"Your gold: 173g"`. `None` means the renderer omits the
    /// footer row entirely (no blank gap).
    pub footer: Option<String>,
}

impl<T> Default for ChoicePrompt<T> {
    /// Equivalent to [`ChoicePrompt::new`]. `Default` is provided so
    /// the type composes with derive macros and generic helpers that
    /// expect it.
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ChoicePrompt<T> {
    /// Start an empty prompt. Chain `.body(...)`, `.choice(...)`,
    /// `.disabled_if(...)`, `.footer(...)` to fill it in.
    pub fn new() -> Self {
        Self {
            body: Vec::new(),
            choices: Vec::new(),
            footer: None,
        }
    }

    /// Append one body line. Call repeatedly for multi-line bodies —
    /// each call is a logical paragraph the renderer (Task 5a) will
    /// wrap independently and separate from neighbours.
    pub fn body(mut self, line: impl Into<String>) -> Self {
        self.body.push(line.into());
        self
    }

    /// Append an enabled choice with `key`, stable id `value`, and
    /// display `label`.
    ///
    /// Argument order matches SPEC §5's example
    /// (`.choice('e', LootAction::Equip, "Equip immediately")`) — key,
    /// then game-id, then label. That ordering keeps the visually
    /// noisiest argument (the label string) at the end where it does
    /// not push the key/id off-screen in long chains.
    pub fn choice(mut self, key: impl Into<PromptKey>, value: T, label: impl Into<String>) -> Self {
        self.choices.push(PromptChoice::new(key, label, value));
        self
    }

    /// Disable the most recently added choice when `cond` is true,
    /// attaching `reason` for the renderer and the reducer's
    /// `Disabled` action.
    ///
    /// No-op in three cases (each chosen so partial builder chains
    /// stay safe to inspect mid-construction):
    ///
    /// 1. `cond` is false — the choice stays enabled, reason discarded.
    /// 2. No choice has been added yet — the call silently returns
    ///    `self` instead of panicking. SPEC §5 puts `.disabled_if`
    ///    immediately after `.choice(...)`; chains that violate that
    ///    are arguably author bugs but a panic at builder time would
    ///    crash the whole game on hot-reload.
    /// 3. `cond` is true but the prior choice was already disabled by
    ///    an earlier `.disabled_if(...)` or `.with_disabled_reason(...)`
    ///    — the new reason replaces the old one. Last writer wins, so
    ///    the most specific reason in the chain is the one the player
    ///    sees.
    pub fn disabled_if(mut self, cond: bool, reason: impl Into<String>) -> Self {
        if cond {
            if let Some(last) = self.choices.last_mut() {
                last.enabled = false;
                last.disabled_reason = Some(reason.into());
            }
        }
        self
    }

    /// Replace the footer line. Pass an empty string to set an empty
    /// footer (rendered as a blank row); to remove the footer entirely,
    /// drop the `.footer(...)` call from the chain.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Run SPEC §4.1 hotkey validation against the current choice
    /// list. Returns `Err(PromptError::DuplicateHotkey)` on the first
    /// collision in declaration order; `Ok(())` for a valid prompt
    /// (including the empty-choice case).
    ///
    /// Builder methods do not call this automatically — see the type
    /// docs for why. Call it explicitly at the seam where the prompt
    /// becomes player-visible (typically inside a Task 3a `try_new`).
    pub fn validate(&self) -> Result<(), PromptError> {
        validate_choices(&self.choices)
    }
}

impl<T: Clone> ChoicePrompt<T> {
    /// Send one [`Input`] through the prompt and return a typed
    /// [`PromptAction`] outcome (SPEC_v1_1.md §4.4).
    ///
    /// This is the **direct-key reducer entry point** for `ChoicePrompt`.
    /// Task 3a only wires the dispatch shape: every input currently maps
    /// to [`PromptAction::None`]. The substantive arms — direct hotkey
    /// matching, disabled-choice routing, Esc cancellation, the
    /// non-selection ignore branch — land in Tasks 3b–3e and 4. The
    /// signature is fixed here so downstream code (the optional
    /// [`crate::screen::Screen`] adapter, `ConfirmPrompt`, the docs
    /// examples) can compile against the eventual API immediately.
    ///
    /// # Why `&self`, not `&mut self`?
    ///
    /// Direct-key prompts have no internal state to mutate — selection
    /// is whatever the player just pressed, and the reducer's job is to
    /// classify that press, not remember it. The arrow/Enter navigation
    /// reducer (Task 4) carries a selected-index cursor; that variant
    /// will land as a separate `&mut self` method or a state struct, so
    /// this method stays cheap to call (e.g. from a render path that
    /// previews "would this key select something?").
    ///
    /// # `T: Clone`
    ///
    /// `PromptAction::Selected(T)` and `PromptAction::Disabled { id, .. }`
    /// hand the player a copy of the choice's stable id. Real games use
    /// small `Copy` enums there, so the `Clone` bound is effectively
    /// free; constraining it on the impl block (rather than on every
    /// method) keeps `ChoicePrompt::new`/`body`/`choice`/etc. usable
    /// with non-`Clone` `T` during construction-only flows like tests
    /// that introspect the data without ever calling `handle`.
    pub fn handle(&self, input: Input) -> PromptAction<T> {
        // Translate the raw runtime event into the prompt's narrower
        // vocabulary first. Anything that is not a printable hotkey,
        // Enter, or Esc collapses to `None` here (resize, arrows,
        // backspace, Ctrl combos, Unknown) — SPEC_v1_1.md §7's "resize
        // ignored by prompt selection logic" falls out of this single
        // gate without per-arm special cases.
        let Some(key) = PromptKey::from_input(input) else {
            return PromptAction::None;
        };

        // Direct hotkey path (Task 3b). Only `Char` keys participate in
        // direct-key matching here; `Enter`/`Esc` are handled by Tasks
        // 3d (cancellation) and 4c (navigation-mode confirm), so they
        // currently fall through to `None`. Choices declared with
        // `PromptKey::Enter`/`PromptKey::Esc` are exotic data-level
        // configurations that the navigation/cancellation reducers will
        // address explicitly.
        if let PromptKey::Char(_) = key {
            // Linear scan — choice lists are short (SPEC §4.4 ~5
            // typical) and `validate()` already enforces uniqueness, so
            // the first match is unambiguous. Keys are pre-normalised
            // to lowercase on both sides (`PromptKey::char` on storage,
            // `PromptKey::from_input` on input), so equality is the
            // entire comparison — no per-call `to_ascii_lowercase`.
            for choice in &self.choices {
                if choice.key == key && choice.enabled {
                    return PromptAction::Selected(choice.value.clone());
                }
            }
        }

        // Tasks 3c (disabled-key Disabled), 3d (Esc → Cancelled when
        // configured), and 4c (Enter on highlighted choice) layer in
        // on top of this scan. Until they land, anything that does not
        // match an enabled hotkey is a no-op so the prompt never
        // invents a selection from a press the player did not make.
        PromptAction::None
    }
}

/// Outcome of sending one [`Input`] through a prompt reducer
/// (SPEC_v1_1.md §4.4).
///
/// Generic over the game's stable choice id `T` so the kit stays
/// decoupled from any particular inventory/shop/dialog vocabulary —
/// see [`PromptChoice`] for why stable ids beat returning labels.
///
/// # Variants and when each is produced
///
/// - [`PromptAction::None`] — the input was not prompt-relevant or was
///   one of the inputs the SPEC says to ignore (resize, unknown keys,
///   modifier-only events). The caller SHOULD render the prompt
///   unchanged. `None` covers the SPEC §4.4 rules:
///   - "`Resize` input MUST NOT select a choice."
///   - "Unknown keys SHOULD produce `None`."
/// - [`PromptAction::Selected`] — the player pressed an enabled
///   choice's hotkey (or pressed Enter on the highlighted choice in the
///   navigation mode). The wrapped `T` is the stable id of that choice.
/// - [`PromptAction::Disabled`] — the player pressed a hotkey bound to
///   a *disabled* choice. SPEC §4.1 requires that disabled choices
///   reserve their hotkey; rather than swallowing the press silently,
///   the reducer surfaces it so the game can show a status message
///   like "can't equip — bag full". `reason` mirrors
///   `PromptChoice::disabled_reason` so the game does not have to
///   re-derive the cause.
/// - [`PromptAction::Cancelled`] — the player pressed Esc on a prompt
///   that opted into cancellation. Prompts that disable Esc never
///   produce this variant.
/// - [`PromptAction::ConfirmRequested`] — the player selected a choice
///   that is configured to require a follow-up confirmation. The kit
///   does **not** perform the confirmation itself; it hands the id back
///   so the game can stage a `ConfirmPrompt` (Task 6a) and decide what
///   to do on yes/no. Reserving this variant at 3a, even though only
///   `ConfirmPrompt` will produce it, keeps the public API stable
///   across the rest of v1.1.
///
/// # Why a flat enum, not nested types?
///
/// Game match arms read more naturally against a flat enum
/// (`PromptAction::Selected(id) => ...`) than against a tree (e.g.
/// `Outcome::Choice(ChoiceOutcome::Selected(id))`). Equality and
/// `Debug` derives stay cheap, and the variant set is the SPEC's
/// vocabulary — adding to it is a SPEC-level change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction<T> {
    /// No state-changing outcome — the input was ignored. Includes
    /// resize, unknown/modifier-only inputs, and (depending on the
    /// reducer's configuration) Esc when cancellation is disabled.
    None,
    /// The player selected an enabled choice; the wrapped value is
    /// that choice's stable id.
    Selected(T),
    /// The player pressed a hotkey bound to a disabled choice.
    /// Surfaced (not swallowed) so the game can show feedback —
    /// SPEC §4.1's "disabled rows still reserve their hotkey".
    Disabled {
        /// Stable id of the disabled choice the player pressed.
        id: T,
        /// Optional human-readable reason carried over from
        /// [`PromptChoice::disabled_reason`].
        reason: Option<String>,
    },
    /// The player cancelled the prompt (typically via Esc on a
    /// cancellation-enabled prompt).
    Cancelled,
    /// The player selected a choice that requires a follow-up
    /// confirmation step. Produced only by future variants of the
    /// reducer (see `ConfirmPrompt`, Task 6a); `ChoicePrompt::handle`
    /// itself does not currently emit this variant.
    ConfirmRequested(T),
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
    fn prompt_builder_collects_body_choices_and_footer() {
        // Mirrors the SPEC §5 Giant Spider example. Asserts the
        // builder records each call into the corresponding field
        // without re-ordering or de-duplicating.
        let prompt = ChoicePrompt::new()
            .body("You found this on the Giant Spider's corpse.")
            .choice('e', LootAction::Equip, "Equip immediately")
            .choice('t', LootAction::Take, "Take to inventory")
            .choice('p', LootAction::Pass, "Pass")
            .footer("Press a key to choose.");

        assert_eq!(
            prompt.body,
            vec!["You found this on the Giant Spider's corpse."]
        );
        assert_eq!(prompt.footer.as_deref(), Some("Press a key to choose."));

        let labels: Vec<&str> = prompt.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Equip immediately", "Take to inventory", "Pass"],
        );

        let keys: Vec<PromptKey> = prompt.choices.iter().map(|c| c.key).collect();
        assert_eq!(
            keys,
            vec![
                PromptKey::Char('e'),
                PromptKey::Char('t'),
                PromptKey::Char('p'),
            ],
        );

        // All defaults: enabled, no hint, no disabled reason.
        for choice in &prompt.choices {
            assert!(choice.enabled, "{:?} should default enabled", choice.label);
            assert_eq!(choice.disabled_reason, None);
            assert_eq!(choice.hint, None);
        }
    }

    #[test]
    fn prompt_builder_body_calls_accumulate_in_order() {
        // SPEC §5 wandering-monk example uses two `.body(...)` calls to
        // express a two-paragraph body. Each call appends one entry; the
        // renderer (Task 5a) is responsible for blank-line spacing.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("A wandering monk approaches after the battle...")
            .body("\"I carry mana potions for those who wield magic.\"");
        assert_eq!(prompt.body.len(), 2);
        assert_eq!(
            prompt.body[0],
            "A wandering monk approaches after the battle..."
        );
        assert!(prompt.body[1].starts_with("\"I carry mana potions"));
    }

    #[test]
    fn disabled_if_true_marks_last_choice_with_reason() {
        // Wandering-monk vendor: the mana-potion row is added, then
        // `.disabled_if(!can_buy, ...)` flips it to disabled when the
        // condition is true. The reason text reaches the choice for
        // both renderer and reducer to surface.
        let prompt = ChoicePrompt::new()
            .choice('m', LootAction::Take, "Mana potions")
            .disabled_if(true, "not enough gold or potion bag is full")
            .choice('n', LootAction::Pass, "No thanks");

        assert!(!prompt.choices[0].enabled);
        assert_eq!(
            prompt.choices[0].disabled_reason.as_deref(),
            Some("not enough gold or potion bag is full"),
        );
        // Subsequent choices are unaffected — `.disabled_if(...)` only
        // touches the most recently added row, never neighbours.
        assert!(prompt.choices[1].enabled);
    }

    #[test]
    fn disabled_if_false_leaves_last_choice_enabled() {
        // Same builder shape, condition flipped: the prompt stays fully
        // enabled. The reason string is silently discarded — that is
        // the SPEC §5 expectation, since the chain reads "disable IF
        // can't buy", and not-can't-buy means no disable to apply.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Take, "Mana potions")
            .disabled_if(false, "would-be reason")
            .choice('n', LootAction::Pass, "No thanks");

        assert!(prompt.choices[0].enabled);
        assert_eq!(prompt.choices[0].disabled_reason, None);
    }

    #[test]
    fn disabled_if_with_no_prior_choice_is_a_no_op() {
        // Out-of-order chain: `.disabled_if(...)` before any
        // `.choice(...)`. Documented behaviour is "silently no-op" so
        // partial builder values stay safe to inspect — never panic
        // mid-construction (a hot-reloaded game would crash).
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .disabled_if(true, "nothing to disable")
            .choice('e', LootAction::Equip, "Equip");
        assert!(prompt.choices[0].enabled);
        assert_eq!(prompt.choices[0].disabled_reason, None);
    }

    #[test]
    fn disabled_if_replaces_prior_disabled_reason() {
        // "Last writer wins" — a later `.disabled_if(true, ...)` on the
        // same row overrides the earlier reason. Lets a chain start
        // from a `with_disabled_reason` default and refine it later.
        let prompt = ChoicePrompt::new()
            .choice('t', LootAction::Take, "Tip for rumor")
            .disabled_if(true, "stale reason")
            .disabled_if(true, "need 50g");
        assert!(!prompt.choices[0].enabled);
        assert_eq!(
            prompt.choices[0].disabled_reason.as_deref(),
            Some("need 50g"),
        );
    }

    #[test]
    fn footer_replaces_previous_footer() {
        // `.footer(...)` is a setter, not an appender — only the last
        // call survives. Mirrors the wandering-monk example which sets
        // the footer once at the end of the chain.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .footer("Your gold: 100g")
            .footer("Your gold: 173g");
        assert_eq!(prompt.footer.as_deref(), Some("Your gold: 173g"));
    }

    #[test]
    fn builder_validate_passes_for_unique_keys_and_rejects_duplicates() {
        // The opt-in `validate()` shim runs SPEC §4.1 hotkey validation
        // against the assembled prompt. Builder methods themselves do
        // not return Result; validation is the explicit seam between
        // "shape building" and "ready for the player".
        let good: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");
        assert_eq!(good.validate(), Ok(()));

        let bad: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('E', LootAction::Take, "Eat");
        assert!(matches!(
            bad.validate(),
            Err(PromptError::DuplicateHotkey {
                key: PromptKey::Char('e'),
                ..
            }),
        ));
    }

    #[test]
    fn default_matches_new() {
        // `Default` is a convenience for derive macros / generic code.
        // It MUST behave identically to `new()` so callers can pick
        // either without surprises.
        let a: ChoicePrompt<LootAction> = ChoicePrompt::new();
        let b: ChoicePrompt<LootAction> = ChoicePrompt::default();
        assert_eq!(a, b);
    }

    #[test]
    fn prompt_action_variants_construct_and_compare() {
        // SPEC §4.4: the action enum carries None / Selected / Disabled
        // / Cancelled / ConfirmRequested. Verify each variant builds
        // with the expected payload shape and that equality/Debug derive
        // correctly — downstream tests rely on `assert_eq!` against
        // these values across the whole prompt module.
        let none: PromptAction<LootAction> = PromptAction::None;
        let selected = PromptAction::Selected(LootAction::Equip);
        let disabled = PromptAction::Disabled {
            id: LootAction::Take,
            reason: Some("full".to_string()),
        };
        let cancelled: PromptAction<LootAction> = PromptAction::Cancelled;
        let confirm = PromptAction::ConfirmRequested(LootAction::Pass);

        // Distinctness: variants are not equal to one another.
        assert_ne!(none, PromptAction::Selected(LootAction::Equip));
        assert_ne!(selected, PromptAction::Selected(LootAction::Take));
        assert_ne!(cancelled, none);
        assert_ne!(confirm, selected);

        // Disabled carries an optional reason; both `Some` and `None`
        // forms are valid (Task 3c will exercise the `Some` path).
        assert_eq!(
            disabled,
            PromptAction::Disabled {
                id: LootAction::Take,
                reason: Some("full".to_string()),
            }
        );
        assert_ne!(
            disabled,
            PromptAction::Disabled {
                id: LootAction::Take,
                reason: None,
            }
        );

        // Clone round-trips preserve payload — sanity check that the
        // derive lines up with the `T: Clone` impl bound on `handle`.
        assert_eq!(selected.clone(), PromptAction::Selected(LootAction::Equip));
    }

    #[test]
    fn handle_direct_key_lowercase_selects_enabled_choice() {
        // SPEC_v1_1.md §4.4: pressing an enabled choice's hotkey returns
        // `Selected(id)` with the stable game id. The lowercase press
        // is the canonical path — choices are stored normalised, so this
        // is a direct equality match.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass");

        assert_eq!(
            prompt.handle(Input::Char('e')),
            PromptAction::Selected(LootAction::Equip),
        );
        assert_eq!(
            prompt.handle(Input::Char('t')),
            PromptAction::Selected(LootAction::Take),
        );
        assert_eq!(
            prompt.handle(Input::Char('p')),
            PromptAction::Selected(LootAction::Pass),
        );
    }

    #[test]
    fn handle_direct_key_uppercase_selects_same_enabled_choice_as_lowercase() {
        // SPEC §4.1: "Character hotkeys MUST match case-insensitively
        // by default." This is the test the checklist names explicitly
        // for Task 3b — uppercase input must produce the same Selected
        // outcome as lowercase, with no shift handling required from
        // the game.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        assert_eq!(
            prompt.handle(Input::Char('E')),
            prompt.handle(Input::Char('e')),
        );
        assert_eq!(
            prompt.handle(Input::Char('E')),
            PromptAction::Selected(LootAction::Equip),
        );
        assert_eq!(
            prompt.handle(Input::Char('T')),
            PromptAction::Selected(LootAction::Take),
        );
    }

    #[test]
    fn handle_unmatched_key_returns_none() {
        // Pressing a printable key that no choice binds is a no-op.
        // Distinguishes "the input was prompt-relevant but not bound"
        // from "the input was not prompt-relevant at all" — both flow
        // through `None` here, but for different reasons; future
        // disabled-routing (Task 3c) will diverge for the bound case.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        assert_eq!(prompt.handle(Input::Char('z')), PromptAction::None);
        assert_eq!(prompt.handle(Input::Char('1')), PromptAction::None);
    }

    #[test]
    fn handle_ignores_non_prompt_inputs() {
        // SPEC_v1_1.md §7: resize MUST NOT select a choice. Arrows,
        // backspace, Ctrl combos, and Unknown collapse through
        // `PromptKey::from_input -> None`, so the reducer reports `None`
        // without touching the choice list. Task 4 will add an opt-in
        // navigation mode that consumes arrows separately.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        for input in [
            Input::Resize {
                width: 80,
                height: 24,
            },
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::Backspace,
            Input::Ctrl('c'),
            Input::Unknown,
        ] {
            assert_eq!(
                prompt.handle(input),
                PromptAction::None,
                "non-prompt input {input:?} must be a no-op",
            );
        }
    }

    #[test]
    fn handle_does_not_select_disabled_choice_via_direct_key() {
        // Task 3b is "direct hotkey selection of an enabled choice".
        // Disabled-key routing is Task 3c — until that lands, pressing
        // a disabled hotkey must NOT return `Selected` (that would be
        // the worst possible silent bug: a disabled label that quietly
        // fires the action). `None` is the safe interim outcome; 3c
        // will upgrade it to `Disabled { id, reason }`.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Take, "Mana potions")
            .disabled_if(true, "full")
            .choice('n', LootAction::Pass, "No thanks");

        assert_eq!(prompt.handle(Input::Char('m')), PromptAction::None);
        assert_eq!(prompt.handle(Input::Char('M')), PromptAction::None);
        // Enabled neighbour still selects normally — the disabled row
        // does not poison the rest of the prompt.
        assert_eq!(
            prompt.handle(Input::Char('n')),
            PromptAction::Selected(LootAction::Pass),
        );
    }

    #[test]
    fn handle_enter_and_esc_are_no_ops_until_their_tasks_land() {
        // Enter belongs to navigation mode (Task 4c); Esc belongs to
        // cancellation (Task 3d). Neither is wired yet, so both must
        // be no-ops on a default direct-key prompt — guards against a
        // future change accidentally selecting the first choice on
        // Enter, or marking the prompt cancelled on stray Esc.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");

        assert_eq!(prompt.handle(Input::Enter), PromptAction::None);
        assert_eq!(prompt.handle(Input::Esc), PromptAction::None);
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
