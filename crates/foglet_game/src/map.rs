//! ASCII map parser.
//!
//! Foglet door games describe their world with two artifacts:
//!
//! 1. **A tile legend** in TOML — a `[tiles]` table mapping a single
//!    character glyph to a semantic kind string (`"wall"`, `"floor"`.
//!    `"door"`, `"npc:<id>"`, `"item:<id>"`, or an author-defined
//!    custom kind). ships this alongside the map so the same
//!    glyph can mean different things in different games.
//! 2. **An ASCII grid** — a plain-text rectangle where every cell
//!    matches a glyph in the legend. The player spawn glyph (`@`) is
//!    treated like any other glyph by the parser; `[game].start_x` /
//!    `start_y` in `assets/game.toml` are the authoritative spawn.
//!    so games typically map `@` to "floor" in their legend and rely
//!    on config for placement.
//!
//! # Why parse this in the library and not the game
//!
//! Every door game would otherwise re-implement the same loop: walk
//! the grid, stamp a tile per glyph, tease NPC/item placements out of
//! the cells. Centralising the parser here means:
//!
//! - The walkability rule ("walls are solid, floors and doors are
//!   passable, NPCs/items occupy a passable cell underneath") is
//!   defined once.
//! - Authoring errors (unknown glyph, ragged grid) raise the same
//!   message everywhere instead of silently producing weird worlds.
//! - The `fgk new` template can ship a working map without re-deriving
//!   the parser.
//!
//! # What this module does NOT do
//!
//! - It does not load files — game code reads the grid string and
//!   passes it to [`parse_map`]. This keeps the parser pure and
//!   testable, and lets the runtime decide whether maps come from
//!   `include_str!`, `assets/maps/*.txt`, or somewhere else.
//! - It does not enforce a player-spawn glyph. names
//!   `start_x` `start_y` in `[game]` as the spawn, so spawn lives
//!   in config, not the map.
//! - It does not implement movement or pathing. Walkability is a
//!   per-tile predicate; how a `Screen` consumes it is the game's
//!   choice.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Glyph reserved for the player spawn marker in author-facing maps.
///
/// The parser does not require this glyph to be present, but games
/// that *do* use `@` in their grid generally want it treated as
/// floor (the player stands on a walkable tile). Documented here so
/// the constant has one canonical spelling rather than being
/// scattered through templates.
pub const PLAYER_GLYPH: char = '@';

/// Semantic meaning of a tile, derived from a legend entry.
///
/// The legend is a string-typed map in TOML; we promote its values
/// to a typed enum so games can pattern-match instead of comparing
/// strings. The `Custom` variant preserves the original legend
/// string verbatim — this is the escape hatch for author-defined
/// terrain (lava, water, ladder, etc.) without forcing the to
/// enumerate every possibility.
///
/// # Walkability
///
/// `Wall` is the only built-in *blocking* kind. `Floor`, `Door`.
/// `Npc`, and `Item` are walkable so the player can stand on them
/// (NPCs and items occupy a floor-equivalent cell underneath; the
/// game decides whether to block movement when the player tries to
/// step *through* the entity). `Custom` defaults to walkable; if a
/// game needs blocking custom terrain, it should consult its own
/// state — keeping `Custom` permissive avoids silently locking
/// players out of areas behind unrecognised glyphs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TileKind {
    /// Solid wall. Not walkable.
    Wall,
    /// Open floor. Walkable.
    Floor,
    /// Door tile (open passage). Walkable. Locked-door semantics are
    /// game-defined; the parser does not encode lock state.
    Door,
    /// NPC anchor — the cell hosts an NPC identified by `id`. The
    /// underlying tile is treated as floor for walkability purposes.
    Npc(String),
    /// Item anchor — the cell hosts a collectable item identified by
    /// `id`. The underlying tile is treated as floor.
    Item(String),
    /// Author-defined kind. Treated as walkable. Use this for
    /// game-specific terrain that doesn't fit the built-in kinds.
    Custom(String),
}

impl TileKind {
    /// Whether the player can stand on this tile.
    ///
    /// Centralised so renderers and movement code reach for the same
    /// answer; see the doc on [`TileKind`] for the rule.
    pub fn is_walkable(&self) -> bool {
        !matches!(self, TileKind::Wall)
    }
}

