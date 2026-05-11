//! Text-interface prompt primitives — the v1.1 layer between
//! `Screen`/`Input` and door-game prompt flows.
//!
//! # Why this module exists
//!
//! lets a game move a player around an ASCII map and run scripted
//! dialog graphs. v1.1 adds the missing piece for classic BBS/RPG
//! flows: loot prompts, vendor transactions, confirmation gates.
//! "press any key" pauses, and post-action feedback..md
//! and establish the contract; this module is the home for that
//! contract's implementation.
//!
//! # Architecture in one sentence
//!
//! Prompts are **pure reducers plus Ratatui render helpers**. A
//! `ChoicePrompt` value is data; sending an [`crate::Input`] through
//! it returns a typed `PromptAction`; rendering is a separate function
//! that draws into a `Frame` or buffer. Game state lives outside the
//! prompt — the prompt routes choices, it does not own gold.
//! inventory, or flags.
//!
//! ## Why not a scripting VM, an "engine", or a widget with hidden state?
//!
//! Considered and rejected during:
//!
//! - **Scripting VM** (Lua/Rhai/custom) —.md forbids
//!   it, and door-game prompts need live Rust state (inventory, gold.
//!   capacity, flag sets) that is awkward to thread through an
//!   interpreter. Builder-driven Rust prompts also test under
//!   `TestBackend` without spinning up a runtime.
//! - **Stateful widget that owns selection internally** (the obvious
//!   "Ratatui-flavored" route) — hides cursor state behind a `&mut`
//!   handle, makes input handling order-dependent, and resists
//!   table-driven reducer tests. The `DialogState` already
//!   established the pure-reducer pattern; v1.1 stays consistent.
//! - **Forcing every prompt through `DialogState` YAML** — works for
//!   static NPC chatter but cannot express dynamic labels like
//!   `"Mana potions: 174g each | You have: 0/26"`..md
//!   explicitly keeps Rust-authored prompts first-class.
//!
//! Pure reducers + render helpers won because they (1) test cleanly
//! under `ratatui::backend::TestBackend`, (2) compose with the
//! existing `Screen` trait without a new runtime, and (3) leave game
//! semantics in game code where `disabled_if(!can_buy,...)` reads
//! naturally next to the `gold`/`inventory` fields it inspects. The
//! optional [`crate::screen::Screen`] adapter is a thin
//! convenience over the same primitives.
//!
//! # Examples
//!
//! Three small end-to-end doctests covering the prompt shapes
//! is required to demonstrate: a direct-key choice, an Enter-default
//! confirmation, and an any-key pause. Each test wires the reducer
//! directly to [`Input`] so the contract is exercised without a live
//! terminal — the same pattern an in-game `Screen::handle` uses.
//!
//! ## Direct-key choice prompt
//!
//! Build a [`ChoicePrompt`], send a hotkey through it, and route the
//! [`PromptAction`] back into game state. The Murder Motel
//! Lost-and-Found Drawer (.md ) uses exactly this shape.
//!
//! ```
//! use foglet_game::{ChoicePrompt, Input, PromptAction};
//!
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)]
//! enum DrawerAction { TakeKey, Leave }
//!
//! let prompt = ChoicePrompt::new()
//!     .body("A musty drawer holds a tagged room key.")
//!     .choice('k', DrawerAction::TakeKey, "Take the Room 7 key")
//!     .choice('l', DrawerAction::Leave, "Leave it");
//!
//! // Lowercase and uppercase reach the same arm
//! assert_eq!(
//!     prompt.handle(Input::Char('K')),
//!     PromptAction::Selected(DrawerAction::TakeKey),
//! );
//! // Resize is not a prompt-relevant signal.
//! assert_eq!(
//!     prompt.handle(Input::Resize { width: 80, height: 24 }),
//!     PromptAction::None,
//! );
//! ```
//!
//! ## Confirmation prompt with Enter default
//!
//! [`ConfirmPrompt`] fixes the choice list to `(Y)es (N)o`, projects
//! outcomes onto [`ConfirmOutcome`], and lets the author opt into an
//! Enter-default. calls these out as a distinct prompt shape
//! because their input contract — Enter-as-default, Esc-as-cancel — is
//! load-bearing for "are you sure?" beats.
//!
//! ```
//! use foglet_game::{ConfirmOutcome, ConfirmPrompt, Input};
//!
//! let prompt = ConfirmPrompt::new("Overwrite the existing save?")
//!     .default_no();
//!
//! // Enter takes the safe default — a stray press will not destroy data.
//! assert_eq!(prompt.handle(Input::Enter), ConfirmOutcome::No);
//! // The hotkey still works when the player commits to the action.
//! assert_eq!(prompt.handle(Input::Char('y')), ConfirmOutcome::Yes);
//! // Esc backs out without resolving the question.
//! assert_eq!(prompt.handle(Input::Esc), ConfirmOutcome::Cancelled);
//! ```
//!
//! ## Any-key pause prompt
//!
//! [`AnyKeyPrompt`] is the smallest reducer in the kit: show
//! some narration, wait for any meaningful keypress to acknowledge.
//! Resize and [`Input::Unknown`] are intentionally ignored so a player
//! resizing their terminal mid-pause never loses the body.
//!
//! ```
//! use foglet_game::{AnyKeyOutcome, AnyKeyPrompt, Input};
//!
//! let pause = AnyKeyPrompt::new()
//!     .body("The night clerk slides a chipped mug across the counter.")
//!     .footer("Press any key to continue...");
//!
//! // Resize is a layout event, not acknowledgement.
//! assert_eq!(
//!     pause.handle(Input::Resize { width: 80, height: 24 }),
//!     AnyKeyOutcome::None,
//! );
//! // Any meaningful key completes the pause.
//! assert_eq!(pause.handle(Input::Char(' ')), AnyKeyOutcome::Completed);
//! assert_eq!(pause.handle(Input::Enter), AnyKeyOutcome::Completed);
//! ```
//!
//! # Module map (target — populated across )
//!
//! - `PromptKey` — normalized direct-input key.
//! - `PromptChoice<T>` — one selectable option with stable id and
//!   optional disabled reason/hint/style role.
//! - `ChoicePrompt<T>` + `PromptAction<T>` — the core reducer
//!   .
//! - `ConfirmPrompt`, `AnyKeyPrompt` — small specialised reducers
//!   layered over `ChoicePrompt`.
//! - `TextBlock`, semantic style roles, prompt rendering helpers
//!   .
//! - `PromptScreen` — optional `Screen` adapter.
//!
//!  only stands the module up so `lib.rs` re-exports compile;
//! the types above land alongside the tasks that exercise them.

use crate::Input;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

/// Normalized direct-input key for prompts (.md ).
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
/// `Input` is the runtime's full event vocabulary (resize, ctrl, arrow.
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
    /// Enter Return — used to confirm the highlighted choice in the
    /// optional arrow/Enter navigation mode.
    Enter,
    /// Escape — used to cancel the prompt when cancellation is
    /// configured.
    Esc,
}

impl PromptKey {
    /// Construct a character prompt key, lowercasing ASCII letters so
    /// direct-key matching is case-insensitive.
    ///
    /// Use this when defining choice hotkeys in a `PromptChoice` (or
    /// equivalent builder, landing in ); the reducer will
    /// compare incoming [`PromptKey::Char`] values for equality with no
    /// further casing.
    pub fn char(c: char) -> Self {
        PromptKey::Char(c.to_ascii_lowercase())
    }

    /// Translate an [`Input`] into a [`PromptKey`] when the input is
    /// prompt-relevant; return `None` otherwise.
    ///
    /// `None` covers the inputs prompts deliberately ignore: resize
    /// (.md — "resize ignored by prompt selection logic").
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
            // ; the direct-key reducer treats them as not its
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

/// Semantic style role for prompt text and choices (.md ).
///
/// Prompts attach roles, not raw ANSI. The render layer maps
/// roles to a Ratatui `Style`, which keeps two properties that the
///  requires:
///
/// 1. **Color is never the only carrier of meaning** — disabled rows
///    still render visibly different in monochrome because the
///    renderer adds a marker/prefix when it sees `StyleRole::Disabled`.
///    not just because the text is dim.
/// 2. **Themes are overridable** — a game (or a future operator config)
///    can swap the role-to-`Style` mapping without touching the prompt
///    data, because the data only carries the role enum.
///
/// The variants are the -listed initial roles. Adding a role is a
/// -level change; do not extend this enum to carry per-prompt
/// styling — use `hint` `disabled_reason` text instead.
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
    /// Numeric currency cost annotations.
    Currency,
    /// Disabled choice rows. Renderer pairs this with a monochrome
    /// marker so the disabled state survives a black/white
    /// terminal.
    Disabled,
    /// Highlighted hotkey character inside a choice marker like `(E)`.
    Hotkey,
}

/// Mapping from semantic [`StyleRole`] to a concrete Ratatui [`Style`]
/// (.md ).
///
/// Prompts carry roles, not raw styling — see [`StyleRole`] for the why.
/// `Theme` is the layer that resolves a role into the [`Style`] the
/// renderer hands to Ratatui. Splitting the data (role) from the
/// presentation (theme) is what lets a game (or, in the future, an
/// operator config) re-skin every prompt without re-walking the prompt
/// tree.
///
/// # Default theme contract
///
///  requires that "the default theme MUST be readable on
/// black/white terminals" and that "color MUST NOT be the only carrier
/// of meaning". The defaults below honour both:
///
/// - Every "loud" role (`Title`, `Emphasis`, `Hotkey`, `Error`) carries
///   a [`Modifier`] so the role survives a monochrome terminal.
/// - Color choices follow ANSI conventions a BBS operator will
///   recognise (red/error, green/success, yellow/currency, cyan/hotkey)
///   so a colored terminal communicates *the same thing* the modifier
///   already says.
/// - `Disabled` and `Muted` use `Modifier::DIM` only — the disabled-row
///   visual difference is already carried by the `- [K] Label (reason)`
///   marker shape in the choice-row formatter, so the dim is a *bonus*.
///   not a load-bearing signal.
///
/// # Overriding
///
/// `Theme` derefs each role lookup through [`Theme::style`]. To swap a
/// single role, build off the default with [`Theme::with_role`]:
///
/// ```ignore
/// use foglet_game::{StyleRole, prompt::Theme};
/// use ratatui::style::{Color, Modifier, Style};
/// let theme = Theme::default
///     .with_role(StyleRole::Hotkey, Style::default.fg(Color::Magenta));
/// ```
///
/// The renderer integration is layered: today this type is a standalone
/// mapping unit-tested for deterministic field values. Wiring the theme
/// into [`ChoicePrompt::render`] [`TextBlock::render`] happens once
/// the choice-row spans need per-segment styling ( feedback line
/// helper is the natural caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    // Storing one Style per variant in declaration order keeps the
    // resolver branch-light (the match below compiles to a fixed
    // index) and makes `with_role` a single field assignment. A
    // `HashMap<StyleRole, Style>` would also work but adds an alloc
    // and a hash on every lookup for no functional gain — there are
    // ten roles and the set is closed.
    normal: Style,
    title: Style,
    emphasis: Style,
    muted: Style,
    hint: Style,
    error: Style,
    success: Style,
    currency: Style,
    disabled: Style,
    hotkey: Style,
}

impl Theme {
    /// Resolve a role to its [`Style`] under this theme.
    ///
    /// `Copy` return because [`Style`] is itself `Copy` and the
    /// renderer wants to compose multiple roles into one `Style`
    /// without dragging a borrow around.
    pub fn style(&self, role: StyleRole) -> Style {
        match role {
            StyleRole::Normal => self.normal,
            StyleRole::Title => self.title,
            StyleRole::Emphasis => self.emphasis,
            StyleRole::Muted => self.muted,
            StyleRole::Hint => self.hint,
            StyleRole::Error => self.error,
            StyleRole::Success => self.success,
            StyleRole::Currency => self.currency,
            StyleRole::Disabled => self.disabled,
            StyleRole::Hotkey => self.hotkey,
        }
    }

    /// Replace the [`Style`] mapped to `role`. Returns the modified
    /// theme so calls chain off [`Theme::default`].
    pub fn with_role(mut self, role: StyleRole, style: Style) -> Self {
        match role {
            StyleRole::Normal => self.normal = style,
            StyleRole::Title => self.title = style,
            StyleRole::Emphasis => self.emphasis = style,
            StyleRole::Muted => self.muted = style,
            StyleRole::Hint => self.hint = style,
            StyleRole::Error => self.error = style,
            StyleRole::Success => self.success = style,
            StyleRole::Currency => self.currency = style,
            StyleRole::Disabled => self.disabled = style,
            StyleRole::Hotkey => self.hotkey = style,
        }
        self
    }
}

impl Default for Theme {
    /// -compliant default mapping. See the type-level docs for
    /// the rationale behind each role's modifier and color pick.
    fn default() -> Self {
        Self {
            // Plain body text. No modifier and no color so a TUI host
            // that never overrides theming still gets the terminal's
            // own foreground — the same behaviour the prompt renderer
            // had before.
            normal: Style::default(),
            // BOLD makes titles stand out in monochrome; no color so
            // the title sits in the terminal's foreground unless a
            // game opts into a tint via `with_role`.
            title: Style::default().add_modifier(Modifier::BOLD),
            // BOLD is the 's "loud but not error-loud" carrier.
            emphasis: Style::default().add_modifier(Modifier::BOLD),
            // DIM is universally understood as secondary text on
            // black/white terminals; pairs with the `Hint` role for
            // inline footnotes.
            muted: Style::default().add_modifier(Modifier::DIM),
            // Same DIM as `Muted` by default — hints are semantically a
            // narrower role, but the calls for the same visual
            // weight ("footnote-style"). Splitting them as separate
            // fields lets a game push them apart without breaking the
            // default behaviour.
            hint: Style::default().add_modifier(Modifier::DIM),
            // Error: red foreground PLUS bold so a monochrome terminal
            // still reads "loud" even though it cannot show the red.
            // 's "color MUST NOT be the only carrier" test.
            error: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            // Success: green is the universal positive signal; no
            // modifier because the surrounding success-feedback line
            //  carries its own marker prefix.
            success: Style::default().fg(Color::Green),
            // Currency: yellow tracks the BBS convention of gold/coin
            // text being yellow; the value still reads correctly when
            // color is stripped because the label itself spells it out
            // (`"50g"`).
            currency: Style::default().fg(Color::Yellow),
            // Disabled rows already carry the `- [K]...` marker shape
            //  — the DIM here is supplementary. No color so
            // a terminal that maps DIM to gray still leaves the row
            // legible against a dark background.
            disabled: Style::default().add_modifier(Modifier::DIM),
            // Hotkey: cyan is the historical BBS color for action
            // keys; BOLD is the monochrome carrier so the `(E)` marker
            // visibly differs from surrounding label text even with
            // color stripped.
            hotkey: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        }
    }
}

