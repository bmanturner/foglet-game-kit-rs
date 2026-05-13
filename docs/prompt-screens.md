# When to use `PromptScreen` vs. a custom `Screen`

`PromptScreen<T>` (in `foglet_game::prompt_screen`) is an opt-in `Screen`
adapter that wraps a `ChoicePrompt<T>` and routes each `PromptAction<T>`
through a callback into a `ScreenCommand`. It exists so that screens
which are *just* a prompt — a confirmation, an any-key pause, a loot
drawer, a vendor menu — can be one constructor call instead of a
hand-rolled `Screen` impl.

This document is the answer to a question that comes up every time a
new screen lands: **should this be a `PromptScreen`, or should I write
my own `Screen`?**

## TL;DR

| Situation | Use |
|---|---|
| Screen renders one prompt and nothing else | `PromptScreen` |
| Yes/no confirmation, any-key pause, loot drawer, vendor menu | `PromptScreen` |
| Prompt is one panel beside a map, sidebar, or HUD | Custom `Screen` |
| Choice labels recompute from live game state every frame | Custom `Screen` |
| Screen owns animation, ticking timers, or layout caches | Custom `Screen` |
| Callback needs the per-frame `GameContext` to decide what to do | Custom `Screen` |
| You want a quick scaffold and may upgrade later | `PromptScreen` |

When in doubt, start with `PromptScreen`. Promoting to a custom
`Screen` later is mechanical; the prompt reducer (`ChoicePrompt::handle`
+ `step_from_input`) and the renderer (`ChoicePrompt::render` /
`render_modal`) are the same primitives the adapter calls, so the move
is a copy-paste of two method bodies plus whatever extra rendering the
new screen actually needs.

## Reach for `PromptScreen` when…

- **The screen *is* the prompt.** Nothing else paints into the frame.
  The Murder Motel Lost-and-Found Drawer and night-clerk vendor both
  fit: render the prompt, route the choice, pop
  back. No map, no sidebar, no clock.
- **You want the standard reducer wiring for free.** `PromptScreen`
  already calls `step_from_input` before `handle`, so navigation keys
  (Up/Down, optional `j`/`k`) move the cursor without double-firing as
  direct hotkeys. Hand-rolling that ordering is easy to get wrong.
- **The mapping from `PromptAction` to `ScreenCommand` is the only
  policy.** A boxed `FnMut` closes over your `Rc<RefCell<GameState>>`,
  mutates inventory or cash, and returns `ScreenCommand::Pop`. That
  callback is the entire screen-specific logic; everything else is the
  adapter.
- **Layout is one of the two standard shapes.** Compact
  for menu-style prompts, modal for confirmations and
  interruptions. `PromptScreen::new(...)` defaults to compact;
  `.modal()` switches.

## Compose `ChoicePrompt` inside a custom `Screen` when…

- **The prompt shares the frame with other widgets.** A map screen with
  an inline action menu in the bottom rows is a custom `Screen`: you
  split the `Rect`, render the map into the upper region, render the
  prompt into the lower region with `ChoicePrompt::render(area, buf)`,
  and forward inputs to `ChoicePrompt::handle` yourself.
- **Choice labels depend on per-frame game data.** A vendor whose
  labels need to read the live cash *every* render — not just at the
  moment the screen was constructed — is a custom `Screen`. Rebuild
  the `ChoicePrompt` in `render` from the current `GameContext`
  snapshot, or mutate the prompt's labels via `prompt_mut()` between
  frames if you go the `PromptScreen` route. The latter works but
  trades clarity for convenience; once labels start drifting, prefer
  the explicit custom screen.
- **You need to inspect `GameContext` while routing the action.** The
  `PromptScreen` callback runs after `handle_input` returns, with no
  context handle. If your decision of "Pop? Replace? Push?" depends on
  reading the context (e.g. "only Pop if save policy is per-user"), do
  it in a custom `Screen::handle_input` so you have `ctx` in scope.
- **The screen has its own lifecycle beyond input.** If you implement
  `tick`, `on_resize`, or care about animation frames, you've outgrown
  the adapter — `PromptScreen` defaults `tick` and `on_resize` to
  `ScreenCommand::None` because prompts have no time-based state.
- **You want the prompt to be a struct field of a richer screen.** A
  `MotelDrawerScreen { prompt: ChoicePrompt<DrawerChoice>, log:
  FeedbackLine, … }` reads more clearly than wrapping the screen in a
  `PromptScreen` and stuffing extras into the callback's captured
  state.