/// Legend mapping single-character glyphs to [`TileKind`] values.
///
/// Built from the `[tiles]` TOML table that ships alongside each
/// map. The legend is intentionally tiny (one glyph → one kind) so
/// authors can read a map at a glance and the parser can reject
/// unknown glyphs with a precise error.
///
/// # Why a wrapper instead of `HashMap<char, TileKind>` directly
///
/// 1. The constructor enforces "exactly one character per key".
///    which `serde` won't catch on its own.
/// 2. It's the natural place to hang a [`TileLegend::lookup`] helper
///    that returns the structured error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileLegend {
    glyphs: HashMap<char, TileKind>,
}

impl TileLegend {
    /// Build a legend from raw `(glyph_string, kind_string)` pairs as
    /// they appear in TOML.
    ///
    /// Each `glyph_string` MUST be exactly one Unicode scalar value;
    /// each `kind_string` is interpreted by the internal `parse_kind`
    /// helper (see this module's source — recognised forms are
    /// `wall`, `floor`, `door`, `npc:<id>`, `item:<id>`, or anything
    /// else as [`TileKind::Custom`]). This constructor is the single
    /// funnel through which legend data enters the type system.
    pub fn from_pairs<I, S>(pairs: I) -> Result<Self, MapError>
    where
        I: IntoIterator<Item = (S, S)>,
        S: AsRef<str>,
    {
        let mut glyphs = HashMap::new();
        for (raw_glyph, raw_kind) in pairs {
            let glyph_str = raw_glyph.as_ref();
            let mut chars = glyph_str.chars();
            let glyph = chars.next().ok_or(MapError::EmptyGlyph)?;
            if chars.next().is_some() {
                return Err(MapError::MultiCharGlyph(glyph_str.to_string()));
            }
            let kind = parse_kind(raw_kind.as_ref())?;
            glyphs.insert(glyph, kind);
        }
        Ok(Self { glyphs })
    }

    /// Look up the kind for a glyph, returning the structured
    /// "unknown glyph" error so callers don't have to re-wrap.
    pub fn lookup(&self, glyph: char) -> Result<&TileKind, MapError> {
        self.glyphs
            .get(&glyph)
            .ok_or(MapError::UnknownGlyph { glyph })
    }

    /// Number of glyphs registered. Useful for tests and diagnostics.
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Whether the legend contains zero glyphs.
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// Parse a legend value string into a [`TileKind`].
///
/// Recognised forms:
///
/// - `"wall"` → [`TileKind::Wall`]
/// - `"floor"` → [`TileKind::Floor`]
/// - `"door"` → [`TileKind::Door`]
/// - `"npc:<id>"` → [`TileKind::Npc`] with `<id>` (must be non-empty)
/// - `"item:<id>"` → [`TileKind::Item`] with `<id>` (must be non-empty)
/// - anything else → [`TileKind::Custom`] preserving the raw string
///
/// Empty input is rejected so a legend with `"" = "floor"` can't sneak
/// through.
fn parse_kind(raw: &str) -> Result<TileKind, MapError> {
    if raw.is_empty() {
        return Err(MapError::EmptyKind);
    }
    if let Some(id) = raw.strip_prefix("npc:") {
        if id.is_empty() {
            return Err(MapError::EmptyEntityId {
                kind: "npc".to_string(),
            });
        }
        return Ok(TileKind::Npc(id.to_string()));
    }
    if let Some(id) = raw.strip_prefix("item:") {
        if id.is_empty() {
            return Err(MapError::EmptyEntityId {
                kind: "item".to_string(),
            });
        }
        return Ok(TileKind::Item(id.to_string()));
    }
    Ok(match raw {
        "wall" => TileKind::Wall,
        "floor" => TileKind::Floor,
        "door" => TileKind::Door,
        other => TileKind::Custom(other.to_string()),
    })
}

/// One cell in a parsed [`Map`].
///
/// Stored as the [`TileKind`] resolved through the legend at parse
/// time, plus the originating `glyph` so renderers can fall back to
/// raw ASCII without re-stringifying the kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    /// Original character from the source grid.
    pub glyph: char,
    /// Resolved semantic kind from the legend.
    pub kind: TileKind,
}

impl Tile {
    /// Convenience: walkability of this cell. Mirrors
    /// [`TileKind::is_walkable`].
    pub fn is_walkable(&self) -> bool {
        self.kind.is_walkable()
    }
}

