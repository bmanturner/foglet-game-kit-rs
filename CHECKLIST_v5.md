# foglet-game-kit-rs Checklist — v5 Job Boards, Travel, Capacity, Log Screen, Multi-User Test Harness

One unchecked item per iteration. Dependencies in `[brackets]` must be checked off before the dependent item is eligible. This checklist extends `CHECKLIST_v4.md`; do not begin v5 implementation until v4 acceptance criteria are complete.

## Implementation tasks

- [x] **Task 1 — Re-orient on v4 surface and v5 wishlist**
      Read `SPEC_v4.md`, `SPEC_v5.md`, `CHECKLIST_v4.md`, `docs/game-idea/DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md` §§4–8, and the existing `crates/foglet_game/src/{spatial,presence,place_recall,inventory,world_ticks}/` modules. Confirm v5 namespaces under `crates/foglet_game/src/{contracts,job_board,travel,inventory_capacity}/`, `crates/foglet_game/src/screens/event_log.rs`, and `crates/foglet_game/src/test_support/multi_user.rs`. Note any cross-cutting changes needed in `world_db` migrations and in `GameContext` and in DECISIONS.md. [v4]

### Task 2 — v5 configuration

- [x] **2a** — Add optional `[contracts]`, `[job_board]`, `[travel]`, `[inventory_capacity]`, and `[screens.event_log]` sections to `GameConfig`, each with an `enabled` flag. Test: absent sections leave each primitive disabled. [Task 1]
- [x] **2b** — Parse `[screens.event_log].default_page_size` as a positive integer with a documented default. Test: zero/negative values fail clearly; absent value applies default. [2a]
- [x] **2c** — Reject configs that enable `job_board` without `contracts`. Test: clear error message. [2a]
- [x] **2d** — Reject configs that enable `travel` without `spatial` and `presence`. Test: clear error message. [2a]
- [x] **2e** — Reject configs that enable `inventory_capacity` without `inventory`. Test: clear error message. [2a]
- [x] **2f** — Reject configs that enable `screens.event_log` without v2 events. Test: clear error message. [2a]

### Task 3 — Contract schema and CRUD

- [x] **3a** — Add `contracts` migration with `id`, nullable `key`, `kind`, `issuer_owner_kind`, `issuer_owner_id`, nullable `acceptor_player_id`, `state`, `objective_json`, `reward_json`, `metadata_json`, `created_at`, nullable `accepted_at`, nullable `completed_at`, nullable `expires_at`, plus a CHECK constraint restricting `state` to the allowed enum values. [v4 Task 7]
- [x] **3b** — Implement `Contract` type and `create_contract` returning a row in `available` state. Test: round-trip preserves `objective_json` and `reward_json` byte-for-byte. [3a]
- [x] **3c** — Implement `contract_by_id` and `available_contracts(filter)` with deterministic ordering. Test: ordering is stable across inserts. [3b]
- [x] **3d** — Implement `contracts_for_acceptor(player_id, state?)` and `contracts_by_issuer(owner_kind, owner_id, state?)`. Test: filters narrow results correctly. [3b]

### Task 4 — Contract lifecycle transitions

- [ ] **4a** — Implement `accept_contract(id, player_id, on_commit?)` transactional API. Test: state moves to `accepted`; `acceptor_player_id` and `accepted_at` set. [3b]
- [ ] **4b** — Test `accept_contract` rejects when `acceptor_player_id` is already set. [4a]
- [ ] **4c** — Test `accept_contract` rejects when `expires_at <= now`. [4a]
- [ ] **4d** — Test rollback when `accept_contract` `on_commit` returns an error: state unchanged. [4a]
- [ ] **4e** — Implement `complete_contract(id, on_commit?)`. Test: state moves to `completed` with `completed_at`; rejects when state is not `accepted`. [4a]
- [ ] **4f** — Implement `fail_contract(id, on_commit?)` and `abandon_contract(id, on_commit?)`. Test: each rejects from invalid source states; `abandon_contract` clears `acceptor_player_id`. [4a]
- [ ] **4g** — Implement `expire_contract(id)` and `sweep_expired_contracts(now)`. Test: sweep moves only past-due `available` rows. [4a]
- [ ] **4h** — Surface distinct error variants for invalid transition, already-accepted, expired-on-accept, and rejected `on_commit`. Test: each path produces its own variant. [4a, 4e, 4f, 4g]

### Task 5 — Job Board aggregation

