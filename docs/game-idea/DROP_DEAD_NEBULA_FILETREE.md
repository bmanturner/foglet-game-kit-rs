# Drop Dead Nebula — Aspirational Final File Tree

Status: Draft v0.1  
Purpose: Speculate about the final completed project layout so early architecture choices can grow toward a clean, idiomatic Rust structure without prematurely implementing everything.

---

## 1. Intent

This document is deliberately aspirational. It describes where the project could end up after the MVP, living-world systems, salvage, economy, factions, outposts, corporations, campaign, admin tooling, and production packaging all exist.

It is **not** a demand that Milestone 0 create this whole tree.

Use this as:

- a north star for module boundaries;
- a way to avoid dumping all game code into `main.rs`;
- a vocabulary bridge between design docs and future implementation plans;
- a guardrail against coupling UI, persistence, content, and simulation too tightly.

---

## 2. High-Level Architectural Assumptions

This tree assumes Drop Dead Nebula becomes its own Rust game crate consuming `foglet_game` from `foglet-game-kit-rs`.

If the game starts inside the `foglet-game-kit-rs` workspace as an example, the internal layout can remain similar under:

```text
examples/drop_dead_nebula/
```

If it becomes a standalone repository, the same structure can live at repo root.

The recommended long-term shape is:

- one primary game library crate for domain/UI/simulation code;
- one binary entrypoint for the Foglet door;
- optional small companion binaries for content validation, world inspection, or admin operations;
- content under `assets/`;
- design docs under `docs/design/` eventually, even if currently at repo root.

---

## 3. Top-Level Final Tree

