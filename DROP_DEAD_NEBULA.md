# Drop Dead Nebula — Game Design Document

Status: Draft GDD v0.1  
Target platform: Foglet `:external_pty` BBS door via `foglet-game-kit-rs`  
Genre: async space trading, salvage, smuggling, faction-war, colony, and rivalry sim  
Design bias: ambitious BBS-native systems depth; satisfying solo play; async multiplayer as enrichment, not dependency

---

## 1. One-Sentence Pitch

**Drop Dead Nebula** is a terminal-first BBS space sim where each caller spends limited daily turns as a salvage captain in a collapsing frontier nebula: trading volatile goods, exploring directed sector routes, boarding derelicts, building outposts, joining factions, fighting pirates, manipulating markets, leaving traps and mail for rivals, and watching a shared galaxy evolve through NPC activity and durable world ticks even when the BBS population is tiny.

---

## 2. Design Pillars

1. **BBS-native async, not pseudo-MMO realtime**  
   No live sockets, no long-lived game server, no requirement that two players be online together. The world changes through daily turns, persisted actions, notices, event logs, market stockpiles, NPC ticks, and delayed consequences.

2. **Solo-first living galaxy**  
   A one-player BBS must still feel alive. Named NPC captains, faction fronts, market drift, procedural contracts, rotating crises, derelicts, patrols, pirates, rumors, and world ticks create a full single-player campaign loop.

3. **Economic play is strategy, not grinding**  
   Profits depend on information, risk, faction policy, route hazards, stock scarcity, heat, ship build, timing, and player/NPC interference. Repeating the same route forever burns it down.

4. **Information is a resource**  
   Stale route recall, market rumors, scans, faction intelligence, player messages, false leads, and NPC gossip are as important as credits.

5. **Everything leaves a scar**  
   Trade affects prices. Failed expeditions leave wrecks. NPCs remember grudges. Factions advance clocks. Bounties expire. Routes destabilize. Outposts consume supplies. Mines and decoys wait for future callers.

6. **Multiple careers, no single golden path**  
   Trader, smuggler, salvager, explorer, privateer, faction agent, colony founder, market manipulator, pirate, bounty hunter, and corporate logistics chief should all be viable.

7. **Terminal drama**  
   The game should feel great in 80x24: compact dashboards, hotkey menus, plain-text reports, ASCII sector maps, modal prompts, clear consequences, and satisfying “press any key” reveals.

---

## 3. Inspirations and Non-Clone Boundary

Drop Dead Nebula borrows broad genre DNA from classic BBS space-trading games, especially the TradeWars tradition: sector graphs, ports, commodities, turns, ships, mines, fighters/drones, planets/outposts, corporations, pirates, and territorial ambition.

It should not be a clone. Its signature differences:

- **No realtime server dependency.** Simulation advances through lazy or cron-triggered world ticks.
- **Solo resiliency is a first-class requirement.** NPC captains and faction systems are not filler; they are the base population.
- **Derelict boarding and salvage are major verbs.** Local ASCII maps complement the galaxy graph.
- **Faction projects reshape the world.** Players and NPCs complete shared goals that alter routes, markets, and station services.
- **Intel decay and misinformation matter.** A discovered route or price is not eternally reliable.
- **Combat favors consequences over deletion.** Crippling, escape pods, insurance, ransom, cargo loss, and wreck creation are usually better than hard player erasure.
- **Bounded player-authored text is built in.** Mail, dead drops, rumors, bounty blurbs, and memorials are short, safe, terminal-friendly artifacts.

---

## 4. Player Fantasy

The player is a captain of a half-legal salvage vessel operating in the **Drop Dead Nebula**, a region where the old empire’s relay network collapsed and left behind wrecks, quarantined colonies, smugglers, corporate claim-jumpers, machine cults, and hungry stations.

The player should feel like they are:

- limping into port with illegal cargo and one turn left;
- finding a dead route that was profitable yesterday;
- reading a notice that an NPC rival undercut them;
- risking a derelict boarding action because the black box might complete a bounty;
- placing a decoy beacon in a dangerous sector for future prey;
- funding a faction relay that permanently opens a safer route;
- becoming known as a trader, pirate, marshal, relic diver, or disaster magnet;
- logging off after leaving the galaxy slightly changed.

---

## 5. Platform and Foglet Assumptions

Drop Dead Nebula is designed specifically for the Foglet game-kit roadmap:

- runs as a packaged `:external_pty` door;
- uses safe terminal setup/restore;
- assumes 80x24 baseline readability;
- uses Foglet context for local player identity;
- stores per-player private state in per-user save files where appropriate;
- stores shared world state in a game-owned SQLite world DB;
- uses daily turns, event logs, leaderboards, notices, challenges, markets, factions, bounties, spatial graph, presence, place recall, owner-keyed inventory, and world ticks;
- explicitly does not use realtime multiplayer or direct Foglet DB access.

