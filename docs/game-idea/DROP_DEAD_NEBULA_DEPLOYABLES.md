# Drop Dead Nebula — Deployables, Mines, Traps, Drones, and Route Control Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M6, playable M9, mature M10–M11  
Purpose: Define persistent player/NPC objects that affect future travel, hazards, intel, and territory.

---

## 1. Design Intent

Deployables let the route remember what players and NPCs left behind.

They should:

- create async strategy;
- support route control;
- provide counterplay;
- generate Notices/Events;
- integrate with scanning, travel, combat, outposts, and Factions.

They should not:

- create unavoidable griefing;
- make safe routes unusable;
- become invisible instant death;
- require realtime monitoring.

---

## 2. Deployable Types

### Sensor Ghost

False or enhanced signal marker. Reveals movement, misleads scans, or creates rumors.

### Decoy Wreck

Lures salvagers/pirates; may create ambush or false opportunity.

### Mine

Damages or stops ships entering Place/Route.

### EMP Mine

Disables modules or shields rather than pure damage.

### Interdiction Buoy

Increases chance of interception or forces extra turn cost.

### Cargo Leech

Steals small cargo or marks ship for pirates.

### Sentry Drone

Light defensive/offensive deployable tied to Outpost/Route.

### Dead Drop

Hidden cache/message/inventory transfer point.

---

## 3. Milestone Placement

| Milestone | Role |
| --- | --- |
| M6 | Modules can seed deployable capability. |
| M8 | NPCs may threaten/use simple deployables in flavor. |
| M9 | First playable deployables and route control. |
| M10 | Corp/outpost route control. |
| M11 | Faction fronts and campaign route warfare. |

---

## 4. Deployable State

Design-level fields:

- owner;
- Place or Route;
- type;
- trigger condition;
- visibility/detection difficulty;
- faction friendliness;
- charges/durability;
- expiry/decay;
- legal status;
- effect payload;
- last triggered.

---

## 5. Trigger Conditions

Examples:

- any travel through route;
- hostile faction only;
- high-Heat captains;
- cargo contains contraband;
- scanner failed;
- bounty target enters;
- random chance on travel.

---

## 6. Counterplay

Every deployable needs counterplay:

- scan;
- sweep;
- alternate route;
- faction permit;
- stealth module;
- sacrificial drone;
- bribe/local intel;
- route-clearing Bounty.

---

## 7. Anti-Griefing Rules

- safe zones restrict harmful deployables;
- new-player routes have strong protections;
- deployables decay;
- triggering creates evidence or risk for owner;
- sweeping is viable;
- maximum density per route/place;
- Factions/patrols clear illegal devices.

---

## 8. Route Control UX

```text
ROUTE CONTROL — BLUE BLIND
Your deployables:
> Sensor Ghost       expires 2d   triggered 0
  Decoy Wreck        expires 1d   triggered 1

Known hazards:
  Pirate pressure    moderate
  Mine density       unknown

(D)eploy  (S)weep  (R)ecall intel  (Esc) back
```

---

## 9. Deployable Reports

Trigger Notice example:

```text
Your Sensor Ghost near Blue Blind recorded Brass Jory heading toward Red Maw.
```

Event example:

```text
A minefield near Red Maw was swept by Port Authority patrols.
```

---

## 10. Integration Points

- **Inventory:** deployables are items before placement.
- **Travel:** trigger during route resolution.
- **Scanning:** detection/counterplay.
- **Combat:** mines/drones initiate or modify encounters.
- **Outposts:** defense and local sensors.
- **Factions:** legality and friendliness.
- **Jobs:** sweep/clear/deploy tasks.

---

## 11. Recommended First Deployable Slice

M9:

- Sensor Ghost and Decoy Wreck only;
- visible route/place placement;
- scan/sweep action;
- expiry by tick;
- simple Notice when triggered;
- safe-zone restrictions.

Defer:

- damaging mines;
- complex trap chains;
- player-targeted PvP traps;
- faction minefields;
- corp route warfare.

---

## 12. Open Questions

1. Are deployables attached to Places, Routes, or both?
2. How many deployables per owner per route?
3. Should deployables be anonymous or reveal owner?
4. Can deployables trigger on NPCs?
5. How strong should safe-zone cleanup be?
6. Should dead drops allow player-authored text?