```text
drop-dead-nebula/
  Cargo.toml
  Cargo.lock
  README.md
  LICENSE
  CHANGELOG.md
  rust-toolchain.toml
  .gitignore
  .cargo/
    config.toml

  src/
    lib.rs
    main.rs
    prelude.rs
    app.rs
    boot.rs
    error.rs

    config/
      mod.rs
      game_config.rs
      balance.rs
      paths.rs
      feature_flags.rs

    foglet_integration/
      mod.rs
      context.rs
      manifest.rs
      package.rs
      local_dev.rs

    content/
      mod.rs
      keys.rs
      registry.rs
      validation.rs
      loading.rs
      seed.rs
      text.rs

      definitions/
        mod.rs
        places.rs
        routes.rs
        commodities.rs
        ships.rs
        modules.rs
        contracts.rs
        bounties.rs
        factions.rs
        npcs.rs
        events.rs
        encounters.rs
        salvage.rs
        outposts.rs
        campaign.rs

    state/
      mod.rs
      save.rs
      player.rs
      session.rs
      world.rs
      migrations.rs
      repositories/
        mod.rs
        players.rs
        captains.rs
        ships.rs
        inventory.rs
        markets.rs
        contracts.rs
        jobs.rs
        notices.rs
        events.rs
        ticks.rs
        npcs.rs
        factions.rs
        salvage.rs
        outposts.rs
        corporations.rs
        campaign.rs

    domain/
      mod.rs
      ids.rs
      time.rs
      money.rs
      quantities.rs
      outcome.rs
      requirements.rs
      rewards.rs

      captain/
        mod.rs
        model.rs
        creation.rs
        reputation.rs
        heat.rs
        stats.rs

      ship/
        mod.rs
        model.rs
        hull.rs
        module.rs
        loadout.rs
        damage.rs
        repair.rs
        recovery.rs
        capacity.rs

      world/
        mod.rs
        place.rs
        route.rs
        region.rs
        presence.rs
        recall.rs
        travel.rs
        hazards.rs
        scanning.rs

      economy/
        mod.rs
        commodity.rs
        market.rs
        pricing.rs
        stock.rs
        trade.rs
        listings.rs
        scarcity.rs
        production.rs

      inventory/
        mod.rs
        item.rs
        owner.rs
        transfer.rs
        capacity.rs
        stockpile.rs
        escrow.rs

      jobs/
        mod.rs
        contract.rs
        bounty.rs
        challenge.rs
        board.rs
        objective.rs
        proof.rs
        reward.rs
        generation.rs
        expiry.rs

      notices/
        mod.rs
        notice.rs
        inbox.rs
        daily_intel.rs
        digest.rs
        templates.rs

      events/
        mod.rs
        event.rs
        log.rs
        news.rs
        visibility.rs
        render.rs

      npcs/
        mod.rs
        actor.rs
        archetype.rs
        memory.rs
        goals.rs
        behavior.rs
        generation.rs
        dialogue_hooks.rs

      factions/
        mod.rs
        faction.rs
        standing.rs
        influence.rs
        shared_goal.rs
        contribution.rs
        fronts.rs
        rewards.rs

      salvage/
        mod.rs
        site.rs
        derelict.rs
        deck_map.rs
        room.rs
        hazard.rs
        loot.rs
        black_box.rs
        decay.rs

      encounters/
        mod.rs
        random_table.rs
        travel_event.rs
        dockside_event.rs
        salvage_event.rs
        customs.rs
        pirate.rs
        anomaly.rs
        resolution.rs

      combat/
        mod.rs
        encounter.rs
        stance.rs
        targeting.rs
        resolution.rs
        damage.rs
        piracy.rs
        policies.rs

      deployables/
        mod.rs
        deployable.rs
        mine.rs
        sensor.rs
        decoy.rs
        interdiction.rs
        drone.rs
        trigger.rs
        sweeping.rs

      outposts/
        mod.rs
        outpost.rs
        colony.rs
        project.rs
        access.rs
        production.rs
        defense.rs
        reports.rs

      corporations/
        mod.rs
        charter.rs
        corporation.rs
        member.rs
        role.rs
        permissions.rs
        bank.rs
        depot.rs
        task.rs
        notices.rs

      campaign/
        mod.rs
        season.rs
        phase.rs
        dead_gate.rs
        chronicle.rs
        outcome.rs
        legacy.rs

      progression/
        mod.rs
        careers.rs
        achievements.rs
        leaderboards.rs
        balance.rs
        pacing.rs

      social/
        mod.rs
        text.rs
        sanitize.rs
        moderation.rs
        reports.rs
        blocks.rs

    services/
      mod.rs
      game_service.rs
      transaction.rs
      rng.rs
      clock.rs
      telemetry.rs

      captain_service.rs
      travel_service.rs
      market_service.rs
      job_service.rs
      notice_service.rs
      event_service.rs
      tick_service.rs
      npc_service.rs
      faction_service.rs
      salvage_service.rs
      ship_service.rs
      encounter_service.rs
      outpost_service.rs
      corporation_service.rs
      campaign_service.rs
      admin_service.rs

    simulation/
      mod.rs
      ticks/
        mod.rs
        scheduler.rs
        catchup.rs
        daily_turns.rs
        market_drift.rs
        job_refresh.rs
        npc_flow.rs
        named_npcs.rs
        route_hazards.rs
        derelict_decay.rs
        faction_progress.rs
        outpost_production.rs
        deployable_decay.rs
        campaign_phase.rs
        digest.rs

      npc_ai/
        mod.rs
        planner.rs
        goal_selection.rs
        action_resolution.rs
        memory_updates.rs
        low_pop_scaling.rs

      economy/
        mod.rs
        restock.rs
        pricing.rs
        consumption.rs
        production.rs
        shortages.rs
        anti_farming.rs

      generation/
        mod.rs
        world_gen.rs
        contract_gen.rs
        bounty_gen.rs
        rumor_gen.rs
        salvage_site_gen.rs
        npc_gen.rs
        event_table_gen.rs

    ui/
      mod.rs
      app.rs
      theme.rs
      layout.rs
      widgets.rs
      nav.rs
      input.rs
      feedback.rs
      symbols.rs

      screens/
        mod.rs
        title.rs
        dashboard.rs
        travel.rs
        market.rs
        jobs.rs
        cargo.rs
        log.rs
        help.rs
        star_chart.rs
        inbox.rs
        daily_intel.rs
        ship.rs
        salvage.rs
        combat.rs
        factions.rs
        leaderboards.rs
        outposts.rs
        corporations.rs
        admin.rs
        settings.rs

      modals/
        mod.rs
        confirm.rs
        result.rs
        details.rs
        quantity.rs
        error.rs
        help.rs

      components/
        mod.rs
        header.rs
        footer.rs
        status_bar.rs
        signal_list.rs
        key_hints.rs
        table.rs
        tabs.rs
        progress_bar.rs
        cargo_summary.rs
        job_card.rs
        market_row.rs
        route_row.rs
        notice_row.rs
        event_row.rs
        requirements.rs

    admin/
      mod.rs
      health.rs
      diagnostics.rs
      rescue.rs
      repair.rs
      backup.rs
      export.rs
      audit.rs

    cli/
      mod.rs
      args.rs
      commands.rs
      run.rs
      local_dev.rs
      tick.rs
      validate_content.rs
      inspect_world.rs
      export_summary.rs

  assets/
    game.toml
    balance.toml
    worlds/
      ash_mercy_corridor.toml
      default_nebula.toml
    content/
      places.toml
      routes.toml
      commodities.toml
      ships.toml
      modules.toml
      markets.toml
      contracts.toml
      bounties.toml
      factions.toml
      npcs.toml
      encounters.toml
      salvage.toml
      outposts.toml
      campaign.toml
      help.toml
    dialog/
      ash_coil_clerk.yaml
      mercy_relay_operator.yaml
      brass_jory.yaml
      moth_9.yaml
      marshal_senn.yaml
      sister_static.yaml
      kara_vex.yaml
    maps/
      derelicts/
        choirless_bell.txt
        rust_chapel.txt
        blackglass_freighter.txt
      stations/
        ash_coil.txt
        mercy_relay.txt
    text/
      intro.md
      help.md
      first_run.md
      daily_intel_templates.md
      event_templates.md
      notice_templates.md

  migrations/
    0001_core_world.sql
    0002_economy.sql
    0003_jobs.sql
    0004_notices_events.sql
    0005_ticks.sql
    0006_npcs.sql
    0007_salvage.sql
    0008_ships_heat.sql
    0009_factions.sql
    0010_deployables_outposts.sql
    0011_corporations.sql
    0012_campaign.sql
    0013_admin_audit.sql

  tests/
    common/
      mod.rs
      fixtures.rs
      fake_contexts.rs
      scripted_session.rs
      assertions.rs
      temp_world.rs

    mvp_flow.rs
    travel.rs
    market.rs
    jobs.rs
    cargo.rs
    save_resume.rs
    world_ticks.rs
    npc_flow.rs
    economy.rs
    risk_events.rs
    salvage.rs
    factions.rs
    ships.rs
    combat.rs
    deployables.rs
    outposts.rs
    corporations.rs
    campaign.rs
    admin_ops.rs

  benches/
    tick_catchup.rs
    market_drift.rs
    world_generation.rs

  examples/
    local_dev_run.rs
    run_tick_once.rs
    inspect_seed_world.rs

  docs/
    design/
      DROP_DEAD_NEBULA.md
      DROP_DEAD_NEBULA_DOC_INDEX.md
      DROP_DEAD_NEBULA_GLOSSARY.md
      DROP_DEAD_NEBULA_SYSTEMS.md
      DROP_DEAD_NEBULA_MVP.md
      DROP_DEAD_NEBULA_CONTENT_SEED.md
      DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md
      DROP_DEAD_NEBULA_SCREEN_DESIGN.md
      DROP_DEAD_NEBULA_MILESTONES.md
      DROP_DEAD_NEBULA_OPEN_QUESTIONS.md
      DROP_DEAD_NEBULA_IMPLEMENTATION_READINESS.md
      deep_dives/
        DROP_DEAD_NEBULA_WORLD_TICKS.md
        DROP_DEAD_NEBULA_NPCS.md
        DROP_DEAD_NEBULA_ECONOMY.md
        DROP_DEAD_NEBULA_JOBS.md
        DROP_DEAD_NEBULA_RISK_EVENTS.md
        DROP_DEAD_NEBULA_SALVAGE.md
        DROP_DEAD_NEBULA_FACTIONS.md
        DROP_DEAD_NEBULA_CAMPAIGN.md
        DROP_DEAD_NEBULA_CORPORATIONS.md
        DROP_DEAD_NEBULA_OUTPOSTS.md
        DROP_DEAD_NEBULA_SHIPS.md
        DROP_DEAD_NEBULA_COMBAT.md
        DROP_DEAD_NEBULA_DEPLOYABLES.md
        DROP_DEAD_NEBULA_SOCIAL_SAFETY.md
        DROP_DEAD_NEBULA_ADMIN_OPS.md
        DROP_DEAD_NEBULA_PROGRESSION_BALANCE.md

    implementation/
      M0_IMPLEMENTATION_PLAN.md
      M1_FOUNDATION_HARDENING_PLAN.md
      M2_LIVING_CORRIDOR_PLAN.md

    ops/
      install.md
      package.md
      backup_restore.md
      admin_guide.md
      sysop_config.md
      troubleshooting.md

    player/
      quickstart.md
      captain_guide.md
      trading.md
      jobs.md
      salvage.md
      factions.md
      faq.md
```