---

## 6. Core Session Loop

### 6.1 Normal Daily Session, 5–15 Minutes

1. **Login / resume captain**
   - Load Foglet identity.
   - Resolve player record.
   - Run bounded lazy world tick catch-up if needed.
   - Deliver daily intel packet.

2. **Read the board**
   - Unread notices.
   - Local station news.
   - Bounties and contracts.
   - Market highlights.
   - Route warnings.
   - Recent world events.

3. **Choose today’s intent**
   - Safe profit run.
   - Risky smuggling job.
   - Salvage expedition.
   - Faction contribution.
   - Bounty hunt.
   - Exploration / survey.
   - Outpost management.
   - Revenge / trap / market manipulation.

4. **Spend limited turns**
   - Move between places.
   - Dock and trade.
   - Scan and update intel.
   - Board derelicts.
   - Fight, flee, bribe, hide, or negotiate.
   - Transfer cargo and complete jobs.

5. **Resolve consequences**
   - Credits, cargo, damage, heat, faction standing.
   - Event log entries.
   - NPC reactions.
   - Market shifts.
   - Notices to self or others.
   - Leaderboard updates.

6. **Quit cleanly**
   - Save private captain state.
   - Leave presence at current place.
   - Return safely to Foglet.

### 6.2 Low-Population Guarantee

Every session should offer at least:

- one low-risk profit opportunity;
- one dangerous temptation;
- one story hook;
- one world-state consequence from prior ticks/actions;
- one chance encounter or rotating local incident;
- one visible long-term progression path.

This guarantee should be implemented through contract generation, NPC action summaries, market drift, and local station events rather than relying on other humans.

---

## 7. Long-Term Progression Loops

### 7.1 Captain Progression

Players develop reputation tracks through action, not class selection alone:

- **Trader**: better margins, market forecasts, bulk contracts, convoy discounts.
- **Smuggler**: false compartments, bribe networks, black-market access, heat decay.
- **Salvager**: better extraction odds, derelict maps, rare part recovery, hazard mitigation.
- **Explorer**: route discovery, anomaly handling, recall upgrades, hidden gate access.
- **Privateer**: interdiction, boarding bonuses, bounty access, intimidation.
- **Faction Agent**: privileged missions, restricted ports, political protection.
- **Colonist / Industrialist**: outpost production, stockpile management, station services.
- **Pirate**: ambush tools, fences, fear reputation, but more patrol heat.

These tracks can hybridize. The game should avoid permanent class lock-in, but choices should create identity by week two.

### 7.2 Ship Progression

Ships are playstyle platforms, not linear stat sticks.

Base hull families:

| Hull | Fantasy | Strengths | Weaknesses |
| --- | --- | --- | --- |
| Courier | information runner | fast, efficient, stealthy | low cargo, fragile |
| Mule | early trader | cheap cargo capacity | weak defenses |
| Prospector | salvage specialist | scan/salvage bonuses | mediocre combat |
| Raider | pirate/privateer | boarding, ambush, intimidation | heat, poor legal access |
| Gunship | bounty/combat | weapons, armor | fuel hungry, low profit margins |
| Tender | support/logistics | drones, repairs, escort tasks | expensive upkeep |
| Colony Ark | strategic builder | outposts, bulk supplies | slow, vulnerable |
| Ghost Clipper | elite smuggler | stealth, speed, black-market perks | limited armor |
| Relic Skiff | anomaly diver | weird tech compatibility | unpredictable maintenance |

Modules:

- cargo pods;
- shield banks;
- afterburners;
- long-range scanners;
- stealth baffles;
- false holds;
- salvage claws;
- hull cutters;
- drone bay;
- mine layer;
- mine sweeper;
- boarding clamps;
- med bay;
- faction transponder;
- jump stabilizer;
- relic containment;
- repair printer;
- crew quarters;
- smuggler’s shrine / morale oddity.

### 7.3 World Progression

The nebula itself progresses through eras:

1. **Frontier Scramble** — small profits, safe contracts, map discovery.
2. **Route Wars** — factions fight over corridors, piracy rises, markets diverge.
3. **Relic Rush** — anomaly sites and derelict megastructures appear.
4. **Succession Crisis** — faction shared goals reshape control of whole regions.
5. **The Dead Gate Opens** — endgame routes, high-risk resources, persistent world finale choices.

A sysop can configure campaign pace, but the default should support a month-long season.

---

## 8. World Model

### 8.1 The Drop Dead Nebula

A generated or authored directed graph of places. The default world should be small enough to learn but large enough to hide secrets.

Recommended default:

