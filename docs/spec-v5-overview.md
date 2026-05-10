# v5 Overview

v5 composes the earlier kit layers into reusable workflows.

Prior layers:

- v1 supplies terminal safety, input normalization, screens, widgets,
  maps, dialogs, and save files.
- v2 supplies the shared world DB, players, turns, events, and
  leaderboards.
- v2.1 supplies higher-level save and dialog screen ergonomics.
- v3 supplies async-BBS multiplayer primitives: notices, challenges,
  markets, factions, and bounties.
- v4 supplies spatial graphs, presence, place recall, owner-keyed
  inventory, and world ticks.

v5 additions:

- [Contracts](contracts.md) add generic work lifecycles.
- [Job Board](job-board.md) aggregates contracts, v3 bounties,
  challenges, and custom providers.
- [Travel](travel.md) composes spatial routes, presence, recall, and
  events in one transaction.
- [Inventory Capacity](inventory-capacity.md) adds policy-driven
  capacity checks to v4 inventory transfers.
- [Event Log Screen](event-log-screen.md) renders v2 events.
- [Multi-User Test Support](test-support-multi-user.md) creates local
  shared-world test scenarios.

Composition example:

1. A delivery Contract stores opaque objective JSON naming pickup and
   destination owners.
2. The Job Board renders that Contract next to bounties or challenges.
3. Accepting the row runs game code with the provider's opaque action
   token.
4. The player uses the Travel helper to move to the destination, with
   validation and cost callbacks.
5. A capacity-validated inventory transfer moves cargo into the
   destination owner.
6. Completion appends a world event.
7. The Event Log screen renders that event.
8. A multi-user harness test proves another fake user can see the
   shared-world result while player-scoped recall remains separate.

v5 is still genre-neutral. It does not add real-time multiplayer,
background daemons, scripted terminal drivers, or sample-game
integration of v5 features into the existing fixture.