---

## 4. Crate Boundary Options

There are three plausible final shapes.

### 4.1 Single Crate, Recommended Until Complexity Proves Otherwise

```text
src/lib.rs
src/main.rs
src/domain/...
src/services/...
src/ui/...
src/simulation/...
```

Pros:

- simplest;
- easy to refactor;
- no premature crate boundary mistakes;
- good for MVP through midgame.

Cons:

- can grow large;
- compile times may eventually rise;
- boundaries are by module convention, not crate boundary.

Recommendation:

- start here.

### 4.2 Workspace Split, Possible Mature Shape

If the game grows very large:

```text
drop-dead-nebula/
  Cargo.toml
  crates/
    ddn_core/
    ddn_content/
    ddn_sim/
    ddn_ui/
    ddn_cli/
    ddn_game/
```

Potential crates:

- `ddn_core`: pure domain types and rules;
- `ddn_content`: content loading and validation;
- `ddn_sim`: world ticks, NPC AI, economy simulation;
- `ddn_ui`: Ratatui screens/components;
- `ddn_cli`: admin and validation commands;
- `ddn_game`: composition/application service layer.

Pros:

- strong boundaries;
- possible faster incremental builds if done well;
- easier library reuse for bots/tools.

Cons:

- overkill early;
- cross-crate API friction;
- harder refactors.

