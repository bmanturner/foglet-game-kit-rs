# Drop Dead Nebula — Documentation Index

Status: Draft v0.1  
Purpose: Map the Drop Dead Nebula documentation set, define recommended reading order, and identify which docs are authoritative for vocabulary, scope, roadmap, and readiness.

---

## 1. What This Index Is For

The Drop Dead Nebula docs are now large enough that future readers need a map. This index answers:

- What does each document cover?
- Which document should I read first?
- Which docs are authoritative for specific decisions?
- Which docs are design-only versus readiness/planning docs?
- Where should future implementation plans draw context from?

---

## 2. Recommended Reading Order

### 2.1 For Project Vision

1. `DROP_DEAD_NEBULA.md`
2. `DROP_DEAD_NEBULA_GLOSSARY.md`
3. `DROP_DEAD_NEBULA_SYSTEMS.md`
4. `DROP_DEAD_NEBULA_MILESTONES.md`

### 2.2 For MVP Implementation Planning

1. `DROP_DEAD_NEBULA_IMPLEMENTATION_READINESS.md`
2. `DROP_DEAD_NEBULA_MVP.md`
3. `DROP_DEAD_NEBULA_CONTENT_SEED.md`
4. `DROP_DEAD_NEBULA_SCREEN_DESIGN.md`
5. `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`
6. relevant deep dives:
   - `DROP_DEAD_NEBULA_ECONOMY.md`
   - `DROP_DEAD_NEBULA_JOBS.md`
   - `DROP_DEAD_NEBULA_WORLD_TICKS.md` if implementing M2 immediately after MVP

### 2.3 For Living World Simulation

1. `DROP_DEAD_NEBULA_WORLD_TICKS.md`
2. `DROP_DEAD_NEBULA_NPCS.md`
3. `DROP_DEAD_NEBULA_ECONOMY.md`
4. `DROP_DEAD_NEBULA_RISK_EVENTS.md`
5. `DROP_DEAD_NEBULA_JOBS.md`

### 2.4 For Ambitious Mid/Late Game

1. `DROP_DEAD_NEBULA_MILESTONES.md`
2. `DROP_DEAD_NEBULA_FACTIONS.md`
3. `DROP_DEAD_NEBULA_SALVAGE.md`
4. `DROP_DEAD_NEBULA_SHIPS.md`
5. `DROP_DEAD_NEBULA_COMBAT.md`
6. `DROP_DEAD_NEBULA_DEPLOYABLES.md`
7. `DROP_DEAD_NEBULA_OUTPOSTS.md`
8. `DROP_DEAD_NEBULA_CORPORATIONS.md`
9. `DROP_DEAD_NEBULA_CAMPAIGN.md`

### 2.5 For Operations and Safety

1. `DROP_DEAD_NEBULA_SOCIAL_SAFETY.md`
2. `DROP_DEAD_NEBULA_ADMIN_OPS.md`
3. `DROP_DEAD_NEBULA_PROGRESSION_BALANCE.md`
4. `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`

---

## 3. Authoritative Docs by Topic

