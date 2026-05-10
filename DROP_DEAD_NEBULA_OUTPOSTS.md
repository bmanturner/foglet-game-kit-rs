# Drop Dead Nebula — Outposts, Colonies, Stockpiles, and Production Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M7, playable M9, mature M10–M11  
Purpose: Define durable player footprint: claimable places, stockpiles, access policies, production, and colony-like growth.

---

## 1. Design Intent

Outposts let players leave a mark on the nebula.

They should:

- store goods outside the Ship;
- create long-term logistics goals;
- support solo Charters and Corporations;
- connect to Faction projects;
- create route-control and market strategy;
- remain manageable in terminal UI.

They should not:

- become a spreadsheet colony sim too early;
- be mandatory for basic progression;
- create unrecoverable passive losses;
- allow griefy route domination without counterplay.

---

## 2. Terms

### Stockpile

Owner-keyed inventory outside a Ship, usually at Station/Outpost/Depot/Faction.

### Cache

Small hidden or private stockpile with limited services.

### Outpost

Claimed or built Place with storage, access policy, and possible upgrades.

### Colony

Advanced Outpost with population, production, needs, morale, and vulnerability.

### Project

Upgrade or construction task at an Outpost.

---

## 3. Milestone Placement

| Milestone | Role |
| --- | --- |
| M0–M6 | Flavor and future hooks only. |
| M7 | Faction shared goals seed infrastructure needs. |
| M9 | First caches, depots, claimable Outpost, deployable interaction. |
| M10 | Production, Charters/Corps, broader economy. |
| M11 | Outposts affect campaign logistics. |
| M12 | Recovery/admin tools for abandoned or broken sites. |

---

## 4. Outpost Lifecycle

Suggested progression:

```text
hidden cache -> claim beacon -> rough dock -> fortified depot -> productive outpost -> colony / specialized base
```

States:

- unclaimed;
- claimed;
- active;
- under-supplied;
- damaged;
- raided;
- abandoned;
- archived/ruined.

---

## 5. Ownership

Owners may be:

- Captain;
- Charter;
- Corporation;
- Faction;
- NPC;
- system/world.

Start with Captain/Charter ownership. Add corp/faction ownership once permissions and Factions are useful.

---

## 6. Stockpiles

Stockpiles are the first Outpost feature.

Uses:

- store trade goods;
- stage Faction contributions;
- prepare for market shifts;
- hold salvage;
- supply local projects;
- support corp logistics.

UX must show:

- capacity;
- contents;
- owner;
- access rule;
- recent changes;
- project reservations.

---

## 7. Access Policies

Start simple:

- private;
- Charter/Corp;
- Faction;
- public deposit only;
- public market later.

Rules:

- deposits can be more permissive than withdrawals;
- withdrawals must be logged;
- dangerous access changes need confirmation;
- safe-zone rules may restrict hostile actions.

---

## 8. Projects and Upgrades

Project examples:

- expand storage;
- install beacon;
- add repair cradle;
- add market terminal;
- add shield grid;
- install scanner array;
- build refinery rig;
- add hidden hold;
- faction office.

Projects consume:

- credits;
- Commodities;
- salvage items;
- time/ticks;
- Faction standing;
- route safety prerequisites.

---

## 9. Production and Consumption

M10+.

Production examples:

- mining outpost produces Ore;
- refinery outpost produces Coolant from Ore;
- agri colony produces Food;
- drone foundry produces deployables;
- relay station produces intel/Notices/jobs.

Consumption examples:

- population consumes Food/Water/Med Gel;
- machinery consumes Parts/Coolant;
- defenses consume Energy/Parts;
- projects consume strategic goods.

Do not add full production chains before Markets and stockpiles can use them.

---

## 10. Raids, Decay, and Safety

Outposts should have risk, but passive destruction must be careful.

Risk sources:

- pirate pressure;
- Faction conflict;
- under-supply;
- neglected defenses;
- public access abuse;
- campaign events.

Failure ladder:

1. warning Notice;
2. production reduced;
3. stock loss;
4. project delay;
5. damage state;
6. service disabled;
7. rare abandonment/ruin.

Low-pop protection:

- absence alone should not erase major investments;
- long inactivity may pause or degrade slowly;
- recovery Contracts should appear.

---

## 11. UX Screens

### Outpost Overview

```text
OUTPOST — CINDER CACHE                    Owner: Vanta Charter
Status: rough dock       Storage: 18/80       Security: low
Access: private          Production: none     Projects: 1 ready

(D)epot  (P)rojects  (A)ccess  (R)eports  (Esc) back
```

### Project Screen

```text
PROJECT: Install Relay Beacon
Needs: Relay Coil x1, Reactor Coolant x8, 500 credits
Effect: unlocks faster notices and route intel near Cinder Pocket
Progress: ready to fund

(C)ontribute  (F)und from Charter  (Esc) back
```

---

## 12. Integration Points

- **Inventory:** stockpiles and capacity.
- **World Ticks:** production, consumption, decay.
- **Markets:** local selling/buying later.
- **Factions:** protection, offices, shared goals.
- **Corporations:** shared ownership and depots.
- **Deployables:** defenses and route control.
- **Jobs:** supply, repair, defend, upgrade tasks.

---

## 13. Recommended First Outpost Slice

M9:

- one claimable cache/outpost;
- storage only;
- private access;
- deposit/withdraw;
- one upgrade project;
- one tick report;
- one supply Contract tied to it.

Defer:

- population;
- full production chain;
- raids;
- public markets;
- multi-member permissions unless Corps already exist.

---

## 14. Open Questions

1. Should Outposts be built on existing Places or create new Places?
2. How many Outposts can one Captain/Charter own?
3. Can Outposts be destroyed?
4. Should public access be allowed before social safety tools mature?
5. How much production complexity is fun in terminal UI?
6. Should Outpost location be hidden or public?
