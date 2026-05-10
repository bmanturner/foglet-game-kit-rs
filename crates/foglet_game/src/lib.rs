//! `foglet_game` — authoring kit for Foglet `:external_pty` terminal
//! door games.
//!
//! This crate's role, in one sentence: provide the runtime, terminal
//! safety guarantees, and primitives a game author needs so their
//! `main.rs` is "wire up screens, hand control to `Game::run()`".
//!
//! # Module map (target — populated across the implementation tasks)
//!
//! The eventual module layout follows SPEC §6:
//!
//! - `terminal` — raw-mode/alt-screen guard (Task 5)
//! - `foglet`  — `FogletContext` loader (Task 2)
//! - `input`   — `crossterm` event → `Input` normalization (Task 6, done)
//! - `screen`  — `Screen` trait + `ScreenCommand` (Task 7)
//! - `runtime` — top-level `Game` builder + loop (Task 7)
//! - `save`    — atomic save manager (Task 8)
//! - `world`, `map`, `entity`, `dialog`, `widgets` — primitives
//!   (Task 9)
//! - `error`   — library-internal `thiserror` types
//!
//! Task 1 only stands up the crate so the workspace builds. Modules
//! land alongside the tasks that exercise them.
//!
//! # Stability
//!
//! Pre-1.0. The public API is allowed to break between minor versions
//! while we converge on the SPEC §8 contract. Breaking changes will
//! be called out in commit messages and (eventually) `CHANGELOG.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs, rust_2018_idioms)]

pub mod bounties;
pub mod challenges;
pub mod config;
pub mod contracts;
pub mod dialog;
pub mod dialog_screen;
pub mod events;
pub mod factions;
pub mod foglet;
pub mod input;
pub mod inventory;
pub mod job_board;
pub mod leaderboards;
pub mod manifest;
pub mod map;
pub mod market;
pub mod notices;
pub mod place_recall;
pub mod players;
pub mod presence;
pub mod prompt;
pub mod prompt_screen;
pub mod roles;
pub mod runtime;
pub mod save;
pub mod screen;
pub mod spatial;
pub mod terminal;
pub mod turns;
pub mod widgets;
pub mod world_db;
pub mod world_ticks;