| Topic | Authoritative Doc(s) |
| --- | --- |
| Vision and player fantasy | `DROP_DEAD_NEBULA.md` |
| Canonical terminology | `DROP_DEAD_NEBULA_GLOSSARY.md` |
| System boundaries | `DROP_DEAD_NEBULA_SYSTEMS.md` |
| MVP scope | `DROP_DEAD_NEBULA_MVP.md` |
| MVP authored content | `DROP_DEAD_NEBULA_CONTENT_SEED.md` |
| Screen/UX principles | `DROP_DEAD_NEBULA_SCREEN_DESIGN.md` |
| Build order | `DROP_DEAD_NEBULA_MILESTONES.md` |
| Game-kit feature requests | `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md` |
| World simulation | `DROP_DEAD_NEBULA_WORLD_TICKS.md` |
| NPCs | `DROP_DEAD_NEBULA_NPCS.md` |
| Economy | `DROP_DEAD_NEBULA_ECONOMY.md` |
| Jobs/contracts/bounties/challenges | `DROP_DEAD_NEBULA_JOBS.md` |
| Risk/random events/customs | `DROP_DEAD_NEBULA_RISK_EVENTS.md` |
| Salvage/derelicts | `DROP_DEAD_NEBULA_SALVAGE.md` |
| Factions/shared goals | `DROP_DEAD_NEBULA_FACTIONS.md` |
| Campaign/seasons/Dead Gate | `DROP_DEAD_NEBULA_CAMPAIGN.md` |
| Corporations/charters | `DROP_DEAD_NEBULA_CORPORATIONS.md` |
| Outposts/stockpiles/colonies | `DROP_DEAD_NEBULA_OUTPOSTS.md` |
| Ships/modules/damage | `DROP_DEAD_NEBULA_SHIPS.md` |
| Combat/conflict | `DROP_DEAD_NEBULA_COMBAT.md` |
| Deployables/route control | `DROP_DEAD_NEBULA_DEPLOYABLES.md` |
| Player text/social safety | `DROP_DEAD_NEBULA_SOCIAL_SAFETY.md` |
| Admin/ops/recovery | `DROP_DEAD_NEBULA_ADMIN_OPS.md` |
| Progression/balance | `DROP_DEAD_NEBULA_PROGRESSION_BALANCE.md` |
| Pre-implementation gate | `DROP_DEAD_NEBULA_IMPLEMENTATION_READINESS.md` |
| Consolidated open questions | `DROP_DEAD_NEBULA_OPEN_QUESTIONS.md` |

---

## 4. Core Design Docs

### `DROP_DEAD_NEBULA.md`

The main Game Design Document. Use for:

- one-sentence pitch;
- design pillars;
- player fantasy;
- platform assumptions;
- long-term loops;
- world model;
- major systems overview;
- scope ladder;
- north star.

Read this first when trying to understand what the game is.

### `DROP_DEAD_NEBULA_GLOSSARY.md`

Canonical vocabulary. Use for:

- Place vs Sector;
- Route;
- Daily Turn vs World Tick;
- Notice vs Event;
- Contract vs Bounty vs Challenge;
- Cargo vs Inventory Slot;
- Heat;
- Recall;
- Presence.

If terminology conflicts, prefer this doc unless a later decision log explicitly changes it.

### `DROP_DEAD_NEBULA_SYSTEMS.md`

System inventory. Use for:

- system responsibilities;
- player-facing behavior;
- owned state at design level;
- game-kit dependency mapping;
- MVP shape;
- later ambition;
- open questions.

This is the best bridge from GDD to milestone planning.

---

## 5. MVP Docs

### `DROP_DEAD_NEBULA_MVP.md`

Strict vertical-slice definition.

MVP north star:

> Create/resume Captain → start at Ash Coil → accept First Mercy Run → buy Med Gel → travel to Mercy Relay → sell/deliver → complete Contract → see Event Log → quit → resume intact.

Use this to prevent scope creep during first implementation.

### `DROP_DEAD_NEBULA_CONTENT_SEED.md`

The tiny authored seed world for MVP:

- Ash Coil;
- Mercy Relay;
- Cinder Pocket;
- Blue Blind;
- Red Maw Approach;
- Saint Vex Drift;
- Dead Gate Verge;
- Med Gel, Reactor Coolant, Ore;
- Rustbucket Mule;
- First Mercy Run.

Use this for first content and smoke tests.

### `DROP_DEAD_NEBULA_SCREEN_DESIGN.md`

Screen and UX design. Use for:

- 80x24 UX rules;
- MVP screen map;
- dashboard/travel/market/jobs/cargo/log/help wireframes;
- future system screen sketches;
- visual language;
- screen acceptance checklist.

---

## 6. Roadmap and Planning Docs

