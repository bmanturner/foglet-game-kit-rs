# Shared-world SQLite contract

This is the operator- and game-author-facing reference for the v2
shared-world layer: where the SQLite file lives, how migrations are
applied, how to back the database up, and how to recover from a lock
that won't clear.

The contract this document satisfies lives in
[`SPEC_v2.md`](../SPEC_v2.md) — §3 (system overview), §4 (domain
model), §5 (configuration), §6 (packaging), and §8 (operational
requirements). If anything below conflicts with the SPEC, the SPEC
wins.

For the install-side mechanics (creating `world/`, ownership, and the
in-context backup commands), see
[`foglet-install.md`](foglet-install.md) §3.1 and §5.1; this doc
focuses on the *why* and on author-facing concerns.

## 1. Why a per-game SQLite file at all

v1 saves are per-user JSON blobs under `saves/<FOGLET_USER_ID>/`. They
are perfect for "what does *this* caller see" but cannot represent
"what did the *previous* caller change." Many BBS door games leaned on
exactly that asynchronous shared state — a town economy, a rumor
board, a leaderboard — so v2 adds a per-game shared-world database
without attempting real-time multiplayer (SPEC_v2 §1, §2.2).

The kit deliberately picks SQLite over a network service:

- It is a single file the operator can copy, archive, and inspect with
  `sqlite3` on the host. No extra service to deploy or monitor.
- It is owned by the game package, not by Foglet's app database
  (SPEC_v2 §8). The kit never reads or writes Foglet's Postgres.
- It is testable end-to-end without a live terminal — every primitive
  in `crates/foglet_game/src/world_db.rs` is exercised under `cargo
  test` against a `tempfile`-backed DB.
- It is fast enough for door-game write rates (turn spend, event
  append, leaderboard increment) that the runtime can keep all
  shared-world writes inside a synchronous request handler without a
  background worker.

What v2 explicitly does *not* do:

- No real-time presence, broadcasts, or push between live sessions.
- No cross-door shared state. Each game's world DB is its own file.
- No automatic schema generation or ORM. Game authors write the SQL
  for their tables; the kit only owns the migration framing,
  bootstrap, transaction wrapper, and a handful of named primitives
  (player registry, turn ledger, event log, leaderboards).

## 2. File locations

A world-enabled package lays out as (SPEC_v2 §6.1):

```text
/srv/foglet/doors/<slug>/
  <slug>            # release binary
  run.sh            # boring wrapper
  manifest.json
  assets/
    world/migrations/  # author-written SQL, optional in v2
  world/            # writable runtime data
    .keep
    world.sqlite    # created on first launch
    world.sqlite-wal  # only when journal_mode = "wal" and a writer is active
    world.sqlite-shm  # ditto
  saves/<FOGLET_USER_ID>/save.json
```

`[world].path` in `assets/game.toml` selects the file. Resolution
rules (SPEC_v2 §4.1):

- Default is `world/world.sqlite`, relative to the package install
  directory — i.e. the directory the binary lives in. This is what
  `fgk new` and `fgk package` produce, and what
  [`foglet-install.md`](foglet-install.md) assumes.
- A relative path resolves under the same install directory, *not* the
  shell's current working directory. Game code never opens a path
  derived from inherited environment beyond the documented `FOGLET_*`
  fallbacks (SPEC §5.6).
- An absolute path is allowed only when the operator supplies it
  explicitly via CLI or sysop config. The default-on packaging path
  refuses to bake an absolute path into a manifest because it makes
  the bundle non-portable.

For local development, `fgk run` MAY redirect the world DB to
`.fgk/world/world.sqlite` so dev iteration does not dirty the assets
directory (SPEC_v2 §6.2). Treat `.fgk/world/` as throwaway state — the
file there is not authoritative.

The `world/` directory MUST be writable by the door runtime user
(SPEC_v2 §6.1). SQLite creates `-wal` and `-shm` siblings next to
`world.sqlite` while a writer holds the file, so write permission is
required on the *directory*, not just the file.

## 3. Configuring the world

`assets/game.toml` opts into the world layer:

```toml
[world]
enabled = true
path = "world/world.sqlite"
busy_timeout_ms = 5000
journal_mode = "wal"
```

Field reference (SPEC_v2 §4.1):

| Field | Default | Notes |
| --- | --- | --- |
| `enabled` | `false` | v1 projects without `[world]` keep working. |
| `path` | `world/world.sqlite` | Relative to the install dir. |
| `busy_timeout_ms` | `5000` | Applied via `PRAGMA busy_timeout`. |
| `journal_mode` | `wal` | Or `delete` for environments that forbid WAL siblings. |

Pair `[world]` with `[turns]` and `[[leaderboards]]` if the game uses
those primitives — see [`SPEC_v2.md`](../SPEC_v2.md) §5 for the full
schema.

