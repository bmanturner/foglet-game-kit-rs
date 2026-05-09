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

## Authoring a vendor prompt

The loot prompt above stays static once the corpse is on the floor.
A shop or vendor is the opposite case: prices are fixed by the world
but the *player* state — gold on hand, free slots in a potion bag —
shifts every time they open the menu. SPEC §5 calls this out
explicitly with the wandering monk example, and §4.2 requires that
disabled choices stay visible with a reason. This section shows the
end-to-end shape using the same builder API as the loot prompt.

The shape it targets:

```text
A wandering monk approaches after the battle.
"I carry mana potions for those who wield magic."

(M) Mana potions: 25g each | You have: 2/4 potions
(N) No thanks

Your gold: 60
Your choice:
```

When the player can't afford a potion *or* the bag is full, the `(M)`
row stays on screen but is marked disabled with a reason — never
silently hidden. That keeps the menu's shape stable across visits so
muscle memory still works.

### 1. Define the action enum and snapshot the player state

Vendor prompts read live game state, so build them from a small
snapshot rather than holding a `&mut Player` for the lifetime of the
prompt. The snapshot keeps the builder pure and testable.

```rust
use foglet_game::prompt::{ChoicePrompt, FeedbackLine, PromptAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonkOffer {
    BuyManaPotion,
    NoThanks,
}

struct VendorView {
    gold: u32,
    mana_potions: u32,
    max_mana_potions: u32,
    potion_price: u32,
}

impl VendorView {
    fn can_buy_potion(&self) -> bool {
        self.gold >= self.potion_price && self.mana_potions < self.max_mana_potions
    }

    fn disabled_reason(&self) -> &'static str {
        if self.mana_potions >= self.max_mana_potions {
            "potion bag is full"
        } else {
            "not enough gold"
        }
    }
}
```

Two reasons, two checks: surface the *binding* constraint so the
player knows whether to drop a potion or earn more gold. A single
"unavailable" string is a SPEC §4.7 anti-pattern — it forces the
player to guess.

### 2. Build the prompt with dynamic labels

Builder calls are plain function chains, so `format!` slots in
naturally. `.disabled_if(...)` attaches to the most recently added
choice, so chain it directly after `.choice(...)`.

```rust
fn monk_prompt(view: &VendorView) -> ChoicePrompt<MonkOffer> {
    ChoicePrompt::new()
        .body("A wandering monk approaches after the battle.")
        .body("\"I carry mana potions for those who wield magic.\"")
        .choice(
            'm',
            MonkOffer::BuyManaPotion,
            format!(
                "Mana potions: {price}g each | You have: {have}/{max} potions",
                price = view.potion_price,
                have = view.mana_potions,
                max = view.max_mana_potions,
            ),
        )
        .disabled_if(!view.can_buy_potion(), view.disabled_reason())
        .choice('n', MonkOffer::NoThanks, "No thanks")
        .footer(format!("Your gold: {}", view.gold))
}
```

The footer is the right home for *global* status (gold, party HP,
turn counter). Per-row state (potion count, free slots) belongs in
the row label so it sits beside the choice it gates.

### 3. Route the action and update the snapshot

`PromptAction::Disabled` carries the same reason string the builder
attached, so the feedback line and the row label stay in sync
without a second source of truth.

```rust
use foglet_game::input::Input;

fn on_input(view: &mut VendorView, prompt: &ChoicePrompt<MonkOffer>, input: Input)
    -> Option<FeedbackLine>
{
    match prompt.handle(input) {
        PromptAction::Selected(MonkOffer::BuyManaPotion) => {
            view.gold -= view.potion_price;
            view.mana_potions += 1;
            Some(FeedbackLine::success(format!(
                "Bought a mana potion. {} gold remaining.",
                view.gold,
            )))
        }
        PromptAction::Selected(MonkOffer::NoThanks) => {
            Some(FeedbackLine::info("The monk bows and walks on."))
        }
        PromptAction::Disabled { reason, .. } => Some(FeedbackLine::error(
            reason.unwrap_or_else(|| "Unavailable.".into()),
        )),
        PromptAction::None
        | PromptAction::Cancelled
        | PromptAction::ConfirmRequested(_) => None,
    }
}
```

After mutating `view`, rebuild the prompt before the next render —
that's how the labels and the disabled flag pick up the new gold and
potion counts. Holding one `ChoicePrompt` instance across multiple
purchases will show stale numbers.

### 4. Test that labels and disabled state track the snapshot

The point of a vendor prompt is the dynamic surface, so the
`TestBackend` assertion should pin both the affordable and the
broke-or-full cases. Two short tests are enough to catch label drift
and silently-skipped disable conditions.

```rust
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn render_to_string(prompt: &ChoicePrompt<MonkOffer>) -> String {
    let mut term = Terminal::new(TestBackend::new(72, 10)).unwrap();
    term.draw(|f| { prompt.render(f.area(), f.buffer_mut()); }).unwrap();
    term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
}

#[test]
fn monk_prompt_shows_live_gold_and_capacity() {
    let view = VendorView { gold: 60, mana_potions: 2, max_mana_potions: 4, potion_price: 25 };
    let body = render_to_string(&monk_prompt(&view));
    assert!(body.contains("(M) Mana potions: 25g each | You have: 2/4 potions"));
    assert!(body.contains("Your gold: 60"));
}

#[test]
fn monk_prompt_disables_purchase_when_broke() {
    let view = VendorView { gold: 10, mana_potions: 0, max_mana_potions: 4, potion_price: 25 };
    let body = render_to_string(&monk_prompt(&view));
    assert!(body.contains("not enough gold"));
}
```

If a test fails because the row wraps onto a second line, widen the
`TestBackend` rather than shortening the label — vendors in real
games tend to grow longer labels, and the prompt renderer's wrap
behaviour is covered separately by the SPEC §6 rendering tests.

## See also

- [`prompt-screens.md`](prompt-screens.md) — when to wrap a prompt
  in `PromptScreen<T>` versus a hand-rolled `Screen`.
- SPEC §4.3 — full `PromptChoice` / `ChoicePrompt` contract.
- SPEC §4.5 — `ConfirmPrompt` for the dangerous-action variant.
- SPEC §4.6 — `AnyKeyPrompt` for the post-feedback pause.
- SPEC §5 — wandering monk vendor example this walkthrough mirrors.