Recommendation:

- do not start here unless Drop Dead Nebula immediately leaves the game-kit workspace and expects a long standalone life.

### 4.3 In-Game-Kit Example First

Inside existing repo:

```text
examples/drop_dead_nebula/
  Cargo.toml
  src/
  assets/
  migrations/
```

Pros:

- dogfoods `foglet_game` directly;
- easy to promote generic helpers to game-kit;
- good for developing P0 wishlist items.

Cons:

- example may become too large;
- game-specific code can distract from kit maintenance;
- packaging as standalone game may be less clean.

Recommendation:

- plausible for M0/M1 if the main goal is proving and improving the kit.
- migrate to standalone once the game’s identity solidifies.

---

## 5. Module Layering Philosophy

### 5.1 Domain Layer

`src/domain/` contains game concepts and rules without terminal UI concerns.

Examples:

- Captain;
- Ship;
- Commodity;
- Contract;
- Route;
- NPC memory;
- Faction standing;
- Outpost project.

Domain code should not know about Ratatui frames or screen navigation.

### 5.2 State Layer

`src/state/` owns persistence boundaries and data access.

Examples:

- repositories;
- migrations;
- save state;
- world DB access patterns.

State code should avoid game copy/prose/UI formatting.

### 5.3 Services Layer

`src/services/` coordinates domain rules and state mutations.

Examples:

- buy cargo;
- complete Contract;
- move Captain;
- run tick;
- post Notice;
- resolve encounter.

Services are where transactions and cross-system orchestration live.

### 5.4 Simulation Layer

`src/simulation/` owns tick and generation systems.

Examples:

- Market drift;
- NPC planning;
- Contract generation;
- route hazard drift;
- campaign phase advancement.

Simulation should call services or repositories rather than directly becoming UI.

### 5.5 UI Layer

`src/ui/` owns terminal presentation and input handling.

Examples:

- Dashboard screen;
- Market screen;
- Job Board screen;
- components;
- modals.

UI should request actions from services and render results. It should not own core rules.

### 5.6 Content Layer

`src/content/` owns authored data loading and validation.

Examples:

- stable keys;
- definitions;
- seed world;
- content validation.

Content definitions should be data-shaped; game effects belong in domain/services.

---

## 6. MVP Minimal Tree

Milestone 0 should not create the full final tree. A disciplined MVP could start with:

```text
src/
  lib.rs
  main.rs
  app.rs
  error.rs

  config/
    mod.rs

  content/
    mod.rs
    keys.rs
    seed.rs

  domain/
    mod.rs
    captain.rs
    ship.rs
    world.rs
    economy.rs
    inventory.rs
    jobs.rs

  state/
    mod.rs
    save.rs
    world.rs
    repositories.rs

  services/
    mod.rs
    captain_service.rs
    travel_service.rs
    market_service.rs
    job_service.rs
    event_service.rs

  ui/
    mod.rs
    app.rs
    screens/
      mod.rs
      title.rs
      dashboard.rs
      travel.rs
      market.rs
      jobs.rs
      cargo.rs
      log.rs
      help.rs
    components/
      mod.rs
      header.rs
      footer.rs
      key_hints.rs
    modals/
      mod.rs
      quantity.rs
      result.rs
      confirm.rs

assets/
  game.toml
  worlds/
    ash_mercy_corridor.toml

migrations/
  0001_core_world.sql
  0002_mvp_economy_jobs.sql

tests/
  common/
    mod.rs
  mvp_flow.rs
  travel.rs
  market.rs
  jobs.rs
  save_resume.rs
```

