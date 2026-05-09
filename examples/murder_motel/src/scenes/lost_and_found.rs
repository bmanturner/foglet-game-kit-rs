//! Lost-and-Found Drawer SPEC §9 proof scene: prompt data, action
//! handler, feedback mapper, and the screen factory the lobby pushes.
//!
//! Each layer is testable in isolation — the prompt builder is pure,
//! the apply handler mutates a [`SharedSlots`] handle, the feedback
//! mapper is a `match` over outcomes, and the screen factory wires
//! all three through a `PromptScreen`.

use foglet_game::{ChoicePrompt, FeedbackLine, PromptAction, PromptScreen, ScreenCommand};

use crate::map::MapScreen;
use crate::state::SharedSlots;

/// Player-facing narration shown above the Lost-and-Found Drawer loot
/// prompt (SPEC_v1_1.md §9 step 1).
///
/// Stored as two body lines because `ChoicePrompt::body` appends one
/// logical line per call and treats them as separate paragraphs the
/// renderer wraps independently. Authoring the text as a constant keeps
/// it close to the prompt builder and lets the test assert the exact
/// string the player sees without re-typing it.
pub const LOST_AND_FOUND_BODY: [&str; 2] = [
    "Behind the desk, the lost-and-found drawer sticks halfway open.",
    "Inside: a tarnished room key tagged \"7\", a cracked matchbook, and a receipt from last night.",
];

/// Stable id returned by [`lost_and_found_drawer_prompt`] when the
/// player picks one of its four options (SPEC_v1_1.md §9 step 2).
///
/// Carries semantics, not display strings: copy edits to the prompt
/// labels MUST NOT cascade into the action handler. Task 10c wires
/// each variant into the corresponding state mutation; Task 10d adds
/// the disabled-`(K)` branch on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAndFoundChoice {
    /// `(K)` — move the Room 7 key into the player's inventory.
    TakeRoom7Key,
    /// `(M)` — pocket the cracked matchbook (shares the catalog id with
    /// the lobby map's matchbook so the two pickup paths can't duplicate
    /// the same keepsake).
    PocketMatchbook,
    /// `(R)` — set the `receipt_read` narrative flag and surface the
    /// receipt body as feedback.
    ReadReceipt,
    /// `(L)` — leave the drawer untouched and exit the prompt.
    Leave,
}

/// Disabled-reason string surfaced when the player presses `(K)` after
/// the Room 7 key is already in their inventory (SPEC_v1_1.md §9 step
/// 3). Centralised so the prompt builder, the runtime feedback line,
/// and the test that pins the contract all reference one source — copy
/// edits to the player-facing reason are a one-line change here.
pub const ROOM_7_KEY_ALREADY_HELD_REASON: &str = "already in inventory";

/// Build the Lost-and-Found Drawer loot prompt with every choice
/// enabled (SPEC_v1_1.md §9 step 2).
///
/// Convenience wrapper around [`lost_and_found_drawer_prompt_with_state`]
/// for the common "fresh-state" case used by Task 10b's data tests and
/// Task 10c's action-handler tests, where the player has not yet picked
/// anything up. Production scene wiring (Task 10f) goes through the
/// state-aware builder so `(K)` greys out automatically once the key is
/// in inventory.
pub fn lost_and_found_drawer_prompt() -> ChoicePrompt<LostAndFoundChoice> {
    lost_and_found_drawer_prompt_with_state(false)
}

/// Build the Lost-and-Found Drawer loot prompt with the disabled-`(K)`
/// branch wired in (SPEC_v1_1.md §9 step 3, Task 10d).
///
/// `has_room_7_key` is a plain bool rather than a `&SharedSlots` borrow
/// so the function stays trivially pure: the caller computes the
/// inventory predicate once at scene-entry and passes it in. That keeps
/// the prompt builder testable without spinning up the runtime, and
/// avoids tangling the renderer with `RefCell` borrow scheduling when
/// the same scene later wants to redraw after a successful pickup
/// flips the predicate.
///
/// `disabled_if` attaches to the **most recently added choice**
/// (`crates/foglet_game/src/prompt.rs:651`), so the call sits
/// immediately after `(K)`. The reducer in `foglet_game` then surfaces
/// the press as [`foglet_game::PromptAction::Disabled`] with the
/// SPEC-mandated reason, ensuring the disabled branch can never reach
/// [`apply_lost_and_found_choice`] and double-insert the key.
pub fn lost_and_found_drawer_prompt_with_state(
    has_room_7_key: bool,
) -> ChoicePrompt<LostAndFoundChoice> {
    let mut prompt = ChoicePrompt::new();
    for line in LOST_AND_FOUND_BODY {
        prompt = prompt.body(line);
    }
    prompt
        .choice('K', LostAndFoundChoice::TakeRoom7Key, "Take the Room 7 key")
        .disabled_if(has_room_7_key, ROOM_7_KEY_ALREADY_HELD_REASON)
        .choice(
            'M',
            LostAndFoundChoice::PocketMatchbook,
            "Pocket the matchbook",
        )
        .choice('R', LostAndFoundChoice::ReadReceipt, "Read the receipt")
        .choice('L', LostAndFoundChoice::Leave, "Leave it alone")
}