- [ ] **5a** — Define `JobBoardEntry`, `OpportunityProvider` trait, and a `ContractProvider` built-in. Test: provider returns one entry per `available` Contract with the documented field shape. [3b]
- [ ] **5b** — Implement `JobBoard::query(filter, providers)` returning a stable-ordered list. Test: default ordering is `expires_at` asc, `source`, `source_id`. [5a]
- [ ] **5c** — Add a `BountyProvider` and a `ChallengeProvider` gated on v3 enablement. Test: providers omitted when v3 is disabled; included when enabled. [5b]
- [ ] **5d** — Test that a game-supplied custom provider injects entries with `source = external` and that ordering remains stable. [5b]
- [ ] **5e** — Sanitize provider-supplied `title` and `summary` strings at the aggregation boundary using v3's bounded-text rules. Test: control sequences and overlong strings are normalized. [5b]

### Task 6 — Job Board screen

- [ ] **6a** — Implement a `JobBoardScreen` widget rendering at 80×24 with `kind_label`, `state_label`, `title`, `expires_at`. Test: synthetic long titles do not overflow. [5b]
- [ ] **6b** — Support keyboard navigation, pagination, and a quit hotkey. Test: cursor wraps at page boundaries; quit returns control to caller. [6a]
- [ ] **6c** — Support a detail modal whose body is a game-supplied callback. Test: modal opens and closes without redrawing the underlying list incorrectly. [6a]
- [ ] **6d** — Render empty-state text when the aggregated query returns zero entries. Test: empty providers produce the configured copy. [6a]
- [ ] **6e** — Game-supplied accept/claim callbacks are invoked with the entry's `accept_action` token. Test: callback receives the exact opaque token from the provider. [6a]

### Task 7 — Travel transaction helper

- [ ] **7a** — Define `TravelRequest`, `TravelResult`, and `TravelError` enums covering `NoPresence`, `NoRoute`, `AmbiguousRoute`, `ValidationFailed`, `CostFailed`, and `InventoryError`. [v4 Task 5, v4 Task 6]
- [ ] **7b** — Implement `travel(ctx, req)` executing the documented step order inside a single transaction. Test: success path updates presence, touches recall, and appends event. [7a]
- [ ] **7c** — Test `validate` failure rolls back: presence, recall, and events unchanged. [7b]
- [ ] **7d** — Test `charge_cost` failure rolls back: presence, recall, and events unchanged. [7b]
- [ ] **7e** — Test omitted `route_id` resolves a unique outbound route; ambiguous case returns `AmbiguousRoute`. [7b]
- [ ] **7f** — Test `touch_recall = false` leaves recall unchanged even when `place_recall` is enabled. [7b]
- [ ] **7g** — Test `append_event` returning `None` produces a `TravelResult` with `event_id = None` and no event row written. [7b]
- [ ] **7h** — Test the helper functions correctly when `place_recall` and v2 events are independently disabled. [7b]

### Task 8 — Inventory capacity helper

- [ ] **8a** — Define `CapacityPolicy` trait and `CapacityError` variants `InsufficientCapacity`, `PolicyError`, `InventoryError`. [v4 Task 8]
- [ ] **8b** — Implement `used_capacity(owner_kind, owner_id, &policy)`. Test: sums `volume * quantity` across all owner slots. [8a]
- [ ] **8c** — Implement `validate_incoming(owner_kind, owner_id, item_key, quantity, &policy)`. Test: rejects when adding the proposed quantity would exceed capacity. [8b]
- [ ] **8d** — Implement `transfer_with_capacity(source, dest, item_key, quantity, &policy, on_commit?)` performing capacity validation inside the v4 inventory transfer transaction. Test: success path debits and credits as in v4. [8c]
- [ ] **8e** — Test rollback on capacity overflow: source and dest unchanged. [8d]
- [ ] **8f** — Test `owner_capacity = None` allows transfers regardless of volume. [8d]
- [ ] **8g** — Test policy callback errors propagate as `CapacityError::PolicyError` and roll back. [8d]
- [ ] **8h** — Test concurrent capacity-validated transfers cannot collectively overflow capacity. [8d]

### Task 9 — Event Log / News screen

- [ ] **9a** — Implement `EventLogScreen` widget reading v2 events with configured `scope`, `page_size`, `timestamp_style`, and `empty_state_text`. Test: renders within 80×24. [v2 events]
- [ ] **9b** — Test empty-result rendering uses `empty_state_text`. [9a]
- [ ] **9c** — Test forward and backward pagination across a 100-event fixture. [9a]
- [ ] **9d** — Test player-scoped filter excludes other players' events. [9a]
- [ ] **9e** — Test the per-event formatter callback is invoked once per visible event and its return value drives rendering. [9a]
- [ ] **9f** — Implement embeddable region mode that renders within a caller-supplied rectangle. Test: embedded mode does not redraw outside the region. [9a]
- [ ] **9g** — Confirm the screen is read-only. Test: rendering does not mutate any event row. [9a]

### Task 10 — Multi-user local-dev test harness

