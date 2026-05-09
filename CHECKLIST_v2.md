# foglet-game-kit-rs Checklist — v2 Shared World

One unchecked item per iteration. Dependencies in `[brackets]` must be checked off before the dependent item is eligible. This checklist extends `CHECKLIST.md`; do not begin v2 implementation until v1 acceptance criteria are complete.

## Implementation tasks

- [x] **Task 1 — Add SQLite dependency and ADR**
      Add the chosen SQLite crate to the workspace crate budget and record an ADR explaining why. Candidate: `rusqlite` with bundled SQLite disabled unless the ADR justifies bundling. `cargo check` must still pass. [v1]

### Task 2 — World configuration

- [x] **2a** — Extend `GameConfig` with optional `[world]` fields: `enabled`, `path`, `busy_timeout_ms`, and `journal_mode`. Tests: absent section defaults to disabled. [Task 1]
- [x] **2b** — Parse `[turns]` config with `daily_allowance`, `reset`, and `carryover_max`. Tests: valid config parses; negative/zero allowance fails clearly. [2a]
- [x] **2c** — Parse `[[leaderboards]]` config with `name` and `sort`. Tests: duplicate leaderboard names are rejected. [2a]
- [x] **2d** — Add a config fixture for `examples/murder_motel/assets/game.toml` enabling world, turns, and `investigators` leaderboard. [2a, 2b, 2c]

### Task 3 — World DB open/bootstrap

- [x] **3a** — Create `crates/foglet_game/src/world_db.rs` with `WorldDb` type and module docs. Test: opens SQLite under a temp dir. [Task 1]
- [x] **3b** — Create parent directories before opening the DB. Test: nested `world/world.sqlite` path is created. [3a]
- [x] **3c** — Apply busy timeout from config. Test: connection reports configured timeout behavior or exposes stored setting. [3a]
- [x] **3d** — Apply WAL journal mode when configured and supported. Test: querying `PRAGMA journal_mode` returns `wal` or documented fallback. [3a]

### Task 4 — Migration foundation

- [x] **4a** — Add `world_migrations` table creation. Test: table exists after bootstrap. [3a]
- [x] **4b** — Add `WorldMigration { version, name, sql }` and apply one migration. Test: version is recorded. [4a]
- [x] **4c** — Make migration application idempotent. Test: applying the same migration twice records one row and leaves schema valid. [4b]
- [x] **4d** — Surface failed migration errors without recording success. Test: invalid SQL returns an error and no migration row. [4b]

### Task 5 — Player registry

- [x] **5a** — Add `players` migration with `foglet_user_id`, `handle`, `role`, `security_level`, `first_seen_at`, `last_seen_at`, and `local_dev_key`. [4b]
- [x] **5b** — Implement `PlayerRecord` and `WorldDb::upsert_player(&FogletContext)`. Test: user_id creates stable record. [5a]
- [x] **5c** — Support local-dev player keys when `FogletContext.user_id` is missing. Test: two local-dev handles do not collide. [5b]
- [x] **5d** — Update `last_seen_at` on repeat upsert without changing `first_seen_at`. [5b]
- [x] **5e** — Add `FogletRole` parsing and `FogletContext::security_level()` mapping: `sysop` = 100, `mod` = 90, `user`/missing/unknown = 50. Tests cover mixed-case and unknown roles. [5b]
- [x] **5f** — Persist normalized role/security metadata during player upsert without using it for launch authorization. Test: sysop/mod/user contexts create distinct advisory metadata. [5e]

### Task 6 — Turn ledger

- [x] **6a** — Add `turn_ledger` migration keyed by player and local date. [5a]
- [x] **6b** — Add injectable date provider abstraction for turn tests. [6a]
- [x] **6c** — Implement initial daily allowance creation. Test: new player gets configured allowance. [6b]
- [x] **6d** — Implement atomic turn spend. Test: spending decrements balance. [6c]
- [x] **6e** — Reject insufficient turns without changing balance. [6d]
- [x] **6f** — Implement carryover cap on new-day reset. Tests cover no carryover and capped carryover. [6d]

### Task 7 — Event log

- [x] **7a** — Add `world_events` migration with timestamp, kind, player id, message, and metadata JSON. [5a]
- [x] **7b** — Implement `append_event`. Test: event is stored with player id and kind. [7a]
- [x] **7c** — Implement `recent_events(limit)`. Test: newest events return first with deterministic tie ordering. [7b]
- [x] **7d** — Implement `player_events(player_id, limit)`. Test: filters by player. [7b]
- [x] **7e** — Add message sanitization/validation guard rejecting empty messages and overlong messages. [7b]

### Task 8 — Leaderboards

