# When to use `SaveSlot<T>` vs. a hand-rolled save handle

`SaveSlot<T>` (in `foglet_game::save`) is a typed wrapper around the
persistence primitives — `read_save`, `write_atomic`, and
[`SaveStrategy`](../crates/foglet_game/src/save.rs) — that bundles
the persisted game state, an interior-mutable handle, and a dirty
flag into one cheaply-cloned struct. It exists so that games which
already persist a single serializable state struct can drop the
hand-rolled `Rc<RefCell<GameState>>` plumbing in favour of one type
that documents the sharing model and the persistence contract at the
call site.

This document is the answer to a question that comes up the moment a
game grows past a single screen: **should this state live in a
`SaveSlot<T>`, or should I keep my own `Rc<RefCell<_>>`?**

## TL;DR

| Situation | Use |
|---|---|
| One serializable struct holds the entire persisted game state | `SaveSlot<T>` |
| Multiple screens need to mutate the same persisted state | `SaveSlot<T>` |
| You want `Game::with_save_handler` to call `save` for you on Quit | `SaveSlot<T>` |
| State is purely ephemeral (toasts, focus rings, animation timers) | Plain `Rc<RefCell<_>>` |
| State must be coordinated across processes (shared-world SQLite) | `SharedWorld` (see [`shared-world.md`](shared-world.md)) |
| You need to persist *several* unrelated structs to *separate* files | One `SaveSlot<T>` per file |
| You're prototyping and unsure of the schema yet | Plain `Rc<RefCell<_>>`, promote later |

When in doubt, start with `SaveSlot<T>` if the state is supposed to
survive across runs. Promoting later is mechanical; demoting back to
a raw handle is a one-line change because `SaveSlot` is itself
`Rc`-cheap to clone and exposes `borrow` / `borrow_mut` that look
exactly like `RefCell`'s.

## Reach for `SaveSlot<T>` when…

- **One struct is the save file.** If `serde_json::to_string(&state)`
  is the entire on-disk contract, `SaveSlot<T>` is the typed entry
  point: `SaveSlot::load_or_default(&path)` on startup,
  `slot.save(&path)` on Quit. The Murder Motel example does exactly
  this with `SaveState` — see `examples/murder_motel/src/state.rs`.
- **Multiple screens mutate the same state.** Every clone of a
  `SaveSlot` is a refcount bump on the same `Rc<RefCell<T>>` and the
  same `Rc<Cell<bool>>` dirty flag. Mutating through any handle from
  any screen flips the same flag, which is exactly what the runtime's
  save hook reads. Hand-rolling that with two separate `Rc`s is easy
  to get subtly wrong.
- **You want the runtime to drive persistence.**
  `slot.save_handler(path)` returns a `SaveHandler` closure that
  captures the slot's `Rc`s by clone and calls `save(&path)` when
  invoked. Pass it to `Game::with_save_handler` and the runtime fires
  it on every `SideEffect::Save` and once on the `Quit` drain — no
  manual `if let Some(path) = save_path { write_atomic(..)? }` tail
  in `main.rs`.
- **You want the dirty flag for free.** `borrow_mut` and `apply` flip
  it on; `save` clears it after the rename succeeds; `is_dirty()` is
  advisory. `is_dirty` must not be used to *gate*
  persistence — that decision belongs to the runtime or to the
  author — but it's the right primitive for a "skip the no-op write
  on idle ticks" optimisation later.
- **You want `Clone` to mean "another handle to the same state."**
  `SaveSlot::handle()` is a deliberately-named alias for `Clone` so
  constructor signatures like `MapScreen::with_slots(slots.save.handle())`
  read as cheap-`Rc` semantics rather than "deep-clone the world."

## Keep a plain `Rc<RefCell<_>>` when…

- **The state is ephemeral.** Toast feedback lines, focus rings,
  animation tickers, transient cursor positions — anything that
  *must not* end up in the save file should not live in the
  `SaveSlot`. The Murder Motel example keeps `feedback:
  Rc<RefCell<FeedbackLine>>` outside of `slots.save` precisely so
  the dirty flag can't accidentally pull it into the JSON.
- **The state is per-process and never persisted.** A shared
  in-memory cache between two screens that exists for the lifetime of
  one run is a `Rc<RefCell<_>>`, not a `SaveSlot<T>`. `SaveSlot`
  carries persistence contract baggage (`Serialize`,
  `DeserializeOwned`, atomic-write semantics) that you'd be opting
  into for nothing.
- **The state needs cross-process coordination.** `SaveSlot<T>` is
  single-process only and makes no synchronisation guarantees. Two
  Foglet doors writing to the same path through their own
  `SaveSlot`s would last-writer-wins each other into the ground.
  Reach for the [`SharedWorld`](shared-world.md) layer (SQLite
  with WAL and explicit transactions) when more than one process
  needs to see the same state.
- **You have several unrelated structs.** `SaveSlot<T>` is one type
  per file. If your save policy demands "one JSON for inventory, one
  for quest log, one for settings," you want three slots — one per
  file — not one slot with a tuple `T`. Authors who try to merge
  unrelated states into one `T` to satisfy the slot's "one type"
  shape end up with a `T` whose `Default` impl is a lie.