- 72–120 places for a normal BBS season;
- 6–10 regions;
- 8–12 safe or semi-safe hub stations;
- 20–40 ordinary route nodes;
- 12–20 dangerous anomaly/wreck nodes;
- 6–12 faction strongholds or contested sites;
- 6–10 hidden or unlockable places.

### 8.2 Place Types

- **Relay Station** — services, mail, market, contracts.
- **Free Port** — market hub, black-market risk, player messages.
- **Mining Rock** — ore supply, industrial contracts, pirate targets.
- **Agri Cylinder** — food supply, humanitarian crises.
- **Refinery** — fuel and industrial goods.
- **Shipyard** — repairs, modules, hull sales.
- **Faction Enclave** — special missions, restricted services.
- **Wreck Field** — salvage and danger.
- **Derelict Interior Entry** — local ASCII boarding map.
- **Anomaly Pocket** — rare events, route instability, relics.
- **Dead Gate** — locked/endgame route node.
- **Pirate Den** — fences, ambushes, illicit jobs.
- **Quarantine Zone** — high medical demand, access restrictions.
- **Hidden Cache** — owner-keyed inventory stash.
- **Outpost / Colony** — player, NPC, faction, or system-owned production site.

### 8.3 Directed Routes

Routes are not just edges; they are gameplay objects.

Route metadata:

- distance / turn cost;
- kind: lane, tunnel, drift, wormhole, smuggler path, military corridor;
- hazard: mines, pirates, radiation, customs, anomaly, debris;
- requirements: module, faction standing, route key, recent scan, bribe;
- stability;
- public/private/hidden visibility;
- last-known intel snapshot.

Examples:

- **Ash Coil -> Mercy Relay**: normal lane, low cost, customs risk.
- **Mercy Relay -> Red Maw**: one-way nebula current, unstable.
- **Saint Vex -> Blackglass Cache**: hidden route requiring rumor or scan.
- **Dead Gate -> Throne of Static**: endgame route unlocked by faction project.

### 8.4 Place Recall and Fog of War

Each player maintains recall records:

- first seen;
- last seen;
- last-known market highlights;
- last-known route hazards;
- contacts discovered;
- local faction influence;
- personal notes / tags;
- whether data is stale.

Recall should not auto-update everywhere. Scanning, visiting, buying intel, or receiving notices updates it. Old information can be wrong.

---

## 9. Turn and Action Economy

### 9.1 Daily Turns

Default values:

- base daily turns: 30;
- reserve cap: 60, allowing one missed day to carry over;
- emergency turns: rare consumables from events, modules, or favors;
- sysop-configurable season presets: casual, classic, brutal.

### 9.2 Action Costs

| Action | Cost | Notes |
| --- | ---: | --- |
| Move along normal route | 1 | modified by hull/module |
| Move dangerous/long route | 2–3 | may trigger encounter |
| Dock / undock | 0 | unless under blockade |
| Trade at known market | 0–1 | configurable; trading should not consume too much friction |
| Scan route/place | 1 | updates recall |
| Salvage attempt | 1 | local derelict actions may cost more |
| Combat round | 1 | discourages infinite fights |
| Bribe / negotiation | 1 | sometimes avoids larger loss |
| Deploy mine/trap | 1 | plus inventory cost |
| Clear mines | 1 | plus risk |
| Faction mission action | 1–3 | based on mission |
| Outpost construction step | 2–5 | strategic investment |

### 9.3 Why Turns Matter

Turns create BBS pacing:

- strategic route planning;
- meaningful risk decisions;
- natural session length;
- daily anticipation;
- fairer async competition;
- player stories around “one turn left.”

---

## 10. Economy

### 10.1 Commodity Families

**Staples**
- water;
- food paste;
- med gel;
- oxygen candles;
- refugee berths.

**Industrial**
- ore;
- alloys;
- machine parts;
- reactor coolant;
- sealant foam;
- station glass.

**Technology**
- nav cores;
- sensor wafers;
- drone brains;
- relay coils;
- shield emitters.

**Illicit / Restricted**
- black AI fragments;
- narcotic hymns;
- forged transponders;
- weapon crates;
- illegal clones;
- unlicensed relics.

**Strategic**
- terraforming kits;
- colony seedbanks;
- defense grids;
- fuel catalysts;
- jump-gate anchors.

**Relic / Exotic**
- dead-star amber;
- precursor keys;
- choir metal;
- memory fossils;
- bottled void.

### 10.2 Port Archetypes

Each station or outpost has supply/demand tendencies.

