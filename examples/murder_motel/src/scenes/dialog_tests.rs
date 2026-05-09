//! Tests for the modal NPC dialog: branching gates, advancing through
//! lines, choosing options, and the dialog↔map shared-flag contract
//! that powers SPEC §13's win condition.
//!
//! Lives in a sibling file (wired in via `#[cfg(test)] #[path] mod
//! tests;` from `dialog.rs`) so the production module stays under the
//! v2.1 line-count budget without weakening test coverage.

use std::cell::RefCell;
use std::rc::Rc;

use super::DialogScreen;
use crate::map::MapScreen;
use crate::test_support::{fixture_config, fixture_context};
use foglet_game::{FlagSet, GameContext, Input, Screen, ScreenCommand};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Construct a Night Clerk dialog screen with an empty flag store
/// for tests that need to drive the conversation directly.
fn fresh_clerk_dialog() -> (DialogScreen, Rc<RefCell<FlagSet>>) {
    let flags = Rc::new(RefCell::new(FlagSet::new()));
    let clerk = MapScreen::NPCS
        .iter()
        .find(|n| n.name == "Night Clerk")
        .expect("night clerk in roster");
    let screen = DialogScreen::new(clerk, Rc::clone(&flags));
    (screen, flags)
}

#[test]
fn dialog_starts_on_speaker_and_first_line() {
    let (screen, _flags) = fresh_clerk_dialog();
    assert_eq!(screen.speaker(), "Night Clerk");
    assert!(!screen.is_finished());
    assert_eq!(screen.state().current_node(), "greeting");
}

#[test]
fn dialog_clerk_branch_is_gated_until_rumor_flag_set() {
    // Branching contract: the "Got a master key" choice must NOT
    // appear before the player asks about the murder, and MUST
    // appear after the rumor flag has been set.
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, flags) = fresh_clerk_dialog();
    // Advance through both greeting lines so choices are visible.
    screen.handle_input(&mut ctx, Input::Enter);
    screen.handle_input(&mut ctx, Input::Enter);
    let initial = screen.current_choice_labels();
    assert!(
        !initial.iter().any(|t| t.contains("master key")),
        "master-key choice should be gated initially; saw {initial:?}"
    );

    // Set the flag directly to keep the test focused on the gate
    // rather than re-driving the whole conversation; the choose()
    // path is exercised by the round-trip test below.
    flags.borrow_mut().insert("heard_rumor".into());
    let unlocked = screen.current_choice_labels();
    assert!(
        unlocked.iter().any(|t| t.contains("master key")),
        "master-key choice should unlock once `heard_rumor` is set; saw {unlocked:?}"
    );
}

#[test]
fn dialog_clerk_choose_rumor_then_key_sets_flags() {
    // End-to-end: drive the conversation through the rumor →
    // key handover branch and assert both flags persist on the
    // shared store.
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, flags) = fresh_clerk_dialog();
    // Pump greeting lines.
    screen.handle_input(&mut ctx, Input::Enter);
    screen.handle_input(&mut ctx, Input::Enter);
    // Highlight "I heard about the murder." (index 1) and pick it.
    screen.handle_input(&mut ctx, Input::Down);
    screen.handle_input(&mut ctx, Input::Enter);
    assert_eq!(screen.state().current_node(), "rumor");
    assert!(flags.borrow().contains("heard_rumor"));
    // Pump rumor lines.
    screen.handle_input(&mut ctx, Input::Enter);
    screen.handle_input(&mut ctx, Input::Enter);
    // First choice on rumor is "What about that key?" — pick it.
    screen.handle_input(&mut ctx, Input::Enter);
    assert_eq!(screen.state().current_node(), "key_handed_over");
    assert!(flags.borrow().contains("has_key"));
}

