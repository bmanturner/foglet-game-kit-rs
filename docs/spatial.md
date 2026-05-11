# Spatial primitives: places and routes

This document explains the spatial layer in `foglet_game`: durable
`places` plus directed `routes` stored in the shared world DB.

## 1. Why this primitive exists

The spatial graph lets games represent "where things are" without
forcing a specific genre or map model.

- A **space exploration** game can treat places as docks, gates, or
  sectors.
- A **dungeon crawler** can treat places as rooms, halls, or vaults.
- A **town simulation** can treat places as shops, homes, or plazas.

The kit deliberately does not add coordinates, pathfinding, tile grids,
line-of-sight, or any map renderer. It only stores direct adjacency.

## 2. Data model

`crates/foglet_game/src/spatial.rs` exposes:

- `Place` rows: `id`, unique `key`, `display_name`, `kind`,
  `metadata_json`, `created_at`
- `Route` rows: `id`, `from_place_id`, `to_place_id`, `kind`,
  `requirements_json`, `metadata_json`, `created_at`

Both JSON columns are opaque game-owned payloads. The kit stores and
returns them but does not interpret or mutate their meaning.

## 3. Place APIs

### 3.1 Insert a place

Use `WorldDb::insert_place` when seeding or authoring world locations:

```rust
let starport = world.insert_place(
    "orion-starport",
    "Orion Starport",
    "dock",
    Some(r#"{"faction":"relay-guild"}"#),
)?;
```

```rust
let crypt = world.insert_place(
    "ossuary-antechamber",
    "Ossuary Antechamber",
    "room",
    Some(r#"{"hazard":"bone-dust"}"#),
)?;
```

Both examples use the same API and schema. Only your authored values
change by genre.

### 3.2 Look up and list places

- `WorldDb::get_place_by_key(&str)` resolves authored keys to row IDs.
- `WorldDb::list_places()` returns deterministic key-ordered rows.

Deterministic ordering keeps admin tools and tests stable across runs.

## 4. Route APIs

### 4.1 Create directed routes

Use `WorldDb::create_route` to add one explicit directed edge:

```rust
let _lane = world.create_route(
    starport.id,
    relay.id,
    "jump-lane",
    Some(r#"{"clearance":"civilian"}"#),
    None,
)?;
```

```rust
let _stair = world.create_route(
    crypt.id,
    catacomb.id,
    "spiral-stair",
    Some(r#"{"requires":"rusted-key"}"#),
    Some(r#"{"light_level":"dim"}"#),
)?;
```

Both are just directed rows. No reverse link is implied.

### 4.2 Query adjacency

- `WorldDb::outbound_routes(place_id)` returns routes that leave a place.
- `WorldDb::inbound_routes(place_id)` returns routes that arrive at a place.

Two key rules:

- Direction is asymmetric by default: `A -> B` does not create `B -> A`.
- Bidirectional travel requires two rows.

Parallel routes are allowed when they differ by `kind`, so a game can
model multiple channels between the same two places.

## 5. Authoring guidance

- Keep `key` stable for the life of the world so saved references and
  migration code remain valid.
- Treat `kind`, `requirements_json`, and `metadata_json` as game-owned
  vocabulary. The kit intentionally does not prescribe economy, combat,
  faction, or navigation semantics.
- Use transactions around higher-level game actions that combine
  adjacency checks with movement or inventory effects.

## 6. What this layer does not do

- No automatic player placement.
- No automatic movement logic.
- No automatic fog-of-war recall updates.
- No map projection, coordinates, or pathfinding.

Those behaviors are composed by higher-level primitives and game
logic, not embedded in the shared spatial table contract.