/// One selectable option inside a `ChoicePrompt` (.md ).
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
/// one-liner. Optional fields — `disabled_reason`, `hint`, `style`
/// are set with `with_*` chain methods. The richer
/// `disabled_if`/`.choice(...)` builder helpers land in on top
/// of this same data.
///
/// # Disabled choices
///
/// `enabled` defaults to `true`. Setting `enabled = false` SHOULD be
/// paired with [`PromptChoice::with_disabled_reason`] so the renderer
///  can show *why* the option is unavailable in monochrome
/// e.g. `- [M] Mana potions... (full)`. The reducer returns
/// the same reason string in `PromptAction::Disabled`, so the game can
/// surface it as feedback without re-deriving the cause.
///
/// # What this type intentionally does NOT do
///
/// - It does not hold game state (gold, inventory). The author wires
///   `enabled = gold >= price` at construction time; the prompt does
///   not re-evaluate predicates on its own.
/// - It does not validate hotkey uniqueness. Duplicate-key detection
///   is a `ChoicePrompt`-level concern because it depends on
///   the *set* of active choices.
/// - It does not own a confirmation policy yet. reserves a
///   `confirm` field; it lands when `ConfirmPrompt` does so
///   the data and the reducer arrive together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptChoice<T> {
    /// Direct-input hotkey. Stored normalised — see [`PromptKey::char`].
    pub key: PromptKey,
    /// Display label. Renderer wraps and styles this; MUST NOT contain
    /// raw terminal control sequences.
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
    /// `key` accepts anything that converts into a [`PromptKey`]
    /// most commonly a `char`, which goes through `PromptKey::char`
    /// and lowercases ASCII letters automatically. `label` accepts any
    /// `Into<String>` so callers can pass `&str` or `String`.
    pub fn new(key: impl Into<PromptKey>, label: impl Into<String>, value: T) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            value,
            // : "`enabled` bool, default `true`". Centralising
            // the default here means the rest of the codebase never has
            // to remember it.
            enabled: true,
            disabled_reason: None,
            hint: None,
            style: None,
        }
    }

    /// Mark this choice disabled and attach the reason. Pairs with
    /// 's "disabled choices MUST render visibly distinct" by
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
/// an invariant the renderer reducer depend on.
///
/// Returned from validators like [`validate_choices`] and.
/// later, builder finalisation and `ChoicePrompt::new`
/// . Authoring errors are intentionally separate from runtime
/// `PromptAction` outcomes — a prompt with duplicate hotkeys is never a
/// player-facing condition, it is a programmer bug that should fail loud
/// the moment the prompt is built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromptError {
    /// Two choices in the same prompt share a hotkey after
    /// case-normalisation..md makes this a hard error
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

/// Reject a choice list that violates.md hotkey rules.
///
/// Specifically: **every** choice in the list reserves its hotkey.
/// regardless of `enabled`. The 's exact wording is "Disabled
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
    // digits in practice; talks about ~5 typical), so the
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

/// A prompt body + choice list + footer, built up via the
/// builder API.
///
/// `ChoicePrompt<T>` is **just data** at this layer. The
/// reducer (`PromptAction<T>`, input-handling, selection) lands in
///  on top of the same struct, and rendering reads
/// the same fields. Splitting "shape of the prompt" from "what
/// pressing a key does" keeps the builder testable without a
/// runtime — `ChoicePrompt::new().body(...).choice(...)` is a pure
/// expression that produces an inspectable value.
///
/// # Builder shape
///
///  fixes the call sites the kit MUST support:
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
/// choice**. That is the form 's wandering-monk example uses.
/// and it lets a prompt express "this row is disabled because …" in
/// the same chain as the row's `.choice(...)` call without naming the
/// row separately. Calling `.disabled_if(...)` before any `.choice(...)`
/// is a no-op (rather than a panic) so partial chains stay safe to
/// inspect during construction.
///
/// # Validation policy
///
/// The builder methods themselves never return `Result` — that would
/// break the fluent chain. Duplicate-hotkey detection runs
/// via [`ChoicePrompt::validate`], and the reducer constructors that
/// arrive in will call it during their `try_*` builders. For
/// now, callers can call `prompt.validate?` at the boundary if they
/// want the same guarantee.
///
/// # What this type intentionally does NOT do (yet)
///
/// - No `confirm` per-choice flag. That lands with `ConfirmPrompt`
///   .
/// - No theme/style storage. Per-choice `style: Option<StyleRole>`
///   already lives on `PromptChoice`; whole-prompt theming is a
///   render-layer concern.
///
/// # Optional arrow/Enter navigation cursor
///
/// `selected` is `None` for the default direct-key flow (every
///  example so far). Calling [`ChoicePrompt::navigable`] with `true`
/// switches the prompt into the optional arrow/Enter mode by seeding
/// `selected` with the index of the first enabled choice; Up/Down/
/// Enter behavior layers on in. Storing the cursor on the
/// prompt (not in a sibling state struct) keeps the navigation-mode
/// path round-trippable through `Clone`/`PartialEq` for tests, and
/// matches how the `DialogState` reducer stores its cursor
/// alongside its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoicePrompt<T> {
    /// Body lines, one per `.body(...)` call. Multi-line bodies are
    /// expressed as multiple `.body(...)` calls so the renderer (Task
    /// 5a) can preserve blank-line spacing without parsing embedded
    /// `\n` sequences.
    pub body: Vec<String>,
    /// Active choices in declaration order. The reducer
    /// scans this list directly; the renderer iterates it for layout.
    pub choices: Vec<PromptChoice<T>>,
    /// Optional footer line — typically dynamic context like
    /// `"Your gold: 173g"`. `None` means the renderer omits the
    /// footer row entirely (no blank gap).
    pub footer: Option<String>,
    /// When `true`, pressing Esc on this prompt returns
    /// [`PromptAction::Cancelled`]; when `false` (the default), Esc
    /// collapses to [`PromptAction::None`] alongside other ignored
    /// inputs..md lists Esc support as
    /// "when configured" — opt-in cancellation prevents a stray Esc
    /// from dismissing a load-bearing prompt (vendor confirmation, save
    /// overwrite) that the game wants to force the player to resolve.
    pub cancellable: bool,
    /// Highlighted choice index for arrow/Enter navigation mode
    /// (.md ).
    ///
    /// `None` — direct-key mode (the default). Hotkeys select choices;
    /// there is no cursor and Up/Down are ignored.
    ///
    /// `Some(i)` — navigation mode is active and the cursor sits on
    /// `choices[i]`. Set by [`ChoicePrompt::navigable`] to the first
    /// enabled choice's index, then advanced by Up/Down and
    /// confirmed by Enter. Index validity is the prompt's
    /// invariant: `i < choices.len` whenever the field is `Some`.
    /// `None` is also produced when navigation is requested but no
    /// choice is enabled — direct-key prompts already handle the empty
    /// case as "no selection possible", and an out-of-bounds `Some(0)`
    /// on an empty prompt would just be a panic waiting to happen.
    pub selected: Option<usize>,
    /// Optional Vim-style `j`/`k` navigation (.md alt
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
    /// Optional modal title rendered into the top border by
    /// [`ChoicePrompt::render_modal`] (.md bordered modal
    /// mode).
    ///
    /// `None` — the bordered modal still renders, but the top border is
    /// drawn unbroken. Compact unboxed mode ([`ChoicePrompt::render`])
    /// ignores this field entirely; it is purely a modal-mode label, so
    /// authors who never call `render_modal` pay nothing for it.
    pub title: Option<String>,
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
    /// Start an empty prompt. Chain `.body(...)`, `.choice(...)`.
    /// `.disabled_if(...)`, `.footer(...)` to fill it in.
    pub fn new() -> Self {
        Self {
            body: Vec::new(),
            choices: Vec::new(),
            footer: None,
            // : Esc support is "when configured". Default off
            // so the safer behaviour (Esc ignored) is the one a careless
            // author gets for free; opting into cancellation is an
            // explicit, single-line builder call.
            cancellable: false,
            // : arrow/Enter navigation is an optional mode.
            // `None` means direct-key only; opt in via `.navigable(true)`
            // *after* the choices are added, since the seeding logic
            // needs to scan them for the first enabled row.
            selected: None,
            //  lists `j`/`k` as an *alternative* keymap, not a
            // default — pure-arrow players (and prompts whose authors
            // bound `j` or `k` as a choice hotkey) get the safer
            // behaviour for free; opt in via `.vim_navigation(true)`.
            vim_navigation: false,
            //  modal title is optional; the default unboxed
            // renderer never reads this field.
            title: None,
        }
    }

    /// Append one body line. Call repeatedly for multi-line bodies
    /// each call is a logical paragraph the renderer will
    /// wrap independently and separate from neighbours.
    pub fn body(mut self, line: impl Into<String>) -> Self {
        self.body.push(line.into());
        self
    }

    /// Append an enabled choice with `key`, stable id `value`, and
    /// display `label`.
    ///
    /// Argument order matches 's example
    /// (`.choice('e', LootAction::Equip, "Equip immediately")`) — key.
    /// then game-id, then label. That ordering keeps the visually
    /// noisiest argument (the label string) at the end where it does
    /// not push the key/id off-screen in long chains.
    pub fn choice(mut self, key: impl Into<PromptKey>, value: T, label: impl Into<String>) -> Self {
        self.choices.push(PromptChoice::new(key, label, value));
        self
    }

    /// Disable the most recently added choice when `cond` is true.
    /// attaching `reason` for the renderer and the reducer's
    /// `Disabled` action.
    ///
    /// No-op in three cases (each chosen so partial builder chains
    /// stay safe to inspect mid-construction):
    ///
    /// 1. `cond` is false — the choice stays enabled, reason discarded.
    /// 2. No choice has been added yet — the call silently returns
    ///    `self` instead of panicking. puts `.disabled_if`
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
    /// footer (rendered as a blank row); to remove the footer entirely.
    /// drop the `.footer(...)` call from the chain.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Set the modal title rendered into the top border by
    /// [`ChoicePrompt::render_modal`] (.md ).
    ///
    /// Compact unboxed mode ignores the title — the body acts as the
    /// header in that layout — so adding `.title(...)` to a prompt that
    /// is later rendered through [`ChoicePrompt::render`] is harmless.
    /// Callers who want to switch a prompt between layouts at runtime
    /// can populate the title unconditionally and let the layout choice
    /// decide visibility.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Configure whether Esc cancels this prompt (.md ).
    ///
    /// Pass `true` for prompts that are safe to dismiss (a loot prompt.
    /// a vendor menu, an informational pause); pass `false` for prompts
    /// that must be resolved (a save-overwrite confirmation, an opening
    /// menu where Esc would strand the player). The setter form (rather
    /// than a marker `.cancellable` method) lets the builder express
    /// the dynamic case `cancellable(player.has_alt_exit)` without an
    /// `if/else` arm.
    pub fn cancellable(mut self, cancellable: bool) -> Self {
        self.cancellable = cancellable;
        self
    }

    /// Toggle the optional arrow/Enter navigation cursor
    /// (.md ).
    ///
    /// Pass `true` to seed [`ChoicePrompt::selected`] with the index of
    /// the first **enabled** choice — the cursor never starts on a row
    /// the player cannot pick, so a freshly opened prompt is always
    /// ready for an immediate Enter. When no choice is
    /// enabled (an unusual data state, but representable), the field is
    /// left as `None` rather than `Some(0)`: an unselectable cursor is
    /// worse than no cursor at all, and can short-circuit
    /// on `None` instead of guarding every move against the all-disabled
    /// case.
    ///
    /// Pass `false` to drop back to direct-key mode and clear the
    /// cursor. Round-tripping
    /// `.navigable(true).navigable(false).navigable(true)` is supported
    /// so dynamic chains like
    /// `.navigable(player.prefers_arrow_keys)` compile.
    ///
    /// # Call this *after* `.choice(...)` calls
    ///
    /// The seeding scan looks at the current `choices` list. Calling
    /// `.navigable(true)` on an empty prompt produces `selected = None`
    /// (correctly — there is nothing to highlight), and choices added
    /// later do not retroactively move the cursor onto themselves. The
    ///  builder-chain shape (`.body ….choice ….choice …
    /// .navigable(true)`) makes this ordering natural; the docs spell
    /// it out so a misordered chain reads as the author's bug, not the
    /// kit's.
    pub fn navigable(mut self, enabled: bool) -> Self {
        self.selected = if enabled {
            // Linear scan is fine here for the same reason it is fine
            // in `handle`: prompt choice lists are short.
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
    /// (.md ).
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
    /// `.vim_navigation(player.prefers_vim_keys)` compile without an
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
    /// state, and the prompt-validation pipeline intentionally
    /// reasons only about the `choices` list. If you ship Vim mode.
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
    /// method returns `false` without touching `move_up`/`move_down`.
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
            // Vim alt-keymap. `PromptKey::char` lowercases on storage.
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

    /// Run hotkey validation against the current choice
    /// list. Returns `Err(PromptError::DuplicateHotkey)` on the first
    /// collision in declaration order; `Ok` for a valid prompt
    /// (including the empty-choice case).
    ///
    /// Builder methods do not call this automatically — see the type
    /// docs for why. Call it explicitly at the seam where the prompt
    /// becomes player-visible (typically inside a `try_new`).
    pub fn validate(&self) -> Result<(), PromptError> {
        validate_choices(&self.choices)
    }

    /// Move the navigation cursor to the previous enabled choice
    /// (.md, "Up/Down selection movement when arrow mode
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
    ///   lands on the next enabled one. keeps disabled rows
    ///   visible (and their hotkeys reserved) precisely because they
    ///   are non-pickable; the cursor honours that by refusing to
    ///   highlight them.
    /// - **Wrap, don't clamp.** Going Up from the topmost enabled choice
    ///   moves to the bottommost enabled choice. Door-game menus are
    ///   short and wrapping matches what
    ///   players already expect from BBS-era door UIs; clamping would
    ///   require an extra Down press to reach the bottom of a
    ///   four-item menu, which is the exact "feels broken" failure
    ///   mode calls out by listing the cursor at all.
    /// - **All-disabled single-enabled** lists are stable: the cursor
    ///   stays where it is rather than spinning forever or being
    ///   nudged to a disabled row.
    ///
    /// Mirrors [`ChoicePrompt::move_down`] in shape so authors can pair
    /// them with their own keymap (`Up`/`k` → `move_up`, `Down`/`j` →
    /// `move_down`, see ).
    pub fn move_up(&mut self) {
        self.step_selection(Direction::Up);
    }

    /// Move the navigation cursor to the next enabled choice
    /// (.md ).
    ///
    /// Mirror of [`ChoicePrompt::move_up`]; see that doc-comment for the
    /// full skip-disabled wrap-around no-op policy. The two methods
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
    /// "movement", and because spinning the loop for `len` steps
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
        // the caller's render still has a valid cursor; 's Enter
        // handler will continue to fire on the current (still-enabled)
        // row.
    }

    /// Default prompt-line label rendered after the choice list in
    /// compact unboxed mode (.md ). Returns the
    /// 's example text `"Your choice:"`. Customisation hooks
    /// (`>`, `"Choice:"`, …) are reserved for the layout
    /// surface; carving the value out as its own method now means the
    /// rendering pipeline already reads from a single source of truth
    /// when that lands.
    fn prompt_label_text(&self) -> &str {
        "Your choice:"
    }

    /// Render the prompt as one [`String`] per visual row in the
    /// **compact unboxed** layout (.md — the bordered
    /// modal layout lands in ).
    ///
    /// Row order, top to bottom:
    ///
    /// 1. Wrapped body lines (using [`TextBlock`]'s wrap rules so the
    ///    same blank-line preservation applies).
    /// 2. A single blank row separating body and choices, but only when
    ///    both are present — empty bodies do not push a stray gap onto
    ///    a top-of-screen prompt.
    /// 3. Each choice as `"({KEY}) {label}"`, wrapped to `width`. The
    ///    marker is uppercased 's "canonical display keys
    ///    SHOULD store as uppercase when rendered". Disabled-row
    ///    formatting (the `- [M] … (full)` marker family) and the
    ///    selected-row marker layer in on top in and 5d.
    /// 4. Optional footer, preceded by one blank row when anything has
    ///    already been rendered.
    /// 5. A blank row plus the prompt label (`"Your choice:"`) when the
    ///    prompt has at least one choice. 's example
    ///    rendering ends with that label, and a choiceless prompt
    ///    (used as a pure narration block) skips it so the layout does
    ///    not invite a key the prompt cannot accept.
    ///
    /// Width `0` returns an empty `Vec` so callers with a degenerate
    /// area do not panic — forbids that. The `render`
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
    /// .
    ///
    /// The marker layer is character-based (`> ` prefix on the
    /// selected row, two-space indent on every other choice row) so
    /// 's "MUST NOT require color support for comprehension"
    /// holds — a strictly monochrome BBS terminal still shows the
    /// cursor. The reverse-video style applied in `render` is layered
    /// on top for color-capable terminals where the marker alone would
    /// feel subtle.
    ///
    /// Direct-key prompts (no `.navigable(true)`) keep `selected =
    /// None` and render exactly as before — no prefix, no reversed
    /// row — so the reference rendering and every
    /// test continue to match byte-for-byte.
    ///
    /// When the available width is narrower than the 2-cell prefix
    /// (`width < 3`), the indent is dropped and the choice rows
    /// render unprefixed; the cursor disappears but the prompt is at
    /// least readable. forbids panicking on small areas; this
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
    /// Lines past the available height are silently clipped
    /// forbids panicking on small areas, and overflow handling for the
    /// bordered modal layout lives in.
    ///
    /// In navigation mode (`.navigable(true)`) the row matching
    /// `selected` is drawn with `Modifier::REVERSED`. The character
    /// `> ` prefix added by the row-building helper is the
    /// monochrome-safe carrier of the same information; the reversed
    /// style is the color-capable layer on top. The full theme/style
    /// system lands in — today the cursor row is the only
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

    /// Render the prompt as a bordered modal (.md ) into
    /// `area` of `buf`. Returns the number of inner content rows actually
    /// written (excluding the top/bottom border rows).
    ///
    /// Layout: a Ratatui [`Block`] with `Borders::ALL` plus the optional
    /// [`ChoicePrompt::title`] in the top border. The block's `inner`
    /// rect (one cell of margin on every side) is then handed to the
    /// existing compact renderer so the wrap, blank-line, choice, and
    /// selected-row logic stays in one place — the modal is purely a
    /// frame around the same row stream.
    ///
    /// # Small-area policy
    ///
    ///  forbids panicking on tight slots. A border needs at least
    /// 2×2 to draw; anything smaller is a no-op (returns `0`). When the
    /// block's inner rect collapses to zero width or height (e.g. a 2×N
    /// or N×2 area), the border is still drawn but no content rows are
    /// written — the player sees an empty box rather than a panic. The
    /// title is also clipped by Ratatui's own block layout: an
    /// over-wide title trails into the corner glyph rather than
    /// overflowing the border.
    pub fn render_modal(&self, area: Rect, buf: &mut Buffer) -> u16 {
        // : 0×0 must be a no-op. We also bail before constructing
        // the block when either dimension is below the 2-cell minimum a
        // border needs, so Ratatui never sees an underspec'd rect.
        if area.width < 2 || area.height < 2 {
            return 0;
        }
        // Build the block from references — the title String is owned by
        // the prompt and outlives this call, so a borrowed `&str` keeps
        // us from cloning per render. `Borders::ALL` is the
        // "bordered modal" shape; future layout modes (full-screen
        // transcript) would compose differently.
        let mut block = Block::default().borders(Borders::ALL);
        if let Some(title) = self.title.as_deref() {
            block = block.title(title);
        }
        let inner = block.inner(area);
        // Render the border first so the inner content writes over an
        // already-blanked rect. `Widget::render` consumes the block; we
        // own it locally so this is fine.
        block.render(area, buf);
        // 2-cell-tall or 2-cell-wide modals have a zero inner — the box
        // renders but there is nowhere to put content. Returning 0 keeps
        // the contract ("rows actually written") honest.
        if inner.width == 0 || inner.height == 0 {
            return 0;
        }
        self.render(inner, buf)
    }
}

