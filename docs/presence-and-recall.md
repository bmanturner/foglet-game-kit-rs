# Presence and Recall

This document explains two related but distinct primitives in
`foglet_game`:

- `presence`: a player's current location now
- `place_recall`: a player's remembered places over time

## 1. Why they are separate

`presence` and `place_recall` answer different questions:

- **Presence** answers: "Where is this player right now?"
- **Recall** answers: "Which places has this player seen, and when?"

Keeping them separate gives games control over fog-of-war policy:

- A **space exploration** game may only update recall after an explicit
  scan action, not every movement.
- A **dungeon crawler** may touch recall on room entry, but skip secret
  rooms until the player succeeds at detection.

The kit does not auto-touch recall during movement.

## 2. Presence primitive

`crates/foglet_game/src/presence.rs` exposes:

- `WorldDb::set_presence(player_id, place_id, metadata_json)`
- `WorldDb::move_player(player_id, dest_place_id, on_commit)`
- `WorldDb::get_presence(player_id)`
- `WorldDb::players_at(place_id)`

`presence` rows store `player_id`, `place_id`, `entered_at`, and optional
`metadata_json`.

### 2.1 Initial placement

Use `set_presence` when a game decides a player should start somewhere:

```rust
// Space exploration: place a captain at a dock.
let captain_presence = world.set_presence(captain_id, orion_dock_id, None)?;
```

```rust
// Dungeon crawler: place a hero at the crypt entrance.
let hero_presence = world.set_presence(hero_id, crypt_entrance_id, None)?;
```

This is explicit by design: the kit does not auto-place players.

### 2.2 Transactional movement

Use `move_player` for atomic movement plus game-defined commit checks:

```rust
let moved = world.move_player(captain_id, relay_gate_id, |tx, row| {
    // Space exploration: write a flight-log event in the same transaction.
    tx.execute(
        "INSERT INTO flight_log (player_id, place_id) VALUES (?1, ?2)",
        rusqlite::params![row.player_id, row.place_id],
    )?;
    Ok(())
})?;
```

```rust
let moved = world.move_player(hero_id, flooded_hall_id, |tx, row| {
    // Dungeon crawler: reject move if an encounter gate is still locked.
    let locked: i64 = tx.query_row(
        "SELECT locked FROM encounter_gates WHERE place_id = ?1",
        rusqlite::params![row.place_id],
        |r| r.get(0),
    )?;
    if locked == 1 {
        return Err(rusqlite::Error::ExecuteReturnedResults);
    }
    Ok(())
})?;
```

If `on_commit` returns `Err`, the move rolls back and presence stays
unchanged.

## 3. Recall primitive

`crates/foglet_game/src/place_recall.rs` exposes:

- `WorldDb::touch_recall(player_id, place_id, snapshot_json)`
- `WorldDb::recall_for_player(player_id)`

`place_recall` rows store `(player_id, place_id)` plus `first_seen_at`,
`last_seen_at`, and optional `snapshot_json`.

### 3.1 Touch recall when your game chooses

Use `touch_recall` when your game wants to record a seen place:

```rust
// Space exploration: update memory only after a successful station scan.
let recall = world.touch_recall(
    captain_id,
    relay_gate_id,
    Some(r#"{"scan":"complete","threat":"low"}"#),
)?;
```

```rust
// Dungeon crawler: mark chamber knowledge after crossing the threshold.
let recall = world.touch_recall(
    hero_id,
    flooded_hall_id,
    Some(r#"{"torch":"lit","hazard":"slick-stone"}"#),
)?;
```

`touch_recall` is idempotent:

- first call inserts the row
- later calls preserve `first_seen_at`, update `last_seen_at`, and replace
  `snapshot_json`

### 3.2 Query remembered places

`recall_for_player` returns newest-touched-first entries:

```rust
let recent_memory = world.recall_for_player(captain_id)?;
```

```rust
let room_memory = world.recall_for_player(hero_id)?;
```

This ordering supports stable fog-of-war or "recently seen" UI panels.

## 4. Composition pattern

Typical composition is explicit and policy-driven:

1. Validate adjacency with `routes`
2. Call `move_player`
3. Optionally call `touch_recall` if this action should reveal memory

That sequence can be different by game family:

- A **space game** may require a scan action before step 3.
- A **dungeon game** may touch recall on every legal move.
- A **town simulation** may touch recall only for named landmarks.

The kit stays genre-agnostic by not forcing one policy.
