//! Save-path resolution for Foglet door games (SPEC §12).
//!
//! Task 8 splits into two halves: this module is **path resolution
//! only** (Task 8a). Atomic write/read lands in Task 8b.
//!
//! # Why a dedicated module
//!
//! SPEC §7.1 step 5 names "Resolve per-user save path" as part of the
//! startup ordering — it happens *before* terminal raw mode is entered,
//! so the runtime can surface a clear error (e.g. malformed
//! `FGK_SAVE_DIR`) without first wrecking the terminal. Keeping the
//! resolution pure and side-effect-free (no directory creation, no
//! file I/O) means the runtime can call it from the unsafe pre-raw-
//! mode startup band without worrying about partial state.
//!
//! # Precedence
//!
//! Resolution honours four sources in order, matching the wrapper
//! script in SPEC §10.4 and the deployment paths in SPEC §12:
//!
//! 1. **`SaveStrategy::None`** — game opted out of persistence
//!    (`assets/game.toml` says so). Returns `Ok(None)`.
//! 2. **`--save-dir <dir>`** CLI override. The wrapper passes this on
//!    every production launch (`--save-dir "$DIR/saves/$USER_ID"`).
//!    Treated as the directory that already contains (or will contain)
//!    `save.json` — no further `<user_id>` interpolation happens here.
//! 3. **`FGK_SAVE_DIR`** environment variable. Same shape as the CLI
//!    override; lets operators redirect saves without rebuilding the
//!    wrapper. Per SPEC §10.4 the wrapper itself reads this var, but
//!    that wrapper isn't used in `cargo run` smoke tests, so the
//!    runtime honours the env directly as a redundant safety net.
//! 4. **Fallback by [`ContextSource`]**:
//!    - `ContextFile` / `Env` (running under Foglet) →
//!      `/srv/foglet/doors/<slug>/saves/<user_id>/save.json` per
//!      SPEC §12. Missing `user_id` falls back to a literal
//!      `anonymous` segment so saves still go *somewhere* per-door —
//!      consistent with Foglet supporting anonymous-access doors
//!      (SPEC §5.1).
//!    - `LocalDev` → project-local `.fgk/saves/local-dev/save.json`,
//!      relative to the current working directory.
//!
//! # Side effects
//!
//! None at this layer. The returned [`PathBuf`] always ends in
//! `save.json` and may point at a directory that does not yet exist;
//! Task 8b's writer is responsible for `mkdir -p` and atomic write.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

use crate::config::SaveStrategy;
use crate::foglet::{ContextSource, FogletContext};

/// Filename stored at the resolved save directory.
///
/// Lifted to a constant so Task 8b's writer and any future tooling
/// (e.g. `fgk` debug commands that locate a user's save) agree on one
/// spelling. SPEC §12 documents `save.json` explicitly.
pub const SAVE_FILENAME: &str = "save.json";

/// Environment variable that overrides the resolved save directory.
///
/// Matches the variable the `run.sh` wrapper reads in SPEC §10.4. The
/// runtime honours it directly in addition to the wrapper's expansion
/// so `cargo run` smoke tests of the binary still respect operator
/// overrides without going through `run.sh`.
pub const SAVE_DIR_ENV: &str = "FGK_SAVE_DIR";

/// Production root for per-user saves. SPEC §12.
const PROD_SAVE_ROOT: &str = "/srv/foglet/doors";

/// Local-dev save directory, relative to the current working dir.
const LOCAL_DEV_SAVE_DIR: &str = ".fgk/saves/local-dev";

/// Filesystem segment used when running under Foglet but `user_id` is
/// absent. Foglet supports anonymous doors (SPEC §5.1), so the loader
/// surfaces `user_id = None`; rather than failing here we direct those
/// saves to a stable per-door `anonymous` bucket. Documented for the
/// operator reading the failure mode in `docs/foglet-install.md`
/// (Task 14b).
const ANONYMOUS_USER_SEGMENT: &str = "anonymous";

/// Errors produced while resolving a save path.
///
/// Library-internal (`thiserror`); the CLI converts at the boundary
/// per the PROMPT crate budget.
#[derive(Debug, Error)]
pub enum SavePathError {
    /// `slug` was empty. The runtime treats this as a programmer
    /// error — `GameConfig` parsing already rejects empty slugs
    /// (SPEC §5.2 / `config::GameConfig` validation) — but this layer
    /// double-checks so a hand-built [`SavePathInputs`] (e.g. in
    /// tests) can't sneak past and produce a path like
    /// `/srv/foglet/doors//saves/<user>/save.json`.
    #[error("save path resolution requires a non-empty slug")]
    EmptySlug,
}

/// Inputs required to resolve a save path.
///
/// Bundled into a struct (rather than a long argument list) so the
/// runtime can construct it once at startup and pass it through to
/// Task 8b's writer alongside the resolved path. New knobs (e.g. a
/// future `force_per_machine` mode) land additively here without
/// rippling through call sites.
#[derive(Debug, Clone, Copy)]
pub struct SavePathInputs<'a> {
    /// Slug from `[game].slug` — the directory name under
    /// `/srv/foglet/doors/`.
    pub slug: &'a str,
    /// Save strategy from `[save].strategy`. Drives the
    /// "no persistence" short-circuit.
    pub strategy: SaveStrategy,
    /// Loaded Foglet context. Provides `user_id` and `source`, which
    /// together pick between the production and local-dev paths.
    pub context: &'a FogletContext,
    /// `--save-dir` from the CLI, if the operator passed one. Wins
    /// over `FGK_SAVE_DIR` and over the SPEC §12 default roots.
    pub cli_override: Option<&'a Path>,
}