/// A placement extracted from the grid for entity-like tiles
/// (currently NPCs and items).
///
/// The map renderer typically draws the floor under the entity and
/// lets the entity layer overlay its own glyph; the placement record
/// is what the entity layer reads to decide where to spawn each NPC
/// or item. Carrying the placement alongside the [`Map`] keeps the
/// parser the single owner of "where things start out".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityPlacement {
    /// Column (0-based, left to right).
    pub x: u16,
    /// Row (0-based, top to bottom).
    pub y: u16,
    /// What kind of entity stood on this cell. Currently always
    /// [`TileKind::Npc`] or [`TileKind::Item`]; gated on those
    /// variants by the parser.
    pub kind: TileKind,
}

/// Parsed ASCII map.
///
/// The grid is rectangular by construction (the parser rejects ragged
/// input). `entities` is the *separate* list of NPC/item anchors
/// the corresponding cells in `cells` are stored as their entity kind
/// so renderers that want to honour the original glyph can do so;
/// movement code that wants "is this cell walkable" should still call
/// [`Map::is_walkable`] which collapses entity tiles to their
/// floor-equivalent passability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    /// Width in columns.
    pub width: u16,
    /// Height in rows.
    pub height: u16,
    /// `cells[y][x]` — row-major. Always exactly `height` rows of
    /// exactly `width` tiles.
    pub cells: Vec<Vec<Tile>>,
    /// Entity placements harvested from `Npc(_)` `Item(_)` cells.
    /// in row-major reading order.
    pub entities: Vec<EntityPlacement>,
}

impl Map {
    /// Return the tile at `(x, y)`, or `None` if out of bounds.
    pub fn tile_at(&self, x: u16, y: u16) -> Option<&Tile> {
        let row = self.cells.get(y as usize)?;
        row.get(x as usize)
    }

    /// Whether `(x, y)` is in-bounds. Cheaper than `tile_at` when the
    /// caller doesn't need the tile itself.
    pub fn in_bounds(&self, x: u16, y: u16) -> bool {
        x < self.width && y < self.height
    }

    /// Whether the player can stand on `(x, y)`.
    ///
    /// Out-of-bounds coordinates return `false` so movement code can
    /// uniformly treat "off the map" as a wall without first
    /// bounds-checking.
    pub fn is_walkable(&self, x: u16, y: u16) -> bool {
        self.tile_at(x, y).is_some_and(Tile::is_walkable)
    }
}

/// Game-authored node declaration for [`MapNodeTopology`].
///
/// A node is anchored by one glyph on the ASCII map and carries any
/// game-owned metadata the caller wants to keep with that room, station,
/// encounter space, or local-map point. The kit validates anchors and
/// exits, but it does not inspect `metadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapNodeSpec<M> {
    /// Stable game key for this node.
    pub key: String,
    /// Glyph that must appear exactly once in the parsed map.
    pub anchor: char,
    /// Declared outbound exit target keys in deterministic render/order
    /// order.
    pub exits: Vec<String>,
    /// Game-owned metadata for descriptions, hazards, encounter rules,
    /// or any other domain state.
    pub metadata: M,
}

impl<M> MapNodeSpec<M> {
    /// Create a node declaration with no exits.
    pub fn new(key: impl Into<String>, anchor: char, metadata: M) -> Self {
        Self {
            key: key.into(),
            anchor,
            exits: Vec::new(),
            metadata,
        }
    }

    /// Attach outbound exits in declaration order.
    #[must_use]
    pub fn with_exits<I, S>(mut self, exits: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.exits = exits.into_iter().map(Into::into).collect();
        self
    }
}

/// One validated node in a [`MapNodeTopology`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapNode<M> {
    /// Stable game key for this node.
    pub key: String,
    /// Anchor glyph that located this node on the source ASCII map.
    pub anchor: char,
    /// Column of the anchor glyph.
    pub x: u16,
    /// Row of the anchor glyph.
    pub y: u16,
    /// Declared outbound exits in deterministic order.
    pub exits: Vec<MapNodeExit>,
    /// Game-owned metadata carried through from [`MapNodeSpec`].
    pub metadata: M,
}

/// One declared outbound edge from a map-backed node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapNodeExit {
    /// Target node key.
    pub target_key: String,
}

/// Parsed ASCII map plus validated named-node anchors and exits.
///
/// This helper is intentionally local-map oriented. Games still own room
/// prose, hazards, encounter resolution, pickup rules, and durable world
/// state. The kit only verifies that authored node anchors exist on the
/// same grid the screen renders and that declared exits target known
/// nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapNodeTopology<M> {
    map: Map,
    nodes: Vec<MapNode<M>>,
    node_index_by_key: HashMap<String, usize>,
}

