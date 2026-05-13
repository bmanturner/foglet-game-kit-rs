# DateProvider patterns for game services

Use `DateProvider` anywhere a service needs "today". Do not call the
host clock directly from travel, turn spending, scans, world ticks, or
other date-sensitive service code. Injecting the date keeps tests fixed
and lets a game decide what "local day" means.

## Thread a provider through services

Small games can pass a provider directly:

```rust
use foglet_game::{FixedDateProvider, LocalDate};

let date_provider =
    FixedDateProvider::new(LocalDate::parse("2026-05-13")?);

let row = world.spend_turns(
    player_id,
    1,
    daily_allowance,
    carryover_max,
    &date_provider,
)?;
```

Larger games can bundle config, Foglet identity, and the provider in a
service context:

```rust
use foglet_game::{DateProvider, ServiceContext};

struct TravelService<'a, D: DateProvider + ?Sized> {
    services: ServiceContext<'a, D>,
}

impl<'a, D: DateProvider + ?Sized> TravelService<'a, D> {
    fn spend_for_scan(
        &self,
        world: &foglet_game::WorldDb,
        player_id: i64,
    ) -> Result<(), foglet_game::TurnError> {
        let turns = self.services.turns().expect("turns enabled");
        world.spend_turns(
            player_id,
            1,
            turns.daily_allowance,
            turns.carryover_max,
            self.services.date_provider,
        )?;
        Ok(())
    }
}
```

`ServiceContext` is only a borrowed helper. Existing APIs remain usable
when passing `GameConfig`, `FogletContext`, or `DateProvider` separately
is clearer.

## Fixed-date travel costs

Travel costs often need to spend a turn inside travel's active
transaction. Capture the injected date once before building the request,
then call `spend_turns_on` from `with_charge_cost_tx`:

```rust
use foglet_game::{spend_turns_on, TravelRequest};

let turns = services.turns().expect("turns enabled");
let today = services.today();

let result = world.travel(
    TravelRequest::new(player_id, destination_id)
        .with_charge_cost_tx(move |tx, presence, route| {
            spend_turns_on(
                tx,
                presence.player_id,
                1,
                turns.daily_allowance,
                turns.carryover_max,
                &today,
            )
            .map_err(|err| foglet_game::TravelError::CostFailed {
                player_id: presence.player_id,
                route_id: route.id,
                reason: err.to_string(),
            })?;
            Ok(())
        }),
)?;
```

In tests, use `FixedDateProvider` and assert the ledger row's
`local_date`. No test needs the machine's current date, and no test has
to sleep until midnight.

## World ticks and generated content

World ticks should receive the same provider through their service layer:

- tick due-date checks read `services.today()`;
- generated daily opportunities seed from `services.today()` plus stable
  game facts;
- scan or market refresh cooldowns compare against injected dates stored
  in game-owned tables.

This keeps runtime behavior deterministic under fixed-date tests while
leaving production free to supply a provider backed by the host clock,
Foglet deployment policy, or another game-owned calendar.