/// Resolve the save file path for the current launch.
///
/// `getenv` is injected so tests don't have to mutate process env.
/// Production callers pass [`crate::foglet::process_env`].
///
/// Returns `Ok(None)` exactly when the game opts out of saves
/// ([`SaveStrategy::None`]). All other branches return
/// `Ok(Some(<path>/save.json))`.
pub fn resolve_save_path<F>(
    inputs: &SavePathInputs<'_>,
    getenv: F,
) -> Result<Option<PathBuf>, SavePathError>
where
    F: Fn(&str) -> Option<String>,
{
    if inputs.slug.is_empty() {
        return Err(SavePathError::EmptySlug);
    }

    if matches!(inputs.strategy, SaveStrategy::None) {
        return Ok(None);
    }

    // 1. CLI override wins outright. The wrapper script in SPEC §10.4
    //    builds this path from `FOGLET_USER_ID`, so by the time we see
    //    it the per-user component is already encoded — we just append
    //    the filename.
    if let Some(dir) = inputs.cli_override {
        return Ok(Some(dir.join(SAVE_FILENAME)));
    }

    // 2. Env override behaves identically to the CLI flag. Empty values
    //    are treated as "unset" so a stray `FGK_SAVE_DIR=` in a shell
    //    profile doesn't collapse the save into the cwd.
    if let Some(dir) = getenv(SAVE_DIR_ENV).filter(|s| !s.is_empty()) {
        return Ok(Some(PathBuf::from(dir).join(SAVE_FILENAME)));
    }

    // 3. Default roots, picked by where the Foglet context came from.
    //    `ContextFile`/`Env` mean we believe Foglet (or a Foglet-like
    //    shell) is in the loop, so the SPEC §12 production root
    //    applies. `LocalDev` means we synthesised the context and
    //    must not write to `/srv/foglet/...`.
    let path = match inputs.context.source {
        ContextSource::ContextFile | ContextSource::Env => {
            let user = inputs
                .context
                .user_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(ANONYMOUS_USER_SEGMENT);
            PathBuf::from(PROD_SAVE_ROOT)
                .join(inputs.slug)
                .join("saves")
                .join(user)
                .join(SAVE_FILENAME)
        }
        ContextSource::LocalDev => PathBuf::from(LOCAL_DEV_SAVE_DIR).join(SAVE_FILENAME),
    };

    Ok(Some(path))
}

/// Errors produced while reading or writing a save file.
///
/// Library-internal (`thiserror`) so the runtime can match on a stable
/// closed set; `anyhow` translation happens at the binary boundary
/// per PROMPT.md's "errors at boundaries" rule.
///
/// Variants are deliberately granular: when a save fails the operator
/// reading the door log (SPEC §11) wants to know whether the parent
/// directory was unwritable, the JSON was corrupt, or the rename
/// itself blew up — each suggests a different remediation.
#[derive(Debug, Error)]
pub enum SaveIoError {
    /// The save path had no parent component, e.g. a bare relative
    /// `save.json` with no directory. Canonical save paths always
    /// include at least one directory segment (`/srv/.../save.json` or
    /// `.fgk/saves/local-dev/save.json`), so this only fires when a
    /// caller hand-constructs a degenerate path.
    #[error("save path has no parent directory: {path}")]
    NoParent {
        /// The offending path the caller passed in.
        path: String,
    },

