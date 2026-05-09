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
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

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
/// - No `confirm` per-choice flag. That lands with `ConfirmPrompt`
///   (Task 6a).
/// - No theme/style storage. Per-choice `style: Option<StyleRole>`
///   already lives on `PromptChoice`; whole-prompt theming is a
///   render-layer concern (Task 5f).
///
/// # Optional arrow/Enter navigation cursor
///
/// `selected` is `None` for the default direct-key flow (every
/// SPEC §5 example so far). Calling [`ChoicePrompt::navigable`] with `true`
/// switches the prompt into the optional arrow/Enter mode by seeding
/// `selected` with the index of the first enabled choice; Up/Down/
/// Enter behavior layers on in Tasks 4b–4c. Storing the cursor on the
/// prompt (not in a sibling state struct) keeps the navigation-mode
/// path round-trippable through `Clone`/`PartialEq` for tests, and
/// matches how the v1 `DialogState` reducer stores its cursor
/// alongside its data.
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
    /// When `true`, pressing Esc on this prompt returns
    /// [`PromptAction::Cancelled`]; when `false` (the default), Esc
    /// collapses to [`PromptAction::None`] alongside other ignored
    /// inputs. SPEC_v1_1.md §4.3 lists Esc support as
    /// "when configured" — opt-in cancellation prevents a stray Esc
    /// from dismissing a load-bearing prompt (vendor confirmation, save
    /// overwrite) that the game wants to force the player to resolve.
    pub cancellable: bool,
    /// Highlighted choice index for arrow/Enter navigation mode
    /// (SPEC_v1_1.md §4.5).
    ///
    /// `None` — direct-key mode (the default). Hotkeys select choices;
    /// there is no cursor and Up/Down are ignored.
    ///
    /// `Some(i)` — navigation mode is active and the cursor sits on
    /// `choices[i]`. Set by [`ChoicePrompt::navigable`] to the first
    /// enabled choice's index, then advanced by Up/Down (Task 4b) and
    /// confirmed by Enter (Task 4c). Index validity is the prompt's
    /// invariant: `i < choices.len()` whenever the field is `Some`.
    /// `None` is also produced when navigation is requested but no
    /// choice is enabled — direct-key prompts already handle the empty
    /// case as "no selection possible", and an out-of-bounds `Some(0)`
    /// on an empty prompt would just be a panic waiting to happen.
    pub selected: Option<usize>,
    /// Optional Vim-style `j`/`k` navigation (SPEC_v1_1.md §4.5 alt
    /// keymap). Default `false`, meaning only `Up`/`Down` arrows drive
    /// the cursor. Set via [`ChoicePrompt::vim_navigation`]; consumed by
    /// [`ChoicePrompt::step_from_input`] which treats `j` as Down and
    /// `k` as Up when this flag is on.
    ///
    /// The flag is independent of [`ChoicePrompt::selected`]: turning it
    /// on without `.navigable(true)` is harmless (no cursor → no
    /// movement), and turning it off does not clear the cursor. The two
    /// configs compose so authors can ship a default-arrow build and a
    /// "Vim mode" build from the same prompt definition.
    pub vim_navigation: bool,
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
            // SPEC §4.3: Esc support is "when configured". Default off
            // so the safer behaviour (Esc ignored) is the one a careless
            // author gets for free; opting into cancellation is an
            // explicit, single-line builder call.
            cancellable: false,
            // SPEC §4.5: arrow/Enter navigation is an optional mode.
            // `None` means direct-key only; opt in via `.navigable(true)`
            // *after* the choices are added, since the seeding logic
            // needs to scan them for the first enabled row.
            selected: None,
            // SPEC §4.5 lists `j`/`k` as an *alternative* keymap, not a
            // default — pure-arrow players (and prompts whose authors
            // bound `j` or `k` as a choice hotkey) get the safer
            // behaviour for free; opt in via `.vim_navigation(true)`.
            vim_navigation: false,
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

    /// Configure whether Esc cancels this prompt (SPEC_v1_1.md §4.3).
    ///
    /// Pass `true` for prompts that are safe to dismiss (a loot prompt,
    /// a vendor menu, an informational pause); pass `false` for prompts
    /// that must be resolved (a save-overwrite confirmation, an opening
    /// menu where Esc would strand the player). The setter form (rather
    /// than a marker `.cancellable()` method) lets the builder express
    /// the dynamic case `cancellable(player.has_alt_exit())` without an
    /// `if/else` arm.
    pub fn cancellable(mut self, cancellable: bool) -> Self {
        self.cancellable = cancellable;
        self
    }

    /// Toggle the optional arrow/Enter navigation cursor
    /// (SPEC_v1_1.md §4.5).
    ///
    /// Pass `true` to seed [`ChoicePrompt::selected`] with the index of
    /// the first **enabled** choice — the cursor never starts on a row
    /// the player cannot pick, so a freshly opened prompt is always
    /// ready for an immediate Enter (Task 4c). When no choice is
    /// enabled (an unusual data state, but representable), the field is
    /// left as `None` rather than `Some(0)`: an unselectable cursor is
    /// worse than no cursor at all, and Tasks 4b/4c can short-circuit
    /// on `None` instead of guarding every move against the all-disabled
    /// case.
    ///
    /// Pass `false` to drop back to direct-key mode and clear the
    /// cursor. Round-tripping
    /// `.navigable(true).navigable(false).navigable(true)` is supported
    /// so dynamic chains like
    /// `.navigable(player.prefers_arrow_keys())` compile.
    ///
    /// # Call this *after* `.choice(...)` calls
    ///
    /// The seeding scan looks at the current `choices` list. Calling
    /// `.navigable(true)` on an empty prompt produces `selected = None`
    /// (correctly — there is nothing to highlight), and choices added
    /// later do not retroactively move the cursor onto themselves. The
    /// SPEC §5 builder-chain shape (`.body … .choice … .choice …
    /// .navigable(true)`) makes this ordering natural; the docs spell
    /// it out so a misordered chain reads as the author's bug, not the
    /// kit's.
    pub fn navigable(mut self, enabled: bool) -> Self {
        self.selected = if enabled {
            // Linear scan is fine here for the same reason it is fine
            // in `handle`: prompt choice lists are short (SPEC §4.4).
            // `position` returns `None` on an empty list or one with
            // no enabled rows, which is exactly the value we want for
            // the unselectable case.
            self.choices.iter().position(|c| c.enabled)
        } else {
            None
        };
        self
    }

    /// Toggle the optional Vim-style `j`/`k` navigation alt keymap
    /// (SPEC_v1_1.md §4.5).
    ///
    /// When `enabled` is `true`, [`ChoicePrompt::step_from_input`] maps
    /// `Input::Char('j')` to [`ChoicePrompt::move_down`] and
    /// `Input::Char('k')` to [`ChoicePrompt::move_up`], in addition to
    /// the always-on `Input::Up`/`Input::Down` arrows. When `false` (the
    /// default) those characters are not movement keys, leaving them
    /// free to be used as choice hotkeys.
    ///
    /// # Setter form, not a marker
    ///
    /// Mirrors [`ChoicePrompt::cancellable`] and
    /// [`ChoicePrompt::navigable`] in shape so dynamic chains like
    /// `.vim_navigation(player.prefers_vim_keys())` compile without an
    /// `if/else` arm.
    ///
    /// # Precedence over choice hotkeys
    ///
    /// `step_from_input` is the movement entry point; `handle` is the
    /// selection entry point. They are separate by design — authors
    /// route navigation inputs through `step_from_input` first, then
    /// fall through to `handle` for anything it did not consume. With
    /// `vim_navigation = true`, `j`/`k` are consumed by `step_from_input`
    /// and never reach `handle`. The kit does **not** validate against
    /// authors binding `j`/`k` as choice hotkeys while Vim mode is on:
    /// that combination is a config bug, not a representable runtime
    /// state, and the prompt-validation pipeline (Task 2c) intentionally
    /// reasons only about the `choices` list. If you ship Vim mode,
    /// reserve `j` and `k` for movement.
    pub fn vim_navigation(mut self, enabled: bool) -> Self {
        self.vim_navigation = enabled;
        self
    }

    /// Try to consume `input` as cursor movement and return `true` if
    /// it was. The caller's normal flow is "movement first, then
    /// selection":
    ///
    /// ```ignore
    /// if !prompt.step_from_input(input) {
    ///     match prompt.handle(input) { /* … */ }
    /// }
    /// ```
    ///
    /// # Inputs consumed
    ///
    /// - `Input::Up` → [`ChoicePrompt::move_up`]
    /// - `Input::Down` → [`ChoicePrompt::move_down`]
    /// - `Input::Char('j')` → [`ChoicePrompt::move_down`] *iff*
    ///   [`ChoicePrompt::vim_navigation`] is `true`
    /// - `Input::Char('k')` → [`ChoicePrompt::move_up`] *iff*
    ///   `vim_navigation` is `true`
    ///
    /// Anything else returns `false` without mutating the prompt.
    ///
    /// # Direct-key mode short-circuit
    ///
    /// When [`ChoicePrompt::selected`] is `None` (no cursor active) the
    /// method returns `false` without touching `move_up`/`move_down`,
    /// matching their existing no-op-in-direct-key-mode policy. The
    /// caller's `handle` arm can then dispatch the same input as a
    /// regular hotkey — Vim mode does not silently swallow `j`/`k` on a
    /// direct-key prompt.
    ///
    /// # Why a separate method, not folded into `handle`
    ///
    /// `handle` is `&self` by design (see its doc comment). Movement
    /// must mutate the cursor, so it cannot share that signature. A
    /// separate `&mut self` entry point keeps the two reducer surfaces
    /// honest about what they touch and lets games preview "would this
    /// key select something?" without paying the mutation cost.
    pub fn step_from_input(&mut self, input: Input) -> bool {
        // Direct-key prompts have no cursor; movement is undefined.
        // Returning early (rather than calling `move_up`/`move_down`
        // which would also no-op) makes the caller's "fall through to
        // handle" path obviously correct — pressing `j` on a direct-key
        // prompt with Vim mode on stays available as a hotkey.
        if self.selected.is_none() {
            return false;
        }
        match input {
            Input::Up => {
                self.move_up();
                true
            }
            Input::Down => {
                self.move_down();
                true
            }
            // Vim alt-keymap. `PromptKey::char` lowercases on storage,
            // so we match the lowercase form here and let the Input
            // layer's verbatim `Char` carry through; uppercase `J`/`K`
            // arrive as `Char('J')`/`Char('K')` and would only fire if
            // the player held Shift, which is conventionally a
            // different action — leaving them out matches Vim itself.
            Input::Char('j') if self.vim_navigation => {
                self.move_down();
                true
            }
            Input::Char('k') if self.vim_navigation => {
                self.move_up();
                true
            }
            _ => false,
        }
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

    /// Move the navigation cursor to the previous enabled choice
    /// (SPEC_v1_1.md §4.5, "Up/Down selection movement when arrow mode
    /// is enabled").
    ///
    /// # Policy
    ///
    /// - **Direct-key mode** (`selected == None`): no-op. Up/Down only
    ///   participate when navigation was opted into via
    ///   [`ChoicePrompt::navigable`]; a stray arrow press on a hotkey
    ///   prompt must not silently spawn a cursor.
    /// - **Disabled rows are skipped.** The cursor walks backwards from
    ///   the current index, ignoring `enabled == false` rows, until it
    ///   lands on the next enabled one. SPEC §4.1 keeps disabled rows
    ///   visible (and their hotkeys reserved) precisely because they
    ///   are non-pickable; the cursor honours that by refusing to
    ///   highlight them.
    /// - **Wrap, don't clamp.** Going Up from the topmost enabled choice
    ///   moves to the bottommost enabled choice. Door-game menus are
    ///   short (SPEC §4.4 ~5 typical) and wrapping matches what
    ///   players already expect from BBS-era door UIs; clamping would
    ///   require an extra Down press to reach the bottom of a
    ///   four-item menu, which is the exact "feels broken" failure
    ///   mode SPEC §4.5 calls out by listing the cursor at all.
    /// - **All-disabled / single-enabled** lists are stable: the cursor
    ///   stays where it is rather than spinning forever or being
    ///   nudged to a disabled row.
    ///
    /// Mirrors [`ChoicePrompt::move_down`] in shape so authors can pair
    /// them with their own keymap (`Up`/`k` → `move_up`, `Down`/`j` →
    /// `move_down`, see Task 4d).
    pub fn move_up(&mut self) {
        self.step_selection(Direction::Up);
    }

    /// Move the navigation cursor to the next enabled choice
    /// (SPEC_v1_1.md §4.5).
    ///
    /// Mirror of [`ChoicePrompt::move_up`]; see that doc-comment for the
    /// full skip-disabled / wrap-around / no-op policy. The two methods
    /// share an internal helper to guarantee they cannot drift.
    pub fn move_down(&mut self) {
        self.step_selection(Direction::Down);
    }

    /// Shared driver for [`ChoicePrompt::move_up`] and
    /// [`ChoicePrompt::move_down`]. Walks the `choices` ring starting
    /// after the current cursor position, skipping disabled rows, and
    /// returns at the first enabled row found. If the only enabled row
    /// is the one already selected (or none exist), the cursor stays
    /// put — both because the externally visible behaviour is
    /// "movement", and because spinning the loop for `len()` steps
    /// without a destination would just hand the player back the same
    /// index anyway.
    fn step_selection(&mut self, direction: Direction) {
        let Some(current) = self.selected else {
            // Direct-key mode: ignore arrow input at the prompt layer
            // entirely. The runtime will route the arrow event to
            // whatever screen wraps the prompt.
            return;
        };
        let len = self.choices.len();
        // `selected = Some(i)` carries the invariant `i < len`, so an
        // empty `choices` list cannot happen here. The check is cheap
        // insurance against a future builder method dropping a row
        // without resyncing `selected`.
        if len == 0 {
            return;
        }
        // Walk at most `len - 1` neighbour positions. We deliberately
        // skip distance 0 (the current row) so a single Up on a
        // one-enabled-choice prompt is a no-op rather than re-selecting
        // the same row. `len - 1` is the largest meaningful step: one
        // more would land back on `current`.
        for step in 1..len {
            let idx = match direction {
                Direction::Up => (current + len - step) % len,
                Direction::Down => (current + step) % len,
            };
            if self.choices[idx].enabled {
                self.selected = Some(idx);
                return;
            }
        }
        // No other enabled choice exists. Leave `selected` untouched so
        // the caller's render still has a valid cursor; Task 4c's Enter
        // handler will continue to fire on the current (still-enabled)
        // row.
    }

    /// Default prompt-line label rendered after the choice list in
    /// compact unboxed mode (SPEC_v1_1.md §4.3, §4.8). Returns the
    /// SPEC's example text `"Your choice:"`. Customisation hooks
    /// (`>`, `"Choice:"`, …) are reserved for the Task 5e/5f layout
    /// surface; carving the value out as its own method now means the
    /// rendering pipeline already reads from a single source of truth
    /// when that lands.
    fn prompt_label_text(&self) -> &str {
        "Your choice:"
    }

    /// Render the prompt as one [`String`] per visual row in the
    /// **compact unboxed** layout (SPEC_v1_1.md §4.8 — the bordered
    /// modal layout lands in Task 5e).
    ///
    /// Row order, top to bottom:
    ///
    /// 1. Wrapped body lines (using [`TextBlock`]'s wrap rules so the
    ///    same blank-line preservation applies).
    /// 2. A single blank row separating body and choices, but only when
    ///    both are present — empty bodies do not push a stray gap onto
    ///    a top-of-screen prompt.
    /// 3. Each choice as `"({KEY}) {label}"`, wrapped to `width`. The
    ///    marker is uppercased per SPEC §4.1's "canonical display keys
    ///    SHOULD store as uppercase when rendered". Disabled-row
    ///    formatting (the `- [M] … (full)` marker family) and the
    ///    selected-row marker layer in on top in Tasks 5c and 5d.
    /// 4. Optional footer, preceded by one blank row when anything has
    ///    already been rendered.
    /// 5. A blank row plus the prompt label (`"Your choice:"`) when the
    ///    prompt has at least one choice. SPEC §4.3's example
    ///    rendering ends with that label, and a choiceless prompt
    ///    (used as a pure narration block) skips it so the layout does
    ///    not invite a key the prompt cannot accept.
    ///
    /// Width `0` returns an empty `Vec` so callers with a degenerate
    /// area do not panic — SPEC §6 forbids that. The `render`
    /// companion calls back into this method, so width handling stays
    /// in one place.
    pub fn rendered_lines(&self, width: u16) -> Vec<String> {
        self.rendered_rows(width)
            .into_iter()
            .map(|(row, _)| row)
            .collect()
    }

    /// Compute the prompt's visual rows alongside a per-row "is this
    /// the selected choice?" flag, used by [`ChoicePrompt::render`] to
    /// reverse-style the cursor row in arrow/Enter navigation mode
    /// (Task 5d).
    ///
    /// The marker layer is character-based (`> ` prefix on the
    /// selected row, two-space indent on every other choice row) so
    /// SPEC §6's "MUST NOT require color support for comprehension"
    /// holds — a strictly monochrome BBS terminal still shows the
    /// cursor. The reverse-video style applied in `render` is layered
    /// on top for color-capable terminals where the marker alone would
    /// feel subtle.
    ///
    /// Direct-key prompts (no `.navigable(true)`) keep `selected =
    /// None` and render exactly as before — no prefix, no reversed
    /// row — so the SPEC §4.3 reference rendering and every Task 5b/5c
    /// test continue to match byte-for-byte.
    ///
    /// When the available width is narrower than the 2-cell prefix
    /// (`width < 3`), the indent is dropped and the choice rows
    /// render unprefixed; the cursor disappears but the prompt is at
    /// least readable. SPEC §6 forbids panicking on small areas; this
    /// fallback honors that for the same reason `width == 0` returns
    /// an empty `Vec`.
    fn rendered_rows(&self, width: u16) -> Vec<(String, bool)> {
        if width == 0 {
            return Vec::new();
        }
        let mut rows: Vec<(String, bool)> = Vec::new();

        // 1. Body — re-use `TextBlock`'s wrap so blank-line
        //    preservation, hard-break-on-overlong-word, and the
        //    deterministic-under-`TestBackend` contract carry through
        //    without a second implementation drifting from the first.
        let body = TextBlock::from_lines(self.body.iter().cloned());
        rows.extend(body.wrapped_rows(width).into_iter().map(|r| (r, false)));

        // 2. Body→choices gap.
        if !rows.is_empty() && !self.choices.is_empty() {
            rows.push((String::new(), false));
        }

        // 3. Choices. In navigation mode the choice rows reserve a
        //    2-cell prefix on the left for the selected-row marker
        //    (`> `) so the cursor never overflows the slot — wrap at
        //    `width - 2` and prepend the prefix post-wrap, because
        //    `wrap_line_into` collapses leading whitespace via
        //    `split_whitespace`.
        let nav_mode = self.selected.is_some();
        let indent_choices = nav_mode && width >= 3;
        let choice_wrap_width = if indent_choices { width - 2 } else { width };
        for (i, choice) in self.choices.iter().enumerate() {
            let line = format_choice_row(choice);
            let wrapped = TextBlock::new(&line).wrapped_rows(choice_wrap_width);
            let is_selected = self.selected == Some(i);
            for (row_idx, row) in wrapped.into_iter().enumerate() {
                let prefixed = if indent_choices {
                    // Marker on the first wrapped row only — continuation
                    // rows keep the indent so the label column stays
                    // aligned but do not stack `> ` markers.
                    let prefix = if is_selected && row_idx == 0 {
                        "> "
                    } else {
                        "  "
                    };
                    format!("{prefix}{row}")
                } else {
                    row
                };
                rows.push((prefixed, is_selected));
            }
        }

        // 4. Footer.
        if let Some(footer) = self.footer.as_deref() {
            if !rows.is_empty() {
                rows.push((String::new(), false));
            }
            rows.extend(
                TextBlock::new(footer)
                    .wrapped_rows(width)
                    .into_iter()
                    .map(|r| (r, false)),
            );
        }

        // 5. Prompt label.
        if !self.choices.is_empty() {
            rows.push((String::new(), false));
            rows.extend(
                TextBlock::new(self.prompt_label_text())
                    .wrapped_rows(width)
                    .into_iter()
                    .map(|r| (r, false)),
            );
        }

        rows
    }

    /// Render the prompt into `area` of `buf`, top-down, in the
    /// compact unboxed layout. Returns the number of visual rows
    /// actually written (clamped to `area.height`).
    ///
    /// Lines past the available height are silently clipped — SPEC §6
    /// forbids panicking on small areas, and overflow handling for the
    /// bordered modal layout lives in Task 5e.
    ///
    /// In navigation mode (`.navigable(true)`) the row matching
    /// `selected` is drawn with `Modifier::REVERSED`. The character
    /// `> ` prefix added by the row-building helper is the
    /// monochrome-safe carrier of the same information; the reversed
    /// style is the color-capable layer on top. The full theme/style
    /// system lands in Task 5f — today the cursor row is the only
    /// styled cell; everything else writes with the buffer's default
    /// style so `TestBackend` assertions stay deterministic.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if area.width == 0 || area.height == 0 {
            return 0;
        }
        let rows = self.rendered_rows(area.width);
        let drawn = rows.len().min(area.height as usize);
        let selected_style =
            ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::REVERSED);
        let default_style = ratatui::style::Style::default();
        for (i, (row, is_selected)) in rows.iter().take(drawn).enumerate() {
            let y = area.y + i as u16;
            let style = if *is_selected {
                selected_style
            } else {
                default_style
            };
            buf.set_stringn(area.x, y, row, area.width as usize, style);
        }
        drawn as u16
    }
}

