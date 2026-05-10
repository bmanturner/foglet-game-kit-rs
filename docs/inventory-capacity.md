# Inventory Capacity

Inventory capacity is a v5 helper layered on v4 owner-keyed inventory.
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

Examples:

- RPG backpack weight: rations consume `1`, armor consumes `8`, and a
  player owner has capacity based on strength. Chests can return `None`
  for uncapped storage.
- Town warehouse slots: crates consume shelf slots, bulky machinery uses
  metadata to consume more slots, and municipal warehouses have a fixed
  cap while outdoor yards are uncapped.
