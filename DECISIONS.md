## 2026-05-10 — Concurrency for `run_due_ticks`

- **Context**: Task 9f requires that two concurrent `run_due_ticks` callers
  observe disjoint due-task sets. The previous prefetch-all approach could let
  both callers claim the same due row under overlap, leading one caller to skip
  the next task when `max_catchup_per_call` is small.
- **Decision**: Claim one due task at a time inside each loop iteration with a
  `SELECT ... LIMIT 1` + conditional `UPDATE ... SET last_run_at = now WHERE
  key = ? AND (due predicate)` pattern. If `UPDATE` matches zero rows, the caller
  retries from the next due row; if it matches one row, it proceeds to the
  callback in that transaction.
- **Consequence**: concurrent callers now partition due work without duplicate
  callback execution, while preserving callback-level transactional rollback and
  keeping the same explicit cap via `max_catchup_per_call`.

## 2026-05-10 — Test-only callback delay for `fgk tick` interruption smoke

- **Context**: Task 10d requires an interruption-safe CLI smoke test that can
  reliably signal the command during an in-process tick callback and verify
  transactional rollback of `last_run_at`.
- **Decision**: In `fgk::tick::run_tick`, parse optional env var
  `FGK_TICK_TEST_CALLBACK_DELAY_MS` and sleep for that duration inside each
  temporary CLI callback used to execute due tasks.
- **Consequence**: The CLI interruption test can deterministically interrupt a
  running callback while normal `fgk tick` behavior stays unchanged unless the
  variable is explicitly set.

## 2026-05-10 — Open Question: `fgk new` scaffold dependency source

- **Context**: Completion condition 11 requires `cargo run -p fgk -- new <tmp>`
  to produce a project that itself passes `cargo test`. The scaffold template
  currently pins `foglet_game = "0.1"`, and this local environment cannot
  resolve that crate from crates.io, so `cargo test` fails before compilation.
- **Question**: Should scaffolds keep crates.io-only dependencies, or should
  `fgk new` support an opt-in local-path mode for in-repo verification loops
  and CI environments where `foglet_game` is not published?

## 2026-05-10 — `fgk new` auto-detects local `foglet_game` checkout

- **Context**: v4 Task 13b requires `fgk new <tmp>` projects to pass
  `cargo test` in-repo, but crates.io-only scaffolds fail when `foglet_game`
  is not published or not resolvable in local CI/dev loops.
- **Decision**: Keep `fgk new` command shape unchanged and resolve
  `foglet_game` dependency source at scaffold time. When a sibling
  `crates/foglet_game/Cargo.toml` exists relative to `crates/fgk`,
  scaffold `Cargo.toml` with a path dependency; otherwise emit the
  existing crates.io version requirement (`"0.1"`).
- **Consequence**: In-repo verification loops can compile scaffolded
  projects without publishing `foglet_game`, while standalone `fgk`
  installs still generate crates.io-compatible scaffolds.

## 2026-05-10 — v5 follows the repo's flat primitive-module layout

- **Context**: `CHECKLIST_v5.md` Task 1 names future namespaces as
  `crates/foglet_game/src/{contracts,job_board,travel,inventory_capacity}/`,
  but the existing codebase already standardizes on one top-level Rust
  file per primitive (`spatial.rs`, `presence.rs`, `place_recall.rs`,
  `inventory.rs`, `world_ticks.rs`) plus explicit subtrees only when a
  namespace truly groups multiple screens or test helpers.
- **Decision**: Implement the new v5 primitives as flat top-level modules
  (`contracts.rs`, `job_board.rs`, `travel.rs`, `inventory_capacity.rs`)
  and reserve nested paths for the checklist's explicit grouped surfaces
  (`screens/event_log.rs` and `test_support/multi_user.rs`). Keep
  `GameContext` integration additive in `screen.rs`, and continue the
  built-in migration version sequence after v4's `world_tick_tasks`
  migration (`version = 15`) so v5 tables can slot cleanly into the
  existing `WorldDb::apply_migration` flow.
- **Consequence**: v5 lands in the same shape contributors already know,
  avoids a mixed flat-vs-directory primitive layout inside
  `crates/foglet_game/src`, and makes the upcoming config/runtime wiring
  changes straightforward because they can mirror the established v4
  integration pattern.
