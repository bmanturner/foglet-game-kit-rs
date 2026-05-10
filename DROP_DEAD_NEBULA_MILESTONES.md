# Drop Dead Nebula — Milestones

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`, `DROP_DEAD_NEBULA_MVP.md`, `DROP_DEAD_NEBULA_CONTENT_SEED.md`, `DROP_DEAD_NEBULA_SCREEN_DESIGN.md`, `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`  
Purpose: Present the recommended high-level build order, starting with Milestone 0: MVP, while allowing systems to begin in one milestone and mature across later milestones.

---

## 1. Milestone Philosophy

Drop Dead Nebula is intentionally ambitious. The safest way to build it is not to finish one giant system at a time, but to grow several systems in layers.

A feature may appear in three forms across milestones:

1. **Seeded** — visible as a placeholder, content hook, or minimal data shape.
2. **Playable** — useful to the player in a narrow slice.
3. **Mature** — connected to other systems, balanced, persistent, and content-rich.

Example:

- Factions can be seeded as flavor in Milestone 1.
- Faction jobs can become playable in Milestone 4.
- Faction wars and shared regional consequences can mature in Milestone 8.

This document is not an implementation plan. It is a high-level build roadmap.

---

## 2. Recommended Milestone Sequence

| Milestone | Name | Primary Goal |
| --- | --- | --- |
| 0 | MVP: First Mercy Run | Prove the trade-route spine. |
| 1 | Foundation Hardening | Make MVP systems reliable, testable, and extensible. |
| 2 | Living Corridor | Add world ticks, simple market drift, notices, and NPC background activity. |
| 3 | First Danger | Add random events, route hazards, and early risk choices. |
| 4 | Salvage Signature | Add Blue Blind derelict boarding and salvage loop. |
| 5 | Jobs Expand | Add unified Job Board depth: bounties, more contracts, early challenges. |
| 6 | Ships and Heat | Add ship modules, repairs, fuel/damage, smuggling, customs, heat. |
| 7 | Factions and Shared Goals | Make faction alignment and shared projects playable. |
| 8 | NPC Rivalry and Async Social | Mature named NPCs, notices, async challenges, player mail. |
| 9 | Outposts and Route Control | Add stockpiles, deployables, caches, route control, early outposts. |
| 10 | Corporations and Economy Expansion | Add charters/corps, broader economy, player listings, deeper markets. |
| 11 | Campaign Layer: Dead Gate | Add season structure, faction fronts, endgame route unlocks. |
| 12 | Polish, Balance, and Operations | Harden UX, admin tools, balancing, documentation, packaging. |

---

## 3. Milestone 0 — MVP: First Mercy Run

### Goal

Prove the core shared-world trading loop:

> Create/resume Captain → start at Ash Coil → accept First Mercy Run → buy Med Gel → travel to Mercy Relay → sell/deliver → complete Contract → see Event Log → quit → resume intact.

### Built in This Milestone

- Captain identity and resume.
- Starter Ship: Rustbucket Mule.
- Tiny authored world from `DROP_DEAD_NEBULA_CONTENT_SEED.md`.
- 7 Places, with Ash Coil and Mercy Relay as active Stations.
- Directed Routes, especially Ash Coil -> Mercy Relay.
- Presence: Captain has one current Place.
- Daily Turns: travel spends turns.
- Basic Cargo and inventory capacity.
- Three Commodities: Med Gel, Reactor Coolant, Low-Grade Ore.
- Two simple Markets: Ash Coil and Mercy Relay.
- One Contract: First Mercy Run.
- Event Log for meaningful completion events.
- MVP screens from `DROP_DEAD_NEBULA_SCREEN_DESIGN.md`.
- Safe quit and resume.

### Systems Seeded but Not Mature

- Job Board exists, but only Contract is active.
- Place Recall exists, but only records visits.
- Market exists, but pricing is authored/simple.
- Event Log exists, but not yet a full news feed.
- Future Places exist as flavor hooks only.

### Explicitly Not Built

- NPC Captains.
- Random events.
- Dynamic pricing.
- Salvage interiors.
- Combat.
- Smuggling/Heat beyond possible placeholder display.
- Bounties.
- Factions as mechanics.
- Notices/mail.
- Outposts/corporations.

### Exit Criteria

- A new player can complete First Mercy Run without external docs.
- The player can quit and resume at Mercy Relay with correct state.
- The Log shows the successful delivery.
- The UI is readable at 80x24.
- No unbuilt system appears as broken functionality.

---

## 4. Milestone 1 — Foundation Hardening

### Goal

Turn the MVP from a proof into a stable foundation for expansion.

### Built in This Milestone

- Better first-run onboarding and Help.
- Cleaner dashboard Signals.
- More robust content key discipline.
- Clearer disabled-choice feedback.
- Basic Star Chart / Recall screen if not fully present in MVP.
- Stronger manual smoke checklist.
- Multi-user local-dev scenarios at a design/test level.
- Initial leaderboard seeds for simple scores, if low effort.

### Systems Advanced

- **Captain identity**: better local-dev multiple-user behavior.
- **Save/resume**: more confidence around short BBS sessions.
- **Content seed**: cleaned up after MVP playthrough.
- **Market UI**: improved quantity selection and detail panels.
- **Job Board**: ready to display multiple opportunity types later.

### Systems Seeded

- Leaderboards may start with Credits or Contracts Completed.
- Notices may be represented by static dashboard Signals, but not full inbox yet.
- Admin/diagnostic concepts may be noted, not built.

### Exit Criteria

- MVP loop feels smooth rather than merely functional.
- The seed content is stable enough to extend.
- The UI vocabulary matches the glossary.
- The next milestone can add simulation without restructuring the MVP screens.

---

## 5. Milestone 2 — Living Corridor

### Goal

Make the Ash Mercy Corridor move when the player is not actively clicking through the MVP loop.

### Built in This Milestone

- Basic World Tick flow.
- Market restock/reprice in simple bounded form.
- Daily turn reset or clearer daily turn lifecycle.
- First Daily Intel Packet screen or login summary.
- System Notices for tick summaries and Contract outcomes.
- Background NPC trade as abstract flow, not named NPCs yet.
- Event Log evolves toward world news.

### Systems Advanced

- **Markets**: no longer purely static.
- **Event Log**: shows system/world changes, not just player actions.
- **Notices**: system notices become real.
- **World Ticks**: catch-up policy begins, but remains small.
- **Contracts**: may expire or refresh in limited form.

### Systems Seeded

- Named NPCs may appear in Events as names, but do not yet have full memory.
- Factions may influence flavor lines, not mechanics.
- Low-population support begins through NPC/background actions.

### Exit Criteria

- Returning after time away produces a short, readable update.
- Markets can change without manual player action.
- Tick catch-up is bounded and does not overwhelm the session.
- The player starts to feel the corridor is alive.

---

## 6. Milestone 3 — First Danger

### Goal

Introduce risk without yet building full combat or salvage.

### Built in This Milestone

- Random travel events.
- Basic route hazard states.
- Scanning as a simple action or preview mechanic.
- First risk/reward travel choices.
- Early Heat placeholder becomes meaningful for inspections or warnings.
- Dangerous-route confirmation.

### Systems Advanced

- **Travel**: route choice now has risk and not just cost.
- **Place Recall**: may store last-known hazard or route note.
- **Dashboard Signals**: highlight hazards and opportunities.
- **Event Log**: records notable danger outcomes.
- **Help**: explains risk, hazards, and safe routes.

### Systems Seeded

- Combat appears as avoidance/toll/escape choices, not full combat rounds.
- Smuggling appears through inspection flavor, not full contraband economy.
- Red Maw Approach starts to matter.

### Exit Criteria

- Travel can surprise the player but remains fair.
- The player can understand and avoid danger.
- Risk creates interesting choices without frequent catastrophic loss.

---

## 7. Milestone 4 — Salvage Signature

### Goal

Add Drop Dead Nebula’s first distinctive loop beyond trading: derelict boarding at Blue Blind.

### Built in This Milestone

- Blue Blind becomes mechanically active.
- One small derelict local ASCII map.
- Room navigation.
- One or two salvage hazards.
- Loot containers using owner-keyed inventory.
- Cargo-capacity tradeoffs inside salvage.
- First black-box or salvage recovery objective.
- Salvage result modal and Event Log entries.

### Systems Advanced

- **Maps**: local derelict maps become playable.
- **Cargo**: capacity matters in a more interesting way.
- **Random events**: salvage-specific table begins.
- **Contracts**: can reference salvage recovery.
- **Bounties**: may be seeded but not fully broad yet.

### Systems Seeded

- Derelict decay and shared stripping can be minimal or deferred.
- NPC rival Moth-9 can be mentioned but not fully simulated.
- Relics can appear as flavor, not full mechanics.

### Exit Criteria

- Player can discover/enter/exit a derelict safely.
- Salvage produces meaningful loot or Contract progress.
- Cargo constraints create at least one interesting choice.
- The game now has a signature identity beyond hauling Med Gel.

---

## 8. Milestone 5 — Jobs Expand

### Goal

Turn the Job Board into a real opportunity hub.

### Built in This Milestone

- Multiple Contract types:
  - delivery;
  - supply;
  - survey;
  - salvage recovery.
- Bounties appear on the same Job Board.
- First Bounty lifecycle: open -> claimed -> completed/expired.
- Job expiry and refresh.
- Job details and filtering.
- More Contract destinations across the seed corridor.

### Systems Advanced

- **Job Board**: becomes a unified surface for Contracts and Bounties.
- **Contracts**: broaden beyond First Mercy Run.
- **Bounties**: become playable, probably around Blue Blind and route hazards.
- **Notices**: can report job completion/expiry.
- **Leaderboards**: Contracts Completed and Bounties Completed become meaningful.

### Systems Seeded

- Challenges may appear as NPC-issued challenge offers, but can remain simple.
- Faction jobs may appear as locked or flavor-labeled opportunities.

### Exit Criteria

- The player can choose from multiple kinds of work.
- Bounties feel distinct from Contracts.
- The Job Board remains readable and not overwhelming.
- Work opportunities support both short and medium sessions.

---

## 9. Milestone 6 — Ships and Heat

### Goal

Make the Ship a real build platform and make illegal/risky play legible.

### Built in This Milestone

- Ship status screen matures.
- Hull damage and repair.
- Fuel if still desired after earlier testing.
- First modules:
  - cargo rack;
  - scanner;
  - false hold;
  - salvage tool;
  - shield patch or escape system.
- Shipyard or repair service at Ash Coil.
- Contraband flag on selected goods.
- Customs/inspection events.
- Heat details screen.

### Systems Advanced

- **Ship**: no longer just cargo capacity.
- **Market**: can sell modules and restricted goods.
- **Random events**: customs and hazards use ship capabilities.
- **Smuggling**: becomes a viable early career.
- **Travel**: modules affect route choices.

### Systems Seeded

- Combat still can be mostly encounter-choice based.
- Insurance/recovery can be simple.
- Faction legal differences can be hinted but not complete.

### Exit Criteria

- Players can make at least two meaningfully different ship builds.
- Heat is visible and understandable.
- Smuggling has risk, reward, and counterplay.
- Damage is scary but not campaign-ending.

---

## 10. Milestone 7 — Factions and Shared Goals

### Goal

Let players and NPC background activity reshape the corridor through faction projects.

### Built in This Milestone

- Faction overview screen.
- Initial factions become mechanically meaningful:
  - Mourning Union;
  - Helix Cartel;
  - Port Authority Black Office;
  - Saints of Vacuum, if ready.
- Faction standing.
- Faction-tagged Contracts.
- First shared goal: Mercy Relay repair or corridor stabilization.
- Contribution flow using inventory/credits/actions.
- Faction outcome affects at least one route, market, or service.

### Systems Advanced

- **Contracts/Bounties**: faction issuers and rewards.
- **Markets**: faction modifiers begin.
- **World Ticks**: faction progress advances or decays.
- **Event Log/Notices**: faction progress reported.
- **Place/Route state**: shared goals can change them.

### Systems Seeded

- Faction conflict and fronts remain simple.
- Deep faction war waits for later.
- Multi-faction membership rules can be conservative.

### Exit Criteria

- Faction standing changes player options.
- Shared goal completion visibly changes the world.
- Solo players can contribute meaningfully.
- Faction UI explains consequences clearly.

---

## 11. Milestone 8 — NPC Rivalry and Async Social

### Goal

Make the galaxy feel populated by both NPCs and asynchronous players.

### Built in This Milestone

- Named NPC Captains with lightweight state.
- NPC memory for a small set of interactions.
- NPC Notices.
- Inbox matures beyond system messages.
- Async Challenges become playable, first with NPCs, then players if safe.
- Player-to-player mail may begin if bounded text and moderation rules are ready.
- Rival actions appear in Event Log and Daily Intel.

### Systems Advanced

- **NPC simulation**: named actors, not just background flows.
- **Notices**: become a major consequence surface.
- **Challenges**: create rivalry without simultaneous play.
- **Leaderboards**: support social comparison.
- **Job Board**: can show challenge opportunities.

### Systems Seeded

- Player-posted jobs may be deferred.
- Full PvP combat policies may still wait.
- Corp/social groups can be teased but not required.

### Exit Criteria

- A low-pop player receives meaningful NPC-driven hooks.
- At least one NPC can become a recognizable rival or ally.
- Async Challenges resolve clearly through future actions or ticks.
- Inbox is useful, not spammy.

---

## 12. Milestone 9 — Outposts and Route Control

### Goal

Let players leave durable infrastructure in the world.

### Built in This Milestone

- Hidden caches or simple depots.
- First claimable/upgradeable Outpost.
- Owner-keyed stockpiles outside the Ship.
- Basic access policy: private/public/faction/corp later.
- Deployables begin:
  - sensor ghost;
  - decoy beacon;
  - simple mine or interdiction buoy if safe.
- Route control screen.
- Sweep/scan counterplay.

### Systems Advanced

- **Inventory**: owners beyond ship/station matter.
- **Outposts**: begin as storage/project sites, not full colonies.
- **Traps/deployables**: introduce async area effects.
- **Contracts/Bounties**: can target route clearing and depot supply.
- **World Ticks**: can decay deployables or process outpost changes.

### Systems Seeded

- Colonies and production chains remain limited.
- Corporations can use the same structures later.
- Route warfare is constrained to avoid griefing.

### Exit Criteria

- A player can store goods outside the Ship.
- A player can create or improve at least one durable world asset.
- Deployables have visible counterplay and safe-zone rules.
- Outposts deepen the economy without becoming a spreadsheet.

---

## 13. Milestone 10 — Corporations and Economy Expansion

### Goal

Support shared enterprise and a broader economy without losing solo viability.

### Built in This Milestone

- Solo Charters.
- Player Corporations if population warrants.
- Shared bank/depot basics.
- Corp/Charter task board.
- Player or corp market listings.
- More commodities and port archetypes.
- Production/consumption loops for outposts.
- More leaderboards.

### Systems Advanced

- **Outposts**: support shared ownership or charter ownership.
- **Markets**: player listings and broader commodity roles.
- **Contracts**: corp/charter tasks and supply chains.
- **Factions**: charters/corps can align or conflict.
- **Notices**: corp notices and reports.

### Systems Seeded

- Full colony simulation can remain partial.
- Corp warfare should wait until route control and safety rules are proven.

### Exit Criteria

- Solo players can use Charter mechanics without needing a group.
- Multiple players can share resources if enabled.
- The economy supports more than one profitable playstyle.
- Player-created listings or depots do not break low-pop balance.

---

## 14. Milestone 11 — Campaign Layer: Dead Gate

### Goal

Turn the accumulated systems into a season-scale arc.

### Built in This Milestone

- Season phase structure.
- Faction fronts.
- Dead Gate shared goal chain.
- Endgame routes and Places.
- Relic/anomaly content matures.
- World events that change multiple Regions.
- Season summaries and possible reset/legacy rules.

### Systems Advanced

- **Factions**: move from local projects to regional fronts.
- **Routes**: unlock, collapse, stabilize, or mutate.
- **Salvage/Relics**: connect to campaign progression.
- **NPCs**: participate in campaign events.
- **Leaderboards/Event Log**: become season chronicle.
- **Outposts/Corps**: affect endgame logistics.

### Systems Seeded

- Post-season legacy can be simple first.
- Full procedural world generation may still be optional.

### Exit Criteria

- The Dead Gate is no longer just flavor.
- Player and faction actions can alter the campaign outcome.
- The season has a readable arc and conclusion.
- The endgame uses prior systems instead of introducing an unrelated minigame.

---

## 15. Milestone 12 — Polish, Balance, and Operations

### Goal

Make the game stable, legible, maintainable, and pleasant to operate on a real BBS.

### Built in This Milestone

- Balance passes for turns, prices, rewards, damage, heat, and progression.
- Admin/diagnostic world health screen or CLI flow.
- Stuck player recovery policy.
- Market/contract repair tools.
- Better onboarding.
- Better help/glossary in-game.
- Accessibility/monochrome review.
- Packaging and install docs.
- Backup/restore guidance.
- Season management guidance.

### Systems Advanced

- **All systems** receive polish.
- **Admin/diagnostics** become real.
- **Documentation** catches up to implementation.
- **Testing/simulation harnesses** mature.

### Exit Criteria

- A sysop can install, run, inspect, and recover the game.
- A new player can understand the first session without external coaching.
- A returning player can understand what changed while away.
- Core loops remain fun under low population.
- The game is ready for a broader alpha/beta.

---

## 16. Feature Maturity Matrix

| Feature / System | Seeded | Playable | Mature |
| --- | --- | --- | --- |
| Captain identity | M0 | M0 | M1 |
| Save/resume | M0 | M0 | M1 |
| Places/Routes | M0 | M0 | M3+ |
| Presence | M0 | M0 | M1 |
| Place Recall | M0 | M1 | M3/M7 |
| Daily Turns | M0 | M0 | M2/M6 |
| Cargo/Inventory | M0 | M0 | M9/M10 |
| Markets | M0 | M0 | M2/M10 |
| Contracts | M0 | M0 | M5 |
| Event Log | M0 | M0 | M2/M11 |
| Notices | M1 flavor | M2 | M8 |
| World Ticks | M1 design | M2 | M7/M11 |
| Random Events | M2 seed | M3 | M6+ |
| Route Hazards | M2 seed | M3 | M9 |
| Salvage | M0 flavor | M4 | M8/M11 |
| Bounties | M4 seed | M5 | M8 |
| Ship Modules | M4 seed | M6 | M10 |
| Heat/Customs | M3 seed | M6 | M8 |
| Factions | M0 flavor | M7 | M11 |
| Shared Goals | M4 seed | M7 | M11 |
| NPC Captains | M2 names | M8 | M11 |
| Async Challenges | M5 seed | M8 | M10 |
| Leaderboards | M1 seed | M5/M8 | M11 |
| Deployables/Traps | M6 seed | M9 | M10 |
| Outposts | M7 seed | M9 | M10 |
| Corporations/Charters | M8 seed | M10 | M11 |
| Dead Gate campaign | M0 flavor | M11 | M12 |
| Admin/Diagnostics | M1 notes | M12 | M12+ |

---

## 17. Game-Kit Wishlist Alignment by Milestone

### Milestone 0–1

Most useful kit support:

- Contract primitive + unified Job Board surface.
- Spatial Travel transaction helper.
- Owner Inventory capacity helper.
- Event Log / News screen.
- Multi-user local-dev test harness.

### Milestone 2–3

Most useful kit support:

- Notice Inbox screen.
- World Tick catch-up summary helper.
- Deterministic Random Table helper.
- Reusable Market screen and transaction adapters.

### Milestone 4–6

Most useful kit support:

- Scripted terminal session test harness.
- Bounded player text sanitizer.
- Leaderboard screen.
- Content key validation helper.

### Milestone 7+

Most useful kit support:

- Relationship/reputation primitive if proven necessary.
- Admin/diagnostic world inspection helpers.
- Data-driven content loader conventions.

---

## 18. Recommended Build Discipline

### 18.1 Keep Every Milestone Playable

Each milestone should end with a coherent game, even if small. Do not leave the player in a half-connected state where old loops are broken while new systems are incomplete.

### 18.2 Prefer Thin Vertical Additions

When adding a system, build enough UI, state, and content to make it real.

Bad:

- add faction database records but no player-facing effect.

Good:

- add one faction standing value, one faction job, one contribution, and one visible consequence.

### 18.3 Let Systems Mature When They Have Customers

Do not fully build a generic deployable system before travel hazards, scanning, notices, and route control can use it.

Do not fully build corporations before stockpiles, outposts, notices, and permissions have a reason to exist.

### 18.4 Protect the MVP Loop

The First Mercy Run loop remains the regression path. Every milestone should preserve:

- create/resume;
- travel;
- buy/sell;
- complete job;
- log event;
- quit/resume.

If that breaks, the milestone is not healthy.

---

## 19. Open Roadmap Questions

1. Should Milestone 2 introduce Notices before or after World Ticks?
2. Should random events arrive before market drift, or vice versa?
3. Should Salvage precede Bounties, as recommended here, or should Bounties arrive first to create more job structure?
4. Should Heat/Customs come before combat to support noncombat danger?
5. Should Factions arrive before named NPCs, or should NPCs introduce factions through personality first?
6. Should Corporations wait until there is evidence of multiple active players?
7. Should solo Charters arrive earlier to support outposts?
8. Should the Dead Gate campaign require a season reset model, or can it run indefinitely?
9. Which milestone should introduce procedural world generation, if ever?
10. Which game-kit wishlist items should be built before the game milestone that wants them?

---

## 20. Current Recommendation

Build order should remain:

1. **Trade spine** — Milestones 0–1.
2. **World movement** — Milestones 2–3.
3. **Signature play** — Milestone 4.
4. **Opportunity depth** — Milestone 5.
5. **Risk/build identity** — Milestone 6.
6. **World allegiance** — Milestone 7.
7. **Async social life** — Milestone 8.
8. **Durable player footprint** — Milestones 9–10.
9. **Season/campaign arc** — Milestone 11.
10. **Operational polish** — Milestone 12.

This sequence keeps the project ambitious without asking any milestone to carry too much weight alone.
