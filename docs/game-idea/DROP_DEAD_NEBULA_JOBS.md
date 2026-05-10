# Drop Dead Nebula — Jobs, Contracts, Bounties, Challenges, and Goals Deep Dive

Status: Draft v0.1  
Primary milestone relevance: Contract MVP M0, expanded Job Board M5, Faction Goals M7, Challenges M8  
Purpose: Define the opportunity layer that gives players clear objectives and connects economy, salvage, factions, NPCs, and async play.

---

## 1. Design Intent

Jobs are the player’s “what should I do next?” layer.

They should:

- turn world state into clear opportunities;
- support short BBS sessions;
- work solo and async;
- unify multiple opportunity types in one board;
- create rewards, risks, and stories;
- feed Events, Notices, and Leaderboards.

They should not:

- hide lifecycle rules;
- overwhelm the dashboard;
- require realtime competition;
- collapse Contracts, Bounties, Challenges, and Faction Goals into one blurry concept.

---

## 2. Canonical Distinctions

### Contract

Broad obligation: deliver, survey, escort, supply, repair, transport, inspect.

### Bounty

Target-oriented job: recover object, clear hazard, defeat actor, scan anomaly.

### Challenge

Contest against player or NPC under defined conditions.

### Faction Goal

Shared project advanced by contributions/actions.

### Job Board

Aggregation surface that can display all of these together.

---

## 3. Milestone Placement

| Milestone | Job Role |
| --- | --- |
| M0 | First Mercy Run Contract proves basic objective/reward. |
| M1 | Job Board UI is cleaned up. |
| M2 | Expiry/refresh and system Notices begin. |
| M4 | Salvage recovery jobs are seeded. |
| M5 | Unified Job Board with multiple Contracts and first Bounties. |
| M6 | Smuggling/customs jobs. |
| M7 | Faction-tagged jobs and shared goals. |
| M8 | NPC/player Challenges and richer Notices. |
| M9 | Route-clearing and outpost supply jobs. |
| M10 | Corp/Charter tasks and player listings. |
| M11 | Campaign jobs for Dead Gate phases. |

---

## 4. Job Board UX

The board should label opportunity type clearly.

```text
MERCY RELAY JOB BOARD

Type        Job                              State       Reward
> Contract  Coolant for the Antennas         available   220 cr
  Bounty    Black Box at Blue Blind          open        500 cr
  Faction   Relay Repair Fund                43%         access
  Challenge Beat Brass Jory's cargo run      expires 1d  wager
```

Rules:

- Type label is always visible.
- State is always visible.
- Requirements are visible in detail view.
- Disabled actions explain why.
- Board stays filtered/compact at 80x24.

---

## 5. Contract Design

### Lifecycle

```text
available -> accepted -> completed
available -> expired
accepted -> abandoned
accepted -> failed
accepted -> expired
```

### Contract Families

- delivery;
- supply;
- courier packet;
- survey;
- salvage recovery;
- smuggling;
- escort abstraction;
- repair/contribution;
- passenger/refugee transport;
- route inspection.

### Completion Proof

Could be:

- correct location;
- required cargo;
- scan record;
- recovered item;
- event flag;
- faction contribution;
- route traversal;
- survival/result state.

### Reward Types

- credits;
- commodity;
- standing;
- intel;
- access;
- module discount;
- Heat reduction;
- leaderboard increment.

---

## 6. Bounty Design

Bounties focus on targets.

Target types:

- black box;
- derelict room/object;
- pirate hazard;
- minefield;
- NPC actor;
- anomaly scan;
- stolen cargo;
- route control object.

States:

```text
open -> claimed -> completed
open -> expired
claimed -> failed
claimed -> expired
```

Bounty detail should show:

- issuer;
- target;
- location;
- proof required;
- risk;
- reward;
- expiry;
- exclusivity.

Bounties become playable in M5, seeded by salvage in M4.

---

## 7. Challenge Design

Challenges are async contests.

Types:

- profit race;
- courier race;
- salvage value contest;
- faction contribution race;
- route-clearing contest;
- policy-based duel later.

Rules:

- resolution condition must be explicit;
- expiry must be visible;
- stakes must be visible;
- target player should be able to decline unless the design intentionally allows open contests;
- NPC Challenges are ideal low-pop content.

M8 makes Challenges truly playable.

---

## 8. Faction Goals

Shared goals are not normal jobs, but they belong on the board.

They show:

- progress;
- needed contributions;
- world consequence;
- participating Faction;
- player’s contribution;
- completion rewards.

M7 makes these playable.

---

## 9. Job Generation Sources

Jobs can come from:

- static authored seed;
- market shortages;
- route hazards;
- derelict discovery;
- NPC actions;
- faction needs;
- outpost needs;
- player/corp postings later;
- campaign phase.

### Economy-Generated Examples

- Low Med Gel at Mercy Relay -> relief delivery.
- High Ore at Cinder Pocket -> freight route.
- Low Coolant at Relay -> emergency supply.

### Hazard-Generated Examples

- Pirate pressure high -> route-clearing Bounty.
- Mine density high -> sweep job.
- Anomaly unstable -> scan Contract.

### NPC-Generated Examples

- Brass Jory posts profit Challenge.
- Moth-9 leaks salvage Bounty.
- Marshal Senn posts lawful interdiction job.

---

## 10. Expiry and Refresh

Jobs should not linger forever.

Rules:

- expired accepted jobs notify player;
- public board refreshes should be summarized;
- rare jobs can expire into Events or rumors;
- expiry should not punish players harshly if unclear.

---

## 11. Job Rewards and Balance

Reward should reflect:

- turn cost;
- route danger;
- cargo lockup;
- deadline;
- required reputation;
- opportunity cost;
- rarity.

Avoid:

- guaranteed best job every day;
- rewards that invalidate trading;
- jobs that require unavailable systems.

---

## 12. Milestone Slice Recommendations

### M0

- First Mercy Run only.

### M5

- Add 3–5 job types:
  - delivery;
  - supply;
  - survey;
  - salvage recovery;
  - route-clearing Bounty.

### M7

- Add Faction goals and Faction-tagged jobs.

### M8

- Add NPC/player Challenges.

### M9–M10

- Add outpost and corp tasks.

---

## 13. Open Questions

1. Are Contracts exclusive once accepted?
2. Can multiple players complete the same public job?
3. Does Contract completion consume cargo or verify prior sale?
4. How many jobs should a station show at once?
5. Should failed jobs hurt reputation?
6. Can players post jobs before corporations exist?
7. Should Challenges require explicit acceptance?
