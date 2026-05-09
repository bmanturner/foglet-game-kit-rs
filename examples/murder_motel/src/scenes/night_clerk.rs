//! Night-clerk vendor SPEC §9 proof scene: vendor prompt, the
//! disabled-(T) and no-thanks branches, the post-purchase any-key
//! continuation screen, and the lobby screen factory that ties them
//! together.

use foglet_game::{
    AnyKeyOutcome, AnyKeyPrompt, ChoicePrompt, FeedbackLine, GameContext, Input, PromptAction,
    PromptScreen, Screen, ScreenCommand,
};
use ratatui::Frame;

use crate::state::SharedSlots;

/// Player-facing narration shown above the night-clerk vendor prompt
/// (SPEC_v1_1.md §9 step 4).
///
/// Two body lines mirror the SPEC's two-paragraph framing so the
/// renderer can wrap them independently — the first sets the scene,
/// the second drops the pricing-fueled barb. Authoring the lines as a
/// constant keeps the wording version-controlled and lets the data
/// test below pin it byte-for-byte without re-typing the SPEC quote.
pub const NIGHT_CLERK_VENDOR_BODY: [&str; 2] = [
    "The night clerk drums his fingers beside a locked cigar box.",
    "\"Evidence costs extra after midnight.\"",
];

/// Stable id returned by [`night_clerk_vendor_prompt`] when the player
/// picks one of its three options (SPEC_v1_1.md §9 step 4).
///
/// Carries semantics, not display strings, so Task 11b's dynamic
/// label refactor (price/cash interpolation) and Task 11c's transaction
/// handler can both branch on the variant without re-parsing the
/// rendered label. The enum mirrors SPEC §9 step 4's three-row layout
/// one-for-one — there is intentionally no separate "leave" variant
/// because [`Self::NoThanks`] already serves that role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightClerkVendorChoice {
    /// `(B)` — pay the coffee price (Task 11c will decrement cash by
    /// [`COFFEE_PRICE`] gold and emit a success feedback line).
    BuyCoffee,
    /// `(T)` — tip for a rumor; Task 11d disables this row when the
    /// player has fewer than [`RUMOR_TIP_PRICE`] gold and surfaces the
    /// `need 50g` reason.
    TipForRumor,
    /// `(N)` — close the prompt without spending anything.
    NoThanks,
}

/// Cost of the night clerk's black coffee, in gold pieces
/// (SPEC_v1_1.md §9 step 4). Centralised so Task 11b's dynamic label
/// and Task 11c's transaction handler share one source of truth — a
/// future copy-edit to "30g" only changes the constant, never the
/// branching logic.
pub const COFFEE_PRICE: u32 = 25;

/// Cost of the rumor tip, in gold pieces (SPEC_v1_1.md §9 step 4).
/// Pairs with [`PlayerSlot::STARTING_CASH`] (40g) so the proof scene
/// always boots into the disabled-`(T)` branch Task 11d wires up.
pub const RUMOR_TIP_PRICE: u32 = 50;

/// Disabled-row reason shown next to `(T) Tip the clerk for a rumor`
/// when the player's wallet is below [`RUMOR_TIP_PRICE`]
/// (SPEC_v1_1.md §9 step 4: "Tipping is disabled when the player lacks
/// enough cash and shows `need 50g`"). Centralised so the prompt
/// builder, the disabled-press regression test, and any future copy
/// edit share one source of truth — the SPEC pins the wording verbatim,
/// so a future bump of `RUMOR_TIP_PRICE` deliberately invalidates this
/// constant rather than silently desyncing the player-facing string.
pub const NEED_RUMOR_TIP_REASON: &str = "need 50g";

/// Format the priced-row label for the night-clerk vendor prompt
/// (SPEC_v1_1.md §9 step 4, Task 11b).
///
/// The SPEC pins the shape `"<action>: <price>g | You have: <cash>g"`
/// — a single source of truth keeps `Buy a black coffee` and
/// `Tip the clerk for a rumor` in lockstep so a copy-edit to the
/// separator (`|`) only touches one place. Returning `String` (rather
/// than `Cow<'static, str>`) is intentional: the cash component is
/// dynamic on every render, so the allocation is unavoidable.
fn priced_vendor_label(action: &str, price: u32, cash: u32) -> String {
    format!("{action}: {price}g | You have: {cash}g")
}