/// Format a single choice row in the SPEC §4.3 / §4.2 styles.
///
/// Two visual shapes share this helper because every choice row carries
/// the same label/hint pipeline; only the leader and bracket glyphs
/// change with the choice's `enabled` flag:
///
/// - **Enabled** (SPEC §4.3): `(K) Label`. Hotkey in parentheses,
///   uppercased per SPEC §4.1's display rule.
/// - **Disabled** (SPEC §4.2): `- [K] Label (reason)`. Leading `- `
///   plus square brackets are the *monochrome* visual difference the
///   SPEC requires — the row stays distinguishable from enabled rows
///   even when the renderer is theme-stripped (Task 5f) or running on
///   a strictly monochrome BBS terminal where the eventual
///   `StyleRole::Disabled` dim style is invisible. The trailing
///   `(reason)` annotation comes straight from
///   `PromptChoice::disabled_reason` so the player sees *why* the row
///   is unavailable without the game having to render a separate
///   feedback line just to explain it.
///
/// The exotic `Enter`/`Esc` choice keys — legal at the data level (see
/// `validate_choices_treats_enter_and_esc_…`) but uncommon — render as
/// their human-readable name so the marker still communicates the
/// binding under either bracket style.
///
/// Hint text is appended after a two-space gutter so dynamic
/// annotations like `"50g"` or `"0/26"` (SPEC §5 vendor example) sit
/// alongside the label without a separate column, regardless of
/// enabled state — disabled rows still benefit from the gold/capacity
/// hint sitting between the label and the trailing `(reason)`.
fn format_choice_row<T>(choice: &PromptChoice<T>) -> String {
    // Lead with the SPEC §4.2 disabled marker only when the row is
    // actually disabled — keeping the enabled path's first character a
    // `(` so the existing SPEC §4.3 reference rendering still passes
    // its byte-for-byte test (no leading whitespace surprises an
    // operator scanning a screenshot).
    let mut row = if choice.enabled {
        format!("({}) {}", marker_glyph(&choice.key), choice.label)
    } else {
        format!("- [{}] {}", marker_glyph(&choice.key), choice.label)
    };
    if let Some(hint) = choice.hint.as_deref() {
        row.push_str("  ");
        row.push_str(hint);
    }
    if !choice.enabled {
        if let Some(reason) = choice.disabled_reason.as_deref() {
            // Single space before the parenthesised reason matches
            // SPEC §4.2's reference example `- [M] Mana potions ...
            // (full)` exactly; using two spaces here would visibly
            // drift the reason out of column alignment with the SPEC
            // and break authors who paste the example into a manifest.
            row.push_str(" (");
            row.push_str(reason);
            row.push(')');
        }
    }
    row
}

