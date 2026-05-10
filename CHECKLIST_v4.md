# foglet-game-kit-rs Checklist — v4 Spatial Worlds and Stockpiles

One unchecked item per iteration. Dependencies in `[brackets]` must be checked off before the dependent item is eligible. This checklist extends `CHECKLIST_v3.md`; do not begin v4 implementation until v3 acceptance criteria are complete.

## Implementation tasks

- [x] **Task 1 — Re-orient on v3 async multiplayer surface**
      Read `SPEC_v3.md`, `SPEC_v4.md`, `CHECKLIST_v3.md`, current migrations, and the v3 multiplayer modules. Confirm v4 primitives namespace cleanly under `crates/foglet_game/src/{spatial,presence,place_recall,inventory,world_ticks}/`. Note any cross-cutting changes needed in `world_db` for new migrations. [v3]

### Task 2 — v4 configuration

- [x] **2a** — Add optional `[spatial]`, `[presence]`, `[place_recall]`, `[inventory]`, and `[world_ticks]` sections to `GameConfig`, each with an `enabled` flag. Test: absent sections leave each primitive disabled. [Task 1]
- [x] **2b** — Parse `[world_ticks].max_catchup_per_call` as a positive integer with a documented default. Test: zero/negative values fail clearly; absent value applies default. [2a]
- [x] **2c** — Reject configs that enable `place_recall` without enabling `spatial`. Test: clear error message. [2a]
- [x] **2d** — Reject configs that enable `presence` without enabling `spatial`. Test: clear error message. [2a]

### Task 3 — Place schema and CRUD

- [x] **3a** — Add `places` migration with `id`, `key` UNIQUE, `display_name`, `kind`, `metadata_json`, `created_at`. [v2 Task 4]
- [x] **3b** — Implement `Place` type and `insert_place`. Test: stored row round-trips. [3a]
- [x] **3c** — Enforce `key` uniqueness. Test: second insert with same key fails clearly. [3b]
- [x] **3d** — Implement `get_place_by_key` and `list_places`. Test: deterministic ordering. [3b]
- [x] **3e** — Confirm `metadata_json` is stored opaquely. Test: arbitrary JSON object preserved byte-for-byte through round-trip. [3b]

### Task 4 — Route schema and adjacency

- [ ] **4a** — Add `routes` migration with `id`, `from_place_id`, `to_place_id`, `kind`, `requirements_json`, `metadata_json`, `created_at` and FK constraints to `places`. [3a]
- [ ] **4b** — Implement `create_route`. Test: stored row round-trips. [4a]
- [ ] **4c** — Implement `outbound_routes(place_id)` adjacency query. Test: returns only outbound routes. [4b]
- [ ] **4d** — Implement `inbound_routes(place_id)` query. Test: returns only inbound routes. [4b]
- [ ] **4e** — Allow parallel routes between same two places when `kind` differs. Test: two rows coexist. [4b]
- [ ] **4f** — Test asymmetric topology: route A→B without B→A; outbound from A includes it, outbound from B does not. [4c]
- [ ] **4g** — Test bidirectional access requires two rows. [4c, 4d]

### Task 5 — Presence

- [ ] **5a** — Add `presence` migration: `player_id` PK, `place_id`, `entered_at`, `metadata_json`. [v2 Task 5, 3a]
- [ ] **5b** — Implement `set_presence(player_id, place_id)` initial-placement API. Test: row created with timestamp. [5a]
- [ ] **5c** — Implement `move_player(player_id, dest_place_id, on_commit)` transactional API. Test: presence updates and `entered_at` advances. [5b]
- [ ] **5d** — Test rollback when `on_commit` returns an error: presence unchanged. [5c]
- [ ] **5e** — Implement `get_presence(player_id)` returning optional row. Test: unplaced player returns `None`. [5b]
- [ ] **5f** — Implement `players_at(place_id)`. Test: returns current occupants and excludes players who have moved on. [5c]
- [ ] **5g** — Confirm the kit does not auto-place players. Test: a freshly upserted player has no presence row until `set_presence` is called. [5b, v2 Task 5]

### Task 6 — Place recall