| Port | Produces | Demands | Risks |
| --- | --- | --- | --- |
| Mining | ore, crystals | food, parts, med gel | cave-ins, pirates |
| Agri | food, organics | machine parts, water | famine, quarantine |
| Refinery | fuel, coolant | ore, labor | explosions, strike |
| Military | weapons, contracts | fuel, drones, intel | inspections |
| Black Market | contraband | relics, forged goods | betrayal, heat |
| Tech Shrine | nav cores, relic modules | artifacts, sensor wafers | cult politics |
| Frontier | mixed scarcity | everything | unstable prices |
| Shipyard | hulls/modules | alloys, credits | sabotage, queues |

### 10.3 Pricing Drivers

Prices react to:

- current stock quantity;
- equilibrium stock;
- recent player and NPC trades;
- faction control;
- route safety;
- blockade/quarantine/war state;
- contract demand;
- world ticks;
- rumors and false rumors;
- season phase;
- hidden station traits.

### 10.4 Anti-Farming Measures

- diminishing margins on repeated same-route loops;
- NPC competitors notice profitable lanes;
- customs/pirate heat rises on overused routes;
- station demand normalizes through ticks;
- contracts expire;
- cargo spoilage for some goods;
- fuel and maintenance costs increase with ship size.

---

## 11. Contracts, Jobs, and Bounties

### 11.1 Contract Board

Every active hub should offer a mix:

- 2 safe low-pay contracts;
- 2 medium-risk profitable contracts;
- 1 high-risk/high-story contract;
- 1 faction-specific offer;
- occasional rare chain starter.

Types:

- legal freight;
- courier packet;
- passenger evacuation;
- black-market delivery;
- station emergency supply;
- route survey;
- derelict black-box recovery;
- pirate cache hunt;
- escort simulation / route clearing;
- mine sweeping;
- faction sabotage;
- humanitarian crisis;
- relic containment;
- debt collection.

### 11.2 Bounties

Bounties can be system, faction, NPC, or player posted.

Bounty states:

- open;
- claimed;
- completed;
- failed;
- expired;
- contested.

Examples:

- recover the **Black Glass Recorder** from a wreck;
- scan the **Red Maw** anomaly after a flare;
- clear three pirate decoys from a corridor;
- deliver med gel to a quarantined cylinder;
- find the NPC captain who stole a faction relic;
- intercept a smuggler convoy;
- contribute 40 fuel catalysts to a relay repair goal.

### 11.3 Async Challenges

Challenges are opt-in, asynchronous, and resolved by future actions.

Examples:

- “Beat my profit on the Ash Coil loop by tomorrow.”
- “First to retrieve a black box from Wreck 77.”
- “Faction courier race: deliver sealed packets to three stations.”
- “Privateer duel: resolve against standing combat policies.”
- “Salvage draft: both players enter the same generated derelict seed; highest value extraction wins.”

---

## 12. NPC Simulation

### 12.1 Why NPCs Are Mandatory

Low-pop BBS play fails if the galaxy waits for humans. NPCs provide:

- market movement;
- competition;
- rumors;
- combat threats;
- recurring personalities;
- faction momentum;
- contracts and consequences;
- revenge and friendship arcs.

### 12.2 NPC Tiers

**Background flows**  
Abstract cargo, patrol, pirate, and refugee movement. These are not individual entities; they modify markets, route hazards, and event logs.

**Named captains**  
Persistent lightweight actors with location, ship, faction, wealth tier, goals, grudges, and memories.

**Major powers**  
Factions and station authorities that advance strategic clocks.

### 12.3 Named NPC Examples

- **Kara Vex, “The Polite Knife”** — smuggler broker who remembers unpaid favors.
- **Marshal Ivo Senn** — lawful hunter who escalates against pirates and contraband runners.
- **Moth-9** — salvage android obsessed with precursor wrecks.
- **Mother Lumen** — cult emissary buying relics at irrational prices.
- **Brass Jory** — loud merchant prince who floods profitable markets.
- **The Weeping Corsair** — pirate captain who ransoms rather than kills.
- **Sister Static** — relay AI fragment that sends cryptic notices.
- **Captain Oxblood** — mercenary convoy chief who can become ally or rival.

### 12.4 NPC Daily Tick Behavior

Each named NPC can:

1. choose goal based on archetype and current state;
2. evaluate routes and risks;
3. perform abstract movement/trade/fight/salvage;
4. mutate stock, hazard, or faction pressure;
5. emit traces: rumor, event log, contract, notice, wreck, bounty;
6. update memories toward players.

NPCs do not need full pathfinding. They can use adjacency, region tags, weighted choices, and scripted goals.

### 12.5 NPC Memory

NPCs should remember small facts:

- player helped/harmed them;
- player undercut route;
- player stole salvage claim;
- player paid debt;
- player ignored distress call;
- player joined enemy faction;
- player repeatedly smuggles through their turf.

Memory generates notices, discounts, ambushes, and dialog changes.

---

## 13. Factions and Corporations

### 13.1 Major Factions