- [x] **8a** — Add `leaderboard_scores` migration with named board, player id, score, updated_at. [5a]
- [x] **8b** — Implement `set_score`. Test: first write creates a row. [8a]
- [x] **8c** — Implement `increment_score`. Test: increments existing score and creates missing score. [8b]
- [x] **8d** — Implement `top_scores(name, n)`. Test: deterministic tie ordering by score then updated/player id. [8c]
- [x] **8e** — Implement `player_rank(name, player_id)`. Test: rank reflects tie ordering. [8d]

### Task 9 — Transaction helper

- [x] **9a** — Expose `WorldDb::transaction` wrapper. Test: commit persists writes. [3a]
- [x] **9b** — Test rollback on closure error. [9a]
- [ ] **9c** — Add helper for spend-turn + mutate + append-event transaction. Test: insufficient turns rolls back event/world mutation. [6d, 7b, 9a]

### Task 10 — Runtime context integration

- [ ] **10a** — Add optional `world_db` handle to `GameContext`. [3a]
- [ ] **10b** — Open world DB during startup only when `[world].enabled = true`. [10a, 2a]
- [ ] **10c** — Ensure DB-open failure restores terminal before printing an error. Test with injectable failing opener. [10b]
- [ ] **10d** — Document that render functions must not run blocking world queries. [10a]

### Task 11 — Packaging integration

- [ ] **11a** — Teach `fgk package` to include `world/.keep` for world-enabled games. [2a]
- [ ] **11b** — Ensure packaged `run.sh` does not delete or recreate `world/`. [11a]
- [ ] **11c** — Add package smoke test asserting `world/` exists for Murder Motel. [11a]
- [ ] **11d** — Update install docs with writable `world/` permissions and backup notes. [11a]

### Task 12 — Murder Motel shared Room 7

- [ ] **12a** — Add Murder Motel migration for `motel_world_state` key/value table. [4b]
- [ ] **12b** — Record `room_7_opened_at` and opener player id when first player unlocks Room 7. [12a, 5b]
- [ ] **12c** — Show later players that Room 7 was already opened by someone else. [12b]
- [ ] **12d** — Add two-player test: Alice opens Room 7, Bob sees shared evidence. [12c]

### Task 13 — Murder Motel turns/events/leaderboard

- [ ] **13a** — Make clue inspection spend one daily turn. [6d]
- [ ] **13b** — Show remaining turns in the map/status UI. [13a]
- [ ] **13c** — Append event when Room 7 opens and when a major clue is found. [7b, 12b]
- [ ] **13d** — Add lobby bulletin/ledger screen that lists recent events. [7c]
- [ ] **13e** — Increment `investigators` leaderboard when clues are found. [8c]
- [ ] **13f** — Add leaderboard screen reachable from the main menu. [8d]
- [ ] **13g** — Add deterministic test for daily reset restoring Murder Motel clue turns. [6f, 13a]
- [ ] **13h** — Add Murder Motel role/security display proof: sysop/mod/user synthetic contexts show distinct labels/security levels, with copy making clear these are in-game/advisory only. [5f]

### Task 14 — Documentation

- [ ] **14a** — Add `docs/shared-world.md` explaining SQLite file locations, migration policy, backups, and lock recovery. [Task 11]
- [ ] **14b** — Add Murder Motel v2 walkthrough to README: two local users demonstrate shared world. [Task 13]
- [ ] **14c** — Document why v2 intentionally avoids real-time multiplayer. [14a]

- [ ] **Task 15 — Final v2 verification**
      Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc --workspace --no-deps`, `cargo run --example murder_motel` local smoke for two users, and `cargo run -p fgk -- package --out <tmp>`. Quote results in the final iteration. [all prior tasks]

## Acceptance criteria — gate for `<promise>V2_COMPLETE</promise>`

- [ ] All v1 acceptance criteria remain true
- [ ] All Task 1–15 items above are checked
- [ ] Shared-world SQLite DB opens, migrates, and survives relaunch
- [ ] Player registry maps Foglet/local-dev identities to stable player records
- [ ] Role/security helpers expose legacy-compatible metadata from modern Foglet context
- [ ] Daily turn ledger supports spend, insufficient-turn rejection, reset, and carryover
- [ ] Append-only event log powers a Murder Motel bulletin/ledger screen
- [ ] Leaderboard helpers power a Murder Motel investigators leaderboard
- [ ] Murder Motel proves shared Room 7 state across two players
- [ ] Murder Motel proves role/security display for sysop/mod/user without treating it as launch authorization
- [ ] `fgk package` emits a package with writable `world/` directory expectations documented
- [ ] Docs explain backup/permissions and explicitly defer real-time multiplayer