- [ ] **6a** — Add `place_recall` migration: `(player_id, place_id)` composite PK, `first_seen_at`, `last_seen_at`, `snapshot_json`. [3a, v2 Task 5]
- [ ] **6b** — Implement `touch_recall(player_id, place_id, snapshot_json?)` idempotent upsert. Test: first call inserts; second call updates `last_seen_at`. [6a]
- [ ] **6c** — Test that `first_seen_at` is preserved across repeated `touch_recall` calls. [6b]
- [ ] **6d** — Test that `snapshot_json` updates on each touch. [6b]
- [ ] **6e** — Implement `recall_for_player(player_id)` newest-first. Test: deterministic ordering. [6b]
- [ ] **6f** — Confirm `move_player` does not auto-touch recall. Test: a move without an explicit `touch_recall` leaves recall unchanged. [5c, 6b]

### Task 7 — Inventory schema

- [ ] **7a** — Add `inventory_slots` migration: `id`, `owner_kind`, `owner_id`, `item_key`, `quantity` non-negative, `equilibrium` nullable, `metadata_json`, `created_at`, `updated_at`. [v2 Task 4]
- [ ] **7b** — Implement `create_slot` and `get_slot(owner_kind, owner_id, item_key)`. Test: round-trips. [7a]
- [ ] **7c** — Add CHECK constraint or runtime guard preventing negative quantity. Test: direct write of negative quantity fails. [7b]
- [ ] **7d** — Confirm `equilibrium` is advisory. Test: setting `equilibrium` does not alter `quantity`; the kit performs no auto-drift. [7b]
- [ ] **7e** — Implement `slots_for_owner(owner_kind, owner_id)`. Test: deterministic ordering. [7b]
- [ ] **7f** — Implement explicit `merge_slots(slot_a, slot_b)` helper that merges only when `(owner_kind, owner_id, item_key, metadata_json)` match exactly. Test: mismatched metadata refuses to merge. [7b]

### Task 8 — Inventory atomic transfer

- [ ] **8a** — Implement `transfer(source, dest, item_key, quantity, on_commit?)` API. Test: source debited, dest credited in single transaction. [7b, v2 Task 9]
- [ ] **8b** — Test rejection when source lacks stock; no partial mutation. [8a]
- [ ] **8c** — Test rejection of zero or negative quantity. [8a]
- [ ] **8d** — Pass post-mutation slot snapshots to `on_commit`. Test: callback observes new quantities for both source and dest. [8a]
- [ ] **8e** — Roll back the entire transfer when `on_commit` returns an error. Test: source and dest unchanged after rejection. [8d]
- [ ] **8f** — Surface distinct error variants for missing source slot, insufficient stock, and rejected `on_commit`. Test: each path produces its own error. [8a, 8e]

### Task 9 — World ticks

- [ ] **9a** — Add `world_tick_tasks` migration: `key` PK, `last_run_at` nullable, `interval_seconds` positive integer, `metadata_json`. [v2 Task 4]
- [ ] **9b** — Implement runtime `register_tick(key, interval_seconds, callback)` API; persist task row on first registration; idempotent on re-register with same interval. [9a]
- [ ] **9c** — Implement `run_due_ticks(now)` returning the count of tasks that ran. Test: tasks where `last_run_at + interval <= now` (or `last_run_at IS NULL`) run; others skip. [9b]
- [ ] **9d** — Each tick callback runs inside a transaction; on error, `last_run_at` is unchanged. Test: failed callback retries on next call. [9c]
- [ ] **9e** — Enforce `max_catchup_per_call` upper bound. Test: when more tasks are due than the bound, only that many run; remainder run on subsequent call. [9c, 2b]
- [ ] **9f** — Guarantee no double-invocation under concurrent `run_due_ticks`. Test: two threads hitting `run_due_ticks` together each see disjoint task sets summing to the due set. [9c]
- [ ] **9g** — Document that tick callbacks must not assume exclusive writer access. [9c]

### Task 10 — `fgk tick` CLI subcommand