## Worked sketch — `PromptScreen` for a confirmation

```rust
use foglet_game::{ChoicePrompt, PromptAction, PromptScreen, ScreenCommand};

#[derive(Clone, Copy)]
enum Confirm { Yes, No }

let prompt = ChoicePrompt::new()
    .title("Quit?")
    .body("Unsaved progress will be lost.")
    .choice('y', Confirm::Yes, "Yes, quit")
    .choice('n', Confirm::No, "No, keep playing")
    .cancellable(true);

let screen = PromptScreen::new(prompt, |action| match action {
    PromptAction::Selected(Confirm::Yes) => ScreenCommand::Quit,
    PromptAction::Selected(Confirm::No) | PromptAction::Cancelled => ScreenCommand::Pop,
    _ => ScreenCommand::None,
})
.modal();
```

That's the entire screen. No `impl Screen` needed.

## Worked sketch — custom `Screen` for a map + action menu

```rust
struct ExploreScreen {
    map: MapView,
    actions: ChoicePrompt<MapAction>,
}

impl Screen for ExploreScreen {
    fn render(&mut self, ctx: &mut GameContext<'_>, frame: &mut ratatui::Frame<'_>) {
        let [map_area, prompt_area] = split_vertical(frame.area(), 70);
        self.map.render(ctx, frame, map_area);
        self.actions.render(prompt_area, frame.buffer_mut());
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        if self.actions.step_from_input(input) {
            return ScreenCommand::None;
        }
        match self.actions.handle(input) {
            PromptAction::Selected(action) => self.dispatch(ctx, action),
            PromptAction::Cancelled => ScreenCommand::Pop,
            _ => ScreenCommand::None,
        }
    }
}
```

The custom screen owns the split, the map, and the prompt; the prompt
is a *component* rather than the whole screen. `step_from_input` runs
before `handle` for the same reason `PromptScreen` does it that way:
without it, an Up arrow gets interpreted as a direct hotkey on top of
moving the cursor.

## Migration: `PromptScreen` → custom `Screen`

If a screen outgrows the adapter, the swap is mechanical:

1. Replace `PromptScreen::new(prompt, callback)` with a struct holding
   the prompt as a field.
2. Move the callback body into `Screen::handle_input` — the `match`
   arms stay identical, but you now have access to `ctx` and to any
   sibling fields.
3. Implement `Screen::render` by calling `self.prompt.render(area,
   buf)` (or `render_modal`); add any sibling widgets here.
4. Add `step_from_input` before `handle` in `handle_input`. The adapter
   was doing this for you; the custom screen has to do it explicitly.

Both shapes call the
same `ChoicePrompt` reducer and renderer.

## Worked sketch — custom detail panes with prompt-owned choices

For multi-panel screens, keep `ChoicePrompt` as the source of truth for
hotkeys, disabled rows, cancellation, and cursor movement. Let the
custom screen own only the surrounding layout and domain-specific panes:

```rust
struct StationServicesScreen {
    services: Vec<ServiceView>,
    actions: ChoicePrompt<ServiceAction>,
    feedback: Option<FeedbackLine>,
}

impl StationServicesScreen {
    fn rebuild_prompt(&mut self, credits: u32, cargo_free: u32) {
        self.actions = ChoicePrompt::new()
            .title("Services")
            .choice('r', ServiceAction::Refuel, "Refuel")
            .disabled_if(credits < 25, "need 25 credits")
            .choice('s', ServiceAction::Scan, "Buy local scan")
            .disabled_if(cargo_free == 0, "cargo full")
            .choice('l', ServiceAction::Leave, "Leave")
            .cancellable(true)
            .navigable(true);
    }

    fn render_details(&self, area: Rect, buf: &mut Buffer) {
        // Draw game-specific details, prices, stock, hazards, or route
        // previews here. Do not duplicate hotkey or disabled-choice
        // logic; the prompt still renders those rows.
    }
}

impl Screen for StationServicesScreen {
    fn render(&mut self, _ctx: &mut GameContext<'_>, frame: &mut Frame<'_>) {
        let [list_area, detail_area] = split_horizontal(frame.area(), 40);
        self.actions.render(list_area, frame.buffer_mut());
        self.render_details(detail_area, frame.buffer_mut());
        if let Some(feedback) = &self.feedback {
            feedback.render(feedback_area(frame.area()), frame.buffer_mut());
        }
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        if self.actions.step_from_input(input) {
            return ScreenCommand::None;
        }

        match self.actions.handle(input) {
            PromptAction::Selected(ServiceAction::Refuel) => self.refuel(ctx),
            PromptAction::Selected(ServiceAction::Scan) => self.buy_scan(ctx),
            PromptAction::Selected(ServiceAction::Leave) | PromptAction::Cancelled => {
                ScreenCommand::Pop
            }
            PromptAction::Disabled { reason, .. } => {
                self.feedback = reason.map(FeedbackLine::error);
                ScreenCommand::None
            }
            PromptAction::None => ScreenCommand::None,
        }
    }
}
```

