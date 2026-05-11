# Changelog

## 0.1.1

- Added `WorldDb::get_place_by_key` and `WorldDb::get_route_between`
  for typed shared-world spatial lookups without game-local raw SQL.
- Added `WorldDb::create_and_accept_contract` for atomically creating
  and accepting a contract with optional transaction-scoped side effects.
- Added `events::append_event_on` for appending validated `world_events`
  rows on an existing SQLite connection or transaction.
- Added `inventory::transfer_on` for composing owner-keyed inventory
  transfers with other kit-owned mutations inside a caller-owned
  transaction.
- Added `TravelRequest::with_charge_cost_tx` so game-defined travel
  costs can mutate kit-owned tables inside the same transaction that
  resolves routes, moves presence, touches recall, and appends travel
  events.
- Added `spend_turns_on` for spending Daily Turns on an existing SQLite
  connection or transaction, including from transaction-aware travel
  cost callbacks.
- Enabled SQLite foreign-key enforcement for every `WorldDb` connection,
  so kit and game migrations that declare parent rows are enforced without
  game-local trigger duplicates.
- Documented the shared-world table boundary and the preferred public
  APIs for kit-owned tables.
