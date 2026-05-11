# Job Board

The Job Board is an aggregation and rendering layer for opportunities.
It does not own the lifecycle of the work it shows.

Aggregation model:

- `JobBoard::query` gathers entries from `OpportunityProvider` values.
- Built-in providers expose available Contracts, Bounties, and
  Challenges when their primitives are enabled.
- Games can add external providers for local goals, rumors, commissions,
  or authored scenario hooks.
- Default ordering is stable: earliest `expires_at`, then source, then
  source id.

Provider boundaries:

- Providers return `JobBoardEntry` rows shaped for UI.
- `accept_action` is an opaque game token. The kit passes it back to the
  game callback without interpreting it.
- `title` and `summary` are sanitized at the aggregation boundary using
  the same bounded text rules as other multiplayer-facing surfaces.

Examples:

- Tavern board: contracts represent escort jobs, bounties represent
  wanted monsters, and an external provider adds a barkeep's errand.
- Noir case board: contracts represent client cases, challenges
  represent rival detective wagers, and an external provider adds
  precinct advisories.