impl<M> MapNodeTopology<M> {
    /// Parse `text` with [`parse_map`] and validate game-authored node
    /// declarations against the parsed grid.
    pub fn from_ascii<I>(
        text: &str,
        legend: &TileLegend,
        node_specs: I,
    ) -> Result<Self, MapNodeTopologyError>
    where
        I: IntoIterator<Item = MapNodeSpec<M>>,
    {
        let map = parse_map(text, legend)?;
        let mut specs = Vec::new();
        let mut known_keys = HashSet::new();
        let mut declared_anchors: HashMap<char, String> = HashMap::new();

        for spec in node_specs {
            if !known_keys.insert(spec.key.clone()) {
                return Err(MapNodeTopologyError::DuplicateNodeKey { node_key: spec.key });
            }
            if let Some(first_node_key) = declared_anchors.insert(spec.anchor, spec.key.clone()) {
                return Err(MapNodeTopologyError::DuplicateDeclaredAnchor {
                    anchor: spec.anchor,
                    first_node_key,
                    second_node_key: spec.key,
                });
            }
            specs.push(spec);
        }

        let mut anchors_by_key = HashMap::new();
        for spec in &specs {
            let positions = anchor_positions(&map, spec.anchor);
            match positions.as_slice() {
                [] => {
                    return Err(MapNodeTopologyError::MissingNodeAnchor {
                        node_key: spec.key.clone(),
                        anchor: spec.anchor,
                    });
                }
                [(x, y)] => {
                    anchors_by_key.insert(spec.key.clone(), (*x, *y));
                }
                _ => {
                    return Err(MapNodeTopologyError::DuplicateNodeAnchor {
                        node_key: spec.key.clone(),
                        anchor: spec.anchor,
                        count: positions.len(),
                    });
                }
            }
        }

        for spec in &specs {
            for target_key in &spec.exits {
                if !known_keys.contains(target_key) {
                    return Err(MapNodeTopologyError::UnknownExit {
                        node_key: spec.key.clone(),
                        target_key: target_key.clone(),
                    });
                }
            }
        }

        let mut nodes = Vec::with_capacity(specs.len());
        let mut node_index_by_key = HashMap::with_capacity(specs.len());
        for spec in specs {
            let Some((x, y)) = anchors_by_key.remove(&spec.key) else {
                return Err(MapNodeTopologyError::MissingNodeAnchor {
                    node_key: spec.key,
                    anchor: spec.anchor,
                });
            };
            let exits = spec
                .exits
                .into_iter()
                .map(|target_key| MapNodeExit { target_key })
                .collect();
            node_index_by_key.insert(spec.key.clone(), nodes.len());
            nodes.push(MapNode {
                key: spec.key,
                anchor: spec.anchor,
                x,
                y,
                exits,
                metadata: spec.metadata,
            });
        }

        Ok(Self {
            map,
            nodes,
            node_index_by_key,
        })
    }

    /// Parsed backing map.
    #[must_use]
    pub fn map(&self) -> &Map {
        &self.map
    }

    /// Validated nodes in declaration order.
    #[must_use]
    pub fn nodes(&self) -> &[MapNode<M>] {
        &self.nodes
    }

    /// Look up one node by key.
    #[must_use]
    pub fn node(&self, node_key: &str) -> Option<&MapNode<M>> {
        let index = self.node_index_by_key.get(node_key)?;
        self.nodes.get(*index)
    }

    /// Return declared exits for `node_key` in deterministic order.
    pub fn exits_for(&self, node_key: &str) -> Result<&[MapNodeExit], MapNodeTopologyError> {
        self.node(node_key)
            .map(|node| node.exits.as_slice())
            .ok_or_else(|| MapNodeTopologyError::UnknownNode {
                node_key: node_key.to_string(),
            })
    }

    /// Render map rows with `current_marker` overlaying the current
    /// node's anchor cell.
    ///
    /// All other cells render their original source glyphs, so games do
    /// not need separate hard-coded "current room" art.
    pub fn render_lines(
        &self,
        current_node_key: &str,
        current_marker: char,
    ) -> Result<Vec<String>, MapNodeTopologyError> {
        let current =
            self.node(current_node_key)
                .ok_or_else(|| MapNodeTopologyError::UnknownNode {
                    node_key: current_node_key.to_string(),
                })?;
        let mut lines = Vec::with_capacity(self.map.cells.len());
        for (y, row) in self.map.cells.iter().enumerate() {
            let mut line = String::with_capacity(row.len());
            for (x, tile) in row.iter().enumerate() {
                if x == usize::from(current.x) && y == usize::from(current.y) {
                    line.push(current_marker);
                } else {
                    line.push(tile.glyph);
                }
            }
            lines.push(line);
        }
        Ok(lines)
    }
}