    /// `mkdir -p` of the parent directory failed. Most common causes:
    /// the production `/srv/foglet/...` tree is missing or the operator
    /// lacks write permission. Surfaced verbatim so the door log makes
    /// the underlying `errno` visible.
    #[error("creating save parent directory failed: {0}")]
    Mkdir(#[source] io::Error),

    /// Generic I/O failure during the write half of the cycle: opening
    /// the temp file, writing JSON bytes, flushing, or fsync. The
    /// inner [`io::Error`] preserves the OS-level cause.
    #[error("writing save file failed: {0}")]
    Write(#[source] io::Error),

    /// `rename(2)` from the temp file to the final path failed. Treated
    /// distinctly from generic write errors so the operator can tell
    /// "the data never made it to disk" from "the swap-in failed and
    /// the previous version is still authoritative".
    #[error("renaming temp save into place failed: {0}")]
    Persist(#[source] io::Error),

    /// Reading an existing save file failed (open/read).
    #[error("reading save file failed: {0}")]
    Read(#[source] io::Error),

    /// Serializing the caller's save value to JSON failed. In practice
    /// this only happens for types whose `Serialize` impl returns an
    /// error (e.g. a map with non-string keys); ordinary plain-data
    /// game state never hits this branch.
    #[error("serializing save state to JSON failed: {0}")]
    Serialize(#[source] serde_json::Error),

    /// Deserializing an existing save file's JSON failed. Surfaces the
    /// parser error so a corrupt or schema-drifted save is debuggable
    /// from the door log without further instrumentation.
    #[error("deserializing save state from JSON failed: {0}")]
    Deserialize(#[source] serde_json::Error),
}

/// Write `value` to `path` atomically.
///
/// The contract — matching SPEC §13's reliability bar and the
/// "atomic writes" architecture tenet in PROMPT.md — is:
///
/// 1. Ensure the parent directory exists (`mkdir -p`).
/// 2. Create a unique temp file *in the same directory* as `path`.
///    Co-locating the temp file is what makes the final `rename(2)`
///    atomic on POSIX: a cross-filesystem rename would fall back to
///    copy+delete and leave a half-written file visible during the
///    copy.
/// 3. Serialize `value` as pretty-printed JSON, flush the writer's
///    user-space buffer, and `sync_all()` the file to push the bytes
///    out of the kernel page cache to disk. Pretty-printing is a
///    deliberate choice for save files: they're rarely written, are
///    read by humans during debugging, and the size cost is
///    negligible compared to the JSON parser overhead.
/// 4. Persist (rename) the temp file over `path`.
///
/// On any failure, the temp file is dropped and removed by `tempfile`
/// before the function returns — `path` either contains the previous
/// successful save or, if the file never existed, remains absent. It
/// is *never* observed half-written, even if the process is killed
/// between step 2 and step 4: `path` itself isn't touched until the
/// rename, and the temp file's name isn't `path`.
///
/// The crash-mid-write test in this module's tests asserts this
/// invariant directly by simulating an interrupted write (constructing
/// a `NamedTempFile` and dropping it without `persist()`).
pub fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), SaveIoError> {
    let parent = path.parent().ok_or_else(|| SaveIoError::NoParent {
        path: path.display().to_string(),
    })?;

    // Empty parent (`save.json` with no directory) is treated the same
    // way: we'd have nothing to mkdir into and no place to drop the
    // temp file safely. Surface the same error so callers can rely on
    // a single failure mode for "degenerate path".
    if parent.as_os_str().is_empty() {
        return Err(SaveIoError::NoParent {
            path: path.display().to_string(),
        });
    }

    fs::create_dir_all(parent).map_err(SaveIoError::Mkdir)?;

    // `NamedTempFile::new_in` places the temp file alongside the target
    // so the eventual rename stays within one filesystem. Names look
    // like `.tmpXXXXXX`, distinct from `save.json`, so concurrent
    // readers (if any) keep seeing the previous version.
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(SaveIoError::Write)?;

    // serde_json::to_writer_pretty streams directly into the tempfile
    // without buffering the whole JSON document in memory — saves can
    // grow large for inventory-heavy games and we'd rather not double
    // the memory footprint at write time.
    serde_json::to_writer_pretty(&mut tmp, value).map_err(SaveIoError::Serialize)?;

    // Flush user-space buffer before the kernel-level fsync. `Write::flush`
    // on a File is a no-op today but documenting the call protects the
    // invariant if the impl ever grows internal buffering.
    tmp.as_file_mut().flush().map_err(SaveIoError::Write)?;
    // `sync_all` corresponds to fsync(2): pushes bytes + metadata to
    // the disk surface. Without it, a power loss between rename and
    // the next periodic flush could still strand the file as a
    // zero-length entry — atomic rename guards visibility, fsync
    // guards durability.
    tmp.as_file().sync_all().map_err(SaveIoError::Write)?;

    // `persist` performs the rename(2). It returns a `PersistError` on
    // failure (which carries the original tempfile so the caller could
    // retry); we discard that and surface only the io error since the
    // tempfile's `Drop` will remove the scratch path either way.
    tmp.persist(path)
        .map_err(|e| SaveIoError::Persist(e.error))?;

    Ok(())
}

/// Read the JSON save at `path` into a typed value.
///
/// Returns `Ok(None)` exactly when `path` does not exist — the common
/// case for a brand-new player launching the game for the first time,
/// and the only branch where "missing" is not an error. Other I/O
/// failures (permission denied, unreadable file) surface as
/// [`SaveIoError::Read`] so the runtime can distinguish "no save yet"
/// from "save exists but unreachable".
pub fn read_save<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, SaveIoError> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SaveIoError::Read(e)),
    };

    // Reading the whole save into memory is fine: SPEC §12 expects a
    // single-document `save.json` per user, which is small. Streaming
    // through `from_reader` would also work but `read_to_string` gives
    // us a tidier error path for malformed UTF-8.
    let mut buf = String::new();
    file.read_to_string(&mut buf).map_err(SaveIoError::Read)?;
    let value = serde_json::from_str(&buf).map_err(SaveIoError::Deserialize)?;
    Ok(Some(value))
}

// ---------------------------------------------------------------------------
// SaveSlot<T> — typed handle bridging the runtime to read_save / write_atomic
// ---------------------------------------------------------------------------
//
// SPEC_v2_1.md §2.1 introduces `SaveSlot<T>` as the v2.1 ergonomics primitive
// that replaces the hand-rolled `Rc<RefCell<T>>` + dirty-bit pattern every
// game previously had to write by hand (see the v2 `murder_motel` example
// before the Task 5 refactor). The slot is deliberately a thin wrapper:
//
// * `inner: Rc<RefCell<T>>` is the shared, interior-mutable handle that
//   screen constructors can clone freely. Multiple screens hold the same
//   `T` and observe each other's writes the same way the v2 example did
//   with hand-written handles.
// * `dirty: Rc<Cell<bool>>` is the "needs persisting" flag that
//   `borrow_mut` flips on (Task 1b) and `save` clears (Task 1d). The
//   runtime's save handler (Task 4) reads this to skip no-op writes.
//
// Persistence is **not** owned by the slot — it delegates to the existing
// `read_save` / `write_atomic` free functions so the v1 reliability bar
// (atomic rename, parent mkdir, fsync) keeps applying unchanged. The slot
// is a typed *handle*, not a second persistence path.

/// Typed shared handle to a serialisable game-state value.
///
/// `SaveSlot<T>` is the v2.1 primitive that authors reach for instead of
/// rolling their own `Rc<RefCell<T>>` plus a sibling dirty flag. It
/// gives every screen in a game the same view of `T`, tracks whether
/// the value has been mutated since the last save, and bridges to the
/// existing [`read_save`] / [`write_atomic`] persistence helpers — so
/// the SPEC §13 atomic-write contract still applies without a second
/// code path.
///
/// # When to reach for it
///
/// - You have a single game-state value (a struct, an enum, a map) that
///   multiple screens need to read or mutate while the runtime is live.
/// - You want "save on quit" / "save on demand" without each screen
///   re-discovering how to serialise the value.
/// - You are willing to model the value as `T: Serialize +
///   DeserializeOwned + Default + Clone`.
///
/// # When *not* to reach for it
///
/// - Truly ephemeral state that must never persist (e.g. transient
///   feedback toasts) — keep that as a plain `Rc<RefCell<T>>` so the
///   dirty flag can't accidentally pull it into the save file.
/// - State that needs cross-process coordination (the shared-world
///   SQLite layer in v2) — `SaveSlot<T>` is single-process only and
///   makes no synchronisation guarantees.
///
/// # Cheap to clone
///
/// All fields are `Rc<...>`, so `Clone` is a refcount bump. Screens
/// hold their own clone; mutating through any of them flips the same
/// shared dirty flag.
#[derive(Debug)]
pub struct SaveSlot<T> {
    /// Shared, interior-mutable handle to the game state value. Kept
    /// private so callers go through [`SaveSlot::borrow`] /
    /// `borrow_mut` (Task 1b) — that's what lets the slot enforce the
    /// "borrow_mut implies dirty" contract.
    pub(crate) inner: Rc<RefCell<T>>,
    /// "Has the value been mutated since the last successful save?"
    /// flag. Flipped on by `borrow_mut` (Task 1b) and cleared by
    /// `save` (Task 1d). The runtime's save handler (Task 4) consults
    /// this to skip writes when nothing changed.
    pub(crate) dirty: Rc<Cell<bool>>,
}

impl<T> Clone for SaveSlot<T> {
    /// Manual `Clone` impl rather than `#[derive(Clone)]` because the
    /// derive would require `T: Clone` even though every field is
    /// already an `Rc<...>` — cloning a slot bumps refcounts, it does
    /// not clone `T`.
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            dirty: Rc::clone(&self.dirty),
        }
    }
}

