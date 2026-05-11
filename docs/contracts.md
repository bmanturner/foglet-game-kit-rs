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

Opaque payloads:

- `objective_json` is owned by the game. The kit stores it byte-for-byte.
- `reward_json` is owned by the game. The kit never grants rewards.
- `metadata_json` is optional auxiliary data for UI, tags, or auditing.

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
