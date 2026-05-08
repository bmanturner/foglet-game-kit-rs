//! Dialog state machine (SPEC §9.3, Task 9b).
//!
//! Dialog in a Foglet door game is a graph of *nodes*. Each node has
//! a sequence of lines the player advances through, an optional set
//! of choices that branch to other nodes, and optional flags it sets
//! on entry. Choices may themselves be gated on flags, which is how
//! "branching dialog" with persistent narrative state works.
//!
//! Authors describe the graph in YAML — see [`load_dialog`] for the
//! format. The library deserializes it into a [`Dialog`] script
//! (immutable, share-by-`Arc` if you like), and gameplay drives a
//! [`DialogState`] that walks the graph one input at a time.
//!
//! # Why a state machine and not a coroutine
//!
//! A door game's runtime loop already pumps one input at a time
//! through `Screen::handle_input` (SPEC §8.2). A pull-based machine
//! fits that shape directly: the dialog screen calls
//! [`DialogState::current_line`] each render and one of
//! [`DialogState::advance`] / [`DialogState::choose`] each input.
//! No threads, no async, no hidden suspensions — easy to snapshot,
//! easy to save, easy to test.
//!
//! # What this module does NOT do
//!
//! - It does not render the dialog box. That's the screen's job
//!   (Task 9c will provide a widget; before then, games can render
//!   directly from `current_line()` and `available_choices()`).
//! - It does not own the flag store. Flags live on the game's
//!   `GameContext` so they can be saved with the rest of the world
//!   state. The dialog reads/writes them through a borrowed
//!   [`FlagSet`] handed in on each call.
//! - It does not load files. `load_dialog` parses a YAML string;
//!   game code decides whether the bytes came from `include_str!`,
//!   `assets/dialog/*.yaml`, or somewhere else.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Persistent narrative-flag store passed into dialog operations.
///
/// `Dialog` does not own flags because they outlive any single
/// conversation — the night clerk's `asked_about_murder` flag must
/// still be set the next time the player walks into the lobby. Games
/// keep the canonical [`FlagSet`] on their runtime state and lend
/// it to dialog calls; the dialog mutates it in place when a node
/// or choice is marked with `set:`.
///
/// `BTreeSet` (rather than `HashSet`) is deliberate: ordered
/// iteration makes save snapshots stable across runs, which keeps
/// JSON saves (SPEC §12) deterministic for diffing and testing.
pub type FlagSet = BTreeSet<String>;

/// Errors raised while loading a dialog script.
///
/// Errors during gameplay (e.g. choosing index 99) are surfaced via
/// `Result` from [`DialogState::choose`] but use a separate type
/// (`ChoiceError`) so the loader's surface stays narrow.
#[derive(Debug, Error)]
pub enum DialogError {
    /// The YAML failed to deserialize into the script schema.
    #[error("dialog YAML is malformed: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// The script's `start` field names a node that doesn't exist.
    #[error("dialog start node `{0}` is not defined")]
    UnknownStart(String),

    /// A node's `goto` (or a choice's `goto`) references a missing
    /// node. Caught at load time so games crash on bad data on
    /// startup, not mid-conversation.
    #[error("dialog node `{from}` references unknown node `{to}`")]
    UnknownGoto {
        /// Node containing the bad reference.
        from: String,
        /// Target name that does not exist.
        to: String,
    },
}

/// Errors raised while advancing through a dialog at runtime.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChoiceError {
    /// `choose` was called but the current node has no available
    /// choices (either it has none defined, or all of them are gated
    /// out by flags). Callers should fall back to `advance` when
    /// `available_choices` is empty.
    #[error("no choices available on current node")]
    NoChoices,

    /// `choose(i)` was called with `i` outside the slice returned by
    /// [`DialogState::available_choices`].
    #[error("choice index {0} out of range")]
    OutOfRange(usize),

    /// `choose` / `advance` was called after the dialog finished.
    /// Callers should check [`DialogState::is_finished`] first.
    #[error("dialog has already finished")]
    Finished,
}

