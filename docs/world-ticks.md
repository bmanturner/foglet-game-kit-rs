# World ticks: durable catch-up without daemons

This document explains the world-tick primitive in `foglet_game`.

## 1. Why this primitive exists

The world-tick primitive provides durable periodic work without forcing
one hosting pattern.

- A **space exploration** game can restock station depots and rotate
  docking boards.
- A **dungeon crawler** can reset trap states and patrol timers.
- A **town simulation** can regrow market produce and advance workshop
  queues.

The primitive is intentionally one-shot and catch-up oriented. It does
not create a background worker, long-lived daemon, or real-time event
loop.

## 2. Data model

`crates/foglet_game/src/world_ticks.rs` stores tick definitions in
`world_tick_tasks`:

- `key` (primary key)
- `last_run_at` (`NULL` before first successful run)
- `interval_seconds` (positive integer)
- `metadata_json` (opaque game-owned payload)

Use `WorldDb::register_tick(key, interval_seconds, callback)` to create
or re-register tasks at startup. Re-registering the same key is
idempotent:

- first registration inserts the row
- same key + same interval keeps one row
- same key + different interval updates the cadence in place

## 3. Running due work

Use `WorldDb::run_due_ticks(now, max_catchup_per_call)` for one bounded
catch-up pass.

A task is due when:

- `last_run_at IS NULL`, or
- `last_run_at + interval_seconds <= now`

The return value is how many tasks committed in that pass.

## 4. Execution patterns

Two explicit scheduling patterns are supported. Games can use either, or
both:

### 4.1 Login-time catch-up (in-process)

Enable this in `assets/game.toml`:

```toml
[world_ticks]
enabled = true
max_catchup_per_call = 100
run_due_ticks_on_login = true
```

This runs one bounded catch-up pass during runtime startup. Keep it
explicit and out of render loops.

### 4.2 Operator cron catch-up (`fgk tick`)

Run one-shot maintenance outside the TUI:

```bash
fgk tick --project /srv/foglet/doors/aurora-port
```

```bash
fgk tick --project /srv/foglet/doors/obsidian-crypt
```

The command opens the project world DB, runs one due-task pass, and
prints a one-line summary:

```text
Ran 3 task(s), skipped 2 task(s)
```

`skipped` means due work remains because the catch-up cap deferred it to
the next invocation.

## 5. Idempotency and safety guarantees

`run_due_ticks` is built for retries and concurrent callers:

- Callback and `last_run_at` update are in one transaction.
- If a callback returns `Err`, the transaction rolls back and
  `last_run_at` stays unchanged.
- Concurrent callers claim disjoint due tasks; one due row is not
  committed twice in the same timestamp window.
- `fgk tick` and runtime login hooks can safely coexist when callbacks
  are written idempotently.

Callback guidance:

- Use SQL that tolerates retries.
- Avoid assuming exclusive writer ownership.
- Keep side effects inside the transaction whenever possible.

## 6. `max_catchup_per_call` and backlog control

`max_catchup_per_call` is a hard upper bound on due tasks executed in a
single pass.

- Default: `100`
- Validation rule: must be greater than zero
- Behavior: if more tasks are due than the bound, only that many run and
  the rest remain due for the next call

This prevents long downtime from causing one unbounded catch-up burst.

## 7. Boundaries and non-goals

- No background daemon or watcher process.
- No real-time multiplayer scheduler.
- No auto invocation from paint/render loops.
- No genre-specific economy or combat policy.

World ticks are a structural primitive: durable due-time bookkeeping plus
transactional callback execution. Game projects choose the semantics.