/// Format a single choice row in the styles.
///
/// Two visual shapes share this helper because every choice row carries
/// the same label/hint pipeline; only the leader and bracket glyphs
/// change with the choice's `enabled` flag:
///
/// - **Enabled**: `(K) Label`. Hotkey in parentheses.
///   uppercased 's display rule.
/// - **Disabled**: `- [K] Label (reason)`. Leading `- `
///   plus square brackets are the *monochrome* visual difference the
///   requires — the row stays distinguishable from enabled rows
///   even when the renderer is theme-stripped or running on
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
/// annotations like `"50g"` or `"0/26"` sit
/// alongside the label without a separate column, regardless of
/// enabled state — disabled rows still benefit from the gold/capacity
/// hint sitting between the label and the trailing `(reason)`.
fn format_choice_row<T>(choice: &PromptChoice<T>) -> String {
    // Lead with the disabled marker only when the row is
    // actually disabled — keeping the enabled path's first character a
    // `(` so the existing reference rendering still passes
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
            // 's reference example `- [M] Mana potions...
            // (full)` exactly; using two spaces here would visibly
            // drift the reason out of column alignment with the
            // and break authors who paste the example into a manifest.
            row.push_str(" (");
            row.push_str(reason);
            row.push(')');
        }
    }
    row
}

/// Pick the display glyph for a [`PromptKey`] inside the `(K)` marker.
/// Char keys uppercase the stored lowercase form ('s display
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
    /// [`PromptAction`] outcome (.md ).
    ///
    /// This is the **direct-key reducer entry point** for `ChoicePrompt`.
    ///  only wires the dispatch shape: every input currently maps
    /// to [`PromptAction::None`]. The substantive arms — direct hotkey
    /// matching, disabled-choice routing, Esc cancellation, the
    /// non-selection ignore branch — land in and 4. The
    /// signature is fixed here so downstream code (the optional
    /// [`crate::screen::Screen`] adapter, `ConfirmPrompt`, the docs
    /// examples) can compile against the eventual API immediately.
    ///
    /// # Why `&self`, not `&mut self`?
    ///
    /// Direct-key prompts have no internal state to mutate — selection
    /// is whatever the player just pressed, and the reducer's job is to
    /// classify that press, not remember it. The arrow/Enter navigation
    /// cursor lives in [`ChoicePrompt::selected`] ; the Up/Down
    /// movement and Enter-confirm reducers that need to mutate it land
    /// as separate `&mut self` methods in, so this `&self`
    /// method stays cheap to call (e.g. from a render path that previews
    /// "would this key select something?").
    ///
    /// # `T: Clone`
    ///
    /// `PromptAction::Selected(T)` and `PromptAction::Disabled { id,.. }`
    /// hand the player a copy of the choice's stable id. Real games use
    /// small `Copy` enums there, so the `Clone` bound is effectively
    /// free; constraining it on the impl block (rather than on every
    /// method) keeps `ChoicePrompt::new`/`body`/`choice`/etc. usable
    /// with non-`Clone` `T` during construction-only flows like tests
    /// that introspect the data without ever calling `handle`.
    pub fn handle(&self, input: Input) -> PromptAction<T> {
        // Translate the raw runtime event into the prompt's narrower
        // vocabulary first. Anything that is not a printable hotkey.
        // Enter, or Esc collapses to `None` here (resize, arrows.
        // backspace, Ctrl combos, Unknown) —.md 's "resize
        // ignored by prompt selection logic" falls out of this single
        // gate without per-arm special cases.
        let Some(key) = PromptKey::from_input(input) else {
            return PromptAction::None;
        };

        // : Esc cancellation, opt-in via `.cancellable(true)`.
        // Checked before the direct-hotkey scan so a choice that
        // explicitly bound `PromptKey::Esc` (exotic but legal at the
        // data level — see `validate_choices_treats_enter_and_esc_...`)
        // does not accidentally short-circuit a configured cancel.
        // When cancellation is off, Esc falls through to the trailing
        // `None` so the prompt stays unchanged.
        if key == PromptKey::Esc && self.cancellable {
            return PromptAction::Cancelled;
        }

        // : Enter confirms the highlighted choice in navigation
        // mode. `selected` is only ever `Some` when `.navigable(true)`
        // has seeded a cursor, so the `if let` here doubles
        // as the "is this a navigation-mode prompt?" gate — direct-key
        // prompts keep the legacy "Enter is a no-op" behaviour because
        // their `selected` is `None`. maintain the invariant
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

        // Direct hotkey path. Only `Char` keys participate in
        // direct-key matching here; `Enter` and `Esc` were already
        // dispatched above ( cancellation, navigation
        // confirm) or fell through as no-ops. Choices declared with
        // `PromptKey::Enter`/`PromptKey::Esc` are exotic data-level
        // configurations that those earlier branches address before
        // we ever reach the hotkey scan.
        if let PromptKey::Char(_) = key {
            // Linear scan — choice lists are short ( ~5
            // typical) and `validate` already enforces uniqueness, so
            // the first match is unambiguous. Keys are pre-normalised
            // to lowercase on both sides (`PromptKey::char` on storage.
            // `PromptKey::from_input` on input), so equality is the
            // entire comparison — no per-call `to_ascii_lowercase`.
            //
            // : a disabled choice still owns its hotkey
            //
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
/// (.md ).
///
/// Generic over the game's stable choice id `T` so the kit stays
/// decoupled from any particular inventory/shop/dialog vocabulary
/// see [`PromptChoice`] for why stable ids beat returning labels.
///
/// # Variants and when each is produced
///
/// - [`PromptAction::None`] — the input was not prompt-relevant or was
///   one of the inputs the says to ignore (resize, unknown keys.
///   modifier-only events). The caller SHOULD render the prompt
///   unchanged. `None` covers the rules:
///   - "`Resize` input MUST NOT select a choice."
///   - "Unknown keys SHOULD produce `None`."
/// - [`PromptAction::Selected`] — the player pressed an enabled
///   choice's hotkey (or pressed Enter on the highlighted choice in the
///   navigation mode). The wrapped `T` is the stable id of that choice.
/// - [`PromptAction::Disabled`] — the player pressed a hotkey bound to
///   a *disabled* choice. requires that disabled choices
///   reserve their hotkey; rather than swallowing the press silently.
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
///   so the game can stage a `ConfirmPrompt` and decide what
///   to do on yes/no. Reserving this variant at 3a, even though only
///   `ConfirmPrompt` will produce it, keeps the public API stable
///   across the rest of v1.1.
///
/// # Why a flat enum, not nested types?
///
/// Game match arms read more naturally against a flat enum
/// (`PromptAction::Selected(id) =>...`) than against a tree (e.g.
/// `Outcome::Choice(ChoiceOutcome::Selected(id))`). Equality and
/// `Debug` derives stay cheap, and the variant set is the 's
/// vocabulary — adding to it is a -level change.
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
    /// Surfaced (not swallowed) so the game can show feedback
    /// 's "disabled rows still reserve their hotkey".
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
    /// reducer (see `ConfirmPrompt`); `ChoicePrompt::handle`
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

/// Stable id for the two confirmation choices a [`ConfirmPrompt`]
/// surfaces to the player. The wrapper type exists rather than reusing
/// a `bool` so [`ChoicePrompt`]'s `T` parameter stays expressive in
/// `Debug`/`PartialEq` — a printed `Selected(Yes)` reads more naturally
/// in test failure output than `Selected(true)`, and refactoring later
/// to add new outcomes (e.g. `Always`) is a single-enum change rather
/// than a public-API rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfirmAction {
    /// Player accepted the action (typically via `(Y)`).
    Yes,
    /// Player declined the action (typically via `(N)`).
    No,
}

/// Outcome of sending one [`Input`] through a [`ConfirmPrompt`]
/// (.md "Return typed yes/no/cancel outcomes").
///
/// A flat enum mirrors [`PromptAction`]'s shape so confirmation match
/// arms read symmetrically with normal choice-prompt match arms — a
/// game routing both kinds of prompt through the same `Screen` does
/// not have to translate vocabulary between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfirmOutcome {
    /// Input was not confirmation-relevant (resize, unknown key.
    /// Enter without a configured default, etc.). The caller should
    /// re-render the prompt unchanged.
    None,
    /// Player chose [`ConfirmAction::Yes`] (or pressed Enter when
    /// [`ConfirmPrompt::default_yes`] was configured).
    Yes,
    /// Player chose [`ConfirmAction::No`] (or pressed Enter when
    /// [`ConfirmPrompt::default_no`] was configured).
    No,
    /// Player cancelled — Esc on a cancellable prompt. Distinct from
    /// `No`: calls out yes/no/cancel as three outcomes
    /// because "no" is a deliberate decline while "cancel" means
    /// "I changed my mind about being asked". Games can collapse the
    /// two when they want to.
    Cancelled,
}

/// Two-way confirmation prompt with optional Esc-cancel and optional
/// Enter-default behaviour (.md ).
///
/// `ConfirmPrompt` is a thin wrapper around [`ChoicePrompt`] that
/// fixes the choice list to the canonical `(Y)es (N)o` pair, layers
/// an Enter-default policy on top, and projects the underlying
/// [`PromptAction`] vocabulary down to the smaller [`ConfirmOutcome`]
/// surface. Reusing `ChoicePrompt` (rather than reimplementing the
/// renderer and reducer) keeps the compact-unboxed and
/// bordered-modal layouts free of duplicate code paths and means every
/// existing `ChoicePrompt` test (wrap, blank-line preservation.
/// disabled glyphs, theme mapping) carries through to confirmations
/// for free.
///
/// The third "cancel" outcome is represented as Esc
/// rather than a third visible choice — door-game conventions treat
/// Esc as "back out" and the test surface
/// (`cancellable_confirm_returns_cancelled_on_esc`) exercises that
/// path directly. Authors who want a literal `(C)ancel` row can
/// drop down to `ChoicePrompt` and route the third value themselves;
/// adding a builder for it here would invite scope drift past the
/// 's "two- or three-way" wording.
///
/// # Why default cancellable, opposite of `ChoicePrompt`?
///
/// `ChoicePrompt::cancellable` defaults to `false` because a generic
/// hotkey prompt may be load-bearing (a save-overwrite confirmation
/// the game wants to force a resolution on). `ConfirmPrompt` is, by
/// definition, the "are you sure?" beat — Esc-as-back-out is the
/// dominant convention for that beat in BBS-era door UIs and matches
/// 's explicit "yes/no/cancel" framing. Authors who need a
/// non-cancellable confirmation (rare, but representable — e.g. a
/// terminal-quit guard) opt out via `.cancellable(false)`.
///
/// # Why no internal cursor arrow navigation?
///
///  defines `ConfirmPrompt` in direct-hotkey terms (`Y`/`N`)
/// plus an Enter default. Adding the optional [`ChoicePrompt::navigable`]
/// cursor on top would multiply the test matrix for negligible UX
/// gain on a two-row prompt. Authors who genuinely need an arrow-mode
/// confirmation can compose `ChoicePrompt<ConfirmAction>` themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    /// Underlying generic prompt — owns the body, cancellable flag.
    /// optional title/footer, and the actual reducer logic. Public
    /// reads are kept package-internal: callers go through the
    /// `ConfirmPrompt` API so the yes/no choice list stays an
    /// invariant of the type (you cannot `.choice(...)` a third row
    /// onto a `ConfirmPrompt` from outside this module).
    inner: ChoicePrompt<ConfirmAction>,
    /// Which confirmation Enter selects when no other key has been
    /// pressed. `None` → Enter is a no-op (`ConfirmOutcome::None`).
    /// matching direct-key `ChoicePrompt` behaviour. lists
    /// the Enter-default as opt-in precisely because some prompts
    /// must not have an "easy yes" path — a delete-save confirmation
    /// should require an explicit `(Y)` press.
    default: Option<ConfirmAction>,
}