/// Pick the display glyph for a [`PromptKey`] inside the `(K)` marker.
/// Char keys uppercase the stored lowercase form (SPEC §4.1's display
/// rule); `Enter`/`Esc` render as their name so the marker is still
/// legible if a prompt binds them. The free-function form keeps the
/// formatter testable without an instance.
fn marker_glyph(key: &PromptKey) -> String {
    match key {
        PromptKey::Char(c) => c.to_ascii_uppercase().to_string(),
        PromptKey::Enter => "Enter".to_string(),
        PromptKey::Esc => "Esc".to_string(),
    }
}

/// Internal direction marker for [`ChoicePrompt::step_selection`].
/// Kept private — the public surface is the two `move_up`/`move_down`
/// methods, so callers never have to construct a direction value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
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
    /// cursor lives in [`ChoicePrompt::selected`] (Task 4a); the Up/Down
    /// movement and Enter-confirm reducers that need to mutate it land
    /// as separate `&mut self` methods in Tasks 4b–4c, so this `&self`
    /// method stays cheap to call (e.g. from a render path that previews
    /// "would this key select something?").
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

        // Task 3d: Esc cancellation, opt-in via `.cancellable(true)`.
        // Checked before the direct-hotkey scan so a choice that
        // explicitly bound `PromptKey::Esc` (exotic but legal at the
        // data level — see `validate_choices_treats_enter_and_esc_...`)
        // does not accidentally short-circuit a configured cancel.
        // When cancellation is off, Esc falls through to the trailing
        // `None` so the prompt stays unchanged.
        if key == PromptKey::Esc && self.cancellable {
            return PromptAction::Cancelled;
        }

        // Task 4c: Enter confirms the highlighted choice in navigation
        // mode. `selected` is only ever `Some` when `.navigable(true)`
        // has seeded a cursor (SPEC §4.5), so the `if let` here doubles
        // as the "is this a navigation-mode prompt?" gate — direct-key
        // prompts keep the legacy "Enter is a no-op" behaviour because
        // their `selected` is `None`. Tasks 4a/4b maintain the invariant
        // that the cursor sits on an enabled row, so the common path
        // emits `Selected`; we still defensively surface `Disabled` for
        // the row should that invariant ever be broken (e.g. a future
        // dynamic-`enabled` toggle), instead of returning a phantom
        // `Selected` for a row the player cannot pick.
        if key == PromptKey::Enter {
            if let Some(idx) = self.selected {
                if let Some(choice) = self.choices.get(idx) {
                    if choice.enabled {
                        return PromptAction::Selected(choice.value.clone());
                    }
                    return PromptAction::Disabled {
                        id: choice.value.clone(),
                        reason: choice.disabled_reason.clone(),
                    };
                }
            }
        }

        // Direct hotkey path (Task 3b). Only `Char` keys participate in
        // direct-key matching here; `Enter` and `Esc` were already
        // dispatched above (Task 3d cancellation, Task 4c navigation
        // confirm) or fell through as no-ops. Choices declared with
        // `PromptKey::Enter`/`PromptKey::Esc` are exotic data-level
        // configurations that those earlier branches address before
        // we ever reach the hotkey scan.
        if let PromptKey::Char(_) = key {
            // Linear scan — choice lists are short (SPEC §4.4 ~5
            // typical) and `validate()` already enforces uniqueness, so
            // the first match is unambiguous. Keys are pre-normalised
            // to lowercase on both sides (`PromptKey::char` on storage,
            // `PromptKey::from_input` on input), so equality is the
            // entire comparison — no per-call `to_ascii_lowercase`.
            //
            // Task 3c: a disabled choice still owns its hotkey
            // (SPEC §4.1: "disabled rows still reserve their hotkey")
            // so we surface the press as `PromptAction::Disabled`
            // rather than letting it fall through to `None`. That gives
            // the game a chance to render the disabled_reason as
            // feedback ("can't equip — bag full") instead of swallowing
            // the press silently, which would leave the player guessing
            // whether the prompt is broken or simply ignoring them.
            for choice in &self.choices {
                if choice.key == key {
                    if choice.enabled {
                        return PromptAction::Selected(choice.value.clone());
                    }
                    return PromptAction::Disabled {
                        id: choice.value.clone(),
                        reason: choice.disabled_reason.clone(),
                    };
                }
            }
        }

        // Anything that did not match a registered hotkey, an opt-in
        // Esc cancel, or an Enter confirm in navigation mode is a no-op
        // so the prompt never invents a selection from a press the
        // player did not make.
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