If `enabled = false` (or `[world]` is missing), the runtime never opens
a SQLite handle and `GameContext::world_db()` returns `None`. World
primitives that require a DB return a clear error rather than panicking,
so a single source tree can ship both v1- and v2-shape projects.

## 4. Migration policy

Migrations are game-owned schema steps. The kit owns the framing and
the recording table; the game owns the SQL.

What the kit guarantees (SPEC_v2 §4.3):

- A `world_migrations` table is created on first open and stores
  `(version, name, applied_at)` for every successfully applied
  migration.
- Migrations are applied in ascending `version` order, exactly once
  per database. A migration whose row already exists is skipped.
- A migration that fails to apply leaves no row in
  `world_migrations` and surfaces the SQL error to the runtime, which
  treats it as a controlled startup failure (terminal restored, error
  printed to the operator's real shell, non-zero exit).
- Migration application is idempotent across launches: re-running a
  migration whose row exists is a no-op, so a clean restart of the
  door does not double-apply schema changes.

What the game author owns:

- Picking version numbers. They MUST be monotonically increasing. The
  conventional pattern is to keep them dense (`1, 2, 3, ...`) and use
  the `name` field for human context.
- Writing forward-only SQL. v2 does not implement automated rollback —
  if a migration is destructive and you need to undo it, restore from
  backup (§5).
- Keeping migrations small and self-contained. A migration that
  depends on application code paths (e.g. a Rust callback that calls
  back into game logic) is allowed but discouraged; future-you reading
  a migration five releases later will thank you for keeping it
  declarative SQL.

`murder_motel` ships its migrations as embedded `WorldMigration`
values in Rust source — see `examples/murder_motel/src/world.rs` for
the canonical pattern. v2 keeps the public `WorldMigration` API
deliberately narrow so a future release can add file-backed
`assets/world/migrations/*.sql` without breaking authors who started
with embedded SQL.

### 4.1 What not to do in a migration

- **Don't store secrets.** The world DB is a backup target and an
  inspection target. Foglet context, API tokens, and DB URLs do not
  belong in any column (SPEC_v2 §4.7, SPEC §5.6).
- **Don't drop tables that hold game-defined player progress.** The
  kit will not stop you, but the data is the only authoritative copy.
  If you must drop, take a backup first (§5).
- **Don't write `PRAGMA foreign_keys = OFF` in a shipped migration.**
  Enforce constraints at write time; relax them only inside a one-off
  data-fix migration that turns them back on at the end.
- **Don't SELECT user input back into a log line.** The kit's
  `tracing` policy never logs SQL bind values that may contain player
  text (SPEC_v2 §4.2). Author-written SQL should follow the same
  rule — bound parameters only, no string interpolation of player
  handles into SQL.

## 5. Backups

The shared-world SQLite file is the *only* place v2 game state like
the player registry, turn ledger, event log, and leaderboards lives.
Saves don't reconstruct it. Back it up.

Two safe strategies (SPEC_v2 §8):

1. **Stop-the-door copy.** Stop Foglet (or otherwise prevent new door
   launches), copy the whole `world/` directory — including any
   `-wal` and `-shm` siblings — restart Foglet.
2. **Online SQLite backup API.** While the door is live, use
   `sqlite3 .backup` to produce a single consistent file. It
   cooperates with WAL.

The exact commands live in [`foglet-install.md`](foglet-install.md)
§5.1. Two rules worth repeating because they are the load-bearing
ones:

- **Never `cp world.sqlite` while a door process is live under WAL.**
  You will get a torn snapshot. Stop the door or use the backup API.
- **Always copy the whole `world/` directory, not just the `.sqlite`
  file.** Until the next checkpoint, the `-wal` file holds committed
  data the main file does not.

Restore is the inverse: stop the door, replace `world/world.sqlite`,
delete any stale `-wal` / `-shm` siblings, then start the door. The
runtime applies any migrations the restored file is missing on the
next open.

Migrations are forward-only (§4). If a future migration is destructive,
your backup is the rollback path.

## 6. Lock recovery

SQLite reports `database is locked` (or `SQLITE_BUSY` returns) when a
writer can't acquire its lock within the configured `busy_timeout_ms`.
In a single-process door runtime this almost always means one of:

- **A previous door process crashed and left a stale `-wal` / `-shm`
  pair.** SQLite normally clears these on the next clean open. If they
  persist, stop the door, confirm no `<slug>` processes are alive
  (`pgrep <slug>`), and remove only the `-wal` and `-shm` files.
  **Never delete `world.sqlite` itself** — that is your live data.
- **A long-running backup or external tool holds a lock.** `sqlite3
  .backup` should be brief; ad-hoc `sqlite3` shells left open in tmux
  panes are the usual culprit. Close them.
- **The door runtime user lost write permission to `world/`.** The
  open succeeds (read), the first write fails. Re-check ownership;
  see [`foglet-install.md`](foglet-install.md) §3.1.
- **The filesystem is full or read-only.** SQLite reports a lock or
  IO error indistinguishably in some cases. Check `df` and the
  syslog.

The runtime surfaces every lock/open failure as a controlled error
*after* terminal restoration (SPEC_v2 §4.1, §8). The operator sees a
short message on their real shell, the door exits non-zero, and Foglet
can relaunch it cleanly. The kit does not retry indefinitely — the
busy timeout is the only retry budget, and beyond it the failure is
the operator's to triage.

## 7. Why no real-time multiplayer

v2 is intentionally an *asynchronous* shared-world layer. SPEC_v2 §2.2
explicitly forbids real-time multiplayer, networked game servers, and
cross-door shared state APIs in this slice (see also SPEC_v2 §17, which
lists "async shared worlds over real-time multiplayer" as a guiding
principle). This section is the long-form answer to "why not?" so that
future contributors don't re-litigate the decision in PR review.

### 7.1 What "asynchronous shared world" actually means

The v2 contract is a single SQLite file per game, mutated by whichever
door process happens to be running, observed by the next door process
that opens it. Concretely:

- Two callers in two PTYs at the same wall-clock minute do **not** see
  each other's cursors, chat, or moves as they happen. The runtime
  never broadcasts anything between live sessions.
- The first caller's commit is durable before the second caller's
  process starts reading; SQLite plus the kit's transaction wrapper
  (SPEC_v2 §4.6) is the entire concurrency story.
- Latency between "Alice did X" and "Bob sees X" is bounded by how
  long Bob takes to launch the door and reach the screen that reads
  the relevant table. In Murder Motel that is seconds-to-minutes —
  enough for a BBS-style "someone was here before you" feel, not
  enough for cooperative or adversarial real-time play.

If your game's correctness depends on Bob seeing Alice's input *while
both are connected*, the v2 kit cannot deliver that, and trying to
bolt it on is out of scope.

### 7.2 Why we picked async

Four load-bearing reasons, in roughly the order they would bite a
real-time alternative:

- **Operational simplicity.** A SQLite file the operator can copy and
  inspect is a much smaller commitment than a service to run, monitor,
  upgrade, secure, and triage at 3am. The whole `world/` directory
  fits in a `tar` (§5). A real-time server would add a process to
  supervise, a port to firewall, a deploy story to maintain, and a new
  failure mode to page on.
- **Foglet's process model.** Doors are `:external_pty` children;
  the adapter owns lifecycle, timeouts, and disconnect handling
  (SPEC §2, SPEC_v2 §3). A real-time game server would have to
  coordinate with that — graceful disconnects, zombie-session cleanup,
  timeouts that don't cross-contaminate Foglet's own — and the kit
  would have to ship its own networking + auth story. Both are
  out of scope for v2; both are big enough to be their own product.
- **Testability.** Every shared-world primitive is exercised under
  `cargo test` against a `tempfile`-backed DB with an injected clock
  and no live terminal (SPEC_v2 §13). Adding a network would push the
  test surface toward integration harnesses that are slower and
  flakier — the exact regression the kit's "no live terminal, no
  live network" default is designed to prevent.
- **Author ergonomics.** Asynchronous shared state is the BBS aesthetic
  the v2 design targets — one caller changes the town, another caller
  sees the consequences. That model covers the Murder Motel acceptance
  fixture (Room 7 first-opener, shared clue ledger, leaderboard) end
  to end without anyone reasoning about race windows, dropped frames,
  or partition recovery. Authors get to write game logic, not network
  logic.

### 7.3 What to do if you think you need real-time

Most "I need real-time" requests for a BBS-style door collapse into
one of these async-shaped patterns. Reach for them before reaching
past the v2 contract:

- **"Players need to see each other's actions."** Append a
  `world_events` row on the action and have the other player's screen
  poll `recent_events` on a turn boundary. Murder Motel's lobby
  bulletin (Task 13d) does exactly this.
- **"Players need to react to each other within a session."** Don't.
  Two callers on the same door at the same instant is rare on a
  classic BBS; designing for it is usually a sign the game wants to
  be a *web* game, not a door.
- **"I want a shared chat."** That belongs in Foglet itself, not in a
  door. The kit doesn't try to compete with the host's communication
  primitives.
- **"I want a leaderboard that updates while the player watches."**
  Re-read the leaderboard from SQLite on screen entry and on a turn
  spend. The freshness is bounded by the player's own input cadence,
  which is plenty.

If after all that you genuinely need synchronous cross-session
coordination — say, a real-time card game — the v2 kit is the wrong
foundation, and you should run that game outside the door system or
wait for a future major version.

### 7.4 Future direction

A future major version MAY revisit real-time semantics if a concrete
game motivates it and Foglet itself grows the supporting primitives
(presence channel, push API, broker). v2 explicitly does not paint
itself into a corner: nothing in the SQLite contract precludes a
later release adding a separate sidecar service for live coordination
while keeping the durable state in the same file.

For now, the v2 contract is, and remains: **shared state, async
semantics, single SQLite file, no network.**
