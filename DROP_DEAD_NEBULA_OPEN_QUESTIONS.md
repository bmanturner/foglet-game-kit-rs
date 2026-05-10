# Drop Dead Nebula — Open Questions

Status: Draft v0.1  
Purpose: Centralize unresolved questions from the Drop Dead Nebula documentation set so implementation planning can distinguish blockers from deferred design choices.

---

## 1. How to Use This Document

Questions are grouped by urgency:

- **MVP blockers** must be answered before the first implementation plan.
- **Pre-implementation shaping questions** should ideally be answered before major architecture decisions.
- **Milestone questions** can wait until the relevant milestone approaches.
- **Deferred questions** should not block near-term work.

When a question is answered, keep the answer here or move it into a future `DROP_DEAD_NEBULA_DECISIONS.md` if we create one.

---

## 2. MVP Blockers

These should be answered before implementation planning begins.

### Q-MVP-001 — Repository location

Question: Should Drop Dead Nebula initially live inside the `foglet-game-kit-rs` workspace as an example, or in a separate game repo consuming the crate?

Why it matters:

- affects project layout;
- affects packaging;
- affects dependency pathing;
- affects whether game-kit changes and game code evolve together.

Current leaning:

- undecided.

### Q-MVP-002 — Game-kit P0 first or local game code first?

Question: Should P0 game-kit wishlist items be implemented before MVP, or should Drop Dead Nebula implement local versions and promote later?

Relevant P0 wishlist items:

- Contract primitive + unified Job Board surface;
- Spatial Travel transaction helper;
- Owner Inventory capacity helper;
- Event Log / News screen;
- multi-user local-dev test harness.

Current leaning:

- implement only what is necessary for MVP, but strongly consider generic Contract/Job Board and capacity/travel helpers early.

### Q-MVP-003 — First Mercy Run completion semantics

Question: Should First Mercy Run consume Med Gel directly, or should it accept proof that the player sold Med Gel at Mercy Relay?

Why it matters:

- affects player clarity;
- affects Market vs Contract relationship;
- affects inventory transfer expectations.

Current leaning:

- direct delivery/consumption is clearer for MVP, but selling first is a better trade tutorial. Needs decision.

### Q-MVP-004 — Trading turn cost

Question: Does trading cost Daily Turns in MVP?

Current leaning:

- no; travel costs turns, trading does not.

Status:

- effectively decided unless challenged.

### Q-MVP-005 — Credits ownership model

Question: Are credits Captain state, ledger-like resource, or inventory-like resource?

Why it matters:

- affects Markets;
- affects rewards;
- affects corp banks later.

Current leaning:

- Captain state for MVP; revisit ledger/bank semantics for Corps.

### Q-MVP-006 — Daily Turn reset in MVP

Question: Does MVP need real daily reset, or just persistent starting/spent turn state?

Current leaning:

- persistent turn state is enough for M0; full reset becomes M2.

### Q-MVP-007 — MVP Notice usage

Question: Should MVP include any Notice, or rely on Event Log and Dashboard Signals?

Current leaning:

- defer Notices to M2; MVP uses Event Log and Signals.

---

## 3. Pre-Implementation Shaping Questions

Important soon, but not all block MVP.

### Q-PI-001 — Data ownership boundaries

Question: Which game data belongs in game-specific tables versus generic game-kit primitives?

Known mapping:

- Places/Routes/Presence/Recall -> game-kit spatial primitives.
- Inventory slots -> game-kit inventory primitive.
- Events/Turns/Players -> game-kit primitives.
- Contracts may be game-specific unless promoted to kit.
- Ship/hull/module rules are game-specific.

### Q-PI-002 — Content format

Question: Should seed content begin as Rust constants, TOML, YAML, RON, or another structured format?

Current leaning:

- no firm decision; avoid overbuilding data loaders until MVP shape is proven.

### Q-PI-003 — Content validation

Question: How soon do stable content key validation and reference checks become necessary?

Current leaning:

- not required for M0 if content is tiny, but valuable soon after.

### Q-PI-004 — Test strategy

Question: Should we create a dedicated test strategy doc before implementation?

Current leaning:

- useful, but can be part of first implementation plan unless the project feels risky.

### Q-PI-005 — Multi-user local-dev harness

Question: Do we need multi-user testing for M0, or can it wait until M1/M2?

Current leaning:

- M0 should at least prove two local users can remain distinct if feasible.

---

## 4. Milestone 1–2 Questions

### Q-M1-001 — Star Chart timing

Question: Should Star Chart/Recall be present in M0 or wait until M1?

Current leaning:

- M0 can record Recall; M1 can improve Star Chart UI.

### Q-M2-001 — Notices before or after World Ticks?

Question: Should M2 introduce Notices before World Ticks, or ticks before Notices?

