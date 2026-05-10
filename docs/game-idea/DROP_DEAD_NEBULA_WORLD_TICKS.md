# Drop Dead Nebula — World Ticks Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M1, playable M2, matures M7–M11  
Purpose: Define how the shared world advances without realtime multiplayer or a long-lived game server.

---

## 1. Design Intent

World Ticks are how Drop Dead Nebula feels alive when nobody is online. They advance markets, turns, jobs, NPC pressure, route hazards, faction projects, salvage decay, outposts, and digest/news output.

They must be:

- bounded;
- transactional;
- summarizeable;
- safe to run lazily on login or from optional cron;
- legible to players through Daily Intel, Events, Notices, and Dashboard Signals.

They must not:

- block login for a long time;
- require a daemon;
- mutate important state partially;
- punish absence with surprise catastrophe;
- spam every tiny simulation result.

---

## 2. Milestone Placement

| Milestone | Tick Role |
| --- | --- |
| M0 MVP | No simulation; persistent state only. |
| M1 Foundation | Prepare tick vocabulary and diagnostics shape. |
| M2 Living Corridor | First real ticks: turn reset, simple market drift, system Notices, background NPC flow. |
| M3 First Danger | Route hazards and random-event weights drift. |
| M4 Salvage | Derelicts can age, refresh, or become partially stripped. |
| M5 Jobs Expand | Contract/Bounty expiry and generation. |
| M6 Ships and Heat | Heat decay, repairs, fuel/service timers. |
| M7 Factions | Shared goals and influence advance. |
| M8 NPC Rivalry | Named NPC actions and challenge resolution. |
| M9 Outposts | Stockpiles, deployable decay, early production. |
| M10 Economy Expansion | Production/consumption loops and player listings. |
| M11 Dead Gate | Season phases, faction fronts, regional shocks. |
| M12 Operations | Admin diagnostics, repair, and tick health. |

---

## 3. Tick Modes

### 3.1 Login Catch-Up

Runs when a Captain enters. It should do only enough work to make the world current and produce a useful summary.

Player-facing output:

```text
WHILE YOU WERE OUT
$ Mercy Relay consumed most of its Med Gel.
! Pirate pressure rose near Blue Blind.
+ 2 new jobs posted in the corridor.
@ Brass Jory moved coolant through Cinder Pocket.
```

### 3.2 Safe-Boundary Refresh

Runs on low-risk boundaries such as returning to Dashboard or opening Jobs. Use for cleanup and local refresh, not heavy simulation.

### 3.3 Optional Cron Tick

External scheduler can run ticks so the first returning player does not pay catch-up cost. The game must still work without this.

### 3.4 Admin Tick

Sysop/manual diagnostic run. Any mutation should be auditable.

---

## 4. Recommended Tick Work Order

1. Discover due tasks and apply catch-up cap.
2. Reset/refill Daily Turns.
3. Expire Contracts, Bounties, Challenges, temporary effects.
4. Process station consumption and production.
5. Restock/reprice Markets.
6. Resolve background NPC flows.
7. Resolve named NPC actions, once available.
8. Drift Route hazards.
9. Generate new Contracts/Bounties/opportunities.
10. Advance Faction/shared goals.
11. Update derelicts, outposts, deployables.
12. Generate digest, Events, and Notices.

This order prevents jobs being generated from stale market/hazard state.

---

## 5. Catch-Up Policy

### Exact Catch-Up

Use for one missed day or critical lifecycle updates.

### Compressed Catch-Up

Use after long absence. Aggregate several intervals into one outcome:

- “Markets normalized after 6 quiet days.”
- “Three minor contracts expired; two new jobs were posted.”
- “Pirate pressure drifted down after patrol sweeps.”

### Deferred Catch-Up

Process a safe slice now and leave later work due. Use when world is large or storage is slow.

### Hard Rules

- No unbounded loops on login.
- No large windfalls from idleness alone.
- No catastrophic loss purely because a player was absent.
- Always prefer readable summary over exhaustive simulation dump.

---

## 6. Tick Categories

### 6.1 Daily Turn Reset

M2. Refills Daily Turns and applies reserve cap. Later modules/factions may adjust allowance.

### 6.2 Market Drift

M2. Stations consume/produce goods and drift stock toward equilibrium. Major changes create Events or digest lines.

### 6.3 Job Expiry and Generation

M2 seed, M5 playable. Expire stale opportunities, generate jobs from shortages, hazards, factions, salvage sites, and NPC actions.

### 6.4 Background NPC Flow

M2. Abstract merchants, patrols, pirates, refugees, and scavengers alter markets and hazards without full individual simulation.

### 6.5 Named NPC Actions

M8. Named NPCs choose goals and produce targeted Events/Notices/Challenges.

### 6.6 Route Hazard Drift

M3. Pirate pressure, customs scrutiny, anomaly instability, mine density, and patrol presence change over time.

### 6.7 Derelict Decay

M4+. Salvage sites progress fresh -> mapped -> partially stripped -> unstable/exhausted/gone.

### 6.8 Faction Progress

M7+. Shared goals advance, stall, complete, and mutate world state.

### 6.9 Outpost and Deployable Processing

M9+. Stockpiles, production, access policy effects, and deployable expiration/trigger reports.

### 6.10 Campaign Phase Progression

M11. Dead Gate phases and regional/faction fronts.

---

## 7. Events, Notices, and Digests

### Event

Durable world fact. Use for public or historical changes.

### Notice

Targeted message to player/faction/corp. Use when the player needs to act or know personally.

### Digest

Login summary. Use for minor aggregated changes.

Send a Notice for:

- accepted job expiry;
- Challenge resolution;
- trap/deployable trigger;
- outpost problem;
- direct NPC message;
- faction standing threshold;
- shared goal completion relevant to player.

Append an Event for:

- public job completion;
- major shortage/crisis;
- route opens/closes;
- notable NPC action;
- derelict appears/clears;
- faction project changes world state.

Digest only:

- minor price changes;
- routine restock;
- ordinary turn reset;
- generic NPC flow.

---

## 8. Low-Population Scaling

When recent human activity is low, ticks should:

- generate more solo-completable jobs;
- increase background NPC trade enough to move Markets;
- avoid stripping all salvage before the player arrives;
- reduce shared-goal requirements or add NPC contributions;
- send occasional named NPC hooks;
- create Events that imply life without stealing all agency.

This should feel like a living world, not pity mode.

---

## 9. Failure and Recovery

A failed tick should:

- roll back its transaction;
- remain retryable or mark diagnostic state;
- not block core play if noncritical;
- produce admin-visible diagnostics;
- maybe show player-safe copy: “Some corridor updates are delayed.”

M12 should add tick health tools.

---

## 10. Recommended First Tick Slice

For M2, build only:

- Daily Turn reset;
- simple Market restock/reprice;
- simple job expiry/refresh;
- background NPC trade as aggregate pressure;
- Daily Intel digest;
- one or two notable Events.

Defer:

- full named NPC AI;
- faction fronts;
- outpost production;
- derelict decay;
- deployable processing;
- season changes.

---

## 11. Open Questions

1. How much catch-up can run before Dashboard appears?
2. Should Daily Intel be stored as Notices, generated from Events, or both?
3. Should background NPC flow be deterministic by world seed/day?
4. Should Market drift happen before or after abstract NPC trade?
5. How do long absences compress faction progress fairly?
6. Should players be protected from negative outpost/job effects after extended absence?
