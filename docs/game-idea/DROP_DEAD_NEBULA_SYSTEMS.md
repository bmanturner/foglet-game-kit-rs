# Drop Dead Nebula — Systems Inventory

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`  
Purpose: Turn the GDD into a catalog of buildable game systems layered on top of `foglet-game-kit-rs`.

---

## 1. How to Read This Document

This is not an implementation plan yet. It is a systems inventory: what the game needs, what each system owns, which Foglet game-kit primitives it should use, and how systems depend on each other.

Each system includes:

- **Purpose** — why the system exists for players.
- **Player-facing behaviors** — what the player can see or do.
- **State owned** — durable or transient data the system controls.
- **Foglet kit dependencies** — existing kit primitives this system should lean on.
- **Game dependencies** — other Drop Dead Nebula systems required.
- **MVP shape** — smallest useful version.
- **Later ambition** — dream-big version.
- **Open questions** — design or implementation choices to resolve later.

---

## 2. System Layering Overview

### 2.1 Platform Layer

Provided by Foglet and `foglet-game-kit-rs`:

- external PTY launch;
- terminal guard and restoration;
- input normalization;
- screen stack;
- prompt/dialog/modal helpers;
- per-user saves;
- shared SQLite world DB;
- player registry;
- daily turns;
- event log;
- leaderboards;
- notices;
- challenges;
- market primitives;
- factions/shared goals;
- bounties;
- spatial graph;
- presence;
- place recall;
- owner-keyed inventory;
- world ticks.

### 2.2 Game Foundation Layer

Drop Dead Nebula-specific foundation:

1. game configuration;
2. content key registry;
3. captain profile;
4. ship model;
5. commodity/item catalog;
6. world seed/bootstrap;
7. UI shell/dashboard;
8. transaction conventions;
9. deterministic random helpers.

### 2.3 Core Play Layer

Systems that create the primary loop:

1. turns/action economy;
2. travel/routes;
3. markets/trading;
4. cargo/inventory;
5. contracts/jobs;
6. random events;
7. notices/intel packet;
8. event log/news;
9. save/resume.

### 2.4 Living World Layer

Systems that keep the world alive:

1. world ticks;
2. NPC captains;
3. market drift;
4. faction influence;
5. route hazards;
6. contract generation/expiry;
7. rumor generation;
8. leaderboard updates.

### 2.5 Advanced Play Layer

Ambitious expansion systems:

1. salvage/derelict boarding;
2. combat;
3. smuggling/heat/customs;
4. bounties;
5. factions/shared goals;
6. async challenges;
7. traps/mines/drones;
8. outposts/colonies;
9. corporations/charters;
10. season/campaign phases.

---

## 3. System Dependency Map

### 3.1 Recommended Build Order

1. **Content keys and config**
2. **Captain/player identity**
3. **World bootstrap with Places and Routes**
4. **Ship and cargo basics**
5. **Turns and travel**
6. **Station dashboard and market basics**
7. **Contracts**
8. **Event log and notices**
9. **World tick skeleton**
10. **NPC market/background activity**
11. **Random events**
12. **Salvage or combat, choose one as first signature feature**
13. **Factions and bounties**
14. **Outposts, traps, async challenges, corporations**

### 3.2 MVP Critical Path

A true MVP requires only:

- captain identity;
- one ship;
- daily turns;
- small spatial graph;
- travel;
- station services;
- cargo inventory;
- simple commodities;
- basic market;
- one contract type;
- event log;
- save/quit/resume.

Everything else can be added after the first playable loop works.

---

## 4. System: Game Configuration

### Purpose

Centralize sysop/game constants and content paths so behavior is tunable without scattering numbers through code.

### Player-Facing Behaviors

- Difficulty presets can affect turns, market volatility, and failure severity.
- World name/season label appears in UI.
- Optional sysop configuration can produce different BBS vibes.

### State Owned

Configuration values:

- world seed;
- season name;
- daily turns;
- reserve turn cap;
- starting credits;
- starting ship;
- market volatility;
- NPC activity level;
- low-population boost thresholds;
- terminal minimum size;
- content file paths.

### Foglet Kit Dependencies

- `GameConfig` / `assets/game.toml` style config loading.
- Manifest/package config.
- World DB path config.

### Game Dependencies

None. This is foundational.

### MVP Shape

Hardcode or load:

- title;
- slug;
- min terminal size;
- starting station;
- daily turns;
- starting credits;
- small world seed/content file.

### Later Ambition

- sysop-tunable seasons;
- generated-world parameters;
- low/high population scaling;
- faction enable flags;
- content module loading;
- balance profile presets.

### Open Questions

1. Should world generation be configured in TOML or separate seed JSON/TOML?
2. Should sysop overrides be part of the game or deferred to kit-level support?
3. Should difficulty be per-world, per-player, or fixed?

---

## 5. System: Content Key Registry

### Purpose

Give every authored concept a stable identifier for saves, DB records, tests, and content references.

### Player-Facing Behaviors

Invisible directly, but prevents broken saves and inconsistent content.

### State Owned

Stable keys for:

- places;
- routes;
- commodities;
- ship hulls;
- modules;
- factions;
- NPCs;
- contracts;
- random event tables;
- dialog nodes.

### Foglet Kit Dependencies

- Place keys.
- Faction slugs.
- Item keys in inventory slots.
- Bounty/challenge kind strings.

### Game Dependencies

- configuration;
- all content systems.

### MVP Shape

Define a small static list:

- 5–8 place keys;
- 3 commodity keys;
- 1 ship hull key;
- 1 contract kind;
- 1 event table.

### Later Ambition

- content validation command;
- orphan key detection;
- migration helpers for renamed content;
- schema for content packs.

### Open Questions

1. Should keys be plain strings or typed newtypes?
2. Should content validation live in the game or become a kit feature?

---

## 6. System: Captain and Player Identity

### Purpose

Map Foglet callers to in-game captains and preserve progression.

### Player-Facing Behaviors

- Create/resume captain.
- Show captain name, handle, reputation, faction standing, heat, and career stats.
- Recognize sysop/mod roles in flavor if desired.

### State Owned

Shared world state:

- player/captain id;
- Foglet user id/handle mapping;
- display name;
- created/last seen timestamps;
- current ship id;
- credits;
- heat;
- broad reputation stats;
- faction standing references.

Per-user save state:

- tutorial dismissed;
- UI preferences;
- private story flags;
- local notes.

### Foglet Kit Dependencies

- Foglet context loader.
- Player registry helper.
- Role/security-level normalization.
- Per-user save / SaveSlot.

### Game Dependencies

- game config;
- save/resume;
- ship system.

### MVP Shape

- Auto-create captain from Foglet handle or local-dev user.
- Assign starting credits and starter ship.
- Store last login/current place.

### Later Ambition

- captain creation prompt;
- portraits as ASCII badges;
- titles/nicknames;
- career track summaries;
- legacy across seasons;
- public captain profile screen.

### Open Questions

1. Is captain name always Foglet username, or player-chosen? Player-chosen.
2. Which captain fields are shared DB vs per-user save? Probably mostly shared DB. per-user save is more for preferences and tutorial flags, etc. Other user's can see Captain information, so should be shared.
3. Should users be allowed multiple captains? No.

---

## 7. System: Save and Resume

### Purpose

Guarantee short BBS sessions can quit safely and resume cleanly.

### Player-Facing Behaviors

- Quit from any safe screen.
- Resume at last location.
- Preserve private settings and current state.
- No terminal corruption on crash/quit.

### State Owned

Per-user save:

- UI state;
- tutorial flags;
- local preferences;
- potentially active screen context if needed;
- private story state.

Shared DB:

- canonical captain, ship, cargo, turns, and world state.

### Foglet Kit Dependencies

- Terminal guard.
- Save manager.
- SaveSlot.
- Save-on-quit handler.

### Game Dependencies

- captain identity;
- ship;
- travel/presence.

### MVP Shape

- Save tutorial/settings and resume captain from DB.
- Clean quit from dashboard.

### Later Ambition

- interrupted-session recovery;
- “safe return to station” policy after disconnect;
- crash report after terminal restoration;
- autosave checkpoints after major transactions.

### Open Questions

1. Should cargo/ship be fully shared DB canonical from day one?
2. Should active local derelict map state be resumable mid-boarding?

---

## 8. System: UI Shell and Dashboard

### Purpose

Provide the player’s main command center and unify navigation between systems.

### Player-Facing Behaviors

- See current captain/ship/location/turns/alerts.
- Navigate by hotkeys.
- View disabled options with reasons.
- Access travel, market, contracts, ship, cargo, notices, log, faction, recall, help, quit.

### State Owned

Mostly transient UI state:

- selected menu index;
- last opened tab;
- feedback messages;
- modal state.

### Foglet Kit Dependencies

- Screen stack.
- Prompt primitives.
- Modal helpers.
- DialogScreen.
- Input normalization.

### Game Dependencies

- captain;
- ship;
- turns;
- presence;
- notices;
- market;
- contracts.

### MVP Shape

Dashboard with:

- captain;
- ship;
- turns;
- current station;
- credits;
- cargo summary;
- hotkeys for Travel, Market, Jobs, Cargo, Log, Quit.

### Later Ambition

- alert prioritization;
- local signal feed;
- customizable dashboard panels;
- color themes while preserving monochrome readability;
- compact/mobile terminal mode.

### Open Questions

1. Should dashboard be station-only, or available while in space?
2. How much information fits comfortably in 80x24?

---

## 9. System: Help, Tutorial, and Onboarding

### Purpose

Make a complex BBS game learnable without external documentation.

### Player-Facing Behaviors

- First-run tutorial.
- Contextual help.
- “What should I do today?” suggestions.
- Glossary-like in-game help for terms.

### State Owned

Per-user save:

- tutorial progress;
- dismissed hints;
- first completion flags.

### Foglet Kit Dependencies

- Text blocks.
- Prompt flows.
- Any-key pauses.
- Per-user save.

### Game Dependencies

- UI shell;
- captain;
- first MVP systems.

### MVP Shape

- One first-run message.
- Help screen listing basic controls and loop.

### Later Ambition

- adaptive mentor NPC;
- tutorial contracts;
- glossary browser;
- recommended route hints;
- low-pop solo guidance.

### Open Questions

1. Should tutorial be diegetic via a relay AI?
2. Should experienced BBS players be able to skip all onboarding immediately?

---

## 10. System: Spatial World Bootstrap

### Purpose

Create the initial navigable nebula.

### Player-Facing Behaviors

- Travel between stations/sectors.
- Discover routes and places.
- Learn regional identity.

### State Owned

Shared DB:

- places;
- routes;
- region metadata;
- initial station services;
- initial route hazards;
- initial market stock;
- initial NPC/faction presence.

### Foglet Kit Dependencies

- Spatial places.
- Routes.
- Migrations/bootstrap helpers.

### Game Dependencies

- content keys;
- configuration;
- market;
- travel;
- station services.

### MVP Shape

Seed:

- 5–8 Places;
- 8–12 directed Routes;
- 2 market stations;
- 1 safe start station;
- 1 risky route;
- 1 dead-end or one-way route.

### Later Ambition

- deterministic Big Bang-style generator;
- authored landmarks plus generated filler;
- hidden routes;
- faction regions;
- route stability;
- world season seed export/import.

### Open Questions

1. Authored first world or generated first world?
2. Should world bootstrap be a game CLI command or part of first run?
3. How many places are ideal for the first public alpha?

---

## 11. System: Presence and Location

### Purpose

Track where captains and NPCs are in the nebula.

### Player-Facing Behaviors

- Show current location.
- Limit services to current place.
- Show local actors if desired.
- Support “last seen” flavor.

### State Owned

Shared DB:

- player presence;
- NPC presence;
- entered_at;
- optional metadata.

### Foglet Kit Dependencies

- presence primitive.
- players_at query.

### Game Dependencies

- captain;
- spatial world;
- travel.

### MVP Shape

- Player has exactly one current Place.
- Moving updates presence transactionally.

### Later Ambition

- local roster;
- stealth/hidden presence;
- last-seen intel;
- faction patrol presence;
- NPC proximity events.

### Open Questions

1. Should offline players be “present” and targetable?
2. Should safe stations hide exact player presence for privacy/game balance?

---

## 12. System: Place Recall and Star Chart

### Purpose

Give players persistent, imperfect memory of the nebula.

### Player-Facing Behaviors

- View known places/routes.
- See last-known market/hazard snapshots.
- Notice stale intel.
- Decide whether to scan or risk old data.

### State Owned

Shared DB or kit recall:

- player/place recall;
- first seen;
- last seen;
- snapshot JSON;
- tags/notes if implemented.

### Foglet Kit Dependencies

- place recall primitive.

### Game Dependencies

- spatial world;
- travel;
- market;
- route hazards;
- scan actions.

### MVP Shape

- Touch recall when visiting a Place.
- Star chart lists visited Places and outbound known Routes.

### Later Ambition

- stale market snapshots;
- player notes;
- bought intel;
- hidden route discovery;
- false rumor overlay;
- scan quality levels.

### Open Questions

1. Is recall stored per player in shared DB only, or partly in per-user save?
2. Should old market prices be exact or approximate?

---

## 13. System: Turns and Action Economy

### Purpose

Pace play and create meaningful decisions.

### Player-Facing Behaviors

- See daily turns remaining.
- Spend turns on movement, scans, salvage, combat, and special actions.
- Receive daily reset.
- Maybe carry limited reserve turns.

### State Owned

Shared DB:

- daily allowance;
- spent today;
- reserve;
- last reset date;
- transaction history if useful.

### Foglet Kit Dependencies

- turn ledger.
- Date-provider injection for tests.

### Game Dependencies

- captain;
- travel;
- contracts;
- world tick.

### MVP Shape

- 30 daily turns.
- Moving costs 1 turn.
- Insufficient turns disables travel.

### Later Ambition

- reserve turns;
- emergency turns;
- ship/module efficiency;
- faction perks;
- turn refunds on failed transactions;
- action categories.

### Open Questions

1. Does trading cost turns?
2. Should docking/undocking cost turns?
3. How generous should reserve turns be for casual callers?

---

## 14. System: Travel and Route Resolution

### Purpose

Move captains through the spatial graph and generate risk/reward.

### Player-Facing Behaviors

- Choose outbound route.
- See cost, known hazards, and requirements.
- Spend turns/fuel.
- Trigger encounters.
- Arrive or fail with consequences.

### State Owned

Shared DB:

- presence updates;
- route state/hazards;
- travel events;
- event log entries.

Transient:

- route choice prompt;
- encounter modal.

### Foglet Kit Dependencies

- spatial routes;
- presence move transaction;
- turns;
- event log;
- prompt choices.

### Game Dependencies

- ship;
- turns;
- route hazards;
- random events;
- place recall.

### MVP Shape

- List outbound routes.
- Moving costs one turn.
- Update presence and recall.
- Append event optionally.

### Later Ambition

- fuel cost;
- hidden routes;
- route requirements;
- hazard checks;
- customs/pirate encounters;
- route instability;
- one-way drift;
- travel policies.

### Open Questions

1. Should route hazard be visible before travel?
2. Should failed travel still move the player?
3. Should fuel exist in MVP?

---

## 15. System: Ship Model

### Purpose

Represent the player’s vehicle, capabilities, risks, and progression.

### Player-Facing Behaviors

- View ship stats.
- Install modules.
- Repair damage.
- Upgrade or buy hulls.
- Feel different career playstyles.

### State Owned

Shared DB likely:

- active ship;
- hull key;
- name;
- hull integrity;
- fuel;
- installed modules;
- cargo capacity;
- combat stats;
- salvage stats.

### Foglet Kit Dependencies

- owner-keyed inventory for cargo/modules.
- per-user save if some private customization is non-shared.

### Game Dependencies

- captain;
- cargo;
- travel;
- market;
- combat;
- salvage.

### MVP Shape

- One starter ship.
- Fixed cargo capacity.
- Hull integrity maybe displayed but not deeply used.

### Later Ambition

- hull classes;
- modules;
- damage/repair;
- fuel efficiency;
- scanners;
- stealth;
- crew;
- insurance;
- named flagship perks.

### Open Questions

1. Is ship a separate DB entity or embedded in captain state?
2. Are modules inventory items or separate installed records?
3. Should players own multiple ships?

---

## 16. System: Cargo and Inventory

### Purpose

Track what ships, stations, outposts, derelicts, and factions own.

### Player-Facing Behaviors

- View cargo manifest.
- Buy/sell goods.
- Loot salvage.
- Deposit/withdraw stockpiles.
- See cargo capacity constraints.

### State Owned

Shared DB:

- owner-keyed inventory slots;
- item quantities;
- metadata;
- capacity usage if not derived.

### Foglet Kit Dependencies

- inventory slots.
- Atomic transfer API.

### Game Dependencies

- item/commodity catalog;
- ship;
- market;
- contracts;
- salvage.

### MVP Shape

- Ship cargo slots for 3 commodities.
- Station market stock slots.
- Buy/sell transfers with capacity check.

### Later Ambition

- containers;
- contraband flags;
- spoilage;
- module inventory;
- mission escrow;
- faction depots;
- hidden caches;
- stack merge policies.

### Open Questions

1. Should capacity be per unit, per item volume, or simple total quantity?
2. Should credits be inventory, captain field, or ledger?
3. How should metadata affect stack merging?

---

## 17. System: Commodity and Item Catalog

### Purpose

Define the goods, modules, deployables, relics, and mission items the game recognizes.

### Player-Facing Behaviors

- Understand what items are for.
- See category, legality, cargo size, base value.
- Learn trade routes and special uses.

### State Owned

Content data:

- item key;
- display name;
- category;
- base value;
- volume;
- legality;
- tags;
- flavor text;
- handling rules.

### Foglet Kit Dependencies

- Item keys for inventory slots and markets.

### Game Dependencies

- content registry;
- market;
- cargo;
- contracts;
- smuggling;
- salvage.

### MVP Shape

Three commodities:

- Ore;
- Med Gel;
- Reactor Coolant.

### Later Ambition

- dozens of commodities;
- modules;
- deployables;
- relics;
- mission items;
- faction-specific goods;
- volatile/spoilage rules.

### Open Questions

1. Should item definitions be TOML, RON, YAML, or Rust constants first?
2. How much economic metadata belongs in item catalog vs market config?

---

## 18. System: Market and Pricing

### Purpose

Create the main economic play loop.

### Player-Facing Behaviors

- Buy low/sell high.
- See station stock and prices.
- Exploit scarcity.
- Watch prices react to actions.
- Compete with NPCs/players.

### State Owned

Shared DB:

- station stock;
- market listings;
- price modifiers;
- recent trade history;
- equilibrium quantities;
- restock timestamps.

### Foglet Kit Dependencies

- market listing primitive.
- owner-keyed inventory and atomic transfers.
- world DB transactions.

### Game Dependencies

- commodity catalog;
- cargo;
- station/place;
- captain credits;
- world ticks;
- NPC simulation.

### MVP Shape

- Two stations with fixed buy/sell prices and finite stock.
- Buy/sell cargo with credits and capacity validation.

### Later Ambition

- dynamic pricing;
- equilibrium restock;
- NPC trade effects;
- faction modifiers;
- black markets;
- contract-driven demand;
- scarcity crises;
- market rumors;
- player listings.

### Open Questions

1. Should MVP prices be fixed or stock-sensitive?
2. Should station buy and sell stock be same pool?
3. How transparent should pricing formulas be?

---

## 19. System: Contracts and Job Board

### Purpose

Give players clear, bounded objectives that create direction and rewards.

### Player-Facing Behaviors

- Browse jobs at stations.
- Accept contract.
- See requirements, reward, expiry.
- Complete and receive reward.
- Fail or abandon.

### State Owned

Shared DB:

- contract instances;
- issuer;
- objective kind;
- target/origin/destination;
- reward;
- expiry;
- accepted_by;
- state;
- metadata.

### Foglet Kit Dependencies

- world DB.
- turns.
- event log.
- notices.
- inventory transfers.

Potential kit wishlist:

- generic contract primitive.

### Game Dependencies

- captain;
- market/cargo;
- travel;
- item catalog;
- station services;
- world ticks.

### MVP Shape

- One delivery contract from Station A to Station B.
- Accept, carry commodity, complete via inventory transfer, reward credits.

### Later Ambition

- generated contracts;
- faction contracts;
- salvage recovery;
- survey jobs;
- smuggling jobs;
- escort abstraction;
- contract chains;
- contract competition.

### Open Questions

1. Should contract cargo be supplied by issuer or purchased by player?
2. Can multiple contracts target same cargo?
3. Should contracts be exclusive once accepted?

---

## 20. System: Bounties

### Purpose

Support target-oriented async objectives with claim and completion lifecycle.

### Player-Facing Behaviors

- Browse bounties.
- Claim bounty.
- Hunt target or recover item.
- Complete for reward and reputation.

### State Owned

Shared DB via bounty primitive:

- title;
- description;
- reward;
- state;
- posted_by;
- claimed_by;
- timestamps;
- metadata.

### Foglet Kit Dependencies

- bounty primitive.
- event log.
- notices.
- inventory transfer.
- leaderboards.

### Game Dependencies

- contracts;
- combat or salvage;
- factions;
- NPCs;
- travel.

### MVP Shape

Defer until after basic contracts.

### Later Ambition

- system bounties;
- faction bounties;
- NPC-posted bounties;
- player-posted bounties;
- contested bounties;
- bounty hunter NPCs.

### Open Questions

1. Should bounties be mechanically separate from contracts in UI?
2. Are player bounties allowed, and under what safety rules?

---

## 21. System: Notices, Mail, and Daily Intel

### Purpose

Deliver async consequences and make the world feel alive between sessions.

### Player-Facing Behaviors

- Read inbox.
- Receive daily intel packet.
- Get contract/bounty/faction updates.
- Receive NPC and player messages.
- Archive notices.

### State Owned

Shared DB via notice primitive:

- sender;
- recipient;
- subject;
- body;
- kind;
- read/archive state;
- expiry;
- metadata.

### Foglet Kit Dependencies

- notices.
- bounded player-authored text.
- prompt/text UI.

### Game Dependencies

- captain;
- contracts;
- NPCs;
- factions;
- event log;
- world ticks.

### MVP Shape

- System notices for completed contracts or daily turn reset.
- Inbox screen.

### Later Ambition

- player mail;
- NPC relationship messages;
- faction dispatches;
- trap reports;
- outpost summaries;
- configurable daily intel digest.

### Open Questions

1. Should daily intel be stored as notices or generated on read?
2. Should player mail launch in first multiplayer slice?
3. What moderation tools are required before player-authored mail?

---

## 22. System: Event Log and News

### Purpose

Record world history and surface meaningful shared consequences.

### Player-Facing Behaviors

- Read recent events/news.
- See own history.
- Notice player/NPC/faction actions.
- Understand why prices/routes changed.

### State Owned

Shared DB via event primitive:

- event kind;
- actor;
- place;
- timestamp;
- message;
- metadata;
- visibility/scope if added.

### Foglet Kit Dependencies

- event log.

### Game Dependencies

- almost every mutating system.

### MVP Shape

Append events for:

- travel;
- trade;
- contract completion.

Show recent 10 events.

### Later Ambition

- scoped news;
- rumor generation from events;
- event filters;
- season chronicle;
- NPC commentary;
- leaderboard milestones.

### Open Questions

1. Which actions deserve global events vs private history?
2. Should event text be stored directly or rendered from structured payload?

---

## 23. System: Random Events

### Purpose

Prevent repetitive routes and create solo-session drama.

### Player-Facing Behaviors

- Encounter travel, dockside, salvage, smuggling, and faction events.
- Make choices with consequences.
- Experience surprise without unfair opacity.

### State Owned

Content data:

- event tables;
- weights;
- prerequisites;
- outcomes;
- text.

Shared DB may record:

- triggered events;
- temporary modifiers;
- spawned contracts/wrecks/notices.

### Foglet Kit Dependencies

- prompts;
- event log;
- turns;
- world DB transactions.

Potential kit wishlist:

- deterministic random table helper.

### Game Dependencies

- travel;
- market;
- ship;
- factions;
- NPCs.

### MVP Shape

- One travel event table with 4 outcomes:
  - quiet;
  - debris salvage;
  - customs warning;
  - minor pirate threat.

### Later Ambition

- weighted tables by region/place/heat/faction;
- chained events;
- rare jackpots;
- event memory;
- player/NPC-authored consequences.

### Open Questions

1. Should event rolls be deterministic per action for test reproducibility?
2. How often should travel events fire?
3. Should players see odds?

---

## 24. System: World Ticks and Simulation

### Purpose

Advance the galaxy without requiring realtime multiplayer or a long-lived daemon.

### Player-Facing Behaviors

- Markets restock/change.
- Contracts expire/appear.
- NPCs act.
- Factions progress.
- Route hazards drift.
- Daily intel summarizes changes.

### State Owned

Shared DB:

- tick task registry;
- last run times;
- generated events;
- market modifiers;
- contract generation;
- NPC state changes;
- faction progress.

### Foglet Kit Dependencies

- world ticks.
- `fgk tick` optional CLI.
- SQLite transactions.

### Game Dependencies

- market;
- contracts;
- NPCs;
- factions;
- route hazards;
- notices/events.

### MVP Shape

- Daily turn reset.
- Simple market restock task.

### Later Ambition

- multi-layer simulation:
  1. global/region drift;
  2. station market refresh;
  3. named NPC actions;
  4. faction fronts;
  5. outpost production;
  6. contract generation;
  7. rumor generation.

### Open Questions

1. How much catch-up is allowed on login?
2. Which ticks need cron vs lazy?
3. How do we summarize long inactivity?

---

## 25. System: NPC Captains and Background Population

### Purpose

Make the galaxy active on low-pop BBSes.

### Player-Facing Behaviors

- See NPC rival actions in news.
- Receive NPC notices.
- Compete for salvage/contracts/markets.
- Gain allies/rivals.
- Encounter NPCs in travel/combat/dialog.

### State Owned

Shared DB:

- NPC identity;
- archetype;
- location/presence;
- ship tier;
- wealth tier;
- faction lean;
- current goal;
- relationship/memory records;
- last action timestamp.

### Foglet Kit Dependencies

- world DB;
- presence;
- event log;
- notices;
- world ticks.

### Game Dependencies

- spatial world;
- market;
- contracts;
- factions;
- combat later.

### MVP Shape

- Background NPC trade flow changes market stock during ticks.
- No individual named NPC required for first MVP.

### Later Ambition

- 12–20 named NPC captains;
- grudges/favors;
- recurring dialog;
- generated bounties;
- NPC challenges;
- rival market moves;
- rescue/betrayal arcs.

### Open Questions

1. How transparent should NPC simulation be?
2. Should named NPCs obey the same rules as players or abstract rules?
3. How many NPCs before the world feels noisy?

---

## 26. System: Route Hazards and Intel

### Purpose

Make travel strategic and information-driven.

### Player-Facing Behaviors

- See known route danger.
- Scan for updated intel.
- Risk stale information.
- Watch routes become safer or worse.

### State Owned

Shared DB:

- hazard type;
- hazard level;
- route/place association;
- last updated;
- source;
- expiry/decay;
- hidden/revealed state.

Per-player recall:

- last-known hazard snapshot.

### Foglet Kit Dependencies

- routes;
- place recall;
- world ticks.

### Game Dependencies

- travel;
- random events;
- scanning;
- factions;
- NPCs.

### MVP Shape

- One route marked risky.
- Risky routes have chance of random event.

### Later Ambition

- mines;
- pirate pressure;
- customs pressure;
- radiation;
- anomaly instability;
- route decay;
- scan quality;
- misinformation.

### Open Questions

1. Are hazards attached to Routes, Places, or both?
2. Should scanning cost turns?
3. How quickly does intel decay?

---

## 27. System: Scanning and Intel Acquisition

### Purpose

Let players spend resources to reduce uncertainty.

### Player-Facing Behaviors

- Scan route/place/derelict.
- Buy intel from contacts.
- Receive rumors.
- Update recall.
- Discover hidden routes or sites.

### State Owned

Shared DB:

- discovered hidden places/routes if global;
- scan-generated events;
- intel offers.

Per-player recall:

- scan snapshots;
- confidence;
- discovered private routes.

### Foglet Kit Dependencies

- place recall;
- prompts;
- turns;
- notices.

### Game Dependencies

- route hazards;
- ship sensors;
- random events;
- contacts/NPCs.

### MVP Shape

Defer, except auto-recall on visit.

### Later Ambition

- scan action;
- sensor modules;
- hidden route discovery;
- rumor verification;
- faction intel reports.

### Open Questions

1. Are hidden routes player-private or become global when discovered?
2. Can players sell intel to each other?

---

## 28. System: Smuggling, Heat, and Customs

### Purpose

Create an illegal-trade playstyle with risk and tension.

### Player-Facing Behaviors

- Carry contraband.
- Hide cargo.
- Bribe inspectors.
- Use false holds.
- Gain heat.
- Access black markets.

### State Owned

Shared DB:

- captain heat;
- regional/faction heat;
- contraband flags;
- customs event history;
- bribe/debt records if needed.

### Foglet Kit Dependencies

- inventory metadata;
- random events;
- turns;
- notices;
- event log.

### Game Dependencies

- commodity catalog;
- ship modules;
- travel;
- market;
- factions.

### MVP Shape

Defer. Maybe one “restricted goods” flag with no full customs system.

### Later Ambition

- contraband markets;
- inspection chance;
- heat decay;
- faction-specific legality;
- forged transponders;
- smuggling contracts;
- customs NPCs.

### Open Questions

1. Is heat global, regional, faction-specific, or layered?
2. How punishing should confiscation be?
3. Should bribes be deterministic or chance-based?

---

## 29. System: Combat

### Purpose

Provide danger, piracy, bounty hunting, defense, and consequence-rich conflict.

### Player-Facing Behaviors

- Fight, flee, bribe, hide, surrender cargo, or use modules.
- Resolve concise combat choices.
- Suffer or inflict damage/loss.
- Receive combat summaries.

### State Owned

Shared DB:

- ship damage;
- combat events;
- cargo loss/transfers;
- heat/reputation changes;
- wreck creation;
- bounty progress.

### Foglet Kit Dependencies

- prompts;
- turns;
- inventory transfers;
- event log;
- notices.

### Game Dependencies

- ship;
- cargo;
- route hazards;
- NPCs;
- bounties;
- traps.

### MVP Shape

Defer or implement only “pirate toll” random event with non-combat choices.

### Later Ambition

- combat stances;
- targeting;
- modules;
- boarding;
- escape pods;
- insurance;
- async PvP policies;
- wreck creation.

### Open Questions

1. How deterministic should combat be?
2. How much choice per round before it becomes too slow?
3. Should PvP combat be enabled by default?

---

## 30. System: Salvage and Derelict Boarding

### Purpose

Create a signature gameplay loop beyond trading.

### Player-Facing Behaviors

- Discover wrecks.
- Board derelicts.
- Move through ASCII rooms.
- Choose salvage under risk/capacity/turn pressure.
- Recover black boxes, modules, relics.

### State Owned

Shared DB:

- derelict site;
- room states;
- loot containers;
- hazards;
- claimed/stripped state;
- decay.

Per-user or shared session:

- local position inside derelict;
- temporary hazard state.

### Foglet Kit Dependencies

- maps;
- prompts;
- inventory transfers;
- turns;
- event log;
- place recall;
- world ticks.

### Game Dependencies

- travel;
- ship/cargo;
- item catalog;
- random events;
- contracts/bounties.

### MVP Shape

Could be first signature vertical slice after trade:

- one static derelict map;
- 3 rooms;
- 1 hazard;
- 1 loot container;
- return to station and sell loot.

### Later Ambition

- generated derelicts;
- black-box contracts;
- survivor dilemmas;
- trap rooms;
- rival salvage claims;
- decay/partial stripping;
- anomaly relics.

### Open Questions

1. Should derelict state be shared globally or per-player instance?
2. Can multiple players strip the same derelict asynchronously?
3. Should derelict exploration be resumable after quit?

---

## 31. System: Factions and Shared Goals

### Purpose

Create long-term world-shaping progression and ideological choices.

### Player-Facing Behaviors

- Join/support factions.
- Gain standing.
- Unlock services/missions.
- Contribute to shared goals.
- See faction influence change the map.

### State Owned

Shared DB via faction primitives:

- faction definitions;
- memberships;
- standing;
- shared goals;
- contributions;
- influence by region/place.

### Foglet Kit Dependencies

- factions/shared goals.
- event log.
- notices.
- inventory transfers.

### Game Dependencies

- contracts;
- market;
- travel;
- world ticks;
- NPCs.

### MVP Shape

Defer, or seed one faction as flavor only.

### Later Ambition

- six factions;
- mutually conflicting goals;
- faction offices;
- faction-specific markets;
- region control;
- campaign phase outcomes.

### Open Questions

1. Can a player join multiple factions?
2. Are faction standings numeric, ranks, or both?
3. How much faction change should be reversible?

---

## 32. System: Leaderboards and Stats

### Purpose

Give BBS-style social comparison and long-term goals.

### Player-Facing Behaviors

- View richest captains.
- See top traders, salvagers, bounty hunters, explorers, pirates.
- Track own rank.

### State Owned

Shared DB via leaderboard primitive:

- named scoreboards;
- player scores;
- rank queries;
- seasonal snapshots.

### Foglet Kit Dependencies

- leaderboard helpers.

### Game Dependencies

- captain;
- market;
- contracts;
- salvage;
- factions;
- combat.

### MVP Shape

- Credits leaderboard.
- Contracts completed leaderboard.

### Later Ambition

- seasonal boards;
- faction boards;
- NPC inclusion;
- title awards;
- weekly summaries.

### Open Questions

1. Should NPCs appear on leaderboards?
2. Should leaderboards reset per season?
3. Which scores are increment-only vs recalculated?

---

## 33. System: Dialog and Contacts

### Purpose

Give stations, NPCs, and factions personality and branchable interactions.

### Player-Facing Behaviors

- Talk to brokers, mechanics, faction officers, smugglers, relay AIs.
- Unlock jobs, rumors, discounts, or consequences.
- Make choices with flags and standing changes.

### State Owned

Content:

- dialog trees;
- choices;
- requirements;
- effects;
- speaker metadata.

Shared/per-user:

- flags;
- relationship memory;
- completed dialog nodes.

### Foglet Kit Dependencies

- DialogScreen.
- Prompt integration.
- FlagSet/dialog state.

### Game Dependencies

- station services;
- NPCs;
- factions;
- contracts;
- notices.

### MVP Shape

- One relay AI or station clerk help dialog.

### Later Ambition

- recurring contacts;
- NPC memory branches;
- faction recruiters;
- informants;
- black-market brokers;
- derelict AI terminals.

### Open Questions

1. Should dialog be data-driven from YAML/TOML, or coded first?
2. Where do dialog flags live?
3. Can dialog perform world transactions directly or emit commands?

---

## 34. System: Station Services

### Purpose

Bundle local actions available at a dockable Place.

### Player-Facing Behaviors

- Market.
- Refuel.
- Repair.
- Jobs.
- Notices.
- Shipyard.
- Contacts.
- Faction office.
- Bank/debt.

### State Owned

Place metadata/content:

- services available;
- requirements;
- local modifiers;
- NPC contacts;
- market associations.

### Foglet Kit Dependencies

- prompts;
- screen stack;
- inventory;
- world DB.

### Game Dependencies

- place;
- market;
- ship;
- contracts;
- notices;
- factions.

### MVP Shape

Start station has:

- market;
- jobs;
- log;
- travel;
- quit.

### Later Ambition

- modular service lists;
- disabled services with reasons;
- hidden black-market service;
- faction-locked services;
- station damage/service outages.

### Open Questions

1. Are services defined in place metadata or separate station config?
2. Should all stations have inbox access?

---

## 35. System: Shipyard, Repairs, and Modules

### Purpose

Provide ship progression and money sinks.

### Player-Facing Behaviors

- Repair hull.
- Refuel.
- Buy/sell/install modules.
- Upgrade hull.
- Compare ship stats.

### State Owned

Shared DB:

- ship state;
- module inventory;
- installed modules;
- station stock;
- repair pricing modifiers.

### Foglet Kit Dependencies

- inventory transfers;
- market;
- prompts/disabled choices.

### Game Dependencies

- ship;
- cargo;
- item catalog;
- market;
- station services.

### MVP Shape

- Refuel/repair may be deferred if no fuel/damage yet.

### Later Ambition

- module slots;
- install/uninstall costs;
- damaged modules;
- rare modules;
- faction tech;
- relic modules.

### Open Questions

1. Does fuel exist in MVP?
2. Are modules commodities, unique items, or separate records?

---

## 36. System: Traps, Mines, and Deployables

### Purpose

Support async area denial, piracy, defense, and strategic route play.

### Player-Facing Behaviors

- Deploy mines/traps.
- Scan/sweep hazards.
- Receive triggered reports.
- Build route defenses or ambushes.

### State Owned

Shared DB:

- deployable id;
- owner;
- place/route;
- trigger conditions;
- effect;
- detection difficulty;
- decay;
- faction friendliness.

### Foglet Kit Dependencies

- inventory;
- world DB;
- route/place spatial data;
- notices;
- event log;
- world ticks.

### Game Dependencies

- item catalog;
- travel;
- combat;
- scanning;
- factions.

### MVP Shape

Defer.

### Later Ambition

- mines;
- EMP traps;
- decoy wrecks;
- interdiction buoys;
- sentry drones;
- mine sweeping contracts;
- trap chains.

### Open Questions

1. How do we prevent griefing in low-pop BBSes?
2. Should safe routes forbid traps?
3. How visible are traps before triggering?

---

## 37. System: Outposts and Colonies

### Purpose

Provide strategic ownership, production, storage, and long-term investment.

### Player-Facing Behaviors

- Claim/build outpost.
- Deposit stock.
- Configure production/policies.
- Defend against raids.
- Use as storage/repair/market.

### State Owned

Shared DB:

- outpost place/owner;
- upgrade level;
- production queues;
- stockpiles;
- defense;
- access policy;
- morale/population if colony.

### Foglet Kit Dependencies

- places;
- owner-keyed inventory;
- world ticks;
- factions;
- event log.

### Game Dependencies

- cargo;
- market;
- factions;
- travel;
- traps/defense;
- corporations.

### MVP Shape

Defer.

### Later Ambition

- claim beacons;
- hidden caches;
- production chains;
- faction-aligned colonies;
- raids;
- public/private markets;
- strategic route beacons.

### Open Questions

1. Should one player be allowed many outposts?
2. How much management is fun in terminal UI?
3. Can outposts be destroyed or only degraded?

---

## 38. System: Corporations and Charters

### Purpose

Support group identity and shared resources while preserving solo compatibility.

### Player-Facing Behaviors

- Create/join corporation or solo charter.
- Share depots, credits, route notes, and goals.
- Manage ranks/permissions.

### State Owned

Shared DB:

- corp id;
- members;
- roles;
- bank;
- depots;
- notices;
- outposts;
- route notes.

### Foglet Kit Dependencies

- factions maybe not directly, but similar membership primitives.
- notices.
- inventory.
- leaderboards.

Potential kit wishlist:

- generic group/shared-resource primitive if faction APIs are not enough.

### Game Dependencies

- captain;
- inventory;
- outposts;
- notices;
- contracts.

### MVP Shape

Defer.

### Later Ambition

- player corps;
- one-player charters;
- NPC-backed charter crew;
- corp projects;
- corp market listings;
- corp-vs-corp season goals.

### Open Questions

1. Can faction membership and corporation membership overlap freely?
2. What permission model is enough?
3. Should solo charters be mechanically identical to corps?

---

## 39. System: Async Challenges

### Purpose

Support BBS-native competition without simultaneous play.

### Player-Facing Behaviors

- Issue/accept challenges.
- Compete on profit, delivery, salvage, duel, or faction objective.
- Receive result notice.

### State Owned

Shared DB via challenge primitive:

- challenger;
- target;
- kind;
- stake;
- state;
- expiry;
- result.

### Foglet Kit Dependencies

- challenges.
- notices.
- event log.
- leaderboards.

### Game Dependencies

- contracts;
- market;
- salvage;
- combat;
- factions.

### MVP Shape

Defer.

### Later Ambition

- profit race;
- courier race;
- salvage draft;
- privateer duel;
- faction challenge board;
- NPC challenges for low population.

### Open Questions

1. Should challenges require explicit acceptance?
2. Can NPCs issue challenges by default?
3. What stakes are allowed?

---

## 40. System: Achievements and Milestones

### Purpose

Provide medium-term goals and celebrate stories.

### Player-Facing Behaviors

- Unlock achievements.
- See milestone notices.
- Earn titles or cosmetic labels.

### State Owned

Shared or per-player DB:

- achievement key;
- unlocked_at;
- progress;
- title/reward if any.

### Foglet Kit Dependencies

- event log;
- notices;
- leaderboards maybe.

### Game Dependencies

- all major systems.

### MVP Shape

Defer, except simple stats counters.

### Later Ambition

- local achievements;
- seasonal titles;
- faction medals;
- hidden achievements;
- obituary/memorial achievements.

### Open Questions

1. Are achievements purely in-game or exposed to Foglet later?
2. Should hidden achievements be discoverable?

---

## 41. System: Administration and Recovery

### Purpose

Give sysops or game maintainers ways to inspect and recover broken world state.

### Player-Facing Behaviors

Normally invisible. Sysop/mod roles may unlock debug/admin screens.

### State Owned

- admin audit events;
- world repair actions;
- stuck contract cleanup;
- market reset tools;
- player rescue tools.

### Foglet Kit Dependencies

- Foglet role/security mapping.
- World DB.
- event log.

### Game Dependencies

- all durable systems.

### MVP Shape

- None, but ensure DB is inspectable and transactions are safe.

### Later Ambition

- admin inspect screen;
- stuck player rescue;
- force tick;
- regenerate contracts;
- close broken bounties;
- export world summary.

### Open Questions

1. Should sysop admin exist inside the door or through CLI only?
2. How do we audit admin actions?

---

## 42. System: Testing and Simulation Harness

### Purpose

Make a complex async game testable without manual terminal play.

### Player-Facing Behaviors

None directly, but improves reliability.

### State Owned

Test fixtures:

- seeded world;
- fake players;
- deterministic RNG;
- scripted input;
- snapshot states;
- expected event logs.

### Foglet Kit Dependencies

- Test backend rendering.
- Date-provider injection.
- Local-dev Foglet context.
- World DB temp dirs.

### Game Dependencies

- all systems as they are built.

### MVP Shape

- Unit tests for travel and market transaction.
- Integration test for create captain -> buy cargo -> travel -> sell cargo -> save.

### Later Ambition

- multi-player async simulations;
- NPC tick golden tests;
- random event deterministic tests;
- economic balance smoke tests;
- terminal frame regression tests;
- fuzz transaction boundaries.

### Open Questions

1. Should random event outcomes be fully seedable from tests?
2. Should we create a standalone simulation CLI?

---

## 43. Game-Kit Wishlist Candidates

These are not necessarily required before development, but they are candidates for generic kit features rather than Drop Dead Nebula-only code.

### 43.1 Generic Contract / Job Board Primitive

Why:

- Many BBS games need structured jobs.
- Current kit has bounties/challenges but not generic contracts.

Candidate features:

- contract lifecycle;
- issuer/acceptor;
- expiry;
- typed objective payload;
- reward payload;
- completion validation hook;
- job board UI helper.

### 43.2 Deterministic Random Table Helper

Why:

- Random events need testable weighted outcomes.

Candidate features:

- weighted table definition;
- seeded roller;
- prerequisite filters;
- outcome metadata;
- test helpers.

### 43.3 Reusable Notice Inbox Screen

Why:

- notices are generic; every game needs inbox/read/archive UI.

Candidate features:

- list notices;
- unread/read/archive;
- detail modal;
- hotkeys;
- bounded body rendering.

### 43.4 Reusable Leaderboard Screen

Why:

- leaderboards need a standard terminal presentation.

Candidate features:

- top N;
- player rank;
- multiple boards;
- pagination;
- empty-state text.

### 43.5 World Tick Catch-Up Summary Helper

Why:

- Games need to avoid huge catch-up dumps.

Candidate features:

- bounded catch-up policy;
- summary aggregation;
- tick result report;
- player-facing digest generation.

### 43.6 Owner Inventory Capacity Helper

Why:

- the inventory primitive intentionally avoids weight/volume, but many games need capacity checks.

Candidate features:

- game-supplied item volume callback;
- capacity check wrapper around transfer;
- clear insufficient-space errors.

### 43.7 Market Pricing Examples / Helper

Why:

- Kit does not prescribe economics, but reusable examples would accelerate game authors.

Candidate features:

- stock-sensitive price formula example;
- equilibrium restock example;
- transaction wrapper examples;
- no universal pricing engine.

### 43.8 Spatial Travel Helper

Why:

- Many games will do route validation + turn spend + movement + recall + event append.

Candidate features:

- composable transaction helper;
- route requirement callback;
- turn spend callback;
- recall touch option;
- event append option.

### 43.9 Bounded Player Text Sanitizer

Why:

- Notices, bounties, corp bulletins, and mail all need safe terminal text.

Candidate features:

- max length;
- control character stripping;
- line wrapping;
- optional profanity/mod hook later;
- test vectors.

### 43.10 Multi-Player Local-Dev Test Harness

Why:

- Async BBS mechanics need two or more fake players in tests.

Candidate features:

- create fake Foglet contexts;
- run scripted sessions;
- inspect shared DB effects;
- compare notices/events/leaderboards.

---

## 44. First Vertical Slice Recommendation

The first slice should be intentionally small:

> A player can create/resume a captain, start at Ash Coil, view dashboard, buy Med Gel, spend a turn traveling to Mercy Relay, sell Med Gel for profit, complete one delivery contract, see an event log entry, quit, and resume with state intact.

### Included Systems

- config;
- content keys;
- captain;
- save/resume;
- UI dashboard;
- world bootstrap;
- presence;
- turns;
- travel;
- ship basics;
- cargo/inventory;
- commodity catalog;
- market/pricing;
- one contract;
- event log.

### Excluded Systems

- combat;
- factions;
- bounties;
- NPC captains;
- salvage;
- mines/traps;
- outposts;
- corporations;
- async challenges;
- dynamic pricing beyond simple stock.

### Why This Slice

It proves the spine:

- identity;
- shared DB;
- spatial movement;
- turns;
- inventory transfer;
- market transaction;
- event log;
- terminal UI;
- save/resume.

Everything ambitious can hang off that spine later.

---

## 45. Immediate Next Documentation After This

Recommended next docs:

1. `DROP_DEAD_NEBULA_MVP.md`  
   A strict vertical-slice specification with acceptance criteria.

2. `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`  
   A separated list of kit feature requests, each labeled required/nice/defer.

3. `DROP_DEAD_NEBULA_CONTENT_SEED.md`  
   The tiny authored world for the first slice: places, routes, commodities, markets, one contract.

4. `DROP_DEAD_NEBULA_DATA_MODEL.md`  
   DB ownership and schema sketch for game-specific tables beyond the kit.

Do not write a broad implementation plan until the MVP doc and content seed are settled.

---

## 46. Open System-Level Questions

1. Should Drop Dead Nebula live inside the `foglet-game-kit-rs` workspace as an example, or become a separate game repo consuming the crate?
2. Should the first vertical slice emphasize trading or salvage?
3. Which data belongs in game-specific tables versus generic kit primitives?
4. How soon should world generation exist versus authored seed content?
5. Should NPC simulation be introduced before factions?
6. How much of the game should be data-driven before a second content set exists?
7. Should the kit be extended first with helpers, or should helpers emerge from game code and later be promoted?
8. How should local-dev multi-player testing be structured?
9. Should the game target 80x24 strictly, or support a richer layout at larger sizes?
10. Should first alpha be single-player-only-with-NPCs before enabling player mail/challenges?

---

## 47. Recommended Near-Term Decision

The next real decision is:

> Is the first playable slice a **trade-route slice** or a **salvage-derelict slice**?

Recommendation: start with the **trade-route slice** because it proves more of the shared-world spine with less content complexity. Then add a tiny derelict as the first signature expansion.

Trade first proves:

- places/routes;
- turns;
- cargo;
- market;
- inventory transfers;
- contracts;
- event log;
- save/resume.

Salvage second proves:

- local maps;
- random events;
- loot;
- hazards;
- richer prompts;
- stronger game identity.
