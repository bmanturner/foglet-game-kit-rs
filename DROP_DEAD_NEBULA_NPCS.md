# Drop Dead Nebula — NPC Creation and AI Deep Dive

Status: Draft v0.1  
Primary milestone relevance: background flow M2, named NPCs M8, campaign actors M11  
Purpose: Define how NPCs make the nebula feel populated without requiring realtime multiplayer.

---

## 1. Design Intent

NPCs are the population floor. They keep the game alive on a quiet BBS and enrich async multiplayer when humans exist.

NPCs should:

- move Markets;
- create rumors;
- compete for salvage/jobs;
- issue Contracts, Bounties, and Challenges;
- represent Factions;
- remember the player;
- generate Events and Notices.

NPCs should not:

- fully simulate hundreds of ships;
- secretly invalidate player plans too often;
- dominate human action;
- spam names into every log line.

---

## 2. NPC Layers

### 2.1 Background Flows

Abstract activity: merchants, patrols, pirates, refugees, scavengers. No individual identity.

Milestone: M2.

### 2.2 Named NPC Captains

Persistent personalities with light state and memory.

Milestone: seeded as names M2/M4, playable M8, mature M11.

### 2.3 Station Contacts

NPCs attached to services: dock clerks, brokers, quartermasters, salvage desks.

Milestone: flavor M0, functional M5/M7.

### 2.4 Faction Representatives

Faces for Factions and shared goals.

Milestone: M7.

---

## 3. Initial Named Cast

| NPC | Archetype | First Role | Milestone |
| --- | --- | --- | --- |
| Brass Jory | merchant rival | undercuts trade routes | M2 seed, M8 mature |
| Moth-9 | salvage android | Blue Blind hooks | M4 seed, M8 mature |
| Marshal Ivo Senn | law/patrol | customs pressure | M3/M6 seed, M8 mature |
| Sister Static | relay AI | Daily Intel voice | M2 seed, M7/M11 mature |
| Kara Vex | fixer/smuggler | black-market hooks | M6/M8 |
| Mother Lumen | relic mystic | Saints/Dead Gate hooks | M7/M11 |

---

## 4. NPC Archetypes

### Merchant Rival

Moves goods, undercuts routes, issues profit Challenges, notices repeated competition.

### Salvager

Finds wrecks, claims derelicts, races for black boxes, sells rare parts.

### Patrol / Law

Raises/lowers customs pressure, posts legal Bounties, warns high-Heat players.

### Pirate / Raider

Raises route danger, demands tolls, creates wrecks, reacts to bounty hunters.

### Fixer / Broker

Sells rumors, offers smuggling work, launders Heat, opens black markets.

### Faction Agent

Issues faction jobs, requests contributions, rewards standing.

### Relay AI / Weird Entity

Delivers intel, interprets signals, foreshadows anomalies and Dead Gate.

---

## 5. NPC State, Design-Level

Named NPCs need enough state to feel persistent:

- key and display name;
- archetype;
- home Region;
- current Place/Region;
- Faction lean;
- wealth tier;
- ship tier;
- preferred goods/jobs;
- current goal;
- cooldowns;
- relationship memories;
- last action timestamp;
- visibility level.

Avoid early over-modeling:

- exact cargo for every NPC;
- full pathfinding;
- offscreen tactical combat;
- deep personality simulation.

---

## 6. NPC Memory

Memory types:

- helped;
- harmed;
- undercut;
- rescued;
- betrayed;
- ignored distress;
- paid debt;
- stole claim;
- shared faction;
- opposing faction;
- route rivalry.

Plain-language states are better than visible numbers:

- noticed;
- annoyed;
- grateful;
- hostile;
- indebted;
- rival;
- ally.

Memory surfaces through:

- Notices;
- dialog changes;
- Events;
- special jobs;
- discounts/threats;
- Challenges.

---

## 7. NPC Tick Loop

For each active named NPC on due tick:

1. Check availability/cooldown.
2. Choose goal from archetype and world state.
3. Choose target Place, Market, Route, player, or job.
4. Resolve offscreen action abstractly.
5. Emit consequences: Event, Notice, job, Market/hazard/Faction change.
6. Update memory and cooldown.

NPC goals:

- trade;
- salvage;
- patrol;
- pirate;
- faction work;
- challenge player;
- recover after failure;
- send message.

---

## 8. Fairness Rules

- NPCs leave traces before major interference.
- NPCs create opportunities more often than they remove them in low-pop worlds.
- NPCs should not buy out every useful good before the player acts.
- NPCs can compete, but not always win.
- Major NPC harm should be understandable and recoverable.
- Named NPCs should have recognizable habits.

---

## 9. NPC-Generated Content

Events:

```text
Brass Jory dumped coolant at Ash Coil, depressing prices.
Moth-9 marked a fresh wreck signature near Blue Blind.
Marshal Senn increased patrols along the Mercy route.
```

Notices:

```text
Brass Jory: You are making my route noisy.
Mercy Relay Salvage Desk: Moth-9 found something at Blue Blind.
Marshal Senn: Keep your manifest clean near Mercy Relay.
```

Challenges:

```text
Beat Brass Jory’s profit on the Ash Coil ⇄ Mercy Relay corridor.
Recover more salvage value than Moth-9 before tomorrow’s tick.
```

---

## 10. Low-Pop Behavior

If few humans are active, NPCs should:

- generate solo jobs;
- move Markets;
- seed salvage hooks;
- send personal Notices;
- contribute modestly to shared goals;
- avoid over-punishing unattended systems.

If many humans are active, NPCs should:

- become less dominant;
- react more to crowded routes;
- create contested jobs;
- reduce Notice noise.

---

## 11. NPC Milestone Roadmap

- **M2:** background flows; NPC names in Events.
- **M3:** patrol/pirate archetypes affect hazards.
- **M4:** Moth-9 salvage flavor at Blue Blind.
- **M5:** NPCs issue jobs/Bounties.
- **M6:** law/fixer NPCs interact with Heat.
- **M7:** Faction representatives matter.
- **M8:** named NPC memory, Notices, and Challenges become playable.
- **M11:** NPCs participate in campaign/faction outcomes.

---

## 12. Recommended First NPC Slice

For M2:

- background merchant flow changes stock;
- Brass Jory appears in one Market Event;
- Marshal Senn appears in one patrol/hazard digest line;
- Moth-9 appears in one Blue Blind rumor;
- no full NPC memory yet.

For M8:

- promote those names into lightweight persistent NPCs.

---

## 13. Open Questions

1. How many named NPCs before the world feels crowded?
2. Should NPCs have exact ships/cargo or approximate tiers?
3. Can NPCs complete visible jobs?
4. Should NPCs appear on leaderboards?
5. Can players permanently remove named NPCs?
6. How should NPC memory be displayed without exposing machinery?