/// Result of applying a [`LostAndFoundChoice`] against shared player
/// state (SPEC_v1_1.md §9 step 5).
///
/// Mirrors the prompt's variants one-for-one so callers can branch on
/// what just happened without re-deriving it from the choice. Task 10e
/// pairs each variant with the player-facing feedback line; Task 10d
/// will introduce a separate disabled-`(K)` branch (the prompt itself
/// will return [`foglet_game::PromptAction::Disabled`] in that case, so
/// this enum stays narrowly scoped to *successful* applications).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAndFoundOutcome {
    /// Room 7 key was added to the player's inventory.
    TookRoom7Key,
    /// The cracked matchbook was added to the player's inventory.
    /// Inventory is a `BTreeSet`, so pocketing twice is idempotent —
    /// the player never ends the scene with two matchbooks even if the
    /// lobby pickup also fired.
    PocketedMatchbook,
    /// The `receipt_read` narrative flag is now set.
    ReadReceipt,
    /// Player walked away; no state mutated.
    Left,
}

/// Apply the player's [`LostAndFoundChoice`] to shared runtime state
/// (SPEC_v1_1.md §9 steps 2–3, 5).
///
/// Mutates `slots` directly because every consumer in the example
/// already holds an [`Rc<RefCell<_>>`] handle, so threading a `&mut`
/// view through the call site would just shadow the existing
/// borrow-checked sharing. Returns the typed [`LostAndFoundOutcome`]
/// so the caller (a future scene controller) can drive the post-action
/// feedback line in Task 10e without re-matching on the input choice.
///
/// Task 10c keeps this handler unconditional: pressing `(K)` always
/// inserts the Room 7 key. Task 10d wraps the prompt with the
/// `disabled_if(...)` branch so the disabled press never reaches this
/// function in the first place.
pub fn apply_lost_and_found_choice(
    slots: &SharedSlots,
    choice: LostAndFoundChoice,
) -> LostAndFoundOutcome {
    match choice {
        LostAndFoundChoice::TakeRoom7Key => {
            slots
                .inventory
                .borrow_mut()
                .insert(MapScreen::ROOM_7_KEY_ID.to_string());
            LostAndFoundOutcome::TookRoom7Key
        }
        LostAndFoundChoice::PocketMatchbook => {
            slots
                .inventory
                .borrow_mut()
                .insert(MapScreen::MATCHBOOK_ID.to_string());
            LostAndFoundOutcome::PocketedMatchbook
        }
        LostAndFoundChoice::ReadReceipt => {
            slots
                .flags
                .borrow_mut()
                .insert(MapScreen::RECEIPT_READ_FLAG.to_string());
            LostAndFoundOutcome::ReadReceipt
        }
        LostAndFoundChoice::Leave => LostAndFoundOutcome::Left,
    }
}

/// Player-facing line emitted after a successful `(K)` press
/// (SPEC_v1_1.md §9 step 5). Centralised so the renderer, the test that
/// pins the contract, and any future transcript log share one source.
pub const TOOK_ROOM_7_KEY_FEEDBACK: &str = "Moved Room 7 key to inventory.";

/// Player-facing line emitted after pocketing the cracked matchbook.
/// Mirrors the SPEC §9 step 5 sample format ("Moved X to inventory.")
/// while staying narratively distinct so the player can tell which
/// `(M)` press just registered.
pub const POCKETED_MATCHBOOK_FEEDBACK: &str = "Pocketed the cracked matchbook.";

/// Player-facing line emitted when the player reads the receipt.
///
/// SPEC §9 step 5 only pins the post-action *shape*, not the receipt's
/// wording, so the body lives here as a deliberate copy hook: the
/// receipt is what unlocks the `receipt_read` flag's downstream value
/// (J.M. initials, 23:47 timestamp, cash payment) without forcing the
/// player to memorise it from the prompt.
pub const READ_RECEIPT_FEEDBACK: &str =
    "Receipt: Room 7, paid cash at 23:47 last night. Signed \"J.M.\"";