pub use bounties::{
    Bounty, BountyError, BountyState, BOUNTIES_MIGRATION, BOUNTY_DESCRIPTION_MAX_CHARS,
    BOUNTY_TITLE_MAX_CHARS,
};
pub use challenges::{Challenge, ChallengeError, ChallengeState, CHALLENGES_MIGRATION};
pub use config::{
    ConfigError, FactionSeed, FactionsSection, GameConfig, GameSection, InventorySection,
    LeaderboardSection, LeaderboardSort, ManifestSection, MultiplayerSection, PlaceRecallSection,
    PresenceSection, SaveSection, SaveStrategy, SpatialSection, TurnReset, TurnsSection,
    WorldSection, WorldTicksSection,
};
pub use contracts::{
    Contract, ContractError, ContractState, CreateContractInput, CONTRACTS_MIGRATION,
};
pub use dialog::{
    dialog_choice_prompt, dialog_handle_prompt_input, load_dialog, Choice, ChoiceError, Dialog,
    DialogError, DialogState, FlagSet, Node, DIALOG_PROMPT_MAX_CHOICES,
};
pub use dialog_screen::{DialogAction, DialogLayout, DialogScreen};
pub use events::{EventError, EventRecord, MAX_EVENT_MESSAGE_LEN, WORLD_EVENTS_MIGRATION};
pub use factions::{
    Faction, FactionError, FactionMembership, SharedGoal, FACTIONS_MIGRATION,
    FACTION_GOAL_COMPLETED_EVENT_KIND,
};
pub use foglet::{
    load_context, load_context_from_env, load_context_from_file, load_context_with_options,
    process_env, synthesize_local_dev, ContextError, ContextSource, FogletContext, LoadOptions,
};
pub use input::{from_event, from_key_event, Input};
pub use inventory::{InventoryError, InventorySlot, INVENTORY_SLOTS_MIGRATION};
pub use job_board::{
    BountyProvider, BuiltInProviders, ChallengeProvider, ContractProvider, JobBoard, JobBoardEntry,
    JobBoardError, JobBoardFilter, JobBoardSort, JobBoardSource, OpportunityProvider,
};
pub use leaderboards::{LeaderboardError, ScoreRecord, LEADERBOARD_SCORES_MIGRATION};
pub use manifest::{
    FogletManifest, ManifestError, ManifestInputs, DEFAULT_AUTH_SCOPE, DEFAULT_IDLE_TIMEOUT_MS,
    DEFAULT_TIMEOUT_MS, DEFAULT_VISIBILITY, RUNTIME_EXTERNAL_PTY,
};
pub use map::{
    parse_map, EntityPlacement, Map, MapError, Tile, TileKind, TileLegend, PLAYER_GLYPH,
};
pub use market::{
    MarketError, MarketListing, MARKET_BUY_EVENT_KIND, MARKET_DISPLAY_NAME_MAX_CHARS,
    MARKET_LISTINGS_MIGRATION,
};
pub use notices::{Notice, NoticeError, NOTICES_MIGRATION, NOTICE_SUBJECT_MAX_CHARS};
pub use place_recall::{PlaceRecallError, PlaceRecallRecord, PLACE_RECALL_MIGRATION};
pub use players::{PlayerError, PlayerRecord, PLAYERS_MIGRATION};
pub use presence::{PresenceRecord, PRESENCE_MIGRATION};
pub use prompt::{
    AnyKeyOutcome, AnyKeyPrompt, ChoicePrompt, ConfirmAction, ConfirmOutcome, ConfirmPrompt,
    FeedbackKind, FeedbackLine, PromptAction, PromptChoice, PromptError, PromptKey, StyleRole,
    Theme,
};
pub use prompt_screen::{PromptLayout, PromptScreen};
pub use roles::{FogletRole, MOD_SECURITY_LEVEL, SYSOP_SECURITY_LEVEL, USER_SECURITY_LEVEL};
pub use runtime::{
    run_built, run_with_io, BuiltGame, CrosstermEventSource, EventSource, Game, GameError,
    GameResult, SaveHandler, SavePolicy, TICK_INTERVAL,
};
pub use save::{
    read_save, resolve_save_path, write_atomic, SaveIoError, SavePathError, SavePathInputs,
    SaveSlot, SAVE_DIR_ENV, SAVE_FILENAME,
};
pub use screen::{
    apply_command, ExitReason, GameContext, Screen, ScreenCommand, ScreenStack, SideEffect,
};
pub use spatial::{Place, PlaceError, Route, RouteError, PLACES_MIGRATION, ROUTES_MIGRATION};
pub use terminal::{
    arm_panic_hook, disarm_panic_hook, flush_stdout, install_panic_hook, install_panic_hook_with,
    is_panic_hook_armed, CrosstermBackend, PanicRestoreFn, TerminalBackend, TerminalError,
    TerminalGuard,
};
pub use turns::{
    DateProvider, FixedDateProvider, LocalDate, TurnError, TurnLedgerRow, TURN_LEDGER_MIGRATION,
};
pub use widgets::{
    centred_rect, render_hint_line, render_inventory_list, render_menu_list, render_message_line,
    render_modal, InventoryList, MenuList, MessageKind, MessageLine,
};
pub use world_db::{WorldDb, WorldDbError, WorldDbOptions, WorldMigration};
pub use world_ticks::{WorldTickError, WorldTickTask, WORLD_TICK_TASKS_MIGRATION};

/// Crate version string, sourced from `Cargo.toml` at build time.
///
/// Exposed primarily so the `fgk` CLI and example games can print
/// "built against foglet_game vX.Y.Z" diagnostics. Authoring code
/// usually has no reason to read this directly.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check: the crate compiles and `VERSION` is wired to the
    /// Cargo manifest. Replaced with real coverage as modules land.
    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty(), "CARGO_PKG_VERSION should be set");
    }
}