impl<T> SaveSlot<T> {
    /// Wrap `value` in a fresh slot.
    ///
    /// The slot starts **clean** (`is_dirty() == false`): the value has
    /// not yet been mutated since "load", and nothing needs persisting.
    /// Authors typically build a slot from `read_save`'s output via
    /// `SaveSlot::load_or_default` (Task 1c); calling `new` directly
    /// is for tests and for games that synthesise their initial state.
    pub fn new(value: T) -> Self {
        Self {
            inner: Rc::new(RefCell::new(value)),
            dirty: Rc::new(Cell::new(false)),
        }
    }

    /// Borrow the wrapped value immutably.
    ///
    /// Panics on an active mutable borrow, matching `RefCell` semantics
    /// — the runtime is single-threaded so any panic here is a
    /// programmer error (a screen holding a `borrow_mut` across an
    /// `await` point or a nested re-entrant render call), not an
    /// environmental failure.
    pub fn borrow(&self) -> Ref<'_, T> {
        self.inner.borrow()
    }

    /// Borrow the wrapped value mutably, **marking the slot dirty**.
    ///
    /// The dirty flag is flipped on **entry**, before the caller has a
    /// chance to actually mutate `T`. SPEC_v2_1 §4.1 requires this
    /// even if the resulting borrow goes unused: the slot has no way
    /// to observe whether a caller mutated through the returned
    /// [`RefMut`], so it conservatively assumes any `borrow_mut`
    /// indicates intent to change. Authors who want a peek at the
    /// value without dirtying it should call [`SaveSlot::borrow`]
    /// instead.
    ///
    /// Panics on an active immutable or mutable borrow, matching
    /// `RefCell::borrow_mut` semantics.
    pub fn borrow_mut(&self) -> RefMut<'_, T> {
        self.dirty.set(true);
        self.inner.borrow_mut()
    }

    /// Return a deep clone of the wrapped value.
    ///
    /// Useful for tests and for screens that need an owned copy to
    /// pass to a renderer or a comparison helper without holding a
    /// borrow across an `await`/render boundary. Does **not** mutate
    /// the slot or touch the dirty flag — taking a snapshot is a
    /// read-only operation.
    pub fn snapshot(&self) -> T
    where
        T: Clone,
    {
        self.inner.borrow().clone()
    }

    /// Overwrite the wrapped value in place and mark the slot dirty.
    ///
    /// Equivalent to `*slot.borrow_mut() = value;` but avoids the
    /// `RefMut` round-trip at the call site and reads more naturally
    /// when authors are bulk-replacing state (e.g. a "reset to new
    /// game" button or a "load this snapshot" admin command). Like
    /// [`SaveSlot::borrow_mut`], it always sets the dirty flag —
    /// assigning the same value is still treated as a write because
    /// the slot can't
    /// cheaply prove equality for an arbitrary `T`.
    pub fn apply(&self, value: T) {
        *self.inner.borrow_mut() = value;
        self.dirty.set(true);
    }

    /// Has the slot been mutated since the last `save` (or load)?
    ///
    /// Advisory only: SPEC_v2_1 §4.1 explicitly forbids using this to
    /// *gate* a save — authors decide when to persist. The runtime's
    /// save handler (Task 4) reads it to skip no-op writes when
    /// nothing has changed since startup.
    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    /// Return a clone-equivalent handle to the same underlying state.
    ///
    /// Functionally identical to [`Clone::clone`]; exposed under a
    /// different name so authors reading constructor signatures like
    /// `MapScreen::with_slots(slots.save.handle())` immediately see
    /// the cheap-`Rc` semantics. SPEC_v2_1 §4.1 mandates this method
    /// precisely so the API documents the sharing model at the call
    /// site.
    pub fn handle(&self) -> SaveSlot<T> {
        self.clone()
    }
}

impl<T: Serialize> SaveSlot<T> {
    /// Persist the slot's current value to `path` atomically, then clear
    /// the dirty flag.
    ///
    /// Thin typed wrapper over [`write_atomic`]: SPEC §13's reliability
    /// bar (parent `mkdir -p`, temp-file write, fsync, `rename(2)`)
    /// keeps applying because the actual byte-level write is delegated.
    /// The slot adds two things on top:
    ///
    /// 1. A typed entry point — callers don't have to remember to pass
    ///    a `&T` borrowed from the right place, the slot already holds
    ///    the canonical handle.
    /// 2. Dirty-flag bookkeeping — once `write_atomic` succeeds the slot
    ///    is by definition in sync with disk, so `is_dirty()` returns
    ///    `false` again and the runtime's "save iff dirty" hook
    ///    (Task 4) won't immediately rewrite the same bytes.
    ///
    /// On failure the dirty flag is **left set** so the next save
    /// attempt still tries to push the unsaved changes — clearing it
    /// before knowing the bytes landed would silently swallow data
    /// loss. This is the same reason `read_save` distinguishes "missing"
    /// from "corrupt" rather than collapsing both into `None`.
    pub fn save(&self, path: &Path) -> Result<(), SaveIoError> {
        // Borrow immutably while serialising so other handles can still
        // read concurrently from the same `RefCell` (interior reads do
        // not block each other). Using `borrow_mut` here would pointlessly
        // contend with renderers — the write goes through `write_atomic`
        // on a separate temp file, not into `T`.
        write_atomic(path, &*self.inner.borrow())?;
        // Only clear after the rename has succeeded. Failed writes leave
        // the flag set so a retry actually retries.
        self.dirty.set(false);
        Ok(())
    }
}

