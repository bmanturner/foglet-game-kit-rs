# Authoring a loot prompt

This walkthrough builds the **Giant Spider corpse** loot prompt from
SPEC §4.3 end-to-end: the choice model, the reducer, the post-action
[`FeedbackLine`], and a small render check. It is meant as the
canonical "first prompt" example for new authors — copy-paste, then
swap in your game's enum and items.

The shape it targets:

```text
You found this on the Giant Spider's corpse.

(E) Equip immediately
(T) Take to inventory
(P) Pass

Your choice:
```

That output comes out of the kit unchanged — the renderer in
`ChoicePrompt::render` produces SPEC §4.3's example verbatim from a
plain builder chain.

## 1. Define the action enum

Prompt actions are generic (`PromptAction<T>`); the engine never
inspects `T`. Define the enum next to the scene that owns the prompt
so the variants stay readable and the match site is local.

```rust
use foglet_game::prompt::{ChoicePrompt, FeedbackLine, PromptAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpiderLoot {
    Equip,
    Take,
    Pass,
}
```

`Clone` is required by `ChoicePrompt::handle` — it returns the chosen
value by value so the prompt can be reused for another input.

## 2. Build the prompt

Builder order matches the rendered order. Each `.choice(...)` adds
one row, and `({key})` markers come from the default layout.

```rust
fn spider_loot_prompt() -> ChoicePrompt<SpiderLoot> {
    ChoicePrompt::new()
        .body("You found this on the Giant Spider's corpse.")
        .choice('e', SpiderLoot::Equip, "Equip immediately")
        .choice('t', SpiderLoot::Take, "Take to inventory")
        .choice('p', SpiderLoot::Pass, "Pass")
}
```

Direct hotkeys are case-insensitive (SPEC §4.2). `'e'` and `'E'`
both select `Equip`. No Enter key is needed for the compact unboxed
mode shown above; the renderer adds a trailing `Your choice:` label
for free.

## 3. Route the action

Hand each `Input` event to `ChoicePrompt::handle`. The reducer
returns one of the five `PromptAction` variants — only `Selected`
moves the world. Everything else is either a no-op (`None`,
`Cancelled` when not configured) or a UI signal you render and stay
on the prompt for (`Disabled`).

```rust
use foglet_game::input::Input;

fn on_input(prompt: &ChoicePrompt<SpiderLoot>, input: Input)
    -> Option<FeedbackLine>
{
    match prompt.handle(input) {
        PromptAction::Selected(SpiderLoot::Equip) => {
            // ...inventory.equip(spider_fang_dagger)...
            Some(FeedbackLine::success("Equipped the Spider Fang Dagger."))
        }
        PromptAction::Selected(SpiderLoot::Take) => {
            // ...inventory.push(spider_fang_dagger)...
            Some(FeedbackLine::info("Stowed the Spider Fang Dagger."))
        }
        PromptAction::Selected(SpiderLoot::Pass) => {
            Some(FeedbackLine::info("You leave the corpse."))
        }
        PromptAction::Disabled { reason, .. } => Some(
            FeedbackLine::error(reason.unwrap_or_else(|| "Unavailable.".into())),
        ),
        PromptAction::None
        | PromptAction::Cancelled
        | PromptAction::ConfirmRequested(_) => None,
    }
}
```

The three `FeedbackLine` constructors map to the three SPEC §4.7
severities: `info` is unmarked, `success` prepends `+ `, `error`
prepends `! `. The markers carry the meaning on a black-and-white
BBS terminal where color is gone — never assume a player can see
your theme.

## 4. Render

A scene that is *just* the prompt should reach for
[`PromptScreen`](prompt-screens.md). For inline use inside a custom
`Screen`, call `render` (compact, unboxed) or `render_modal`
(bordered) and let it paint into the supplied `Rect`:

```rust
use ratatui::layout::Rect;

fn draw(frame_area: Rect, buf: &mut ratatui::buffer::Buffer,
        prompt: &ChoicePrompt<SpiderLoot>)
{
    prompt.render(frame_area, buf);
}
```

`render` returns the row count it consumed so a custom screen can
stack a transcript above and a status footer below without
overlapping. Both render paths honour the default `Theme` and stay
SPEC §4.7 monochrome-safe.

## 5. Write a TestBackend assertion

Every prompt SHOULD have at least one `TestBackend` snapshot test —
it catches accidental layout drift and double-checks the SPEC §4.3
example shape. The whole test fits in fifteen lines:

```rust
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn spider_loot_prompt_matches_spec_example() {
    let prompt = spider_loot_prompt();
    let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();

    term.draw(|f| { prompt.render(f.area(), f.buffer_mut()); }).unwrap();

    let buf = term.backend().buffer().clone();
    let body = buf.content().iter().map(|c| c.symbol()).collect::<String>();
    assert!(body.contains("(E) Equip immediately"));
    assert!(body.contains("(T) Take to inventory"));
    assert!(body.contains("(P) Pass"));
    assert!(body.contains("Your choice:"));
}
```

## See also

- [`prompt-screens.md`](prompt-screens.md) — when to wrap a prompt
  in `PromptScreen<T>` versus a hand-rolled `Screen`.
- SPEC §4.3 — full `PromptChoice` / `ChoicePrompt` contract.
- SPEC §4.5 — `ConfirmPrompt` for the dangerous-action variant.
- SPEC §4.6 — `AnyKeyPrompt` for the post-feedback pause.
