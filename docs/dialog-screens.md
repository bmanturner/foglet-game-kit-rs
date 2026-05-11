# When to use `DialogScreen` vs. a custom `Screen`

`DialogScreen` (in `foglet_game::dialog_screen`) is an opt-in
[`Screen`] adapter that drives a [`Dialog`] / [`DialogState`] pair
through the standard render and input loop. It is the parallel of
[`PromptScreen<T>`](./prompt-screens.md) for the dialog primitive,
and it exists so that screens which are *just* an NPC conversation —
talk, pick a `requires`-gated branch, pop back when the cursor
finishes — can be a constructor call rather than ~500 lines of
hand-rolled `Screen` plumbing.

This document is the answer to a question that comes up every time a
new dialog scene lands: **should this be a `DialogScreen`, or should I
write my own `Screen` that composes `Dialog` directly?**

## TL;DR

| Situation | Use |
|---|---|
| Screen renders one NPC dialog and nothing else | `DialogScreen` |
| Modal "talk to the night clerk", "ask the stranger about the key" | `DialogScreen` |
| Dialog shares the frame with a live HUD, map, or inventory pane | Custom `Screen` |
| Branch availability depends on per-frame game state beyond `FlagSet` | Custom `Screen` |
| The screen owns animation, ticking timers, or layout caches | Custom `Screen` |
| Callback needs `GameContext` to decide what to do next | Custom `Screen` |
| You want a quick scaffold and may upgrade later | `DialogScreen` |

When in doubt, start with `DialogScreen`. The Murder Motel example
proves the migration: every NPC scene is now a thin wrapper around the
adapter, and the hand-rolled implementation collapsed from ~562 lines
to ~140 without losing observable behaviour or test coverage.

## Layout default differs from `PromptScreen`

`PromptScreen` defaults to the compact layout because most prompts are
inline menus. `DialogScreen` defaults to
[`DialogLayout::Modal`] because real Foglet door-game NPC scenes are
almost always centred, bordered, and titled with the speaker's name.
Authors who want the unboxed layout opt in with `.compact()`; modal is
also reachable explicitly via `.modal()` for symmetry.

## Reach for `DialogScreen` when…

- **The screen *is* the dialog.** Nothing else paints into the frame
  beyond the speaker title and any tiny per-game decoration. Every
  Murder Motel NPC scene fits: render the dialog, route the action,
  pop on `Finished` / `Cancelled`. No map, no sidebar, no clock.
- **Branch gating is just `FlagSet` predicates.** The `requires`
  strings (`flag_set`, `!flag_set`, AND/OR groups) are the full
  vocabulary the adapter understands. If your game's branch
  availability is "set/unset" plus the standard combinators, the
  shared `Rc<RefCell<FlagSet>>` is enough.
- **You want the standard reducer wiring for free.**
  `dialog_handle_prompt_input` already routes Up/Down/Enter through
  the same prompt reducer that powers `ChoicePrompt`, with no extra
  hotkey collisions. Hand-rolling the ordering of "step the cursor,
  *then* maybe handle a hotkey" is easy to get subtly wrong.
- **The mapping from `DialogAction` to `ScreenCommand` is the only
  policy.** A boxed `FnMut` closes over your `Rc<RefCell<_>>`,
  optionally records a quest log entry on `ChoicePicked`, and returns
  `ScreenCommand::Pop` on `Finished` / `Cancelled`. That callback is
  the entire screen-specific logic; everything else is the adapter.
- **You need `>9`-choice scrolling and you trust the kit to get it
  right.** `DIALOG_PROMPT_MAX_CHOICES` overflow handling is unit-
  tested at the adapter layer. A custom screen that re-implements
  scrolling has to re-test it.

## Compose `Dialog` inside a custom `Screen` when…

- **The dialog shares the frame with other widgets.** A vendor scene
  with the dialog up top and a live cash readout below is a custom
  `Screen`: split the `Rect`, render `Dialog` into the upper region
  via the kit's lower-level renderers, render the HUD into the lower
  region, and forward inputs to `dialog_handle_prompt_input` yourself.
- **Branch availability depends on data outside the `FlagSet`.** A
  shopkeeper whose "buy this" branch only appears when live cash ≥ N
  is a custom `Screen`. The adapter only consults the `FlagSet` you
  hand it; if your gating logic needs to read inventory, time of day,
  or a SQLite row every frame, do it in your own `render` /
  `handle_input` and call the dialog primitives directly.
- **You need to inspect `GameContext` while routing the action.** The
  `DialogScreen` callback runs after `handle_input` returns, with no
  context handle. If your decision of "Pop? Replace? Push a follow-up
  cutscene?" depends on reading the context (e.g. "only Replace if
  user is admin"), do it in a custom `Screen::handle_input` so you
  have `ctx` in scope.
- **The screen has its own lifecycle beyond input.** If you implement
  `tick`, `on_resize`, or care about animation frames, you've outgrown
  the adapter — `DialogScreen` defaults `tick` and `on_resize` to
  `ScreenCommand::None` because dialogs have no time-based state.