impl<T: Serialize + 'static> SaveSlot<T> {
    /// Build a [`crate::runtime::SaveHandler`] closure that persists this slot's
    /// current contents to `path` whenever the runtime invokes it.
    ///
    /// The returned closure captures **a clone of the slot's `Rc`
    /// handles**, not the binding itself. SPEC_v2_1 §4.1 mandates this
    /// so the handler stays valid even if the original `SaveSlot`
    /// binding goes out of scope (e.g. the author moves the slot into
    /// a screen constructor, then registers the handler with
    /// `Game::with_save_handler`). Cloning is a refcount bump — see
    /// the `Cheap to clone` note on [`SaveSlot`].
    ///
    /// Each invocation calls [`SaveSlot::save`], which delegates to
    /// [`write_atomic`]. Errors are remapped to [`crate::runtime::GameError::Save`]
    /// using the underlying [`SaveIoError`]'s `Display` impl —
    /// stringifying preserves the granular variant message (`writing
    /// save file failed`, `renaming temp save into place failed`,
    /// etc.) in the operator-facing log without forcing the runtime
    /// to grow a `From<SaveIoError> for GameError` impl that the rest
    /// of the codebase doesn't need.
    ///
    /// `path` is taken by value (`PathBuf`) so the closure owns it
    /// outright; passing `&Path` would tie the handler to the
    /// caller's stack frame and defeat the "outlives the original
    /// binding" guarantee above.
    ///
    /// # Why no dirty check here
    ///
    /// The handler unconditionally writes when invoked. SPEC_v2_1
    /// §4.1 explicitly forbids using `is_dirty` to *gate* persistence
    /// — that decision belongs to the runtime / the author. A
    /// "save iff dirty" loop builds on top of this primitive by
    /// checking `is_dirty()` before invoking the handler, not inside
    /// it.
    pub fn save_handler(&self, path: PathBuf) -> crate::runtime::SaveHandler {
        // Clone the `Rc`-bearing slot so the closure owns its own
        // refcounted view. Reusing `Self::clone` (refcount bump only)
        // keeps this allocation-light.
        let slot = self.clone();
        Box::new(move || {
            slot.save(&path)
                .map_err(|e| crate::runtime::GameError::Save(e.to_string()))
        })
    }
}

impl<T: DeserializeOwned> SaveSlot<T> {
    /// Load a slot from `path`, returning `Ok(None)` if no save exists.
    ///
    /// Thin typed wrapper over [`read_save`]: the v1 reliability bar
    /// (atomic rename, distinct corrupt-vs-missing errors) keeps
    /// applying because the actual byte-level read is delegated. The
    /// only addition over `read_save` is wrapping the deserialised
    /// value in a fresh, **clean** [`SaveSlot`] so the runtime's
    /// "save on dirty" hook (Task 4) doesn't immediately rewrite a
    /// just-loaded file.
    ///
    /// Returns:
    /// - `Ok(Some(slot))` when `path` exists and parses successfully.
    /// - `Ok(None)` when `path` does not exist (the brand-new-player
    ///   path — same semantics as `read_save`).
    /// - `Err(SaveIoError::Read | Deserialize | …)` for any other
    ///   failure (permission denied, corrupt JSON, etc.).
    ///
    /// Authors who want a default-on-missing behaviour should reach
    /// for [`SaveSlot::load_or_default`] instead.
    pub fn load(path: &Path) -> Result<Option<SaveSlot<T>>, SaveIoError> {
        match read_save::<T>(path)? {
            Some(value) => Ok(Some(SaveSlot::new(value))),
            None => Ok(None),
        }
    }