Current leaning:

- ticks first, then use Notices/Daily Intel to summarize tick output.

### Q-M2-002 — Daily Intel storage

Question: Should Daily Intel be stored as Notices, generated from Events, or both?

Current leaning:

- digest generated from tick results and notable Events; targeted items become Notices.

### Q-M2-003 — Background NPC determinism

Question: Should background NPC flow be deterministic by world seed/day?

Current leaning:

- deterministic enough for tests; not necessarily player-visible.

---

## 5. Milestone 3–6 Questions

### Q-M3-001 — Random event frequency

Question: How often should travel events fire?

Current leaning:

- enough to matter, not every route. Safe routes should often be quiet.

### Q-M3-002 — Visible odds

Question: Should event/combat odds be visible?

Current leaning:

- show qualitative risk first: low/moderate/high.

### Q-M4-001 — Derelict state sharing

Question: Are derelicts globally shared, per-player instanced, or hybrid?

Current leaning:

- hybrid: important loot/objectives shared; minor salvage may be per-player or refreshable.

### Q-M4-002 — Quit mid-derelict

Question: Can a player quit while inside a derelict?

Current leaning:

- eventually yes with recovery rules, but first slice may require return-to-ship before safe quit.

### Q-M6-001 — Fuel

Question: Should fuel be part of core play?

Current leaning:

- no fuel in M0; reconsider in M6 only if it adds interesting planning without stranding players.

### Q-M6-002 — Heat scope

Question: Is Heat global, regional, Faction-specific, or layered?

Current leaning:

- start simple, probably global plus local/Faction flavor later.

---

## 6. Milestone 7–8 Questions

### Q-M7-001 — Multi-faction membership

Question: Can players join multiple Factions?

Current leaning:

- allow broad standing with all Factions; formal allegiance may be limited later.

### Q-M7-002 — Faction reversibility

Question: How reversible are faction choices?

Current leaning:

- early choices should be reversible or recoverable; hard allegiance waits.

### Q-M8-001 — Player mail default

Question: Should player-to-player mail be enabled by default?

Current leaning:

- only after bounded text sanitizer and sysop controls exist.

### Q-M8-002 — Challenges acceptance

Question: Should Challenges require explicit acceptance?

Current leaning:

- yes for player-targeted Challenges; open public contests can be opt-in by participation.

---

## 7. Milestone 9–10 Questions

### Q-M9-001 — Outpost location model

Question: Are Outposts built on existing Places or do they create new Places?

Current leaning:

- first Outposts should attach to existing Places; creating new Places can wait.

### Q-M9-002 — Deployable attachment

Question: Are deployables attached to Places, Routes, or both?

Current leaning:

- both eventually; start with one attachment type to keep UI simple.

### Q-M9-003 — Deployable anti-griefing

Question: What safe-zone and density rules prevent griefing?

Current leaning:

- safe routes restrict harmful deployables; all deployables decay; scanning/sweeping must be viable.

### Q-M10-001 — Charter to Corporation migration

Question: Can a Charter become a Corporation without migration pain?

Current leaning:

- yes; design Charter as single-member Corporation-like shell.

### Q-M10-002 — Player listings before corporations

Question: Should player listings be allowed before Corporations?

Current leaning:

- likely no; wait until social safety and economy are ready.

---

## 8. Milestone 11–12 Questions

### Q-M11-001 — Season reset model

Question: Do seasons reset, partially reset, or run indefinitely?

Current leaning:

- undecided; campaign can be designed before reset policy is final.

### Q-M11-002 — Captain legacy

Question: Does Captain legacy persist across seasons?

Current leaning:

- likely yes in some lightweight form if seasons reset.

### Q-M12-001 — Admin surface

Question: Should admin tools be CLI-first or in-door-first?

Current leaning:

- CLI-first for destructive/maintenance actions; in-door for diagnostics and safe rescue.

### Q-M12-002 — Backup model

Question: How should backups handle shared world DB plus per-user saves?

Current leaning:

- needs explicit operations doc/planning before production.

---

## 9. Questions That Should Not Block MVP

- full world generation vs authored/generator hybrid;
- all six Factions and their final mechanics;
- complete ship hull catalog;
- PvP policy;
- corp permissions;
- outpost production chains;
- season reset policy;
- player text moderation beyond avoiding free text in MVP;
- procedural derelict generation;
- exact long-term balance curves.

---

## 10. Recommended Next Decisions

Before implementation planning, answer only these:

1. Repository location.
2. Game-kit P0 strategy: build generic helpers first or local MVP logic first.
3. First Mercy Run completion semantics.
4. Credits ownership for MVP.
5. Whether M0 includes real Daily Turn reset or defers to M2.

Everything else can remain open.