/// Map a [`LostAndFoundOutcome`] to the player-facing feedback line the
/// scene should display next (SPEC_v1_1.md §9 step 5, Task 10e).
///
/// Returns `None` for [`LostAndFoundOutcome::Left`] because walking away
/// is a deliberate "no narration" path: we do not want a confirmation
/// message implying the drawer remembered the player's hesitation.
/// Every other variant emits a `FeedbackLine` so the caller can drop it
/// straight into the runtime feedback slot without re-matching the
/// outcome.
///
/// All three messages use [`FeedbackLine::info`] rather than `success`:
/// the loot prompt is descriptive narration, not a transactional win,
/// and the SPEC §9 step 5 sample shows no leading marker. The
/// `success` style is reserved for the night-clerk vendor (Task 11)
/// where a coffee purchase reads as an unambiguous positive outcome.
pub fn lost_and_found_feedback(outcome: LostAndFoundOutcome) -> Option<FeedbackLine> {
    match outcome {
        LostAndFoundOutcome::TookRoom7Key => Some(FeedbackLine::info(TOOK_ROOM_7_KEY_FEEDBACK)),
        LostAndFoundOutcome::PocketedMatchbook => {
            Some(FeedbackLine::info(POCKETED_MATCHBOOK_FEEDBACK))
        }
        LostAndFoundOutcome::ReadReceipt => Some(FeedbackLine::info(READ_RECEIPT_FEEDBACK)),
        LostAndFoundOutcome::Left => None,
    }
}

/// Build the [`PromptScreen`] the lobby pushes when the player searches
/// the Lost-and-Found Drawer (SPEC §9, Task 10f).
///
/// Centralising the wiring here keeps the [`crate::map::MapScreen`]
/// input handler a one-liner and lets tests exercise the same factory
/// the runtime uses, so a future regression in the callback's
/// outcome→`ScreenCommand` mapping cannot hide behind a private closure
/// literal.
///
/// The closure clones the supplied [`SharedSlots`] handle so it can
/// outlive the synchronous `handle_input` call: `PromptScreen` keeps the
/// callback alive across frames, and the captured slots reach into the
/// same `RefCell`s the map screen reads — there is only ever one logical
/// inventory/feedback pair.
///
/// Outcome routing:
///
/// - `Selected(_)` applies the choice via [`apply_lost_and_found_choice`],
///   stores the matching [`FeedbackLine`] (if any) in
///   [`SharedSlots::feedback`], and pops back to the lobby. The disabled
///   branch is `(K)`-specific and never reaches `Selected`.
/// - `Disabled { reason, .. }` surfaces the reason as an error feedback
///   line *without* popping — the player is told why the press was
///   rejected and stays in the prompt to make a different choice.
/// - `Cancelled` (Esc) and any-key fall-through (`None`) follow the
///   SPEC §4.4 cancellation contract: pop the prompt, leave state
///   untouched.
pub fn lost_and_found_drawer_screen(slots: SharedSlots) -> PromptScreen<LostAndFoundChoice> {
    let has_room_7_key = slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID);
    let prompt = lost_and_found_drawer_prompt_with_state(has_room_7_key).cancellable(true);
    let callback_slots = slots;
    PromptScreen::new(prompt, move |action| match action {
        PromptAction::Selected(choice) => {
            // Snapshot the matchbook-inventory predicate *before*
            // apply mutates the slot so SPEC_v2 §Task 13c-ii can
            // detect a *new* pickup. The `(K)` branch needs no
            // equivalent — the prompt's disabled-state rule already
            // keeps re-takes out of the action handler.
            let had_matchbook = callback_slots
                .inventory
                .borrow()
                .contains(MapScreen::MATCHBOOK_ID);
            let outcome = apply_lost_and_found_choice(&callback_slots, choice);
            if let Some(event) = crate::world::pending_clue_event_for(outcome, had_matchbook) {
                callback_slots.pending_clue_events.borrow_mut().push(event);
            }
            if let Some(line) = lost_and_found_feedback(outcome) {
                *callback_slots.feedback.borrow_mut() = Some(line);
            }
            ScreenCommand::Pop
        }
        PromptAction::Disabled { reason, .. } => {
            // Disabled hotkeys (currently only `(K)` once the Room 7
            // key is held) keep the prompt open — the player gets an
            // error line explaining the rejection and can pick a
            // different choice without re-opening the drawer.
            if let Some(reason) = reason {
                *callback_slots.feedback.borrow_mut() = Some(FeedbackLine::error(reason));
            }
            ScreenCommand::None
        }
        PromptAction::Cancelled => ScreenCommand::Pop,
        PromptAction::None | PromptAction::ConfirmRequested(_) => ScreenCommand::None,
    })
    .modal()
}

