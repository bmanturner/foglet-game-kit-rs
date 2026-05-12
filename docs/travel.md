# Travel

The travel helper composes spatial routes, presence, optional recall,
and optional events into one transaction.

Transaction order:

1. Read current presence for the player.
2. Resolve the explicit route, or the unique outbound route to the
   destination when no `route_id` is supplied.
3. Run the game `validate` callback.
4. Run the game `charge_cost` callback, or the transaction-aware
   `with_charge_cost_tx` callback when installed.
5. Move presence to the destination.
6. Touch place recall when enabled and requested.
7. Run the optional transaction-aware post-move callback for triggered
   outcomes.
8. Append an event when the optional event callback returns a draft and
   the event table is present.

Any error rolls back prior writes. `validate` is for rules such as
locked doors or required permits. `charge_cost` is for game-owned costs
such as stamina, fuel, or inventory debits; the kit does not define a
currency or turn cost.

Use `TravelRequest::with_charge_cost_tx` when charging travel cost needs
to mutate kit-owned tables in the active travel transaction. The
callback receives the SQLite connection for that transaction, so it can
call helpers such as `spend_turns_on`, `inventory::transfer_on`, or
`events::append_event_on` without opening a nested transaction or
writing raw SQL against `presence`, `place_recall`, `turn_ledger`,
`inventory_slots`, or `world_events`.

For Daily Turn travel costs, call `spend_turns_on` from
`TravelRequest::with_charge_cost_tx`. The turn spend then rolls back
with route validation, presence movement, recall touch, and travel event
append if any later travel step fails.

Use `TravelRequest::with_after_move_tx` for arrival consequences that
need to observe the committed destination inside the same transaction.
The callback receives the transaction connection, the pre-move presence
row, the moved presence row, and the route. It runs after movement and
recall, before travel's optional event append and commit, and returns
structured `TriggeredOutcome` values through `TravelResult`.

Triggered outcomes separate immediate UI feedback from durable history.
If a game wants an Event Log row for an outcome, append it inside the
callback with `events::append_event_on` or return an event draft in the
outcome for the caller to inspect. Notices, Daily Intel, world-state
changes, and proof tables remain separate game-owned channels.
Idempotency is also game-owned: build a stable key from
`TriggerContext`, insert a proof/progress row with a uniqueness guard,
and emit feedback/events only when that insert wins.

Travel remains genre-neutral. The kit orchestrates atomic route
resolution, validation, optional cost charging, presence movement,
optional recall touch, post-move outcomes, and optional event append;
the game still decides fuel, credits, turn costs, hazards, and route
policy.

Examples:

- Dungeon room movement: validate that the connecting door is unlocked,
  charge torch oil, move the player, touch room recall, and append a
  "entered chamber" event.
- Town district navigation: validate curfew access, charge carriage
  fare, move presence to the market district, skip recall for familiar
  districts when the game requests it, and append a city ledger event.