/// Build the night-clerk vendor prompt with dynamic price/cash hints
/// (SPEC_v1_1.md §9 step 4, Task 11b).
///
/// `cash` is the player's current gold balance, which the priced rows
/// (`(B)` and `(T)`) splice into their labels in the SPEC's
/// `"<action>: <price>g | You have: <cash>g"` shape. Re-calling this
/// function after a transaction is the supported path for refreshing
/// the labels — the prompt itself does not retain a reference to the
/// player's wallet, which keeps the data model a value type and lets
/// every reducer test pin a deterministic balance without plumbing
/// shared state through `Rc<RefCell<_>>`.
///
/// `(N) No thanks` keeps a static label because the SPEC's reference
/// block deliberately omits a price annotation for the leave path —
/// adding one would imply a cost the row does not actually charge.
///
/// Task 11d gates the `(T)` row with `disabled_if(cash < RUMOR_TIP_PRICE,
/// NEED_RUMOR_TIP_REASON)` so the disabled-`(T)` branch fires whenever
/// the player can't actually afford the tip. The reducer then surfaces
/// the press as [`foglet_game::PromptAction::Disabled`], which means a
/// disabled tip can never reach [`apply_night_clerk_vendor_choice`] and
/// silently decrement cash. `(B)` stays unconditionally enabled — the
/// 25g coffee always sits within reach of the 40g
/// [`PlayerSlot::STARTING_CASH`] floor, so a separate gate would just be
/// unreachable code.
///
/// The choice ordering matches SPEC §9 step 4 verbatim (`B`, `T`, `N`)
/// so the rendered prompt reads top-to-bottom in the same order an
/// operator scanning the SPEC's reference block would expect.
pub fn night_clerk_vendor_prompt(cash: u32) -> ChoicePrompt<NightClerkVendorChoice> {
    let mut prompt = ChoicePrompt::new();
    for line in NIGHT_CLERK_VENDOR_BODY {
        prompt = prompt.body(line);
    }
    prompt
        .choice(
            'B',
            NightClerkVendorChoice::BuyCoffee,
            priced_vendor_label("Buy a black coffee", COFFEE_PRICE, cash),
        )
        .choice(
            'T',
            NightClerkVendorChoice::TipForRumor,
            priced_vendor_label("Tip the clerk for a rumor", RUMOR_TIP_PRICE, cash),
        )
        // `disabled_if` attaches to the most-recently-added choice, so
        // this call must sit immediately after the `(T)` row. The
        // reducer in `foglet_game` then routes the disabled press to
        // `PromptAction::Disabled` with NEED_RUMOR_TIP_REASON — the
        // apply handler never sees a TipForRumor it couldn't afford.
        .disabled_if(cash < RUMOR_TIP_PRICE, NEED_RUMOR_TIP_REASON)
        .choice('N', NightClerkVendorChoice::NoThanks, "No thanks")
}

/// Result of applying a [`NightClerkVendorChoice`] against shared
/// player state (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Mirrors the prompt's variants one-for-one *only as they get wired
/// up* — Task 11c lands [`Self::BoughtCoffee`], Task 11e adds the
/// no-thanks path, and Task 11d will route the `(T)` row through a
/// disabled-prompt branch (so it never reaches this enum). Keeping the
/// enum narrowly scoped to *successful, state-mutating* outcomes mirrors
/// [`crate::scenes::lost_and_found::LostAndFoundOutcome`] and lets the
/// feedback helper return a single [`FeedbackLine`] without juggling
/// `Option<Option<…>>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NightClerkVendorOutcome {
    /// `(B)` succeeded: the coffee price was deducted from the player's
    /// wallet and the success feedback line should be surfaced. Task
    /// 11c is the *only* path that produces this variant — Task 11f's
    /// any-key continuation pipes it through unchanged.
    BoughtCoffee,
}

/// Player-facing line emitted after a successful `(B)` press
/// (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Centralised so the renderer, the test that pins the contract, and
/// any future transcript log share one source. Wording bakes in the
/// price so the player sees the deduction confirmed in the same line
/// — handy when the wallet field is off-screen during the prompt
/// dismissal.
pub const BOUGHT_COFFEE_FEEDBACK: &str = "Bought a black coffee for 25g.";

/// Apply the player's [`NightClerkVendorChoice`] to shared runtime state
/// (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Returns `Some(outcome)` for the *implemented* mutating paths and
/// `None` for the still-unscoped rows so future tasks (11d, 11e) can
/// fill in their behaviour without rewriting this signature. Task 11c
/// lands `BuyCoffee`: cash decrements by [`COFFEE_PRICE`] and the caller
/// surfaces the success feedback. The other variants intentionally
/// return `None` for now — Task 11d disables `(T)` upstream (so it
/// never reaches this handler) and Task 11e wires `(N)` to a
/// no-state-change exit.
///
/// Mutates `slots` directly to mirror
/// [`crate::scenes::lost_and_found::apply_lost_and_found_choice`]'s
/// shape — every consumer already holds [`SharedSlots`] handles, so a
/// `&mut`-style API would just shadow the existing `RefCell` sharing.
///
/// Cash is decremented with plain `-=` (not `saturating_sub`) so an
/// unexpected underflow is a loud panic in debug builds rather than a
/// silent wrap to zero. Task 11d's disabled-`(T)` branch is the
/// gatekeeper for tip affordability; Task 11c's coffee at 25g is
/// always within reach of the 40g [`PlayerSlot::STARTING_CASH`] floor,
/// so no separate `(B)` gate exists.
pub fn apply_night_clerk_vendor_choice(
    slots: &SharedSlots,
    choice: NightClerkVendorChoice,
) -> Option<NightClerkVendorOutcome> {
    match choice {
        NightClerkVendorChoice::BuyCoffee => {
            let mut player = slots.player.borrow_mut();
            player.cash -= COFFEE_PRICE;
            Some(NightClerkVendorOutcome::BoughtCoffee)
        }
        // Task 11d gates `(T)` at the prompt layer (disabled row), so
        // it should never reach the apply handler. Task 11e will add
        // the `(N)` no-state-change exit. Returning `None` keeps the
        // match exhaustive without pretending to mutate state.
        NightClerkVendorChoice::TipForRumor | NightClerkVendorChoice::NoThanks => None,
    }
}