/// Errors produced while building or querying a [`MapNodeTopology`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MapNodeTopologyError {
    /// The underlying ASCII map failed validation.
    #[error(transparent)]
    Map(#[from] MapError),
    /// Two node declarations used the same key.
    #[error("duplicate map node key `{node_key}`")]
    DuplicateNodeKey {
        /// Repeated node key.
        node_key: String,
    },
    /// Two node declarations used the same anchor glyph.
    #[error(
        "duplicate declared map node anchor {anchor:?} for `{first_node_key}` and `{second_node_key}`"
    )]
    DuplicateDeclaredAnchor {
        /// Anchor glyph reused by two declarations.
        anchor: char,
        /// First node key that declared the glyph.
        first_node_key: String,
        /// Second node key that declared the glyph.
        second_node_key: String,
    },
    /// A declared anchor glyph was not present in the parsed map.
    #[error("map node `{node_key}` anchor {anchor:?} is missing from the map")]
    MissingNodeAnchor {
        /// Node key whose anchor was missing.
        node_key: String,
        /// Expected anchor glyph.
        anchor: char,
    },
    /// A declared anchor glyph appeared more than once in the parsed map.
    #[error("map node `{node_key}` anchor {anchor:?} appears {count} times")]
    DuplicateNodeAnchor {
        /// Node key whose anchor was repeated.
        node_key: String,
        /// Repeated anchor glyph.
        anchor: char,
        /// Number of occurrences found in the parsed map.
        count: usize,
    },
    /// A declared exit target does not match any node key.
    #[error("map node `{node_key}` declares exit to unknown node `{target_key}`")]
    UnknownExit {
        /// Node key containing the bad exit.
        node_key: String,
        /// Unknown target node key.
        target_key: String,
    },
    /// A query referenced a node key that is not in this topology.
    #[error("unknown map node `{node_key}`")]
    UnknownNode {
        /// Unknown node key.
        node_key: String,
    },
}

fn anchor_positions(map: &Map, anchor: char) -> Vec<(u16, u16)> {
    let mut positions = Vec::new();
    for (y, row) in map.cells.iter().enumerate() {
        for (x, tile) in row.iter().enumerate() {
            if tile.glyph == anchor {
                positions.push((
                    u16::try_from(x).unwrap_or(u16::MAX),
                    u16::try_from(y).unwrap_or(u16::MAX),
                ));
            }
        }
    }
    positions
}

/// Errors produced by [`parse_map`] and the legend constructor.
///
/// All variants carry enough structured context that a CLI wrapping
/// them with `anyhow` can render a useful single-line message
/// (`fgk run` does this) without losing the typed branch for tests.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MapError {
    /// Source text was empty or contained only blank lines.
    #[error("map is empty (no rows)")]
    Empty,
    /// Rows in the grid disagreed on width — `expected` came from the
    /// first non-blank row, `found` is the offending row.
    #[error("ragged map: row {row} has width {found}, expected {expected}")]
    Ragged {
        /// 0-based row index.
        row: u16,
        /// Width of the first non-blank row.
        expected: u16,
        /// Width of the offending row.
        found: u16,
    },
    /// Glyph appeared in the grid but not in the legend.
    #[error("unknown glyph {glyph:?} in map")]
    UnknownGlyph {
        /// The unknown glyph as it appeared in the source.
        glyph: char,
    },
    /// Legend entry's glyph string was the empty string.
    #[error("legend glyph must be exactly one character (was empty)")]
    EmptyGlyph,
    /// Legend entry's glyph string had more than one character.
    #[error("legend glyph must be exactly one character (got {0:?})")]
    MultiCharGlyph(String),
    /// Legend entry mapped a glyph to an empty kind string.
    #[error("legend kind string is empty")]
    EmptyKind,
    /// `npc:` or `item:` prefix was given without an id (e.g. `npc:`).
    #[error("legend kind {kind:?} requires a non-empty id")]
    EmptyEntityId {
        /// The kind prefix (`"npc"` or `"item"`).
        kind: String,
    },
}

