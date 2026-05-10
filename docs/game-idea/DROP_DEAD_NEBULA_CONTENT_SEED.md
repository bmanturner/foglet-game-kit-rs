# Drop Dead Nebula — MVP Content Seed

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`, `DROP_DEAD_NEBULA_MVP.md`  
Purpose: Define the tiny authored world used by the first playable MVP vertical slice.

---

## 1. Seed Design Goal

This content seed exists to support exactly one first playable loop:

> Start at Ash Coil, accept First Mercy Run, buy Med Gel, travel to Mercy Relay, sell or deliver Med Gel for profit/reward, see an Event, quit, and resume intact.

The seed should be small, legible, and deterministic. It should hint at the larger Drop Dead Nebula without requiring any advanced systems.

---

## 2. Scope Boundaries

### 2.1 Included Content

The seed includes:

- 7 Places;
- 10 directed Routes;
- 3 Commodities;
- 2 active Station Markets;
- 1 starter Ship;
- 1 delivery Contract;
- a small set of initial help/news flavor lines.

### 2.2 Excluded Content

The seed does not include:

- NPC Captains;
- factions as active mechanics;
- bounties;
- async challenges;
- salvageable derelict interiors;
- combat encounters;
- random travel events;
- mines/traps;
- outposts/colonies;
- dynamic pricing;
- generated world content.

References to future content may appear as flavor only, but must not imply available mechanics.

---

## 3. Content Key Conventions

Use stable content keys. Display names may change later; keys should not change casually.

Recommended key style:

- Places: `place.<name>`
- Routes: `route.<from>.<to>`
- Commodities: `commodity.<name>`
- Ship hulls: `ship.<name>`
- Contracts: `contract.<name>`
- Markets: `market.<place>`

Examples:

- `place.ash_coil`
- `route.ash_coil.mercy_relay`
- `commodity.med_gel`
- `contract.first_mercy_run`

This is a design convention, not a code prescription.

---

## 4. Seed World Overview

The MVP slice takes place in a small corridor on the civilized edge of the Drop Dead Nebula.

### 4.1 Fiction Summary

**Ash Coil** is an overworked refinery station at the edge of settled space. It has industrial output, cheap medical surplus from a nearby relief depot, and too many captains looking for their first break.

**Mercy Relay** is a damaged communications and humanitarian station that needs Med Gel and Reactor Coolant to stay online.

Between them is a short route safe enough for new captains, but the surrounding Places hint at the larger game: a mining rock, a dead gate, a wreck field, a black-market drift, and a future faction outpost.

### 4.2 Seed Region

Region name:

- **Ash Mercy Corridor**

Region role:

- tutorial trade corridor;
- first safe route;
- early hints of salvage, smuggling, and faction pressure;
- small enough to understand in one session.

---

## 5. Places

The MVP seed contains 7 Places.

### 5.1 Ash Coil

Key:

- `place.ash_coil`

Kind:

- Station
- Port
- Starting Place

Display name:

- Ash Coil

Short description:

> A refinery station wrapped around a cooling industrial core. Cheap coolant, tired dockworkers, and beginner freight jobs keep the outer rings alive.

Player-facing role:

- starting Station;
- first Market;
- first Job Board;
- safe hub;
- tutorial dashboard anchor.

MVP services:

- Market;
- Jobs;
- Travel;
- Cargo;
- Log;
- Help;
- Quit.

Future flavor hooks:

- Mourning Union labor office;
- Helix Cartel broker;
- refinery accident events;
- shipyard repairs.

### 5.2 Mercy Relay

Key:

- `place.mercy_relay`

Kind:

- Station
- Relay
- Port

Display name:

- Mercy Relay

Short description:

> A half-repaired comms relay serving refugee traffic and emergency broadcasts. Its med bays are always short, its antennas always sparking.

Player-facing role:

- first delivery destination;
- second Market;
- Contract completion Place;
- proof that the world remembers travel.

MVP services:

- Market;
- Jobs / Contract completion;
- Travel;
- Cargo;
- Log;
- Help;
- Quit.

Future flavor hooks:

- notice inbox hub;
- humanitarian faction work;
- relay repair shared goal;
- daily intel source.

### 5.3 Cinder Pocket

Key:

- `place.cinder_pocket`

Kind:

- Mining Rock
- Frontier Place

Display name:

- Cinder Pocket

Short description:

> A pitted mining rock coughing low-grade ore into battered cargo sleds.

Player-facing role:

- nearby flavor Place;
- source of Ore in later economy;
- optional travel destination in MVP if desired.

MVP services:

- Travel only, or limited Market if needed for flavor.

Future flavor hooks:

- pirate tolls;
- ore contracts;
- mining accident events;
- NPC merchant competition.

### 5.4 Blue Blind

Key:

- `place.blue_blind`

Kind:

- Wreck Field
- Future Salvage Site

Display name:

- Blue Blind

Short description:

> A cold scatter of hull plates and frozen signal buoys. Something large died here before the relay maps were updated.

Player-facing role:

- visible promise of future salvage;
- not mechanically active in MVP beyond travel/description.

MVP services:

- Travel only.

Future flavor hooks:

- first derelict boarding slice;
- black-box bounty;
- salvage random events;
- NPC rival Moth-9.

### 5.5 Red Maw Approach

Key:

- `place.red_maw_approach`

Kind:

- Anomaly Edge
- Dangerous Frontier

Display name:

- Red Maw Approach

Short description:

> The nebula reddens here. Instruments lag. Old captains stop joking before they cross the approach line.

Player-facing role:

- hints at danger;
- optional route endpoint;
- no combat/anomaly mechanics yet.

MVP services:

- Travel only.

Future flavor hooks:

- anomaly events;
- route instability;
- relic content;
- smuggling shortcuts.

### 5.6 Saint Vex Drift

Key:

- `place.saint_vex_drift`

Kind:

- Free Drift
- Future Black Market

Display name:

- Saint Vex Drift

Short description:

> A loose knot of ships, shrines, and unlicensed docking clamps. Nobody owns it, which means everyone taxes it.

Player-facing role:

- flavor hint for future black-market/smuggling systems;
- optional travel endpoint.

MVP services:

- Travel only.

Future flavor hooks:

- black market;
- Saints of Vacuum contact;
- smuggling contracts;
- contraband trading.

### 5.7 Dead Gate Verge

Key:

- `place.dead_gate_verge`

Kind:

- Gate
- Endgame Teaser

Display name:

- Dead Gate Verge

Short description:

> The broken mouth of an old jump gate. Its ring is dark, but every relay in the corridor still listens to it.

Player-facing role:

- long-term mystery hook;
- not mechanically active in MVP.

MVP services:

- Travel only or inaccessible route endpoint, depending on UI needs.

Future flavor hooks:

- Dead Gate campaign;
- faction shared goals;
- endgame route unlock;
- relic/anomaly content.

---

## 6. Routes

The MVP seed contains 10 directed Routes.

### 6.1 Route Design Rules

- Routes are directed.
- Travel from Ash Coil to Mercy Relay must cost 1 Daily Turn.
- Routes should imply a small corridor, not a full galaxy.
- At least one destination should be reachable only through another Place, showing the graph shape.
- Hazard flavor may exist, but no random event or combat resolution is required in MVP.

### 6.2 Route List

| Key | From | To | Cost | MVP Note |
| --- | --- | --- | ---: | --- |
| `route.ash_coil.mercy_relay` | Ash Coil | Mercy Relay | 1 | Primary tutorial route. |
| `route.mercy_relay.ash_coil` | Mercy Relay | Ash Coil | 1 | Return route. |
| `route.ash_coil.cinder_pocket` | Ash Coil | Cinder Pocket | 1 | Ore flavor route. |
| `route.cinder_pocket.ash_coil` | Cinder Pocket | Ash Coil | 1 | Return route. |
| `route.mercy_relay.blue_blind` | Mercy Relay | Blue Blind | 1 | Future salvage teaser. |
| `route.blue_blind.mercy_relay` | Blue Blind | Mercy Relay | 1 | Return from wreck field. |
| `route.blue_blind.red_maw_approach` | Blue Blind | Red Maw Approach | 2 | Risk-flavored route, no event yet. |
| `route.red_maw_approach.saint_vex_drift` | Red Maw Approach | Saint Vex Drift | 2 | One-way-feeling frontier movement. |
| `route.saint_vex_drift.mercy_relay` | Saint Vex Drift | Mercy Relay | 2 | Smuggler-return flavor. |
| `route.red_maw_approach.dead_gate_verge` | Red Maw Approach | Dead Gate Verge | 3 | Endgame teaser; may be disabled in MVP if desired. |

### 6.3 Required Route for MVP Acceptance

The only mandatory route for the first playthrough is:

- `route.ash_coil.mercy_relay`

The return route is strongly recommended:

- `route.mercy_relay.ash_coil`

Other routes provide context, future hooks, and a better Star Chart, but should not distract from the first loop.

---

## 7. Commodities

The MVP seed contains 3 Commodities.

### 7.1 Med Gel

Key:

- `commodity.med_gel`

Display name:

- Med Gel

Category:

- Staple / Medical

Short description:

> Shelf-stable trauma gel packed in silver tubes. Mercy Relay never has enough.

MVP role:

- primary tutorial Commodity;
- bought at Ash Coil;
- sold or delivered at Mercy Relay;
- tied to First Mercy Run.

Future hooks:

- humanitarian contracts;
- quarantine crises;
- black-market organ clinics;
- Mourning Union reputation.

### 7.2 Reactor Coolant

Key:

- `commodity.reactor_coolant`

Display name:

- Reactor Coolant

Category:

- Industrial

Short description:

> Blue-white thermal slurry used by stations that cannot afford to shut down.

MVP role:

- secondary trade Commodity;
- shows Markets can support more than one good.

Future hooks:

- refinery economy;
- station emergency contracts;
- ship repair/refuel systems;
- blockade scarcity.

### 7.3 Ore

Key:

- `commodity.ore`

Display name:

- Low-Grade Ore

Category:

- Industrial / Raw

Short description:

> Common rock with uncommon impurities. Worth little until a refinery gets desperate.

MVP role:

- simple third Commodity;
- source flavor for Cinder Pocket and Ash Coil.

Future hooks:

- production chains;
- outpost construction;
- mining contracts;
- NPC merchant routes.

---

## 8. Starter Ship

### 8.1 Rustbucket Mule

Key:

- `ship.rustbucket_mule`

Display name:

- Rustbucket Mule

Short description:

> A stained little cargo hauler with more patched seams than original plating. Ugly, honest, and barely yours.

MVP role:

- starter Ship;
- supports the first cargo loop;
- has no modules in MVP.

Suggested player-facing stats:

- modest cargo capacity;
- no special module behavior;
- no combat role;
- no fuel complexity in MVP unless already desired.

Design requirements:

- cargo capacity must be large enough to carry meaningful Med Gel for First Mercy Run;
- capacity must be small enough that capacity UI matters;
- Ship identity appears on dashboard.

Future hooks:

- upgrades;
- hull damage;
- modules;
- insurance;
- named-ship history.

---

## 9. Starting Captain Assumptions

A new Captain starts with:

- current Place: Ash Coil;
- Ship: Rustbucket Mule;
- Daily Turns: 30;
- enough credits to buy Med Gel for First Mercy Run;
- no Cargo, or only harmless starter flavor Cargo;
- no active Heat;
- no faction membership;
- no accepted Contracts;
- no completed Contracts.

Design intent:

- the player should be able to accept and complete the first Contract without prior knowledge;
- the player should have enough credits to make a mistake but not enough to ignore basic tradeoffs forever.

---

## 10. Markets

The MVP seed contains 2 active Station Markets.

### 10.1 Ash Coil Market

Key:

- `market.ash_coil`

Place:

- Ash Coil

Market fiction:

> The refinery commissary sells industrial surplus and whatever relief crates fell off official manifests.

MVP behavior:

- sells Med Gel at a tutorial-friendly price;
- sells Reactor Coolant;
- may buy Ore or Reactor Coolant if needed;
- should make Med Gel attractive for Mercy Relay.

Required Commodity role:

- Med Gel must be available in sufficient stock for First Mercy Run.

Optional flavor line:

> MED GEL SURPLUS: Relief tubes priced to move before the next audit.

### 10.2 Mercy Relay Market

Key:

- `market.mercy_relay`

Place:

- Mercy Relay

Market fiction:

> Mercy Relay pays too much for anything that keeps bodies alive and antennas cold.

MVP behavior:

- buys Med Gel above Ash Coil’s sell price;
- may buy Reactor Coolant;
- may sell little or nothing useful in MVP;
- exists primarily as destination market.

Required Commodity role:

- Med Gel must be profitable when moved from Ash Coil to Mercy Relay.

Optional flavor line:

> EMERGENCY DEMAND: Med bays operating past rated capacity.

### 10.3 Pricing Philosophy for MVP

Use simple authored prices.

Do not require:

- dynamic pricing;
- equilibrium drift;
- NPC market movement;
- regional modifiers;
- faction discounts;
- price rumors.

The seed should be compatible with those future systems, but not depend on them.

---

## 11. First Contract

### 11.1 First Mercy Run

Key:

- `contract.first_mercy_run`

Display name:

- First Mercy Run

Issuer:

- Ash Coil dock clerk / relief broker

Origin:

- Ash Coil

Destination:

- Mercy Relay

Contract type:

- Delivery

Required Commodity:

- Med Gel

Short description:

> Mercy Relay’s med bays are burning through trauma gel faster than Ash Coil can misplace the surplus. Carry a small lot through the corridor and make yourself useful.

Player-facing objective:

- Bring Med Gel from Ash Coil to Mercy Relay.

Reward:

- credits;
- optional small Event Log recognition;
- no faction standing in MVP unless used as flavor only.

MVP state flow:

1. available at Ash Coil;
2. accepted by Captain;
3. completed at Mercy Relay when requirements are met;
4. reward granted;
5. completion Event recorded;
6. not repeatable by the same Captain without reset.

Failure conditions:

- none required in MVP beyond inability to complete without the required Commodity or location.

Disabled choice examples:

- “Complete First Mercy Run — requires Med Gel in Cargo.”
- “Complete First Mercy Run — must be at Mercy Relay.”

Future hooks:

- Mourning Union reputation;
- Mercy Relay shared goal;
- follow-up notice;
- NPC rival undercutting relief routes;
- emergency supply chain.

---

## 12. Initial Event and News Flavor

The MVP can include a small static news/help signal to make the world feel less empty without adding simulation.

### 12.1 Optional Initial News Lines

- “Mercy Relay requests medical freight after another antenna-bay accident.”
- “Ash Coil refinery reports surplus relief stock awaiting transport.”
- “Blue Blind wreck field remains under advisory: salvage licenses not yet available.”
- “Dead Gate Verge remains dark.”

### 12.2 Event Log Expectations

The Event Log should become meaningful through player action.

Minimum required Event:

- First Mercy Run completion.

Optional Events:

- Captain first launched;
- Captain traveled from Ash Coil to Mercy Relay;
- Captain completed first trade.

Design rule:

- Event text must be short, terminal-friendly, and not expose private data.

---

## 13. Help Text Seed

The Help screen should explain the MVP without mentioning unbuilt systems as if they are playable.

### 13.1 Suggested Help Topics

- What is a Captain?
- What is a Daily Turn?
- How do I make money?
- How do Contracts work?
- How do I travel?
- How do I quit safely?

### 13.2 Suggested First-Run Text

> You are a new Captain in the Ash Mercy Corridor. Ash Coil has Med Gel. Mercy Relay needs it. Accept the First Mercy Run, buy cargo, travel the route, sell or deliver at Mercy Relay, and check the Log when you are done.

### 13.3 Explicit Future-System Wording

If future systems are mentioned, use locked/coming-soon language:

- “Salvage licenses are not available in this slice.”
- “Faction offices are closed for now.”
- “The Dead Gate is silent.”

Avoid making MVP players think a broken or unfinished system is available.

---

## 14. Star Chart / Recall Seed Expectations

The Star Chart or Recall screen should support the MVP by showing known Places.

### 14.1 Initial Recall

At first launch, the player should know:

- Ash Coil;
- Mercy Relay as a known destination;
- perhaps Cinder Pocket as a visible nearby place.

### 14.2 On Visit

When the Captain visits a Place, Recall should record it as seen.

### 14.3 Stale Intel

Stale intel is not required in MVP.

Optional copy for later:

> Market snapshots and stale route intel will matter in later slices.

---

## 15. UI Flavor Anchors

### 15.1 Ash Coil Dashboard Signal Ideas

- “Dock crews are arguing over relief crates.”
- “Mercy Relay traffic marked priority.”
- “Refinery heat shimmer visible through outer glass.”

### 15.2 Mercy Relay Dashboard Signal Ideas

- “Emergency lights pulse behind cracked med-bay windows.”
- “Relay dishes twitch toward the Dead Gate between broadcasts.”
- “A clerk is still stamping forms from three disasters ago.”

### 15.3 Travel Arrival Text

Ash Coil to Mercy Relay:

> The Rustbucket Mule clears Ash Coil’s refinery glare and follows the mercy beacons through thin static. One turn later, Mercy Relay catches your transponder and begs you not to block the med-bay dock.

Return route:

> Mercy Relay falls behind in a scatter of blue-white signal bursts. Ash Coil’s industrial glow returns like a bruise on the dark.

---

## 16. MVP Content Acceptance Criteria

The content seed is acceptable when:

- all required Places have keys, names, kinds, and descriptions;
- all required Routes have keys, endpoints, and turn costs;
- Med Gel supports the first profitable trade loop;
- Ash Coil and Mercy Relay both have coherent market roles;
- First Mercy Run has origin, destination, requirement, and reward concept;
- starter Ship has a clear identity and cargo role;
- no excluded advanced system is required for the MVP to make sense;
- flavor hints point toward the larger GDD without implying unavailable mechanics are broken.

---

## 17. Deferred Content Hooks

The following hooks are intentionally present but inactive.

### 17.1 Blue Blind

Future slice:

- first derelict boarding map;
- salvage tutorial;
- black-box bounty.

### 17.2 Saint Vex Drift

Future slice:

- black market;
- smuggling;
- Saints of Vacuum contact;
- contraband.

### 17.3 Red Maw Approach

Future slice:

- route hazards;
- anomaly events;
- dangerous travel;
- relic precursor content.

### 17.4 Dead Gate Verge

Future slice:

- faction shared goal;
- endgame unlock;
- season campaign.

### 17.5 Cinder Pocket

Future slice:

- Ore economy;
- mining contracts;
- NPC trade competition;
- pirate pressure.

---

## 18. Open Content Questions

1. Should the Dead Gate Verge route be visible but disabled, or hidden entirely in MVP?
2. Should Cinder Pocket have an active Market, or be travel-only until the economy expands?
3. Should First Mercy Run require generic Med Gel or special sealed relief cargo?
4. Should the player need to sell Med Gel and then complete the Contract, or should Contract completion consume the Med Gel directly?
5. Should Mercy Relay have a return Contract to Ash Coil, or would that distract from the first slice?
6. Should the starter Ship be named by default or ask the player to name it?
7. Should Ash Coil and Mercy Relay be considered Safe Zones in this seed?
8. Should initial news lines be static flavor or actual Events?

---

## 19. Recommended Content Defaults

Use these defaults unless changed deliberately:

1. Dead Gate Verge is visible but unreachable or clearly marked as silent/locked.
2. Cinder Pocket is reachable but has no full Market in MVP.
3. First Mercy Run uses generic Med Gel, not special sealed cargo.
4. Contract completion consumes or verifies Med Gel according to whichever creates the clearer first playthrough, but it must not allow infinite reward loops.
5. No return Contract yet.
6. Starter Ship display name is Rustbucket Mule; player naming can wait.
7. Ash Coil and Mercy Relay are Safe Zones.
8. Initial news lines are static flavor; player completion creates the first meaningful Event.

---

## 20. Next Step After Content Seed

Once this seed is accepted, the next documentation step is a real implementation plan for the MVP vertical slice.

That plan should remain aligned with:

- this content seed;
- the MVP acceptance criteria;
- the systems inventory;
- the glossary vocabulary;
- Foglet game-kit capabilities.