#[test]
fn dialog_clerk_hub_loops_and_exhausts_topics() {
    // Regression coverage for the hub-loop refactor: the player
    // can take any branch, return to the hub, and pick another;
    // each choice disappears once it has been taken; once every
    // topic is exhausted only "Never mind." remains.
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, _flags) = fresh_clerk_dialog();

    // Walk past the greeting lines into the hub.
    screen.handle_input(&mut ctx, Input::Enter);
    screen.handle_input(&mut ctx, Input::Enter);
    let initial = screen.current_choice_labels();
    assert_eq!(
        initial,
        vec![
            "Just checking in.".to_string(),
            "I heard about the murder.".to_string(),
            "Never mind.".to_string(),
        ],
        "hub on first entry must show the two unflagged topics plus the exit"
    );

    // Pick "Just checking in." (index 0). Walk its single line
    // back to the hub.
    screen.handle_input(&mut ctx, Input::Enter);
    assert_eq!(screen.state().current_node(), "checkin");
    screen.handle_input(&mut ctx, Input::Enter); // line + chase -> hub
    assert_eq!(screen.state().current_node(), "hub");
    let after_checkin = screen.current_choice_labels();
    assert_eq!(
        after_checkin,
        vec![
            "I heard about the murder.".to_string(),
            "Never mind.".to_string(),
        ],
        "checking in must hide the room topic"
    );

    // Pick "I heard about the murder." (index 0 now that the
    // checkin topic is gone). Walk its lines + pick "Thanks. I'll
    // be careful." (index 1) to bounce back to the hub without
    // taking the key from the rumor branch.
    screen.handle_input(&mut ctx, Input::Enter);
    assert_eq!(screen.state().current_node(), "rumor");
    screen.handle_input(&mut ctx, Input::Enter); // rumor line 1
    screen.handle_input(&mut ctx, Input::Enter); // rumor line 2; choices visible
    screen.handle_input(&mut ctx, Input::Down); // index 0 -> 1
    screen.handle_input(&mut ctx, Input::Enter); // pick "Thanks..."
    assert_eq!(screen.state().current_node(), "hub");
    let after_rumor = screen.current_choice_labels();
    assert_eq!(
        after_rumor,
        vec![
            "Got a master key I can borrow?".to_string(),
            "Never mind.".to_string(),
        ],
        "the rumor topic must hide and the master key must unlock"
    );

    // Pick the master key (index 0). Walk its two lines back to
    // the hub; only "Never mind." remains.
    screen.handle_input(&mut ctx, Input::Enter);
    assert_eq!(screen.state().current_node(), "key_handed_over");
    screen.handle_input(&mut ctx, Input::Enter); // key line 1
    screen.handle_input(&mut ctx, Input::Enter); // key line 2 + chase -> hub
    assert_eq!(screen.state().current_node(), "hub");
    let exhausted = screen.current_choice_labels();
    assert_eq!(
        exhausted,
        vec!["Never mind.".to_string()],
        "every topic taken must leave only the exit choice"
    );
}

#[test]
fn dialog_esc_pops() {
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, _flags) = fresh_clerk_dialog();
    let cmd = screen.handle_input(&mut ctx, Input::Esc);
    assert!(matches!(cmd, ScreenCommand::Pop));
}

#[test]
fn dialog_q_quits() {
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, _flags) = fresh_clerk_dialog();
    assert!(matches!(
        screen.handle_input(&mut ctx, Input::Char('q')),
        ScreenCommand::Quit
    ));
}

#[test]
fn dialog_finished_then_enter_pops() {
    // Walk the linear Bellhop dialog to its terminal node and
    // confirm one more Enter pops the screen rather than
    // erroring.
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let flags = Rc::new(RefCell::new(FlagSet::new()));
    let bellhop = MapScreen::NPCS
        .iter()
        .find(|n| n.name == "Bellhop")
        .expect("bellhop in roster");
    let mut screen = DialogScreen::new(bellhop, Rc::clone(&flags));
    // Two greeting lines, then advance past the last line into
    // the goto, then a final advance to land on the empty `end`
    // node and finish.
    for _ in 0..4 {
        screen.handle_input(&mut ctx, Input::Enter);
    }
    assert!(
        screen.is_finished(),
        "expected linear dialog to finish after 4 advances"
    );
    let cmd = screen.handle_input(&mut ctx, Input::Enter);
    assert!(matches!(cmd, ScreenCommand::Pop));
}

#[test]
fn dialog_renders_into_test_backend() {
    // Headless render check: the speaker name appears in the
    // border (overlay) and the first greeting line shows up in
    // the body (kit render).
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, _flags) = fresh_clerk_dialog();
    let mut term = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
    term.draw(|frame| screen.render(&mut ctx, frame))
        .expect("draw");
    let buf = term.backend().buffer().clone();
    let mut found = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            found.push_str(buf.cell((x, y)).expect("cell").symbol());
        }
        found.push('\n');
    }
    assert!(
        found.contains("Night Clerk"),
        "expected speaker name in dialog frame; buffer was:\n{found}"
    );
    assert!(
        found.contains("slouches"),
        "expected first greeting line in dialog body; buffer was:\n{found}"
    );
}

#[test]
fn win_flag_matches_dialog_yaml() {
    // Renaming the flag in either place would silently un-gate the
    // win condition. Drive the Night Clerk's rumor branch and
    // confirm the flag the dialog sets is the same one the map
    // checks.
    let cfg = fixture_config();
    let fc = fixture_context();
    let mut ctx = GameContext::new(&cfg, &fc, (80, 24));
    let (mut screen, flags) = fresh_clerk_dialog();
    screen.handle_input(&mut ctx, Input::Enter);
    screen.handle_input(&mut ctx, Input::Enter);
    // Highlight "I heard about the murder." (index 1) and pick it.
    screen.handle_input(&mut ctx, Input::Down);
    screen.handle_input(&mut ctx, Input::Enter);
    assert!(
        flags.borrow().contains(MapScreen::WIN_FLAG),
        "Night Clerk rumor branch must set the WIN_FLAG; saw {:?}",
        flags.borrow()
    );
}