#[cfg(test)]
mod tests {
    //! Tests for the Lost-and-Found Drawer SPEC §9 proof scene: prompt
    //! data, the disabled-(K) branch, the action handler, the feedback
    //! mapper, case-folded hotkeys, and the screen factory's
    //! outcome→`ScreenCommand` routing.

    use super::*;
    use crate::map::MapScreen;
    use crate::state::SharedSlots;
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{GameContext, Input, PromptKey, Screen};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // ---- Prompt data (SPEC §9 Task 10b) -------------------------------

    #[test]
    fn lost_and_found_prompt_exposes_spec_choices_and_body() {
        // SPEC §9 step 2 nails down the prompt's four hotkeys, their
        // labels, and the narration above them. Asserting against the
        // typed prompt (rather than the rendered buffer) catches drift
        // in the data the action handler will read in Task 10c, even
        // before the renderer hooks the prompt into a Screen.
        let prompt = lost_and_found_drawer_prompt();

        assert_eq!(
            prompt.body.as_slice(),
            &LOST_AND_FOUND_BODY[..],
            "narration must match SPEC §9 step 1 verbatim"
        );

        let expected: &[(char, &str, LostAndFoundChoice)] = &[
            ('k', "Take the Room 7 key", LostAndFoundChoice::TakeRoom7Key),
            (
                'm',
                "Pocket the matchbook",
                LostAndFoundChoice::PocketMatchbook,
            ),
            ('r', "Read the receipt", LostAndFoundChoice::ReadReceipt),
            ('l', "Leave it alone", LostAndFoundChoice::Leave),
        ];
        assert_eq!(
            prompt.choices.len(),
            expected.len(),
            "Lost-and-Found Drawer must expose the four SPEC §9 choices"
        );
        for (choice, (key, label, value)) in prompt.choices.iter().zip(expected) {
            assert_eq!(
                choice.key,
                PromptKey::char(*key),
                "hotkey for {label:?} drifted from SPEC §9"
            );
            assert_eq!(choice.label, *label, "label for {key} drifted from SPEC §9");
            assert_eq!(choice.value, *value, "value for {label:?} drifted");
            assert!(
                choice.enabled,
                "Task 10b ships every choice enabled; Task 10d adds the disabled-(K) branch"
            );
        }
    }