The important boundary is simple: the custom screen renders the detail
panes and executes domain actions; `ChoicePrompt` still owns the reducer
and choice-row behavior.

## Worked sketch — map, details, and action prompt

When a room screen has a map panel, a detail panel, and an action prompt,
use a custom `Screen`. Rebuild the prompt from live state whenever the
room state changes; do not keep a second table of hotkeys or disabled
reasons beside it.

```rust
#[derive(Clone)]
enum RoomAction {
    MoveTo(String),
    TakeItem,
    Leave,
}

struct RoomState {
    cargo_full: bool,
    hazard_unresolved: bool,
}

struct DerelictRoomScreen {
    current_node: String,
    actions: ChoicePrompt<RoomAction>,
}

impl DerelictRoomScreen {
    fn rebuild_actions(&mut self, exits: &[MapNodeExit], state: &RoomState) {
        let take_disabled_reason = if state.hazard_unresolved {
            Some("clear hazard first")
        } else if state.cargo_full {
            Some("cargo full")
        } else {
            None
        };

        let mut prompt = ChoicePrompt::new().title("Actions").navigable(true);
        for (index, exit) in exits.iter().enumerate() {
            let key = char::from(b'1' + index as u8);
            prompt = prompt.choice(
                key,
                RoomAction::MoveTo(exit.target_key.clone()),
                format!("Move to {}", exit.target_key),
            );
        }
        prompt = prompt
            .choice('t', RoomAction::TakeItem, "Take black box")
            .disabled_if(take_disabled_reason.is_some(), take_disabled_reason.unwrap_or(""))
            .choice('l', RoomAction::Leave, "Leave")
            .cancellable(true);
        self.actions = prompt;
    }

    fn handle_input(&mut self, ctx: &mut GameContext<'_>, input: Input) -> ScreenCommand {
        if self.actions.step_from_input(input) {
            return ScreenCommand::None;
        }
        match self.actions.handle(input) {
            PromptAction::Selected(RoomAction::MoveTo(node)) => self.move_to(ctx, node),
            PromptAction::Selected(RoomAction::TakeItem) => self.take_item(ctx),
            PromptAction::Selected(RoomAction::Leave) | PromptAction::Cancelled => {
                ScreenCommand::Pop
            }
            PromptAction::Disabled { reason, .. } => {
                self.show_feedback(reason.unwrap_or_else(|| "unavailable".to_string()));
                ScreenCommand::None
            }
            PromptAction::None => ScreenCommand::None,
        }
    }
}
```

The render side is ordinary layout code: draw
`topology.render_lines(&self.current_node, '@')` in the map panel, draw
game-owned room description/hazard text in the detail panel, then call
`self.actions.render(action_area, frame.buffer_mut())` for the prompt.
`ChoicePrompt` remains the only owner of action hotkeys, navigation, and
disabled-choice text.

Pin the layout and disabled rows with `TestBackend`:

```rust
#[test]
fn derelict_room_screen_renders_layout_and_disabled_take_reason() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut screen = DerelictRoomScreen::test_room(RoomState {
        cargo_full: true,
        hazard_unresolved: false,
    });
    let mut term = Terminal::new(TestBackend::new(72, 14)).unwrap();

    term.draw(|frame| screen.render_for_test(frame)).unwrap();

    let body = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(body.contains("#####"));          // map panel
    assert!(body.contains("Outer lock"));     // detail panel
    assert!(body.contains("Take black box")); // prompt row
    assert!(body.contains("cargo full"));     // disabled reason
}
```

This is the point where `PromptScreen` is too small for the job: the
screen owns multiple panels and live room state, while `ChoicePrompt`
owns the action reducer and rows.
