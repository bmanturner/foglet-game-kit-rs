# Transaction composition APIs

`foglet_game` 0.1.1 adds small transaction-local APIs for games that
need several shared-world primitives to commit or roll back together.

Use these APIs instead of direct SQL against kit-owned tables:

- `events::append_event_on(conn, kind, player_id, message, metadata)`
  appends a validated `world_events` row and returns the assigned id and
  timestamp.
- `get_place_by_id_on(conn, place_id)` reads a `Place` row by id from
  the active connection or transaction.
- `get_presence_on(conn, player_id)` reads the player's current
  `PresenceRecord` from the active connection or transaction.
- `get_route_by_id_between_on(conn, route_id, from_place_id,
  to_place_id)` verifies an outbound route id against an exact directed
  origin and destination in the active connection or transaction.
- `inventory::transfer_on(conn, source, destination, item_key, quantity,
  on_commit)` moves owner-keyed inventory using the same semantics as
  `WorldDb::transfer`, but leaves commit or rollback to the surrounding
  transaction.
- `TravelRequest::with_charge_cost_tx` lets travel cost logic call
  transaction-local kit APIs inside the active travel transaction.

## Example

```rust
let result = world.travel(
    TravelRequest::new(player_id, destination_id)
        .with_charge_cost_tx(|tx, presence, _route| {
            foglet_game::transfer_on(
                tx,
                ("player", presence.player_id),
                ("world-sink", 1),
                "movement-token",
                1,
                None::<fn(
                    &rusqlite::Connection,
                    &foglet_game::InventorySlot,
                    &foglet_game::InventorySlot,
                ) -> rusqlite::Result<()>>,
            )?;
            Ok(())
        })
        .with_append_event(|presence, route| {
            Some(TravelEventDraft {
                kind: "travel".to_string(),
                message: format!(
                    "player {} moved from {} to {}",
                    presence.player_id, route.from_place_id, route.to_place_id
                ),
                metadata_json: None,
            })
        }),
)?;
```

If cost charging, validation, presence movement, recall touch, or event
append fails, the whole travel transaction rolls back.

## Drop Dead Nebula workaround mapping

Drop Dead Nebula can replace its normal-flow kit-table SQL like this:

| Current workaround | Replacement in `foglet_game` 0.1.1 |
| --- | --- |
| Direct inserts into `world_events` from market buy/sell and contract completion callbacks | `events::append_event_on` from the active callback transaction |
| Game-local decrement/increment helpers against `inventory_slots` | `inventory::transfer_on` from the active callback transaction |
| Direct `presence` updates during travel turn spending | `WorldDb::travel` |
| Direct `place_recall` insert/update during travel turn spending | `WorldDb::travel` with recall enabled |
| Travel costs that must mutate shared state before movement | `TravelRequest::with_charge_cost_tx` plus the relevant transaction-local kit APIs |

These APIs do not add Drop Dead Nebula concepts to the kit. Owner kinds,
item keys, event kinds, turn policy, cost policy, station/cargo/contract
names, and reward payloads remain game-authored strings or callbacks.
