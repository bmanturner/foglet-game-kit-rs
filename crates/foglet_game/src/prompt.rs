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