**Mourning Union**  
Labor, repair, refugees, reconstruction.  
Bonuses: repair discounts, humanitarian contracts, station trust.  
Conflicts: hates pirates and exploitative cartel behavior.

**Helix Cartel**  
Trade monopolists, smugglers, market manipulators.  
Bonuses: black-market access, arbitrage intel, bribes.  
Conflicts: targeted by authorities and union radicals.

**Port Authority Black Office**  
Lawful regulators with secret methods.  
Bonuses: patrol protection, legal bounties, inspection immunity windows.  
Conflicts: restricts contraband and piracy.

**Saints of Vacuum**  
Mystic relic cult and anomaly divers.  
Bonuses: anomaly access, relic interpretation, strange modules.  
Conflicts: distrusted by rational factions.

**Machine Concord**  
Post-human logistics intelligence.  
Bonuses: drones, automation, efficient production.  
Conflicts: cold reputation, weird demands.

**Dust Clans**  
Pirates, free raiders, survivalist fleets.  
Bonuses: ambush tools, fences, fear reputation.  
Conflicts: hunted by lawful factions.

### 13.2 Shared Goals

Shared faction goals are persistent projects:

- repair a relay;
- build a shield over a famine station;
- stabilize an anomaly corridor;
- blockade a pirate den;
- evacuate a doomed colony;
- construct a deep-nebula beacon;
- unlock faction shipyards;
- open or seal the Dead Gate.

Players and NPCs contribute goods, credits, completed contracts, scans, or combat victories. Completion changes world state.

### 13.3 Player Corporations

If population allows, players can form corporations. If population is low, NPC-backed corporations still exist.

Player corp features:

- shared bank;
- shared depots;
- corp notices;
- route notes;
- faction alignment;
- role permissions;
- pooled bounty board;
- outpost ownership;
- corp leaderboard.

Solo fallback: a player can create a one-captain charter with NPC crew support, so corp mechanics are not locked behind population.

---

## 14. Combat and Conflict

### 14.1 Combat Philosophy

Combat should be readable, risky, and decisive without being a tedious tactical minigame. It should produce stories and consequences more often than irreversible deletion.

### 14.2 Encounter Types

- travel ambush;
- customs interdiction;
- pirate toll;
- bounty intercept;
- mine detonation;
- derelict boarding hazard;
- port blockade;
- convoy defense;
- faction skirmish;
- async duel/challenge.

### 14.3 Combat Choices

At encounter start, present compact choices:

- `(F)` Fight;
- `(R)` Run;
- `(B)` Bribe / bargain;
- `(H)` Hide / spoof transponder;
- `(S)` Surrender cargo;
- `(E)` Use special module;
- `(D)` Deploy drone/mine;
- `(V)` View odds.

If fighting, choose stance:

- aggressive;
- balanced;
- evasive;
- boarding;
- disabling;
- all-out escape.

Targeting:

- engines;
- weapons;
- shields;
- cargo;
- drones;
- morale/crew.

### 14.4 Consequence Ladder

Instead of binary death:

1. minor hull damage;
2. cargo loss;
3. fuel leak;
4. module damage;
5. crew injury;
6. forced surrender/bribe;
7. ship crippled and towed;
8. escape pod rescue;
9. wreck created for future salvage;
10. rare true ship loss.

### 14.5 Async PvP Policies

Players can configure standing policies:

- avoid player conflict;
- defend only;
- attack wanted captains;
- attack faction enemies;
- raid cargo above threshold;
- never fire in safe zones;
- auto-flee below hull threshold.

When another player triggers an interaction offline, the system resolves using these policies and sends summaries via notices.

---

## 15. Mines, Traps, Drones, and Area Denial

Async traps are a major BBS strategy layer.

Deployables:

- fragmentation mines;
- EMP mines;
- sensor ghosts;
- decoy wreck beacons;
- interdiction buoys;
- cargo leeches;
- hidden sentry drones;
- smuggler dead drops;
- false distress calls;
- route toll markers.

Properties:

- owner-kind / owner-id;
- faction friendliness;
- trigger conditions;
- detection difficulty;
- decay timer;
- legal status;
- event-log verbosity.

Counterplay:

- scans;
- sweepers;
- sacrificial drones;
- faction patrols;
- route-clearing bounties;
- stealth modules;
- intel purchases;
- NPC rumors.

Trap chains can create emergent stories:

> A decoy wreck pulls a captain into a dead-end, an EMP mine strips shields, a pirate drone marks them, and a bounty hunter intercepts on the next route.

---

## 16. Salvage and Derelict Boarding

### 16.1 Salvage Role

Salvage is the game’s signature alternative to pure trading. It adds risk, exploration, local maps, narrative, and rare loot.

### 16.2 Derelict Flow