    /// Load a slot from `path`, falling back to `T::default()` on
    /// missing file.
    ///
    /// The 90% authoring path: a brand-new player has no save on disk,
    /// so the game starts from `T::default()`; a returning player gets
    /// their previous state. Corrupt or unreadable saves still surface
    /// as `Err` — silently overwriting a player's broken save with a
    /// fresh default would be a data-loss bug.
    ///
    /// The returned slot is always **clean**: a default-constructed
    /// slot has no unsaved changes, and a freshly-loaded slot's bytes
    /// are already on disk by definition. This pairs with Task 1d's
    /// `save` (which clears the flag) so a "save iff dirty" loop is
    /// well-behaved from the first launch.
    pub fn load_or_default(path: &Path) -> Result<SaveSlot<T>, SaveIoError>
    where
        T: Default,
    {
        match Self::load(path)? {
            Some(slot) => Ok(slot),
            None => Ok(SaveSlot::new(T::default())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(source: ContextSource, user: Option<&str>) -> FogletContext {
        FogletContext {
            door_id: "door-1".into(),
            user_id: user.map(str::to_string),
            username: None,
            role: None,
            session_id: None,
            terminal_width: 80,
            terminal_height: 24,
            source,
        }
    }

    fn empty_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn strategy_none_short_circuits() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap();
        assert!(path.is_none(), "SaveStrategy::None must disable saves");
    }

    #[test]
    fn cli_override_wins_over_everything_else() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let dir = Path::new("/tmp/explicit");
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: Some(dir),
            },
            |name| {
                // Even if the env var is set, the CLI flag must win.
                if name == SAVE_DIR_ENV {
                    Some("/tmp/from-env".into())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .expect("non-None strategy should yield a path");
        assert_eq!(path, PathBuf::from("/tmp/explicit/save.json"));
    }

    #[test]
    fn fgk_save_dir_env_used_when_no_cli_override() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            |name| {
                if name == SAVE_DIR_ENV {
                    Some("/var/saves/door".into())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(path, PathBuf::from("/var/saves/door/save.json"));
    }

    #[test]
    fn empty_fgk_save_dir_is_ignored() {
        // Empty string from a stray `export FGK_SAVE_DIR=` must NOT be
        // treated as "save next to cwd"; it falls through to defaults.
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            |name| {
                if name == SAVE_DIR_ENV {
                    Some(String::new())
                } else {
                    None
                }
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-1/save.json")
        );
    }

    #[test]
    fn production_path_uses_user_id_under_context_file_source() {
        let context = ctx(ContextSource::ContextFile, Some("u-42"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-42/save.json")
        );
    }

    #[test]
    fn production_path_used_when_context_came_from_env_vars() {
        // Source = Env means a Foglet-like shell wrapped us with
        // `FOGLET_*` env vars but no JSON context file. Same SPEC §12
        // production path applies.
        let context = ctx(ContextSource::Env, Some("u-99"));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/u-99/save.json")
        );
    }

    #[test]
    fn anonymous_segment_used_when_user_id_missing_in_production() {
        let context = ctx(ContextSource::ContextFile, None);
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/anonymous/save.json")
        );
    }

    #[test]
    fn empty_user_id_treated_as_missing() {
        let context = ctx(ContextSource::ContextFile, Some(""));
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/srv/foglet/doors/murder-motel/saves/anonymous/save.json")
        );
    }

    #[test]
    fn local_dev_path_is_relative_to_cwd() {
        let context = ctx(ContextSource::LocalDev, None);
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap()
        .unwrap();
        assert_eq!(path, PathBuf::from(".fgk/saves/local-dev/save.json"));
    }

    #[test]
    fn empty_slug_is_an_error() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let err = resolve_save_path(
            &SavePathInputs {
                slug: "",
                strategy: SaveStrategy::PerFogletUser,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap_err();
        assert!(matches!(err, SavePathError::EmptySlug));
    }

    #[test]
    fn empty_slug_is_an_error_even_when_strategy_is_none() {
        // Strategy::None short-circuits *after* validation so an empty
        // slug never silently masks a programmer bug.
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let err = resolve_save_path(
            &SavePathInputs {
                slug: "",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: None,
            },
            empty_env,
        )
        .unwrap_err();
        assert!(matches!(err, SavePathError::EmptySlug));
    }

    #[test]
    fn cli_override_with_strategy_none_still_returns_none() {
        let context = ctx(ContextSource::ContextFile, Some("u-1"));
        let dir = Path::new("/tmp/explicit");
        let path = resolve_save_path(
            &SavePathInputs {
                slug: "murder-motel",
                strategy: SaveStrategy::None,
                context: &context,
                cli_override: Some(dir),
            },
            empty_env,
        )
        .unwrap();
        assert!(path.is_none());
    }

    // ----- Atomic write/read tests (Task 8b) -------------------------
    //
    // These tests live alongside the path-resolution tests because the
    // two halves of `save` are tightly coupled in the runtime: the
    // resolved path is what the writer renames into. Keeping them in
    // one module lets a future reader see the contract end-to-end.

    use serde::Deserialize;

    /// Tiny stand-in for an authored game's save state. Fields cover
    /// the shapes a real game cares about (scalar, string, sequence,
    /// nested) so the round-trip test exercises serde's normal paths.
    // `Default` derive lets the Task 1c `load_or_default` tests use
    // `SaveFixture::default()` for the brand-new-player branch. The
    // derived value is `version: 0`, empty player/inventory/flags —
    // deliberately distinct from `fixture_v1` (which uses `version: 1`)
    // so a test failure can distinguish "we hit the default path" from
    // "we loaded v1 from disk".
    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct SaveFixture {
        version: u32,
        player: String,
        inventory: Vec<String>,
        flags: Vec<(String, bool)>,
    }

    fn fixture_v1() -> SaveFixture {
        SaveFixture {
            version: 1,
            player: "alice".into(),
            inventory: vec!["matches".into(), "key-203".into()],
            flags: vec![("met-clerk".into(), true)],
        }
    }

    fn fixture_v2() -> SaveFixture {
        SaveFixture {
            version: 2,
            player: "alice".into(),
            inventory: vec!["matches".into(), "key-203".into(), "ledger".into()],
            flags: vec![("met-clerk".into(), true), ("found-ledger".into(), true)],
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let original = fixture_v1();

        write_atomic(&path, &original).unwrap();
        let loaded: SaveFixture = read_save(&path).unwrap().expect("file should exist");

        assert_eq!(loaded, original);
    }

    #[test]
    fn read_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-written.json");

        let loaded: Option<SaveFixture> = read_save(&path).unwrap();
        assert!(
            loaded.is_none(),
            "missing file is the brand-new-player path"
        );
    }

    #[test]
    fn write_creates_missing_parent_directories() {
        // SPEC §12's per-user save path includes a `<user_id>` dir that
        // doesn't exist on first launch. The writer must `mkdir -p` it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saves").join("u-42").join("save.json");

        write_atomic(&path, &fixture_v1()).unwrap();
        assert!(
            path.exists(),
            "save.json should be created under freshly-made dirs"
        );
    }

    #[test]
    fn second_write_overwrites_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");

        write_atomic(&path, &fixture_v1()).unwrap();
        write_atomic(&path, &fixture_v2()).unwrap();

        let loaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(loaded, fixture_v2());
    }

    #[test]
    fn crash_mid_write_leaves_previous_version_intact() {
        // Simulates the failure mode the SPEC §13 reliability bar and
        // PROMPT.md's "atomic writes" tenet are meant to defend
        // against: process death between "started writing" and
        // "successfully renamed". The temp file is created and written
        // but never `persist()`ed; on drop, `tempfile` removes the
        // scratch file and the previously-renamed `save.json` must
        // still hold v1 — never half-written, never absent.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");

        // First, an honest save lands v1 atomically.
        write_atomic(&path, &fixture_v1()).unwrap();

        // Now begin a v2 write the way `write_atomic` does, but bail
        // out before the `persist()` step.
        {
            let parent = path.parent().unwrap();
            let mut tmp = tempfile::NamedTempFile::new_in(parent).unwrap();
            serde_json::to_writer_pretty(&mut tmp, &fixture_v2()).unwrap();
            tmp.as_file_mut().flush().unwrap();
            // Intentionally drop `tmp` without calling `.persist(&path)`.
            // This models a crash mid-write: the temp file vanishes,
            // and `path` itself was never touched.
            drop(tmp);
        }

        // Final check: `save.json` is *exactly* v1, byte-for-byte the
        // outcome of the previous successful write. Never absent,
        // never half-written, never v2.
        let loaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(loaded, fixture_v1());

        // And no stray temp files were left behind in the dir.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "save.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "no scratch files should outlive a dropped NamedTempFile, got {leftovers:?}",
        );
    }

    #[test]
    fn read_surfaces_deserialize_error_for_corrupt_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        fs::write(&path, b"{ not valid json").unwrap();

