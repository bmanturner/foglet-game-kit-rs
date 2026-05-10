# Drop Dead Nebula — Ships, Modules, Damage, and Progression Deep Dive

Status: Draft v0.1  
Primary milestone relevance: starter Ship M0, modules/damage M6, mature builds M10  
Purpose: Define Ship identity, hull classes, module progression, repairs, damage, and recovery.

---

## 1. Design Intent

The Ship is the player’s build, risk profile, and identity.

Ships should:

- support different careers;
- create meaningful tradeoffs;
- make progression visible;
- interact with travel, Markets, salvage, Heat, and combat;
- suffer consequences without frequent hard deletion.

Ships should not:

- be a linear bigger-is-always-better ladder;
- require complex fitting before MVP;
- punish experimentation too harshly;
- hide important constraints.

---

## 2. Milestone Placement

| Milestone | Ship Role |
| --- | --- |
| M0 | Rustbucket Mule, fixed cargo capacity. |
| M1 | Better status/cargo UI. |
| M4 | Salvage tools can be seeded. |
| M6 | Modules, repair, damage, fuel/Heat interactions become playable. |
| M8 | NPC rivals recognize Ship/build. |
| M10 | Mature hull classes and economy around modules. |
| M11 | Relic/endgame modules. |

---

## 3. Hull Classes

### Mule

Beginner/trader. Cheap cargo, weak defenses.

### Courier

Fast, efficient, good intel/scanning, low cargo.

### Prospector

Salvage specialist. Better wreck scans and extraction.

### Raider

Piracy/privateer. Boarding and intimidation, high Heat risk.

### Gunship

Combat/bounty. Strong weapons, expensive upkeep, lower trade efficiency.

### Tender

Support/logistics. Drones, repair, convoy/outpost support.

### Colony Ark

Strategic builder. Outposts and bulk supplies, slow and vulnerable.

### Ghost Clipper

Smuggler. Stealth, false holds, speed, low armor.

### Relic Skiff

Anomaly diver. Weird modules and risks.

---

## 4. Ship Stats

Core stats:

- cargo capacity;
- hull integrity;
- fuel or range, if used;
- scanner quality;
- stealth;
- shield/defense;
- hardpoints/module slots;
- salvage rating;
- heat profile;
- upkeep/repair cost.

MVP only needs cargo capacity.

---

## 5. Modules

Module categories:

- cargo expansion;
- scanner;
- stealth/false hold;
- shield/armor;
- salvage tool;
- mine/deployable tool;
- drone bay;
- engine/jump stabilizer;
- med bay;
- faction transponder;
- relic containment.

Module rules:

- show clear benefit;
- show slot/cost requirements;
- allow replacement with confirmation;
- damaged modules can be disabled until repaired;
- rare modules should create build identity.

---

## 6. Damage and Repair

Damage states:

- scratched;
- damaged;
- critical;
- crippled;
- towed/rescued.

Damage sources:

- hazards;
- combat;
- failed salvage;
- mines;
- anomaly events;
- neglect/upkeep later.

Repair sources:

- station services;
- modules;
- outposts;
- faction perks;
- emergency field patch.

Failure should create costs and stories, not constant deletion.

---

## 7. Fuel

Fuel is optional and should be added only if it improves choices.

Pros:

- adds route planning;
- creates Market sink;
- supports rescue stories.

Cons:

- can strand players;
- adds UI load;
- can punish casual play.

Recommendation:

- no fuel in MVP;
- consider fuel in M6 alongside repair/rescue systems;
- never allow easy unrecoverable stranding.

---

## 8. Insurance and Recovery

Recovery ladder:

1. pay repair;
2. limp to station;
3. tow service;
4. faction rescue;
5. insurance payout;
6. escape pod;
7. rare ship loss with legacy preserved.

Keep Captain progression even if Ship is lost.

---

## 9. UX Screens

### Ship Status

```text
RUSTBUCKET MULE                           Hull 82%  Hold 20
Cargo: 8/20       Scanner: basic       Heat profile: normal

Modules
> Patchwork Cargo Rack    +5 hold
  Basic Scanner           reveals common hazards
  Empty Slot

(R)epair  (I)nstall  (B)uy hulls  (Esc) back
```

### Module Detail

```text
FALSE HOLD
Effect: hides 6 cargo from routine customs checks
Cost: 900 credits
Requires: utility slot
Risk: illegal at Port Authority stations

(B)uy/install  (Esc) back
```

---

## 10. Progression Principles

- New hulls should open playstyles, not just bigger numbers.
- Modules should create dilemmas: cargo vs safety vs stealth vs salvage.
- Repair/upkeep are credit sinks but should not feel like chores.
- Rare modules can be tied to salvage, Factions, or campaign.

---

## 11. Recommended First Ship Expansion

M6:

- hull damage and repair;
- cargo rack;
- basic scanner;
- false hold;
- salvage claw;
- shield patch;
- simple Ship Status screen;
- one module sold at Ash Coil;
- one module found through salvage.

Defer:

- large hull catalog;
- crew;
- complex fuel;
- full combat stats;
- relic modules.

---

## 12. Open Questions

1. Should fuel be part of core play?
2. Can players own multiple Ships?
3. Are modules inventory items, installed records, or both?
4. How much damage should persist across sessions?
5. Should hull class affect Daily Turn efficiency?
6. How easy is it to respec modules?