/// A reusable narration / status fragment rendered above a prompt
/// (SPEC_v1_1.md §4.7).
///
/// `TextBlock` is the data shape behind a prompt body: a sequence of
/// logical lines that the renderer wraps to the supplied area's width
/// while preserving the author's explicit blank lines as visual gaps.
/// It carries an optional [`StyleRole`] default so a status block can
/// declare itself as `Success` / `Error` / `Muted` once instead of
/// per-line; the role-to-`Style` mapping itself lives in Task 5f and is
/// intentionally absent here so this task stays focused on layout.
///
/// # Wrapping rules
///
/// - Logical lines are the entries in [`TextBlock::lines`], which
///   [`TextBlock::new`] derives by splitting the input on `\n`.
/// - Each non-empty logical line wraps to the area width on whitespace
///   boundaries (greedy fill). A single word longer than the width is
///   hard-broken at the width — terminal output never silently drops
///   characters.
/// - An **empty logical line stays empty**: it produces exactly one
///   blank visual row, regardless of width. This is what SPEC §4.7's
///   "preserve explicit blank lines" requires, and it matches how
///   author-written narration uses `\n\n` as a paragraph break.
/// - A width of `0` collapses to a no-op render (no panic). This keeps
///   the SPEC §6 "MUST NOT panic on small areas" guarantee true even
///   when the layout caller hands the body a zero-width slot.
///
/// # What this type intentionally does NOT do
///
/// - It does not carry per-span styling. Inline emphasis is the job of
///   [`StyleRole`] applied at the choice/feedback level (Tasks 5c–5f),
///   not of in-line markup inside a body string.
/// - It does not own scroll state. SPEC §4.8 lists "optional scrolling
///   for long bodies" as future scope; the v1.1 prompt rendering MUST
///   support compact unboxed and bordered modal modes (Tasks 5b, 5e),
///   both of which clip rather than scroll.
/// - It does not perform side effects. Rendering only writes cells to
///   a `Buffer`; the reducer / game state is untouched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextBlock {
    /// Logical lines, in order. An empty `String` means "render a
    /// blank visual row" — see the wrapping rules in the type doc.
    lines: Vec<String>,
    /// Optional default style role for the whole block. The Task 5f
    /// theme layer maps this to a `ratatui::style::Style`; until then
    /// the renderer ignores it (no styling is still SPEC-legal).
    style: Option<StyleRole>,
}

impl TextBlock {
    /// Build a [`TextBlock`] from any string-ish input by splitting on
    /// `\n`.
    ///
    /// `\r\n` is normalised away by stripping a trailing `\r` from each
    /// line — Windows-authored content stays readable without leaking
    /// stray carriage returns into the buffer. The split is on the
    /// **logical** line break, so a leading or trailing `\n` produces
    /// the empty line the author intended.
    pub fn new(text: impl AsRef<str>) -> Self {
        let text = text.as_ref();
        // `split('\n')` (rather than `lines()`) preserves a trailing
        // empty line — `"a\n"` becomes `["a", ""]`. SPEC §4.7's
        // blank-line contract relies on that round-trip behaviour.
        let lines = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        Self { lines, style: None }
    }

    /// Build a [`TextBlock`] from an explicit list of logical lines.
    ///
    /// Useful when an author has already split their content (e.g. one
    /// `String` per generated line of a status block) and does not want
    /// to re-join with `\n` only to have [`TextBlock::new`] split it
    /// again.
    pub fn from_lines<I, S>(lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            lines: lines.into_iter().map(Into::into).collect(),
            style: None,
        }
    }

    /// Attach a default semantic [`StyleRole`] to the block.
    ///
    /// The theme layer in Task 5f turns this into a Ratatui `Style`;
    /// today it is a recorded intent the renderer reads when that task
    /// lands. Storing the role on the data (not on the renderer) keeps
    /// theming overridable without re-walking the prompt tree.
    pub fn with_style(mut self, role: StyleRole) -> Self {
        self.style = Some(role);
        self
    }

    /// Logical lines (post-split, pre-wrap). Mostly useful in tests and
    /// for layout code that needs to know how many paragraphs the body
    /// carries before it commits to a `Rect`.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Default [`StyleRole`] declared via [`TextBlock::with_style`], if
    /// any.
    pub fn style(&self) -> Option<StyleRole> {
        self.style
    }

    /// Wrap the block's logical lines to the given visual width.
    ///
    /// Returns one [`String`] per visual row, in render order. An empty
    /// logical line yields exactly one empty row; a non-empty line is
    /// greedily filled at whitespace boundaries, and a word longer than
    /// `width` is hard-broken so output never silently drops content.
    ///
    /// Width `0` returns an empty `Vec` (no panic, no work) — that is
    /// the contract the bordered modal renderer in Task 5e relies on
    /// when the caller area is too small to host a body column.
    pub fn wrapped_rows(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return Vec::new();
        }
        let width = width as usize;
        let mut out: Vec<String> = Vec::with_capacity(self.lines.len());
        for line in &self.lines {
            if line.is_empty() {
                // SPEC §4.7: blank lines are preserved as one row.
                out.push(String::new());
                continue;
            }
            wrap_line_into(line, width, &mut out);
        }
        out
    }

    /// Render the block into `area` of `buf`, top-down. Returns the
    /// number of visual rows actually written (clamped to
    /// `area.height`).
    ///
    /// Lines past the available height are silently clipped; SPEC §6
    /// forbids panicking on small areas, and a body that overflows its
    /// slot is a layout decision for the caller (compact unboxed and
    /// bordered modes both clip — see Task 5e). The unused `style`
    /// field is reserved for the Task 5f theme integration; rendering
    /// today writes cells with the buffer's default style so test
    /// assertions stay deterministic without the theme present.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if area.width == 0 || area.height == 0 {
            return 0;
        }
        let rows = self.wrapped_rows(area.width);
        let drawn = rows.len().min(area.height as usize);
        for (i, row) in rows.iter().take(drawn).enumerate() {
            // `set_stringn` truncates if the wrapper produced a row
            // wider than `area.width` (defence in depth — the wrapper
            // already enforces this, but a hard-break on a multibyte
            // boundary would still be safe through `set_stringn`).
            let y = area.y + i as u16;
            buf.set_stringn(
                area.x,
                y,
                row,
                area.width as usize,
                ratatui::style::Style::default(),
            );
        }
        drawn as u16
    }
}

