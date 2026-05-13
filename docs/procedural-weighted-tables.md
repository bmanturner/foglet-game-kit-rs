# Deterministic weighted tables

Use `foglet_game::WeightedEntry` plus `select_weighted` when a game needs
a seeded procedural pick without giving the kit ownership of the content
schema.

The table stays game-authored. The kit only needs stable keys and
non-negative weights:

```rust
use foglet_game::{select_weighted, WeightedEntry};

let travel_events = [
    WeightedEntry::new("travel.clear", 6),
    WeightedEntry::new("travel.delay", 2),
    WeightedEntry::new("travel.find-cache", 1),
];

let selection = select_weighted("player:42:route:9:2026-05-13", &travel_events);
if let Some(key) = selection.selected_key {
    match key.as_ref() {
        "travel.clear" => { /* arrive normally */ }
        "travel.delay" => { /* spend extra time or show a warning */ }
        "travel.find-cache" => { /* grant a small discovery */ }
        _ => unreachable!("table only contains known keys"),
    }
}
```

The same seed and entry list produce the same result. Changing the seed
changes the bucket, which is useful for day keys, player keys, route
keys, or world-tick identifiers.

## World ticks

World-tick jobs can use stable tick identifiers in the seed:

```rust
use foglet_game::{select_weighted, WeightedEntry};

let table = [
    WeightedEntry::new("tick.quiet", 10),
    WeightedEntry::new("tick.price-shift", 3),
    WeightedEntry::new("tick.new-opportunity", 1),
];

let picked = select_weighted("tick:market:2026-05-13", &table);
```

Store the selected key in a game-owned event, proof row, or task log if
the tick must be idempotent across process restarts.

## NPC and content generation

NPC, rumor, encounter, or room-detail systems can derive the seed from
stable game facts:

```rust
use foglet_game::{explain_weighted_selection, WeightedEntry};

let lines = [
    WeightedEntry::new("npc.greeting.brief", 4),
    WeightedEntry::new("npc.greeting.trade", 2),
    WeightedEntry::new("npc.greeting.closed", 0),
];

let explanation = explain_weighted_selection("npc:clerk:player:42", &lines);
```

`explain_weighted_selection` returns the seed, stable hash, total weight,
bucket, selected key, and rejected zero-weight entries. Use it in tests,
debug screens, or operator logs. The non-explaining `select_weighted`
path avoids those allocations for normal gameplay.

Empty tables and tables whose weights total zero return no selection
with an explicit reason. Zero-weight entries are valid for "temporarily
disabled" content and appear in explanations as rejected entries.
