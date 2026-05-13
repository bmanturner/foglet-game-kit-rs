# Changelog

## 0.1.1

- Changed the runtime render loop to draw only when the terminal frame
  is dirty, avoiding repeated `Terminal::draw` calls while static
  screens sit idle on tick timeouts.
- Added `ScreenCommand::Redraw` so animated or countdown screens can
  explicitly opt into timer-driven paints without forcing all screens
  to repaint every idle tick.
- Added runtime-loop coverage for idle no-redraw behavior, redraw after
  input, redraw after resize, stack-transition redraws, and explicit
  animated redraws.
- Added `WorldDb::get_place_by_key` and `WorldDb::get_route_between`
  for typed shared-world spatial lookups without game-local raw SQL.
- Added deterministic weighted-table helpers for seeded procedural
  choices, including stable seed-to-bucket hashing, explicit no-selection
  results, optional roll explanations, and genre-neutral usage docs.
- Added transaction-scoped `get_place_by_id_on`, `get_presence_on`, and
  `get_route_by_id_between_on` helpers so game services can query common
  spatial and presence rows inside an existing SQLite transaction.
- Added place-recall snapshot merge helpers that update game-owned JSON
  objects while preserving unrelated keys, preserving first-seen
  timestamps, advancing last-seen timestamps, and surfacing invalid
  existing JSON as typed errors.
- Added namespaced place-recall merge helpers that deep-merge a JSON
  object at a game-owned namespace path while preserving sibling
  namespaces and rejecting non-object path collisions with typed errors.
- Added map-backed node topology helpers over the ASCII map parser so
  games can validate named node anchors, declared exits, and render the
  same authored map with a current-node marker while keeping room rules
  game-owned.
- Added prompt-composition and DateProvider service-layer guidance,
  including a lightweight `ServiceContext` for threading `GameConfig`,
  `FogletContext`, and an injected date provider through game services.
- Documented a custom map/detail/action-prompt `Screen` pattern that
  keeps `ChoicePrompt` authoritative for hotkeys, navigation, and
  disabled-choice reasons, including a `TestBackend` layout assertion.
- Added `WorldDb::create_and_accept_contract` for atomically creating
  and accepting a contract with optional transaction-scoped side effects.
- Added `events::append_event_on` for appending validated `world_events`
  rows on an existing SQLite connection or transaction.
- Added `inventory::transfer_on` for composing owner-keyed inventory
  transfers with other kit-owned mutations inside a caller-owned
  transaction.
- Added `WorldDb::take_finite_pickup_with_capacity` and
  `FinitePickupResult` for capacity-checked finite shared pickups that
  report source exhaustion and let game-owned callback effects roll back
  with inventory movement.
- Added `TravelRequest::with_charge_cost_tx` so game-defined travel
  costs can mutate kit-owned tables inside the same transaction that
  resolves routes, moves presence, touches recall, and appends travel
  events.
- Added generic triggered-outcome contracts plus
  `TravelRequest::with_after_move_tx`, letting games apply
  transaction-scoped arrival consequences after presence moves and
  return structured UI feedback through `TravelResult`.
- Added reusable contract job read models for available, accepted,
  ready-to-complete, completed, failed, abandoned, and expired work,
  with game-supplied objective readiness and requirement rows.
- Added a custom contract objective regression example using
  `ContractObjectiveViewProvider`, game-owned proof rows, and
  `transfer_on` inside contract completion so cargo consumption remains
  atomic with lifecycle changes.
- Added `inventory::grant_inventory_on` and
  `inventory_capacity::grant_inventory_with_capacity_on` for
  transaction-scoped direct rewards and pickups with metadata-compatible
  upsert behavior.
- Added connection-scoped contract key/acceptor lookup helpers for
  transaction-local lifecycle checks without raw SQL.
- Added player-scoped map projection helpers that combine presence,
  place recall, game-authored visibility policy, and route availability
  for graph, room, atlas, or star-chart UIs.
- Expanded multi-user test-support guidance with a shared finite-state
  scenario proving one player can exhaust shared inventory while proof
  events and recall remain player-scoped unless the game writes shared
  history intentionally.
- Added `spend_turns_on` for spending Daily Turns on an existing SQLite
  connection or transaction, including from transaction-aware travel
  cost callbacks.
- Documented why generic market quote/read-model support is deferred
  until pricing, balance, and disabled-reason patterns repeat across
  more downstream games.
- Enabled SQLite foreign-key enforcement for every `WorldDb` connection,
  so kit and game migrations that declare parent rows are enforced without
  game-local trigger duplicates.
- Documented the shared-world table boundary and the preferred public
  APIs for kit-owned tables.
