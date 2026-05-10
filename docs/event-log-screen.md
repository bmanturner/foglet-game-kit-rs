# Event Log Screen

The Event Log screen is a reusable read-only view over v2
`world_events`.

Scope:

- `global` shows all events.
- `player(player_id)` shows global events plus that player's events in
  harness helpers, and the screen scope can filter to one player.
- `kind(string)` filters by event kind.
- custom scopes can be supplied through a predicate.

Formatting:

- The screen accepts a per-event formatter callback.
- The callback returns an `EventLogLine` with a primary line and an
  optional secondary line.
- The kit does not interpret event message meaning; games decide the
  rendered text.

Embedded mode:

- Top-level screen mode implements the normal `Screen` trait.
- Embedded mode renders into a caller-supplied `Rect` and does not draw
  outside that region.
- Load event rows before rendering. Do not query the world DB from a
  paint loop.

Examples:

- Dungeon death log: scope by `kind = "death"` and format each row as a
  terse epitaph with room name and cause.
- Mystery case ledger: player-scope the log to one detective and format
  rows as clue discoveries, interviews, and evidence updates.