1. Discover wreck through scan, rumor, contract, random event, or aftermath.
2. Decide whether to spend turns boarding.
3. Enter local ASCII map.
4. Explore rooms with hazards and loot.
5. Make extraction choices under cargo/turn/danger pressure.
6. Return to ship or get forced out.
7. Wreck persists, decays, or becomes stripped.

### 16.3 Derelict Room Types

- airlock;
- cargo bay;
- engineering;
- bridge;
- med pod;
- crew quarters;
- AI core;
- reactor room;
- chapel / shrine;
- sealed vault;
- drone nest;
- black box compartment.

### 16.4 Salvage Hazards

- radiation leak;
- unstable reactor;
- hull collapse;
- dormant defense drone;
- pressure loss;
- parasite spores;
- pirate claim beacon;
- cursed relic effect;
- time/turn sink;
- moral dilemma survivor pod.

### 16.5 Salvage Rewards

- scrap;
- modules;
- cargo;
- black boxes;
- route intel;
- faction secrets;
- relic fragments;
- NPC memory hooks;
- bounty evidence;
- derelict maps.

---

## 17. Outposts, Colonies, and Planets

### 17.1 Ownership

Outposts may be owned by:

- system;
- NPC;
- faction;
- player;
- corporation.

### 17.2 Outpost States

- claim beacon;
- hidden cache;
- rough dock;
- fortified depot;
- refinery outpost;
- colony seed;
- industrial colony;
- fortress world;
- relic sanctuary;
- dying settlement.

### 17.3 Outpost Gameplay

Players can:

- stockpile goods;
- set buy/sell policies;
- build defenses;
- produce commodities;
- repair/refit;
- post local contracts;
- restrict access;
- declare faction alignment;
- hide contraband;
- install route beacons.

### 17.4 World Tick Integration

Ticks handle:

- production;
- consumption;
- morale;
- raids;
- decay;
- construction progress;
- stock drift toward equilibrium;
- event generation.

---

## 18. Notices, Mail, Rumors, and Player Text

### 18.1 Notice Types

- system daily intel;
- faction dispatch;
- contract update;
- bounty result;
- market alert;
- NPC message;
- player mail;
- trap triggered report;
- combat summary;
- outpost report;
- leaderboard milestone;
- debt warning.

### 18.2 Player-Authored Text

Allowed short text artifacts:

- mail body;
- dead drop label;
- bounty blurb;
- corp bulletin;
- outpost motto;
- ship epitaph;
- rumor submission if enabled.

Rules:

- length bounded;
- terminal sanitized;
- no rich formatting;
- moderation-friendly logs;
- clear block/report affordances if supported by sysop tooling later.

### 18.3 Rumor Truth Model

Rumors may be:

- true;
- outdated;
- partly true;
- false but useful;
- planted by NPC;
- planted by player;
- faction propaganda.

This makes intel verification valuable.

---

## 19. Random Events

### 19.1 Travel Events, d12

1. Quiet passage; no event.
2. Merchant convoy spotted; gain market intel or attempt trade.
3. Customs ping; submit, bribe, spoof, or run.
4. Distress beacon; aid, ignore, exploit, or investigate.
5. Ion squall; lose fuel, spend turn to stabilize, or risk damage.
6. Pirate shadow; evade, pay toll, fight, or lure into trap.
7. Debris cloud; minor salvage chance.
8. Patrol inspection; faction standing modifies outcome.
9. Refugee flotilla; humanitarian contract appears.
10. Wreck signature; optional salvage site.
11. Hidden shortcut discovered; temporary route recall update.
12. Anomaly flare; rare loot, route mutation, or ship damage.

### 19.2 Dockside Events, d12

1. Broker offers discounted legal freight.
2. Informant sells route intel.
3. Crew trouble; pay credits, spend turn, or lose morale.
4. Black market opens for one session.
5. Crackdown raises inspection risk.
6. Faction recruiter offers job.
7. Debt collector appears.
8. Auction lot: damaged rare module.
9. News of war spikes munitions prices.
10. Dock accident creates emergency supply contract.
11. Rival captain undercuts local market.
12. Strange relic buyer seeks discreet seller.

### 19.3 Salvage Events, d12

1. Empty husk; minimal scrap.
2. Standard salvage yield.
3. Spare parts cache.
4. Data core with rumor unlock.
5. Trapped compartment.
6. Survivor pod moral dilemma.
7. Pirate claim beacon.
8. Unstable reactor timed extraction.
9. Faction-tagged military wreck.
10. Rare prototype fragment.
11. Precursor signature.
12. False wreck signal; ambush.

### 19.4 Smuggling Complications, d10

