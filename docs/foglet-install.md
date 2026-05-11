# Installing a packaged game as a Foglet door

This is the operator-facing walkthrough. It picks up where the
[Quickstart](../README.md#quickstart--from-zero-to-a-working-foglet-door)
leaves off: you have a `dist/` bundle from `fgk package` and a manifest
from `fgk emit-manifest`, and you want Foglet to launch the game as a
real `:external_pty` door.

## 1. What you are deploying

`fgk package --out dist/` produces this layout:

```text
dist/
  <slug>          # release binary, executable
  run.sh          # boring auditable wrapper, executable
  manifest.json   # operator manifest
  assets/         # game.toml, maps, dialog, ...
```

`run.sh` is the single entry point Foglet calls. It `cd`s next to the
binary, picks the per-user save directory from `FOGLET_USER_ID` (with
an `FGK_SAVE_DIR` override for operators), and `exec`s the binary.
It never interpolates user input into a shell command beyond that
bounded env-var path selection.

The matching `manifest.json` declares `runtime: "external_pty"`,
absolute `command` and `working_dir` paths, an explicit non-secret
`env` map, an `env_allowlist`, `pty: true`, and the timeouts /
visibility / auth scope from `assets/game.toml`.

## 2. Choose an install directory

Pick the absolute path the door will live at on the Foglet host **before**
running `fgk emit-manifest`, because that path is baked into the
manifest's `command` and `working_dir` fields. The convention is:

```text
/srv/foglet/doors/<slug>/
```

The `<slug>` MUST match `[game].slug` in `assets/game.toml` and the
binary name inside `dist/`. Use the same value you passed to
`--install-dir`.

Re-emit the manifest if you change your mind about the path:

```bash
fgk emit-manifest \
  --install-dir /srv/foglet/doors/<slug> \
  > /tmp/<slug>.manifest.json
```

`fgk emit-manifest` rejects relative paths — the manifest must be
deployable as-is.

## 3. Place the bundle on the Foglet host

Copy the `dist/` tree to the install directory you committed to in
step 2. Preserve the executable bits on `<slug>` and `run.sh`.

```bash
sudo install -d -m 0755 /srv/foglet/doors/<slug>
sudo rsync -a dist/ /srv/foglet/doors/<slug>/
sudo chmod 0755 /srv/foglet/doors/<slug>/run.sh
sudo chmod 0755 /srv/foglet/doors/<slug>/<slug>
```

The save directory is created lazily by the game on first launch under
`/srv/foglet/doors/<slug>/saves/<FOGLET_USER_ID>/save.json`. It needs
to be writable by the user Foglet runs the door under — typically the
same UID Foglet itself runs as, or whatever the sandbox identity in
your manifest resolves to. Pre-creating the parent directory avoids
ENOENT on the first launch:

```bash
sudo install -d -m 0755 -o foglet -g foglet \
  /srv/foglet/doors/<slug>/saves
```

Substitute the actual user/group Foglet runs under on your host. If
you use `FGK_SAVE_DIR` to override the save root, point it at a
directory with the same ownership.

### 3.1 Shared-world directory (v2 world-enabled games)

If `[world].enabled = true` in `assets/game.toml`, the package ships a
`world/` directory at the install root (SPEC_v2 §6.1). The runtime
opens (and on first launch creates) the SQLite file at
`<install-dir>/world/world.sqlite` — by default `world/world.sqlite`
relative to the binary, or whatever absolute/relative path
`[world].path` points at. The directory MUST be writable by the same
identity that owns `saves/`, because SQLite needs to write the DB file
itself plus `-wal` and `-shm` siblings when WAL journaling is enabled
(SPEC_v2 §6, default `journal_mode = "wal"`).

Pre-create the directory with the same ownership as `saves/`:

```bash
sudo install -d -m 0755 -o foglet -g foglet \
  /srv/foglet/doors/<slug>/world
```

`run.sh` never deletes or recreates `world/world.sqlite` (SPEC_v2
§6.1), and `fgk package` does not pre-populate it — the database is
authored by the game on first launch, then evolves through the
migrations declared in `assets/world/migrations/`. Treat the file as
durable game state, not a build artifact.

### 3.2 Scheduling `fgk tick` from cron (v4 world ticks)

If your game enables `[world_ticks].enabled = true`, run `fgk tick`
outside the active TUI on a schedule (for example, once per minute).
`fgk tick` is one-shot: each invocation opens the world DB, runs due
tasks once, prints a summary, and exits.

For a door installed at `/srv/foglet/doors/<slug>`:

```bash
* * * * * /usr/local/bin/fgk tick --project /srv/foglet/doors/<slug> >> /var/log/foglet/<slug>-tick.log 2>&1
```

Notes:

- Run the cron entry as the same user/group that owns the door's
  `world/` directory.
- Keep the cadence conservative (for example, 1-5 minutes) unless your
  tick callbacks are known to be quick and idempotent.
- Verify with a manual run first:

```bash
fgk tick --project /srv/foglet/doors/<slug>
```

If your platform prefers systemd timers over cron, use the same
command in `ExecStart=` and keep it as a short-lived oneshot unit.

## 4. Install the manifest

Foglet reads operator manifests from its configured manifest directory.
For a default install that's `/etc/foglet/manifests/`; check
`config/runtime.exs` (or your equivalent) for the path Foglet actually
loads. The filename should match the slug:

```bash
sudo install -m 0644 /tmp/<slug>.manifest.json \
  /etc/foglet/manifests/<slug>.json
```

Manifests are read on Foglet startup. Reload Foglet (`systemctl
restart foglet`, your `mix` release equivalent, or the documented
hot-reload path if your deployment supports it) and confirm the door
appears in the BBS Door Games list for a member. If it does not, check
Foglet's logs — manifest validation errors are printed at boot.

## 5. Permission and isolation expectations

The kit does not provide sandboxing. Process isolation is controlled
entirely by the Foglet manifest and host deployment (SPEC §13.3):

- The `env` and `env_allowlist` fields in the generated manifest
  define the only environment the door sees. The kit emits
  `TERM=xterm-256color` and `LANG=C.UTF-8` by default; add anything
  else explicitly. **No app secrets, DB URLs, or API tokens** belong
  here — the game is not allowed to depend on them (SPEC §2.2).
- Foglet's PTY adapter is responsible for process-group cleanup,
  timeouts, idle timeouts, resize forwarding, and disconnect handling
  (SPEC §2.1). The game cooperates by restoring the terminal on every
  exit path; see [`terminal-safety.md`](terminal-safety.md).
- If your Foglet deployment uses the helper-backed PTY with sandbox
  identity, ensure that identity has read access to the install
  directory and write access to the saves root. Sandboxed doors fail
  closed if the helper or sandbox identity is unavailable.
- File permissions: `0755` on the binary and `run.sh`, `0644` on the
  manifest, `0755` on `assets/` and its contents. Save files are
  created `0644` by default; tighten with umask if your deployment
  requires it. For world-enabled games, `world/` is `0755` and
  `world/world.sqlite` (plus `-wal` / `-shm` when WAL is on) is `0644`
  — both owned by the door runtime user. Sandboxed deployments must
  ensure the sandbox identity retains write access to `world/`, or the
  shared world will fail to open and the door will exit with a
  controlled error after terminal restoration (SPEC_v2 §8).

The kit refuses to read inherited host environment beyond the
documented `FOGLET_*` variables, and save files contain only
game-defined state — never the Foglet context (SPEC §5.6, §12).

### 5.1 Backing up and maintaining the shared world

The shared-world SQLite file is the *only* place durable world state
lives. That includes the player registry, turn ledger, event log,
leaderboards, spatial state (`places`, `routes`, `presence`,
`place_recall`, `inventory_slots`, `world_tick_tasks`), and contracts
(accepted work, objectives, rewards, deadlines, lifecycle timestamps).
None of that is reconstructible from per-player saves. Back it up on a
schedule that matches your tolerance for losing in-game state.

Two safe backup strategies:

1. **Stop-the-door copy.** Stop Foglet (or at least make the door
   un-launchable so no new processes open the DB), then copy the
   files:

   ```bash
   sudo systemctl stop foglet
   sudo cp -a /srv/foglet/doors/<slug>/world \
       /var/backups/foglet/<slug>/world-$(date +%Y%m%dT%H%M%S)
   sudo systemctl start foglet
   ```

   Copy the whole `world/` directory, not just `world.sqlite` —
   under WAL journaling the `-wal` and `-shm` files are part of the
   committed state until the next checkpoint.

2. **Online SQLite backup API.** Use `sqlite3 .backup` while the
   door is live; it cooperates with WAL and produces a consistent
   snapshot:

   ```bash
   sudo -u foglet sqlite3 \
     /srv/foglet/doors/<slug>/world/world.sqlite \
     ".backup '/var/backups/foglet/<slug>/world-$(date +%Y%m%dT%H%M%S).sqlite'"
   ```

   The output is a single file you can restore by stopping the door
   and copying it back into place as `world/world.sqlite` (deleting
   any stale `-wal`/`-shm` siblings first).

**Do not** `cp world.sqlite` while a door process is live — under WAL
that produces a torn snapshot. **Do not** rely on filesystem
snapshots alone unless your snapshot tool is consistent across the
DB file and its `-wal`/`-shm` siblings at the same instant.

The kit does not migrate world data for you. If a migration in
`assets/world/migrations/` is destructive, take a backup first; world
migrations run idempotently on launch (SPEC_v2 §4) but the schema
they leave behind is binding.

For routine maintenance, run occasional `PRAGMA wal_checkpoint(TRUNCATE);`
during low traffic and `VACUUM` during scheduled downtime if the world
file grows from churn in v4-heavy tables like `place_recall` and
`inventory_slots`, or v5-heavy tables like `contracts` when games post
and expire large numbers of jobs. Always take a backup first.

## 6. Foglet QA standards

SPEC §14 requires the following before signing off on an install:

- **80x24 baseline.** The game must launch and play through one full
  loop at the standard BBS size.
- **At least one cramped size.** Test at 64x22. The game must either
  refuse cleanly (no broken raw-mode/alternate-screen state) or
  degrade gracefully — never corrupt the terminal.
- **SSH/TUI harness evidence.** Real integration claims require
  launching through Foglet over SSH/TUI, not just local terminal
  execution. Static-only signoff (manifest validates, binary exits 0)
  is **insufficient** for TUI/PTY behavior.
- **Full lifecycle smoke.** Verify input, resize, normal quit,
  relaunch, timeout, idle timeout, disconnect cleanup, and that the
  caller's terminal returns to a usable state in every case.

The recommended smoke flow once the door is live:

1. SSH into Foglet as a member account.
2. Open the Door Games list and confirm the slug appears.
3. Launch the door; play through title → menu → map → dialog →
   inventory → save → quit.
4. Relaunch and verify the save survived (per-user persistence,
   SPEC §12).
5. Resize the terminal mid-game; confirm no corruption.
6. Force a disconnect (close the SSH session) and reconnect; confirm
   Foglet has cleaned up the PTY and the door is launchable again.
7. Trigger the idle timeout from the manifest and confirm clean exit.

## 7. Updating an installed door

Re-run `fgk package` and re-deploy `dist/` over the install directory.
The manifest only needs re-emitting if `[game]` or `[manifest]` fields
in `assets/game.toml` changed. Foglet picks up the new binary on the
next door launch — there is no in-process hot reload.

Save files are owned by the game and survive package updates as long
as the slug, save schema, and install directory are stable. Breaking
the save schema is a game-level decision; the kit does not migrate
saves for you.

## 8. Troubleshooting

- **Door missing from the BBS list.** Check Foglet logs at startup
  for manifest validation errors. The most common causes are relative
  paths (the kit prevents this on emit, but hand-edited manifests
  often regress), a slug mismatch between the manifest and the binary,
  or the manifest landing in a directory Foglet doesn't load.
- **Door launches but exits immediately.** Run the binary directly on
  the host with the documented `FOGLET_*` env vars set; the game's
  controlled-error path prints a short message after terminal
  restoration (SPEC §7.3). Check that `assets/` shipped alongside the
  binary — the runtime resolves asset paths from `--assets`, which
  `run.sh` points at the install directory's `assets/`.
- **Shared world fails to open / SQLite `database is locked`.**
  Confirm `/srv/foglet/doors/<slug>/world/` is writable by the door
  runtime user, including write access for the `-wal` and `-shm`
  siblings SQLite creates next to `world.sqlite`. Stale lock files
  from a crashed process clear themselves on the next clean open;
  if they persist, stop the door, confirm no `<slug>` processes are
  alive, and remove only the `-wal` / `-shm` files (never
  `world.sqlite` itself). The runtime opens the DB during startup
  and surfaces failures as a controlled error after terminal
  restoration (SPEC_v2 §8).
- **Saves not persisting across launches.** Confirm
  `/srv/foglet/doors/<slug>/saves/` is writable by the user Foglet
  runs the door under. Atomic writes go through a temp file in the
  same directory and rename into place; missing write permission
  surfaces as a controlled error after terminal restoration.
- **Terminal corruption on disconnect or crash.** The kit installs a
  panic hook that runs the restoration sequence before the panic
  message prints (SPEC §13.1). If you see corruption, capture the
  reproduction recipe and check
  [`terminal-safety.md`](terminal-safety.md) — the contract there is
  the source of truth for what should and should not happen on every
  exit path.