- **You want the dialog to be a struct field of a richer screen.** A
  `BarScreen { dialog: DialogScreen-or-Dialog, ambient_log:
  FeedbackLine, … }` reads more clearly than wrapping the screen in a
  `DialogScreen` and stuffing extras into the callback's captured
  state.

## The Murder Motel pattern — wrapping, not composing

The Murder Motel example does *not* drop the kit adapter; it wraps
`kit::DialogScreen` inside a tiny per-game `Screen` that adds the
genuinely game-specific decorations:

1. **Speaker name overlay.** The kit's `Dialog` schema has no speaker
   field, so the wrapper paints the NPC name into the top border row
   *after* the kit's render returns.
2. **Quit affordances.** `Q` / `Ctrl-C` hard-quit, `Backspace` pops.
   The kit deliberately stays out of the global hotkey conversation
   so each game can pick its own; the wrapper translates these into
   `ScreenCommand::Quit` and an `Esc` forwarded to the inner adapter.
3. **`j` / `k` aliases.** Kit only knows `Up` / `Down`; the wrapper
   translates for players who prefer vim-style navigation.
4. **Read accessors for tests.** `current_choice_labels()` re-derives
   the visible labels from `state().available_choices(dialog, &flags)`
   so tests can assert "what would the player see right now" without
   owning a `Frame`.

This is the recommended pattern when a game wants the adapter's wiring
but needs *small* additions. If the additions outgrow a thin shim —
i.e. once the wrapper starts re-implementing render or input rather
than overlaying onto it — that is the signal to drop one layer down
and use `Dialog` / `dialog_handle_prompt_input` directly.

## Worked sketch — `DialogScreen` for a one-shot NPC

```rust
use std::cell::RefCell;
use std::rc::Rc;

use foglet_game::{
    DialogAction, DialogScreen, DialogState, FlagSet, ScreenCommand, load_dialog,
};

let flags: Rc<RefCell<FlagSet>> = Rc::new(RefCell::new(FlagSet::new()));
let dialog = load_dialog(include_str!("../assets/clerk.yaml"))
    .expect("clerk dialog parses at build time");
let start = {
    let mut fs = flags.borrow_mut();
    DialogState::start(&dialog, &mut fs)
};

let screen = DialogScreen::new(dialog, start, Rc::clone(&flags), |action| match action {
    DialogAction::ChoicePicked { .. } => ScreenCommand::None,
    DialogAction::Finished | DialogAction::Cancelled => ScreenCommand::Pop,
});
```

That's the entire screen. No `impl Screen` needed if the game does not
need a speaker overlay or custom hotkeys.

## Worked sketch — custom `Screen` for a dialog beside live HUD

```rust
struct VendorScreen {
    dialog: Dialog,
    state: DialogState,
    flags: Rc<RefCell<FlagSet>>,
    cash: Rc<RefCell<u32>>,
}

impl Screen for VendorScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let [dialog_area, hud_area] = split_vertical(frame.area(), 70);
        // call kit's Dialog renderer into dialog_area
        // call HUD widget into hud_area showing *self.cash.borrow()
    }

    fn handle_input(&mut self, _ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        let mut flags = self.flags.borrow_mut();
        match dialog_handle_prompt_input(&self.dialog, &mut self.state, &mut flags, input) {
            PromptAction::Selected(idx) => {
                // inspect self.cash before deciding to Pop / Replace
                ScreenCommand::None
            }
            PromptAction::Cancelled => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        }
    }
}
```

The custom screen owns the split, the dialog primitive, and the HUD;
the dialog is a *component* rather than the whole screen. The reducer
runs through `dialog_handle_prompt_input` directly — the same function
the adapter calls — so observable navigation behaviour is identical.

## Migration: `DialogScreen` → custom `Screen`

If a dialog scene outgrows the adapter, the swap is mechanical:

1. Replace `DialogScreen::new(dialog, state, flags, callback)` with a
   struct holding `dialog: Dialog`, `state: DialogState`, `flags:
   Rc<RefCell<FlagSet>>`, and any sibling fields.
2. Move the callback body into `Screen::handle_input` — the `match`
   arms stay structurally identical, but you now have access to `ctx`
   and to any sibling fields.
3. Implement `Screen::render` by calling the kit's `Dialog` renderer
   (or by drawing your own shape around `state.current_node(...)`);
   add any sibling widgets here.
4. Route input through `dialog_handle_prompt_input(&self.dialog, &mut
   self.state, &mut self.flags.borrow_mut(), input)` and translate the
   resulting `PromptAction` into a `ScreenCommand` exactly as the
   adapter does internally.

Both shapes call the same `Dialog` reducer and the same `requires`
evaluator; the adapter is a convenience over those primitives, not a
separate implementation.

## See also

- [`docs/prompt-screens.md`](./prompt-screens.md) — the parallel
  authoring rule of thumb for `PromptScreen<T>`. The decision tree is
  the same shape; only the wrapped primitive differs.