        let err = read_save::<SaveFixture>(&path).unwrap_err();
        assert!(matches!(err, SaveIoError::Deserialize(_)));
    }

    // ----- SaveSlot<T> tests (Task 1) ------------------------------
    //
    // 1a covers only the constructor + immutable borrow. Mutation,
    // dirty-flag tracking, snapshot/apply, handle cloning, and the
    // load/save helpers land in 1b–1e and grow the test surface there.

    #[test]
    fn save_slot_new_round_trips_value_through_borrow() {
        // Smallest possible contract for 1a: a freshly-constructed slot
        // exposes the value the caller put in, byte-for-byte, through
        // an immutable borrow. The fixture struct re-uses the same
        // shape as `SaveFixture` above on purpose — this is the kind of
        // record an authored game would actually persist.
        let original = fixture_v1();
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(original.clone());

        let view = slot.borrow();
        assert_eq!(*view, original);
    }

    #[test]
    fn save_slot_starts_clean() {
        // 1b precondition: a freshly-built slot is not dirty. The dirty
        // flag is intended to track *post-construction* mutations, not
        // the initial assignment via `new`. Authors rely on this so a
        // "save on Quit if dirty" runtime hook (Task 4) doesn't write
        // an unmodified file every launch.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        assert!(!slot.is_dirty(), "freshly-constructed slot must be clean");
    }

    #[test]
    fn save_slot_borrow_mut_sets_dirty_flag() {
        // SPEC_v2_1 §4.1 rule: `borrow_mut()` flips the dirty flag on
        // entry, regardless of whether the caller actually mutates
        // through the returned `RefMut`. We exercise both halves:
        // (a) merely calling `borrow_mut` dirties the slot, and
        // (b) an actual mutation through it is observable on a later
        //     `borrow`.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        {
            let mut view = slot.borrow_mut();
            view.player = "bob".into();
        }
        assert!(slot.is_dirty(), "borrow_mut must set the dirty flag");
        assert_eq!(slot.borrow().player, "bob");
    }

    #[test]
    fn save_slot_borrow_mut_dirties_even_without_mutation() {
        // The slot can't prove the caller didn't mutate, so it
        // conservatively dirties on any `borrow_mut`. This test pins
        // that behaviour so a future "optimise: track real writes"
        // change has to update the contract deliberately.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        {
            let _view = slot.borrow_mut();
            // Drop without writing.
        }
        assert!(slot.is_dirty(), "borrow_mut dirties even on no-op writes");
    }

    #[test]
    fn save_slot_borrow_does_not_dirty() {
        // The mirror of the borrow_mut test: read-only access must
        // never set the dirty flag. Otherwise a "save on dirty" loop
        // would write on every render.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        {
            let _view = slot.borrow();
        }
        assert!(!slot.is_dirty(), "borrow must not flip the dirty flag");
    }

    #[test]
    fn save_slot_snapshot_returns_independent_clone() {
        // `snapshot` returns an *owned* clone — mutating it must not
        // be visible through the slot. This is the property that lets
        // authors hand a snapshot to a renderer or a serialiser
        // without worrying about aliasing.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let mut snap = slot.snapshot();
        snap.player = "mallory".into();

        assert_eq!(slot.borrow().player, "alice", "slot is unchanged");
        assert_eq!(snap.player, "mallory", "snapshot was mutated locally");
    }

    #[test]
    fn save_slot_snapshot_does_not_dirty() {
        // Read-only operation. Snapshotting on every frame would
        // otherwise mark the slot dirty forever.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let _snap = slot.snapshot();
        assert!(!slot.is_dirty(), "snapshot must not flip the dirty flag");
    }

    #[test]
    fn save_slot_apply_overwrites_in_place_and_dirties() {
        // `apply` is the bulk-replace path used by reset / load. After
        // it returns, observers see the new value and the slot is
        // dirty so the next save will persist it.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        slot.apply(fixture_v2());

        assert_eq!(*slot.borrow(), fixture_v2());
        assert!(slot.is_dirty(), "apply must set the dirty flag");
    }

    #[test]
    fn save_slot_handle_shares_state_with_original() {
        // Cloning via `handle` (or `Clone::clone`) bumps refcounts —
        // both handles see each other's writes and share one dirty
        // flag. This is the property that makes `SaveSlot<T>` viable
        // as a per-screen constructor argument: every screen's clone
        // is the same logical slot.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let handle = slot.handle();

        // Mutate through `handle`; the original observes it.
        handle.apply(fixture_v2());
        assert_eq!(*slot.borrow(), fixture_v2());
        assert!(slot.is_dirty());
        assert!(handle.is_dirty(), "dirty flag is shared, not per-handle");
    }

    #[test]
    fn save_slot_handle_is_alias_for_clone() {
        // `handle()` and `clone()` are documented as equivalent; pin
        // that. If one ever grows divergent semantics it should be
        // a deliberate API change with a `DECISIONS.md` entry.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let via_handle = slot.handle();
        let via_clone = slot.clone();

        // Both should see the same mutation through `slot`.
        slot.apply(fixture_v2());
        assert_eq!(*via_handle.borrow(), fixture_v2());
        assert_eq!(*via_clone.borrow(), fixture_v2());
    }

    // ----- SaveSlot::load / load_or_default tests (Task 1c) ---------

    #[test]
    fn save_slot_load_returns_none_for_missing_file() {
        // Mirror of `read_returns_none_for_missing_file` at the typed
        // slot layer: `load` propagates the `None` so authors can
        // distinguish "no save yet" from "save exists but unreadable".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-written.json");

        let loaded = SaveSlot::<SaveFixture>::load(&path).unwrap();
        assert!(loaded.is_none(), "missing file must surface as Ok(None)");
    }

    #[test]
    fn save_slot_load_round_trips_existing_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let original = fixture_v1();
        write_atomic(&path, &original).unwrap();

        let slot = SaveSlot::<SaveFixture>::load(&path)
            .unwrap()
            .expect("file written above; load must yield Some");
        assert_eq!(*slot.borrow(), original);
        assert!(
            !slot.is_dirty(),
            "freshly-loaded slot must be clean — its bytes are already on disk",
        );
    }

    #[test]
    fn save_slot_load_surfaces_deserialize_error() {
        // Corrupt JSON must not be silently swallowed: that would
        // shadow real data corruption behind a fresh default and
        // surprise the operator reading the door log. Mirrors the
        // free-function `read_surfaces_deserialize_error_for_corrupt_json`
        // test for the typed slot path.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        fs::write(&path, b"{ not valid json").unwrap();

        let err = SaveSlot::<SaveFixture>::load(&path).unwrap_err();
        assert!(matches!(err, SaveIoError::Deserialize(_)));
    }

    #[test]
    fn save_slot_load_or_default_uses_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("never-written.json");

        let slot = SaveSlot::<SaveFixture>::load_or_default(&path).unwrap();
        assert_eq!(*slot.borrow(), SaveFixture::default());
        assert!(
            !slot.is_dirty(),
            "default-constructed slot starts clean — Task 1b contract",
        );
    }

    #[test]
    fn save_slot_load_or_default_loads_existing_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        write_atomic(&path, &fixture_v1()).unwrap();

        let slot = SaveSlot::<SaveFixture>::load_or_default(&path).unwrap();
        assert_eq!(*slot.borrow(), fixture_v1());
        assert!(
            !slot.is_dirty(),
            "loaded slot must be clean so save-on-dirty doesn't rewrite immediately",
        );
    }

    #[test]
    fn save_slot_load_or_default_surfaces_corrupt_save_error() {
        // Critical: a corrupt save must NOT be silently replaced with
        // a default. The player's broken file is the only evidence
        // they ever played; overwriting it with a default would be a
        // data-loss bug. Authors handle the error explicitly (e.g.
        // back up the corrupt file, prompt the player) before deciding
        // whether to start fresh.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        fs::write(&path, b"\x00not json at all").unwrap();

        let err = SaveSlot::<SaveFixture>::load_or_default(&path).unwrap_err();
        assert!(matches!(err, SaveIoError::Deserialize(_)));
    }

    // ----- SaveSlot::save tests (Task 1d) ---------------------------

    #[test]
    fn save_slot_save_round_trips_through_write_atomic() {
        // After `save`, the bytes on disk must match what a subsequent
        // `load` returns — the slot is just a typed handle on top of
        // `write_atomic` / `read_save`, so the round-trip property the
        // free functions guarantee must propagate to the slot API.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());

        slot.save(&path).unwrap();

        let reloaded = SaveSlot::<SaveFixture>::load(&path)
            .unwrap()
            .expect("save wrote the file");
        assert_eq!(*reloaded.borrow(), fixture_v1());
    }

    #[test]
    fn save_slot_save_clears_dirty_flag() {
        // The whole point of the dirty bit is to let a "save iff dirty"
        // loop skip no-op writes. `save` must clear the flag once the
        // bytes are durable on disk.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        slot.borrow_mut().player = "bob".into();
        assert!(slot.is_dirty(), "precondition: borrow_mut dirties");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        slot.save(&path).unwrap();

        assert!(
            !slot.is_dirty(),
            "successful save must clear the dirty flag",
        );
    }

    #[test]
    fn save_slot_save_persists_latest_mutations() {
        // A mutation through `borrow_mut` between construction and save
        // must end up in the file — verifying the slot serialises its
        // *current* contents, not whatever was passed to `new`.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        slot.apply(fixture_v2());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        slot.save(&path).unwrap();

        let reloaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(reloaded, fixture_v2());
    }

    #[test]
    fn save_slot_save_failure_leaves_dirty_flag_set() {
        // If the write fails (here: degenerate path with no parent
        // directory), the slot is still out of sync with disk — leaving
        // the flag set lets a retry actually retry, instead of silently
        // dropping unsaved changes.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        slot.borrow_mut().player = "bob".into();
        assert!(slot.is_dirty());

        // `save.json` with no directory triggers `SaveIoError::NoParent`,
        // mirroring the existing `write_to_path_with_no_parent_errors`
        // case for the free function.
        let err = slot.save(Path::new("save.json")).unwrap_err();
        assert!(matches!(err, SaveIoError::NoParent { .. }));
        assert!(
            slot.is_dirty(),
            "failed save must NOT clear the dirty flag — retry has to retry",
        );
    }

    // ----- SaveSlot::save_handler tests (Task 1e) ------------------

    #[test]
    fn save_slot_save_handler_persists_current_contents() {
        // A handler invocation must produce the same on-disk bytes as
        // a direct `save()` call — proving the indirection through
        // `SaveHandler` is just plumbing, not a behavioural fork.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());

        let mut handler = slot.save_handler(path.clone());
        handler().expect("handler write should succeed");

        let reloaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(reloaded, fixture_v1());
        assert!(
            !slot.is_dirty(),
            "underlying save() clears dirty; handler must inherit that",
        );
    }

    #[test]
    fn save_slot_save_handler_observes_post_registration_mutations() {
        // The handler holds an `Rc` to the same backing state, so a
        // mutation made *after* the handler was constructed must end
        // up in the file. This is the property that makes the handler
        // useful at all: register once at startup, persist the live
        // state at quit/`SideEffect::Save` time.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());

        let mut handler = slot.save_handler(path.clone());
        slot.apply(fixture_v2());
        handler().unwrap();

        let reloaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(reloaded, fixture_v2());
    }

    #[test]
    fn save_slot_save_handler_outlives_original_binding() {
        // SPEC_v2_1 §4.1 rule: the handler captures the slot via `Rc`,
        // so it MUST keep working after the originally-named binding
        // is dropped. We model the realistic call shape: build the
        // slot, hand a handle to a "screen" (here, a vector cell),
        // register the handler, then drop the original binding by
        // moving it into a no-op closure that immediately falls out
        // of scope.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("save.json");

        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let mut handler = slot.save_handler(path.clone());

        // "Move" the original binding away — represents the author
        // handing the slot to a screen constructor and never holding
        // it themselves again. The handler still owns its own clone.
        drop(slot);

        handler().expect("handler must work after the original slot is dropped");
        let reloaded: SaveFixture = read_save(&path).unwrap().unwrap();
        assert_eq!(reloaded, fixture_v1());
    }

    #[test]
    fn save_slot_save_handler_surfaces_errors_as_game_error_save() {
        // Failure mode: a degenerate path (`save.json` with no
        // directory) makes `write_atomic` return `SaveIoError::NoParent`.
        // The handler must remap that into `GameError::Save(_)` so the
        // runtime can convert it to its public error variant without
        // knowing about `SaveIoError`.
        let slot: SaveSlot<SaveFixture> = SaveSlot::new(fixture_v1());
        let mut handler = slot.save_handler(PathBuf::from("save.json"));

        let err = handler().unwrap_err();
        assert!(
            matches!(err, crate::runtime::GameError::Save(_)),
            "expected GameError::Save, got {err:?}",
        );
    }

    #[test]
    fn write_to_path_with_no_parent_errors() {
        // Path `save.json` (no directory component) is degenerate: we
        // can't drop a temp file alongside it deterministically. The
        // writer surfaces a clear error rather than silently writing
        // somewhere surprising like the cwd.
        let path = Path::new("save.json");
        let err = write_atomic(path, &fixture_v1()).unwrap_err();
        assert!(matches!(err, SaveIoError::NoParent { .. }));
    }
}
