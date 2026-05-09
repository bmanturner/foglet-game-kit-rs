# When to use `PromptScreen` vs. a custom `Screen`

`PromptScreen<T>` (in `foglet_game::prompt_screen`) is an opt-in `Screen`
adapter that wraps a `ChoicePrompt<T>` and routes each `PromptAction<T>`
through a callback into a `ScreenCommand`. It exists so that screens
which are *just* a prompt — a confirmation, an any-key pause, a loot
drawer, a vendor menu — can be one constructor call instead of a
hand-rolled `Screen` impl.

This document is the answer to a question that comes up every time a
new screen lands: **should this be a `PromptScreen`, or should I write
my own `Screen`?** SPEC §4 (text-interface primitives) and SPEC §5.5
(`ScreenCommand` vocabulary) are the underlying contracts; this file
is the authoring rule of thumb.

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
  The Murder Motel Lost-and-Found Drawer (Task 10) and night-clerk
  vendor (Task 11) both fit: render the prompt, route the choice, pop
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
- **Layout is one of the two SPEC-blessed shapes.** Compact (SPEC §4.3)
  for menu-style prompts, modal (SPEC §4.8) for confirmations and
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

No SPEC contract changes during the migration — both shapes call the
same `ChoicePrompt` reducer and renderer.