/// Greedy whitespace-boundary wrap for one non-empty logical line.
///
/// Pushed-in style avoids a transient `Vec` per line; the caller already
/// owns the output vector and we just append visual rows. Words longer
/// than `width` are hard-broken at the width boundary so the renderer
/// never has to decide between dropping characters and overflowing the
/// area — SPEC §6's "truncation or wrapping at terminal width"
/// requirement is satisfied by always wrapping.
fn wrap_line_into(line: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width: usize = 0;

    // Token stream: alternating runs of whitespace and non-whitespace.
    // We greedily fit non-whitespace runs ("words") onto the current
    // visual row, separated by a single ' ' when both fit. Multiple
    // spaces between words collapse to one because terminal rendering
    // does not preserve internal whitespace runs visually anyway, and
    // the author intent is captured by explicit blank lines.
    for word in line.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            // Word doesn't fit even on its own row — flush whatever we
            // have and hard-break the word at width chunks. This keeps
            // SPEC §6's "no silent drop" promise; URLs and long IDs
            // remain visible.
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut buf = String::new();
            let mut buf_width = 0;
            for ch in word.chars() {
                if buf_width == width {
                    out.push(std::mem::take(&mut buf));
                    buf_width = 0;
                }
                buf.push(ch);
                buf_width += 1;
            }
            current = buf;
            current_width = buf_width;
            continue;
        }
        let needs_space = !current.is_empty();
        let projected = current_width + if needs_space { 1 } else { 0 } + word_len;
        if projected > width {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_len;
        } else {
            if needs_space {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_len;
        }
    }
    // Flush any partial row. If the input line was whitespace-only
    // (`split_whitespace` produced zero words), push a single blank
    // row so the author's vertical spacing still shows up — that is
    // what SPEC §4.7's "preserve explicit blank lines" calls for, and
    // it keeps `wrapped_rows` returning at least one row per logical
    // line.
    out.push(current);
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
    fn handle_non_prompt_inputs_do_not_mutate_selection() {
        // Task 3e: SPEC_v1_1.md §4.4 requires resize and unknown inputs
        // be inert — no selection invented, and crucially no latent
        // state change that would corrupt a follow-up press. The
        // reducer's `&self` signature already proves immutability at
        // the type level, but this test pins the *behavioural*
        // contract: after an arbitrary stream of ignored inputs, the
        // very next bound hotkey still resolves to the right enabled
        // choice. When Task 4a adds a `selected_index` cursor, this
        // test will catch any accidental mutation of it from the
        // ignored-input path.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        for input in [
            Input::Resize {
                width: 80,
                height: 24,
            },
            Input::Resize {
                width: 1,
                height: 1,
            },
            Input::Up,
            Input::Down,
            Input::Backspace,
            Input::Ctrl('c'),
            Input::Unknown,
        ] {
            // Burn the input — must be a no-op.
            assert_eq!(prompt.handle(input), PromptAction::None);
        }

        // Selection still resolves correctly afterwards: nothing about
        // the prompt's internal state was disturbed by the ignored
        // stream above.
        assert_eq!(
            prompt.handle(Input::Char('e')),
            PromptAction::Selected(LootAction::Equip),
        );
        assert_eq!(
            prompt.handle(Input::Char('t')),
            PromptAction::Selected(LootAction::Take),
        );
    }

    #[test]
    fn handle_disabled_choice_returns_disabled_with_id_and_reason() {
        // Task 3c: pressing a disabled choice's hotkey MUST NOT silently
        // succeed (would fire the action) and MUST NOT silently drop to
        // `None` (would leave the player wondering if the keypress
        // registered). Instead the reducer surfaces a `Disabled` action
        // carrying the choice's id and its `disabled_reason`, so the
        // game can render feedback like "bag full" without re-deriving
        // why the row was disabled in the first place.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Take, "Mana potions")
            .disabled_if(true, "full")
            .choice('n', LootAction::Pass, "No thanks");

        // Lowercase and uppercase both route through the same disabled
        // path — case-insensitivity is a `PromptKey` invariant, not a
        // per-arm concern, but locking it in here guards against a
        // future refactor that accidentally adds a case branch.
        assert_eq!(
            prompt.handle(Input::Char('m')),
            PromptAction::Disabled {
                id: LootAction::Take,
                reason: Some("full".to_string()),
            },
        );
        assert_eq!(
            prompt.handle(Input::Char('M')),
            PromptAction::Disabled {
                id: LootAction::Take,
                reason: Some("full".to_string()),
            },
        );
        // Enabled neighbour still selects normally — the disabled row
        // does not poison the rest of the prompt.
        assert_eq!(
            prompt.handle(Input::Char('n')),
            PromptAction::Selected(LootAction::Pass),
        );
    }

    #[test]
    fn handle_disabled_choice_without_reason_returns_disabled_with_none() {
        // SPEC §4.2 makes `disabled_reason` optional. A choice flagged
        // disabled without a reason (e.g. via direct field assignment
        // or a future `with_disabled` helper) must still produce
        // `Disabled` — `reason: None` — rather than falling through to
        // `None`. The id is what the game keys feedback off; the reason
        // is a humane bonus, not a prerequisite for the variant.
        let mut choice = PromptChoice::new('k', "Take key", LootAction::Take);
        choice.enabled = false;
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt {
            body: Vec::new(),
            choices: vec![choice],
            footer: None,
            cancellable: false,
            selected: None,
            vim_navigation: false,
        };

        assert_eq!(
            prompt.handle(Input::Char('k')),
            PromptAction::Disabled {
                id: LootAction::Take,
                reason: None,
            },
        );
    }

    #[test]
    fn handle_enter_is_a_no_op_on_direct_key_prompts() {
        // Enter is the navigation-mode confirm key (Task 4c). On a
        // direct-key prompt (no `.navigable(true)`) the cursor is `None`,
        // so Enter must not invent a selection — otherwise SPEC §5's
        // hotkey-only loot prompts would silently route the first row
        // through `Selected` whenever the player tapped Return.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");

        assert_eq!(prompt.selected, None);
        assert_eq!(prompt.handle(Input::Enter), PromptAction::None);
    }

    #[test]
    fn handle_esc_returns_none_when_cancellation_disabled() {
        // SPEC_v1_1.md §4.3: Esc support is "when configured". A prompt
        // that has not opted into cancellation MUST treat Esc as a
        // no-op — otherwise a save-overwrite confirmation could be
        // dismissed by a stray keystroke.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");

        assert!(!prompt.cancellable);
        assert_eq!(prompt.handle(Input::Esc), PromptAction::None);
    }

    #[test]
    fn handle_esc_returns_cancelled_when_cancellation_enabled() {
        // Task 3d: opt-in via `.cancellable(true)`. Esc now produces
        // `Cancelled`; non-Esc inputs are unaffected — direct-key
        // selection still wins for a bound hotkey, and unbound printable
        // keys still collapse to `None`.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .cancellable(true);

        assert!(prompt.cancellable);
        assert_eq!(prompt.handle(Input::Esc), PromptAction::Cancelled);
        assert_eq!(
            prompt.handle(Input::Char('e')),
            PromptAction::Selected(LootAction::Equip),
        );
        assert_eq!(prompt.handle(Input::Char('z')), PromptAction::None);
    }

    #[test]
    fn cancellable_setter_round_trips_both_ways() {
        // The setter takes a bool so dynamic chains like
        // `.cancellable(player.can_back_out())` compile. Both arms must
        // round-trip — an `if/else` that flipped the default would defeat
        // the setter form.
        let on: ChoicePrompt<LootAction> = ChoicePrompt::new().cancellable(true);
        let off: ChoicePrompt<LootAction> =
            ChoicePrompt::new().cancellable(true).cancellable(false);
        assert!(on.cancellable);
        assert!(!off.cancellable);
    }

    #[test]
    fn default_prompt_is_not_cancellable() {
        // Default-off is load-bearing: it is the SPEC §4.3 contract that
        // Esc support is opt-in. If a future refactor flips the default
        // to `true`, every existing prompt becomes silently dismissible.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new();
        assert!(!prompt.cancellable);
    }

    #[test]
    fn navigable_defaults_to_none_for_direct_key_prompts() {
        // SPEC §4.5: arrow/Enter is opt-in. The default `new()` flow —
        // every example in SPEC §5 so far — must leave `selected = None`
        // so direct-key prompts never accidentally render a cursor.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn navigable_seeds_cursor_on_first_enabled_choice() {
        // Task 4a contract: enabling arrow mode highlights the first
        // enabled row so a fresh prompt is immediately Enter-ready.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn navigable_skips_leading_disabled_choices_when_seeding() {
        // The cursor must never start on a row the player cannot pick,
        // even when the data lists a disabled choice first. Without this
        // skip, an Enter press on a freshly opened arrow-mode prompt
        // would route through `Disabled` (or worse, through Task 4c's
        // future Enter handling), surprising the player.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .disabled_if(true, "bag full")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(1));
    }

    #[test]
    fn navigable_on_empty_prompt_leaves_selection_none() {
        // Edge case: a partial builder chain that flips navigation on
        // before any `.choice(...)` call must not produce `Some(0)` —
        // there is no row at index 0 to highlight, and a stale cursor
        // would be a panic vector for Task 4b's Up/Down reducer.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new().navigable(true);
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn navigable_with_all_disabled_choices_leaves_selection_none() {
        // No enabled row → no valid cursor position. Returning `None`
        // here (instead of `Some(0)`) lets later movement/Enter logic
        // treat "nothing selectable" as a single short-circuit case.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .disabled_if(true, "bag full")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "bag full");
        let prompt = prompt.navigable(true);
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn navigable_false_clears_cursor_for_round_trip() {
        // Dynamic chains like `.navigable(player.prefers_arrows())` must
        // round-trip both ways. Toggling off after on must clear the
        // cursor, not freeze it on the previously-seeded index.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .navigable(true)
            .navigable(false);
        assert_eq!(prompt.selected, None);

        // And re-enabling re-seeds on the first enabled choice.
        let prompt = prompt.navigable(true);
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn navigable_preserves_direct_key_handle() {
        // Direct-key dispatch must keep working in navigation mode so a
        // player can still tap a hotkey instead of arrow-stepping —
        // SPEC §4.5 frames arrow/Enter as an *additional* affordance,
        // not a replacement. Enter on the seeded cursor (index 0 =
        // Equip) routes through Task 4c's confirm path.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(
            prompt.handle(Input::Char('t')),
            PromptAction::Selected(LootAction::Take),
        );
        assert_eq!(
            prompt.handle(Input::Enter),
            PromptAction::Selected(LootAction::Equip),
        );
    }

    #[test]
    fn handle_enter_selects_highlighted_choice_in_navigation_mode() {
        // Task 4c: Enter on a navigation-mode prompt confirms the row
        // the cursor currently sits on. Stepping Down once moves the
        // cursor from the seeded index 0 (`Equip`) to index 1 (`Take`),
        // and Enter must surface that as `Selected(Take)`.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true);
        prompt.move_down();
        assert_eq!(prompt.selected, Some(1));
        assert_eq!(
            prompt.handle(Input::Enter),
            PromptAction::Selected(LootAction::Take),
        );
    }

    #[test]
    fn handle_enter_skips_disabled_seed_via_cursor_invariant() {
        // The cursor seed (Task 4a) and Up/Down (Task 4b) guarantee the
        // cursor lands on an enabled row even when index 0 is disabled.
        // Enter must therefore confirm the *enabled* row, not return
        // `Disabled` from a stale-looking index 0.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .disabled_if(true, "bag full")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(1));
        assert_eq!(
            prompt.handle(Input::Enter),
            PromptAction::Selected(LootAction::Take),
        );
    }

    #[test]
    fn handle_enter_is_no_op_when_no_enabled_choices_exist() {
        // If every choice is disabled, `.navigable(true)` leaves the
        // cursor at `None` (pinned by `navigable_with_all_disabled_...`),
        // so Enter has nothing to confirm and must collapse to `None`
        // rather than panic on an out-of-bounds lookup.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .disabled_if(true, "bag full")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "no slot")
            .navigable(true);
        assert_eq!(prompt.selected, None);
        assert_eq!(prompt.handle(Input::Enter), PromptAction::None);
    }

    #[test]
    fn move_down_advances_to_next_enabled_choice() {
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
        prompt.move_down();
        assert_eq!(prompt.selected, Some(1));
    }

    #[test]
    fn move_up_walks_back_to_previous_enabled_choice() {
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true);
        prompt.move_down();
        prompt.move_down();
        assert_eq!(prompt.selected, Some(2));
        prompt.move_up();
        assert_eq!(prompt.selected, Some(1));
    }

    #[test]
    fn move_down_skips_disabled_rows() {
        // Middle choice is disabled — Down from the top must land on
        // index 2, not on the disabled index 1.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "no slot")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
        prompt.move_down();
        assert_eq!(prompt.selected, Some(2));
    }

    #[test]
    fn move_up_skips_disabled_rows() {
        // Same shape; Up from the bottom must skip the disabled middle
        // row and land on the top.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "no slot")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true);
        // Seed the cursor on the bottom row by stepping past the
        // disabled middle.
        prompt.move_down();
        assert_eq!(prompt.selected, Some(2));
        prompt.move_up();
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn move_down_wraps_from_last_enabled_to_first_enabled() {
        // SPEC §4.5 leaves wrap-vs-clamp to the implementation; this
        // pins the kit's chosen wrap policy so a future regression to
        // clamping fails loudly.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        prompt.move_down();
        assert_eq!(prompt.selected, Some(1));
        prompt.move_down();
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn move_up_wraps_from_first_enabled_to_last_enabled() {
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
        prompt.move_up();
        assert_eq!(prompt.selected, Some(2));
    }

    #[test]
    fn wrap_skips_disabled_trailing_row() {
        // Last choice is disabled — Down from index 1 must wrap past
        // the disabled tail row and land on index 0, not on index 2.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass")
            .disabled_if(true, "out of stock")
            .navigable(true);
        prompt.move_down();
        assert_eq!(prompt.selected, Some(1));
        prompt.move_down();
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn move_is_noop_when_only_one_enabled_choice() {
        // One enabled, several disabled — cursor stays put on every
        // direction press (no spinning, no landing on a disabled row).
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "no slot")
            .choice('p', LootAction::Pass, "Pass")
            .disabled_if(true, "vendor closed")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
        prompt.move_down();
        assert_eq!(prompt.selected, Some(0));
        prompt.move_up();
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn move_is_noop_in_direct_key_mode() {
        // Without `.navigable(true)` the cursor is `None` and arrow
        // movement at the prompt layer must not invent one.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");
        assert_eq!(prompt.selected, None);
        prompt.move_down();
        prompt.move_up();
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn vim_navigation_defaults_off() {
        // SPEC §4.5 lists `j`/`k` as opt-in. A freshly built prompt must
        // leave the alt keymap disabled so authors who bound `j` or `k`
        // as a choice hotkey are not surprised.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new();
        assert!(!prompt.vim_navigation);
    }

    #[test]
    fn vim_navigation_setter_round_trips_both_ways() {
        // Mirrors the cancellable/navigable round-trip contract so
        // dynamic chains like `.vim_navigation(prefs.vim)` compile.
        let on: ChoicePrompt<LootAction> = ChoicePrompt::new().vim_navigation(true);
        let off: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .vim_navigation(true)
            .vim_navigation(false);
        assert!(on.vim_navigation);
        assert!(!off.vim_navigation);
    }

    #[test]
    fn step_from_input_arrows_drive_cursor_without_vim_flag() {
        // Up/Down are the always-on movement keys — the Vim flag only
        // adds `j`/`k`, it never gates the arrows.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
        assert!(prompt.step_from_input(Input::Down));
        assert_eq!(prompt.selected, Some(1));
        assert!(prompt.step_from_input(Input::Up));
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn step_from_input_ignores_j_and_k_when_vim_disabled() {
        // Default config: `j`/`k` are not movement keys. The cursor
        // must stay on row 0 and `step_from_input` must report the
        // input as unconsumed so the caller can route it to `handle`.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert!(!prompt.vim_navigation);
        assert!(!prompt.step_from_input(Input::Char('j')));
        assert_eq!(prompt.selected, Some(0));
        assert!(!prompt.step_from_input(Input::Char('k')));
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn step_from_input_maps_j_and_k_when_vim_enabled() {
        // SPEC §4.5 alt keymap: `j` → Down, `k` → Up. Both presses must
        // mirror the equivalent arrow press exactly.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .choice('p', LootAction::Pass, "Pass")
            .navigable(true)
            .vim_navigation(true);
        assert_eq!(prompt.selected, Some(0));
        assert!(prompt.step_from_input(Input::Char('j')));
        assert_eq!(prompt.selected, Some(1));
        assert!(prompt.step_from_input(Input::Char('j')));
        assert_eq!(prompt.selected, Some(2));
        assert!(prompt.step_from_input(Input::Char('k')));
        assert_eq!(prompt.selected, Some(1));
    }

    #[test]
    fn step_from_input_returns_false_in_direct_key_mode() {
        // Without `.navigable(true)` the cursor is `None`. Even with
        // `vim_navigation` on, `j`/`k` must not invent a cursor; the
        // caller is expected to dispatch them through `handle` as
        // potential direct hotkeys.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .vim_navigation(true);
        assert_eq!(prompt.selected, None);
        assert!(!prompt.step_from_input(Input::Up));
        assert!(!prompt.step_from_input(Input::Down));
        assert!(!prompt.step_from_input(Input::Char('j')));
        assert!(!prompt.step_from_input(Input::Char('k')));
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn step_from_input_does_not_consume_unrelated_keys() {
        // Anything other than the configured movement keys must report
        // unconsumed so the caller's `handle` arm can pick it up. Resize
        // is the SPEC §7 canonical "ignored" input; Enter is the
        // navigation-mode confirm key handled by `handle`, not here.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true)
            .vim_navigation(true);
        for input in [
            Input::Enter,
            Input::Esc,
            Input::Char('e'),
            Input::Char('J'), // uppercase: Vim itself does not move on Shift+j
            Input::Char('K'),
            Input::Resize {
                width: 80,
                height: 24,
            },
            Input::Unknown,
        ] {
            assert!(
                !prompt.step_from_input(input),
                "step_from_input should not consume {input:?}",
            );
            assert_eq!(prompt.selected, Some(0));
        }
    }

    // -- Task 5a: TextBlock wrapping & blank-line preservation -------

    fn render_textblock_to_strings(block: &TextBlock, w: u16, h: u16) -> Vec<String> {
        // Drive the renderer through a real Ratatui `TestBackend` so we
        // exercise the SPEC §6 "deterministic output under TestBackend"
        // path, not just the wrap helper. Returns one trimmed-trailing
        // `String` per backend row so assertions read naturally.
        use ratatui::{backend::TestBackend, Terminal};
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            block.render(area, frame.buffer_mut());
        })
        .expect("draw");
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                let mut row = String::new();
                for x in 0..w {
                    row.push_str(buf[(x, y)].symbol());
                }
                row.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn textblock_wraps_long_line_at_word_boundary() {
        // Greedy wrap — "the quick brown fox" into width 10 splits as
        // ["the quick", "brown fox"] (10 + nothing-extra fits "the
        // quick"; "brown fox" together is 9). SPEC §4.7 requires
        // wrapping at the available width.
        let block = TextBlock::new("the quick brown fox");
        let rows = block.wrapped_rows(10);
        assert_eq!(rows, vec!["the quick".to_string(), "brown fox".to_string()]);
    }

    #[test]
    fn textblock_preserves_explicit_blank_lines() {
        // SPEC §4.7: "Preserve explicit blank lines." Two paragraphs
        // separated by `\n\n` MUST produce a blank visual row between
        // them after wrapping.
        let block = TextBlock::new("first paragraph\n\nsecond paragraph");
        let rows = block.wrapped_rows(40);
        assert_eq!(
            rows,
            vec![
                "first paragraph".to_string(),
                String::new(),
                "second paragraph".to_string(),
            ]
        );
    }

    #[test]
    fn textblock_hard_breaks_word_longer_than_width() {
        // SPEC §6 forbids silently dropping content. A word longer
        // than the width must hard-break rather than overflow or
        // disappear.
        let block = TextBlock::new("antidisestablishmentarianism");
        let rows = block.wrapped_rows(10);
        // 28 chars / 10 = 3 rows: "antidisest", "ablishment", "arianism"
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].chars().count(), 10);
        assert_eq!(rows[1].chars().count(), 10);
        assert_eq!(rows[2], "arianism");
        // Concatenating the rows MUST reproduce the original word —
        // "no silent drop" is the load-bearing invariant.
        let joined: String = rows.join("");
        assert_eq!(joined, "antidisestablishmentarianism");
    }

    #[test]
    fn textblock_zero_width_returns_no_rows_and_does_not_panic() {
        // SPEC §6: prompt rendering MUST NOT panic on small areas.
        let block = TextBlock::new("anything");
        assert!(block.wrapped_rows(0).is_empty());
    }

    #[test]
    fn textblock_renders_under_test_backend_with_wrapped_rows() {
        // SPEC §6: deterministic output under TestBackend. Drives the
        // full `render` path (not just the helper) so we catch buffer
        // off-by-ones now rather than during prompt integration in 5b.
        let block = TextBlock::new("the quick brown fox jumps");
        let rows = render_textblock_to_strings(&block, 10, 4);
        // Expected greedy wrap into width 10:
        //   "the quick"      (9)
        //   "brown fox"      (9)
        //   "jumps"          (5)
        //   ""               (unused row)
        assert_eq!(
            rows,
            vec![
                "the quick".to_string(),
                "brown fox".to_string(),
                "jumps".to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn textblock_render_clips_when_height_exceeded() {
        // SPEC §6: clip rather than panic when the body is taller than
        // the slot. Returns the actually-drawn row count so the caller
        // can layout the rest of the prompt.
        let block = TextBlock::new("alpha\nbeta\ngamma\ndelta");
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let drawn = block.render(area, &mut buf);
        assert_eq!(drawn, 2);
        // Only the first two lines made it onto the buffer.
        let row0: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        let row1: String = (0..area.width).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(row0.trim_end(), "alpha");
        assert_eq!(row1.trim_end(), "beta");
    }

    #[test]
    fn textblock_render_zero_area_is_a_noop() {
        // Defensive — the bordered modal renderer in 5e may compute a
        // 0×0 inner rect on a tiny terminal. `render` MUST quietly do
        // nothing rather than panic.
        let block = TextBlock::new("anything");
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(block.render(area, &mut buf), 0);
    }

    #[test]
    fn textblock_strips_trailing_carriage_returns() {
        // Windows-authored content uses `\r\n`; the split-on-`\n`
        // constructor MUST normalise away the trailing `\r` so it does
        // not show up as a stray cell in the buffer.
        let block = TextBlock::new("alpha\r\nbeta\r\n");
        assert_eq!(
            block.lines(),
            &["alpha".to_string(), "beta".to_string(), String::new()]
        );
    }

    #[test]
    fn textblock_with_style_records_role_for_theme_layer() {
        // Task 5f will read this; today it just needs to round-trip.
        let block = TextBlock::new("status").with_style(StyleRole::Success);
        assert_eq!(block.style(), Some(StyleRole::Success));
    }

    #[test]
    fn textblock_from_lines_preserves_input_order_and_blanks() {
        // `from_lines` is the constructor for callers that have
        // already split (e.g. composing a status block from a `Vec`).
        // It MUST NOT re-collapse blank entries.
        let block = TextBlock::from_lines(["one", "", "two"]);
        assert_eq!(
            block.wrapped_rows(40),
            vec!["one".to_string(), String::new(), "two".to_string()]
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

    // -- Task 5b: compact unboxed ChoicePrompt rendering -------------

    /// Render a `ChoicePrompt` through `TestBackend` and return one
    /// trimmed-trailing `String` per visible row. Mirrors the helper
    /// used for `TextBlock` so the two render paths assert against the
    /// same backend semantics.
    fn render_prompt_to_strings<T>(prompt: &ChoicePrompt<T>, w: u16, h: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                let mut row = String::new();
                for x in 0..w {
                    row.push_str(buf[(x, y)].symbol());
                }
                row.trim_end().to_string()
            })
            .collect()
    }

    #[test]
    fn rendered_lines_matches_spec_4_3_giant_spider_example() {
        // SPEC §4.3 reference rendering, modulo the typed-letter trail
        // ("Your choice: e" — the `e` is the player's typed input,
        // which the renderer never produces). The renderer's job ends
        // at the prompt label.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("You found this on the Giant Spider's corpse.")
            .choice('e', LootAction::Equip, "Equip immediately")
            .choice('t', LootAction::Take, "Take to inventory")
            .choice('p', LootAction::Pass, "Pass");

        let rows = prompt.rendered_lines(60);
        assert_eq!(
            rows,
            vec![
                "You found this on the Giant Spider's corpse.".to_string(),
                String::new(),
                "(E) Equip immediately".to_string(),
                "(T) Take to inventory".to_string(),
                "(P) Pass".to_string(),
                String::new(),
                "Your choice:".to_string(),
            ]
        );
    }

    #[test]
    fn rendered_lines_uppercases_marker_for_lowercase_or_uppercase_input() {
        // SPEC §4.1: storage is lowercase, display is uppercase. The
        // builder normalises `'E'` to `'e'` on the way in, and the
        // renderer normalises back on the way out, regardless of which
        // case the author typed.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('E', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        let rows = prompt.rendered_lines(40);
        assert_eq!(rows[0], "(E) Equip");
        assert_eq!(rows[1], "(T) Take");
    }

    #[test]
    fn rendered_lines_matches_spec_9_lost_and_found_example() {
        // SPEC §9 Murder Motel proof scene: four direct-key choices,
        // `Your choice:` label. The renderer's compact mode is the
        // contract that scene relies on once Task 10 wires it up.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Drawer {
            Key,
            Match,
            Receipt,
            Leave,
        }
        let prompt: ChoicePrompt<Drawer> = ChoicePrompt::new()
            .choice('k', Drawer::Key, "Take the Room 7 key")
            .choice('m', Drawer::Match, "Pocket the matchbook")
            .choice('r', Drawer::Receipt, "Read the receipt")
            .choice('l', Drawer::Leave, "Leave it alone");

        let rows = prompt.rendered_lines(60);
        assert_eq!(
            rows,
            vec![
                "(K) Take the Room 7 key".to_string(),
                "(M) Pocket the matchbook".to_string(),
                "(R) Read the receipt".to_string(),
                "(L) Leave it alone".to_string(),
                String::new(),
                "Your choice:".to_string(),
            ]
        );
    }

    #[test]
    fn rendered_lines_includes_optional_footer_with_blank_gap() {
        // SPEC §5 vendor example footer (`Your gold: 173g`) sits below
        // the choices, separated by a blank row, and BEFORE the prompt
        // label so the cost annotation is the last context the player
        // sees before typing.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .footer("Your gold: 173g");

        let rows = prompt.rendered_lines(40);
        assert_eq!(
            rows,
            vec![
                "(E) Equip".to_string(),
                String::new(),
                "Your gold: 173g".to_string(),
                String::new(),
                "Your choice:".to_string(),
            ]
        );
    }

    #[test]
    fn rendered_lines_preserves_explicit_blank_lines_in_body() {
        // Multi-paragraph body via two `.body(...)` calls. The
        // `from_lines` path inside the renderer emits one blank visual
        // row between them — matching the SPEC §4.7 contract that the
        // `TextBlock` tests already pin down.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("first")
            .body("")
            .body("second")
            .choice('e', LootAction::Equip, "Equip");

        let rows = prompt.rendered_lines(40);
        assert_eq!(
            rows,
            vec![
                "first".to_string(),
                String::new(),
                "second".to_string(),
                String::new(),
                "(E) Equip".to_string(),
                String::new(),
                "Your choice:".to_string(),
            ]
        );
    }

    #[test]
    fn rendered_lines_omits_prompt_label_when_no_choices() {
        // A choiceless prompt is a pure narration block — the
        // `Your choice:` label would invite a key press the prompt has
        // no handler for, so the renderer drops it.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new().body("narration only");
        let rows = prompt.rendered_lines(40);
        assert_eq!(rows, vec!["narration only".to_string()]);
    }

    #[test]
    fn rendered_lines_zero_width_returns_no_rows_and_does_not_panic() {
        // SPEC §6: prompt rendering MUST NOT panic on small areas. A
        // zero-width slot is the worst case the bordered-modal path in
        // Task 5e will hand us; pinning it now keeps the contract
        // honest before that layout lands.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        assert!(prompt.rendered_lines(0).is_empty());
    }

    #[test]
    fn render_writes_rows_under_test_backend() {
        // SPEC §6: deterministic output under `TestBackend`. Drives the
        // full `render` path (not just the helper) so a future
        // off-by-one in `Buffer` indexing surfaces here, not during a
        // Murder Motel smoke test.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("You found this on the Giant Spider's corpse.")
            .choice('e', LootAction::Equip, "Equip immediately")
            .choice('t', LootAction::Take, "Take to inventory")
            .choice('p', LootAction::Pass, "Pass");

        let rows = render_prompt_to_strings(&prompt, 60, 8);
        assert_eq!(
            rows,
            vec![
                "You found this on the Giant Spider's corpse.".to_string(),
                String::new(),
                "(E) Equip immediately".to_string(),
                "(T) Take to inventory".to_string(),
                "(P) Pass".to_string(),
                String::new(),
                "Your choice:".to_string(),
                String::new(),
            ]
        );
    }

    #[test]
    fn render_clips_when_height_exceeded_without_panic() {
        // SPEC §6: clip rather than panic when the prompt is taller
        // than the slot. Returning the actually-drawn count lets the
        // caller layout below us; the buffer must still be filled
        // top-down with the rows that fit.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("body")
            .choice('a', LootAction::Equip, "alpha")
            .choice('b', LootAction::Take, "beta")
            .choice('c', LootAction::Pass, "gamma");
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        let drawn = prompt.render(area, &mut buf);
        assert_eq!(drawn, 2);
        let row0: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        let row1: String = (0..area.width).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(row0.trim_end(), "body");
        assert_eq!(row1.trim_end(), "");
    }

    #[test]
    fn render_zero_area_is_a_noop() {
        // Defensive — a 0×0 inner rect must not panic. The bordered
        // modal renderer in Task 5e relies on this guarantee.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("anything")
                .choice('e', LootAction::Equip, "Equip");
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(prompt.render(area, &mut buf), 0);
    }

    #[test]
    fn rendered_lines_wraps_long_choice_label_at_word_boundary() {
        // SPEC §6: truncation OR wrapping at terminal width. A label
        // wider than the slot must wrap rather than overflow — the
        // greedy whitespace policy from `TextBlock` is the same one we
        // re-use, so behaviour stays consistent across body and
        // choice rows.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new().choice(
            'e',
            LootAction::Equip,
            "Equip the very long sword of overflow",
        );
        let rows = prompt.rendered_lines(20);
        // "(E) Equip the very" is 18 cols, "long sword of" is 13,
        // "overflow" is 8. The exact split is the wrapper's policy;
        // pinning the *count* (3 wrapped lines + blank + label) is
        // what the renderer guarantees.
        assert!(rows.len() >= 3);
        for row in rows.iter() {
            assert!(row.chars().count() <= 20, "row {row:?} exceeds width 20");
        }
        assert_eq!(rows.last().map(String::as_str), Some("Your choice:"));
    }

    // ---- Task 5c: disabled-row monochrome marker ----
    //
    // The visible difference between an enabled and a disabled row
    // MUST survive in a strictly monochrome terminal — SPEC §4.2's
    // contract — so these tests assert *characters*, never style. The
    // bracket flip (`(K)` → `[K]`) plus the `- ` leader plus the
    // trailing `(reason)` are the three carriers of meaning the SPEC
    // example `- [M] Mana potions ... (full)` puts on screen.

    #[test]
    fn rendered_lines_marks_disabled_choice_with_monochrome_marker_and_reason() {
        // SPEC §4.2 reference example. The wandering-monk row from
        // SPEC §5 is the canonical disabled-with-reason rendering and
        // is the precise shape this checklist item gates on.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Equip, "Mana potions")
            .disabled_if(true, "full");
        let rows = prompt.rendered_lines(60);
        assert_eq!(rows[0], "- [M] Mana potions (full)");
    }

    #[test]
    fn rendered_lines_disabled_without_reason_still_carries_marker() {
        // Disabled-reason is optional at the data layer (`enabled =
        // false` with no reason is legal — see `PromptChoice::new`).
        // The leader + bracket flip MUST still distinguish the row,
        // because that is the only mono-visible signal left when the
        // reason is absent.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");
        // Hand-craft via the builder's disabled flag without a reason
        // by bypassing `disabled_if` — the builder sets reason every
        // time, but `with_disabled_reason` is independent of `enabled`.
        // We mutate the choice directly to pin the no-reason path.
        let mut prompt = prompt;
        prompt.choices[0].enabled = false;
        let rows = prompt.rendered_lines(40);
        assert_eq!(rows[0], "- [E] Equip");
    }

    #[test]
    fn rendered_lines_mixes_enabled_and_disabled_rows_distinctly() {
        // Two rows side-by-side prove the marker carries differential
        // meaning, not just a global cosmetic change. `(E) Equip` vs
        // `- [T] Take (bag full)` is the visual the player sees in
        // a Murder Motel loot drawer with one slot occupied.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "bag full");
        let rows = prompt.rendered_lines(60);
        assert_eq!(rows[0], "(E) Equip");
        assert_eq!(rows[1], "- [T] Take (bag full)");
    }

    #[test]
    fn rendered_lines_disabled_row_keeps_hint_between_label_and_reason() {
        // Vendor-style rows carry a price/capacity hint AND, when the
        // player cannot afford the row, a disabled reason. Both must
        // remain visible: hint annotates *what* the option would do,
        // reason annotates *why* it is unavailable, and the SPEC §5
        // wandering monk is the prototype prompt that wants both.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Equip, "Mana potions")
            .disabled_if(true, "need 50g");
        prompt.choices[0].hint = Some("174g".to_string());
        let rows = prompt.rendered_lines(60);
        // The wrapper collapses internal whitespace runs to a single
        // space (see `wrap_line_into`), so the two-space gutter that
        // `format_choice_row` writes between label and hint shows up
        // as one space in the rendered row. The contract this test
        // pins is the *order*: label, then hint, then `(reason)`.
        assert_eq!(rows[0], "- [M] Mana potions 174g (need 50g)");
    }

    #[test]
    fn rendered_lines_matches_spec_5_wandering_monk_disabled_row() {
        // SPEC §5: the wandering-monk vendor prompt carries a
        // `disabled_if(!can_buy, ...)` row plus an enabled "no thanks"
        // out. Pinning the full rendering shape here protects future
        // edits to `format_choice_row` from quietly changing what
        // authors see when they paste the SPEC example into their
        // game.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum ShopAction {
            BuyMana,
            NoThanks,
        }
        let prompt: ChoicePrompt<ShopAction> = ChoicePrompt::new()
            .body("A wandering monk approaches after the battle...")
            .choice('m', ShopAction::BuyMana, "Mana potions")
            .disabled_if(true, "not enough gold or potion bag is full")
            .choice('n', ShopAction::NoThanks, "No thanks")
            .footer("Your gold: 32g");
        let rows = prompt.rendered_lines(80);
        assert_eq!(
            rows,
            vec![
                "A wandering monk approaches after the battle...".to_string(),
                String::new(),
                "- [M] Mana potions (not enough gold or potion bag is full)".to_string(),
                "(N) No thanks".to_string(),
                String::new(),
                "Your gold: 32g".to_string(),
                String::new(),
                "Your choice:".to_string(),
            ]
        );
    }

    #[test]
    fn render_writes_disabled_marker_under_test_backend() {
        // SPEC §6: deterministic under TestBackend, including the
        // disabled marker. Drives the full `render` path so a buffer
        // indexing bug surfaces against the visible characters, not
        // just the helper that builds the row strings.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Equip, "Mana potions")
            .disabled_if(true, "full");
        let rows = render_prompt_to_strings(&prompt, 40, 4);
        assert_eq!(rows[0], "- [M] Mana potions (full)");
    }

    // ---- Task 5d: selected-row marker + reverse-video for nav mode ----
    //
    // Two carriers of meaning per SPEC §6: a character marker (`> `
    // prefix on the cursor row) so a strictly monochrome terminal
    // still shows selection, AND a `Modifier::REVERSED` style for
    // color-capable terminals. The tests below assert *both* layers
    // independently so a future refactor cannot drop one and pass.

    #[test]
    fn rendered_lines_marks_selected_row_with_caret_in_nav_mode() {
        // `.navigable(true)` seeds the cursor on the first enabled
        // choice; that row gets `> `, the others get a `  ` indent so
        // the label column stays aligned. Disabled rows in nav mode
        // also receive the indent — they are not the cursor, but the
        // alignment is what makes the marker scannable.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        let rows = prompt.rendered_lines(40);
        assert_eq!(rows[0], "> (E) Equip");
        assert_eq!(rows[1], "  (T) Take");
    }

    #[test]
    fn rendered_lines_marker_follows_cursor_after_move_down() {
        // Pinning the marker to `selected` rather than "first row"
        // protects the SPEC §4.5 contract that Up/Down moves the
        // cursor visibly. After one `move_down` the marker MUST be
        // on the second enabled choice.
        let mut prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        prompt.move_down();
        let rows = prompt.rendered_lines(40);
        assert_eq!(rows[0], "  (E) Equip");
        assert_eq!(rows[1], "> (T) Take");
    }

    #[test]
    fn rendered_lines_no_marker_when_not_in_nav_mode() {
        // Direct-key prompts (no `.navigable(true)`) keep `selected =
        // None`, so the SPEC §4.3 reference rendering survives
        // byte-for-byte. This is the regression guard for Task 5b's
        // existing snapshot tests.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");
        let rows = prompt.rendered_lines(40);
        assert_eq!(rows[0], "(E) Equip");
        assert_eq!(rows[1], "(T) Take");
    }

    #[test]
    fn rendered_lines_marker_indents_disabled_row_in_nav_mode() {
        // Disabled rows already carry the `- [K]` monochrome marker
        // from Task 5c. In nav mode they additionally get the 2-cell
        // indent so the cursor's `> ` and the disabled `- [` line up
        // visually instead of jagging left.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .disabled_if(true, "bag full")
            .navigable(true);
        let rows = prompt.rendered_lines(60);
        assert_eq!(rows[0], "> (E) Equip");
        assert_eq!(rows[1], "  - [T] Take (bag full)");
    }

    #[test]
    fn rendered_lines_marker_skips_unrelated_rows() {
        // Body, footer, blank gaps, and the prompt label are not
        // choices and MUST NOT receive the cursor marker — only the
        // selected choice row does. This is the contract that lets
        // `render` apply `REVERSED` to exactly one row.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .body("pick one")
            .choice('e', LootAction::Equip, "Equip")
            .footer("hint")
            .navigable(true);
        let rows = prompt.rendered_lines(40);
        // body, blank, choice, blank, footer, blank, label
        assert_eq!(rows[0], "pick one");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "> (E) Equip");
        assert_eq!(rows[3], "");
        assert_eq!(rows[4], "hint");
        assert_eq!(rows[5], "");
        assert_eq!(rows[6], "Your choice:");
    }

    #[test]
    fn rendered_lines_marker_only_on_first_wrapped_row_of_selected_choice() {
        // A long label that wraps across multiple visual rows shows
        // the `> ` marker only on the first row; continuation rows
        // keep the 2-cell indent for column alignment. Stacking the
        // marker on every wrapped row would make the cursor look like
        // a multi-line selection, which it is not.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice(
                'e',
                LootAction::Equip,
                "Equip the very long sword of overflow",
            )
            .navigable(true);
        let rows = prompt.rendered_lines(20);
        assert!(rows[0].starts_with("> "));
        // At least one continuation row exists at this width and it
        // must carry the indent, never another `> ` marker.
        assert!(rows.len() >= 2);
        assert!(rows[1].starts_with("  "));
        assert!(!rows[1].starts_with("> "));
    }

    #[test]
    fn rendered_lines_drops_indent_when_width_too_narrow_for_marker() {
        // SPEC §6: MUST NOT panic on small areas. With width < 3 the
        // 2-cell prefix would not fit; the marker layer drops out and
        // the label re-occupies the slot. The reverse-video path in
        // `render` still flags the row, so the cursor stays visible
        // on color terminals even when the character marker cannot.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .navigable(true);
        let rows = prompt.rendered_lines(2);
        // Width 2 wraps `(E) Equip` aggressively but never panics
        // and never produces a row starting with `> ` (no room).
        for row in &rows {
            assert!(
                !row.starts_with("> "),
                "row {row:?} kept the marker at width 2"
            );
            assert!(row.chars().count() <= 2, "row {row:?} exceeded width 2");
        }
    }

    #[test]
    fn render_applies_reversed_modifier_to_selected_row() {
        // Color-layer half of Task 5d. Drives `render` through
        // `TestBackend` and asserts the buffer cell on the cursor
        // row carries `Modifier::REVERSED`, while a non-selected
        // choice row does not. Pinning the modifier (rather than a
        // full `Style`) keeps this resilient to Task 5f's theme
        // layer landing on top.
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;
        use ratatui::Terminal;
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        let backend = TestBackend::new(40, 4);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");
        let buf = term.backend().buffer().clone();
        // Row 0 is the selected `> (E) Equip` — every cell on that
        // row that the renderer wrote MUST carry REVERSED. Checking
        // the marker cell at x=0 is enough; if any cell on the row
        // missed the style we have a bigger bug than this test
        // describes.
        assert!(
            buf[(0, 0)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "selected row missing REVERSED modifier"
        );
        // Row 1 is the non-selected `  (T) Take` — MUST NOT be
        // reverse-styled.
        assert!(
            !buf[(0, 1)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "non-selected row picked up REVERSED modifier"
        );
    }

    #[test]
    fn render_does_not_apply_reversed_when_not_in_nav_mode() {
        // Direct-key prompts MUST render with the buffer's default
        // style — a stray REVERSED on a direct-key prompt would
        // wrongly imply a cursor exists. This is the regression guard
        // for the Task 5b reference snapshot.
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;
        use ratatui::Terminal;
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");
        let backend = TestBackend::new(40, 4);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");
        let buf = term.backend().buffer().clone();
        assert!(
            !buf[(0, 0)]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED),
            "direct-key prompt picked up REVERSED on choice row"
        );
    }
}
