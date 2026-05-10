# foglet-game-kit-rs Checklist — v3 BBS-Native Async Multiplayer

One unchecked item per iteration. Dependencies in `[brackets]` must be checked off before the dependent item is eligible. This checklist extends `CHECKLIST_v2.md`; do not begin v3 implementation until v2 acceptance criteria are complete.

## Implementation tasks

- [x] **Task 1 — Re-orient on v2 shared-world schema**
      Read `SPEC_v2.md`, `CHECKLIST_v2.md`, and current migrations. Add any needed v3 schema namespace notes to docs before coding. [v2]

### Task 2 — Multiplayer config

- [x] **2a** — Add `[multiplayer]` config with `notices`, `challenges`, `market`, `factions`, `bounties`, and `max_notice_body_chars`. Test: absent config disables all primitives. [Task 1]
- [x] **2b** — Parse `[[factions.seed]]` entries. Test: duplicate slugs rejected. [2a]
- [x] **2c** — Extend Murder Motel `game.toml` with v3 multiplayer config and two seeded agencies. [2a, 2b]

### Task 3 — Notices/mail schema and API

- [x] **3a** — Add `notices` migration. Fields: sender, recipient, kind, subject, body, timestamps, metadata. [Task 1]
- [x] **3b** — Implement `Notice` type and `send_notice`. Test: notice is stored unread. [3a]
- [x] **3c** — Enforce subject/body length limits. Tests: empty subject and overlong body fail clearly. [3b]
- [x] **3d** — Implement `inbox(player_id)` newest-first query. [3b]
- [x] **3e** — Implement idempotent `mark_read`. Test: second read call is a no-op. [3d]
- [x] **3f** — Implement `archive_notice`. Test: archived notices disappear from default inbox. [3e]

### Task 4 — Challenge lifecycle

- [x] **4a** — Add `challenges` migration with challenger, target, kind, stake JSON, state, timestamps, result JSON. [Task 1]
- [x] **4b** — Implement `create_challenge`. Test: starts in `open`. [4a]
- [x] **4c** — Implement `accept_challenge`. Test: `open -> accepted`; accepting expired challenge fails. [4b]
- [x] **4d** — Implement `decline_challenge`. Test: only `open -> declined` is valid. [4b]
- [x] **4e** — Implement `resolve_challenge`. Test: only `accepted -> resolved` is valid and stores result JSON. [4c]
- [x] **4f** — Implement `expire_open_challenges(now)`. Test: only due open challenges expire. [4b]
- [x] **4g** — Add invalid-transition table tests for every state pair. [4c, 4d, 4e, 4f]

### Task 5 — Market listings

- [x] **5a** — Add `market_listings` migration with seller, item key, display name, price, quantity, timestamps, metadata. [Task 1]
- [x] **5b** — Implement `create_listing`. Test: negative price/quantity rejected. [5a]
- [x] **5c** — Implement `active_listings` query sorted deterministically. [5b]
- [x] **5d** — Implement atomic `buy_listing` quantity decrement. Test: quantity decrements. [5c]
- [x] **5e** — Add rollback test when buyer inventory/balance callback fails. [5d]
- [x] **5f** — Append world event on successful purchase. [5d, v2 Task 7]

### Task 6 — Factions and shared goals

- [x] **6a** — Add `factions`, `faction_memberships`, and `shared_goals` migrations. [Task 1]
- [x] **6b** — Seed configured factions idempotently from config. Test: second seed call does not duplicate. [6a, 2b]
- [x] **6c** — Implement `join_faction`. Test: membership row is created. [6b]
- [x] **6d** — Implement `leave_faction`. Test: membership removed or marked inactive per chosen schema. [6c]
- [x] **6e** — Implement `create_shared_goal`. [6a]
- [x] **6f** — Implement atomic `contribute_to_goal`. Test: current amount increments. [6e]
- [x] **6g** — Mark shared goal complete when target is reached and append world event. [6f]

### Task 7 — Bounties/job board

- [x] **7a** — Add `bounties` migration with poster, title, description, reward JSON, state, claimant, timestamps. [Task 1]
- [x] **7b** — Implement `post_bounty`. Test: starts open. [7a]
- [x] **7c** — Implement `claim_bounty`. Test: `open -> claimed`; second claimant rejected. [7b]
- [x] **7d** — Implement `complete_bounty`. Test: `claimed -> completed` and reward payload retained. [7c]
- [x] **7e** — Implement `expire_bounties(now)`. Test: open/claimed due bounties expire according to documented policy. [7b]
- [x] **7f** — Add invalid-transition table tests. [7c, 7d, 7e]

### Task 8 — Player lookup helpers for async screens

- [x] **8a** — Add player search by handle prefix for local game UI. Test: prefix query is case-insensitive if chosen. [v2 Task 5]
- [x] **8b** — Add recent players query for target selection screens. [8a]

### Task 9 — Documentation

- [x] **9a** — Add `docs/async-multiplayer.md` explaining mailbox multiplayer, challenges, markets, factions, and bounties. [Tasks 3-7]
- [x] **9b** — Document player-authored text limits and terminal-display sanitization. [14a]
- [x] **9c** — Explicitly document that v3 still has no real-time multiplayer. [14a]

- [ ] **Task 10 — Final v3 verification**
      Run formatting, clippy, tests, docs, and package smoke. Quote results in the final iteration. [all prior tasks, Tasks 11–16]

- [x] **10a** — Mention v3's BBS-native async multiplayer primitives in `README.md` overview/feature list (completion condition #13). [9a]

## Acceptance criteria — gate for `<promise>V3_COMPLETE</promise>`

- [ ] All v2 acceptance criteria remain true
- [ ] All Task 1–15 items above are checked
- [ ] Notices support send/read/archive with bounded player-authored text
- [ ] Challenges support create/accept/decline/resolve/expire with invalid-transition tests
- [ ] Market listings support atomic buy/sell flows and rollback on failure
- [ ] Factions and shared goals support join/contribute/complete flows
- [ ] Bounties support post/claim/complete/expire flows
- [ ] Documentation explains async BBS-native multiplayer and rejects real-time scope
