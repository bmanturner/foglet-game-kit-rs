# Drop Dead Nebula — Risk, Route Hazards, Random Events, Customs, and Failure Deep Dive

Status: Draft v0.1  
Primary milestone relevance: hazards/events M3, Heat/customs M6, route control M9  
Purpose: Define how danger enters the game fairly without requiring full combat first.

---

## 1. Design Intent

Risk turns travel and trade into decisions.

Risk should:

- be telegraphed enough to feel fair;
- create choices, not just damage rolls;
- support noncombat solutions;
- make stale intel matter;
- produce stories and scars;
- avoid frequent hard failure.

Risk should not:

- kill new players suddenly;
- hide all odds;
- make safe play impossible;
- require full combat before it is fun.

---

## 2. Milestone Placement

| Milestone | Risk Role |
| --- | --- |
| M0 | No real risk; safe trade loop. |
| M2 | Dashboard/Daily Intel can mention warnings. |
| M3 | First random events, hazards, scanning, confirmations. |
| M4 | Salvage hazards. |
| M5 | Hazard-clearing Bounties. |
| M6 | Heat, customs, ship modules, repairs. |
| M8 | NPC rivalry and targeted risk. |
| M9 | Mines, traps, route control. |
| M11 | Campaign-scale anomalies/fronts. |

---

## 3. Hazard Types

### Pirate Pressure

Creates tolls, ambushes, Bounties, route avoidance.

### Customs Pressure

Creates inspections, bribes, contraband risk, Heat.

### Anomaly Instability

Creates weird travel outcomes, route mutation, relic hooks.

### Debris / Wreck Density

Creates salvage chances and hull risks.

### Mine / Deployable Density

Player/NPC-created route danger. M9+.

### Faction Patrols

Can protect or threaten depending on standing/Heat/cargo.

---

## 4. Event Design Pattern

Each event should show:

- fiction setup;
- risk category;
- 2–5 choices;
- visible costs;
- likely consequences;
- escape/back rules if any.

Example:

```text
ION SQUALL
> Ride it out          no extra turn, minor hull risk
  Stabilize jump       spend 1 turn, avoid damage
  Dump coolant         consume cargo, safe arrival
```

---

## 5. Event Tables

### Travel Events

- quiet passage;
- merchant rumor;
- customs ping;
- distress beacon;
- ion squall;
- pirate shadow;
- debris cloud;
- patrol inspection;
- refugee flotilla;
- wreck signature;
- hidden shortcut;
- anomaly flare.

### Dockside Events

- broker discount;
- informant intel;
- crew trouble;
- black market opens;
- crackdown;
- faction recruiter;
- debt collector;
- module auction;
- war news;
- dock accident;
- rival undercuts market;
- relic buyer.

### Salvage Events

- empty husk;
- standard yield;
- spare parts;
- data core;
- trapped compartment;
- survivor pod;
- claim beacon;
- unstable reactor;
- faction wreck;
- prototype fragment;
- precursor signature;
- false signal ambush.

---

## 6. Fairness Rules

- First exposure to a danger should be survivable.
- Dangerous routes require confirmation.
- Disabled/blocked options explain requirements.
- Damage should usually be recoverable.
- Events should leave clear feedback.
- Risk can be reduced through scanning, modules, faction standing, cargo choices, bribes, or alternate routes.

---

## 7. Failure Ladder

Prefer partial consequences:

1. lose small credits;
2. spend extra turn;
3. lose cargo;
4. gain Heat;
5. take hull damage;
6. damage module;
7. fail/expire job;
8. forced tow;
9. escape pod rescue;
10. rare ship loss.

---

## 8. Scanning and Intel

Scanning should:

- cost turns or opportunity;
- update Recall;
- reveal hazard category, not exact certainty at first;
- improve with modules;
- sometimes reveal opportunities.

M3 introduces basic scan. M6+ makes modules matter.

---

## 9. Heat and Customs

Heat is risk from law attention.

Sources:

- contraband;
- spoofing;
- fleeing inspection;
- piracy;
- faction hostility;
- repeated suspicious route use.

Sinks:

- time/ticks;
- bribes;
- favors;
- legal jobs;
- faction protection;
- forged papers.

M6 makes Heat a real system.

---

## 10. Risk UX

Use symbols:

- `!` warning;
- `?` unknown/stale;
- `$` profitable risk;
- `✗` missing requirement.

Route screen should eventually show:

```text
Red Maw Approach   Cost 2   ! anomaly / stale intel 2d
```

---

## 11. Open Questions

1. How often should travel events fire?
2. Should event odds be visible?
3. Should safe routes ever have dangerous events?
4. How fast does Heat decay?
5. Should failed scans give partial info?
6. Can random events create permanent world changes?
