# Inventory primitives: owner-keyed slots and atomic transfer

This document explains the inventory layer in `foglet_game`.

## 1. Why this primitive exists

The inventory primitive provides durable stockpile rows without
prescribing one economy model.

- A **role-playing dungeon game** can track player bags, chests, and vaults.
- A **trading simulation** can track market stalls, warehouses, and caravans.
- A **space exploration game** can track ship cargo holds and station depots.

The kit stores quantities and owner keys. It does not impose currency,
weight, durability, stack caps, or auto-pricing.

## 2. Data model

`crates/foglet_game/src/inventory.rs` exposes `InventorySlot` rows backed
by `inventory_slots`:

- `id`
- `owner_kind`
- `owner_id`
- `item_key`
- `quantity` (non-negative)
- `equilibrium` (optional advisory target)
- `metadata_json` (opaque game-owned payload)

`equilibrium` is advisory only. The kit never auto-drifts `quantity`
toward `equilibrium`.

## 3. Slot authoring and lookup

### 3.1 Create a slot

Use `WorldDb::create_slot` to seed or create stock for an owner:

```rust
// Trading simulation: seed grain in a market stall.
let stall_grain = world.create_slot(
    "market-stall",
    17,
    "grain-sack",
    40,
    Some(60),
    Some(r#"{"quality":"winter"}"#),
)?;
```

```rust
// RPG dungeon game: seed potions in a player inventory.
let hero_potions = world.create_slot(
    "player",
    9,
    "healing-potion",
    3,
    None,
    Some(r#"{"rarity":"common"}"#),
)?;
```

### 3.2 Query one slot or all slots for an owner

- `WorldDb::get_slot(owner_kind, owner_id, item_key)` returns one matching
  row or `None`.
- `WorldDb::slots_for_owner(owner_kind, owner_id)` returns deterministic
  `item_key`-ordered rows for stable UI/admin views.

### 3.3 Optional explicit merge

`WorldDb::merge_slots(destination, source)` merges quantities only when
`owner_kind`, `owner_id`, `item_key`, and `metadata_json` all match
exactly. This keeps merge policy explicit and game-controlled.

## 4. Atomic transfer

Use `WorldDb::transfer(source, destination, item_key, quantity, on_commit)`
for all-or-nothing movement between owners.

Transfer guarantees:

- Source debit and destination credit happen in one transaction.
- Missing source slot, insufficient stock, and invalid quantity reject
  before partial writes persist.
- Optional `on_commit` runs after SQL mutations inside the same
  transaction.
- If `on_commit` returns `Err`, the entire transfer rolls back.

### 4.1 Worked example: RPG loot handoff

```rust
let (after_player, after_chest) = world.transfer(
    ("player", hero_id),
    ("chest", crypt_chest_id),
    "silver-key",
    1,
    Some(|tx, source_after, dest_after| {
        tx.execute(
            "INSERT INTO event_log (kind, source_qty, dest_qty) VALUES (?1, ?2, ?3)",
            rusqlite::params!["loot-drop", source_after.quantity, dest_after.quantity],
        )?;
        Ok(())
    }),
)?;
```

If the callback fails, both slot quantities stay as they were before the
call.

### 4.2 Compose inside a caller-owned transaction

Use `inventory::transfer_on(connection, source, destination, item_key,
quantity, on_commit)` when inventory movement must happen inside an
existing SQLite transaction, such as a contract completion callback or a
travel cost hook. It uses the same validation and transfer algorithm as
`WorldDb::transfer`, but it does not commit; the surrounding transaction
decides whether the mutation persists.

`transfer_on` is the preferred replacement for direct SQL against
`inventory_slots` in normal gameplay code.

### 4.3 Worked example: trading-post commerce

```rust
let (after_caravan, after_stall) = world.transfer(
    ("caravan", caravan_id),
    ("market-stall", stall_id),
    "spice-crate",
    12,
    Some(|tx, source_after, dest_after| {
        // Reject delivery if market tax ledger update fails.
        tx.execute(
            "INSERT INTO tax_ledger (stall_id, item_key, qty) VALUES (?1, ?2, ?3)",
            rusqlite::params![stall_id, dest_after.item_key, 12],
        )?;
        // Example policy check after mutation snapshot.
        if source_after.quantity < 0 || dest_after.quantity < 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }),
)?;
```

This pattern lets games keep economy logic in callbacks without giving up
transactional safety.

## 5. Error variants you can branch on

`InventoryError` provides distinct variants for common transfer failures:

- `MissingSourceSlot`
- `InsufficientStock`
- `InvalidTransferQuantity`
- `TransferRejected` (callback returned `Err`)

These variants support clear player-facing messages and reliable retry
logic.

## 6. Boundaries and non-goals

- No automatic slot merge policy.
- No automatic rebalance toward `equilibrium`.
- No built-in currency, pricing, weight, or durability system.
- No genre-specific assumptions about owners or items.

The inventory primitive is intentionally structural so multiple game
families can reuse it unchanged.
