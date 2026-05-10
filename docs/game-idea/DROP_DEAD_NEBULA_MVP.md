# Drop Dead Nebula — MVP Vertical Slice

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`, `DROP_DEAD_NEBULA_CONTENT_SEED.md`  
Purpose: Define the first playable slice without drifting into the full dream game.

---

## 1. MVP North Star

The MVP should prove the shared-world trading spine of Drop Dead Nebula:

> A player can create or resume a Captain, start at Ash Coil, buy Med Gel, spend a Daily Turn traveling to Mercy Relay, sell for profit, complete one delivery Contract, see the action reflected in the Event Log, quit safely, and resume with state intact.

The MVP is intentionally a **trade-route slice**, not a salvage slice, combat slice, faction slice, or colony slice. It should validate that the game can stand on top of the Foglet game-kit primitives before adding the more ambitious systems.

---

## 2. Design Constraints

### 2.1 Must Follow Existing Docs

This MVP uses terms and system boundaries from:

- `DROP_DEAD_NEBULA.md`
- `DROP_DEAD_NEBULA_GLOSSARY.md`
- `DROP_DEAD_NEBULA_SYSTEMS.md`

Canonical terms:

- Captain
- Ship
- Place
- Sector
- Route
- Station
- Port
- Commodity
- Cargo
- Market
- Contract
- Event
- Notice
- Daily Turn
- World Tick
- Recall
- Presence
- Heat

### 2.2 Do Not Solve the Whole Game

The MVP should exclude any system that is not necessary to prove the first route-trading loop.

The guiding question is:

> Does this help the player complete one satisfying buy-travel-sell-contract loop and resume later?

If not, defer it.

### 2.3 No Specific Implementation Details

This document describes design requirements and system mapping only. It intentionally avoids:

- file paths for code;
- schema definitions;
- Rust type names beyond game-kit concepts;
- function names;
- module organization;
- test implementation mechanics.

---

## 3. MVP Player Experience

### 3.1 First Login

The player enters Drop Dead Nebula as a Captain with a starter Ship at **Ash Coil**.

The player should immediately understand:

- where they are;
- how many Daily Turns they have;
- how many credits they have;
- what their Ship can carry;
- what Commodity is worth buying;
- where they can travel;
- what simple Contract is available;
- how to quit safely.

### 3.2 First Session Story

The first session should produce a compact story:

1. The Captain docks at Ash Coil.
2. The dashboard shows 30 Daily Turns.
3. The Market shows Med Gel is available.
4. The Job Board offers a delivery Contract to Mercy Relay.
5. The player buys Med Gel.
6. The player travels along a directed Route to Mercy Relay.
7. Travel spends one Daily Turn.
8. Mercy Relay buys Med Gel at a profit.
9. The player sells Med Gel.
10. The player completes the Contract.
11. The Event Log records the successful delivery.
12. The player quits.
13. On next login, the Captain, credits, location, turns spent, cargo state, and completed Contract state are preserved.

### 3.3 Emotional Target

The MVP should feel like:

- “I understand the loop.”
- “The world remembered me.”
- “This could grow into the bigger game.”

It does not yet need to feel dangerous, deep, or socially alive.

---

## 4. Included Systems

The MVP includes only the following systems from `DROP_DEAD_NEBULA_SYSTEMS.md`.

### 4.1 Game Configuration

Purpose in MVP:

- define the title;
- define starting Place;
- define starting turns;
- define starting credits;
- define starting Ship assumptions;
- point to the authored seed content.

Game-kit mapping:

- uses the game-kit configuration pattern for terminal game setup and packaging metadata.

MVP acceptance:

- the game presents itself as Drop Dead Nebula;
- the minimum terminal experience remains 80x24-friendly;
- starting values are consistent across fresh local-dev sessions unless intentionally reset.

### 4.2 Content Key Registry

Purpose in MVP:

- keep Places, Routes, Commodities, Markets, and Contract definitions stable.

Game-kit mapping:

- Place keys map to spatial graph Places;
- Commodity keys map to owner-keyed inventory item keys;
- Contract keys remain game-defined.

MVP acceptance:

- every authored Place, Route, Commodity, and Contract in the seed has a stable key;
- display names are separate from keys.

### 4.3 Captain and Player Identity

Purpose in MVP:

- map the Foglet caller to a Captain;
- show Captain identity in the dashboard;
- preserve Captain progress.

Game-kit mapping:

- uses Foglet context;
- uses the shared-world player registry concept;
- may use role/security-level flavor only as nonessential display.

MVP acceptance:

- first launch creates or resumes a Captain;
- repeated launches for the same caller resume the same Captain;
- local-dev fake users can represent distinct Captains.

### 4.4 Save and Resume

Purpose in MVP:

- make BBS-style quit/resume reliable;
- preserve private preferences and shared state boundaries.

Game-kit mapping:

- uses terminal-safe exit behavior;
- uses per-user save where appropriate;
- uses save-on-quit behavior where appropriate;
- uses shared world DB for world-facing state.

MVP acceptance:

- quitting from the dashboard is safe;
- relaunching resumes the Captain at the correct Place with correct state;
- no gameplay output corrupts the terminal on exit.

### 4.5 UI Shell and Dashboard

Purpose in MVP:

- provide a single main command center.

Required dashboard information:

- game title;
- Captain name or handle;
- current Ship;
- current Place/Sector;
- Daily Turns remaining;
- credits;
- cargo summary;
- local signals or short hints;
- hotkey menu.

Required dashboard actions:

- Travel;
- Market;
- Jobs;
- Cargo;
- Log;
- Help;
- Quit.

Game-kit mapping:

- uses screens;
- uses input normalization;
- uses prompt/hotkey patterns;
- uses disabled choices where applicable.

MVP acceptance:

- all major MVP actions are reachable by keyboard;
- unavailable actions explain why;
- Help explains the MVP loop.

### 4.6 Spatial World Bootstrap

Purpose in MVP:

- create a tiny authored nebula segment.

Game-kit mapping:

- uses Places and directed Routes from the spatial graph feature set.

MVP acceptance:

- the seed world includes 5–8 Places;
- Routes are directed;
- at least one Route demonstrates that outbound options differ by current Place;
- the player starts at Ash Coil.

### 4.7 Presence and Location

Purpose in MVP:

- track current Captain location.

Game-kit mapping:

- uses player Presence.

MVP acceptance:

- Captain has one current Place;
- travel updates current Place;
- station services reflect current Place.

### 4.8 Place Recall and Star Chart

Purpose in MVP:

- establish the idea that the player remembers visited Places.

Game-kit mapping:

- uses Place Recall.

MVP acceptance:

- visiting a Place records recall;
- Star Chart or Recall screen can show visited Places;
- Recall does not yet need stale market snapshots.

### 4.9 Turns and Action Economy

Purpose in MVP:

- make travel feel like a BBS door-game decision.

Game-kit mapping:

- uses Daily Turn ledger.

MVP acceptance:

- Captain starts with 30 Daily Turns;
- normal travel costs 1 Daily Turn;
- travel is disabled when turns are insufficient;
- turn spending is preserved across resume;
- turn reset behavior may be simple but must be conceptually compatible with daily reset.

### 4.10 Travel and Route Resolution

Purpose in MVP:

- move the Captain between Places.

Game-kit mapping:

- combines Routes, Presence, Turns, Recall, and Event Log.

MVP acceptance:

- Travel screen lists outbound Routes from current Place;
- choosing a Route spends the appropriate turn cost;
- choosing a Route moves Presence;
- arrival is clearly reported;
- travel to Mercy Relay is possible from Ash Coil.

### 4.11 Ship Model

Purpose in MVP:

- provide cargo capacity and identity.

MVP ship:

- one starter Ship;
- one display name;
- fixed cargo capacity;
- no modules required;
- no combat stats required.

Game-kit mapping:

- Ship cargo uses owner-keyed inventory concepts.

MVP acceptance:

- dashboard shows Ship name/type;
- cargo capacity constrains purchases;
- cargo state persists.

### 4.12 Cargo and Inventory

Purpose in MVP:

- track goods carried by the Ship and stocked by Markets.

Game-kit mapping:

- uses owner-keyed inventory;
- uses atomic transfer concepts for buy/sell/contract turn-in.

MVP acceptance:

- Ship can carry Commodities;
- Station Markets own stock;
- buying transfers Commodity into Cargo;
- selling transfers Commodity out of Cargo;
- capacity limits produce disabled choices or clear failure text.

### 4.13 Commodity and Item Catalog

Purpose in MVP:

- define the few goods needed for the first trade loop.

MVP Commodities:

- Med Gel;
- Reactor Coolant;
- Ore.

Game-kit mapping:

- Commodity keys become item keys for inventory and market operations.

MVP acceptance:

- each Commodity has key, display name, short description, and basic trade role;
- Med Gel is profitable from Ash Coil to Mercy Relay.

### 4.14 Market and Pricing

Purpose in MVP:

- enable buy-low/sell-high trading.

Game-kit mapping:

- uses market concepts and owner-keyed inventory transfers;
- pricing remains game-authored for MVP.

MVP acceptance:

- Ash Coil sells Med Gel;
- Mercy Relay buys Med Gel at a higher price;
- buying requires credits;
- selling grants credits;
- Market screen shows price, stock, and cargo/capacity context;
- stock changes after trade.

### 4.15 Contracts and Job Board

Purpose in MVP:

- give a clear objective beyond raw arbitrage.

MVP Contract:

- one delivery Contract from Ash Coil to Mercy Relay involving Med Gel or relief supplies.

Game-kit mapping:

- uses game-defined Contract system on top of shared DB, turns, inventory transfers, notices/events as needed;
- does not require the kit to already have a generic contract primitive.

MVP acceptance:

- Job Board shows the Contract;
- player can accept it;
- player can complete it at Mercy Relay if requirements are met;
- reward is granted;
- completion is recorded;
- completed Contract is not repeatedly claimable by the same Captain unless explicitly reset.

### 4.16 Event Log and News

Purpose in MVP:

- prove shared-world memory.

Game-kit mapping:

- uses append-only Event Log.

MVP acceptance:

- travel, trade, and Contract completion can append Events where appropriate;
- Log screen shows recent Events;
- Contract completion creates a satisfying event line.

### 4.17 Help and Onboarding

Purpose in MVP:

- make the first loop obvious.

Game-kit mapping:

- uses text blocks and prompt/menu UI.

MVP acceptance:

- Help screen explains: buy cargo, travel, sell cargo, complete Contract, quit;
- first-run text can be dismissed or simply read once.

---

## 5. Explicitly Excluded from MVP

The following are excluded even though they are important to the full game.

### 5.1 Excluded Living World Systems

- NPC Captains;
- background NPC trade;
- dynamic contract generation;
- rumor generation;
- low-population boosts;
- regional drift.

Reason:

- these are not needed to prove the core trading spine.

### 5.2 Excluded Signature Systems

- salvage and derelict boarding;
- local ASCII derelict maps;
- random travel event tables;
- combat;
- smuggling;
- heat/customs beyond placeholder display;
- traps/mines/drones.

Reason:

- these add content and rules complexity before the shared-world basics are proven.

### 5.3 Excluded Strategic Systems

- factions;
- shared goals;
- bounties;
- async challenges;
- outposts;
- colonies;
- corporations/charters;
- season phases;
- Dead Gate campaign.

Reason:

- these should hang from the stable MVP spine later.

### 5.4 Excluded Authoring/Operations Systems

- world generator;
- admin screens;
- content validation tooling;
- economic balancing harness;
- multiplayer local-dev scenario harness.

Reason:

- valuable later, but not required for first playable proof.

---

## 6. MVP Seed World Summary

The detailed seed is specified in `DROP_DEAD_NEBULA_CONTENT_SEED.md`.

The MVP seed must include:

- 5–8 Places;
- 8–12 directed Routes;
- 3 Commodities;
- 2 active Station Markets;
- 1 starter Ship;
- 1 delivery Contract;
- simple starting values.

Required named anchors:

- **Ash Coil** — starting Station and first Market;
- **Mercy Relay** — destination Station and profitable Med Gel buyer;
- **Med Gel** — primary tutorial Commodity;
- **First Mercy Run** — first delivery Contract.

---

## 7. Required MVP Screens

### 7.1 Title / Resume Screen

Shows:

- game title;
- new/resume affordance;
- brief one-line premise.

### 7.2 Dashboard

Shows:

- Captain;
- Ship;
- Place;
- Daily Turns;
- credits;
- cargo summary;
- main hotkeys.

### 7.3 Travel Screen

Shows:

- current Place;
- outbound Routes;
- destination names;
- turn cost;
- basic route note;
- disabled state if insufficient turns.

### 7.4 Market Screen

Shows:

- current Station Market;
- Commodity name;
- buy/sell price;
- stock;
- player cargo quantity;
- cargo capacity;
- disabled reasons when purchase is impossible.

### 7.5 Jobs Screen

Shows:

- available Contract;
- accepted Contract;
- completion requirements;
- reward;
- state.

### 7.6 Cargo Screen

Shows:

- current Cargo;
- capacity used;
- Commodity descriptions.

### 7.7 Log Screen

Shows:

- recent Events;
- at minimum, the player’s recent travel/trade/Contract completion history.

### 7.8 Help Screen

Shows:

- controls;
- core loop;
- terms: Captain, Ship, Cargo, Market, Route, Daily Turn, Contract.

---

## 8. Manual Playthrough Script

This script defines the intended MVP experience.

1. Launch Drop Dead Nebula.
2. Resume or create Captain.
3. Confirm the Captain starts at Ash Coil.
4. Confirm dashboard shows 30 Daily Turns.
5. Open Jobs.
6. View **First Mercy Run** Contract.
7. Accept the Contract.
8. Open Market at Ash Coil.
9. Buy Med Gel.
10. Confirm credits decrease and Cargo increases.
11. Return to dashboard.
12. Open Travel.
13. Choose Route to Mercy Relay.
14. Confirm one Daily Turn is spent.
15. Confirm current Place becomes Mercy Relay.
16. Open Market at Mercy Relay.
17. Sell Med Gel.
18. Confirm credits increase.
19. Open Jobs.
20. Complete First Mercy Run.
21. Confirm reward is granted.
22. Open Log.
23. Confirm the delivery appears in recent Events.
24. Quit safely.
25. Relaunch.
26. Confirm Captain resumes at Mercy Relay with updated credits, turns, cargo, and Contract state.

---

## 9. Acceptance Criteria

The MVP is complete when all of the following are true.

### 9.1 Identity and Resume

- A Foglet/local-dev user maps to one persistent Captain.
- A new Captain starts at Ash Coil.
- Returning as the same user resumes the same Captain.
- A different local-dev user can have separate Captain state.

### 9.2 World and Travel

- The seed world contains the required Places and Routes.
- Travel options depend on current Place.
- Travel from Ash Coil to Mercy Relay is available.
- Travel spends Daily Turns.
- Travel updates Presence.
- Visiting a Place updates Recall.

### 9.3 Trading

- Ash Coil sells Med Gel.
- Mercy Relay buys Med Gel at a profit.
- Buying changes credits, Cargo, and Market stock.
- Selling changes credits, Cargo, and Market stock.
- Cargo capacity is enforced.
- Insufficient credits or cargo capacity produces clear disabled choices or clear feedback.

### 9.4 Contract

- First Mercy Run is visible at Ash Coil.
- It can be accepted.
- It can be completed at Mercy Relay when requirements are met.
- It grants a reward.
- It records completion.
- It cannot be completed repeatedly without a reset.

### 9.5 Event Log

- Contract completion creates an Event.
- The Log screen shows the Event.
- Events are readable and terminal-friendly.

### 9.6 Terminal and BBS UX

- All MVP actions are keyboard-accessible.
- Quit is obvious.
- The game remains readable at 80x24.
- Terminal state is restored after quit.
- No gameplay logs are printed outside the TUI while active.

### 9.7 Scope Control

- MVP does not include salvage, combat, factions, bounties, async challenges, outposts, corporations, mines, or world generation.
- Placeholder references to those systems are allowed only if clearly marked as unavailable or future content.

---

## 10. Test and Verification Checklist

This is a design-level checklist, not an implementation plan.

### 10.1 Identity Tests

- new user creates Captain;
- same user resumes Captain;
- two users remain distinct.

### 10.2 World Seed Tests

- required Places exist;
- required Routes exist;
- Ash Coil is starting Place;
- Mercy Relay is reachable from Ash Coil.

### 10.3 Turn Tests

- starting turns are correct;
- travel spends one turn;
- insufficient turns prevents travel.

### 10.4 Market Tests

- Ash Coil has Med Gel stock;
- Mercy Relay buys Med Gel profitably;
- buying changes stock/cargo/credits;
- selling changes stock/cargo/credits;
- capacity and credit limits are enforced.

### 10.5 Contract Tests

- First Mercy Run can be accepted;
- completion requires correct location and goods/state;
- reward is granted once;
- completion event is recorded.

### 10.6 Resume Tests

- after quit and relaunch, location persists;
- credits persist;
- cargo persists;
- turn state persists;
- Contract state persists.

### 10.7 UI Smoke Tests

- dashboard renders;
- travel screen renders;
- market screen renders;
- jobs screen renders;
- cargo screen renders;
- log screen renders;
- help screen renders;
- quit path works.

---

## 11. MVP Non-Goals

The MVP must not be judged against the full GDD. Specifically, the MVP is not trying to prove:

- living galaxy simulation;
- low-pop NPC richness;
- dynamic economy;
- TradeWars-like depth;
- salvage identity;
- faction politics;
- PvP;
- long-term retention;
- procedural generation.

It is only trying to prove:

- the shared-world trading spine works;
- the UI loop is understandable;
- the game-kit feature mapping is viable;
- the seed content can produce one satisfying short session.

---

## 12. Questions to Resolve Before Implementation Planning

1. Should Drop Dead Nebula initially live as an example inside `foglet-game-kit-rs`, or in a separate consuming game project?
2. Is Med Gel the right tutorial Commodity, or should the first loop use a more neutral relief-supply item?
3. Should trading itself cost a Daily Turn in MVP, or only travel?
4. Should credits be treated as Captain state or as a ledger-like resource from the start?
5. Should First Mercy Run require buying generic Med Gel, or should it provide dedicated Contract cargo?
6. Should the Log show only global Events, or a filtered player-local view first?
7. Should Daily Turn reset be implemented in MVP, or is persistent spent-turn state enough for the first local proof?
8. Should the MVP include any Notice at all, or defer Notices until the living-world slice?

---

## 13. Recommended Answers for MVP

To reduce ambiguity, use these defaults unless later overruled:

1. **Trade-route slice first.** Salvage comes second.
2. **Travel costs turns; trading does not.** This keeps first session simple.
3. **Med Gel remains the tutorial Commodity.** It matches the GDD’s Mercy Relay example.
4. **First Mercy Run requires delivering Med Gel.** Keep it easy to understand.
5. **Use authored seed content.** No generator yet.
6. **Use simple fixed prices.** Dynamic pricing comes later.
7. **Use one starter Ship.** No modules yet.
8. **Use Event Log before Notices.** Notices can arrive in the next slice.
9. **No random events yet.** The first loop should be deterministic.
10. **No combat or failure beyond insufficient resources.**

---

## 14. Next Document Dependency

This MVP depends on `DROP_DEAD_NEBULA_CONTENT_SEED.md` for the actual authored seed:

- Places;
- Routes;
- Commodities;
- Markets;
- starter Ship;
- starting Captain assumptions;
- First Mercy Run Contract;
- initial Events or help text.

Once the MVP and content seed are accepted, the next step should be a true implementation plan.
