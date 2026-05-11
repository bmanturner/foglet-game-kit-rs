# Contracts

Contracts are generic work rows in the shared world DB. They model a
game-authored objective and reward payload without teaching the kit what
those payloads mean.

Lifecycle:

- `available` can be accepted by one player.
- `accepted` can become `completed`, `failed`, or `abandoned`.
- `available` and `accepted` rows can expire through explicit expiry
  helpers.
- Invalid transitions return typed errors; transition callbacks run in
  the same transaction and roll back on failure.

Use `WorldDb::create_and_accept_contract` when a game wants to create a
new per-player opportunity and immediately claim it for the current
player. It accepts the same core payload as `WorldDb::create_contract`,
plus the accepting `player_id`, and performs creation, acceptance,
timestamping, and the optional callback inside one SQLite transaction.
If the callback fails or acceptance lifecycle rules reject the row, the
new contract is rolled back instead of being left partially created.

Opaque payloads:

- `objective_json` is owned by the game. The kit stores it byte-for-byte.
- `reward_json` is owned by the game. The kit never grants rewards.
- `metadata_json` is optional auxiliary data for UI, tags, or auditing.

The kit does not inspect, normalize, or validate game-specific objective
or reward semantics. Games still own delivery proof, dungeon objective
checks, credit ledgers, item grants, and faction policy.

Examples:

- RPG escort quest: `kind = "escort"`, objective names an NPC and
  destination room, reward names reputation and coins. Completion can
  run a callback that records the NPC as safe.
- Trading-game delivery: `kind = "delivery"`, objective names cargo,
  pickup owner, destination owner, and deadline, reward names payment.
  Completion can validate that the cargo was delivered before marking
  the contract completed.

When contract completion needs to move inventory or emit events in the
same transaction as the lifecycle transition, use the transaction-local
kit APIs from the callback: `inventory::transfer_on` for
`inventory_slots` and `events::append_event_on` for `world_events`.
Those helpers preserve kit validation while avoiding direct SQL against
kit-owned tables.

Create-and-accept example:

```rust
let accepted = world.create_and_accept_contract(
    CreateContractInput {
        key: Some("guild-job-17"),
        kind: "delivery",
        issuer_owner_kind: "guild",
        issuer_owner_id: 4,
        objective_json: r#"{"pickup":"west-gate","dropoff":"archive"}"#,
        reward_json: r#"{"coins":25,"favor":{"guild":1}}"#,
        metadata_json: None,
        expires_at: None,
    },
    player_id,
    None::<fn(&rusqlite::Transaction<'_>, &Contract) -> rusqlite::Result<()>>,
)?;
```

Use the callback form when accepting the new contract must also reserve
game-owned resources or append a game-owned ledger row. If the callback
returns an error, both the accepted contract and the callback writes roll
back together.
