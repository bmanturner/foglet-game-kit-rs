# Drop Dead Nebula — Combat, Piracy, Interdiction, and Conflict Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M3/M6, playable M6–M8, route-control maturity M9–M10  
Purpose: Define conflict resolution that creates risk and stories without making hard death common.

---

## 1. Design Intent

Combat is one way to resolve danger, not the default game mode.

It should:

- support pirates, bounty hunters, patrols, route control, and rivalries;
- offer noncombat choices;
- create meaningful consequences;
- avoid frequent hard deletion;
- work asynchronously;
- fit short BBS sessions.

It should not:

- become a long tactical game inside every route;
- punish traders for not building gunships;
- require realtime PvP;
- hide stakes from the player.

---

## 2. Milestone Placement

| Milestone | Combat Role |
| --- | --- |
| M3 | Avoidance/toll/escape choices appear in risk events. |
| M4 | Salvage hazards may imply drone/boarding danger. |
| M5 | Bounties can target hazards or pirate actors. |
| M6 | Ship damage, repair, modules, Heat support conflict. |
| M8 | NPC rival conflict and async Challenges. |
| M9 | Mines/deployables and route control. |
| M10 | Corp/charter conflict and broader build variety. |
| M11 | Campaign conflict/faction fronts. |

---

## 3. Encounter Types

- pirate toll;
- customs interdiction;
- bounty intercept;
- mine/deployable trigger;
- patrol warning;
- drone hazard;
- boarding skirmish;
- convoy defense abstraction;
- faction skirmish;
- async duel/challenge.

---

## 4. Conflict Choice Pattern

Every conflict should show:

- enemy/threat;
- stakes;
- player condition;
- options;
- likely cost categories.

Example:

```text
PIRATE INTERDICTION — RED MAW APPROACH
Enemy: Knife Sermon, light raider
Threat: moderate
Demand: 120 credits or 3 cargo
Your: Hull 82%, Cargo 14/20

> Run cold              evade, risk engine damage
  Pay toll              lose credits/cargo, avoid fight
  Fight                 spend turn, risk hull/cargo
  Bluff                 Heat/faction check
  Dump decoy            consume deployable
```

---

## 5. Combat Resolution Levels

### 5.1 Encounter Choice Only

Early form. Player chooses toll/run/fight/bluff. Outcome resolved in one step.

Milestone: M3–M6.

### 5.2 Stance-Based Combat

Player picks stance:

- aggressive;
- balanced;
- evasive;
- disabling;
- boarding;
- escape.

Milestone: M6+.

### 5.3 Policy-Based Async Combat

Offline player defenses use configured policies.

Examples:

- avoid conflict;
- defend only;
- attack wanted captains;
- flee below hull threshold;
- surrender low-value cargo.

Milestone: M8+ if PvP/async conflict is enabled.

---

## 6. Consequence Ladder

Prefer consequences over hard death:

1. warning/no loss;
2. extra turn spent;
3. credits lost;
4. cargo lost;
5. Heat gained;
6. hull damage;
7. module damaged;
8. job failed;
9. forced tow;
10. escape pod rescue;
11. wreck created;
12. rare ship loss.

---

## 7. Piracy

Piracy can be player career, NPC pressure, or Faction/Dust Clan behavior.

Pirate actions:

- demand toll;
- steal cargo;
- plant decoy;
- create Bounty;
- intimidate Market;
- raise route hazard.

Counterplay:

- pay;
- fight;
- hide;
- take alternate route;
- hire patrol/faction help;
- clear route Bounty;
- deploy scanner/drone.

---

## 8. Lawful Conflict

Port Authority and patrols should not just be “good pirates.”

They create:

- inspection pressure;
- contraband risk;
- legal Bounties;
- protection on safe routes;
- standing-based leniency;
- Heat consequences.

---

## 9. Async PvP Safety

PvP should be opt-in or policy-bounded until proven safe.

Rules:

- safe zones limit aggression;
- offline policies control response;
- catastrophic losses are rare;
- player receives Notice summary;
- attacker risk exists;
- griefing routes have counterplay.

---

## 10. Combat UX

Combat screens must show stakes plainly:

- current hull;
- cargo at risk;
- turns at risk;
- Heat/reputation risk;
- escape options;
- disabled reasons.

Never make `Esc` secretly flee or abandon unless displayed.

---

## 11. Integration Points

- **Ships:** hull, modules, repair.
- **Risk Events:** conflict entry point.
- **Bounties:** targets and rewards.
- **Heat:** legality consequences.
- **NPCs:** rival/pirate/patrol personalities.
- **Deployables:** mines, drones, traps.
- **Outposts:** raids/defense later.
- **Factions:** fronts and standing.

---

## 12. Recommended First Combat Slice

M6:

- pirate toll event;
- pay/run/bluff/fight choices;
- hull damage and cargo loss outcomes;
- repair at station;
- one route-clearing Bounty later;
- no full PvP yet.

Defer:

- tactical multi-round combat;
- boarding combat;
- permanent ship destruction;
- complex weapon stats;
- corp warfare.

---

## 13. Open Questions

1. Should fighting consume a Daily Turn per round?
2. How visible should odds be?
3. Can players attack other players by default?
4. Should NPC pirates steal specific cargo or abstract value?
5. How common should forced tow be?
6. Can defeated NPCs become recurring rivals?