/// One branch point inside a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Player-facing text for the option.
    pub text: String,
    /// Node to jump to when the choice is taken. The loader verifies
    /// this name resolves to a real node; mid-game the jump cannot
    /// fail.
    pub goto: String,
    /// Flag that must be present for the choice to be offered. `None`
    /// means the choice is always available. SPEC's "branching
    /// dialog" requirement comes down to this single field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
    /// Flags to add to the [`FlagSet`] when this choice is taken.
    /// Applied *before* the goto, so the destination node's
    /// `requires` checks see the new flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
}

/// One node in the dialog graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Lines the player advances through, top to bottom. May be
    /// empty — a node with no lines is useful for pure branching
    /// hubs ("which topic?") that fall straight through to choices.
    #[serde(default)]
    pub lines: Vec<String>,
    /// Branching options. If empty, the node falls through to `goto`
    /// (or finishes the dialog if `goto` is also `None`).
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Linear successor. Used when a node has no choices; if both
    /// `choices` and `goto` are set, choices take precedence and
    /// `goto` is treated as the fallback when every choice is gated
    /// out by missing flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goto: Option<String>,
    /// Flags added to the [`FlagSet`] the *first* time the player
    /// reaches this node within a single dialog run. Applied on
    /// entry, before any line is shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
}

/// Parsed, validated dialog script.
///
/// Construct with [`load_dialog`]; treat as read-only afterwards.
/// The struct derives `Serialize`/`Deserialize` so authors who'd
/// rather build a script in Rust (or snapshot one to JSON in tests)
/// can do so without round-tripping through YAML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dialog {
    /// Name of the node the conversation begins on. Validated at
    /// load time to exist in `nodes`.
    pub start: String,
    /// Node table keyed by name. Iteration order is not significant
    /// — only the explicit `start` / `goto` edges matter.
    pub nodes: HashMap<String, Node>,
}

/// Parse a YAML string into a validated [`Dialog`].
///
/// # Format
///
/// ```yaml
/// start: greeting
/// nodes:
///   greeting:
///     lines:
///       - "Welcome to the motel."
///       - "What do you want?"
///     choices:
///       - text: "I need a room"
///         goto: room_request
///       - text: "Tell me about the murder"
///         requires: heard_rumor
///         set: [asked_about_murder]
///         goto: murder_topic
///   room_request:
///     lines: ["Room 7 is open."]
///     set: [has_room_assigned]
///     goto: end
///   murder_topic:
///     lines: ["Bad business, that. Room 7."]
///     goto: end
///   end: {}
/// ```
///
/// # Errors
///
/// Returns [`DialogError::Yaml`] if the input is not valid YAML or
/// has the wrong shape, [`DialogError::UnknownStart`] if `start`
/// names a missing node, and [`DialogError::UnknownGoto`] if any
/// node's `goto` (including those inside `choices`) targets a name
/// not present in the table.
pub fn load_dialog(yaml: &str) -> Result<Dialog, DialogError> {
    let dialog: Dialog = serde_yaml::from_str(yaml)?;
    validate(&dialog)?;
    Ok(dialog)
}