impl ConfirmPrompt {
    /// Build a yes/no confirmation with `question` as the body line.
    ///
    /// Defaults: `Y` → Yes, `N` → No, no Enter default.
    /// **cancellable on** (Esc returns `Cancelled`). Customise via
    /// the chained builder methods. Use multiple `.body(...)` calls if
    /// the question spans paragraphs — `ConfirmPrompt::body` mirrors
    /// `ChoicePrompt::body` so wrap and blank-line preservation behave
    /// identically.
    pub fn new(question: impl Into<String>) -> Self {
        // Keys are stored lowercase by `PromptKey::char`, so passing
        // 'y'/'n' here picks up `from_input`'s case-insensitive match
        // for free — `Y` and `y` both reach the same arm in `handle`.
        let inner = ChoicePrompt::new()
            .body(question)
            .choice('y', ConfirmAction::Yes, "Yes")
            .choice('n', ConfirmAction::No, "No")
            .cancellable(true);
        Self {
            inner,
            default: None,
        }
    }

    /// Append another body paragraph (delegates to
    /// [`ChoicePrompt::body`]). Useful for the
    /// "dangerous-action copy" requirement — a stark second line like
    /// `"This cannot be undone."` reads better as its own paragraph
    /// than concatenated to the question.
    pub fn body(mut self, line: impl Into<String>) -> Self {
        self.inner = self.inner.body(line);
        self
    }

    /// Replace the default `"Yes"` label on the affirmative choice
    /// (e.g. `"Delete the save"`). The hotkey stays `Y` — relabelling
    /// does not change the binding, which keeps muscle memory honest
    /// across games.
    pub fn yes_label(mut self, label: impl Into<String>) -> Self {
        // Index 0 is the `Yes` row by construction in `new`. The
        // pattern match keeps us from blowing up if a future refactor
        // changes the order; we silently ignore rather than panic so
        // builder chains stay safe to inspect mid-construction (see
        // the rationale on `ChoicePrompt::disabled_if`).
        if let Some(choice) = self.inner.choices.get_mut(0) {
            choice.label = label.into();
        }
        self
    }

    /// Replace the default `"No"` label on the negative choice (e.g.
    /// `"Keep the save"`). See [`ConfirmPrompt::yes_label`] for the
    /// rationale on why the hotkey stays `N`.
    pub fn no_label(mut self, label: impl Into<String>) -> Self {
        if let Some(choice) = self.inner.choices.get_mut(1) {
            choice.label = label.into();
        }
        self
    }

    /// Configure the Enter-default outcome. Pass `Some(action)` to
    /// make Enter return that confirmation, `None` to clear it. The
    /// setter form (rather than two separate marker methods) lets
    /// authors express dynamic policies like
    /// `default(if dangerous { None } else { Some(ConfirmAction::Yes) })`
    /// without an `if`/`else` branching the chain.
    pub fn default(mut self, action: Option<ConfirmAction>) -> Self {
        self.default = action;
        self
    }

    /// Convenience: make Enter confirm Yes. Equivalent to
    /// `.default(Some(ConfirmAction::Yes))`. Use for low-stakes prompts
    /// where the "obvious" answer is to proceed.
    pub fn default_yes(self) -> Self {
        self.default(Some(ConfirmAction::Yes))
    }

    /// Convenience: make Enter confirm No. Equivalent to
    /// `.default(Some(ConfirmAction::No))`. Use for high-stakes prompts
    /// where a stray Enter must NOT proceed (delete confirmations.
    /// destructive game actions). 's "dangerous-action copy"
    /// guidance pairs naturally with this — copy says "are you sure?"
    /// while the default-no policy makes the safe answer the cheap one.
    pub fn default_no(self) -> Self {
        self.default(Some(ConfirmAction::No))
    }

    /// Toggle Esc cancellation. Defaults to `true` — see the type doc
    /// for why `ConfirmPrompt` inverts `ChoicePrompt::cancellable`'s
    /// default.
    pub fn cancellable(mut self, cancellable: bool) -> Self {
        self.inner = self.inner.cancellable(cancellable);
        self
    }

    /// Replace the footer line (delegates to [`ChoicePrompt::footer`]).
    /// Common use: `"Press Enter to confirm."` when an Enter default is
    /// set, so the cue is part of the rendered prompt rather than
    /// implicit.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.inner = self.inner.footer(footer);
        self
    }

    /// Set the modal title (delegates to [`ChoicePrompt::title`]).
    /// Only the bordered modal layout reads it; compact unboxed mode
    /// ignores titles, mirroring `ChoicePrompt`.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.inner = self.inner.title(title);
        self
    }

    /// Borrow the underlying [`ChoicePrompt`] for renderer access in
    /// custom screens. Crate-private because the public surface is
    /// `render`/`render_modal`; this hook exists for the optional
    /// `PromptScreen` adapter without committing to a stable
    /// public projection of the wrapped prompt.
    #[allow(dead_code)]
    pub(crate) fn inner(&self) -> &ChoicePrompt<ConfirmAction> {
        &self.inner
    }

    /// Send one [`Input`] through the prompt and return a typed
    /// [`ConfirmOutcome`].
    ///
    /// Resolution order:
    ///
    /// 1. **Enter with a configured default** → returns the default's
    ///    matching outcome. Checked first so the Enter default beats
    ///    `ChoicePrompt::handle`'s own Enter handling (which would be
    ///    a no-op here because we never enable navigation mode on the
    ///    inner prompt).
    /// 2. Everything else flows through [`ChoicePrompt::handle`] and
    ///    is projected from [`PromptAction`] to [`ConfirmOutcome`].
    ///    `Disabled` and `ConfirmRequested` cannot occur in practice
    ///    (we never disable a row, never declare confirm-requested
    ///    semantics) but we collapse them to `None` defensively rather
    ///    than panicking, in case a future refactor adds a disabled
    ///    "confirm with override" branch.
    pub fn handle(&self, input: Input) -> ConfirmOutcome {
        if matches!(input, Input::Enter) {
            if let Some(default) = self.default {
                return match default {
                    ConfirmAction::Yes => ConfirmOutcome::Yes,
                    ConfirmAction::No => ConfirmOutcome::No,
                };
            }
        }
        match self.inner.handle(input) {
            PromptAction::Selected(ConfirmAction::Yes) => ConfirmOutcome::Yes,
            PromptAction::Selected(ConfirmAction::No) => ConfirmOutcome::No,
            PromptAction::Cancelled => ConfirmOutcome::Cancelled,
            PromptAction::None
            | PromptAction::Disabled { .. }
            | PromptAction::ConfirmRequested(_) => ConfirmOutcome::None,
        }
    }

    /// Render the confirmation in compact unboxed mode (delegates to
    /// [`ChoicePrompt::render`]). Returns the number of rows written.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> u16 {
        self.inner.render(area, buf)
    }

    /// Render the confirmation as a bordered modal (delegates to
    /// [`ChoicePrompt::render_modal`]). Returns the number of inner
    /// content rows written, excluding the border.
    pub fn render_modal(&self, area: Rect, buf: &mut Buffer) -> u16 {
        self.inner.render_modal(area, buf)
    }
}

/// Outcome of sending one [`Input`] through an [`AnyKeyPrompt`]
/// (.md ).
///
/// Modeled as a flat enum mirroring [`PromptAction`] [`ConfirmOutcome`]
/// so a `Screen` routing through several prompt kinds reads
/// symmetrically: every prompt's `handle` returns `…::None` when the
/// input was not prompt-relevant and a meaningful variant otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnyKeyOutcome {
    /// Input was ignored — resize events and [`Input::Unknown`] (which
    /// covers modifier-only release-only events the runtime decided
    /// not to translate). The caller should re-render unchanged.
    None,
    /// Player pressed a meaningful key. says "any meaningful
    /// keypress" completes the pause; the prompt makes no distinction
    /// between which key, because the contract is *acknowledgement*.
    /// not *choice*. Games that want to know which key was pressed
    /// should use [`ChoicePrompt`] instead.
    Completed,
}

/// "Press any key to continue" pause prompt (.md ).
///
/// `AnyKeyPrompt` is the smallest prompt in the kit: it shows a body of
/// post-event narration (loot description, vendor monologue, scripted
/// flavor text) and waits for the player to acknowledge. The reducer
/// has exactly two states — pending and completed — and a single rule:
/// any meaningful keypress completes; resize and [`Input::Unknown`] do
/// not.
///
/// # Why a dedicated type instead of a `ChoicePrompt` with no choices?
///
/// `ChoicePrompt` validates that at least one hotkey is bound (so it
/// never silently swallows player input) and renders a hotkey legend.
/// Both behaviours are wrong for an any-key pause: there are no
/// hotkeys, and the legend would be a lie. Modelling the pause as its
/// own type keeps the two prompts honest about what they promise.
///
/// # Why ignore resize?
///
///  calls it out explicitly. The intent is that a player
/// who resizes their terminal mid-pause sees the body re-flow rather
/// than the screen instantly disappearing — resize is a layout event.
/// not a player decision. [`Input::Unknown`] is collapsed for the same
/// reason: a stray release event, a `Ctrl` chord the runtime didn't
/// translate, or a focus change MUST NOT count as acknowledgement.
///
/// Rendering lives in (`render` `render_modal` follow the
/// same shape as [`ChoicePrompt`]); this task implements only the
/// reducer surface so the input contract is testable without
/// any layout dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnyKeyPrompt {
    /// Body lines (post-event narration). Empty entries are
    /// "explicit blank line" paragraph breaks — see [`TextBlock`] for
    /// the wrapping contract will apply at render time.
    body: Vec<String>,
    /// Footer cue. `None` means the renderer should fall back to the
    ///  default `"Press any key to continue..."`. The override
    /// exists so authors can localize the cue or replace it with a
    /// scene-specific line ("Press any key to wake up.") without losing
    /// the any-key semantics.
    footer: Option<String>,
    /// Optional modal title — only the bordered modal layout reads it.
    /// matching the [`ChoicePrompt`] convention.
    title: Option<String>,
}

impl AnyKeyPrompt {
    /// Build an empty pause prompt with no body and the default footer.
    /// Add narration lines with [`AnyKeyPrompt::body`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a body paragraph. Multiple calls accumulate, so authors
    /// can build up narration over several `.body(...)` chains the
    /// same way [`ChoicePrompt::body`] supports paragraph stacks.
    pub fn body(mut self, line: impl Into<String>) -> Self {
        self.body.push(line.into());
        self
    }

    /// Override the footer cue. Pass any string to replace the
    ///  default `"Press any key to continue..."`.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Set the modal title (only read by the bordered modal layout).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Body lines as supplied by the author, in order. Exposed for the
    ///  renderer and for tests; not part of the reducer
    /// contract.
    pub fn body_lines(&self) -> &[String] {
        &self.body
    }

    /// Footer cue if one was set; `None` means "use the
    /// default". Exposed for the renderer.
    pub fn footer_text(&self) -> Option<&str> {
        self.footer.as_deref()
    }

    /// Modal title if one was set. Exposed for the renderer.
    pub fn title_text(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Send one [`Input`] through the pause and return whether it
    /// completed.
    ///
    /// Per:
    /// - [`Input::Resize`] → ignored ([`AnyKeyOutcome::None`]).
    /// - [`Input::Unknown`] → ignored. The runtime emits this for
    ///   modifier-only and release events; collapsing them prevents
    ///   spurious completions on focus changes or `Ctrl`-only chords.
    /// - Every other variant → [`AnyKeyOutcome::Completed`]. says
    ///   "any meaningful keypress" and treats arrows, Enter, Esc.
    ///   Backspace, `Char`, and `Ctrl`-prefixed keys as meaningful.
    ///
    /// `&self` rather than `&mut self` because the prompt itself
    /// carries no progress state — the *caller* decides what to do
    /// when the pause completes (typically pop the prompt or transition
    /// the screen). Keeping `handle` non-mutating mirrors
    /// [`ChoicePrompt::handle`] and [`ConfirmPrompt::handle`].
    pub fn handle(&self, input: Input) -> AnyKeyOutcome {
        match input {
            Input::Resize { .. } | Input::Unknown => AnyKeyOutcome::None,
            Input::Up
            | Input::Down
            | Input::Left
            | Input::Right
            | Input::Enter
            | Input::Esc
            | Input::Backspace
            | Input::Char(_)
            | Input::Ctrl(_) => AnyKeyOutcome::Completed,
        }
    }

    /// Compute the prompt's visual rows at `width`, top-down.
    ///
    /// Row order:
    ///
    /// 1. Wrapped body lines via [`TextBlock`], so 's
    ///    blank-line preservation and hard-break-on-overlong-word rules
    ///    behave identically to [`ChoicePrompt::rendered_lines`].
    /// 2. A single blank gap between body and footer when both are
    ///    non-empty — empty bodies do not push a stray gap onto a
    ///    top-of-screen pause, mirroring the choice prompt convention.
    /// 3. The footer cue: either the author override (set via
    ///    [`AnyKeyPrompt::footer`]) or the default
    ///    `"Press any key to continue..."`. The footer always renders
    ///    a pause without an acknowledgement cue would be a UX trap.
    ///
    /// Width `0` returns an empty `Vec` so callers with a degenerate
    /// area do not panic; forbids that. The substituted default
    /// string lives behind [`DEFAULT_ANY_KEY_FOOTER`] so tests and the
    ///  text agree on a single source of truth.
    pub fn rendered_lines(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return Vec::new();
        }
        let mut rows: Vec<String> = Vec::new();

        // Body — wrap via TextBlock so blank-line preservation and the
        // hard-break-on-overlong-word fallback come along for free.
        let body = TextBlock::from_lines(self.body.iter().cloned());
        rows.extend(body.wrapped_rows(width));

        // Body→footer gap: only when both are present, matching the
        // ChoicePrompt body→choices convention.
        if !rows.is_empty() {
            rows.push(String::new());
        }

        // Footer — the author override wins; otherwise we fall back to
        // the default. Wrapping handles a localized override
        // that runs longer than the area width.
        let footer = self.footer.as_deref().unwrap_or(DEFAULT_ANY_KEY_FOOTER);
        rows.extend(TextBlock::new(footer).wrapped_rows(width));

        rows
    }

