# v4 Overview: Spatial Worlds and Stockpiles

This page explains how v4 composes with prior `foglet_game` contracts.
It is a map of the primitives, not a replacement for the source specs:

- v1: [`SPEC_v1.md`](../SPEC_v1.md) runtime and terminal-safety contract
- v2: [`SPEC_v2.md`](../SPEC_v2.md) shared-world SQLite and project tooling
- v2.1: [`SPEC_v2_1.md`](../SPEC_v2_1.md) migration hardening and packaging
- v3: [`SPEC_v3.md`](../SPEC_v3.md) async multiplayer persistence surface
- v4: [`SPEC_v4.md`](../SPEC_v4.md) spatial graph, recall, stockpiles, and ticks

If this page and any SPEC disagree, the SPEC wins.

## 1. Layering model

v4 is additive and structural:

- It keeps v1 terminal guarantees unchanged.
- It keeps v2/v2.1 world DB and migration discipline unchanged.
- It keeps v3 async multiplayer semantics unchanged.
- It adds new tables and APIs for where entities are, what players
  remember, who owns stock, and when periodic world work runs.

## 2. Primitive map by version

### v1 foundation (runtime safety)

- `Game` startup and teardown contract
- terminal guard and panic-safe restoration
- text UI composition boundaries

Reference docs:

- [Terminal Safety](./terminal-safety.md)
- [Text Interface](./text-interface.md)

### v2 and v2.1 foundation (durable shared world)

- world DB ownership and migration model
- project scaffolding, packaging, and manifest emission
- save and state persistence rules

Reference docs:

- [Shared World](./shared-world.md)
- [Save and State](./save-and-state.md)
- [Foglet Install](./foglet-install.md)

### v3 foundation (multiplayer persistence)

- player-scoped async state surfaces
- notice and prompt orchestration layers
- multiplayer-safe write sequencing expectations

Reference docs:

- [Async Multiplayer](./async-multiplayer.md)
- [Prompt Screens](./prompt-screens.md)
- [Dialog Screens](./dialog-screens.md)

### v4 additions (spatial and stockpiles)

- directed place graph: `places` + `routes`
- current location: `presence`
- remembered locations: `place_recall`
- owner-keyed stock rows: `inventory_slots`
- durable due-work cadence: `world_tick_tasks`

Reference docs:

- [Spatial](./spatial.md)
- [Presence and Recall](./presence-and-recall.md)
- [Inventory](./inventory.md)
- [World Ticks](./world-ticks.md)

## 3. Composition patterns

v4 is meant to be composed explicitly by game rules. These are
intentionally genre-neutral patterns built from the same primitives.

### Pattern A: Place-owned stock + tick restock (trading sim)

1. Model each market as a `place`.
2. Model each market's storage as `inventory_slots` with owner
   `("place", place_id)`.
3. Register a tick task (for example `market-restock`) that performs
   bounded transfer or quantity updates in one transaction.
4. Run catch-up via login hook or `fgk tick` to replenish stock.

### Pattern B: Room traversal + selective recall (dungeon crawler)

1. Validate adjacency through `routes`.
2. Move with `move_player` inside a transaction.
3. Touch `place_recall` only when the room should be remembered
   (for example after trap detection succeeds).

### Pattern C: Dock movement + cargo manifests (space exploration)

1. Treat docks and gates as `places`; lanes as directed `routes`.
2. Track captain location through `presence`.
3. Use `transfer` between ship and station owners to move cargo.
4. Use tick tasks for periodic board rotation or depot restocking.

## 4. Why these layers stay separate

The split between presence, recall, inventory, and ticks preserves game
policy control:

- movement does not auto-touch recall
- inventory does not auto-merge or auto-balance
- ticks are bounded one-shot passes, not implicit background work

That separation lets one kit support space ports, dungeons, towns, and
trading posts without prescribing one vocabulary.