/// Parse an ASCII grid against `legend` into a [`Map`].
///
/// The parser:
///
/// 1. Strips trailing blank lines (lets authors leave a newline at
///    EOF without tripping the "empty row" rule).
/// 2. Locks the map width to the first non-blank row's character
///    count.
/// 3. Rejects any subsequent row whose width disagrees.
/// 4. Resolves every glyph through `legend`; an unknown glyph aborts
///    with [`MapError::UnknownGlyph`].
/// 5. Harvests entity placements (NPC/item) into `Map::entities` in
///    row-major reading order.
///
/// # Errors
///
/// See [`MapError`] for the full enumeration. The function never
/// panics on malformed input — every failure mode is surfaced as a
/// typed variant.
pub fn parse_map(text: &str, legend: &TileLegend) -> Result<Map, MapError> {
    let trimmed_lines: Vec<&str> = text
        .lines()
        .skip_while(|l| l.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .skip_while(|l| l.is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    if trimmed_lines.is_empty() {
        return Err(MapError::Empty);
    }

    let width = trimmed_lines[0].chars().count();
    if width == 0 {
        return Err(MapError::Empty);
    }
    let width_u16 = u16::try_from(width).unwrap_or(u16::MAX);

    let mut cells: Vec<Vec<Tile>> = Vec::with_capacity(trimmed_lines.len());
    let mut entities: Vec<EntityPlacement> = Vec::new();

    for (row_idx, line) in trimmed_lines.iter().enumerate() {
        let row_chars: Vec<char> = line.chars().collect();
        if row_chars.len() != width {
            return Err(MapError::Ragged {
                row: u16::try_from(row_idx).unwrap_or(u16::MAX),
                expected: width_u16,
                found: u16::try_from(row_chars.len()).unwrap_or(u16::MAX),
            });
        }
        let mut row: Vec<Tile> = Vec::with_capacity(width);
        for (col_idx, glyph) in row_chars.into_iter().enumerate() {
            let kind = legend.lookup(glyph)?.clone();
            if matches!(kind, TileKind::Npc(_) | TileKind::Item(_)) {
                entities.push(EntityPlacement {
                    x: u16::try_from(col_idx).unwrap_or(u16::MAX),
                    y: u16::try_from(row_idx).unwrap_or(u16::MAX),
                    kind: kind.clone(),
                });
            }
            row.push(Tile { glyph, kind });
        }
        cells.push(row);
    }

    let height_u16 = u16::try_from(cells.len()).unwrap_or(u16::MAX);

    Ok(Map {
        width: width_u16,
        height: height_u16,
        cells,
        entities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a legend that covers every glyph in the example
    /// plus the player marker mapped to floor (the convention `fgk new`
    /// templates follow).
    fn spec_legend() -> TileLegend {
        TileLegend::from_pairs([
            ("#", "wall"),
            (".", "floor"),
            ("+", "door"),
            ("K", "npc:night_clerk"),
            ("@", "floor"),
        ])
        .expect("static legend parses")
    }

    #[test]
    fn parses_spec_example_map() {
        let src = "\
########################
#..........#...........#
#..@.......+.....K.....#
#..........#...........#
########+###############
";
        let legend = spec_legend();
        let map = parse_map(src, &legend).expect("spec example parses");

        assert_eq!(map.width, 24);
        assert_eq!(map.height, 5);
        assert_eq!(map.cells.len(), 5);
        for row in &map.cells {
            assert_eq!(row.len(), 24);
        }

        // Walls form the perimeter (corners + first/last row).
        assert!(matches!(map.tile_at(0, 0).unwrap().kind, TileKind::Wall));
        assert!(matches!(map.tile_at(23, 0).unwrap().kind, TileKind::Wall));
        assert!(matches!(map.tile_at(0, 4).unwrap().kind, TileKind::Wall));
        assert!(matches!(map.tile_at(23, 4).unwrap().kind, TileKind::Wall));

        // Doors at the column-11 split on row 2 and at column 8 on the
        // bottom wall.
        assert!(matches!(map.tile_at(11, 2).unwrap().kind, TileKind::Door));
        assert!(matches!(map.tile_at(8, 4).unwrap().kind, TileKind::Door));

        // NPC harvested.
        assert_eq!(map.entities.len(), 1);
        assert_eq!(map.entities[0].x, 17);
        assert_eq!(map.entities[0].y, 2);
        assert_eq!(
            map.entities[0].kind,
            TileKind::Npc("night_clerk".to_string())
        );
    }

    #[test]
    fn walkability_rule() {
        let legend = spec_legend();
        let src = "\
##.
#@+
##.
";
        let map = parse_map(src, &legend).unwrap();

        assert!(!map.is_walkable(0, 0)); // wall
        assert!(map.is_walkable(2, 0)); // floor
        assert!(map.is_walkable(1, 1)); // floor (player marker glyph mapped to floor)
        assert!(map.is_walkable(2, 1)); // door is walkable
                                        // Out of bounds is treated as not walkable so movement code
                                        // doesn't have to bounds-check separately.
        assert!(!map.is_walkable(99, 99));
        assert!(!map.in_bounds(99, 99));
    }

    #[test]
    fn rejects_ragged_grid() {
        let legend = spec_legend();
        let src = "\
####
###
####
";
        let err = parse_map(src, &legend).unwrap_err();
        assert_eq!(
            err,
            MapError::Ragged {
                row: 1,
                expected: 4,
                found: 3,
            }
        );
    }

    #[test]
    fn rejects_unknown_glyph() {
        let legend = spec_legend();
        let src = "\
####
#?.#
####
";
        let err = parse_map(src, &legend).unwrap_err();
        assert_eq!(err, MapError::UnknownGlyph { glyph: '?' });
    }

    #[test]
    fn rejects_empty_input() {
        let legend = spec_legend();
        assert_eq!(parse_map("", &legend).unwrap_err(), MapError::Empty);
        assert_eq!(parse_map("\n\n\n", &legend).unwrap_err(), MapError::Empty);
    }

    #[test]
    fn ignores_leading_and_trailing_blank_lines() {
        let legend = spec_legend();
        let src = "\n\n\n##\n..\n\n";
        let map = parse_map(src, &legend).expect("blank-bookended map parses");
        assert_eq!(map.height, 2);
        assert_eq!(map.width, 2);
    }

    #[test]
    fn legend_rejects_multi_char_glyph() {
        let err = TileLegend::from_pairs([("##", "wall")]).unwrap_err();
        assert_eq!(err, MapError::MultiCharGlyph("##".to_string()));
    }

    #[test]
    fn legend_rejects_empty_glyph() {
        let err = TileLegend::from_pairs([("", "wall")]).unwrap_err();
        assert_eq!(err, MapError::EmptyGlyph);
    }

    #[test]
    fn legend_rejects_empty_kind() {
        let err = TileLegend::from_pairs([("#", "")]).unwrap_err();
        assert_eq!(err, MapError::EmptyKind);
    }

    #[test]
    fn legend_rejects_entity_without_id() {
        let err = TileLegend::from_pairs([("N", "npc:")]).unwrap_err();
        assert_eq!(
            err,
            MapError::EmptyEntityId {
                kind: "npc".to_string()
            }
        );
        let err = TileLegend::from_pairs([("I", "item:")]).unwrap_err();
        assert_eq!(
            err,
            MapError::EmptyEntityId {
                kind: "item".to_string()
            }
        );
    }

    #[test]
    fn parses_item_placements() {
        let legend = TileLegend::from_pairs([
            ("#", "wall"),
            (".", "floor"),
            ("k", "item:rusty_key"),
            ("c", "item:candlestick"),
        ])
        .unwrap();
        let src = "\
####
#k.#
#.c#
####
";
        let map = parse_map(src, &legend).unwrap();
        assert_eq!(map.entities.len(), 2);
        assert_eq!(
            map.entities[0],
            EntityPlacement {
                x: 1,
                y: 1,
                kind: TileKind::Item("rusty_key".to_string()),
            }
        );
        assert_eq!(
            map.entities[1],
            EntityPlacement {
                x: 2,
                y: 2,
                kind: TileKind::Item("candlestick".to_string()),
            }
        );
    }

    #[test]
    fn custom_kind_is_walkable_and_round_trips_string() {
        let legend = TileLegend::from_pairs([("#", "wall"), (".", "floor"), ("~", "water")])
            .expect("custom legend parses");
        let map = parse_map("###\n#~#\n###\n", &legend).unwrap();
        let tile = map.tile_at(1, 1).unwrap();
        assert_eq!(tile.kind, TileKind::Custom("water".to_string()));
        assert!(tile.is_walkable());
    }

    #[test]
    fn legend_lookup_reports_unknown_glyph() {
        let legend = spec_legend();
        let err = legend.lookup('?').unwrap_err();
        assert_eq!(err, MapError::UnknownGlyph { glyph: '?' });
    }

    #[test]
    fn legend_len_and_is_empty_track_pairs() {
        let empty = TileLegend::from_pairs::<_, &str>([]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let two = TileLegend::from_pairs([("#", "wall"), (".", "floor")]).unwrap();
        assert!(!two.is_empty());
        assert_eq!(two.len(), 2);
    }

    fn topology_legend() -> TileLegend {
        TileLegend::from_pairs([
            ("#", "wall"),
            (".", "floor"),
            ("A", "floor"),
            ("B", "floor"),
            ("C", "floor"),
        ])
        .expect("static topology legend parses")
    }

    #[test]
    fn node_topology_lists_exits_and_renders_current_node() {
        let topology = MapNodeTopology::from_ascii(
            "\
#####
#A.B#
#..C#
#####
",
            &topology_legend(),
            [
                MapNodeSpec::new("airlock", 'A', "Airlock").with_exits(["bridge", "cargo"]),
                MapNodeSpec::new("bridge", 'B', "Bridge").with_exits(["airlock"]),
                MapNodeSpec::new("cargo", 'C', "Cargo Bay").with_exits(["airlock"]),
            ],
        )
        .expect("topology validates");

        let exits = topology.exits_for("airlock").expect("airlock exits");
        assert_eq!(
            exits
                .iter()
                .map(|exit| exit.target_key.as_str())
                .collect::<Vec<_>>(),
            vec!["bridge", "cargo"],
            "exits should remain in declaration order"
        );
        assert_eq!(
            topology
                .render_lines("bridge", PLAYER_GLYPH)
                .expect("map renders")
                .join("\n"),
            "#####\n#A.@#\n#..C#\n#####"
        );
        assert_eq!(
            topology.node("cargo").expect("cargo node").metadata,
            "Cargo Bay"
        );
    }

    #[test]
    fn node_topology_surfaces_ragged_map_errors() {
        let err = MapNodeTopology::<()>::from_ascii(
            "\
###
##
",
            &topology_legend(),
            [],
        )
        .expect_err("ragged source should fail");

        assert_eq!(
            err,
            MapNodeTopologyError::Map(MapError::Ragged {
                row: 1,
                expected: 3,
                found: 2,
            })
        );
    }

    #[test]
    fn node_topology_surfaces_unknown_glyph_errors() {
        let err = MapNodeTopology::<()>::from_ascii(
            "\
###
#?#
###
",
            &topology_legend(),
            [],
        )
        .expect_err("unknown glyph should fail");

        assert_eq!(
            err,
            MapNodeTopologyError::Map(MapError::UnknownGlyph { glyph: '?' })
        );
    }

    #[test]
    fn node_topology_rejects_missing_anchor() {
        let err = MapNodeTopology::from_ascii(
            "\
#####
#A.B#
#####
",
            &topology_legend(),
            [MapNodeSpec::new("cargo", 'C', ())],
        )
        .expect_err("missing anchor should fail");

        assert_eq!(
            err,
            MapNodeTopologyError::MissingNodeAnchor {
                node_key: "cargo".to_string(),
                anchor: 'C',
            }
        );
    }

    #[test]
    fn node_topology_rejects_duplicate_anchor_occurrences() {
        let err = MapNodeTopology::from_ascii(
            "\
#####
#A.A#
#####
",
            &topology_legend(),
            [MapNodeSpec::new("airlock", 'A', ())],
        )
        .expect_err("duplicate anchor should fail");

        assert_eq!(
            err,
            MapNodeTopologyError::DuplicateNodeAnchor {
                node_key: "airlock".to_string(),
                anchor: 'A',
                count: 2,
            }
        );
    }

    #[test]
    fn node_topology_rejects_exit_to_unknown_node() {
        let err = MapNodeTopology::from_ascii(
            "\
#####
#A.B#
#####
",
            &topology_legend(),
            [
                MapNodeSpec::new("airlock", 'A', ()).with_exits(["missing"]),
                MapNodeSpec::new("bridge", 'B', ()),
            ],
        )
        .expect_err("unknown exit target should fail");

        assert_eq!(
            err,
            MapNodeTopologyError::UnknownExit {
                node_key: "airlock".to_string(),
                target_key: "missing".to_string(),
            }
        );
    }
}