    /// Render the pause into `area` of `buf` in the compact unboxed
    /// layout (mirrors [`ChoicePrompt::render`]). Returns the number of
    /// visual rows actually written, clamped to `area.height`.
    ///
    /// Lines past the available height are silently clipped
    /// forbids panicking on small areas, and bordered-modal overflow
    /// handling lives in [`AnyKeyPrompt::render_modal`].
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if area.width == 0 || area.height == 0 {
            return 0;
        }
        let rows = self.rendered_lines(area.width);
        let drawn = rows.len().min(area.height as usize);
        let style = ratatui::style::Style::default();
        for (i, row) in rows.iter().take(drawn).enumerate() {
            let y = area.y + i as u16;
            buf.set_stringn(area.x, y, row, area.width as usize, style);
        }
        drawn as u16
    }

    /// Render the pause as a bordered modal (.md ). Returns
    /// the number of inner content rows written, excluding the border.
    ///
    /// Same small-area policy as [`ChoicePrompt::render_modal`]: areas
    /// smaller than 2×2 are a no-op, and inner rects that collapse to
    /// zero in either dimension still draw the border but write no
    /// content rows.
    pub fn render_modal(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if area.width < 2 || area.height < 2 {
            return 0;
        }
        let mut block = Block::default().borders(Borders::ALL);
        if let Some(title) = self.title.as_deref() {
            block = block.title(title);
        }
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return 0;
        }
        self.render(inner, buf)
    }
}

///  default footer cue substituted by
/// [`AnyKeyPrompt::rendered_lines`] when the author has not set an
/// override via [`AnyKeyPrompt::footer`]. Lifted to a constant so the
///  text and the rendering tests assert against the same string.
pub const DEFAULT_ANY_KEY_FOOTER: &str = "Press any key to continue...";

/// Severity tag for [`FeedbackLine`] (.md ).
///
/// The three variants are the post-action feedback shapes a door game
/// reaches for: a neutral status note, a positive confirmation that the
/// state changed in the player's favor, and a negative report that
/// something went wrong or was rejected. They map onto the
/// style roles (`Normal` `Success` `Error`) so a `Theme` override
/// re-skins the line without touching the call site.
///
/// # Monochrome contract
///
///  requires "monochrome fallbacks through prefixes, punctuation.
/// indentation, or markers". Each variant therefore carries a *distinct*
/// ASCII marker prefix via [`FeedbackKind::marker`]; that marker is what
/// communicates severity on a black/white terminal where the role's color
/// has been stripped. The role-to-color mapping is a *bonus* on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedbackKind {
    /// Neutral status — `Moved Room 7 key to inventory.`-style notes
    /// that report a state change without implying success or failure.
    Info,
    /// Positive feedback — purchase succeeded, item taken, save written.
    Success,
    /// Negative feedback — selection rejected, transaction failed.
    /// validation error. Pairs with the disabled-choice reason text the
    /// reducer returns in [`PromptAction::Disabled`].
    Error,
}

impl FeedbackKind {
    /// Monochrome-safe ASCII marker prefix.
    ///
    /// Each variant returns a distinct prefix so a black/white terminal
    /// can still tell info success error apart by shape alone:
    ///
    /// - `Info` → `""` (no marker; reads as a plain status note).
    /// - `Success` → `"+ "` (BBS convention for "added" "gained").
    /// - `Error` → `"! "` (BBS convention for warnings problems).
    ///
    /// The marker pool is intentionally small and pure-ASCII so it
    /// renders identically under any locale or codepage 's
    /// "labels MUST NOT include raw terminal control sequences" rule.
    pub const fn marker(self) -> &'static str {
        match self {
            FeedbackKind::Info => "",
            FeedbackKind::Success => "+ ",
            FeedbackKind::Error => "! ",
        }
    }

    /// Semantic [`StyleRole`] this kind paints under via [`Theme`].
    ///
    /// `Info` rides on `StyleRole::Normal` (the terminal's default
    /// foreground) so a neutral note doesn't fight the surrounding body
    /// text for attention. `Success` and `Error` map to their
    /// -listed roles, which the default theme already renders
    /// with a color *and* a modifier — keeping color-stripping safe.
    pub const fn style_role(self) -> StyleRole {
        match self {
            FeedbackKind::Info => StyleRole::Normal,
            FeedbackKind::Success => StyleRole::Success,
            FeedbackKind::Error => StyleRole::Error,
        }
    }
}

/// Single-line post-action feedback rendered alongside a prompt
/// (.md example: `Moved Room 7 key to inventory.`).
///
/// `FeedbackLine` is the prompt-flavored cousin of
/// [`crate::widgets::MessageLine`]. The widget version targets the
/// always-on status row at the bottom of a screen and uses ad-hoc
/// `Color`/`Modifier` pairs; this version lives inside the prompt
/// module, derives its visual style from a [`Theme`] via
/// [`StyleRole`], and renders directly into a [`Buffer`] so it composes
/// with [`ChoicePrompt::render`] [`AnyKeyPrompt::render`] without
/// going through `Frame::render_widget`.
///
/// # Why a separate type for prompts?
///
/// Prompt feedback follows the contract: a finite set of
/// severity tags, monochrome-readable markers, and theme-overridable
/// color. Threading those constraints through the freer-form
/// [`crate::widgets::MessageLine`] would either bloat that widget's
/// API or weaken the prompt-side guarantees. Keeping them separate
/// also lets a future extension push prompt feedback through
/// a `Theme` without rewriting screens that still use `MessageLine`
/// for their persistent status row.
///
/// # Rendering shape
///
/// The line writes a single visual row of `marker + text`, styled with
/// the [`FeedbackKind::style_role`] looked up through a [`Theme`].
/// Text longer than `area.width` is right-truncated cell-by-cell — a
/// status line never wraps, because a wrapped feedback line shifts the
/// rest of the layout under the prompt and is harder to scan.
///
/// # What this type intentionally does NOT do
///
/// - It does not own a stack of past messages. Authors store the
///   current feedback in their game state (typically `Option<FeedbackLine>`
///   on the screen) and overwrite it on the next action.
/// - It does not animate, fade, or expire. keeps prompts
///   side-effect-free; lifecycle is the screen's job.
/// - It does not split into multiple rows. Use [`TextBlock`] when a
///   message needs paragraphing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackLine {
    kind: FeedbackKind,
    text: String,
}

impl FeedbackLine {
    /// Build a neutral status note — `FeedbackKind::Info`.
    ///
    /// Use this for state-change reports that are neither a win nor a
    /// failure: `"Moved Room 7 key to inventory."`.
    /// `"You sit down at the bar."`, etc. The renderer prepends no
    /// marker, so the text reads exactly as the author wrote it.
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: FeedbackKind::Info,
            text: text.into(),
        }
    }

    /// Build a positive feedback line — `FeedbackKind::Success`.
    ///
    /// Renders with a `"+ "` marker. Reach for this when the player's
    /// action moved the world in their favor: a purchase, a successful
    /// save, an item taken. The marker keeps the win readable on a
    /// monochrome terminal without relying on the green color.
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            kind: FeedbackKind::Success,
            text: text.into(),
        }
    }

    /// Build a negative feedback line — `FeedbackKind::Error`.
    ///
    /// Renders with a `"! "` marker. Use this for rejected actions.
    /// validation problems, or the disabled-choice reason returned by
    /// [`PromptAction::Disabled`]. The marker survives color stripping
    /// so a BBS terminal still flags the line as a problem.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: FeedbackKind::Error,
            text: text.into(),
        }
    }

    /// Severity tag.
    pub fn kind(&self) -> FeedbackKind {
        self.kind
    }

    /// Author-supplied message body, without the kind's marker.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Final visual string: `marker + text`. Useful in tests and for
    /// callers (e.g. screen-level transcripts) that want the rendered
    /// shape without a `Buffer`. Stays monochrome-safe — the
    /// marker is the only carrier of severity here.
    pub fn rendered_text(&self) -> String {
        let marker = self.kind.marker();
        let mut out = String::with_capacity(marker.len() + self.text.len());
        out.push_str(marker);
        out.push_str(&self.text);
        out
    }

    /// Render the line into the top row of `area` using the default
    /// [`Theme`]. Returns `1` if a row was written, `0` if the area was
    /// too small (zero width or height).
    ///
    /// Most callers want this overload — it matches the
    /// "default theme MUST be readable on black/white terminals"
    /// promise without forcing the screen to plumb a `Theme` through
    /// every render call. Use [`FeedbackLine::render_with_theme`] when
    /// the game has its own theme.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> u16 {
        self.render_with_theme(area, buf, &Theme::default())
    }

    /// Render the line into the top row of `area` using the supplied
    /// [`Theme`]. Returns `1` if a row was written, `0` if the area
    /// was too small.
    ///
    /// Text that doesn't fit in `area.width` is right-truncated by
    /// `Buffer::set_stringn` — 's "MUST NOT panic on small
    /// areas" rule. Extra rows beneath the first are left untouched
    /// so the caller can stack a feedback line over a body without
    /// the line clobbering the body's first row.
    pub fn render_with_theme(&self, area: Rect, buf: &mut Buffer, theme: &Theme) -> u16 {
        if area.width == 0 || area.height == 0 {
            return 0;
        }
        let style = theme.style(self.kind.style_role());
        let line = self.rendered_text();
        // `set_stringn` truncates if `line.len` exceeds `area.width`.
        // which is the contract a single-row status indicator wants
        // wrapping would push the rest of the layout out from under
        // the prompt and break the example's vertical shape.
        buf.set_stringn(area.x, area.y, &line, area.width as usize, style);
        1
    }
}