1. Manifest mismatch.
2. Sniffer drone sweep.
3. Contact moved meeting point.
4. Rival tipped customs.
5. Cargo degrades in transit.
6. Buyer offers double for detour.
7. Hidden stowaway.
8. False compartment fails.
9. Patrol demands bribe.
10. Package is hotter than promised.

### 19.5 Faction Flashpoints, d8

1. Trade embargo.
2. Border skirmish.
3. Pirate letters-of-marque issued.
4. Religious pilgrimage surge.
5. Strike on mining station.
6. Plague quarantine.
7. Relic excavation frenzy.
8. Assassination triggers martial law.

### 19.6 Low-Population Special Events

These fire more often if human activity is low:

- NPC merchant opens temporary arbitrage route;
- derelict megaship drifts into known space;
- faction posts subsidized solo contract;
- station emergency creates guaranteed demand;
- named NPC sends personal challenge;
- old route becomes unstable and reveals shortcut;
- abandoned player/NPC wreck becomes salvageable;
- machine courier misdelivers valuable intel.

---

## 20. UI and Screen Design

### 20.1 Main Screens

- Title / resume screen.
- Captain dashboard.
- Current location screen.
- Travel / route screen.
- Market screen.
- Cargo manifest.
- Ship status / repair / refit.
- Contract and bounty board.
- Notices / mail inbox.
- Faction screen.
- Leaderboards.
- Recent world events.
- Place recall / star chart.
- Derelict boarding map.
- Combat encounter.
- Outpost management.
- Settings / help / quit.

### 20.2 Dashboard Sketch

```text
DROP DEAD NEBULA                                         Day 17
Captain: Vanta        Ship: Last Laugh        Turns: 12/30
Location: Ash Coil    Faction: Helix +3       Heat: WANTED-1
Hull: 82%             Fuel: 7                 Credits: 14,220

Local signals:
 ! Union convoy delayed near Mercy Relay
 $ Ash Coil buying Med Gel at +42%
 ? Wreck ping: DDN-44 / stale 2 days
 * Unread: 3 notices

(T)ravel  (M)arket  (J)obs  (S)hip  (I)nbox  (F)action
(C)argo   (R)ecall  (L)og   (H)elp  (Q)uit
```

### 20.3 UX Rules

- All common actions have hotkeys.
- Disabled choices must explain why.
- Dangerous choices require confirmation.
- Important results get short dramatic text blocks.
- No gameplay logs go to stdout outside TUI control.
- Every screen has an obvious quit/back path.
- Monochrome readability is required.

---

## 21. Data and Foglet Feature Mapping

### 21.1 Per-User Save

Private save fields:

- tutorial flags;
- UI preferences;
- local story flags;
- private captain notes;
- ship/captain convenience cache when not canonical world DB;
- pending modal/tutorial state.

### 21.2 Shared World DB

Shared tables/concepts:

- players;
- turns;
- world events;
- leaderboards;
- notices;
- challenges;
- markets/listings;
- factions/memberships/goals;
- bounties;
- places/routes;
- presence;
- place recall;
- owner-keyed inventory slots;
- world tick tasks;
- contracts;
- NPCs;
- route hazards;
- outposts;
- ship modules;
- rumors.

### 21.3 Owner-Keyed Inventory Uses

Owners:

- player ship;
- station market;
- faction depot;
- derelict container;
- hidden cache;
- outpost warehouse;
- NPC ship;
- corp depot;
- mission escrow.

Atomic transfers power:

- buying/selling;
- looting;
- depositing contributions;
- contract turn-ins;
- stealing cargo;
- salvage extraction;
- market listings;
- outpost production.

### 21.4 World Ticks

Tick tasks:

- daily turn reset;
- market restock/reprice;
- station consumption;
- contract expiry/generation;
- bounty expiry;
- faction project progress;
- route hazard drift;
- NPC captain actions;
- pirate pressure update;
- outpost production/decay;
- rumor generation;
- derelict decay;
- leaderboard seasonal summaries.

Ticks must be bounded. If the game is idle for weeks, catch up in summarized batches rather than simulating every tiny step.

---

## 22. Leaderboards and Achievements

Leaderboards:

- richest captains;
- trade profit this season;
- salvage value recovered;
- bounties completed;
- faction contribution;
- pirate notoriety;
- routes discovered;
- derelicts cleared;
- outpost value;
- longest survival streak;
- most wanted;
- most helpful humanitarian.

Achievements can be local game achievements, not Foglet profile badges:

- First Bloodless Profit — earn 10k without combat.
- Kissed Customs — pass inspection with contraband.
- Last Turn Legend — complete contract with zero turns left.
- Ghost Cartographer — discover five hidden routes.
- Debt Saint — rescue three distress calls for no reward.
- Friendly Firebrand — be loved by one faction and hunted by another.

---

## 23. Failure, Death, and Recovery

Failure should sting without ending the season.

