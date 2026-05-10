# Drop Dead Nebula — Implementation Readiness Checklist

Status: Draft v0.1  
Purpose: Bridge design documentation and the first implementation plan. This is a gate, not an implementation plan.

---

## 1. Readiness Summary

Drop Dead Nebula is nearing implementation readiness for **Milestone 0: MVP — First Mercy Run**, but a few decisions should be made first.

The design docs are broad and mature enough to support planning. The first implementation plan should remain tightly scoped to the MVP:

> Create/resume Captain → start at Ash Coil → accept First Mercy Run → buy Med Gel → travel to Mercy Relay → sell/deliver → complete Contract → see Event Log → quit → resume intact.

---

## 2. Documentation Completeness

### 2.1 Core Docs

- [x] GDD / vision: `DROP_DEAD_NEBULA.md`
- [x] Glossary: `DROP_DEAD_NEBULA_GLOSSARY.md`
- [x] Systems inventory: `DROP_DEAD_NEBULA_SYSTEMS.md`
- [x] MVP vertical slice: `DROP_DEAD_NEBULA_MVP.md`
- [x] MVP content seed: `DROP_DEAD_NEBULA_CONTENT_SEED.md`
- [x] Game-kit wishlist: `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`
- [x] Screen/UX design: `DROP_DEAD_NEBULA_SCREEN_DESIGN.md`
- [x] Milestones: `DROP_DEAD_NEBULA_MILESTONES.md`

### 2.2 Deep Dives

- [x] World Ticks
- [x] NPCs
- [x] Economy
- [x] Jobs
- [x] Risk Events
- [x] Salvage
- [x] Factions
- [x] Campaign
- [x] Corporations
- [x] Outposts
- [x] Ships
- [x] Combat
- [x] Deployables
- [x] Social Safety
- [x] Admin/Ops
- [x] Progression/Balance

### 2.3 Closing Docs

- [x] Documentation Index
- [x] Open Questions
- [x] Implementation Readiness Checklist
- [ ] Decision Log, optional later
- [ ] Test Strategy, optional before or during implementation planning

---

## 3. MVP Scope Readiness

### 3.1 MVP Scope Stable

The following are stable:

- trade-route slice first;
- Ash Coil as starting Station;
- Mercy Relay as destination Station;
- Med Gel as tutorial Commodity;
- First Mercy Run as tutorial Contract;
- Rustbucket Mule as starter Ship;
- 7-Place seed world;
- no salvage/combat/factions/bounties in MVP;
- Event Log before full Notices;
- authored prices before dynamic economy.

### 3.2 MVP Scope Risks

Potential scope creep:

- adding random events too early;
- adding Star Chart polish before core loop works;
- implementing full Contract framework before proving one Contract;
- adding real Notice inbox in MVP;
- adding fuel/damage before repair/recovery exists;
- treating future Places as playable systems.

Mitigation:

- use `DROP_DEAD_NEBULA_MVP.md` as hard scope boundary.

---

## 4. Content Readiness

### 4.1 Seed Content Ready

`DROP_DEAD_NEBULA_CONTENT_SEED.md` defines:

- Ash Coil;
- Mercy Relay;
- Cinder Pocket;
- Blue Blind;
- Red Maw Approach;
- Saint Vex Drift;
- Dead Gate Verge;
- 10 directed Routes;
- Med Gel;
- Reactor Coolant;
- Low-Grade Ore;
- Rustbucket Mule;
- First Mercy Run;
- initial help/news flavor.

### 4.2 Content Decisions Still Needed

Before implementation plan:

- Does First Mercy Run consume Med Gel or verify a sale?
- Does Dead Gate Verge appear as visible locked destination or hidden future hook?
- Does Cinder Pocket have any active Market in MVP or travel-only flavor?

Recommended defaults:

- First Mercy Run consumes or directly delivers Med Gel for clarity unless trade tutorial requires sell-first.
- Dead Gate Verge visible but clearly silent/locked.
- Cinder Pocket travel-only in MVP.

---

## 5. UX Readiness

### 5.1 MVP Screens Defined

`DROP_DEAD_NEBULA_SCREEN_DESIGN.md` defines:

- Title / Resume;
- Dashboard;
- Travel;
- Market;
- Jobs / Contract Board;
- Cargo Manifest;
- Event Log;
- Help;
- result/confirmation modals.

### 5.2 UX Acceptance Rules

Every MVP screen should:

- work at 80x24;
- show current context;
- show back/quit path;
- explain disabled choices;
- use glossary terms consistently;
- provide immediate feedback after action.

---

## 6. Game-Kit Dependency Readiness

### 6.1 Required Existing Kit Concepts

The MVP expects support for or access to:

- external PTY runtime;
- terminal safety;
- screen stack;
- prompt/menu input;
- per-user save or save-on-quit behavior;
- shared world DB;
- player registry / Foglet context;
- Daily Turns;
- Event Log;
- spatial Places/Routes;
- Presence;
- Place Recall, at least minimal;
- owner-keyed inventory;
- Market/listing or equivalent transaction support.

### 6.2 P0 Wishlist Items to Review

Before implementation, decide whether to implement these as kit primitives or MVP-local game code:

- Contract primitive + unified Job Board surface;
- Spatial Travel transaction helper;
- Owner Inventory capacity helper;
- Event Log / News screen;
- multi-user local-dev test harness.

Recommendation:

- avoid blocking MVP on every wishlist item;
- strongly consider generic helpers where existing kit code is already close;
- keep Drop Dead Nebula-specific balance out of the kit.

---

## 7. Open Questions Blocking Implementation Plan

Resolve these first:

1. **Repo location:** in `foglet-game-kit-rs` workspace or separate game repo?
2. **P0 kit strategy:** generic helpers first or MVP-local code first?
3. **First Mercy Run semantics:** consume Med Gel, verify sale, or support both?
4. **Credits model:** Captain field for MVP?
5. **Daily Turn reset:** real reset in M0 or defer to M2?

Everything else can wait.

---

## 8. Recommended First Implementation Plan Shape

When ready, the first implementation plan should be titled something like:

`DROP_DEAD_NEBULA_M0_IMPLEMENTATION_PLAN.md`

It should include tasks for:

1. project/repo placement;
2. minimal game config;
3. Captain creation/resume;
4. seed world loading/creation;
5. starter Ship and Cargo;
6. Daily Turn display/spend;
7. Travel between Places;
8. Market buy/sell;
9. First Mercy Run Contract;
10. Event Log;
11. MVP screens;
12. save/quit/resume;
13. manual playthrough verification;
14. local-dev multi-user smoke if feasible.

Do not include:

- random events;
- world ticks beyond what is necessary;
- NPCs;
- factions;
- salvage;
- combat;
- bounties;
- outposts;
- corporations.

---

## 9. Suggested Pre-Implementation Decision Session

Before writing the implementation plan, answer these in order:

1. Where will the game live?
2. Do we want any game-kit P0 wishlist items implemented first?
3. What exactly happens when completing First Mercy Run?
4. Is Daily Turn reset part of M0?
5. What is the first manual acceptance script?

After that, implementation planning can begin safely.

---

## 10. Current Readiness Verdict

Design readiness:

- **High.** The product vision, MVP, content seed, screens, milestones, and major system deep dives exist.

Implementation readiness:

- **Almost ready.** Needs the small set of blocking decisions in Section 7.

Recommended next action:

- Resolve the five blocking questions, then write the Milestone 0 implementation plan.