### `DROP_DEAD_NEBULA_MILESTONES.md`

High-level recommended build order from M0 to M12. Use for:

- sequencing;
- seeded/playable/mature feature timing;
- milestone exit criteria;
- game-kit wishlist alignment by milestone.

### `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`

Reusable game-kit feature candidates. Use for:

- Contract primitive + unified Job Board;
- Spatial Travel helper;
- Inventory Capacity helper;
- Event Log/News screen;
- multi-user local-dev harness;
- Notice Inbox screen;
- random table helper;
- tick catch-up summary;
- market screen adapters;
- bounded text sanitizer.

This doc separates generic toolkit needs from Drop Dead Nebula-specific rules.

### `DROP_DEAD_NEBULA_OPEN_QUESTIONS.md`

Centralized unresolved questions. Use for:

- MVP blockers;
- pre-implementation decisions;
- milestone-timed questions;
- defer-later questions.

### `DROP_DEAD_NEBULA_IMPLEMENTATION_READINESS.md`

Gate checklist before writing an implementation plan. Use to verify:

- scope stability;
- content readiness;
- game-kit dependency decisions;
- remaining blockers;
- first implementation-plan inputs.

---

## 7. Deep-Dive Docs

### Living World Cluster

- `DROP_DEAD_NEBULA_WORLD_TICKS.md`
- `DROP_DEAD_NEBULA_NPCS.md`
- `DROP_DEAD_NEBULA_ECONOMY.md`
- `DROP_DEAD_NEBULA_JOBS.md`
- `DROP_DEAD_NEBULA_RISK_EVENTS.md`

Use these for M2–M5 and any simulation-heavy planning.

### Signature and Midgame Cluster

- `DROP_DEAD_NEBULA_SALVAGE.md`
- `DROP_DEAD_NEBULA_SHIPS.md`
- `DROP_DEAD_NEBULA_COMBAT.md`
- `DROP_DEAD_NEBULA_FACTIONS.md`
- `DROP_DEAD_NEBULA_DEPLOYABLES.md`

Use these for M4–M9.

### Strategic and Operations Cluster

- `DROP_DEAD_NEBULA_OUTPOSTS.md`
- `DROP_DEAD_NEBULA_CORPORATIONS.md`
- `DROP_DEAD_NEBULA_CAMPAIGN.md`
- `DROP_DEAD_NEBULA_SOCIAL_SAFETY.md`
- `DROP_DEAD_NEBULA_ADMIN_OPS.md`
- `DROP_DEAD_NEBULA_PROGRESSION_BALANCE.md`

Use these for M9–M12 and production readiness.

---

## 8. Implementation Planning Guidance

Before writing implementation plans:

1. Read `DROP_DEAD_NEBULA_IMPLEMENTATION_READINESS.md`.
2. Resolve MVP blockers in `DROP_DEAD_NEBULA_OPEN_QUESTIONS.md`.
3. Confirm whether Drop Dead Nebula lives inside this workspace or a separate repo.
4. Confirm whether P0 game-kit wishlist items are implemented first or developed locally.
5. Use `DROP_DEAD_NEBULA_MVP.md` as the implementation scope boundary.
6. Use `DROP_DEAD_NEBULA_CONTENT_SEED.md` as the first content source.
7. Use `DROP_DEAD_NEBULA_SCREEN_DESIGN.md` for the MVP UI acceptance target.
8. Do not pull in later deep-dive systems unless a milestone requires them.

---

## 9. Maintenance Rules for Docs

- If terminology changes, update the Glossary and any affected docs.
- If build order changes, update Milestones and Implementation Readiness.
- If MVP scope changes, update MVP, Content Seed, Screen Design, and Open Questions.
- If a deep-dive introduces a new canonical system, add it to this Index.
- If a question is answered, move or mark it in Open Questions rather than leaving stale uncertainty scattered across docs.
- Do not let implementation details leak backward into design docs unless intentionally creating an implementation plan.
