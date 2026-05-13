# Multi-User Test Support

The multi-user harness is test-only and gated behind the
`test-support` Cargo feature:

```bash
cargo test -p foglet_game --features test-support
```

Builder API:

```rust
let harness = MultiUserHarness::builder()
    .add_user("alice", FogletRole::User)
    .add_user("bob", FogletRole::Mod)
    .build()?;
```

The harness creates one temp world DB and one save root per user. Use
`context_for(handle)` or `with_user(handle, f)` when testing screen or
runtime code that needs a `GameContext`. Use `world_db()` for direct
shared-world setup and assertions.

Async-BBS patterns:

- Assert shared state by having one user mutate the world DB, then query
  it from another user's perspective.
- Assert privacy by comparing player-scoped rows such as recall,
  notices, or events.
- `assert_event_visible_to` treats global events as visible and hides
  other users' player-scoped events.
- `assert_notice_for` is available only when the harness is built with
  notices enabled.

The harness is not a scripted terminal driver and does not simulate
real-time multiplayer. It is for deterministic local tests over one
shared SQLite world DB.

## Shared finite state with personal proof

Use the harness for regressions where shared world state and personal
state must diverge:

```rust
let mut harness = MultiUserHarness::builder()
    .add_user("alice", FogletRole::User)
    .add_user("bob", FogletRole::User)
    .build()?;
let alice_id = harness.player_id_for("alice").unwrap();
let bob_id = harness.player_id_for("bob").unwrap();

let derelict_id = {
    let world = harness.world_db_mut();
    world.apply_migration(&INVENTORY_SLOTS_MIGRATION)?;
    world.apply_migration(&PLACES_MIGRATION)?;
    world.apply_migration(&PLACE_RECALL_MIGRATION)?;

    let derelict = world.insert_place("derelict-cache", "Derelict Cache", "site", None)?;
    world.create_slot("site", derelict.id, "black-box", 1, None, None)?;
    world.touch_recall(
        alice_id,
        derelict.id,
        Some(r#"{"personal_note":"entered through the forward lock"}"#),
    )?;
    world.take_finite_pickup_with_capacity(
        ("site", derelict.id),
        ("player", alice_id),
        "black-box",
        1,
        &UnitCapacity,
        Some(|tx, pickup| {
            append_event_on(
                tx,
                "salvage_proof",
                Some(alice_id),
                "Alice recovered the black box.",
                None,
            )
            .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
            if pickup.source_exhausted {
                append_event_on(
                    tx,
                    "site_exhausted",
                    None,
                    "The derelict cache is exhausted.",
                    None,
                )
                .map_err(|err| rusqlite::Error::InvalidParameterName(err.to_string()))?;
            }
            Ok(())
        }),
    )?;
    derelict.id
};

assert_eq!(
    harness
        .world_db()
        .get_slot("site", derelict_id, "black-box")?
        .unwrap()
        .quantity,
    0,
);
harness.assert_event_visible_to("bob", |event| event.kind == "site_exhausted")?;
assert!(harness
    .assert_event_visible_to("bob", |event| event.kind == "salvage_proof")
    .is_err());
assert!(harness
    .world_db()
    .recall_for_player(bob_id)?
    .iter()
    .all(|recall| recall.place_id != derelict_id));
```

The source inventory row is shared and exhausted for everyone. The proof
event is scoped to Alice, and Bob's recall remains personal unless the
game intentionally writes a Bob-scoped recall row or a shared event.