/// A reusable narration status fragment rendered above a prompt
/// (.md ).
///
/// `TextBlock` is the data shape behind a prompt body: a sequence of
/// logical lines that the renderer wraps to the supplied area's width
/// while preserving the author's explicit blank lines as visual gaps.
/// It carries an optional [`StyleRole`] default so a status block can
/// declare itself as `Success` `Error` `Muted` once instead of
/// per-line; the role-to-`Style` mapping itself lives in and is
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
///   blank visual row, regardless of width. This is what 's
///   "preserve explicit blank lines" requires, and it matches how
///   author-written narration uses `\n\n` as a paragraph break.
/// - A width of `0` collapses to a no-op render (no panic). This keeps
///   the "MUST NOT panic on small areas" guarantee true even
///   when the layout caller hands the body a zero-width slot.
///
/// # What this type intentionally does NOT do
///
/// - It does not carry per-span styling. Inline emphasis is the job of
///   [`StyleRole`] applied at the choice/feedback level.
///   not of in-line markup inside a body string.
/// - It does not own scroll state. lists "optional scrolling
///   for long bodies" as future scope; the v1.1 prompt rendering MUST
///   support compact unboxed and bordered modal modes ( 5e).
///   both of which clip rather than scroll.
/// - It does not perform side effects. Rendering only writes cells to
///   a `Buffer`; the reducer game state is untouched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextBlock {
    /// Logical lines, in order. An empty `String` means "render a
    /// blank visual row" — see the wrapping rules in the type doc.
    lines: Vec<String>,
    /// Optional default style role for the whole block. The
    /// theme layer maps this to a `ratatui::style::Style`; until then
    /// the renderer ignores it (no styling is still -legal).
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
        // `split('\n')` (rather than `lines`) preserves a trailing
        // empty line — `"a\n"` becomes `["a", ""]`. 's
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
    /// The theme layer in turns this into a Ratatui `Style`;
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
    /// the contract the bordered modal renderer in relies on
    /// when the caller area is too small to host a body column.
    pub fn wrapped_rows(&self, width: u16) -> Vec<String> {
        if width == 0 {
            return Vec::new();
        }
        let width = width as usize;
        let mut out: Vec<String> = Vec::with_capacity(self.lines.len());
        for line in &self.lines {
            if line.is_empty() {
                // : blank lines are preserved as one row.
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
    /// Lines past the available height are silently clipped;
    /// forbids panicking on small areas, and a body that overflows its
    /// slot is a layout decision for the caller (compact unboxed and
    /// bordered modes both clip — see ). The unused `style`
    /// field is reserved for the theme integration; rendering
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
/// area — 's "truncation or wrapping at terminal width"
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
            // 's "no silent drop" promise; URLs and long IDs
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
    // what 's "preserve explicit blank lines" calls for, and
    // it keeps `wrapped_rows` returning at least one row per logical
    // line.
    out.push(current);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_lowercases_ascii_letters() {
        // .md: "Character hotkeys MUST match
        // case-insensitively by default."
        assert_eq!(PromptKey::char('e'), PromptKey::Char('e'));
        assert_eq!(PromptKey::char('E'), PromptKey::Char('e'));
        assert_eq!(PromptKey::char('e'), PromptKey::char('E'));
    }

    #[test]
    fn char_preserves_non_letter_ascii_and_digits() {
        // Digits and punctuation have no case; passthrough verbatim so
        // numeric hotkeys (.md ) work as authored.
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
        // is enough for the reducer to match either input.
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
        // .md: "resize ignored by prompt selection logic".
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
        // The direct-key reducer rejects these; the optional
        // navigation reducer consumes arrows separately and
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
        // : `enabled` defaults to true; metadata fields stay
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
        // : disabled choices carry a reason that the renderer
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
        // Hints (cost, capacity) are independent of disabled state
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
        // : "Duplicate active hotkeys in one prompt MUST be
        // rejected or produce a clear construction error." The error
        // points at both labels so the failing test (or the panicking
        // builder, in ) names the exact culprits.
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
        // `'E'` and `'e'` collapse to the same `PromptKey::Char('e')`.
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
        // : "Disabled choices still reserve their hotkey by
        // default." A disabled row paired with an enabled row sharing
        // the same key is exactly the surprise the forbids — the
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
        // level. Two `Enter`-keyed choices still collide — same rule.
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
        // Mirrors the Giant Spider example. Asserts the
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
        //  wandering-monk example uses two `.body(...)` calls to
        // express a two-paragraph body. Each call appends one entry; the
        // renderer is responsible for blank-line spacing.
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
        // `.disabled_if(!can_buy,...)` flips it to disabled when the
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
        // the expectation, since the chain reads "disable IF
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
        // "Last writer wins" — a later `.disabled_if(true,...)` on the
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
        // The opt-in `validate` shim runs hotkey validation
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
        // `Default` is a convenience for derive macros generic code.
        // It MUST behave identically to `new` so callers can pick
        // either without surprises.
        let a: ChoicePrompt<LootAction> = ChoicePrompt::new();
        let b: ChoicePrompt<LootAction> = ChoicePrompt::default();
        assert_eq!(a, b);
    }

    #[test]
    fn prompt_action_variants_construct_and_compare() {
        // : the action enum carries None Selected Disabled
        // / Cancelled ConfirmRequested. Verify each variant builds
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
        // forms are valid ( will exercise the `Some` path).
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
        // .md: pressing an enabled choice's hotkey returns
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
        // : "Character hotkeys MUST match case-insensitively
        // by default." This is the test the checklist names explicitly
        // for — uppercase input must produce the same Selected
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
        // disabled-routing will diverge for the bound case.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");

        assert_eq!(prompt.handle(Input::Char('z')), PromptAction::None);
        assert_eq!(prompt.handle(Input::Char('1')), PromptAction::None);
    }

    #[test]
    fn handle_ignores_non_prompt_inputs() {
        // .md: resize MUST NOT select a choice. Arrows.
        // backspace, Ctrl combos, and Unknown collapse through
        // `PromptKey::from_input -> None`, so the reducer reports `None`
        // without touching the choice list. will add an opt-in
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
        // :.md requires resize and unknown inputs
        // be inert — no selection invented, and crucially no latent
        // state change that would corrupt a follow-up press. The
        // reducer's `&self` signature already proves immutability at
        // the type level, but this test pins the *behavioural*
        // contract: after an arbitrary stream of ignored inputs, the
        // very next bound hotkey still resolves to the right enabled
        // choice. When adds a `selected_index` cursor, this
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
        // : pressing a disabled choice's hotkey MUST NOT silently
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
        //  makes `disabled_reason` optional. A choice flagged
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
            title: None,
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
        // Enter is the navigation-mode confirm key. On a
        // direct-key prompt (no `.navigable(true)`) the cursor is `None`.
        // so Enter must not invent a selection — otherwise 's
        // hotkey-only loot prompts would silently route the first row
        // through `Selected` whenever the player tapped Return.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new().choice('e', LootAction::Equip, "Equip");

        assert_eq!(prompt.selected, None);
        assert_eq!(prompt.handle(Input::Enter), PromptAction::None);
    }

    #[test]
    fn handle_esc_returns_none_when_cancellation_disabled() {
        // .md: Esc support is "when configured". A prompt
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
        // : opt-in via `.cancellable(true)`. Esc now produces
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
        // `.cancellable(player.can_back_out)` compile. Both arms must
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
        // Default-off is load-bearing: it is the contract that
        // Esc support is opt-in. If a future refactor flips the default
        // to `true`, every existing prompt becomes silently dismissible.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new();
        assert!(!prompt.cancellable);
    }

    #[test]
    fn navigable_defaults_to_none_for_direct_key_prompts() {
        // : arrow/Enter is opt-in. The default `new` flow
        // every example so far — must leave `selected = None`
        // so direct-key prompts never accidentally render a cursor.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take");
        assert_eq!(prompt.selected, None);
    }

    #[test]
    fn navigable_seeds_cursor_on_first_enabled_choice() {
        //  contract: enabling arrow mode highlights the first
        // enabled row so a fresh prompt is immediately Enter-ready.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('e', LootAction::Equip, "Equip")
            .choice('t', LootAction::Take, "Take")
            .navigable(true);
        assert_eq!(prompt.selected, Some(0));
    }

    #[test]
    fn navigable_skips_leading_disabled_choices_when_seeding() {
        // The cursor must never start on a row the player cannot pick.
        // even when the data lists a disabled choice first. Without this
        // skip, an Enter press on a freshly opened arrow-mode prompt
        // would route through `Disabled` (or worse, through 's
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
        // before any `.choice(...)` call must not produce `Some(0)`
        // there is no row at index 0 to highlight, and a stale cursor
        // would be a panic vector for 's Up/Down reducer.
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
        // Dynamic chains like `.navigable(player.prefers_arrows)` must
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
        // player can still tap a hotkey instead of arrow-stepping
        //  frames arrow/Enter as an *additional* affordance.
        // not a replacement. Enter on the seeded cursor (index 0 =
        // Equip) routes through 's confirm path.
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
        // : Enter on a navigation-mode prompt confirms the row
        // the cursor currently sits on. Stepping Down once moves the
        // cursor from the seeded index 0 (`Equip`) to index 1 (`Take`).
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
        // The cursor seed and Up/Down guarantee the
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
        // cursor at `None` (pinned by `navigable_with_all_disabled_...`).
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
        //  leaves wrap-vs-clamp to the implementation; this
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
        //  lists `j`/`k` as opt-in. A freshly built prompt must
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
        //  alt keymap: `j` → Down, `k` → Up. Both presses must
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
        // is the canonical "ignored" input; Enter is the
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

    // --: TextBlock wrapping & blank-line preservation -------

    fn render_textblock_to_strings(block: &TextBlock, w: u16, h: u16) -> Vec<String> {
        // Drive the renderer through a real Ratatui `TestBackend` so we
        // exercise the "deterministic output under TestBackend"
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
        // quick"; "brown fox" together is 9). requires
        // wrapping at the available width.
        let block = TextBlock::new("the quick brown fox");
        let rows = block.wrapped_rows(10);
        assert_eq!(rows, vec!["the quick".to_string(), "brown fox".to_string()]);
    }

    #[test]
    fn textblock_preserves_explicit_blank_lines() {
        // : "Preserve explicit blank lines." Two paragraphs
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
        //  forbids silently dropping content. A word longer
        // than the width must hard-break rather than overflow or
        // disappear.
        let block = TextBlock::new("antidisestablishmentarianism");
        let rows = block.wrapped_rows(10);
        // 28 chars 10 = 3 rows: "antidisest", "ablishment", "arianism"
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].chars().count(), 10);
        assert_eq!(rows[1].chars().count(), 10);
        assert_eq!(rows[2], "arianism");
        // Concatenating the rows MUST reproduce the original word
        // "no silent drop" is the load-bearing invariant.
        let joined: String = rows.join("");
        assert_eq!(joined, "antidisestablishmentarianism");
    }

    #[test]
    fn textblock_zero_width_returns_no_rows_and_does_not_panic() {
        // : prompt rendering MUST NOT panic on small areas.
        let block = TextBlock::new("anything");
        assert!(block.wrapped_rows(0).is_empty());
    }

    #[test]
    fn textblock_renders_under_test_backend_with_wrapped_rows() {
        // : deterministic output under TestBackend. Drives the
        // full `render` path (not just the helper) so we catch buffer
        // off-by-ones now rather than during prompt integration in 5b.
        let block = TextBlock::new("the quick brown fox jumps");
        let rows = render_textblock_to_strings(&block, 10, 4);
        // Expected greedy wrap into width 10:
        //   "the quick" (9)
        //   "brown fox" (9)
        //   "jumps" (5)
        //   "" (unused row)
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
        // : clip rather than panic when the body is taller than
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
        //  will read this; today it just needs to round-trip.
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

    // --: compact unboxed ChoicePrompt rendering -------------

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
        //  reference rendering, modulo the typed-letter trail
        // ("Your choice: e" — the `e` is the player's typed input.
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
        // : storage is lowercase, display is uppercase. The
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
        //  Murder Motel proof scene: four direct-key choices.
        // `Your choice:` label. The renderer's compact mode is the
        // contract that scene relies on once wires it up.
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
        //  vendor example footer (`Your gold: 173g`) sits below
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
        // row between them — matching the contract that the
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
        // : prompt rendering MUST NOT panic on small areas. A
        // zero-width slot is the worst case the bordered-modal path in
        //  will hand us; pinning it now keeps the contract
        // honest before that layout lands.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        assert!(prompt.rendered_lines(0).is_empty());
    }

    #[test]
    fn render_writes_rows_under_test_backend() {
        // : deterministic output under `TestBackend`. Drives the
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
        // : clip rather than panic when the prompt is taller
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
        // modal renderer in relies on this guarantee.
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
        // : truncation OR wrapping at terminal width. A label
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
        // "(E) Equip the very" is 18 cols, "long sword of" is 13.
        // "overflow" is 8. The exact split is the wrapper's policy;
        // pinning the *count* (3 wrapped lines + blank + label) is
        // what the renderer guarantees.
        assert!(rows.len() >= 3);
        for row in rows.iter() {
            assert!(row.chars().count() <= 20, "row {row:?} exceeds width 20");
        }
        assert_eq!(rows.last().map(String::as_str), Some("Your choice:"));
    }

    // ----: disabled-row monochrome marker ----
    //
    // The visible difference between an enabled and a disabled row
    // MUST survive in a strictly monochrome terminal — 's
    // contract — so these tests assert *characters*, never style. The
    // bracket flip (`(K)` → `[K]`) plus the `- ` leader plus the
    // trailing `(reason)` are the three carriers of meaning the
    // example `- [M] Mana potions... (full)` puts on screen.

    #[test]
    fn rendered_lines_marks_disabled_choice_with_monochrome_marker_and_reason() {
        //  reference example. The wandering-monk row from
        //  is the canonical disabled-with-reason rendering and
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
        // The leader + bracket flip MUST still distinguish the row.
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
        // remain visible: hint annotates *what* the option would do.
        // reason annotates *why* it is unavailable, and the
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
        // : the wandering-monk vendor prompt carries a
        // `disabled_if(!can_buy,...)` row plus an enabled "no thanks"
        // out. Pinning the full rendering shape here protects future
        // edits to `format_choice_row` from quietly changing what
        // authors see when they paste the example into their
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
        // : deterministic under TestBackend, including the
        // disabled marker. Drives the full `render` path so a buffer
        // indexing bug surfaces against the visible characters, not
        // just the helper that builds the row strings.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('m', LootAction::Equip, "Mana potions")
            .disabled_if(true, "full");
        let rows = render_prompt_to_strings(&prompt, 40, 4);
        assert_eq!(rows[0], "- [M] Mana potions (full)");
    }

    // ----: selected-row marker + reverse-video for nav mode ----
    //
    // Two carriers of meaning: a character marker (`> `
    // prefix on the cursor row) so a strictly monochrome terminal
    // still shows selection, AND a `Modifier::REVERSED` style for
    // color-capable terminals. The tests below assert *both* layers
    // independently so a future refactor cannot drop one and pass.

    #[test]
    fn rendered_lines_marks_selected_row_with_caret_in_nav_mode() {
        // `.navigable(true)` seeds the cursor on the first enabled
        // choice; that row gets `> `, the others get a ` ` indent so
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
        // protects the contract that Up/Down moves the
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
        // None`, so the reference rendering survives
        // byte-for-byte. This is the regression guard for 's
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
        // from. In nav mode they additionally get the 2-cell
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
        // : MUST NOT panic on small areas. With width < 3 the
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
        // Color-layer half of. Drives `render` through
        // `TestBackend` and asserts the buffer cell on the cursor
        // row carries `Modifier::REVERSED`, while a non-selected
        // choice row does not. Pinning the modifier (rather than a
        // full `Style`) keeps this resilient to 's theme
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
        // Row 1 is the non-selected ` (T) Take` — MUST NOT be
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
        // for the reference snapshot.
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

    // --: bordered modal layout ------------------------------
    //
    // The modal is a thin frame around the same compact row stream the
    // unboxed renderer produces, so these tests verify (1) the border is
    // actually drawn, (2) the optional title appears in the top edge.
    // (3) inner content does not overwrite the border, and (4) tight
    // slots collapse gracefully without panic

    /// Drive `render_modal` through a real `TestBackend` and return a
    /// row-per-string view. Mirrors `render_prompt_to_strings` so modal
    /// tests assert against the same backend semantics as the unboxed
    /// reference snapshots.
    fn render_modal_to_strings<T>(prompt: &ChoicePrompt<T>, w: u16, h: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render_modal(area, frame.buffer_mut());
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
    fn render_modal_draws_full_border_and_inner_content() {
        // : bordered modal mode. Top, bottom, and side edges
        // MUST carry the box-drawing characters Ratatui's `Borders::ALL`
        // emits, and the prompt's compact row stream must land inside
        // not on top of — those edges.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        let rows = render_modal_to_strings(&prompt, 30, 8);
        // Top + bottom edges: a continuous run of horizontal box glyphs
        // bracketed by corners. Asserting the corner cells alone is the
        // tightest contract; the middle is `─` between them.
        assert!(
            rows[0].starts_with('┌'),
            "top-left corner missing: {:?}",
            rows[0]
        );
        assert!(
            rows[0].ends_with('┐'),
            "top-right corner missing: {:?}",
            rows[0]
        );
        assert!(
            rows[7].starts_with('└'),
            "bottom-left corner missing: {:?}",
            rows[7]
        );
        assert!(
            rows[7].ends_with('┘'),
            "bottom-right corner missing: {:?}",
            rows[7]
        );
        // Side edges on every interior row.
        for (i, row) in rows.iter().enumerate().skip(1).take(6) {
            assert!(row.starts_with('│'), "row {i} missing left border: {row:?}");
        }
        // Inner content lives strictly between the borders. The body
        // word "body" is on row 1 (just inside the top border); the
        // first character on that row is the left border, then a space
        // of inner-margin x, then "body".
        assert!(
            rows[1].contains("body"),
            "body did not render inside modal: {:?}",
            rows[1]
        );
    }

    #[test]
    fn render_modal_renders_title_in_top_border() {
        //  lists modal titles as a layout option. When set, the
        // title MUST appear inside the top border so it is visible
        // before the player reads the body — the 's "Title" style
        // role exists precisely for this slot.
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .title("Lost & Found")
            .body("body")
            .choice('e', LootAction::Equip, "Equip");
        let rows = render_modal_to_strings(&prompt, 30, 6);
        assert!(
            rows[0].contains("Lost & Found"),
            "title missing from top border: {:?}",
            rows[0]
        );
        // Top border MUST still close on the right with `┐` even after
        // the title eats some of the run.
        assert!(rows[0].ends_with('┐'));
    }

    #[test]
    fn render_modal_inner_content_does_not_overwrite_borders() {
        // Regression guard: a tall prompt MUST clip to the inner rect
        // rather than punch through the border. Pick choices that would
        // overflow without clipping (5 choices in a 6-tall box: top
        // border + 4 inner rows + bottom border = only 4 inner rows).
        let prompt: ChoicePrompt<LootAction> = ChoicePrompt::new()
            .choice('a', LootAction::Equip, "alpha")
            .choice('b', LootAction::Take, "beta")
            .choice('c', LootAction::Pass, "gamma")
            .choice('d', LootAction::Equip, "delta")
            .choice('e', LootAction::Take, "epsilon");
        let rows = render_modal_to_strings(&prompt, 20, 6);
        // Bottom border MUST survive: if the inner renderer wrote past
        // its rect, the corner glyph would be a content character.
        assert!(
            rows[5].starts_with('└'),
            "bottom border clobbered: {:?}",
            rows[5]
        );
        assert!(
            rows[5].ends_with('┘'),
            "bottom border clobbered: {:?}",
            rows[5]
        );
        // Side borders likewise unbroken on every interior row.
        for (i, row) in rows.iter().enumerate().skip(1).take(4) {
            assert!(row.starts_with('│'), "row {i} left side broken: {row:?}");
            // Right side is at column width-1; the trim drops trailing
            // spaces but the `│` is non-space so it survives the trim.
            assert!(row.ends_with('│'), "row {i} right side broken: {row:?}");
        }
    }

    #[test]
    fn render_modal_zero_area_is_a_noop() {
        //  forbids panicking on small areas. A 0×0 rect must
        // return 0 without touching `buf`.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(prompt.render_modal(area, &mut buf), 0);
    }

    #[test]
    fn render_modal_one_cell_area_does_not_panic() {
        // Single-cell areas cannot fit a border at all. The renderer
        // MUST bail with 0 rather than ask Ratatui to draw an
        // underspec'd box (which would panic on debug assertions in
        // older ratatui versions and is meaningless on any version).
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        let area = Rect::new(0, 0, 1, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(prompt.render_modal(area, &mut buf), 0);
    }

    #[test]
    fn render_modal_two_by_two_draws_border_only() {
        // The minimum viable border is 2×2: four corner glyphs and no
        // inner area. The renderer MUST still draw the box (so a tight
        // layout shows *something* recognisable as a modal frame) but
        // report zero content rows because nothing fit inside.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let drawn = prompt.render_modal(area, &mut buf);
        assert_eq!(drawn, 0, "no inner rows possible at 2×2");
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(1, 0)].symbol(), "┐");
        assert_eq!(buf[(0, 1)].symbol(), "└");
        assert_eq!(buf[(1, 1)].symbol(), "┘");
    }

    #[test]
    fn render_modal_without_title_renders_unbroken_top_edge() {
        // Symmetry with the title-present test: when no title is set.
        // the top edge MUST be a continuous run of horizontal box glyphs
        // — no leading space where the title would have sat.
        let prompt: ChoicePrompt<LootAction> =
            ChoicePrompt::new()
                .body("body")
                .choice('e', LootAction::Equip, "Equip");
        let rows = render_modal_to_strings(&prompt, 12, 5);
        // Between corners, every cell on row 0 is `─`.
        let top = &rows[0];
        let chars: Vec<char> = top.chars().collect();
        assert_eq!(chars.first(), Some(&'┌'));
        assert_eq!(chars.last(), Some(&'┐'));
        for (i, c) in chars.iter().enumerate().skip(1).take(chars.len() - 2) {
            assert_eq!(*c, '─', "expected unbroken top edge at index {i}, got {c}");
        }
    }

    // ----: Theme role-to-Style mapping -------------------------

    #[test]
    fn default_theme_hotkey_role_is_bold_cyan() {
        // : hotkey markers MUST stay legible without color.
        // BOLD is the monochrome carrier; cyan is the BBS-traditional
        // action-key tint that gets stripped on monochrome terminals.
        let style = Theme::default().style(StyleRole::Hotkey);
        assert_eq!(style.fg, Some(Color::Cyan));
        assert!(
            style.add_modifier.contains(Modifier::BOLD),
            "Hotkey role MUST add BOLD so the marker survives monochrome terminals"
        );
    }

    #[test]
    fn default_theme_error_role_is_bold_red() {
        // : error feedback MUST NOT rely on color alone.
        let style = Theme::default().style(StyleRole::Error);
        assert_eq!(style.fg, Some(Color::Red));
        assert!(
            style.add_modifier.contains(Modifier::BOLD),
            "Error role MUST add BOLD so red-blind / monochrome terminals still read it as loud"
        );
    }

    #[test]
    fn default_theme_disabled_role_is_dim_no_color() {
        // : disabled rows already carry a marker shape;
        // the role-level styling is supplementary DIM, no color, so
        // black/white terminals see the same dim secondary weight.
        let style = Theme::default().style(StyleRole::Disabled);
        assert_eq!(style.fg, None, "Disabled MUST NOT lean on a color");
        assert!(
            style.add_modifier.contains(Modifier::DIM),
            "Disabled role MUST add DIM as the supplementary visual weight"
        );
    }

    #[test]
    fn default_theme_normal_role_is_bare_default() {
        // The bare-default normal style is what the existing renderer
        // already writes for non-selected rows. Asserting it explicitly
        // pins the contract so future theme tweaks cannot silently
        // alter normal-row appearance.
        assert_eq!(Theme::default().style(StyleRole::Normal), Style::default());
    }

    #[test]
    fn default_theme_success_currency_carry_expected_colors() {
        // Success/Currency are color-led roles whose monochrome carrier
        // is the surrounding text content (e.g. "Moved Room 7 key..."
        // or "50g"), so the style itself is color-only.
        let theme = Theme::default();
        assert_eq!(theme.style(StyleRole::Success).fg, Some(Color::Green));
        assert_eq!(theme.style(StyleRole::Currency).fg, Some(Color::Yellow));
    }

    #[test]
    fn default_theme_title_and_emphasis_are_bold() {
        let theme = Theme::default();
        assert!(theme
            .style(StyleRole::Title)
            .add_modifier
            .contains(Modifier::BOLD));
        assert!(theme
            .style(StyleRole::Emphasis)
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn default_theme_muted_and_hint_are_dim() {
        let theme = Theme::default();
        assert!(theme
            .style(StyleRole::Muted)
            .add_modifier
            .contains(Modifier::DIM));
        assert!(theme
            .style(StyleRole::Hint)
            .add_modifier
            .contains(Modifier::DIM));
    }

    #[test]
    fn theme_with_role_overrides_only_that_role() {
        // Override one role and confirm (a) the override took effect
        // and (b) other roles still match the default mapping. This is
        // the contract that lets a game retheme a single accent without
        // re-declaring the full table.
        let custom =
            Theme::default().with_role(StyleRole::Hotkey, Style::default().fg(Color::Magenta));
        assert_eq!(
            custom.style(StyleRole::Hotkey),
            Style::default().fg(Color::Magenta)
        );
        // Untouched roles are byte-identical to the default theme.
        let baseline = Theme::default();
        for role in [
            StyleRole::Normal,
            StyleRole::Title,
            StyleRole::Emphasis,
            StyleRole::Muted,
            StyleRole::Hint,
            StyleRole::Error,
            StyleRole::Success,
            StyleRole::Currency,
            StyleRole::Disabled,
        ] {
            assert_eq!(
                custom.style(role),
                baseline.style(role),
                "with_role MUST NOT alter unrelated roles; {role:?} drifted"
            );
        }
    }

    // -- ConfirmPrompt --------------------------------------
    //
    //  fixes the contract: render a question, two-or-three-way
    // choices, optional Enter default, typed yes/no/cancel outcomes.
    // These tests pin the four behaviours the checklist calls out — `y`.
    // `n`, Esc, and the Enter-default policy — plus the boundary cases
    // (case-insensitive hotkeys, Enter without a default, resize ignored)
    // a regression in the inner reducer would silently break.

    #[test]
    fn confirm_prompt_y_returns_yes_case_insensitive() {
        let prompt = ConfirmPrompt::new("Delete the save?");
        assert_eq!(prompt.handle(Input::Char('y')), ConfirmOutcome::Yes);
        // Case-insensitive matching is inherited from `PromptKey::char`'s
        // lowercasing — pin it here so a future "store as authored" change
        // would have to update this test too, not just the storage.
        assert_eq!(prompt.handle(Input::Char('Y')), ConfirmOutcome::Yes);
    }

    #[test]
    fn confirm_prompt_n_returns_no_case_insensitive() {
        let prompt = ConfirmPrompt::new("Delete the save?");
        assert_eq!(prompt.handle(Input::Char('n')), ConfirmOutcome::No);
        assert_eq!(prompt.handle(Input::Char('N')), ConfirmOutcome::No);
    }

    #[test]
    fn confirm_prompt_esc_cancels_by_default() {
        // : yes/no/cancel. ConfirmPrompt inverts ChoicePrompt's
        // default-off cancellable so Esc means "back out" without an
        // explicit opt-in.
        let prompt = ConfirmPrompt::new("Delete the save?");
        assert_eq!(prompt.handle(Input::Esc), ConfirmOutcome::Cancelled);
    }

    #[test]
    fn confirm_prompt_esc_no_op_when_cancellation_disabled() {
        // The inverse of the default — confirms the builder genuinely
        // toggles the underlying ChoicePrompt::cancellable flag rather
        // than hard-wiring Esc handling in `handle`.
        let prompt = ConfirmPrompt::new("Confirm quit?").cancellable(false);
        assert_eq!(prompt.handle(Input::Esc), ConfirmOutcome::None);
    }

    #[test]
    fn confirm_prompt_enter_without_default_is_noop() {
        // : Enter-default is "when configured". Without a
        // default, Enter must not silently pick yes — that would create
        // a phantom confirmation on a stray keypress.
        let prompt = ConfirmPrompt::new("Delete the save?");
        assert_eq!(prompt.handle(Input::Enter), ConfirmOutcome::None);
    }

    #[test]
    fn confirm_prompt_default_yes_makes_enter_confirm() {
        let prompt = ConfirmPrompt::new("Continue?").default_yes();
        assert_eq!(prompt.handle(Input::Enter), ConfirmOutcome::Yes);
        // Hotkeys still work alongside the default — Enter is *additive*.
        // not a replacement for direct-key matching.
        assert_eq!(prompt.handle(Input::Char('n')), ConfirmOutcome::No);
    }

    #[test]
    fn confirm_prompt_default_no_makes_enter_decline() {
        // Dangerous-action default: Enter declines so a stray keypress
        // never destroys progress. calls this out explicitly.
        let prompt = ConfirmPrompt::new("Delete the save?").default_no();
        assert_eq!(prompt.handle(Input::Enter), ConfirmOutcome::No);
        assert_eq!(prompt.handle(Input::Char('y')), ConfirmOutcome::Yes);
    }

    #[test]
    fn confirm_prompt_resize_is_ignored() {
        // : `Resize` MUST NOT select a choice. ConfirmPrompt
        // inherits this through `ChoicePrompt::handle` — pin it here so
        // a future direct match on Input in ConfirmPrompt::handle does
        // not accidentally re-introduce a resize-as-confirm bug.
        let prompt = ConfirmPrompt::new("Continue?").default_yes();
        assert_eq!(
            prompt.handle(Input::Resize {
                width: 80,
                height: 24
            }),
            ConfirmOutcome::None
        );
    }

    #[test]
    fn confirm_prompt_unknown_key_is_ignored() {
        let prompt = ConfirmPrompt::new("Continue?");
        assert_eq!(prompt.handle(Input::Char('q')), ConfirmOutcome::None);
        assert_eq!(prompt.handle(Input::Unknown), ConfirmOutcome::None);
    }

    #[test]
    fn confirm_prompt_labels_are_customisable_without_changing_keys() {
        // Custom labels — body words like "Delete the save" — still bind
        // to Y/N. Hotkey identity is the contract; the visible
        // label is pure presentation.
        let prompt = ConfirmPrompt::new("Delete the save?")
            .yes_label("Delete it")
            .no_label("Keep it");
        let inner = prompt.inner();
        assert_eq!(inner.choices[0].key, PromptKey::Char('y'));
        assert_eq!(inner.choices[0].label, "Delete it");
        assert_eq!(inner.choices[1].key, PromptKey::Char('n'));
        assert_eq!(inner.choices[1].label, "Keep it");
    }

    #[test]
    fn confirm_prompt_renders_question_and_choices() {
        // Smoke test that delegation to ChoicePrompt's renderer
        // produces the expected row stream — the question as body, the
        // (Y) and (N) choice rows, and the default prompt label. We
        // assert containment rather than exact equality because the
        // upstream renderer's row format is already byte-pinned by its
        // own tests.
        let prompt = ConfirmPrompt::new("Delete the save?");
        let rows = prompt.inner().rendered_lines(40);
        let joined = rows.join("\n");
        assert!(
            joined.contains("Delete the save?"),
            "body missing: {joined:?}"
        );
        assert!(joined.contains("(Y) Yes"), "yes row missing: {joined:?}");
        assert!(joined.contains("(N) No"), "no row missing: {joined:?}");
        assert!(
            joined.contains("Your choice:"),
            "prompt label missing: {joined:?}"
        );
    }

    // ---- AnyKeyPrompt (.md ) -----------------------------
    //
    // The reducer surface is small but load-bearing: every meaningful
    // key MUST complete the pause, and resize/Unknown MUST NOT. These
    // tests pin both halves of that contract and serve as the public
    // documentation of which Input variants count as "meaningful".

    #[test]
    fn any_key_prompt_resize_is_ignored() {
        // : "Ignore resize events." A resize during a pause is
        // a layout signal, not an acknowledgement.
        let prompt = AnyKeyPrompt::new().body("You feel a chill.");
        assert_eq!(
            prompt.handle(Input::Resize {
                width: 80,
                height: 24
            }),
            AnyKeyOutcome::None,
        );
    }

    #[test]
    fn any_key_prompt_unknown_is_ignored() {
        //  MAY clause: modifier-only release-only events
        // collapse to Input::Unknown in the runtime mapping; treating
        // them as completion would let a focus-change or stray Ctrl
        // press dismiss the pause.
        let prompt = AnyKeyPrompt::new();
        assert_eq!(prompt.handle(Input::Unknown), AnyKeyOutcome::None);
    }

    #[test]
    fn any_key_prompt_enter_completes() {
        // Enter is the conventional ack key for BBS pauses;
        // calls Enter out implicitly by saying "any meaningful key".
        let prompt = AnyKeyPrompt::new();
        assert_eq!(prompt.handle(Input::Enter), AnyKeyOutcome::Completed);
    }

    #[test]
    fn any_key_prompt_space_completes() {
        // Char(' ') is the other classic continue-key; this test pins
        // the Char(_) arm so a future refactor can't accidentally
        // restrict completion to a hand-picked allowlist.
        let prompt = AnyKeyPrompt::new();
        assert_eq!(prompt.handle(Input::Char(' ')), AnyKeyOutcome::Completed);
    }

    #[test]
    fn any_key_prompt_esc_completes() {
        //  makes no carve-out for Esc — the pause is purely
        // an acknowledgement, so Esc completes like any other
        // meaningful key. Authors who need an Esc-cancellable pause
        // should use ConfirmPrompt instead.
        let prompt = AnyKeyPrompt::new();
        assert_eq!(prompt.handle(Input::Esc), AnyKeyOutcome::Completed);
    }

    #[test]
    fn any_key_prompt_arrow_keys_complete() {
        // Pin the arrow arms explicitly: a player whose hand is on the
        // arrow keys after a map scene shouldn't have to find a letter
        // key to advance.
        let prompt = AnyKeyPrompt::new();
        for input in [Input::Up, Input::Down, Input::Left, Input::Right] {
            assert_eq!(
                prompt.handle(input),
                AnyKeyOutcome::Completed,
                "arrow key {input:?} should complete the pause",
            );
        }
    }

    #[test]
    fn any_key_prompt_backspace_and_ctrl_complete() {
        // Backspace and Ctrl(_) are meaningful keypresses per
        // ; only Resize and Unknown are explicitly excluded.
        let prompt = AnyKeyPrompt::new();
        assert_eq!(prompt.handle(Input::Backspace), AnyKeyOutcome::Completed);
        assert_eq!(prompt.handle(Input::Ctrl('c')), AnyKeyOutcome::Completed);
    }

    #[test]
    fn any_key_prompt_builder_records_body_footer_title() {
        // The reducer ignores body/footer/title, but needs them
        // intact at render time. Pin the builder so a stray refactor
        // can't drop a paragraph or silently overwrite the override.
        let prompt = AnyKeyPrompt::new()
            .body("First.")
            .body("Second.")
            .footer("Press any key to wake up.")
            .title("Lobby");
        assert_eq!(
            prompt.body_lines(),
            &["First.".to_string(), "Second.".to_string()],
        );
        assert_eq!(prompt.footer_text(), Some("Press any key to wake up."));
        assert_eq!(prompt.title_text(), Some("Lobby"));
    }

    #[test]
    fn any_key_prompt_default_footer_is_unset() {
        // `None` at the data layer signals "use the default"
        // to the renderer — not "render an empty footer". The
        // distinction matters for, which will substitute
        // "Press any key to continue..." when no override is present.
        let prompt = AnyKeyPrompt::new();
        assert!(prompt.footer_text().is_none());
        assert!(prompt.title_text().is_none());
        assert!(prompt.body_lines().is_empty());
    }

    // --: AnyKeyPrompt rendering ------------------------------

    /// Drive `AnyKeyPrompt::render` through `TestBackend` so the
    /// rendering tests assert against actual buffer cells, not just the
    /// string-helper output. Mirrors `render_prompt_to_strings`.
    fn render_any_key_to_strings(prompt: &AnyKeyPrompt, w: u16, h: u16) -> Vec<String> {
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
    fn any_key_prompt_renders_default_footer_below_body_with_blank_gap() {
        // : body, then `Press any key to continue...`. A blank
        // gap between body and footer matches the ChoicePrompt
        // body→choices convention so the two prompt kinds visually
        // align when an author swaps one for the other mid-scene.
        let prompt = AnyKeyPrompt::new().body("You feel a chill.");
        assert_eq!(
            prompt.rendered_lines(40),
            vec![
                "You feel a chill.".to_string(),
                String::new(),
                "Press any key to continue...".to_string(),
            ],
        );
    }

    #[test]
    fn any_key_prompt_default_footer_constant_matches_spec() {
        // Pin the literal default so a typo in the constant surfaces
        // here rather than in a Murder Motel screenshot review.
        assert_eq!(DEFAULT_ANY_KEY_FOOTER, "Press any key to continue...");
    }

    #[test]
    fn any_key_prompt_uses_author_footer_override() {
        // Author override wins; the default cue is suppressed, not
        // appended. describes the default as a fallback, not a
        // mandatory line.
        let prompt = AnyKeyPrompt::new()
            .body("The lobby fades.")
            .footer("Press any key to wake up.");
        assert_eq!(
            prompt.rendered_lines(40),
            vec![
                "The lobby fades.".to_string(),
                String::new(),
                "Press any key to wake up.".to_string(),
            ],
        );
    }

    #[test]
    fn any_key_prompt_renders_footer_alone_when_body_is_empty() {
        // A pause with no narration is still legal — for example, a
        // post-modal acknowledgement after a status flash. The footer
        // renders without a leading blank gap so the cue sits at the
        // top of its slot rather than orphaned on row 2.
        let prompt = AnyKeyPrompt::new();
        assert_eq!(
            prompt.rendered_lines(40),
            vec!["Press any key to continue...".to_string()],
        );
    }

    #[test]
    fn any_key_prompt_wraps_body_at_supplied_width() {
        // Body must wrap on whitespace boundaries, same as TextBlock.
        // A 20-cell width forces "You feel a sudden cold breath on the
        // back of your neck." onto multiple visual rows; the footer
        // still renders unchanged below the wrapped body.
        let prompt =
            AnyKeyPrompt::new().body("You feel a sudden cold breath on the back of your neck.");
        let rows = prompt.rendered_lines(20);
        assert_eq!(
            rows,
            vec![
                "You feel a sudden".to_string(),
                "cold breath on the".to_string(),
                "back of your neck.".to_string(),
                String::new(),
                "Press any key to".to_string(),
                "continue...".to_string(),
            ],
        );
    }

    #[test]
    fn any_key_prompt_preserves_blank_lines_between_paragraphs() {
        // Two `.body(...)` calls separated by an explicit empty
        // paragraph render with one blank visual row between them — the
        //  rule the TextBlock tests already pin. The pause's
        // blank gap before the footer sits on top of that, so consecutive
        // blank rows are legal here.
        let prompt = AnyKeyPrompt::new().body("first").body("").body("second");
        assert_eq!(
            prompt.rendered_lines(40),
            vec![
                "first".to_string(),
                String::new(),
                "second".to_string(),
                String::new(),
                "Press any key to continue...".to_string(),
            ],
        );
    }

    #[test]
    fn any_key_prompt_zero_width_returns_no_rows_and_does_not_panic() {
        // : rendering MUST NOT panic on small areas. A zero-width
        // slot is the worst case the bordered-modal path hands us.
        let prompt = AnyKeyPrompt::new().body("body");
        assert!(prompt.rendered_lines(0).is_empty());
    }

    #[test]
    fn any_key_prompt_render_writes_rows_under_test_backend() {
        // Drive the full `render` path so an off-by-one in `Buffer`
        // indexing surfaces here, not in a manual smoke test. The
        // buffer is 40×4 to leave one trailing blank row beneath the
        // three rendered ones.
        let prompt = AnyKeyPrompt::new().body("You feel a chill.");
        let rows = render_any_key_to_strings(&prompt, 40, 4);
        assert_eq!(
            rows,
            vec![
                "You feel a chill.".to_string(),
                String::new(),
                "Press any key to continue...".to_string(),
                String::new(),
            ],
        );
    }

    #[test]
    fn any_key_prompt_render_clips_when_height_is_short() {
        // : lines past the available height are silently clipped
        // rather than panicking. Two rows of room → only the body and
        // the blank-gap row write; the footer is dropped.
        let prompt = AnyKeyPrompt::new().body("You feel a chill.");
        let rows = render_any_key_to_strings(&prompt, 40, 2);
        assert_eq!(rows, vec!["You feel a chill.".to_string(), String::new()],);
    }

    #[test]
    fn any_key_prompt_render_zero_dimensions_writes_nothing() {
        //  small-area policy: 0×0 returns 0 and does not panic.
        // We exercise both `area.width == 0` and `area.height == 0` via
        // a 0×0 frame; the TestBackend rejects 0-sized backends, so
        // call `render` directly with a hand-built buffer.
        let prompt = AnyKeyPrompt::new().body("body");
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        let written = prompt.render(Rect::new(0, 0, 0, 4), &mut buf);
        assert_eq!(written, 0);
        let written = prompt.render(Rect::new(0, 0, 4, 0), &mut buf);
        assert_eq!(written, 0);
    }

    #[test]
    fn any_key_prompt_render_modal_draws_border_with_title() {
        // Bordered modal layout: top border carries the
        // title, content sits inside the 1-cell margin. We assert the
        // top-left corner glyph plus the title, and that the body and
        // footer appear on the inner rows.
        let prompt = AnyKeyPrompt::new().title("Lobby").body("You feel a chill.");
        let rows = render_any_key_to_strings_modal(&prompt, 30, 6);
        // Top border with embedded title.
        assert!(
            rows[0].starts_with("┌Lobby"),
            "top border missing title: {:?}",
            rows[0],
        );
        // Body + blank + footer on inner rows 1..=3.
        assert!(rows[1].contains("You feel a chill."));
        assert!(rows[3].contains("Press any key to continue..."));
        // Bottom border closes the box.
        assert!(rows[5].starts_with('└'));
    }

    /// Modal-flavored counterpart to [`render_any_key_to_strings`].
    /// Inlined alongside the test that uses it so a future test for the
    /// same path stays close to the helper.
    fn render_any_key_to_strings_modal(prompt: &AnyKeyPrompt, w: u16, h: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render_modal(area, frame.buffer_mut());
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
    fn any_key_prompt_render_modal_small_area_is_noop() {
        // : a 1-cell or smaller modal cannot host a border;
        // returning 0 without writing keeps the contract honest.
        let prompt = AnyKeyPrompt::new().body("body");
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(prompt.render_modal(Rect::new(0, 0, 1, 4), &mut buf), 0);
        assert_eq!(prompt.render_modal(Rect::new(0, 0, 4, 1), &mut buf), 0);
    }

    // --: FeedbackLine ----------------------------------------

    #[test]
    fn feedback_kind_markers_are_unique_and_monochrome_safe() {
        // : monochrome readability requires *distinct* prefixes
        // — color is not load-bearing, so the markers themselves must
        // tell info success error apart on a black/white terminal.
        let info = FeedbackKind::Info.marker();
        let success = FeedbackKind::Success.marker();
        let error = FeedbackKind::Error.marker();
        assert_ne!(info, success);
        assert_ne!(info, error);
        assert_ne!(success, error);
        // Pure-ASCII so the prefix renders identically under any
        // codepage; no terminal control sequences.
        for marker in [info, success, error] {
            assert!(
                marker.chars().all(|c| c.is_ascii() && !c.is_control()),
                "marker {marker:?} must be plain ASCII",
            );
        }
    }

    #[test]
    fn feedback_kind_marker_pins_concrete_strings() {
        // Pin the literals so a typo in the marker mapping surfaces here
        // rather than during a Murder Motel screenshot review.
        assert_eq!(FeedbackKind::Info.marker(), "");
        assert_eq!(FeedbackKind::Success.marker(), "+ ");
        assert_eq!(FeedbackKind::Error.marker(), "! ");
    }

    #[test]
    fn feedback_kind_style_role_maps_to_spec_roles() {
        //  lists Success and Error among the initial style
        // roles; Info rides the default `Normal` foreground so it does
        // not compete with the surrounding body.
        assert_eq!(FeedbackKind::Info.style_role(), StyleRole::Normal);
        assert_eq!(FeedbackKind::Success.style_role(), StyleRole::Success);
        assert_eq!(FeedbackKind::Error.style_role(), StyleRole::Error);
    }

    #[test]
    fn feedback_line_constructors_record_kind_and_text() {
        // The three constructors are the public API surface; verify each
        // sets the kind and stores the author-supplied text verbatim
        // (no marker pre-applied, no trimming).
        let info = FeedbackLine::info("Moved Room 7 key to inventory.");
        assert_eq!(info.kind(), FeedbackKind::Info);
        assert_eq!(info.text(), "Moved Room 7 key to inventory.");

        let success = FeedbackLine::success("Bought black coffee for 25g.");
        assert_eq!(success.kind(), FeedbackKind::Success);
        assert_eq!(success.text(), "Bought black coffee for 25g.");

        let error = FeedbackLine::error("Need 50g to tip for a rumor.");
        assert_eq!(error.kind(), FeedbackKind::Error);
        assert_eq!(error.text(), "Need 50g to tip for a rumor.");
    }

    #[test]
    fn feedback_line_rendered_text_concatenates_marker_and_body() {
        //  monochrome contract: the marker is the visible
        // severity carrier, so `rendered_text` MUST surface it.
        let info = FeedbackLine::info("Moved Room 7 key to inventory.");
        assert_eq!(info.rendered_text(), "Moved Room 7 key to inventory.");

        let success = FeedbackLine::success("Bought black coffee for 25g.");
        assert_eq!(success.rendered_text(), "+ Bought black coffee for 25g.");

        let error = FeedbackLine::error("Need 50g to tip for a rumor.");
        assert_eq!(error.rendered_text(), "! Need 50g to tip for a rumor.");
    }

    /// Drive `FeedbackLine::render` through `TestBackend` and return
    /// each visual row as a trimmed string. Mirrors the helper used by
    /// the AnyKeyPrompt rendering tests so the assertions stay uniform.
    fn render_feedback_to_strings(line: &FeedbackLine, w: u16, h: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            line.render(area, frame.buffer_mut());
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
    fn feedback_line_render_writes_marker_plus_text_under_test_backend() {
        // : deterministic output under TestBackend. Exercise all
        // three variants so each marker shape is pinned in the buffer.
        // not just the helper output.
        let info = FeedbackLine::info("Moved Room 7 key to inventory.");
        assert_eq!(
            render_feedback_to_strings(&info, 40, 1),
            vec!["Moved Room 7 key to inventory.".to_string()],
        );

        let success = FeedbackLine::success("Bought black coffee for 25g.");
        assert_eq!(
            render_feedback_to_strings(&success, 40, 1),
            vec!["+ Bought black coffee for 25g.".to_string()],
        );

        let error = FeedbackLine::error("Need 50g to tip for a rumor.");
        assert_eq!(
            render_feedback_to_strings(&error, 40, 1),
            vec!["! Need 50g to tip for a rumor.".to_string()],
        );
    }

    #[test]
    fn feedback_line_render_returns_one_row_and_leaves_extra_rows_blank() {
        // Single-row contract: render writes exactly one row and reports
        // 1, regardless of how tall the area is. The remaining rows must
        // stay untouched so the caller can stack a body beneath.
        let line = FeedbackLine::success("Saved.");
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        let written = line.render(Rect::new(0, 0, 20, 3), &mut buf);
        assert_eq!(written, 1);
        // Row 0: marker + text. Rows 1+: blank (Buffer::empty default).
        let row0: String = (0..20).map(|x| buf[(x, 0)].symbol()).collect();
        let row1: String = (0..20).map(|x| buf[(x, 1)].symbol()).collect();
        assert_eq!(row0.trim_end(), "+ Saved.");
        assert_eq!(
            row1.trim().chars().filter(|c| !c.is_whitespace()).count(),
            0
        );
    }

    #[test]
    fn feedback_line_render_truncates_when_width_is_short() {
        // : small areas MUST NOT panic. A width that can't hold
        // the full message right-truncates rather than wraps — wrapping
        // would shift the layout below the line and break the
        // single-row feedback shape.
        let line = FeedbackLine::error("Need 50g to tip for a rumor.");
        let rows = render_feedback_to_strings(&line, 10, 1);
        // 10 cells: "! Need 50g" (the prefix consumes 2, leaving 8).
        assert_eq!(rows, vec!["! Need 50g".to_string()]);
    }

    #[test]
    fn feedback_line_render_zero_dimensions_is_noop() {
        //  small-area policy: 0×N or N×0 returns 0 and writes no
        // cells. Exercise both axes with a hand-built buffer so the
        // TestBackend rejection of 0-sized backends doesn't apply.
        let line = FeedbackLine::info("hi");
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 4));
        assert_eq!(line.render(Rect::new(0, 0, 0, 4), &mut buf), 0);
        assert_eq!(line.render(Rect::new(0, 0, 4, 0), &mut buf), 0);
    }

    #[test]
    fn feedback_line_render_with_theme_applies_role_style() {
        // Theme overrides MUST flow into the buffer cells the line
        // writes. We override the `Success` role with a magenta
        // foreground (a value the default theme never emits) and assert
        // every cell carrying a marker/text glyph picks up that color.
        // Cells outside the rendered prefix should NOT inherit the role
        // style — that's how `set_stringn` handles a short string.
        let line = FeedbackLine::success("Saved.");
        let theme =
            Theme::default().with_role(StyleRole::Success, Style::default().fg(Color::Magenta));
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        let written = line.render_with_theme(Rect::new(0, 0, 20, 1), &mut buf, &theme);
        assert_eq!(written, 1);
        // Rendered text is "+ Saved." — eight cells. Each MUST carry the
        // overridden foreground; cell 8 onward should not.
        for x in 0..8u16 {
            assert_eq!(
                buf[(x, 0)].style().fg,
                Some(Color::Magenta),
                "cell {x} missing role override",
            );
        }
        // Cells past the rendered string keep the buffer's default
        // foreground (Color::Reset) — the role override doesn't bleed.
        assert_ne!(buf[(8, 0)].style().fg, Some(Color::Magenta));
    }

    #[test]
    fn feedback_line_info_renders_with_normal_role_not_a_loud_one() {
        // : `Info` rides on `StyleRole::Normal`, which the
        // default theme keeps unstyled (no color override, no modifier).
        // The whole point of `Normal` is that it inherits the terminal's
        // own foreground — a neutral status note must not accidentally
        // render as an error or success, which would break the
        // monochrome contract by making severity ambiguous.
        let line = FeedbackLine::info("Moved Room 7 key to inventory.");
        let theme = Theme::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        line.render_with_theme(Rect::new(0, 0, 40, 1), &mut buf, &theme);
        // Compare against a cell that has never been touched: same row.
        // past the end of the rendered string. If `Normal` is truly the
        // identity style, the touched cells must be indistinguishable
        // from the untouched baseline.
        let baseline = buf[(39, 0)].style();
        for x in 0..30u16 {
            let cell_style = buf[(x, 0)].style();
            assert_eq!(
                cell_style.fg, baseline.fg,
                "info cell {x} fg drifted from untouched baseline",
            );
            assert_eq!(
                cell_style.add_modifier, baseline.add_modifier,
                "info cell {x} modifier drifted from untouched baseline",
            );
        }
        // And the loud roles MUST differ — guard against future drift
        // where `Normal` accidentally inherits, say, `Success` styling.
        assert_ne!(
            theme.style(StyleRole::Success),
            theme.style(StyleRole::Normal)
        );
        assert_ne!(
            theme.style(StyleRole::Error),
            theme.style(StyleRole::Normal)
        );
    }
}
