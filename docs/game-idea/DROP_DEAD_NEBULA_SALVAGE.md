# Drop Dead Nebula — Salvage and Derelict Boarding Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M0/M4, playable M4, matures M8/M11  
Purpose: Define the signature derelict boarding and salvage loop.

---

## 1. Design Intent

Salvage is Drop Dead Nebula’s signature alternative to trade.

It should:

- use local ASCII maps;
- create turn/cargo/risk decisions;
- produce loot, intel, black boxes, and story;
- tie into Bounties, NPC rivals, Factions, and campaign relics;
- be playable in short BBS sessions.

It should not:

- become a full roguelike too early;
- require huge maps;
- trap the player without clear exit;
- invalidate trading as the main economy spine.

---

## 2. Milestone Placement

| Milestone | Salvage Role |
| --- | --- |
| M0 | Blue Blind exists as flavor. |
| M3 | Wreck signatures can appear as risk/opportunity. |
| M4 | First playable derelict boarding slice. |
| M5 | Salvage Bounties and recovery Contracts. |
| M6 | Salvage modules/tools. |
| M8 | NPC rivals and shared stripping. |
| M11 | Relics/anomalies tie salvage to Dead Gate campaign. |

---

## 3. Core Salvage Loop

1. Discover wreck via Route, rumor, Job Board, scan, NPC, or Event.
2. Approach site and review risk/value.
3. Spend turn to board or scan.
4. Explore compact local map.
5. Encounter hazards, loot, survivors, data.
6. Choose what to carry with limited Cargo.
7. Return to Ship.
8. Sell, deliver, decode, or keep salvage.
9. Site updates: fresh, mapped, stripped, unstable, exhausted.

---

## 4. Derelict Site States

- fresh;
- scanned;
- boarded;
- partially stripped;
- claimed;
- unstable;
- exhausted;
- decayed/gone.

State may be shared globally, per-player, or hybrid depending on fairness.

Recommendation:

- important loot is shared;
- some minor salvage can be player-local or regenerated;
- black-box/Bounty objectives are shared and finite.

---

## 5. Local Map Design

Maps should be small:

- 5–12 rooms for first slice;
- clear entry/exit;
- visible symbols;
- few hazards;
- no maze frustration.

Room types:

- Airlock;
- Cargo Bay;
- Bridge;
- Engineering;
- Med Pod;
- Crew Quarters;
- AI Core;
- Reactor;
- Vault;
- Drone Nest.

---

## 6. Salvage Choices

Choices should trade off:

- turns;
- cargo capacity;
- hull risk;
- module requirement;
- moral/reputation consequence;
- Heat/legal risk;
- job completion.

Example:

```text
SEALED CARGO CRATE
Take Relay Coils x2? Value high, mass 6, claim beacon risk.
(T)ake  (J)ettison other cargo  (L)eave
```

---

## 7. Loot Categories

- scrap;
- commodities;
- ship modules;
- black boxes;
- route intel;
- faction secrets;
- relic fragments;
- survivor pods;
- mission evidence;
- deployables.

---

## 8. Hazards

- radiation;
- unstable reactor;
- hull collapse;
- drone activity;
- pressure loss;
- parasite spores;
- pirate claim beacon;
- anomaly contamination;
- time sink;
- moral dilemma.

Hazards should be telegraphed and often avoidable/mitigable.

---

## 9. NPC and Async Interactions

NPC salvagers can:

- discover sites;
- mark claims;
- strip rooms on ticks;
- challenge player;
- buy salvage;
- send warnings/threats.

Player async effects:

- one player clears a black box;
- another finds the stripped hull;
- Events note major recoveries;
- Bounties expire when target is gone.

---

## 10. Salvage UX Screens

- Salvage Site Approach;
- Local Derelict Map;
- Room Detail;
- Loot Choice Modal;
- Hazard Result Modal;
- Return to Ship Summary;
- Salvage Bounty Detail.

Always show:

- turns;
- cargo space;
- exit path;
- current room;
- risk state.

---

## 11. Recommended First Salvage Slice

M4:

- Blue Blind active;
- one derelict: Choirless Bell;
- 5-room map;
- one black box;
- one hazard;
- one loot crate;
- one return summary;
- one related Event.

Defer:

- procedural derelicts;
- NPC stripping;
- relic systems;
- combat boarding;
- complex oxygen/timers.

---

## 12. Open Questions

1. Is derelict state shared globally or instanced per player?
2. Can a player quit mid-derelict?
3. How much salvage respawns, if any?
4. Do hazards consume turns, damage hull, or both?
5. Should moral choices affect Faction standing immediately?
6. Can derelicts be player-created from ship losses?