    #[test]
    fn lost_and_found_prompt_renders_body_and_hotkeys() {
        // Drive the prompt through Ratatui's `TestBackend` so the test
        // asserts against the same buffer semantics the live runtime
        // uses (SPEC §6 deterministic-render contract). A 70x10 area
        // gives the body two rows and leaves room for the four choice
        // rows + label without forcing the renderer into modal mode.
        let prompt = lost_and_found_drawer_prompt();
        let backend = TestBackend::new(70, 10);
        let mut term = Terminal::new(backend).expect("test backend");
        term.draw(|frame| {
            let area = frame.area();
            prompt.render(area, frame.buffer_mut());
        })
        .expect("draw");

        let buf = term.backend().buffer().clone();
        let mut rendered = String::new();
        for y in 0..10 {
            for x in 0..70 {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }

        // SPEC §9 step 1 narration — the wrapped fragment "lost-and-found
        // drawer" survives the renderer's word-wrap regardless of where
        // the second body line breaks at 70 columns.
        assert!(
            rendered.contains("lost-and-found drawer"),
            "rendered prompt missing SPEC §9 narration; got:\n{rendered}"
        );
        // Each choice row must surface its `(X)` hotkey marker plus the
        // SPEC §9 label so monochrome terminals stay legible.
        for marker in ["(K)", "(M)", "(R)", "(L)"] {
            assert!(
                rendered.contains(marker),
                "missing hotkey marker {marker} in rendered prompt:\n{rendered}"
            );
        }
        for label in [
            "Take the Room 7 key",
            "Pocket the matchbook",
            "Read the receipt",
            "Leave it alone",
        ] {
            assert!(
                rendered.contains(label),
                "missing label {label:?} in rendered prompt:\n{rendered}"
            );
        }
    }

    // ---- Action handler (SPEC §9 Task 10c) ----------------------------

    #[test]
    fn lost_and_found_lowercase_and_uppercase_hotkeys_match() {
        // SPEC §9 step 7: "lowercase and uppercase hotkeys select the
        // same action". Drive the prompt's reducer with each case and
        // assert the typed `Selected(...)` payload is identical, then
        // apply both through `apply_lost_and_found_choice` against fresh
        // slots and assert the resulting state matches.
        use foglet_game::PromptAction;

        let prompt = lost_and_found_drawer_prompt();
        for (lower, upper, expected) in [
            ('k', 'K', LostAndFoundChoice::TakeRoom7Key),
            ('m', 'M', LostAndFoundChoice::PocketMatchbook),
            ('r', 'R', LostAndFoundChoice::ReadReceipt),
            ('l', 'L', LostAndFoundChoice::Leave),
        ] {
            let lower_action = prompt.handle(Input::Char(lower));
            let upper_action = prompt.handle(Input::Char(upper));
            assert!(
                matches!(lower_action, PromptAction::Selected(v) if v == expected),
                "lowercase {lower} must select {expected:?}, got {lower_action:?}"
            );
            assert!(
                matches!(upper_action, PromptAction::Selected(v) if v == expected),
                "uppercase {upper} must select {expected:?}, got {upper_action:?}"
            );

            // Apply both through the action handler from independent
            // slots and snapshot the result. Equal snapshots prove the
            // case folding is end-to-end, not just a `PromptKey`
            // cosmetic match.
            let lower_slots = SharedSlots::default();
            lower_slots.reset(0, 0);
            let upper_slots = SharedSlots::default();
            upper_slots.reset(0, 0);
            let lower_outcome = apply_lost_and_found_choice(&lower_slots, expected);
            let upper_outcome = apply_lost_and_found_choice(&upper_slots, expected);
            assert_eq!(
                lower_outcome, upper_outcome,
                "outcome must not depend on hotkey case"
            );
            assert_eq!(
                lower_slots.snapshot(),
                upper_slots.snapshot(),
                "post-apply state must not depend on hotkey case for {expected:?}"
            );
        }
    }

    #[test]
    fn lost_and_found_apply_take_key_inserts_room_7_key() {
        // Direct unit on the action handler so a regression in
        // `TakeRoom7Key` points here, not at the higher-level
        // case-folding test. Starts from a fresh `reset` baseline so
        // the assertion isolates the mutation.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        assert!(!slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID));

        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::TakeRoom7Key);

        assert_eq!(outcome, LostAndFoundOutcome::TookRoom7Key);
        assert!(
            slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID),
            "TakeRoom7Key must add ROOM_7_KEY_ID to inventory"
        );
        // Other slots stay untouched — the handler is single-purpose.
        assert!(!slots.inventory.borrow().contains(MapScreen::MATCHBOOK_ID));
        assert!(!slots.flags.borrow().contains(MapScreen::RECEIPT_READ_FLAG));
    }

    #[test]
    fn lost_and_found_apply_pocket_matchbook_inserts_matchbook() {
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::PocketMatchbook);
        assert_eq!(outcome, LostAndFoundOutcome::PocketedMatchbook);
        assert!(
            slots.inventory.borrow().contains(MapScreen::MATCHBOOK_ID),
            "PocketMatchbook must add MATCHBOOK_ID to inventory"
        );
    }

    #[test]
    fn lost_and_found_apply_pocket_matchbook_is_idempotent() {
        // SPEC §9 comment on `LostAndFoundChoice::PocketMatchbook`: the
        // drawer and the lobby map share `MATCHBOOK_ID`, so pocketing
        // twice (or pocketing after lobby pickup) MUST NOT duplicate
        // the keepsake. `BTreeSet::insert` enforces this; the test
        // pins the contract so a future `Vec`-based refactor cannot
        // silently break it.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::MATCHBOOK_ID.to_string());
        apply_lost_and_found_choice(&slots, LostAndFoundChoice::PocketMatchbook);
        let count = slots
            .inventory
            .borrow()
            .iter()
            .filter(|id| id.as_str() == MapScreen::MATCHBOOK_ID)
            .count();
        assert_eq!(count, 1, "matchbook must remain unique in inventory");
    }

    #[test]
    fn lost_and_found_apply_read_receipt_sets_flag() {
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::ReadReceipt);
        assert_eq!(outcome, LostAndFoundOutcome::ReadReceipt);
        assert!(
            slots.flags.borrow().contains(MapScreen::RECEIPT_READ_FLAG),
            "ReadReceipt must set RECEIPT_READ_FLAG"
        );
        // Reading the receipt is purely a flag mutation; inventory
        // stays empty so a future scene can't conflate "read" with
        // "took the receipt as an item".
        assert!(slots.inventory.borrow().is_empty());
    }

    // ---- Disabled-(K) branch (SPEC §9 Task 10d) -----------------------

    #[test]
    fn lost_and_found_prompt_disables_take_key_when_already_held() {
        // SPEC §9 step 3: "If the player already has the Room 7 key,
        // `(K)` renders disabled with reason `already in inventory`."
        // Inspect the typed prompt rather than the buffer so the test
        // pins the data contract (the renderer test above already
        // covers visual surfacing of disabled rows via the shared
        // `ChoicePrompt::render` path in `foglet_game`).
        let prompt = lost_and_found_drawer_prompt_with_state(true);

        let take_key = prompt
            .choices
            .iter()
            .find(|c| c.value == LostAndFoundChoice::TakeRoom7Key)
            .expect("(K) choice must still be present so the player sees why it's locked");
        assert!(
            !take_key.enabled,
            "(K) must be disabled when the Room 7 key is already in inventory"
        );
        assert_eq!(
            take_key.disabled_reason.as_deref(),
            Some(ROOM_7_KEY_ALREADY_HELD_REASON),
            "disabled reason must match SPEC §9 step 3 verbatim"
        );

        // Sibling choices stay enabled — Task 10d only gates `(K)`, not
        // the rest of the drawer's affordances.
        for value in [
            LostAndFoundChoice::PocketMatchbook,
            LostAndFoundChoice::ReadReceipt,
            LostAndFoundChoice::Leave,
        ] {
            let choice = prompt
                .choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value:?} choice missing"));
            assert!(
                choice.enabled,
                "{value:?} must remain enabled when only (K) is gated"
            );
            assert!(choice.disabled_reason.is_none());
        }
    }

    #[test]
    fn lost_and_found_prompt_take_key_stays_enabled_without_room_7_key() {
        // Mirror of the disabled case so a future regression that
        // accidentally inverts the condition lights up here, not in a
        // downstream Murder Motel smoke test.
        let prompt = lost_and_found_drawer_prompt_with_state(false);
        let take_key = prompt
            .choices
            .iter()
            .find(|c| c.value == LostAndFoundChoice::TakeRoom7Key)
            .expect("(K) choice present");
        assert!(take_key.enabled);
        assert!(take_key.disabled_reason.is_none());
    }

    #[test]
    fn lost_and_found_pressing_disabled_take_key_does_not_duplicate_item() {
        // SPEC §9 step 3 + Task 10d test directive: "pressing `k` when
        // disabled returns disabled message and does not duplicate the
        // item." We stage a slot with the key already present, build
        // the state-aware prompt, drive both lowercase and uppercase
        // hotkeys through the reducer, assert the typed
        // `PromptAction::Disabled` payload, and confirm the inventory
        // count stays at one.
        use foglet_game::PromptAction;

        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let before = slots.snapshot();

        let prompt = lost_and_found_drawer_prompt_with_state(true);

        for key in ['k', 'K'] {
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Disabled { reason, .. } => assert_eq!(
                    reason.as_deref(),
                    Some(ROOM_7_KEY_ALREADY_HELD_REASON),
                    "disabled reason for {key} drifted from SPEC §9"
                ),
                other => panic!("expected PromptAction::Disabled, got {other:?} for {key}"),
            }
        }

        // The reducer never reached `apply_lost_and_found_choice`, so
        // state must be byte-identical to the pre-press snapshot — no
        // ghost matchbook, no flag flips, and crucially no second
        // ROOM_7_KEY_ID inserted (BTreeSet would dedupe anyway, but a
        // future Vec-based inventory must not regress this).
        assert_eq!(
            slots.snapshot(),
            before,
            "disabled (K) press must not mutate any slot"
        );
        let count = slots
            .inventory
            .borrow()
            .iter()
            .filter(|id| id.as_str() == MapScreen::ROOM_7_KEY_ID)
            .count();
        assert_eq!(
            count, 1,
            "Room 7 key must not be duplicated by a disabled press"
        );
    }

    // ---- Feedback messages (SPEC §9 Task 10e) -------------------------

    #[test]
    fn lost_and_found_feedback_messages_match_spec_strings() {
        // SPEC §9 step 5 pins the take-key wording verbatim. The
        // matchbook and receipt strings live in the example, but Task
        // 10e calls them out by name in the checklist; pin all three so
        // a copy edit forces an explicit checklist update.
        assert_eq!(TOOK_ROOM_7_KEY_FEEDBACK, "Moved Room 7 key to inventory.");
        assert_eq!(
            POCKETED_MATCHBOOK_FEEDBACK,
            "Pocketed the cracked matchbook."
        );
        assert!(
            READ_RECEIPT_FEEDBACK.contains("Room 7"),
            "receipt feedback should mention Room 7 to reward the read"
        );
    }

    #[test]
    fn lost_and_found_feedback_take_key_emits_take_message() {
        // Action handler returns `TookRoom7Key`; the feedback mapper
        // MUST surface the SPEC §9 step 5 line. Using `rendered_text`
        // also pins the absence of a leading marker (Info kind).
        let line = lost_and_found_feedback(LostAndFoundOutcome::TookRoom7Key)
            .expect("TookRoom7Key must emit a feedback line");
        assert_eq!(line.text(), TOOK_ROOM_7_KEY_FEEDBACK);
        assert_eq!(line.rendered_text(), TOOK_ROOM_7_KEY_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_pocket_matchbook_emits_pocket_message() {
        let line = lost_and_found_feedback(LostAndFoundOutcome::PocketedMatchbook)
            .expect("PocketedMatchbook must emit a feedback line");
        assert_eq!(line.text(), POCKETED_MATCHBOOK_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_read_receipt_emits_receipt_text() {
        let line = lost_and_found_feedback(LostAndFoundOutcome::ReadReceipt)
            .expect("ReadReceipt must emit a feedback line");
        assert_eq!(line.text(), READ_RECEIPT_FEEDBACK);
    }

    #[test]
    fn lost_and_found_feedback_leave_emits_no_message() {
        // Walking away is intentionally silent; emitting a "You walked
        // away." line would imply state mutation the (L) branch
        // explicitly avoids (Task 10e doc + Task 10c noop guarantee).
        assert!(lost_and_found_feedback(LostAndFoundOutcome::Left).is_none());
    }

    #[test]
    fn lost_and_found_feedback_pairs_with_apply_outcome_end_to_end() {
        // End-to-end pin: apply each enabled choice against a fresh
        // SharedSlots, feed the outcome through the feedback mapper,
        // and assert the resulting text. Catches regressions where
        // `apply_lost_and_found_choice` and `lost_and_found_feedback`
        // drift apart on the variant->message contract.
        let cases = [
            (LostAndFoundChoice::TakeRoom7Key, TOOK_ROOM_7_KEY_FEEDBACK),
            (
                LostAndFoundChoice::PocketMatchbook,
                POCKETED_MATCHBOOK_FEEDBACK,
            ),
            (LostAndFoundChoice::ReadReceipt, READ_RECEIPT_FEEDBACK),
        ];
        for (choice, expected) in cases {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let outcome = apply_lost_and_found_choice(&slots, choice);
            let line = lost_and_found_feedback(outcome)
                .unwrap_or_else(|| panic!("{choice:?} should produce feedback"));
            assert_eq!(line.text(), expected, "feedback drift for {choice:?}");
        }
    }

    #[test]
    fn lost_and_found_apply_leave_is_a_noop() {
        // SPEC §9 step 2: "(L) Leave it alone" exits the prompt without
        // mutating state. Snapshot before/after equality is the
        // strongest possible assertion that no slot was touched.
        let slots = SharedSlots::default();
        slots.reset(7, 3);
        let before = slots.snapshot();
        let outcome = apply_lost_and_found_choice(&slots, LostAndFoundChoice::Leave);
        assert_eq!(outcome, LostAndFoundOutcome::Left);
        assert_eq!(
            slots.snapshot(),
            before,
            "Leave must not mutate inventory, flags, or player state"
        );
    }

    // ---- Screen factory wiring (SPEC §9 Task 10f) ---------------------

    #[test]
    fn drawer_screen_callback_take_key_writes_feedback_and_pops() {
        // Drive the same `PromptScreen` the runtime uses, scripting a
        // `(K)` press through `handle_input` and asserting the captured
        // side effects: the Room 7 key lands in inventory, the feedback
        // slot carries the SPEC §9 step 5 line, and the screen returns
        // `Pop` so the lobby comes back into focus.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('k'));
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert!(
            slots.inventory.borrow().contains(MapScreen::ROOM_7_KEY_ID),
            "(K) must move the Room 7 key into the inventory"
        );
        let feedback = slots.feedback.borrow().clone().expect("feedback set");
        assert_eq!(feedback.text(), TOOK_ROOM_7_KEY_FEEDBACK);
    }

    #[test]
    fn drawer_screen_callback_disabled_take_key_writes_error_and_stays_open() {
        // Pre-load the key so the prompt's `(K)` row builds disabled,
        // then verify the callback surfaces the SPEC §9 reason as an
        // error feedback line and emits `None` so the prompt stays on
        // screen for a different choice.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        slots
            .inventory
            .borrow_mut()
            .insert(MapScreen::ROOM_7_KEY_ID.to_string());
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('k'));
        assert!(
            matches!(cmd, ScreenCommand::None),
            "disabled press must keep the prompt open"
        );
        let feedback = slots
            .feedback
            .borrow()
            .clone()
            .expect("disabled press must surface a reason");
        assert_eq!(feedback.text(), ROOM_7_KEY_ALREADY_HELD_REASON);
    }

    #[test]
    fn drawer_screen_callback_take_key_queues_clue_found_event() {
        // SPEC_v2 §Task 13c-ii: when the drawer's `(K)` press lands the
        // Room 7 key in inventory, the callback must enqueue a
        // PendingClueEvent so the next lobby tick can write a
        // `clue_found` row to world_events.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        assert!(
            slots.pending_clue_events.borrow().is_empty(),
            "fresh slots start with an empty mailbox"
        );
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('k'));
        assert!(matches!(cmd, ScreenCommand::Pop));

        let queue = slots.pending_clue_events.borrow();
        assert_eq!(
            queue.len(),
            1,
            "Room 7 key pickup must enqueue exactly one pending event"
        );
        assert_eq!(queue[0].item_id, MapScreen::ROOM_7_KEY_ID);
        assert_eq!(
            queue[0].message,
            crate::world::CLUE_FOUND_ROOM_7_KEY_MESSAGE
        );
    }

    #[test]
    fn drawer_screen_callback_pocket_matchbook_only_queues_first_time() {
        // First press queues an event; second press (with the
        // matchbook now in inventory) must not — otherwise repeated
        // (M) presses would multiply the same keepsake in the
        // bulletin.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let cfg = fixture_config();
        let fc = fixture_context();

        // First press through a fresh prompt: the callback sees an
        // empty inventory and enqueues a clue_found event.
        let mut screen = lost_and_found_drawer_screen(slots.clone());
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let _ = screen.handle_input(&mut ctx, Input::Char('m'));
        assert_eq!(
            slots.pending_clue_events.borrow().len(),
            1,
            "first matchbook pickup must enqueue an event"
        );

        // Drain the mailbox to mimic the lobby tick consuming the
        // queued row before the player presses (M) again.
        slots.pending_clue_events.borrow_mut().clear();

        // Second press: inventory already contains the matchbook, so
        // the callback must observe `had_matchbook = true` and skip
        // the enqueue. Build a *new* drawer screen so the prompt
        // state-aware predicate (currently only gates `(K)`) is rebuilt
        // — even with `(M)` enabled, the callback's pre-snapshot must
        // still detect the duplicate.
        let mut screen = lost_and_found_drawer_screen(slots.clone());
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let _ = screen.handle_input(&mut ctx, Input::Char('m'));
        assert!(
            slots.pending_clue_events.borrow().is_empty(),
            "re-pressing (M) once the matchbook is held must not re-queue an event"
        );
    }

    #[test]
    fn drawer_screen_callback_read_receipt_does_not_queue_event() {
        // The receipt sets a narrative flag, not a clue item — Task
        // 13c-ii's bulletin tracks inventory clues, so reading the
        // receipt must leave the mailbox empty.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let mut screen = lost_and_found_drawer_screen(slots.clone());
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let _ = screen.handle_input(&mut ctx, Input::Char('r'));
        assert!(slots.pending_clue_events.borrow().is_empty());
    }

    #[test]
    fn drawer_screen_callback_cancel_pops_without_mutating_state() {
        // Esc on a cancellable prompt is the SPEC §4.4 "back out" gesture.
        // The lobby must come back unchanged — no key, no flag, no
        // matchbook bonus from a hesitant press.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let mut screen = lost_and_found_drawer_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(slots.snapshot(), before);
        assert!(slots.feedback.borrow().is_none());
    }
}
