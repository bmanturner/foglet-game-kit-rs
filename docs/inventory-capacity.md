# Inventory Capacity

Inventory capacity is a helper layered on owner-keyed inventory.
The kit owns the transaction pattern; the game owns the unit and policy.

`CapacityPolicy` supplies two callbacks:

- `item_volume(item_key, metadata)` returns capacity units consumed by
  one unit of an item. Metadata is the slot metadata JSON or `null`.
- `owner_capacity(owner_kind, owner_id)` returns `Some(capacity)` for a
  capped owner or `None` for an uncapped owner.

Helpers:

- `used_capacity` sums `volume * quantity` across one owner's slots.
- `validate_incoming` checks whether an additional quantity would fit.
- `transfer_with_capacity` validates the destination capacity inside
  the same SQLite transaction that debits the source and credits the
  destination, so concurrent transfers cannot collectively exceed the
  cap.
- `grant_inventory_with_capacity_on` validates capacity and then grants
  quantity with metadata-compatible upsert behavior on an existing
  SQLite connection or transaction. Use it for rewards and pickups that
  should respect backpack, hold, or warehouse limits.
- `take_finite_pickup_with_capacity` transfers from a finite source
  owner to a destination owner, validates destination capacity, returns
  whether the source was exhausted, and lets game-owned callback writes
  roll back with the inventory movement.

Examples:

- RPG backpack weight: rations consume `1`, armor consumes `8`, and a
  player owner has capacity based on strength. Chests can return `None`
  for uncapped storage.
- Town warehouse slots: crates consume shelf slots, bulky machinery uses
  metadata to consume more slots, and municipal warehouses have a fixed
  cap while outdoor yards are uncapped.

## Finite Pickup Pattern

Use owner-keyed inventory for both sides of a finite pickup:

- The shared source owner might be `("site", derelict_id)` or
  `("chest", room_id)`.
- The destination owner might be `("player", player_id)` or
  `("ship", ship_id)`.
- Narrative state, proof rows, and event text stay in game-owned tables.

```rust
let result = world.take_finite_pickup_with_capacity(
    ("site", derelict_id),
    ("player", player_id),
    "black-box",
    1,
    &cargo_policy,
    Some(|tx, pickup| {
        tx.execute(
            "INSERT INTO salvage_proof (player_id, item_key, source_exhausted)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![
                player_id,
                &pickup.destination.item_key,
                if pickup.source_exhausted { 1_i64 } else { 0_i64 },
            ],
        )?;
        Ok(())
    }),
)?;
```

The kit-owned part is only the inventory transfer and capacity check.
If capacity fails, no inventory moves and the callback is not run. If
the callback returns an error after writing game-owned story/proof state,
the transaction rolls back both the inventory movement and those
callback writes.