## Worked sketch — `SaveSlot<T>` driving the save handler

```rust
use foglet_game::{Game, SaveSlot};

#[derive(Default, serde::Serialize, serde::Deserialize, Clone)]
struct SaveState {
    seen_intro: bool,
    inventory: Vec<String>,
}

let slot: SaveSlot<SaveState> = SaveSlot::load_or_default(&save_path)?;
let game = Game::new(/* … */)
    .with_save_handler(slot.save_handler(save_path.clone()));

// Each screen takes a clone of the slot — refcount bump, not deep clone.
let map = MapScreen::with_slots(slot.handle());

// Mutating from any handle dirties the shared flag…
map.slots().save.borrow_mut().inventory.push("dossier".into());

// …and the runtime calls the handler on Quit, which calls
// `slot.save(&save_path)` and clears the flag once the rename lands.
game.run(map)?;
```

That's the entire persistence wiring. No `read_save` / `write_atomic`
calls in `main.rs`, no manual dirty bookkeeping, no chance of
forgetting the post-`run` save tail.

## Worked sketch — keeping ephemeral state outside the slot

```rust
struct SharedSlots {
    /// Persisted half — one slot, one file.
    save: SaveSlot<SaveState>,
    /// Ephemeral half — toast lines and focus state. Cloned around
    /// the same way, but **never** copied into the save file.
    feedback: Rc<RefCell<FeedbackLine>>,
    flags: Rc<RefCell<FlagSet>>,
}
```

The `SaveState` struct contains *only* the fields that survive across
runs. `feedback` and `flags` use plain `Rc<RefCell<_>>` because
flipping `feedback.borrow_mut()` would otherwise mark the slot dirty
and trigger a no-op write on the next save tick. The Murder Motel
example uses exactly this split — the persisted half is one slot,
the ephemeral half lives alongside.

## Migration: `Rc<RefCell<GameState>>` → `SaveSlot<T>`

If a save handle outgrows the raw shape, the swap is mechanical:

1. Replace `Rc<RefCell<GameState>>` with `SaveSlot<GameState>`. The
   field's type changes; every `state.borrow()` /
   `state.borrow_mut()` call site keeps working — `SaveSlot`
   exposes the same two methods with the same `Ref` / `RefMut`
   return types.
2. Delete any hand-written `snapshot` / `apply` helpers — `SaveSlot`
   ships them. The names `snapshot` and `apply` are fixed — don't
   reinvent them per game.
3. Replace startup `read_save(&path)?` with
   `SaveSlot::load_or_default(&path)?` (or `SaveSlot::load(&path)?`
   if you want the missing-file branch to be explicit).
4. Replace the post-`run` `if let Some(path) = save_path {
   write_atomic(&path, &*state.borrow())? }` tail with
   `Game::with_save_handler(slot.save_handler(path.clone()))` at
   builder time. The runtime fires the handler on Quit; the tail goes
   away.
5. If multiple structs were sharing one file, decide whether to merge
   them under one `T` or split them into multiple slots / files. The
   slot is one-type-one-file by design — see the "several unrelated
   structs" caveat above.

Both shapes call the same `read_save` / `write_atomic` reliability
bar. The slot adds typed bookkeeping on top.

## Common pitfalls

- **Treating `is_dirty` as a save gate.** It's advisory. Authors who
  write `if slot.is_dirty() { slot.save(&path)? }` inside a screen
  are reinventing the runtime's save hook badly — the runtime
  already does this once per Save / Quit and adds atomic-write
  guarantees you can't replicate from a screen. Use the handler.
- **Putting context fields in `T`.** Never copy the Foglet context
  wholesale into a save. The slot doesn't enforce that for you — `T`
  is author-defined — but a save file with a `door_id` or `username`
  baked in is a leak. Persist game decisions, not the environment that
  produced them.
- **Re-borrowing across a render boundary.** Holding a
  `slot.borrow_mut()` across a `frame.render_widget` call is a
  `RefCell` panic waiting to happen because a sibling screen's
  renderer might also try to read the slot. Borrow, mutate, drop —
  same discipline as raw `Rc<RefCell<_>>`.
- **Stuffing ephemeral state into `T` "just for now."** It always
  ends up persisted in production. If a field is meant to be
  ephemeral, keep it outside the slot from day one.

## Where the bytes go

`SaveSlot::save` delegates to `write_atomic`, which performs a
parent-dir `mkdir -p`, writes to a temp file, fsyncs, and renames
into place. The slot adds typed bookkeeping but does not introduce a
second persistence path. If a save fails, the dirty flag is left set so the next save
attempt actually retries; if it succeeds, the flag is cleared and the
on-disk bytes are by definition in sync with the slot.

For the runtime side of the contract — when the handler fires, what
`GameError::Save` looks like, why the handler runs while the runtime
still owns the terminal — see the rustdoc on
`Game::with_save_handler` and `SaveSlot::save_handler`.
