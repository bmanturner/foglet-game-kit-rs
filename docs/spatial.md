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
- `WorldDb::get_place_by_id(id)` resolves durable row IDs back to
  `Place` rows.
- `get_place_by_id_on(conn, id)` performs the same lookup on an existing
  `rusqlite::Connection` or transaction handle.
- `WorldDb::list_places()` returns deterministic key-ordered rows.

Deterministic ordering keeps admin tools and tests stable across runs.
`get_place_by_key` returns `Ok(None)` for unknown keys, so bootstrap and
admin flows can treat "not authored yet" as a normal branch instead of
falling back to raw SQL.

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
- `WorldDb::get_route_between(from_place_id, to_place_id)` resolves one
  directed route for an exact pair and returns `Ok(None)` when that edge
  has not been authored.
- `WorldDb::get_route_by_id_between(route_id, from_place_id,
  to_place_id)` verifies a route id belongs to that exact directed edge.
- `get_route_by_id_between_on(conn, route_id, from_place_id,
  to_place_id)` performs that verification inside an existing connection
  or transaction without opening a nested transaction.

Two key rules:

- Direction is asymmetric by default: `A -> B` does not create `B -> A`.
- Bidirectional travel requires two rows.

Parallel routes are allowed when they differ by `kind`, so a game can
model multiple channels between the same two places. `get_route_between`
returns the oldest inserted route for a directed pair; use
`outbound_routes` when a game needs to inspect every parallel channel.

Example pair lookup:

```rust
if let Some(route) = world.get_route_between(starport.id, relay.id)? {
    // Game-owned movement code can now inspect route.kind or opaque JSON.
}
```

## 5. Authoring guidance

- Keep `key` stable for the life of the world so saved references and
  migration code remain valid.
- Treat `kind`, `requirements_json`, and `metadata_json` as game-owned
  vocabulary. The kit intentionally does not prescribe economy, combat,
  faction, or navigation semantics.
- Use transactions around higher-level game actions that combine
  adjacency checks with movement or inventory effects.

## 6. Map-backed local nodes

For compact authored maps, `MapNodeTopology` layers named nodes over the
ASCII map parser. The kit parses the grid with `parse_map`, validates
that each declared node anchor appears exactly once, checks exits target
known nodes, and can render the same map with the current node marked.

Descriptions, hazards, loot, and rules stay game-owned:

```rust
use foglet_game::{ChoicePrompt, MapNodeSpec, MapNodeTopology, TileLegend};

struct RoomMeta {
    description: &'static str,
    hazard_key: Option<&'static str>,
}

let legend = TileLegend::from_pairs([
    ("#", "wall"),
    (".", "floor"),
    ("A", "floor"),
    ("B", "floor"),
    ("C", "floor"),
])?;
let topology = MapNodeTopology::from_ascii(
    "#####\n#A.B#\n#..C#\n#####\n",
    &legend,
    [
        MapNodeSpec::new("airlock", 'A', RoomMeta {
            description: "Outer lock, cold and bright.",
            hazard_key: None,
        }).with_exits(["bridge", "cargo"]),
        MapNodeSpec::new("bridge", 'B', RoomMeta {
            description: "Dead consoles face the viewport.",
            hazard_key: Some("sparks"),
        }).with_exits(["airlock"]),
        MapNodeSpec::new("cargo", 'C', RoomMeta {
            description: "Cargo webbing blocks the aft hatch.",
            hazard_key: Some("jammed-door"),
        }).with_exits(["airlock"]),
    ],
)?;
```

A custom `Screen` can render `topology.render_lines(current, '@')` into
its map panel, read `topology.node(current).metadata.description` for
the detail panel, and build a `ChoicePrompt` from
`topology.exits_for(current)`. The hazard system remains ordinary game
code: it can disable choices, spend turns, or update inventory without
the topology helper learning what a hazard means.

## 7. What this layer does not do

- No automatic player placement.
- No automatic movement logic.
- No automatic fog-of-war recall updates.
- No map projection, coordinates, or pathfinding.

Those behaviors are composed by higher-level primitives and game
logic, not embedded in the shared spatial table contract.
