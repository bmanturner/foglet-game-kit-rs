# Drop Dead Nebula — Foglet Game-Kit Wishlist

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`, `DROP_DEAD_NEBULA_MVP.md`, `DROP_DEAD_NEBULA_CONTENT_SEED.md`  
Purpose: Identify general-purpose Foglet game-kit primitives that would make Drop Dead Nebula easier to build while remaining useful to other BBS door games.

---

## 1. Wishlist Philosophy

This document is not a request to move Drop Dead Nebula’s game-specific rules into the kit. The kit should stay genre-neutral.

A feature belongs on this wishlist only if it satisfies all three tests:

1. **Reusable:** useful to multiple BBS door games, not just Drop Dead Nebula.
2. **Primitive or adapter-shaped:** provides durable lifecycle, UI shell, transaction helper, or testing affordance without owning game-specific fiction.
3. **Composable:** lets game code supply validation, payloads, pricing, item semantics, event text, and balance rules.

Examples:

- Good kit candidate: generic Contract lifecycle with game-defined objective/reward payloads.
- Bad kit candidate: “First Mercy Run” delivery logic.
- Good kit candidate: deterministic weighted random table helper.
- Bad kit candidate: Red Maw anomaly table.
- Good kit candidate: reusable notice inbox screen.
- Bad kit candidate: Drop Dead Nebula faction dispatch prose.

---

## 2. Priority Labels

### P0 — Blocks or strongly shapes the MVP

Not strictly impossible to build without it, but absence would create early duplicate game code that is obviously reusable.

### P1 — High leverage after MVP

Would materially speed up the living-world or signature slices.

### P2 — Valuable later

Useful for polish, scale, authoring, testing, or advanced systems.

### Defer / Probably Game-Specific

Tempting, but should remain in Drop Dead Nebula until at least two games prove the abstraction.

---

## 3. Wishlist Summary Table

| Priority | Candidate | Why it matters |
| --- | --- | --- |
| P0 | Generic Contract primitive + unified Job Board surface | The MVP needs a delivery Contract; existing Bounties/Challenges should also appear on shared job boards. |
| P0 | Spatial Travel transaction helper | Many games need route validation + turn spend + move + recall + event. |
| P0 | Owner Inventory capacity helper | Cargo capacity is needed immediately; capacity is common across genres. |
| P0 | Reusable Event Log / News screen | The MVP needs readable world memory. |
| P0 | Multi-user local-dev test harness | Async BBS games need fake users early. |
| P1 | Reusable Notice Inbox screen | Notices/mail are core to async play. |
| P1 | Deterministic random table helper | Random events need repeatable tests. |
| P1 | World Tick catch-up summary helper | Low-pop worlds need bounded catch-up and player digests. |
| P1 | Reusable Market screen and transaction adapters | Many games need buy/sell UI around generic market/inventory primitives. |
| P1 | Bounded player text sanitizer | Needed for notices, bounties, corp boards, rumors. |
| P1 | Reusable Leaderboard screen | BBS games benefit from standard scoreboard UI. |
| P2 | Content key validation helper | Prevents broken content packs and save references. |
| P2 | Scripted terminal session test harness | Verifies flows without manual play. |
| P2 | Admin/diagnostic world inspection helpers | Helps sysops recover stuck world state. |
| P2 | Generic relationship/reputation primitive | Useful, but risks overgeneralization. |
| P2 | Data-driven weighted content loaders | Useful after formats stabilize. |

---

## 4. P0 Candidate: Generic Contract Primitive and Unified Job Board Surface

### Problem

The game-kit already has Bounties and Challenges, and those absolutely belong on a Job Board. Drop Dead Nebula’s MVP also needs a broader non-target work item: a delivery Contract that can be accepted, completed, expired, abandoned, and rewarded.

The key distinction:

- **Contract** is a broad work/obligation lifecycle: deliver, survey, escort, supply, repair, transport, inspect.
- **Bounty** is a target-oriented work lifecycle: recover this object, clear this hazard, defeat this actor, scan this anomaly.
- **Challenge** is a contest lifecycle: compete with another player or NPC under defined conditions.
- **Job Board** is the aggregation surface that can display all of them together.

So the wishlist is not “Contract instead of Bounty.” It is:

> Add a generic Contract primitive for non-bounty work, and provide a Job Board UI/adapter that can aggregate Contracts, Bounties, Challenges, Faction Goals, and other game-defined opportunities.

This is not space-specific. Fantasy RPGs, mystery games, town sims, dungeon crawlers, cyberpunk doors, and trading games all need job boards that mix several opportunity types.

### General-Purpose Shape: Contract Primitive

A Contract primitive should own lifecycle invariants, not objective semantics.

Suggested lifecycle:

```text
available -> accepted -> completed
available -> expired
accepted -> abandoned
accepted -> failed
accepted -> expired
```

The kit could provide:

- stable contract id;
- contract kind string;
- issuer owner reference;
- optional acceptor player id;
- state;
- created/accepted/completed/expires timestamps;
- objective payload JSON, game-defined;
- reward payload JSON, game-defined;
- metadata JSON;
- transactional state transitions;
- query available/accepted/completed contracts.

### General-Purpose Shape: Job Board Surface

A Job Board helper should be a UI/query aggregation layer, not a separate competing lifecycle.

It could aggregate:

- Contracts;
- Bounties;
- Challenges;
- Faction/shared goals;
- game-defined opportunities;
- system notices that advertise work, if a game chooses.

The Job Board could provide:

- common list rendering;
- opportunity type labels;
- state labels;
- expiry display;
- reward preview;
- location/issuer preview;
- hotkeys and pagination;
- empty-state text;
- detail modal;
- game-supplied action callbacks.

### Game-Supplied Responsibilities

Game code owns:

- objective validation;
- reward application;
- display text;
- whether multiple players can accept;
- whether completion consumes inventory;
- faction/reputation side effects;
- event/notice wording;
- which opportunity types appear on which Job Board;
- how Bounties, Contracts, Challenges, and Faction Goals are prioritized or grouped.

### Drop Dead Nebula Use

MVP:

- First Mercy Run delivery Contract appears on Ash Coil’s Job Board.

Later:

- freight Contracts;
- salvage Bounties;
- smuggling Contracts;
- survey Contracts;
- route-clearing Bounties;
- player/NPC Challenges;
- faction shared goals;
- station emergency requests.

Example board:

```text
JOBS AT MERCY RELAY