This keeps M0 small while leaving obvious expansion points.

---

## 7. Suggested `src/lib.rs`

Long-term `lib.rs` should expose only high-level modules:

```rust
pub mod admin;
pub mod app;
pub mod cli;
pub mod config;
pub mod content;
pub mod domain;
pub mod error;
pub mod foglet_integration;
pub mod prelude;
pub mod services;
pub mod simulation;
pub mod state;
pub mod ui;
```

MVP `lib.rs` can be much smaller.

---

## 8. Suggested `prelude.rs`

A small game prelude can reduce import noise without hiding architecture.

Possible exports:

```rust
pub use crate::error::{Error, Result};
pub use crate::domain::ids::*;
pub use crate::domain::money::Credits;
pub use crate::domain::quantities::Quantity;
```

Avoid dumping every type into prelude.

---

## 9. Domain Module Notes

### 9.1 `domain/captain/`

Owns player-facing identity and progression, not Foglet auth itself.

Likely contents:

- `model.rs`: Captain data shape;
- `creation.rs`: new Captain defaults;
- `reputation.rs`: broad reputation tracks;
- `heat.rs`: law attention;
- `stats.rs`: counters and achievements inputs.

### 9.2 `domain/ship/`

Owns Ship identity and rules.

Likely contents:

- hull catalog;
- module definitions;
- loadouts;
- cargo capacity;
- damage/repair;
- recovery/insurance.

### 9.3 `domain/world/`

Owns spatial rules and navigation concepts.

Likely contents:

- Place/Route types;
- Region tags;
- travel requirements;
- route hazards;
- scanning;
- Recall snapshots.

### 9.4 `domain/economy/`

Owns trade concepts.

Likely contents:

- Commodities;
- Markets;
- price labels;
- stock/equilibrium;
- trade outcome;
- scarcity/crisis.

### 9.5 `domain/jobs/`

Owns opportunity semantics.

Likely contents:

- Contract;
- Bounty view/adapters where game-specific;
- Challenge view/adapters;
- Job Board aggregation;
- objectives;
- proof and reward modeling;
- expiry rules.

### 9.6 `domain/npcs/`

Owns NPC concepts, not full tick processing.

Likely contents:

- actor model;
- archetypes;
- memory;
- goals;
- behavior descriptors.

### 9.7 `domain/social/`

Owns text safety and social surfaces.

Likely contents:

- sanitizer policy;
- report/block concepts;
- player-authored text fields.

---

## 10. Services Module Notes

Services should be verbs. They coordinate rules and persistence.

Examples:

### `travel_service.rs`

Responsible for:

- validate route;
- spend Daily Turns;
- move Presence;
- touch Recall;
- append Event;
- return arrival result.

### `market_service.rs`

Responsible for:

- buy;
- sell;
- capacity checks;
- credits changes;
- stock changes;
- trade Events/feedback.

### `job_service.rs`

Responsible for:

- list opportunities;
- accept Contract;
- claim Bounty;
- complete Contract/Bounty;
- apply reward;
- append Events/Notices.

### `tick_service.rs`

Responsible for:

- run due tick tasks;
- delegate to simulation modules;
- collect digest;
- handle failures.

### `admin_service.rs`

Responsible for:

- health checks;
- safe rescue;
- repair operations;
- audit records.

---

## 11. UI Module Notes

### 11.1 Screens

Screens are top-level states:

- Dashboard;
- Travel;
- Market;
- Jobs;
- Cargo;
- Log;
- Help;
- etc.

A screen should:

- render current view;
- handle input;
- call services for mutations;
- show feedback;
- avoid owning business rules.

### 11.2 Components

Reusable render pieces:

- header;
- footer;
- key hints;
- route row;
- market row;
- job card;
- requirements list;
- progress bar.

### 11.3 Modals

Temporary overlays:

- confirmation;
- result;
- quantity selector;
- details;
- error.

Keep modals generic when possible.

---

## 12. Content and Assets Notes

### 12.1 Content Format Strategy

Start simple. The final tree assumes TOML/YAML/MD content, but M0 can use static seed data if faster.