/// Map a [`NightClerkVendorOutcome`] to the player-facing feedback line
/// the scene should display next (SPEC_v1_1.md §9 step 4, Task 11c).
///
/// Uses [`FeedbackLine::success`] (not `info`) because the coffee
/// purchase is an unambiguous transactional win — the SPEC explicitly
/// reserves the success register for the vendor scene to distinguish
/// it from the Lost-and-Found Drawer's descriptive narration. Returns
/// `Option` so the type stays parallel with
/// [`crate::scenes::lost_and_found::lost_and_found_feedback`], leaving
/// room for a future no-narration variant without re-shaping the call
/// sites.
pub fn night_clerk_vendor_feedback(outcome: NightClerkVendorOutcome) -> Option<FeedbackLine> {
    match outcome {
        NightClerkVendorOutcome::BoughtCoffee => {
            Some(FeedbackLine::success(BOUGHT_COFFEE_FEEDBACK))
        }
    }
}

/// Body line shown on the any-key continuation prompt that follows a
/// successful night-clerk vendor interaction (SPEC_v1_1.md §9 step 6,
/// Task 11f).
///
/// Matches the SPEC verbatim so a doc-driven reader can grep the
/// example for the phrase quoted in the spec and find a single source
/// of truth. The continuation prompt's footer falls back to
/// [`AnyKeyPrompt`]'s default `"Press any key to continue..."` cue, so
/// only the narration line lives here.
pub const NIGHT_CLERK_CONTINUATION_BODY: &str = "The clerk glances toward the staircase.";

/// Build the any-key continuation prompt shown after a vendor
/// interaction (SPEC_v1_1.md §9 step 6, Task 11f).
///
/// Wraps the SPEC narration line in an [`AnyKeyPrompt`] so the prompt
/// keeps the SPEC §4.6 default footer (`"Press any key to continue..."`)
/// without re-stating it here. Returning the bare prompt — rather than
/// the screen — lets tests inspect the body lines without driving a
/// full [`Screen`] cycle, mirroring how [`night_clerk_vendor_prompt`]
/// hands back a [`ChoicePrompt`] for the same reason.
pub fn night_clerk_continuation_prompt() -> AnyKeyPrompt {
    AnyKeyPrompt::new().body(NIGHT_CLERK_CONTINUATION_BODY)
}

/// `Screen` adapter wrapping [`night_clerk_continuation_prompt`] so the
/// vendor flow can push a "press any key to dismiss" pause after a
/// successful purchase (SPEC_v1_1.md §9 step 6, Task 11f).
///
/// We hand-roll this rather than reusing [`PromptScreen`] because
/// `PromptScreen` is parameterised on a [`ChoicePrompt`]; an any-key
/// pause has no choice list to drive. The screen is intentionally
/// state-free — the prompt is recomputed each frame from the
/// [`AnyKeyPrompt`] reducer, and the screen owns no game state, so a
/// `Default` impl is the natural constructor. The vendor flow surfaces
/// the post-purchase [`FeedbackLine`] *before* pushing this screen, so
/// the continuation just needs to dismiss itself when the player
/// acknowledges it.
///
/// Outcome routing per SPEC §4.6:
///
/// - [`Input::Resize`] / [`Input::Unknown`] → [`ScreenCommand::None`].
///   The pause stays put while the terminal re-lays out, so the player
///   never loses the cue to a stray geometry event.
/// - Any other input → [`ScreenCommand::Pop`]. The runtime pops the
///   continuation back to whatever pushed it (the vendor screen or the
///   map), satisfying the Task 11f acceptance "any meaningful key
///   returns to prior screen/map".
#[derive(Debug, Default)]
pub struct NightClerkContinuationScreen {
    prompt: AnyKeyPrompt,
}

impl NightClerkContinuationScreen {
    /// Build a fresh continuation screen with the SPEC §9 step 6 body.
    ///
    /// Provided as an explicit constructor (alongside the derived
    /// `Default`) so call sites in the vendor flow read as
    /// `NightClerkContinuationScreen::new()` rather than relying on
    /// `Default::default()` — the latter reads ambiguously when the
    /// surrounding code already builds several other screens via
    /// `::new()` factories.
    pub fn new() -> Self {
        Self {
            prompt: night_clerk_continuation_prompt(),
        }
    }
}

impl Screen for NightClerkContinuationScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        // Compact unboxed render — the continuation is a momentary
        // narration beat, not a confirmation gate, so the bordered
        // modal would over-emphasise it. Mirrors how SPEC §9 step 6
        // formats the example with no surrounding box.
        let area = frame.area();
        let buf = frame.buffer_mut();
        self.prompt.render(area, buf);
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        match self.prompt.handle(input) {
            // SPEC §4.6 says resize is ignored; mapping `None` straight
            // through to `ScreenCommand::None` keeps the pause on
            // screen until the player acknowledges it.
            AnyKeyOutcome::None => ScreenCommand::None,
            // Any meaningful key dismisses the pause. Pop returns to
            // whatever pushed the continuation, which is the vendor
            // screen for the SPEC §9 step 6 flow but nothing here
            // bakes that assumption in — a future caller can push the
            // continuation from anywhere.
            AnyKeyOutcome::Completed => ScreenCommand::Pop,
        }
    }
}