[Contract] Deliver 12 Med Gel to Mercy Relay
[Bounty]   Recover black box from Blue Blind
[Bounty]   Clear pirate buoy near Red Maw
[Challenge] Beat Brass Jory’s cargo run
[Faction]  Contribute Reactor Coolant to relay repair
```

### Other Game Uses

- RPG tavern boards mixing quests, monster bounties, and guild challenges.
- Noir case boards mixing investigations, warrants, and rival detective challenges.
- Dungeon boards mixing fetch quests, boss bounties, and party goals.
- Town-sim errands mixing supply requests, civic projects, and resident favors.
- Survival games mixing supply contracts, hazard-clearing bounties, and faction calls.

### Acceptance Criteria for Kit Feature

Contract primitive:

- Can create available Contracts with game-defined payloads.
- Can accept a Contract transactionally.
- Can complete a Contract exactly once.
- Invalid transitions are rejected.
- Expired Contracts cannot be accepted.
- Query APIs support available jobs and a player’s accepted jobs.

Job Board surface:

- Can display Contracts and existing Bounties in one list.
- Can include Challenges and Faction Goals without forcing them into Contract semantics.
- Clearly labels opportunity type.
- Supports keyboard navigation and detail views.
- Lets game code provide accept/claim/open/contribute callbacks per opportunity type.
- Does not prescribe objective, reward, or completion semantics.

---

## 5. P0 Candidate: Spatial Travel Transaction Helper

### Problem

Many games using the v4 spatial graph will need the same transaction shape:

1. inspect current Presence;
2. validate Route;
3. check game-defined requirements;
4. spend turns or other cost;
5. move Presence;
6. touch Recall;
7. append Event;
8. return a structured arrival result.

If each game writes this from scratch, subtle bugs will appear around partial moves, failed turn spends, and stale recall.

### General-Purpose Shape

A composable travel helper should not decide route rules. It should coordinate a transaction and call game hooks.

Possible capabilities:

- require current player Presence;
- verify destination is reachable by a selected Route;
- support directed Routes;
- accept game-supplied validation callback;
- accept game-supplied cost callback;
- optionally spend turns;
- move player Presence;
- optionally touch Place Recall;
- optionally append Event;
- return structured success/failure reason.

### Game-Supplied Responsibilities

Game code owns:

- route requirement logic;
- fuel costs;
- hazard rolls;
- faction restrictions;
- event text;
- whether failed travel consumes resources;
- arrival screen behavior.

### Drop Dead Nebula Use

- MVP Ash Coil -> Mercy Relay travel.
- Later hidden routes, one-way currents, smuggler paths, dangerous Red Maw routes.

### Other Game Uses

- Dungeon room movement.
- Town district navigation.
- Overworld travel.
- Social-club room movement.
- Mystery location graph.

### Acceptance Criteria for Kit Feature

- Successful travel updates Presence atomically.
- Failed validation does not move Presence.
- Failed cost spend does not move Presence.
- Recall touch can be enabled/disabled.
- Event append can be enabled/disabled.
- Error reasons are game-displayable.

---

## 6. P0 Candidate: Owner Inventory Capacity Helper

### Problem

The v4 inventory primitive intentionally avoids game-specific weight, volume, and item semantics. That is correct. But many games still need a reusable capacity-check pattern around atomic transfers.

Without a helper, every game will reimplement:

- current used capacity;
- item volume lookup;
- proposed transfer capacity validation;
- disabled-choice reason;
- rollback on insufficient space.

### General-Purpose Shape

A capacity helper should be callback-driven.

Potential capabilities:

- compute used capacity for an owner;
- support game-supplied item weight/volume function;
- support game-supplied owner capacity function;
- validate proposed incoming transfer;
- integrate with inventory transfer transaction;
- return clear insufficient-capacity errors.

### Game-Supplied Responsibilities

Game code owns:

- what “capacity” means;
- item volume/weight/slots;
- owner capacity;
- stacking rules beyond exact inventory metadata;
- UI wording.

### Drop Dead Nebula Use

- Rustbucket Mule cargo capacity in MVP.
- Later ship modules, containers, station warehouses, outpost depots.

### Other Game Uses

- RPG inventory weight.
- Dungeon backpack slots.
- Town warehouse limits.
- Colony stockpile capacity.
- Equipment loadouts.

### Acceptance Criteria for Kit Feature

- Can compute capacity usage from owner inventory slots.
- Can reject incoming transfer before mutation.
- Can participate in an atomic transfer.
- Distinguishes insufficient source quantity from insufficient destination capacity.
- Does not prescribe item schema.

---

## 7. P0 Candidate: Reusable Event Log / News Screen

### Problem

The kit provides event-log primitives, but games need a standard way to show recent Events in a terminal-friendly format.

Drop Dead Nebula’s MVP needs a Log screen immediately. Many BBS games will too.

### General-Purpose Shape

A reusable screen/widget should support:

- recent global Events;
- per-player Events;
- scoped Events if available;
- pagination;
- empty state;
- timestamp display options;
- compact 80x24 layout;
- optional filters by kind/scope;
- safe text wrapping.

### Game-Supplied Responsibilities

Game code owns:

- event message content;
- which events are public/private;
- event categories;
- styling/theme choices.

### Drop Dead Nebula Use

- MVP completion log.
- Later world news, NPC actions, faction progress, bounty completions.

### Other Game Uses

- Town bulletin board.
- Dungeon death log.
- Mystery case ledger.
- Sports league results.
- Kingdom chronicle.

### Acceptance Criteria for Kit Feature

- Renders recent Events in 80x24.
- Handles empty event log gracefully.
- Supports keyboard navigation/pagination.
- Does not print to stdout outside the TUI.
- Accepts game-defined title/help text.

---

## 8. P0 Candidate: Multi-User Local-Dev Test Harness

### Problem

Async BBS games need tests involving multiple fake players affecting one shared world. The game-kit already supports local-dev context patterns, but Drop Dead Nebula will need this constantly.

### General-Purpose Shape

A test harness could provide:

- fake Foglet contexts for named users;
- distinct user ids/handles/roles;
- shared temp world DB setup;
- per-user save roots;
- helpers to switch active user;
- scripted session boundary helpers;
- assertions for notices/events/leaderboards by user.

### Game-Supplied Responsibilities

Game code owns:

- gameplay actions;
- expected state assertions;
- content seed;
- scripted input if UI-level testing.

### Drop Dead Nebula Use

- Ensure Alice’s trade affects Bob’s Market.
- Ensure player-specific Recall differs.
- Ensure notices target correct Captains.
- Ensure contracts cannot be double-completed incorrectly.

### Other Game Uses

- Any async multiplayer BBS game.
- Mail/challenge/faction tests.
- Leaderboard tests.
- Shared-world regression tests.

### Acceptance Criteria for Kit Feature

- Can create at least two distinct fake users.
- Each fake user gets isolated per-user save state.
- Fake users share the same world DB.
- Tests can inspect world effects after alternating sessions.
- Roles can be varied for sysop/mod/user behavior.

---

## 9. P1 Candidate: Reusable Notice Inbox Screen

### Problem

The kit’s notice/mail primitive is broadly useful, but every game will need inbox UI: unread list, read detail, archive, and empty states.

### General-Purpose Shape

A reusable inbox screen/widget should provide:

- list notices for current player;
- unread/read indicators;
- archive support;
- notice detail modal;
- hotkeys;
- pagination;
- expiry visibility if relevant;
- safe text wrapping;
- configurable title and empty-state copy.

### Game-Supplied Responsibilities

Game code owns:

- notice kind meanings;
- notice text;
- actions triggered from notice metadata;
- style/theme.

### Drop Dead Nebula Use

- Daily intel.
- Contract updates.
- NPC messages.
- Trap reports.
- Player mail.

### Other Game Uses

- RPG letters.
- Mystery case updates.
- Faction dispatches.
- Town announcements.
- Async challenge results.

### Acceptance Criteria for Kit Feature

- Can render inbox at 80x24.
- Can open notice detail.
- Can mark read idempotently.
- Can archive notice.
- Handles long bounded text safely.

---

## 10. P1 Candidate: Deterministic Random Table Helper

### Problem

Random events are central to low-pop solo play, but randomness must be testable and reproducible.

### General-Purpose Shape

A random table helper could provide:

- weighted entries;
- deterministic seeded rolls;
- prerequisite filters;
- tags/categories;
- roll result metadata;
- no-result handling;
- test helpers for distribution sanity.

### Game-Supplied Responsibilities

Game code owns:

- event tables;
- weights;
- prerequisites;
- outcome effects;
- text;
- balance.

### Drop Dead Nebula Use

- Travel events.
- Dockside events.
- Salvage events.
- Smuggling complications.
- Faction flashpoints.

### Other Game Uses

- Encounter tables.
- Loot tables.
- NPC behavior tables.
- Weather tables.
- Procedural rumors.

### Acceptance Criteria for Kit Feature

- Same seed and table produce same result.
- Filters remove ineligible entries before roll.
- Empty filtered table returns clear no-result.
- Results expose selected key and payload.
- Tests can assert deterministic outcomes.

---

## 11. P1 Candidate: World Tick Catch-Up Summary Helper

### Problem

Low-pop BBS games may go idle for days. World ticks need bounded catch-up and player-readable summaries. Otherwise a returning player may trigger too much work or receive an unreadable dump.

### General-Purpose Shape

The helper could provide:

- catch-up policy config;
- max tasks per call;
- max simulated periods per call;
- summary aggregation;
- tick result categories;
- player-facing digest builder;
- distinction between full simulation and summarized catch-up.

### Game-Supplied Responsibilities

Game code owns:

- tick callbacks;
- domain summaries;
- what can be compressed;
- balance consequences.

### Drop Dead Nebula Use

- Market drift digest.
- Contract expiry summary.
- NPC action summary.
- Faction progress summary.
- Low-pop reactivation.

### Other Game Uses

- Town simulation.
- Farm/colony growth.
- Dungeon restock.
- League schedule.
- Kingdom turns.

### Acceptance Criteria for Kit Feature

- Catch-up is bounded.
- Failed tick leaves retry-safe state.
- Summary records can be returned without prescribing content.
- Games can present digest after login.
- Long inactivity does not freeze the player session.

---

## 12. P1 Candidate: Reusable Market Screen and Transaction Adapters

### Problem

The kit has market and inventory primitives, but every game with buy/sell flows needs the same terminal UI and transaction affordances.

### General-Purpose Shape

A reusable market adapter could provide:

- list buyable/sellable listings;
- show price, quantity, owned amount;
- support hotkeys and selection;
- disabled reasons for insufficient funds, stock, or capacity;
- confirmation for large/rare transactions;
- transaction wrapper hooks;
- result feedback line.

### Game-Supplied Responsibilities

Game code owns:

- item display metadata;
- price formula;
- currency model;
- capacity rules;
- legality/faction restrictions;
- side effects.

### Drop Dead Nebula Use

- MVP Ash Coil/Mercy Relay Markets.
- Later black markets, faction depots, salvage auctions, player listings.

### Other Game Uses

- RPG shops.
- Equipment vendors.
- Auction houses.
- Town supply depots.
- Crafting exchanges.

### Acceptance Criteria for Kit Feature

- Renders market list at 80x24.
- Displays buy/sell affordances.
- Provides disabled-choice reasons.
- Supports game-defined transaction callbacks.
- Does not prescribe currency or item schema.

---

## 13. P1 Candidate: Bounded Player Text Sanitizer

### Problem

The specs require bounded, terminal-safe player-authored text. Multiple systems need this: notices, bounties, corp bulletins, rumors, outpost names, ship epitaphs.

### General-Purpose Shape

A sanitizer could provide:

- character limit enforcement;
- line limit enforcement;
- control-character stripping;
- ANSI escape stripping;
- whitespace normalization;
- optional printable ASCII mode;
- safe wrapping;
- validation errors suitable for UI;
- test vectors.

### Game-Supplied Responsibilities

Game code owns:

- exact limits by field;
- moderation policy;
- profanity/block lists if any;
- whether Unicode is allowed;
- storage destination.

### Drop Dead Nebula Use

- Player mail.
- Bounty blurbs.
- Corp bulletins.
- Outpost mottos.
- Ship memorials.

### Other Game Uses

- Any player-authored BBS text.
- Guestbooks.
- Notices.
- Character bios.
- Trade listings.

### Acceptance Criteria for Kit Feature

- Rejects or cleans terminal control sequences.
- Enforces max length deterministically.
- Returns display-safe text.
- Produces user-facing validation errors.
- Includes tests for hostile input.

---

## 14. P1 Candidate: Reusable Leaderboard Screen

### Problem

The kit exposes leaderboard helpers, but BBS players expect readable scoreboards. Most games will need similar UI.

### General-Purpose Shape

A leaderboard screen/widget could provide:

- board selection;
- top N display;
- current player rank;
- pagination;
- empty states;
- tie handling display;
- optional season label;
- compact 80x24 layout.

### Game-Supplied Responsibilities

Game code owns:

- board names;
- score meanings;
- score formatting;
- reset/season policy;
- rewards/titles.

### Drop Dead Nebula Use

- Richest Captains.
- Contracts Completed.
- Salvage Value.
- Faction Contribution.
- Most Wanted.

### Other Game Uses

- RPG levels.
- Arena wins.
- Puzzle scores.
- Town contributions.
- Dungeon clears.

### Acceptance Criteria for Kit Feature

- Displays top scores.
- Shows current player rank when present.
- Handles empty board.
- Supports multiple boards.
- Supports keyboard navigation.

---

## 15. P2 Candidate: Content Key Validation Helper

### Problem

Content-heavy games need stable keys. Broken keys cause missing items, bad Routes, invalid Contract objectives, and save incompatibilities.

### General-Purpose Shape

A validation helper could provide:

- stable key format validation;
- duplicate detection;
- reference validation across content sets;
- missing display name warnings;
- orphan content warnings;
- content summary report.

### Game-Supplied Responsibilities

Game code owns:

- content formats;
- allowed key namespaces;
- domain-specific references;
- severity policy.

### Drop Dead Nebula Use

- Places, Routes, Commodities, Contracts, Factions, NPCs, Event Tables.

### Other Game Uses

- Any data-driven game using stable keys.

### Acceptance Criteria for Kit Feature

- Can validate key format and uniqueness.
- Can run reference checks supplied by game code.
- Produces readable diagnostics.
- Does not require a specific content format.

---

## 16. P2 Candidate: Scripted Terminal Session Test Harness

### Problem

BBS door games need confidence that prompt flows work without manual terminal play.

### General-Purpose Shape

A harness could provide:

- scripted input sequences;
- frame capture or text extraction;
- assertions on visible text;
- terminal size configuration;
- session interruption simulation;
- quit-path verification.

### Game-Supplied Responsibilities

Game code owns:

- script flows;
- expected text;
- seeded world state.

### Drop Dead Nebula Use

- MVP manual playthrough automation.
- Market buy/sell flow.
- Travel flow.
- Contract completion flow.

### Other Game Uses

- Any terminal door game with prompt/UI flows.

### Acceptance Criteria for Kit Feature

- Can run a screen flow with scripted inputs.
- Can assert visible text.
- Can test 80x24 baseline.
- Can verify quit path completes cleanly.

---

## 17. P2 Candidate: Admin / Diagnostic World Inspection Helpers

### Problem

Shared-world BBS games can develop stuck Contracts, bad stock, trapped players, or failed ticks. Sysops need safe introspection and repair affordances.

### General-Purpose Shape

Helpers could provide:

- world summary report;
- player/captain lookup;
- current Presence listing;
- recent failed ticks;
- stuck lifecycle rows;
- dry-run repair hooks;
- audit Event append pattern.

### Game-Supplied Responsibilities

Game code owns:

- repair semantics;
- admin authorization mapping;
- domain-specific reports;
- irreversible action confirmation.

### Drop Dead Nebula Use

- Rescue stuck Captain.
- Inspect Market stock.
- Close broken Contract.
- Force tick summary.

### Other Game Uses

- Any shared-world door with durable state.

### Acceptance Criteria for Kit Feature

- Reports core kit state safely.
- Does not mutate without explicit game callback.
- Supports audit-friendly output.
- Can be used from CLI or admin screen.

---

## 18. P2 Candidate: Generic Relationship / Reputation Primitive

### Problem

Many games need actor-to-actor or actor-to-faction standing. The current faction membership/shared goal primitive may not cover NPC memories, local station reputation, or bilateral relationships.

### General-Purpose Shape

A relationship primitive could provide:

- subject owner;
- target owner;
- kind string;
- numeric score;
- flags/tags;
- last changed timestamp;
- metadata JSON;
- bounded adjustments;
- query by subject/target/kind.

### Game-Supplied Responsibilities

Game code owns:

- score meanings;
- thresholds;
- consequences;
- decay;
- display text.

### Drop Dead Nebula Use

- NPC grudges/favors.
- Station trust.
- Faction standing extensions.
- Pirate fear reputation.

### Other Game Uses

- RPG NPC affection.
- Town reputation.
- Guild standing.
- Rivalries.
- Diplomacy.

### Caution

This is easy to overgeneralize. It should probably wait until Drop Dead Nebula or another game proves the need beyond faction standing.

---

## 19. P2 Candidate: Data-Driven Weighted Content Loaders

### Problem

Games will eventually want to load event tables, commodities, places, dialogs, and contracts from structured data. But premature content formats can become rigid.

### General-Purpose Shape

Potentially useful later:

- format-agnostic validation helpers;
- typed loader examples;
- clear error reporting;
- test fixture loading;
- schema documentation conventions.

### Game-Supplied Responsibilities

Game code owns:

- actual content schema;
- domain semantics;
- migration policy.

### Drop Dead Nebula Use

- Event tables.
- Commodity catalog.
- Seed world.
- NPC definitions.
- Contract templates.

### Other Game Uses

- Any data-rich BBS game.

### Caution

Do not add a scripting VM. Keep data as data; effects stay in game code.

---

## 20. Probably Game-Specific for Now

The following are important to Drop Dead Nebula but should not become kit primitives yet.

### 20.1 Space Commodity Pricing Engine

Why not kit:

- pricing curves are game balance and genre-specific.

Possible kit alternative:

- market pricing examples, not a universal engine.

### 20.2 Ship/Hull/Module System

Why not kit:

- deeply genre-specific.

Possible kit alternative:

- inventory capacity helpers and equipment UI patterns after more examples.

### 20.3 Combat Engine

Why not kit:

- combat varies wildly by genre.

Possible kit alternative:

- prompt/choice/result UI helpers.

### 20.4 NPC AI System

Why not kit:

- behavior and simulation are game-specific.

Possible kit alternative:

- world tick scaffolding, random table helper, relationship primitive.

### 20.5 Faction War Model

Why not kit:

- faction conflict rules are game-specific.

Possible kit alternative:

- existing faction/shared goal primitives plus event/log/notice helpers.

### 20.6 Derelict Boarding / Dungeon Crawl Engine

Why not kit:

- local maps exist, but room hazards, loot, and exploration rules are game-specific.

Possible kit alternative:

- map/prompt helpers and deterministic random table support.

### 20.7 Outpost / Colony Production System

Why not kit:

- production chains and colony needs are game balance.

Possible kit alternative:

- owner-keyed inventory, world ticks, capacity helpers.

---

## 21. Suggested Roadmap for Kit Requests

### 21.1 Before or During Drop Dead Nebula MVP

Highest leverage:

1. Generic Contract primitive + unified Job Board surface.
2. Spatial Travel transaction helper.
3. Owner Inventory capacity helper.
4. Reusable Event Log / News screen.
5. Multi-user local-dev test harness.

These align with the first trade-route slice.

### 21.2 Immediately After MVP

Highest leverage:

1. Reusable Notice Inbox screen.
2. Reusable Market screen and transaction adapters.
3. Deterministic Random Table helper.
4. World Tick catch-up summary helper.
5. Bounded Player Text sanitizer.
6. Reusable Leaderboard screen.

These align with the living-world slice.

### 21.3 Later

Useful once content volume grows:

1. Content Key validation helper.
2. Scripted Terminal Session test harness.
3. Admin / Diagnostic world inspection helpers.
4. Generic Relationship / Reputation primitive.
5. Data-driven content loader conventions.

---

## 22. How to Decide Whether to Promote Game Code into the Kit

When Drop Dead Nebula implements a helper locally, consider promoting it to the kit only if:

- it has no Drop Dead Nebula terms in the API;
- it accepts game-defined payloads/callbacks;
- it improves at least two different systems;
- it has tests independent of Drop Dead Nebula content;
- it can be documented with at least two non-space examples;
- it does not require a scripting VM;
- it does not prescribe economy, combat, item, or faction balance.

Examples:

- A local “delivery contract” should not be promoted.
- A generic “contract lifecycle with objective payload” might be promoted.
- A local “Med Gel market price formula” should not be promoted.
- A generic “market screen showing listings and disabled reasons” might be promoted.

---

## 23. Open Questions

1. Should generic Contracts be part of the kit, or are Bounties broad enough if named differently?
2. Should the kit include reusable screens for every durable primitive, or only data APIs?
3. Should transaction helpers live in the kit if they compose several optional primitives?
4. How much UI theming should reusable screens expose?
5. Should capacity helpers support multiple capacity dimensions, or only one game-supplied scalar?
6. Should random table helpers live in the core crate or as an optional utility module?
7. Should multi-user test harnesses be first-class kit APIs or test-only utilities?
8. Should bounded text sanitization be mandatory for notice/bounty APIs or exposed as an opt-in helper?
9. Should admin diagnostics be CLI-first or screen-first?
10. Should Drop Dead Nebula implement first and promote later, or should P0 kit requests be built before the game MVP?

---

## 24. Recommendation

Build the Drop Dead Nebula MVP without waiting for all wishlist items, but treat these as early extraction candidates:

1. **Contract primitive + unified Job Board surface** — likely worth adding before or during MVP because non-bounty jobs are generic, and existing Bounties/Challenges should still appear on the same board.
2. **Spatial Travel transaction helper** — likely worth adding during MVP because it composes existing v4/v2 primitives safely.
3. **Inventory Capacity helper** — likely worth adding during MVP because cargo capacity is immediate and broadly reusable.
4. **Multi-user local-dev test harness** — worth adding early if async/shared-world tests become painful.

Everything else can emerge from game code and be promoted once the pattern repeats.

The key discipline: **do not put Drop Dead Nebula’s balance into the kit. Put reusable BBS door-game workflows into the kit.**