fn validate(d: &Dialog) -> Result<(), DialogError> {
    if !d.nodes.contains_key(&d.start) {
        return Err(DialogError::UnknownStart(d.start.clone()));
    }
    for (name, node) in &d.nodes {
        if let Some(target) = &node.goto {
            if !d.nodes.contains_key(target) {
                return Err(DialogError::UnknownGoto {
                    from: name.clone(),
                    to: target.clone(),
                });
            }
        }
        for choice in &node.choices {
            if !d.nodes.contains_key(&choice.goto) {
                return Err(DialogError::UnknownGoto {
                    from: name.clone(),
                    to: choice.goto.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Mutable cursor walking a [`Dialog`].
///
/// The state stores only the cursor position (current node + line
/// index) and a `finished` flag. Flags live on the game side and
/// are passed in by reference whenever they're consulted or
/// mutated, so `DialogState` itself stays lightweight and trivially
/// snapshot-friendly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogState {
    current: String,
    line_index: usize,
    finished: bool,
}

impl DialogState {
    /// Begin a conversation at the script's `start` node.
    ///
    /// Applies the start node's `set:` flags immediately so a
    /// dialog's mere appearance can mark narrative beats — this is
    /// what lets a quest log update the moment an NPC opens their
    /// mouth, without forcing every dialog to begin with a throwaway
    /// "ok" choice.
    pub fn start(dialog: &Dialog, flags: &mut FlagSet) -> Self {
        let mut state = DialogState {
            current: dialog.start.clone(),
            line_index: 0,
            finished: false,
        };
        state.apply_entry(dialog, flags);
        state
    }

    fn apply_entry(&mut self, dialog: &Dialog, flags: &mut FlagSet) {
        if let Some(node) = dialog.nodes.get(&self.current) {
            for f in &node.set {
                flags.insert(f.clone());
            }
            // A node with no lines, no choices, and no goto is a
            // terminal — entering it ends the conversation. The
            // validator guarantees every `goto` resolves, so the only
            // way to land on a true sink is to enter one explicitly.
            if node.lines.is_empty() && node.choices.is_empty() && node.goto.is_none() {
                self.finished = true;
            }
        }
    }

    /// Whether the conversation has run off the end of the graph.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Name of the node currently being shown. Useful for tests and
    /// for screens that want to react to specific nodes (e.g. play
    /// a portrait change when entering `confession`).
    pub fn current_node(&self) -> &str {
        &self.current
    }

    /// The line the player should be reading right now, or `None`
    /// if the cursor has walked past the last line and the screen
    /// should render choices (or auto-advance) instead.
    pub fn current_line<'a>(&self, dialog: &'a Dialog) -> Option<&'a str> {
        let node = dialog.nodes.get(&self.current)?;
        node.lines.get(self.line_index).map(String::as_str)
    }

    /// Choices available right now, filtered by the supplied flags.
    ///
    /// Returned indices are stable across calls *with the same flag
    /// set*, which is what callers need: the screen highlights one
    /// option, the player presses Enter, the runtime calls
    /// `choose(highlighted_index)`. If flags change between the call
    /// to `available_choices` and the call to `choose`, the index
    /// may shift — this is the same behaviour you'd get from any
    /// gated-menu UI, and is why both methods take `flags`.
    pub fn available_choices<'a>(&self, dialog: &'a Dialog, flags: &FlagSet) -> Vec<&'a Choice> {
        if self.finished {
            return Vec::new();
        }
        let Some(node) = dialog.nodes.get(&self.current) else {
            return Vec::new();
        };
        if self.line_index < node.lines.len() {
            // Still pumping lines — choices are hidden until the
            // player has read the whole node.
            return Vec::new();
        }
        node.choices
            .iter()
            .filter(|c| match &c.requires {
                Some(flag) => flags.contains(flag),
                None => true,
            })
            .collect()
    }

    /// Advance one line, or fall through to a linear `goto` once the
    /// lines are exhausted.
    ///
    /// Returns `Err(ChoiceError::Finished)` if called on a finished
    /// dialog. Callers should not call `advance` when
    /// `available_choices` is non-empty — they should call `choose`
    /// instead. If they do call `advance` with choices waiting, the
    /// state stays put and the call is a no-op (returning `Ok(())`),
    /// because silently picking choice 0 would surprise authors.
    pub fn advance(&mut self, dialog: &Dialog, flags: &mut FlagSet) -> Result<(), ChoiceError> {
        if self.finished {
            return Err(ChoiceError::Finished);
        }
        let node = dialog
            .nodes
            .get(&self.current)
            .expect("validated dialog always resolves current node");
        if self.line_index < node.lines.len() {
            self.line_index += 1;
            return Ok(());
        }
        // Lines exhausted. If choices exist and at least one is
        // available, the screen should be rendering them — bail
        // without mutating state so the caller can re-route to
        // `choose`.
        if !self.available_choices(dialog, flags).is_empty() {
            return Ok(());
        }
        match &node.goto {
            Some(target) => self.jump_to(dialog, target.clone(), flags),
            None => {
                self.finished = true;
            }
        }
        Ok(())
    }

    /// Take a choice by its index in the slice returned by
    /// [`DialogState::available_choices`].
    ///
    /// Applies the choice's `set:` flags before jumping, so the
    /// destination node's `requires` checks see the freshly-added
    /// flags.
    pub fn choose(
        &mut self,
        dialog: &Dialog,
        flags: &mut FlagSet,
        index: usize,
    ) -> Result<(), ChoiceError> {
        if self.finished {
            return Err(ChoiceError::Finished);
        }
        let available = self.available_choices(dialog, flags);
        if available.is_empty() {
            return Err(ChoiceError::NoChoices);
        }
        let choice = available
            .get(index)
            .copied()
            .ok_or(ChoiceError::OutOfRange(index))?;
        // Clone fields out of the immutable borrow before mutating
        // flags / state — `available` borrows `dialog`, and we need
        // a `&mut self` for `jump_to` below.
        let goto = choice.goto.clone();
        let set: Vec<String> = choice.set.clone();
        drop(available);
        for f in set {
            flags.insert(f);
        }
        self.jump_to(dialog, goto, flags);
        Ok(())
    }

    fn jump_to(&mut self, dialog: &Dialog, target: String, flags: &mut FlagSet) {
        self.current = target;
        self.line_index = 0;
        self.apply_entry(dialog, flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_yaml() -> &'static str {
        r#"
start: greeting
nodes:
  greeting:
    lines:
      - "Welcome to the motel."
      - "What do you want?"
    choices:
      - text: "I need a room"
        goto: room_request
      - text: "Tell me about the murder"
        requires: heard_rumor
        set: [asked_about_murder]
        goto: murder_topic
      - text: "Just looking"
        goto: end
  room_request:
    lines: ["Room 7 is open."]
    set: [has_room_assigned]
    goto: end
  murder_topic:
    lines: ["Bad business, that. Room 7."]
    goto: end
  end: {}
"#
    }

    #[test]
    fn loads_well_formed_yaml() {
        let dialog = load_dialog(sample_yaml()).expect("parses");
        assert_eq!(dialog.start, "greeting");
        assert_eq!(dialog.nodes.len(), 4);
    }

    #[test]
    fn rejects_malformed_yaml() {
        let err = load_dialog(": not yaml :::").unwrap_err();
        assert!(matches!(err, DialogError::Yaml(_)), "got {err:?}");
    }

    #[test]
    fn rejects_unknown_start() {
        let yaml = "start: nope\nnodes:\n  greeting: {}\n";
        let err = load_dialog(yaml).unwrap_err();
        assert!(matches!(err, DialogError::UnknownStart(s) if s == "nope"));
    }

    #[test]
    fn rejects_unknown_goto() {
        let yaml = "start: a\nnodes:\n  a:\n    goto: missing\n";
        let err = load_dialog(yaml).unwrap_err();
        assert!(matches!(
            err,
            DialogError::UnknownGoto { from, to } if from == "a" && to == "missing"
        ));
    }

    #[test]
    fn rejects_unknown_choice_goto() {
        let yaml = r#"
start: a
nodes:
  a:
    choices:
      - text: x
        goto: ghost
"#;
        let err = load_dialog(yaml).unwrap_err();
        assert!(matches!(err, DialogError::UnknownGoto { to, .. } if to == "ghost"));
    }

    #[test]
    fn linear_walk_through_lines() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        assert_eq!(state.current_line(&dialog), Some("Welcome to the motel."));
        state.advance(&dialog, &mut flags).unwrap();
        assert_eq!(state.current_line(&dialog), Some("What do you want?"));
        state.advance(&dialog, &mut flags).unwrap();
        // Lines exhausted; choices visible.
        assert!(state.current_line(&dialog).is_none());
        let choices = state.available_choices(&dialog, &flags);
        // "Tell me about the murder" is gated by `heard_rumor`.
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].text, "I need a room");
        assert_eq!(choices[1].text, "Just looking");
    }

    #[test]
    fn choice_sets_flag_and_branches() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        flags.insert("heard_rumor".to_string());
        let mut state = DialogState::start(&dialog, &mut flags);
        // Walk past both lines.
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        let choices = state.available_choices(&dialog, &flags);
        assert_eq!(choices.len(), 3, "rumor flag unlocks the third option");
        // Pick "Tell me about the murder".
        state.choose(&dialog, &mut flags, 1).unwrap();
        assert!(flags.contains("asked_about_murder"));
        assert_eq!(state.current_node(), "murder_topic");
        assert_eq!(
            state.current_line(&dialog),
            Some("Bad business, that. Room 7.")
        );
    }

    #[test]
    fn node_set_flags_apply_on_entry() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        // Pick "I need a room" — node `room_request` sets `has_room_assigned` on entry.
        state.choose(&dialog, &mut flags, 0).unwrap();
        assert!(flags.contains("has_room_assigned"));
    }

    #[test]
    fn linear_goto_then_finishes_at_empty_node() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        state.choose(&dialog, &mut flags, 1).unwrap(); // -> "Just looking" -> end
        assert_eq!(state.current_node(), "end");
        assert!(state.is_finished(), "empty terminal node ends dialog");
    }

    #[test]
    fn linear_goto_walks_through_intermediate_node() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        state.choose(&dialog, &mut flags, 0).unwrap(); // -> room_request
        assert_eq!(state.current_line(&dialog), Some("Room 7 is open."));
        state.advance(&dialog, &mut flags).unwrap(); // line index past last
        state.advance(&dialog, &mut flags).unwrap(); // follow goto -> end (empty node finishes)
        assert!(state.is_finished());
    }

    #[test]
    fn choose_rejects_out_of_range() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        let err = state.choose(&dialog, &mut flags, 99).unwrap_err();
        assert_eq!(err, ChoiceError::OutOfRange(99));
    }

    #[test]
    fn choose_rejects_when_no_choices() {
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        // On the first line — choices are hidden until lines exhaust.
        let err = state.choose(&dialog, &mut flags, 0).unwrap_err();
        assert_eq!(err, ChoiceError::NoChoices);
    }

    #[test]
    fn advance_after_finish_errors() {
        let yaml = "start: a\nnodes:\n  a: {}\n";
        let dialog = load_dialog(yaml).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        assert!(state.is_finished());
        let err = state.advance(&dialog, &mut flags).unwrap_err();
        assert_eq!(err, ChoiceError::Finished);
    }

    #[test]
    fn advance_with_choices_waiting_is_noop() {
        // If the screen mistakenly calls advance() while choices are
        // available, state must not silently mutate (which would skip
        // the choice prompt).
        let dialog = load_dialog(sample_yaml()).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        state.advance(&dialog, &mut flags).unwrap();
        state.advance(&dialog, &mut flags).unwrap();
        let before = state.clone();
        state.advance(&dialog, &mut flags).unwrap();
        assert_eq!(state, before);
    }

    #[test]
    fn all_choices_gated_falls_through_to_goto() {
        // A pure-branching hub with all choices gated and a goto
        // fallback must take the goto, not deadlock.
        let yaml = r#"
start: hub
nodes:
  hub:
    choices:
      - text: secret
        requires: never_set
        goto: secret_node
    goto: fallback
  secret_node: {}
  fallback: {}
"#;
        let dialog = load_dialog(yaml).unwrap();
        let mut flags = FlagSet::new();
        let mut state = DialogState::start(&dialog, &mut flags);
        // No lines, no available choices, but a goto — advance should follow it.
        assert!(state.available_choices(&dialog, &flags).is_empty());
        state.advance(&dialog, &mut flags).unwrap();
        assert_eq!(state.current_node(), "fallback");
        assert!(state.is_finished());
    }
}