/// Build the [`PromptScreen`] the lobby pushes when the player asks
/// the Night Clerk to buy something (SPEC_v1_1.md §9 step 4, Task 11g).
///
/// Mirrors [`crate::scenes::lost_and_found::lost_and_found_drawer_screen`]
/// in shape so the two SPEC §9 proof scenes share an integration
/// pattern: a factory that closes over [`SharedSlots`] and routes
/// [`PromptAction`] outcomes to [`ScreenCommand`]s. Centralising the
/// wiring here keeps the [`crate::map::MapScreen`] handler one line per
/// affordance and lets unit tests drive the same factory the runtime
/// uses.
///
/// The closure clones the supplied [`SharedSlots`] handle so it can
/// outlive each synchronous `handle_input` call: `PromptScreen` stores
/// the callback across frames, and the captured slots reach into the
/// same `RefCell`s the map screen reads.
///
/// Outcome routing per SPEC §9 step 4–6:
///
/// - `Selected(BuyCoffee)` decrements cash via
///   [`apply_night_clerk_vendor_choice`], stores the success
///   [`FeedbackLine`] in [`SharedSlots::feedback`], and
///   [`ScreenCommand::Replace`]s itself with a
///   [`NightClerkContinuationScreen`]. `Replace` (rather than `Pop`
///   then `Push`) ensures the continuation does not stack *on top of*
///   the vendor — when the player presses any key on the continuation,
///   they pop straight back to the map, not to a leftover vendor
///   prompt.
/// - `Selected(NoThanks)` pops without state change. Task 11e nailed
///   the no-mutation invariant; reusing the same path here keeps the
///   exit cheap.
/// - `Selected(TipForRumor)` is unreachable in practice — Task 11d
///   gates the row at the prompt layer — but the match must stay
///   exhaustive. We pop on this arm to preserve the "no leak" property
///   if a future authoring mistake ever lets the choice through.
/// - `Disabled { reason, .. }` writes the reason as an error feedback
///   line and stays on the prompt. Same UX contract as the drawer's
///   disabled-`(K)` path: the player learns *why* the press was
///   rejected and can pick a different choice without re-opening the
///   vendor menu.
/// - `Cancelled` (Esc) and `None` follow SPEC §4.4: pop on cancel,
///   stay put on no-op. The vendor prompt is built `cancellable(true)`
///   so a player who opens it by accident can back out without
///   spending gold or hunting for the `[N]` row.
pub fn night_clerk_vendor_screen(slots: SharedSlots) -> PromptScreen<NightClerkVendorChoice> {
    let cash = slots.player.borrow().cash;
    let prompt = night_clerk_vendor_prompt(cash).cancellable(true);
    let callback_slots = slots;
    PromptScreen::new(prompt, move |action| match action {
        PromptAction::Selected(NightClerkVendorChoice::BuyCoffee) => {
            let outcome =
                apply_night_clerk_vendor_choice(&callback_slots, NightClerkVendorChoice::BuyCoffee);
            if let Some(line) = outcome.and_then(night_clerk_vendor_feedback) {
                *callback_slots.feedback.borrow_mut() = Some(line);
            }
            ScreenCommand::Replace(Box::new(NightClerkContinuationScreen::new()))
        }
        PromptAction::Selected(NightClerkVendorChoice::NoThanks)
        | PromptAction::Selected(NightClerkVendorChoice::TipForRumor) => ScreenCommand::Pop,
        PromptAction::Disabled { reason, .. } => {
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
    //! Tests for the SPEC §9 night-clerk vendor proof scene: prompt
    //! data, the disabled-(T) and no-thanks branches, the post-purchase
    //! continuation screen, and the screen factory's
    //! outcome→`ScreenCommand` routing.

    use super::*;
    use crate::state::{PlayerSlot, SharedSlots};
    use crate::test_support::{fixture_config, fixture_context};
    use foglet_game::{GameContext, Input, PromptAction, PromptKey, Screen};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    // ---- Prompt data (SPEC §9 Task 11a/11b) ---------------------------

    #[test]
    fn night_clerk_vendor_prompt_exposes_spec_choices_and_body() {
        // SPEC §9 step 4 pins the prompt's three hotkeys, their labels,
        // and the two-line narration above them. Asserting against the
        // typed prompt (rather than the rendered buffer) catches drift
        // in the data Task 11b will read for dynamic labels and Task
        // 11c for the transaction handler, before the renderer hooks
        // the prompt into a Screen.

        // Pin the prompt to the SPEC's reference balance (40g) so the
        // priced labels match the example block byte-for-byte. The
        // dynamic-label coverage lives in the dedicated test below.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);

        assert_eq!(
            prompt.body.as_slice(),
            &NIGHT_CLERK_VENDOR_BODY[..],
            "narration must match SPEC §9 step 4 verbatim"
        );

        // SPEC §9 step 4 pins the priced-row layout. After Task 11d the
        // `(T)` row is disabled at STARTING_CASH=40g (40 < 50g rumor
        // price), so the expectation table tracks the post-11d enabled
        // flag per row instead of asserting blanket enabled-ness.
        let expected: &[(char, &str, NightClerkVendorChoice, bool)] = &[
            (
                'b',
                "Buy a black coffee: 25g | You have: 40g",
                NightClerkVendorChoice::BuyCoffee,
                true,
            ),
            (
                't',
                "Tip the clerk for a rumor: 50g | You have: 40g",
                NightClerkVendorChoice::TipForRumor,
                false,
            ),
            ('n', "No thanks", NightClerkVendorChoice::NoThanks, true),
        ];
        assert_eq!(
            prompt.choices.len(),
            expected.len(),
            "Night-clerk vendor must expose the three SPEC §9 choices"
        );
        for (choice, (key, label, value, enabled)) in prompt.choices.iter().zip(expected) {
            assert_eq!(
                choice.key,
                PromptKey::char(*key),
                "hotkey for {label:?} drifted from SPEC §9"
            );
            assert_eq!(choice.label, *label, "label for {key} drifted from SPEC §9");
            assert_eq!(choice.value, *value, "value for {label:?} drifted");
            assert_eq!(
                choice.enabled, *enabled,
                "enabled flag for {key} drifted from SPEC §9 step 4 / Task 11d"
            );
        }
    }

    #[test]
    fn night_clerk_vendor_prompt_renders_body_and_hotkeys() {
        // Drive the prompt through Ratatui's `TestBackend` so the test
        // asserts against the same buffer semantics the live runtime
        // uses (SPEC §6 deterministic-render contract). 70x10 gives the
        // body two rows plus the three choice rows + label without
        // forcing the renderer into modal mode.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);
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

        // SPEC §9 step 4 narration — both body fragments must survive
        // the renderer's word-wrap regardless of where the lines break
        // at 70 columns.
        for fragment in ["night clerk drums", "Evidence costs extra"] {
            assert!(
                rendered.contains(fragment),
                "rendered prompt missing SPEC §9 step 4 fragment {fragment:?}; got:\n{rendered}"
            );
        }
        // Each choice row must surface its hotkey marker plus the SPEC
        // §9 label so monochrome terminals stay legible. After Task 11d
        // the `(T)` row is disabled at STARTING_CASH=40g, so it renders
        // with the SPEC §4.2 disabled marker `- [T]` and the
        // `(need 50g)` reason instead of the enabled `(T)` form.
        for marker in ["(B)", "- [T]", "(N)"] {
            assert!(
                rendered.contains(marker),
                "missing hotkey marker {marker} in rendered prompt:\n{rendered}"
            );
        }
        for label in [
            "Buy a black coffee: 25g | You have: 40g",
            "Tip the clerk for a rumor: 50g | You have: 40g",
            "No thanks",
        ] {
            assert!(
                rendered.contains(label),
                "missing label {label:?} in rendered prompt:\n{rendered}"
            );
        }
        // SPEC §9 step 4 pins the disabled-tip reason verbatim.
        assert!(
            rendered.contains("(need 50g)"),
            "disabled (T) row must surface NEED_RUMOR_TIP_REASON; got:\n{rendered}"
        );
    }

    #[test]
    fn night_clerk_vendor_prompt_labels_track_current_cash() {
        // SPEC §9 step 4: priced rows render `<action>: <price>g | You
        // have: <cash>g`, where the cash component reflects the
        // *current* balance — not the starting balance. Building the
        // prompt with two distinct cash values and asserting the
        // priced labels move in lockstep with the input is the
        // cheapest way to prove the dynamic-label refactor (Task 11b)
        // didn't accidentally hardcode 40g. The leave-row label must
        // stay static across both balances because the SPEC reference
        // block deliberately omits a price annotation for `(N)`.
        let lean = night_clerk_vendor_prompt(0);
        let flush = night_clerk_vendor_prompt(123);

        let lean_labels: Vec<&str> = lean.choices.iter().map(|c| c.label.as_str()).collect();
        let flush_labels: Vec<&str> = flush.choices.iter().map(|c| c.label.as_str()).collect();

        assert_eq!(
            lean_labels,
            vec![
                "Buy a black coffee: 25g | You have: 0g",
                "Tip the clerk for a rumor: 50g | You have: 0g",
                "No thanks",
            ],
            "lean wallet should render `You have: 0g` on both priced rows",
        );
        assert_eq!(
            flush_labels,
            vec![
                "Buy a black coffee: 25g | You have: 123g",
                "Tip the clerk for a rumor: 50g | You have: 123g",
                "No thanks",
            ],
            "flush wallet should render `You have: 123g` on both priced rows",
        );
    }

    // ---- Buy coffee (SPEC §9 Task 11c) --------------------------------

    #[test]
    fn night_clerk_buy_coffee_decrements_cash_and_emits_success_feedback() {
        // SPEC §9 step 4 / CHECKLIST Task 11c: "Implement buying coffee
        // for 25g. Test: cash decrements and success feedback renders."
        // Run the apply handler against a freshly reset slot bundle so
        // the assertion pins the *delta* (one COFFEE_PRICE deduction)
        // rather than a hardcoded post-balance. The success-style
        // feedback contract is the second half of the SPEC's note that
        // the vendor scene reads as a transactional win — pinning the
        // style role here guards against a regression that swaps it
        // back to `info` (the Lost-and-Found register).
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let starting_cash = slots.player.borrow().cash;
        assert_eq!(
            starting_cash,
            PlayerSlot::STARTING_CASH,
            "fresh reset should boot at the documented starting balance"
        );

        let outcome = apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::BuyCoffee);

        assert_eq!(outcome, Some(NightClerkVendorOutcome::BoughtCoffee));
        assert_eq!(
            slots.player.borrow().cash,
            starting_cash - COFFEE_PRICE,
            "BuyCoffee must deduct exactly COFFEE_PRICE from the player's wallet"
        );

        let line = night_clerk_vendor_feedback(NightClerkVendorOutcome::BoughtCoffee)
            .expect("BoughtCoffee must surface a feedback line");
        assert_eq!(line.text(), BOUGHT_COFFEE_FEEDBACK);
        assert_eq!(
            line.kind(),
            foglet_game::FeedbackKind::Success,
            "coffee purchase must use the success style register"
        );
    }

    #[test]
    fn night_clerk_buy_coffee_lowercase_and_uppercase_match() {
        // SPEC §9 step 7 (mirrored from the Lost-and-Found contract):
        // direct-key prompts must treat lowercase/uppercase identically.
        // Drive the prompt itself — not just the apply handler — so the
        // test pins both halves of the case-folding chain end-to-end.
        for upper in [false, true] {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let starting = slots.player.borrow().cash;
            let prompt = night_clerk_vendor_prompt(starting);
            let key = if upper { 'B' } else { 'b' };
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Selected(NightClerkVendorChoice::BuyCoffee) => {}
                other => panic!("expected BuyCoffee for `{key}`, got {other:?}"),
            }
            apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::BuyCoffee);
            assert_eq!(
                slots.player.borrow().cash,
                starting - COFFEE_PRICE,
                "case-folded `{key}` must produce the same wallet delta",
            );
        }
    }

    // ---- Disabled tip branch (SPEC §9 Task 11d) -----------------------

    #[test]
    fn night_clerk_vendor_disables_tip_when_cash_below_rumor_price() {
        // SPEC §9 step 4: "Tipping is disabled when the player lacks
        // enough cash and shows `need 50g`." Pin both halves — the
        // `enabled` flag and the verbatim reason — at the documented
        // 40g starting balance so a future copy-edit to either has to
        // come through here. Inspecting the typed prompt (rather than
        // the rendered buffer) keeps the test cheap; the renderer
        // surface is already covered by
        // `night_clerk_vendor_prompt_renders_body_and_hotkeys` above.
        let prompt = night_clerk_vendor_prompt(PlayerSlot::STARTING_CASH);

        let tip = prompt
            .choices
            .iter()
            .find(|c| c.value == NightClerkVendorChoice::TipForRumor)
            .expect("(T) choice must remain present so the player sees the gated row");
        assert!(
            !tip.enabled,
            "(T) must be disabled when cash < RUMOR_TIP_PRICE"
        );
        assert_eq!(
            tip.disabled_reason.as_deref(),
            Some(NEED_RUMOR_TIP_REASON),
            "disabled-tip reason must match SPEC §9 step 4 verbatim"
        );

        // Sibling rows must stay enabled — Task 11d only gates `(T)`.
        for value in [
            NightClerkVendorChoice::BuyCoffee,
            NightClerkVendorChoice::NoThanks,
        ] {
            let choice = prompt
                .choices
                .iter()
                .find(|c| c.value == value)
                .unwrap_or_else(|| panic!("{value:?} missing from vendor prompt"));
            assert!(choice.enabled, "{value:?} must remain enabled at 40g");
            assert!(choice.disabled_reason.is_none());
        }
    }

    #[test]
    fn night_clerk_vendor_enables_tip_when_cash_meets_rumor_price() {
        // Mirror of the disabled case so a regression that flips the
        // comparison (`<=` vs `<`, or hardcoding 40g) lights up here
        // rather than masquerading as a manifest-level bug. RUMOR_TIP_PRICE
        // is the boundary — the row should be enabled at exactly 50g
        // (the player can afford the tip) and at any larger balance.
        for cash in [RUMOR_TIP_PRICE, RUMOR_TIP_PRICE + 25, 999] {
            let prompt = night_clerk_vendor_prompt(cash);
            let tip = prompt
                .choices
                .iter()
                .find(|c| c.value == NightClerkVendorChoice::TipForRumor)
                .expect("(T) choice present");
            assert!(
                tip.enabled,
                "(T) must be enabled at cash={cash} (>= RUMOR_TIP_PRICE)"
            );
            assert!(tip.disabled_reason.is_none());
        }
    }

    #[test]
    fn night_clerk_vendor_pressing_disabled_tip_does_not_decrement_cash() {
        // SPEC §9 step 4 + CHECKLIST Task 11d: "starting 40g disables
        // tip and selecting `t` does not decrement cash." Drive the
        // reducer with both lowercase and uppercase to mirror the SPEC
        // §9 step 7 case-folding contract — a disabled hotkey must
        // route through `PromptAction::Disabled` regardless of case so a
        // future regression that only folds the enabled path lights up
        // here. The post-press snapshot equality is the guarantee that
        // the disabled branch never reaches `apply_night_clerk_vendor_choice`
        // (which would `-=` COFFEE_PRICE — wrong amount, but still a
        // mutation) or some hypothetical tip-applier that hasn't been
        // gated yet.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let starting_cash = slots.player.borrow().cash;
        assert!(
            starting_cash < RUMOR_TIP_PRICE,
            "test invariant: STARTING_CASH must trigger the disabled-(T) branch"
        );
        let prompt = night_clerk_vendor_prompt(starting_cash);

        for key in ['t', 'T'] {
            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Disabled { reason, .. } => assert_eq!(
                    reason.as_deref(),
                    Some(NEED_RUMOR_TIP_REASON),
                    "disabled reason for {key} drifted from SPEC §9 step 4"
                ),
                other => panic!("expected PromptAction::Disabled for `{key}`, got {other:?}"),
            }
        }

        // Reducer never reached the apply handler → state is byte-
        // identical to the pre-press snapshot. Guards against a future
        // regression that adds a `TipForRumor` apply branch without
        // also re-checking the prompt-layer gate.
        assert_eq!(
            slots.snapshot(),
            before,
            "disabled (T) press must not mutate any slot"
        );
        assert_eq!(
            slots.player.borrow().cash,
            starting_cash,
            "cash must be unchanged after a disabled (T) press"
        );
    }

    // ---- No-thanks exit (SPEC §9 Task 11e) ----------------------------

    #[test]
    fn night_clerk_vendor_no_thanks_exits_without_state_change() {
        // SPEC §9 step 4: "No thanks exits cleanly." CHECKLIST Task 11e
        // pins the contract: pressing `n` resolves to a clean prompt
        // exit with zero state mutations. The test drives both halves
        // of the chain — the prompt reducer's case-folded selection
        // (lowercase + uppercase per SPEC §9 step 7) and the apply
        // handler's `None` return — and snapshots the slot bundle on
        // either side of the press to prove no slot moved. A future
        // regression that adds a `NoThanks` apply branch (e.g. a stray
        // morale tick) would flip the snapshot equality and surface
        // here, not deep in a downstream save-replay test.
        for key in ['n', 'N'] {
            let slots = SharedSlots::default();
            slots.reset(0, 0);
            let before = slots.snapshot();
            let starting_cash = slots.player.borrow().cash;
            let prompt = night_clerk_vendor_prompt(starting_cash);

            let action = prompt.handle(Input::Char(key));
            match action {
                PromptAction::Selected(NightClerkVendorChoice::NoThanks) => {}
                other => panic!("expected NoThanks for `{key}`, got {other:?}"),
            }

            // Apply handler must return `None` for the no-thanks path —
            // there is no `NightClerkVendorOutcome` variant for a clean
            // exit, which is what keeps the feedback helper from
            // accidentally surfacing a misleading success line.
            let outcome = apply_night_clerk_vendor_choice(&slots, NightClerkVendorChoice::NoThanks);
            assert!(
                outcome.is_none(),
                "NoThanks must produce no outcome (got {outcome:?}) for `{key}`"
            );

            // Byte-identical snapshot proves nothing moved: not cash,
            // not inventory, not flags, not coordinates. Pinning the
            // full snapshot (rather than just `cash`) guards against a
            // future no-thanks side-effect (e.g. an `npc_visited` flag)
            // sneaking in without an explicit SPEC update.
            assert_eq!(
                slots.snapshot(),
                before,
                "NoThanks press for `{key}` must not mutate any slot"
            );
            assert_eq!(
                slots.player.borrow().cash,
                starting_cash,
                "cash must be unchanged after a NoThanks press for `{key}`"
            );
        }
    }

    // ---- Continuation prompt (SPEC §9 step 6, Task 11f) ---------------

    #[test]
    fn night_clerk_continuation_prompt_carries_spec_body_and_default_footer() {
        // SPEC §9 step 6 quotes the body line verbatim and falls back
        // to the SPEC §4.6 default footer. We assert both via the
        // reducer's accessors so a future copy-edit to either string
        // shows up here, not in a screenshot diff.
        let prompt = night_clerk_continuation_prompt();
        assert_eq!(
            prompt.body_lines(),
            &[NIGHT_CLERK_CONTINUATION_BODY.to_string()]
        );
        assert!(
            prompt.footer_text().is_none(),
            "continuation prompt must inherit the SPEC §4.6 default footer (got override {:?})",
            prompt.footer_text()
        );
    }

    #[test]
    fn night_clerk_continuation_screen_ignores_resize() {
        // SPEC §4.6 + Task 11f acceptance: resize is *not* a meaningful
        // key, so the continuation must stay on screen until the player
        // acknowledges it. We dispatch a synthetic resize and assert
        // the screen returns `ScreenCommand::None` — anything else
        // (Pop, Replace, Quit) would dismiss the pause prematurely.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = NightClerkContinuationScreen::new();

        let cmd = screen.handle_input(
            &mut ctx,
            Input::Resize {
                width: 100,
                height: 30,
            },
        );

        assert!(
            matches!(cmd, ScreenCommand::None),
            "resize must not dismiss the continuation pause, got {cmd:?}"
        );
    }

    #[test]
    fn night_clerk_continuation_screen_pops_on_meaningful_key() {
        // Task 11f acceptance: "any meaningful key returns to prior
        // screen/map". Drive a representative sample of meaningful
        // inputs (Enter, Space, Esc, an arrow, a `Char`) through the
        // screen and assert each one resolves to `ScreenCommand::Pop`.
        // Covering several variants pins the contract that the screen
        // forwards the SPEC §4.6 "any meaningful key" classifier
        // unchanged — a future regression that filters one input
        // (e.g. swallowing arrows) would flip exactly one row here.
        let cfg = fixture_config();
        let fc = fixture_context();

        for input in [
            Input::Enter,
            Input::Char(' '),
            Input::Esc,
            Input::Up,
            Input::Char('q'),
        ] {
            let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
            let mut screen = NightClerkContinuationScreen::new();
            let cmd = screen.handle_input(&mut ctx, input.clone());
            assert!(
                matches!(cmd, ScreenCommand::Pop),
                "meaningful input {input:?} must Pop the continuation, got {cmd:?}"
            );
        }
    }

    #[test]
    fn night_clerk_continuation_screen_renders_body_and_default_footer() {
        // Smoke-test the render path through `TestBackend` so a future
        // refactor that forgets to wire the prompt to the buffer is
        // caught here. We assert the SPEC §9 step 6 body line is on
        // screen and that the SPEC §4.6 default footer survives the
        // round-trip — together they prove the screen renders the
        // continuation prompt rather than an empty buffer.
        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let mut screen = NightClerkContinuationScreen::new();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test backend constructs");
        terminal
            .draw(|frame| screen.render(&mut ctx, frame))
            .expect("render succeeds against TestBackend");

        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        let rendered = rows.join("\n");

        assert!(
            rendered.contains(NIGHT_CLERK_CONTINUATION_BODY),
            "rendered buffer must include the SPEC §9 step 6 body line; got:\n{rendered}"
        );
        assert!(
            rendered.contains("Press any key to continue"),
            "rendered buffer must include the SPEC §4.6 default footer; got:\n{rendered}"
        );
    }

    // ---- Vendor screen factory (SPEC §9 Task 11g) ---------------------

    #[test]
    fn vendor_screen_callback_buy_coffee_replaces_with_continuation() {
        // SPEC §9 step 6 requires a "press any key" beat after a
        // successful purchase. `Replace` (not `Pop`+`Push`) is the
        // load-bearing detail: the continuation pops back to the *map*,
        // not to a stale vendor prompt that would bounce the player
        // into an infinite buy loop. The test pins both halves: cash
        // decremented + Replace command emitted with the continuation
        // screen on top.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let starting_cash = slots.player.borrow().cash;
        let mut screen = night_clerk_vendor_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('b'));
        assert!(
            matches!(cmd, ScreenCommand::Replace(_)),
            "Buy Coffee must Replace the vendor with the continuation"
        );
        assert_eq!(
            slots.player.borrow().cash,
            starting_cash - COFFEE_PRICE,
            "cash must decrement by COFFEE_PRICE on Buy Coffee"
        );
        let feedback = slots.feedback.borrow().clone().expect("feedback set");
        assert_eq!(feedback.text(), BOUGHT_COFFEE_FEEDBACK);
    }

    #[test]
    fn vendor_screen_callback_no_thanks_pops_without_state_change() {
        // `[N]` is the explicit zero-cost exit. The integration must
        // route through `Pop` (not `Replace`) so no continuation
        // appears — SPEC §9 step 4 reserves the post-vendor pause for
        // *transactional* outcomes, not polite refusal.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let mut screen = night_clerk_vendor_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('n'));
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(slots.snapshot(), before);
        assert!(slots.feedback.borrow().is_none());
    }

    #[test]
    fn vendor_screen_callback_disabled_tip_writes_error_and_stays_open() {
        // STARTING_CASH (40g) is below RUMOR_TIP_PRICE (50g), so the
        // `(T)` row is built disabled. Pressing `t` must surface the
        // SPEC §9 reason as an error feedback line and emit `None` so
        // the prompt stays open for a different choice — same UX
        // contract as the drawer's disabled-`(K)` path.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before_cash = slots.player.borrow().cash;
        assert!(before_cash < RUMOR_TIP_PRICE);
        let mut screen = night_clerk_vendor_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Char('t'));
        assert!(matches!(cmd, ScreenCommand::None));
        assert_eq!(slots.player.borrow().cash, before_cash);
        let feedback = slots
            .feedback
            .borrow()
            .clone()
            .expect("disabled-tip press must surface a reason line");
        assert_eq!(feedback.text(), NEED_RUMOR_TIP_REASON);
    }

    #[test]
    fn vendor_screen_callback_cancel_pops_without_mutating_state() {
        // The vendor prompt is built `cancellable(true)` so a curious
        // player can back out without spending gold. Esc → `Pop` and
        // every slot stays at its pre-press snapshot.
        let slots = SharedSlots::default();
        slots.reset(0, 0);
        let before = slots.snapshot();
        let mut screen = night_clerk_vendor_screen(slots.clone());

        let cfg = fixture_config();
        let fc = fixture_context();
        let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
        let cmd = screen.handle_input(&mut ctx, Input::Esc);
        assert!(matches!(cmd, ScreenCommand::Pop));
        assert_eq!(slots.snapshot(), before);
        assert!(slots.feedback.borrow().is_none());
    }
}