- [ ] **10a** — Add a `test-support` Cargo feature to `crates/foglet_game`. Test: feature-off builds do not compile the harness module. [Task 1]
- [ ] **10b** — Implement `MultiUserHarness::builder().add_user(handle, role).build()` creating one shared temp world DB and per-user save roots. Test: two users observe the same world DB path. [10a]
- [ ] **10c** — Reject duplicate handles. Test: clear error variant. [10b]
- [ ] **10d** — Implement `context_for(handle)` and `with_user(handle, f)`. Test: each handle returns its own `GameContext` with isolated save roots. [10b]
- [ ] **10e** — Implement `assert_event_visible_to(handle, predicate)`. Test: distinguishes player-scoped vs. global events. [10b]
- [ ] **10f** — Implement `assert_notice_for(handle, predicate)` gated on v3 notices. Test: omitted when v3 disabled; functional when enabled. [10b]
- [ ] **10g** — Drop cleanup removes the temp directory and per-user save roots. Test: post-drop, no harness paths remain under the system temp root. [10b]
- [ ] **10h** — Self-test demonstrating Alice's transfer affects Bob's view of inventory and that player-scoped recall differs across users. [10d, 10e]

### Task 11 — Runtime context integration

- [ ] **11a** — Add optional `contracts`, `job_board`, `travel`, `inventory_capacity`, and `event_log_screen` handles to `GameContext`. [v4 Task 11]
- [ ] **11b** — Construct each handle only when its config section enables the primitive. Test: disabled sections leave the corresponding handle as `None`. [11a, 2a]
- [ ] **11c** — Document that the Event Log screen MUST NOT be invoked from inside paint loops. [11a]

### Task 12 — Documentation

- [ ] **12a** — Add `docs/contracts.md` covering the lifecycle, opaque payloads, and at least two distinct game-family examples (e.g., RPG escort quest vs. trading-game delivery). [Task 3, Task 4]
- [ ] **12b** — Add `docs/job-board.md` covering the aggregation model, providers, sanitization, and at least two non-space examples (e.g., tavern board, noir case board). [Task 5, Task 6]
- [ ] **12c** — Add `docs/travel.md` explaining the transaction order, `validate` and `charge_cost` callbacks, optional recall and events, and two non-space examples (e.g., dungeon room movement, town district navigation). [Task 7]
- [ ] **12d** — Add `docs/inventory-capacity.md` explaining the `CapacityPolicy` trait, in-transaction enforcement, and two non-space examples (e.g., RPG backpack weight, town warehouse slots). [Task 8]
- [ ] **12e** — Add `docs/event-log-screen.md` covering scope, formatter callback, embedded mode, and two non-space examples (e.g., dungeon death log, mystery case ledger). [Task 9]
- [ ] **12f** — Add `docs/test-support-multi-user.md` explaining the feature flag, builder API, and async-BBS test patterns. [Task 10]
- [ ] **12g** — Add `docs/spec-v5-overview.md` cross-referencing v1–v4 primitives and showing how v5 composes with them (e.g., delivery Contract + Travel helper + capacity-validated transfer + event-log screen). [12a–12f]
- [ ] **12h** — Update top-level `README.md` with a v5 features section linking to the new docs. [12g]
- [ ] **12i** — Document explicitly that v5 still has no real-time multiplayer, no long-lived daemon, no scripted-terminal driver, and no sample-game integration of v5 features in this kit's fixture. [12g]
- [ ] **12j** — Add a backup/maintenance note covering the new tables: `contracts`. [12g]

- [ ] **Task 13 — Final v5 verification**
      Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo test --workspace --features test-support`, and `cargo doc --workspace --no-deps`. Quote results in the final iteration. [all prior tasks]

## Acceptance criteria — gate for `<promise>V5_COMPLETE</promise>`

- [ ] All Task 1–13 items above are checked
- [ ] Contracts support the documented lifecycle with transactional transitions and `on_commit` rollback
- [ ] Job Board aggregates Contracts and (when v3 is enabled) Bounties and Challenges under one rendered surface, with stable ordering and game-supplied providers
- [ ] Travel helper executes presence move, recall touch, and event append atomically with rollback on validation or cost failure
- [ ] Inventory capacity helper validates incoming transfers inside the v4 transfer transaction with distinct error variants
- [ ] Event Log screen renders v2 events at 80×24 with scope filtering, pagination, embedded mode, and game-supplied formatters
- [ ] `test-support::MultiUserHarness` builds N isolated `GameContext` instances over one shared world DB, gated by the `test-support` Cargo feature
- [ ] Documentation explains every v5 primitive in genre-neutral terms with at least two distinct game-family examples each
- [ ] Documentation explicitly rejects real-time multiplayer, long-lived daemons, scripted-terminal drivers, and sample-game integration of v5 features in this kit's fixture