- [ ] **10a** — Add `fgk tick --project <path>` subcommand that opens the project's world DB and calls `run_due_ticks(now)` once. [9c]
- [ ] **10b** — Print a one-line summary of tasks run and tasks skipped. Test: subcommand returns exit code 0 on success. [10a]
- [ ] **10c** — Surface DB open errors before invoking `run_due_ticks`. Test: missing world DB produces a clear error and non-zero exit. [10a]
- [ ] **10d** — Add CLI smoke test asserting `fgk tick` is interruption-safe (Ctrl-C during a callback leaves `last_run_at` unchanged for that task). [10a, 9d]

### Task 11 — Runtime context integration

- [ ] **11a** — Add optional `spatial`, `presence`, `place_recall`, `inventory`, and `world_ticks` handles to `GameContext`. [v2 Task 10]
- [ ] **11b** — Construct each handle only when its config section enables the primitive. Test: disabled sections leave the corresponding handle as `None`. [11a, 2a]
- [ ] **11c** — Document that `run_due_ticks` MUST NOT be called from inside paint loops; SHOULD run on login or screen transitions. [11a]
- [ ] **11d** — Add an opt-in `run_due_ticks_on_login` runtime hook that callers may enable. Test: when enabled, login path invokes ticks; when disabled, it does not. [11c]

### Task 12 — Documentation

- [ ] **12a** — Add `docs/spatial.md` covering places and routes with at least two distinct game-family examples per concept (e.g., space exploration and dungeon crawling). [Task 3, Task 4]
- [ ] **12b** — Add `docs/presence-and-recall.md` explaining presence vs. recall as separate primitives, with examples for each. [Task 5, Task 6]
- [ ] **12c** — Add `docs/inventory.md` covering owner-keyed slots, atomic transfer, the `on_commit` callback pattern, and worked examples for at least two genres (e.g., RPG inventory transfer and trading-post commerce). [Task 7, Task 8]
- [ ] **12d** — Add `docs/world-ticks.md` explaining the tick model, the lazy-on-login vs. cron `fgk tick` patterns, idempotency guarantees, and the `max_catchup_per_call` bound. [Task 9, Task 10]
- [ ] **12e** — Add `docs/spec-v4-overview.md` cross-referencing v1–v3 primitives and showing how v4 composes with them (e.g., place-owned inventory + tick task = restocking trading post). [12a, 12b, 12c, 12d]
- [ ] **12f** — Update top-level `README.md` with a v4 features section linking to the new docs. [12e]
- [ ] **12g** — Update `docs/foglet-install.md` (or equivalent operator doc) with `fgk tick` cron guidance. [10a]
- [ ] **12h** — Document explicitly that v4 still has no real-time multiplayer and no long-lived daemon. [12d]
- [ ] **12i** — Document the deliberate choice to skip sample-game integration of v4 features in this kit's fixture and the reason (genre-neutrality). [12e]
- [ ] **12j** — Add a backup/maintenance note covering the new tables: `places`, `routes`, `presence`, `place_recall`, `inventory_slots`, `world_tick_tasks`. [12e]

- [ ] **Task 13 — Final v4 verification**
      Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --workspace --no-deps`, and `cargo run -p fgk -- tick --project <fixture>` against a synthetic v4 fixture. Quote results in the final iteration. [all prior tasks]

## Acceptance criteria — gate for `<promise>V4_COMPLETE</promise>`

- [ ] All Task 1–13 items above are checked
- [ ] Spatial graph supports unique-keyed places and asymmetric directed routes
- [ ] Presence supports transactional move with rollback on rejected `on_commit`
- [ ] Place recall is per-player, idempotent, and never auto-touched by movement
- [ ] Inventory supports atomic transfer with `on_commit` rollback and distinct error variants
- [ ] World ticks are durable, idempotent under concurrency, and bounded by `max_catchup_per_call`
- [ ] `fgk tick` CLI subcommand exists, exits cleanly, and is documented for cron use
- [ ] Documentation explains every v4 primitive in genre-neutral terms with at least two distinct game-family examples each
- [ ] Documentation explicitly rejects real-time multiplayer and long-lived daemons for v4
- [ ] Documentation explains the deliberate skip of sample-game integration in this version