Possible failure states:

- cargo lost;
- credits debt;
- ship damaged;
- module broken;
- crew injured;
- faction standing lost;
- heat increased;
- outpost raided;
- forced tow to safe port;
- insurance claim;
- escape pod rescue;
- wreck left behind.

True ship loss should be rare and dramatic. Even then, the player retains captain history, some contacts, debts, reputation scars, and maybe insurance payout.

---

## 24. Balancing and Anti-Staleness

### 24.1 Solved Route Prevention

- margin compression;
- route heat;
- NPC competition;
- fuel price changes;
- surprise inspections;
- pirate migration;
- contract expiry;
- station demand caps.

### 24.2 Low-Pop Boosts

If recent human activity is low:

- NPC trade increases;
- contracts favor solo completion;
- derelict discovery rate rises;
- faction goals scale down;
- market volatility rises;
- named NPCs send personal hooks;
- event log includes more NPC-authored activity.

### 24.3 High-Pop Safeguards

If population is high:

- stock replenishment and contract generation scale carefully;
- anti-monopoly pressure rises;
- more faction conflict zones open;
- player-authored text moderation boundaries matter more;
- market manipulation remains possible but not totalizing.

---

## 25. Example Day in Play

1. Captain logs in at Ash Coil with 30 turns.
2. Daily intel says Med Gel demand is high at Mercy Relay, but customs patrols increased.
3. Inbox includes a faction notice, an NPC debt warning, and a player challenge.
4. Player buys Med Gel, installs a cheap false hold, and takes the Mercy route.
5. Travel event: customs ping. Player spoofs transponder, succeeds but gains heat.
6. At Mercy Relay, market price is still high but stock is nearly satisfied by an NPC convoy.
7. Player sells half, keeps half for a better contract, and accepts a black-box bounty.
8. Player scans a nearby wreck field, boards a derelict, finds a survivor pod and a data core.
9. Cargo is full: disabled prompt explains they must jettison goods or leave the core.
10. Player saves survivor, loses profit opportunity, gains Mourning Union rep.
11. Event log records the rescue. NPC rival Moth-9 later claims the data core.
12. Player logs off with 3 turns, a damaged hull, a new Union contact, and a grudge.

---

## 26. Ambitious Scope Ladder

This is a dream-big GDD, but the design can be layered.

### Foundation Slice

- captain creation/resume;
- sector graph;
- travel;
- daily turns;
- 6 commodities;
- 4 port types;
- basic market;
- cargo inventory;
- event log;
- simple contracts;
- save-on-quit.

### Living Galaxy Slice

- world ticks;
- market restock/reprice;
- NPC merchants/pirates;
- random travel/dock events;
- bounties;
- leaderboards;
- notices.

### Signature Slice

- derelict boarding maps;
- salvage hazards;
- named NPC memory;
- factions/shared goals;
- route recall and stale intel;
- deployable mines/traps.

### Strategic Slice

- outposts/colonies;
- corporations;
- async challenges;
- faction wars;
- advanced ship modules;
- endgame Dead Gate campaign.

---

## 27. Feature Coverage Verdict

Drop Dead Nebula uses essentially the full Foglet game-kit feature surface:

- terminal runtime, screens, input, safe quit;
- per-user save;
- maps, dialogs, menus, inventory;
- prompt flows, hotkeys, disabled choices, confirmations, pauses;
- shared SQLite world;
- player registry and role flavor;
- daily turns;
- append-only event log;
- leaderboards;
- notices/mail;
- async challenges;
- shared market;
- factions and shared goals;
- bounties;
- spatial graph;
- player presence;
- place recall/fog-of-war;
- owner-keyed inventory and atomic transfers;
- durable world ticks.

The design does not merely use a majority of supported BBS game features; it is built to showcase almost all of them as player-facing systems.

---

## 28. Open Design Questions

1. Should the default world be generated, authored, or generated with authored landmarks?
2. How many turns per day feels right for Foglet’s actual session patterns?
3. Should trading consume turns, or should only movement/risk actions consume turns?
4. How punishing should piracy be on a small BBS?
5. Should player corporations launch in v1, or should NPC/faction structures come first?
6. How much NPC simulation is enough before it becomes opaque?
7. Should seasons reset, partially reset, or run indefinitely?
8. Should sysops have in-game admin screens for stuck markets/contracts?
9. How visible should exact formulas be to players?
10. How weird should relic/anomaly mechanics become before they undermine the trading core?

---

## 29. North Star

The north star is simple:

> A player with no one else online should still say, “One more run.”  
> A player with ten others active should say, “They changed my galaxy while I was gone.”

Drop Dead Nebula succeeds if every login produces a compact story: a route chosen, a risk taken, a system nudged, a message received, a scar left behind.
