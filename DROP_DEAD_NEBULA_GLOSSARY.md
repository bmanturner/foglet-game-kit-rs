# Drop Dead Nebula — Glossary

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`  
Purpose: Establish stable vocabulary for design, implementation, tests, content authoring, and future Foglet game-kit requests.

---

## 1. Glossary Principles

This glossary is intentionally opinionated. Its job is to prevent design drift while Drop Dead Nebula moves from broad GDD to buildable systems.

### 1.1 Naming Rules

- Use **Place** for the generic graph node stored by the Foglet game-kit spatial primitive.
- Use **Sector** for the in-fiction navigable space around a place when the player thinks in star-map terms.
- Use **Station**, **Outpost**, **Derelict**, **Anomaly**, etc. for specific place kinds.
- Use **Route** for a directed graph edge between places.
- Use **Turn** for the daily action budget.
- Use **Tick** for world-simulation advancement.
- Use **Notice** for durable in-game messages delivered through the kit notice/mail system.
- Use **Event** for append-only world-history records.
- Use **Contract** for job-board tasks with structured objectives and rewards.
- Use **Bounty** for target-oriented public or faction jobs with claim/complete lifecycle.
- Use **Challenge** for asynchronous player-vs-player or player-vs-NPC contests.
- Use **Cargo** for goods carried by a ship.
- Use **Inventory Slot** for the implementation-level owner-keyed storage primitive.

### 1.2 Fiction vs Implementation

Many terms have both a fiction-facing meaning and an implementation-facing meaning. For example:

- A **station warehouse** is fiction.
- A station warehouse is implemented as owner-keyed inventory slots where `owner_kind = "place"` or `"station"`.

When writing specs or tests, prefer implementation terms. When writing player UI or content, prefer fiction terms.

### 1.3 Avoid These Ambiguities

- Do not use “system” to mean both star system and software system. Prefer **Region**, **Place**, or **Game System**.
- Do not use “map” for the galaxy graph unless discussing player UI. The data model is a **spatial graph**.
- Do not use “message” generically. Prefer **Notice**, **Event**, **Rumor**, or **Dialog Line**.
- Do not use “mission” as a generic system name. Prefer **Contract**, **Bounty**, **Faction Goal**, or **Story Chain**.
- Do not use “planet” unless it is specifically a planet-like outpost. Most owned sites should be **Outposts** or **Colonies**.

---

## 2. Core Game Identity Terms

### Drop Dead Nebula

The title and primary setting. The Drop Dead Nebula is a dangerous frontier region full of failed relays, wreck fields, unstable routes, faction enclaves, black markets, salvage opportunities, and half-dead colonies.

Design usage:

- The game is not “the entire galaxy.” It is one dense, legible, reactive frontier region.
- The setting should support mystery, trade, salvage, piracy, faction politics, and weird relic phenomena.

Implementation usage:

- The default world database represents one nebula instance or season.
- Future sysop configuration may support different generated nebula seeds.

### Captain

The player’s in-fiction role: owner/operator of a ship.

Player-facing includes:

- captain name;
- reputation;
- faction standing;
- heat/wanted status;
- contacts;
- active ship;
- personal history.

Implementation notes:

- A Captain maps to a local player record created from Foglet context.
- Some Captain data may live in shared world DB; some may live in per-user save state.
- Shared DB should hold data other players or NPCs can reference.
- Per-user save should hold private UI/tutorial/story state and any non-shared convenience state.

### Ship

The player’s main vehicle and build platform.

Player-facing includes:

- hull type;
- name;
- hull integrity;
- cargo capacity;
- fuel state;
- installed modules;
- heat visibility;
- crew/morale if implemented;
- insurance or recovery status.

Implementation notes:

- Ship cargo should usually use owner-keyed inventory.
- Ship modules may be records, inventory items, or both depending on implementation needs.
- Ship state participates in travel, trade, combat, salvage, and random events.

### Run

A short play session inside the BBS door.

Design usage:

- A run is not a roguelike life. It is one login/session.
- The game should produce a satisfying run in 5–15 minutes.

### Career

The long-lived arc of a captain across many sessions.

Examples:

- trader career;
- smuggler career;
- salvage career;
- faction operative career;
- pirate career;
- outpost builder career.

### Season

A configured world lifecycle, usually lasting weeks or months.

Design usage:

- Seasons may reset leaderboards, market history, and some world state.
- Captain legacy may optionally persist across seasons, but this is not required for MVP.

---

## 3. Spatial and World Terms

### Place

The generic spatial graph node.

Player-facing synonyms may include sector, station, wreck, route marker, gate, outpost, or anomaly.

Implementation source:

- Foglet game-kit v4 spatial primitive.

Fields likely include:

- stable key;
- display name;
- kind;
- metadata JSON;
- creation timestamp.

Design rules:

- Every navigable location is a Place.
- Places do not need coordinates.
- Places are connected by directed Routes.
- Place kind determines available screens, encounters, services, and risks.

### Sector

A player-facing navigational label for a Place or cluster of local space around a Place.

Use Sector when:

- showing the star chart;
- describing route hazards;
- writing TradeWars-flavored UI;
- referring to travel state.

Avoid Sector when:

- writing database schema names, unless the schema explicitly chooses that term;
- describing the generic kit primitive.

### Region

A higher-level grouping of Places.

Examples:

- Ash Coil;
- Red Maw;
- Mercy Drift;
- The Static Crown;
- Union Fringe;
- Dead Gate Verge.

Design purpose:

- regional market modifiers;
- faction fronts;
- random event weighting;
- route danger themes;
- campaign progression;
- low-pop simulation summaries.

Implementation notes:

- Region may be a metadata field on Place.
- Region may have its own table if needed for faction control, crisis state, and market modifiers.

### Route

A directed edge from one Place to another.

Implementation source:

- Foglet game-kit v4 route primitive.

Player-facing examples:

- jump lane;
- smuggler cut;
- nebula current;
- wormhole;
- convoy corridor;
- dead-end tunnel;
- one-way drift.

Route attributes:

- from place;
- to place;
- kind;
- turn cost;
- hazard profile;
- stability;
- visibility;
- requirements;
- faction control;
- scan difficulty;
- metadata.

Design rules:

- Routes are directed.
- Bidirectional travel requires two routes.
- Parallel routes are allowed.
- Route validation is game logic.
- Routes can be hidden, temporary, unstable, faction-locked, or one-way.

### Gate

A special Route endpoint or Place representing old jump infrastructure.

Examples:

- Dead Gate;
- Mercy Relay Gate;
- Blackglass Gate.

Design purpose:

- unlockable map progression;
- faction shared goals;
- endgame access;
- route instability events.

### Relay

A communication/navigation infrastructure Place.

Player-facing role:

- mail access;
- route intel;
- news board;
- world event summaries;
- faction dispatches;
- possibly safe docking.

Design role:

- Relay repair can be a shared faction goal.
- Broken relays justify stale intel and missing route data.

### Station

A dockable Place with services.

Common services:

- market;
- repair;
- refuel;
- contracts;
- notices;
- shipyard;
- faction office;
- black market;
- bank/debt office;
- bar/informant.

Implementation notes:

- Station stock is owner-keyed inventory.
- Station services may be metadata and/or separate config/content data.

### Port

A market-bearing station or station subsystem.

Use Port when focusing on buy/sell economy. Use Station when focusing on place/services.

Port attributes:

- commodity supply;
- commodity demand;
- pricing modifiers;
- law level;
- bribeability;
- restock behavior;
- faction influence;
- local event state.

### Outpost

A persistent constructed or claimed Place, usually smaller than a Station.

Owners:

- player;
- corporation;
- faction;
- NPC;
- system.

Functions:

- stockpile;
- production;
- repair;
- hiding place;
- faction project site;
- defensive position;
- local market.

### Colony

An advanced Outpost with population, production, needs, morale, and vulnerability.

Design usage:

- mid/late-game strategic system;
- logistics sink for commodities;
- faction and corporation objective.

### Derelict

A wrecked or abandoned structure/ship available for salvage and exploration.

Two spatial layers:

- galaxy-level Place where the derelict exists;
- local ASCII map for boarding/exploration.

Derelict states:

- fresh;
- partially stripped;
- claimed;
- trapped;
- unstable;
- quarantined;
- exhausted;
- decayed.

### Wreck Field

A Place containing one or more salvage opportunities.

Difference from Derelict:

- Wreck Field is the outer site.
- Derelict is a specific salvage target inside it.

### Anomaly

A hazardous or supernatural/weird-science Place or route condition.

Effects:

- random events;
- rare commodities;
- route mutation;
- ship damage;
- faction interest;
- relic discovery;
- strange dialog/notices.

### Safe Zone

A Place or region where combat, piracy, or traps are limited.

Design purpose:

- protect new players;
- enable recovery;
- create lawless/safe contrast.

Safe Zone does not mean consequence-free. It may still have customs, taxes, debt collectors, and faction pressure.

### Lawless Zone

A Place or region where piracy, traps, black markets, and dangerous events are more common.

### Presence

The current Place occupied by a player or NPC.

Implementation source:

- Foglet game-kit v4 presence primitive.

Player-facing uses:

- current location;
- local-only services;
- “last seen” or “currently docked” flavor;
- async interaction targeting;
- sector danger awareness.

Design rule:

- Presence should be meaningful but not require realtime co-presence.

### Place Recall

A player’s remembered information about previously visited or scanned Places.

Implementation source:

- Foglet game-kit v4 place recall primitive.

Recall may include:

- first seen;
- last seen;
- market snapshot;
- hazard snapshot;
- known routes;
- station services;
- local notes;
- discovered contacts;
- confidence/staleness.

Design rule:

- Recall can be stale. Old information being wrong is a feature.

### Stale Intel

Information that was true or believed true at a prior time but may now be outdated.

Examples:

- market prices;
- route hazards;
- pirate presence;
- contract availability;
- station stock;
- faction control.

---

## 4. Time, Turns, and Simulation Terms

### Daily Turn

The player’s primary action budget.

Implementation source:

- Foglet game-kit v2 turns primitive.

Design purpose:

- BBS pacing;
- session length control;
- strategic scarcity;
- fair async competition.

### Turn Allowance

The number of turns granted per day.

Default design target:

- 30 turns/day;
- reserve cap of 60;
- configurable by sysop later.

### Reserve Turns

Unspent turns that carry over within a cap.

Purpose:

- forgiving for missed days;
- supports less frequent callers;
- avoids infinite hoarding.

### Emergency Turn

A rare bonus action resource earned through events, modules, favors, or consumables.

Use sparingly. Emergency turns create drama when the player is stranded or one action short.

### World Tick

A simulation advancement pass.

Implementation source:

- Foglet game-kit v4 world tick primitive.

Tick responsibilities:

- market restock/reprice;
- contract generation/expiry;
- NPC actions;
- faction goal progress;
- route hazard drift;
- outpost production;
- derelict decay;
- rumor generation;
- leaderboard summaries.

### Lazy Tick

A world tick triggered during player login, screen transition, or safe boundary rather than by a continuously running daemon.

Design rule:

- Lazy ticks must be bounded and should summarize large absences.

### Cron Tick

A world tick invoked by an external scheduled call such as `fgk tick`.

Design rule:

- Cron tick is optional. The game must still work without it.

### Catch-Up

Processing world ticks after a period of inactivity.

Rules:

- bounded per invocation;
- idempotent where possible;
- no long render blocking;
- summarize instead of simulating too much detail after long downtime.

### Day

A game-calendar period used for turn reset, contract expiry, faction phases, and daily news.

Need not match local wall-clock day exactly; default should use UTC or configured BBS timezone consistently.

### Season Phase

A broader campaign state affecting tables, events, and available content.

Examples:

- Frontier Scramble;
- Route Wars;
- Relic Rush;
- Succession Crisis;
- Dead Gate Opens.

---

## 5. Economy and Inventory Terms

### Commodity

A stackable trade good.

Examples:

- food paste;
- water;
- ore;
- med gel;
- reactor coolant;
- nav cores;
- weapon crates;
- black AI fragments;
- relic shards.

Design rules:

- Commodities should have clear source/sink logic.
- Commodity value depends on place, stock, demand, risk, faction state, and events.

### Cargo

Commodities or items currently carried by a ship.

Implementation:

- owner-keyed inventory where owner is the player ship.

### Item

A broader term for inventory objects, including commodities, modules, quest items, relics, and deployables.

### Inventory Slot

The implementation-level storage record for an item owned by an entity.

Implementation source:

- Foglet game-kit v4 owner-keyed inventory primitive.

Owners may include:

- player;
- ship;
- station;
- place;
- faction;
- corporation;
- derelict container;
- hidden cache;
- NPC;
- mission escrow.

### Stockpile

A durable collection of inventory slots owned by a station, outpost, faction, corp, or player.

### Equilibrium

A target inventory quantity used for restocking, decay, pricing, or production behavior.

Design rule:

- Equilibrium is advisory. Actual drift should be implemented by game world ticks.

### Market

A place or service where items can be bought or sold.

Implementation source:

- Foglet game-kit v3 market/listing primitive plus v4 inventory transfers.

Market types:

- station market;
- black market;
- faction depot;
- player/corp listing board;
- salvage auction;
- emergency relief market;
- shipyard module shop.

### Listing

A buy/sell offer in a Market.

Attributes:

- seller;
- item key;
- display name;
- quantity;
- price;
- expiry;
- metadata;
- restrictions.

### Price Spread

Difference between buy price and sell price across markets.

Gameplay role:

- drives trade routes;
- affected by risk, information, and scarcity.

### Demand

A place’s desire for a commodity or item.

Demand affects:

- price;
- contract generation;
- faction project needs;
- station crisis state.

### Supply

A place’s available stock or production capacity.

### Scarcity

A state where demand significantly exceeds supply.

Consequences:

- high prices;
- emergency contracts;
- unrest;
- faction opportunities;
- piracy risk.

### Market Drift

World-tick changes to stock or prices.

Drivers:

- NPC trade;
- station consumption;
- production;
- crises;
- player action;
- faction control.

### Arbitrage

Buying goods where cheap and selling where expensive.

Design rule:

- Arbitrage should require route knowledge, risk management, and timing.

### Contraband

Illegal or restricted goods.

Examples:

- black AI fragments;
- narcotic hymns;
- forged transponders;
- unlicensed relics;
- weapon crates.

Associated systems:

- heat;
- customs;
- smuggling modules;
- black markets;
- faction standing;
- random inspections.

### Heat

A measure of law-enforcement attention or suspicion.

Heat may be:

- global;
- regional;
- faction-specific;
- commodity-specific;
- route-specific.

High heat increases:

- inspections;
- bounty risk;
- bribe costs;
- denied services;
- NPC hunter activity.

### Credit

Primary currency.

Potential aliases in UI:

- credits;
- scrip;
- chits;
- station credit.

Use one canonical implementation term: **credits**.

### Debt

Negative or obligated credits owed to a lender, station, NPC, or faction.

Design use:

- recovery after ship loss;
- story pressure;
- low-pop solo hooks;
- risk/reward financing.

---

## 6. Jobs, Bounties, Challenges, and Goals

### Contract

A structured job offered to a player.

Attributes:

- issuer;
- objective;
- origin;
- destination or target;
- requirements;
- reward;
- expiry;
- risk;
- faction effects;
- completion proof.

Examples:

- deliver cargo;
- scan route;
- recover black box;
- clear mines;
- escort abstract convoy;
- smuggle package;
- supply outpost.

### Job Board

The UI/system where Contracts are listed.

### Bounty

A target-oriented job with claim/complete/expire lifecycle.

Implementation source:

- Foglet game-kit v3 bounty primitive.

Targets:

- NPC pirate;
- route hazard;
- item recovery;
- derelict objective;
- player if enabled and safe;
- faction project milestone.

### Claim

A player or NPC reserves or signals intent to complete a Bounty.

Design question:

- Some bounties should allow multiple claimants; others should be exclusive.

### Completion Evidence

Game-defined proof that a Contract or Bounty is complete.

Examples:

- delivered item;
- scan record;
- event flag;
- defeated NPC;
- recovered black box;
- transferred contribution.

### Challenge

An asynchronous contest between players or between player and NPC.

Implementation source:

- Foglet game-kit v3 challenge primitive.

Examples:

- profit race;
- salvage race;
- courier race;
- duel by policy;
- faction objective race.

### Shared Goal

A persistent faction/corp/world objective advanced by contributions or actions.

Implementation source:

- Foglet game-kit v3 faction/shared-goal primitive.

Examples:

- repair relay;
- build beacon;
- stabilize route;
- evacuate colony;
- blockade pirate den;
- open Dead Gate.

### Project

A construction-like Shared Goal, often owned by faction, corp, station, or outpost.

### Objective Chain

A multi-step sequence of Contracts, Bounties, Dialog flags, or world events.

Use instead of “questline” in implementation docs if it is system-driven.

---

## 7. Faction, Corporation, and Social Terms

### Faction

A major NPC organization with goals, reputation, territory, services, and conflicts.

Implementation source:

- Foglet game-kit v3 faction primitive.

Default factions:

- Mourning Union;
- Helix Cartel;
- Port Authority Black Office;
- Saints of Vacuum;
- Machine Concord;
- Dust Clans.

### Faction Standing

A player’s relationship score with a Faction.

Effects:

- service access;
- prices;
- missions;
- inspections;
- NPC behavior;
- route permissions;
- combat policies.

### Faction Influence

A faction’s control or soft power over a Place, Region, Route, or Market.

### Faction Front

A contested Region or route cluster where faction influence is actively changing.

### Corporation

A player-created or NPC-created group for shared resources and goals.

Features:

- shared bank;
- depots;
- notices;
- route notes;
- outpost ownership;
- role permissions;
- corp leaderboard.

### Charter

A solo-compatible corporation-like legal entity.

Design purpose:

- allows one active player to use corp-style outpost/storage systems.

### Ally

An NPC, faction, or player with positive relationship and likely benefits.

### Rival

An NPC or player with persistent competitive relationship.

### Enemy

A faction or actor likely to attack, block, tax, or sabotage the player.

### Reputation

A broad public perception track.

Types:

- lawful;
- pirate;
- humanitarian;
- trader;
- relic diver;
- faction-specific;
- local station.

### Notoriety

A more dramatic public track for illegal, violent, or legendary acts.

Notoriety is not always bad: pirates may benefit from fear.

---

## 8. NPC Terms

### NPC

Any non-player actor simulated or presented by the game.

Types:

- background flow;
- named captain;
- station contact;
- faction officer;
- pirate;
- patrol;
- merchant;
- salvage rival;
- relay AI.

### Named NPC Captain

A persistent NPC ship captain with lightweight state and memory.

Attributes:

- name;
- archetype;
- faction lean;
- ship;
- home region;
- current presence;
- wealth tier;
- risk tolerance;
- relationships;
- grudges;
- current goal.

### Background Flow

Abstract NPC activity represented as market, event, hazard, or faction changes rather than individual actors.

Examples:

- merchant traffic;
- refugee movement;
- patrol sweeps;
- pirate pressure;
- black-market supply.

### NPC Memory

Durable facts an NPC remembers about player actions.

Examples:

- helped in distress;
- stole claim;
- unpaid debt;
- shared faction;
- repeated route conflict;
- spared in combat.

### Contact

A friendly or usable NPC at a station, faction office, black market, or relay.

Uses:

- dialog;
- contracts;
- discounts;
- rumors;
- special services.

### Rival Event

A world event generated by an NPC rival that affects player plans.

Examples:

- undercut market;
- claimed derelict;
- posted challenge;
- stole contract bonus;
- sent taunting notice.

---

## 9. Combat, Danger, and Recovery Terms

### Encounter

A discrete event requiring player choice and resolution.

Types:

- travel encounter;
- dockside event;
- combat encounter;
- salvage event;
- smuggling complication;
- faction flashpoint.

### Combat

A conflict resolution mode involving ship damage, cargo risk, reputation, and possible escape.

Design rule:

- Combat should be concise and consequence-rich, not a long realtime battle.

### Stance

Player’s high-level combat approach.

Examples:

- aggressive;
- balanced;
- evasive;
- boarding;
- disabling;
- escape.

### Targeting

Combat focus.

Examples:

- engines;
- weapons;
- shields;
- cargo;
- drones;
- crew.

### Hull

Ship structural integrity.

### Shield

Damage buffer, module-driven.

### Module Damage

A state where installed ship equipment is disabled or degraded.

### Crippled

A ship state where normal travel/combat is impossible until repair, tow, or rescue.

### Escape Pod

Recovery fiction for severe loss.

Design purpose:

- preserve captain progression while making failure meaningful.

### Insurance

A recovery system that mitigates catastrophic loss at credit/favor/reputation cost.

### Wreck Creation

The process by which destroyed/crippled ships or NPC events create future salvage sites.

### Trap

A deployed hazard triggered by future travel or action.

Examples:

- mine;
- EMP mine;
- decoy beacon;
- interdiction buoy;
- cargo leech;
- sentry drone.

### Mine

A common Trap that damages, disables, or marks ships moving through a Place or Route.

### Drone

A deployable or module-supported automated unit.

Uses:

- combat;
- scanning;
- mine sweeping;
- decoying;
- salvage assistance;
- outpost defense.

### Patrol

Lawful or faction enforcement NPC presence.

Effects:

- inspections;
- pirate suppression;
- contraband risk;
- bounty interactions.

### Pirate Pressure

A regional/route hazard level representing pirate activity.

### Customs

Law enforcement check for contraband, heat, permits, or faction hostility.

---

## 10. Salvage and Exploration Terms

### Salvage

Recovering value from wrecks, derelicts, debris, anomalies, or abandoned stockpiles.

### Boarding

Entering a Derelict local map.

### Local Map

An ASCII map distinct from the galaxy graph.

Uses:

- derelict interiors;
- station interiors if needed;
- special outpost events.

### Room

A local-map node or tile area inside a Derelict.

Examples:

- bridge;
- cargo bay;
- engine room;
- AI core;
- med pod;
- vault;
- drone nest.

### Hazard

Any dangerous condition attached to Place, Route, Room, Contract, or cargo.

Examples:

- radiation;
- mines;
- pirates;
- customs;
- reactor leak;
- parasites;
- faction patrol;
- anomaly surge.

### Black Box

A high-value salvage objective containing evidence, intel, or contract proof.

### Relic

A rare weird artifact from precursor, anomaly, or cult content.

Relic design rule:

- Relics should create temptation and strange tradeoffs, not just high-value loot.

### Anomaly Diver

A captain/career style focused on high-risk anomaly exploration.

---

## 11. Messaging and History Terms

### Notice

A durable in-game message to a player, faction, corp, place, or global audience.

Implementation source:

- Foglet game-kit v3 notices/mail primitive.

Notice types:

- inbox mail;
- faction dispatch;
- contract update;
- bounty result;
- combat summary;
- NPC message;
- daily intel;
- outpost report.

### Mail

Player-facing subset of Notices that appears in inbox form.

### Event

Append-only world-history record.

Implementation source:

- Foglet game-kit v2 event log primitive.

Event examples:

- market action;
- route discovery;
- faction goal progress;
- bounty completion;
- derelict cleared;
- outpost raided.

### News

Player-facing summary of selected Events and generated world state.

### Rumor

Unverified information, possibly true, stale, partial, false, or planted.

### Daily Intel Packet

A login-time summary containing notices, rumors, local market highlights, route warnings, and major Events.

### Memorial

A special text artifact for ship loss, NPC death, season finale, or major world event.

---

## 12. UI and Interaction Terms

### Screen

A major terminal UI state.

Examples:

- dashboard;
- market;
- travel;
- cargo;
- shipyard;
- notices;
- derelict map;
- combat.

Implementation source:

- Foglet game-kit v1 Screen trait and screen stack.

### Modal

A temporary overlay for confirmation, details, warning, or result.

### Prompt

A text-first interaction with choices and hotkeys.

Implementation source:

- Foglet game-kit v1.1 prompt primitives.

### Hotkey

A single key that selects a choice.

Design rule:

- Common BBS actions should always have hotkeys.

### Disabled Choice

A visible but unavailable option with an explanation.

Examples:

- insufficient fuel;
- cargo full;
- standing too low;
- route unknown;
- heat too high.

### Any-Key Pause

A dramatic or informational pause waiting for input.

Uses:

- salvage reveal;
- combat result;
- death/recovery;
- major world news.

### Dashboard

The main at-a-glance screen showing captain, ship, turns, location, alerts, and menu actions.

### Star Chart

Player-facing route/Place recall UI.

### Manifest

The ship cargo view.

Do not confuse with Foglet door manifest.

### Door Manifest

Foglet installation metadata for launching the game as an external PTY door.

---

## 13. Foglet and Implementation Terms

### Foglet Context

Runtime identity and terminal metadata provided to external doors.

Used for:

- player record lookup;
- username/handle;
- role/security flavor;
- terminal size baseline.

### External PTY Door

A Foglet-launched terminal program using PTY handoff.

Drop Dead Nebula targets this runtime.

### Per-User Save

Private save file for user-specific game state.

Implementation source:

- Foglet game-kit v1/v2.1 save primitives.

### Shared World DB

Game-owned SQLite database for shared persistent state.

Implementation source:

- Foglet game-kit v2 world DB primitives.

### Atomic Transfer

A transaction that debits one inventory owner and credits another, rolling back on failure.

Implementation source:

- Foglet game-kit v4 inventory transfer primitive.

### Transaction Boundary

A set of mutations that must succeed or fail together.

Examples:

- spend turn + move + append event;
- buy cargo + debit credits + update market;
- complete bounty + transfer reward + append notice;
- salvage item + mark room looted + damage ship.

### Bounded Text

Player-authored text with maximum length and terminal-safe sanitization.

Implementation source:

- Foglet game-kit v3 privacy/moderation guidance.

### Ralph Loop

Autonomous implementation loop terminology from the game-kit specs. Not a player-facing term.

---

## 14. Content Authoring Terms

### Content Key

A stable string identifier for authored content.

Examples:

- `commodity.med_gel`;
- `place.ash_coil`;
- `route.ash_coil.mercy_relay`;
- `faction.mourning_union`;
- `npc.kara_vex`.

### Display Name

Human-readable name shown in UI.

### Flavor Text

Non-mechanical prose shown to players.

### Mechanical Effect

Structured rule impact.

Examples:

- price modifier;
- faction standing change;
- turn cost;
- hazard chance;
- inventory transfer.

### Seed Data

Initial world content inserted during world creation.

### Procedural Content

Generated content using tables, weights, seeds, or simulation.

### Landmark

An authored Place intended to be memorable and stable.

### Generated Place

A place created from generator rules rather than explicit authoring.

### Event Table

A weighted set of possible events.

Examples:

- travel event table;
- dockside event table;
- salvage event table;
- smuggling complication table.

---

## 15. Canonical Term Table

| Canonical Term | Avoid / Use Carefully | Notes |
| --- | --- | --- |
| Place | node, system | Generic graph node. |
| Sector | place, node | Player-facing navigation term. |
| Route | edge, warp | Directed graph connection. |
| Station | port | Station is broader; port is market-focused. |
| Outpost | planet, base | Use planet only for planet-like sites. |
| Daily Turn | action point | Turn is the BBS idiom. |
| World Tick | background loop | No long-lived daemon required. |
| Notice | message | Durable in-game mail/notice. |
| Event | log message | Append-only world history. |
| Contract | mission, quest | Structured job. |
| Bounty | mission | Target-oriented lifecycle. |
| Challenge | duel | Async contest lifecycle. |
| Commodity | good | Stackable trade item. |
| Cargo | inventory | Ship-carried goods. |
| Inventory Slot | cargo | Implementation storage primitive. |
| Heat | wanted level | Heat can be broader than wanted status. |
| Recall | map memory | Player-specific place memory. |
| Presence | location | Implementation current-place record. |

---

## 16. Open Vocabulary Questions

1. Should the player-facing navigation unit be called **Sector** everywhere, even when the implementation uses Place?
2. Should currency be plain **credits**, or a more flavorful term like **scrip**?
3. Should outpost ownership use **Charter** as the solo/corp abstraction?
4. Should **Heat** be global only at first, with regional/faction heat later?
5. Should **Contract** and **Bounty** remain separate in UI, or appear under one Job Board?
6. Should **NPC Captain** be shortened to **Captain** in content, or would that confuse player/captain language?
7. Should **Region** be player-visible from day one, or mostly internal for simulation?
8. Should **Season** be a real feature or just future sysop vocabulary?

---

## 17. Working Vocabulary Recommendation for MVP

For the first playable slice, use only these terms in code/content unless a new term is necessary:

- Captain;
- Ship;
- Place;
- Sector;
- Route;
- Station;
- Port;
- Commodity;
- Cargo;
- Market;
- Contract;
- Event;
- Notice;
- Daily Turn;
- World Tick;
- Recall;
- Presence;
- Heat.

Defer these until later slices:

- Corporation;
- Colony;
- Relic;
- Challenge;
- Faction Front;
- Charter;
- Dead Gate;
- Season Phase.
