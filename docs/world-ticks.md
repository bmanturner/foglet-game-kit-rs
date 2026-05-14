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
- `schedule_kind` (`interval` or `daily_slots`)
- `daily_slots_json` (canonical `HH:MM` slots for fixed daily schedules)
- `last_completed_slot_at` (the last scheduled slot completed by a
  daily-slot task)

Use `WorldDb::register_tick(key, interval_seconds, callback)` to create
or re-register interval tasks at startup. Interval ticks run after
elapsed duration since their last success. Re-registering the same key is
idempotent:

- first registration inserts the row
- same key + same interval keeps one row
- same key + different interval updates the cadence in place

Use `WorldDb::register_daily_slot_tick(key, slots, callback)` for fixed
daily clock slots. Slot strings must be unique `HH:MM` values. This:

- rejects empty lists, duplicates, and malformed times;
- persists `schedule_kind = 'daily_slots'` and the sorted slot list;
- updates one durable row when the same key is re-registered with a new
  slot list.

Four fixed daily slots:

```rust
world.register_daily_slot_tick(
    "world_simulation",
    &["00:00", "06:00", "12:00", "18:00"],
    |tx, context| {
        tx.execute(
            "INSERT INTO tick_diagnostics (task_key, scheduled_at, actual_run_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![
                &context.task_key,
                &context.scheduled_at,
                &context.actual_run_at
            ],
        )?;
        Ok(())
    },
)?;
```

The runner treats `now` as an explicit local wall-clock datetime string
in `YYYY-MM-DD HH:MM:SS` form. Games should pass the same timezone
consistently for registration, login catch-up, and operator runs. Do not
fake a daily `12:00:00` timestamp for fixed-slot schedules; pass the
actual current datetime so the scheduler can discover which named slots
are due.

## 3. Running due work

Use `WorldDb::run_due_ticks(now, max_catchup_per_call)` for one bounded
catch-up pass.

An interval task is due when:

- `last_run_at IS NULL`, or
- `last_run_at + interval_seconds <= now`

A fixed daily slot task is due when at least one configured scheduled
slot is later than `last_completed_slot_at` and less than or equal to
`now`. Missed slots are applied in chronological order. If the `06:00`
slot runs late at `06:37`, the next due slot is still `12:00`, not
`12:37`.

The return value is how many scheduled callbacks committed in that pass.
`max_catchup_per_call` limits scheduled slots, not just task keys, so a
long absence is chunked across repeated invocations.

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

The command opens the project world DB, registers no-op callbacks for
both interval and fixed daily slot tasks, runs one due-schedule pass, and
prints a one-line summary:

```text
Ran 3 task(s), skipped 2 task(s)
```

`skipped` means due work remains because the catch-up cap deferred it to
the next invocation.

## 5. Idempotency and safety guarantees

`run_due_ticks` is built for retries and concurrent callers:

- Callback and `last_run_at` update are in one transaction.
- Daily slot callbacks receive `WorldTickContext`, including task key,
  scheduled slot datetime, actual runner datetime, schedule kind, and
  catch-up index.
- If a callback returns `Err`, the transaction rolls back and
  the interval `last_run_at` or daily `last_completed_slot_at` marker
  stays unchanged for retry.
- Concurrent callers claim disjoint due tasks or scheduled slots; one
  due schedule item is not committed twice in the same timestamp window.
- `fgk tick` and runtime login hooks can safely coexist when callbacks
  are written idempotently.

Callback guidance:

- Use SQL that tolerates retries.
- Avoid assuming exclusive writer ownership.
- Keep side effects inside the transaction whenever possible.

## 6. `max_catchup_per_call` and backlog control

`max_catchup_per_call` is a hard upper bound on due scheduled callbacks
executed in a single pass.

- Default: `100`
- Validation rule: must be greater than zero
- Behavior: if more tasks are due than the bound, only that many run and
  the rest remain due for the next call

For low-pop BBS games, a good pattern is a modest login cap plus a cron
or operator `fgk tick` pass. Interval ticks are best for "every N
seconds after the last success" work. Daily slot ticks are best for
named wall-clock beats such as morning/noon/evening simulation,
maintenance reports, or digest generation.

Daily Turns can stay date-based even when world ticks run multiple times
per day. Treat turns as the player action budget for a local date, and
world ticks as background simulation slots.

This prevents long downtime from causing one unbounded catch-up burst.

## 7. Boundaries and non-goals

- No background daemon or watcher process.
- No render-loop ticking.
- No real-time multiplayer scheduler.
- No auto invocation from paint/render loops.
- No genre-specific economy or combat policy.

World ticks are a structural primitive: durable due-time bookkeeping plus
transactional callback execution. Game projects choose the semantics.
