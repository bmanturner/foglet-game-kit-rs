# Installing a packaged game as a Foglet door

This is the operator-facing walkthrough. It picks up where the
[Quickstart](../README.md#quickstart--from-zero-to-a-working-foglet-door)
leaves off: you have a `dist/` bundle from `fgk package` and a manifest
from `fgk emit-manifest`, and you want Foglet to launch the game as a
real `:external_pty` door.

The contract this walkthrough satisfies lives in
[`SPEC.md`](../SPEC.md) — §2 (Foglet adapter grounding), §10 (CLI
contract), §12 (persistence), §13 (operational requirements), and §14
(QA standards). If anything below conflicts with the SPEC, the SPEC
wins.

## 1. What you are deploying

`fgk package --out dist/` produces this layout (SPEC §10.4):

```text
dist/
  <slug>          # release binary, executable
  run.sh          # boring auditable wrapper, executable
  manifest.json   # operator manifest (SPEC §10.3)
  assets/         # game.toml, maps, dialog, ...
```

`run.sh` is the single entry point Foglet calls. It `cd`s next to the
binary, picks the per-user save directory from `FOGLET_USER_ID` (with
an `FGK_SAVE_DIR` override for operators), and `exec`s the binary.
It never interpolates user input into a shell command beyond that
bounded env-var path selection (SPEC §10.4).

The matching `manifest.json` declares `runtime: "external_pty"`,
absolute `command` and `working_dir` paths, an explicit non-secret
`env` map, an `env_allowlist`, `pty: true`, and the timeouts /
visibility / auth scope from `assets/game.toml`. See the SPEC §10.3
example for the exact shape.

## 2. Choose an install directory

Pick the absolute path the door will live at on the Foglet host **before**
running `fgk emit-manifest`, because that path is baked into the
manifest's `command` and `working_dir` fields. The convention from
SPEC §12 is:

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
  requires it.

The kit refuses to read inherited host environment beyond the
documented `FOGLET_*` variables, and save files contain only
game-defined state — never the Foglet context (SPEC §5.6, §12).

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