Promotion path:

1. static seed data;
2. structured seed file;
3. content validation;
4. richer content packs;
5. optional generated worlds.

### 12.2 Content Categories

- Places and Routes;
- Commodities and Markets;
- Ships and Modules;
- Contracts and Bounties;
- Factions and NPCs;
- Encounters and random tables;
- Salvage maps;
- Campaign phases;
- help and template text.

### 12.3 Dialog

Dialog should live in content files once NPC conversations are substantial.

Early MVP does not need a dialog system beyond Help and static text.

---

## 13. Migration Notes

`migrations/` should be boring and incremental.

Recommended grouping:

- core world first;
- economy/jobs next;
- tick/simulation tables;
- advanced systems only when they become playable.

Do not create final tables for every dream system in M0.

MVP can start with minimal migrations and expand.

---

## 14. Testing Tree Notes

### 14.1 `tests/common/`

Shared helpers:

- fake Foglet contexts;
- temp world setup;
- seed world creation;
- scripted session helpers;
- assertions.

### 14.2 Integration Tests by Milestone

Tests should mirror milestones:

- `mvp_flow.rs` protects the First Mercy Run regression path;
- `world_ticks.rs` begins M2;
- `salvage.rs` begins M4;
- `factions.rs` begins M7;
- etc.

### 14.3 Avoid UI-Only Testing Too Early

Test domain/services first. Add scripted terminal tests for high-value flows once screens stabilize.

---

## 15. CLI Notes

The game binary can support subcommands if appropriate:

```text
drop-dead-nebula run
drop-dead-nebula local-dev
drop-dead-nebula tick
drop-dead-nebula validate-content
drop-dead-nebula inspect-world
drop-dead-nebula export-summary
```

Alternatively, keep Foglet-run behavior as default and add admin commands later.

Do not let CLI complexity delay MVP.

---

## 16. What Not to Do Early

Avoid these early architecture traps:

- creating a workspace with six crates before MVP;
- implementing content loaders before content stabilizes;
- implementing every final migration table at once;
- building full NPC AI before background flow proves useful;
- building generic combat framework before one pirate event is fun;
- coupling Ratatui screens directly to storage queries everywhere;
- putting rules in UI components;
- making `main.rs` own game state and business logic;
- introducing macros before repeated patterns exist.

---

## 17. Early Architecture Decisions This Tree Suggests

Even in M0, prefer:

1. **Separate UI from services.**
   - Screens call service methods; services own mutations.

2. **Separate content keys from display names.**
   - Avoid future save/content breakage.

3. **Use a small domain layer.**
   - Even if types are simple, avoid anonymous strings everywhere.

4. **Put persistence behind repositories or state services.**
   - Prevent SQL/filesystem details from spreading into UI.

5. **Keep simulation separate from immediate player actions.**
   - Ticks should not be hidden inside random screens.

6. **Make MVP tests protect the trade spine.**
   - Every future milestone should keep First Mercy Run working.

7. **Design for promotion to game-kit.**
   - Generic helpers should have no Drop Dead Nebula nouns in their APIs.

---

## 18. Possible Final Workspace Tree

If the project later deserves multiple crates, an idiomatic workspace could look like:

```text
drop-dead-nebula/
  Cargo.toml
  crates/
    ddn_core/
      Cargo.toml
      src/
        lib.rs
        domain/
        content/
        error.rs
    ddn_world/
      Cargo.toml
      src/
        lib.rs
        state/
        repositories/
        migrations.rs
    ddn_sim/
      Cargo.toml
      src/
        lib.rs
        ticks/
        npc_ai/
        economy/
        generation/
    ddn_ui/
      Cargo.toml
      src/
        lib.rs
        screens/
        components/
        modals/
    ddn_game/
      Cargo.toml
      src/
        lib.rs
        services/
        app.rs
    ddn_cli/
      Cargo.toml
      src/
        main.rs
        commands/
  assets/
  migrations/
  tests/
  docs/
```

But again: this is a later extraction path, not the recommended starting point.

---

## 19. Recommended M0 Starting Point

For Milestone 0, start small but shaped like the future:

```text
src/
  main.rs
  lib.rs
  app.rs
  error.rs
  content/
  domain/
  state/
  services/
  ui/
```

Within each folder, create only what M0 needs.

The final file tree should be allowed to emerge through milestones, but early code should already respect the core separation:

```text
content -> domain -> services -> ui
              │         │
            state <─────┘
```

That separation is the most important architectural decision to preserve.
