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

## Contract job views

Use `player_contract_job_views` when rendering a player's accepted and
completed work. The helper projects durable `Contract` rows into
`ContractJobView` values with list/detail fields such as title, summary,
kind label, state label, reward preview, location preview, next step,
requirements, and accept/complete action tokens.

The kit still treats `objective_json` and `reward_json` as opaque. A
game supplies objective readiness through `ContractObjectiveViewProvider`.
That provider can read game-owned proof/progress tables and return
whether an accepted contract is ready to complete, what the next step is,
which requirements are met, and what completion action token the UI
should pass back to game code.

`available_contract_job_views` provides the same projection shape for
available contracts. It complements `JobBoard::query`; it does not
replace the existing available-opportunity provider.

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

Transaction-local contract checks:

- `contract_by_key_for_acceptor_on(connection, player_id, key)` returns
  the matching `Contract` row, if this exact player accepted this exact
  key.
- `contract_state_by_key_for_acceptor_on(connection, player_id, key)`
  returns the typed `ContractState`, if present.
- `acceptor_has_contract_state_on(connection, player_id, key, state)` is
  the small predicate form for travel hooks, outcome callbacks, and
  completion checks.

Prefer these helpers inside existing transactions instead of raw SQL
against `contracts`.

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

## Custom objective example

Use `ContractObjectiveViewProvider` when a contract is ready only after
game-owned requirements are met. The kit projects the row; the game
parses its own `objective_json` and reads its own proof/progress tables.

```rust
let views = player_contract_job_views(&world, player_id, &|world, contract| {
    let objective: CargoProofObjective = serde_json::from_str(&contract.objective_json)?;
    let player_id = contract.acceptor_player_id.expect("accepted contract");
    let at_place = get_presence_on(world.connection(), player_id)?
        .is_some_and(|presence| presence.place_id == objective.archive_place_id);
    let has_cargo = world
        .get_slot("player", player_id, &objective.item_key)?
        .is_some_and(|slot| slot.quantity > 0);
    let has_proof = game_has_salvage_proof(world.connection(), player_id, &objective.proof_key)?;

    Ok(ContractObjectiveView {
        ready_to_complete: at_place && has_cargo && has_proof,
        next_step: Some("Bring cargo and proof to the archive.".to_string()),
        requirements: vec![
            JobRequirementView { label: "At archive".to_string(), met: at_place },
            JobRequirementView { label: "Cargo aboard".to_string(), met: has_cargo },
            JobRequirementView { label: "Proof recorded".to_string(), met: has_proof },
        ],
        complete_action: (at_place && has_cargo && has_proof)
            .then(|| format!("complete:{}", contract.id)),
    })
})?;
```

Completion still uses the normal lifecycle helper. Consume or transfer
required cargo inside the completion callback so contract state and
inventory move atomically:

```rust
world.complete_contract(contract_id, Some(|tx, _contract| {
    foglet_game::transfer_on(
        tx,
        ("player", player_id),
        ("archive", archive_place_id),
        "black-box",
        1,
        None::<fn(&rusqlite::Connection, &InventorySlot, &InventorySlot) -> rusqlite::Result<()>>,
    )
    .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
    Ok(())
}))?;
```

The proof row is game-owned history, so completion should not delete it
just because the item was consumed. `objective_json` remains opaque to
the kit throughout this flow; only the game provider and completion
handler interpret it.
