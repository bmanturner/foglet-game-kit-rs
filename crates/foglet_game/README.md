# foglet_game

`foglet_game` is the library crate in `foglet-game-kit-rs`. It gives
Foglet door authors a terminal-safe runtime, screen-stack game loop,
save helpers, config/manifest loaders, and reusable text UI and
shared-world primitives for `:external_pty` games.

This published crate contains the authoring library only. Repository
fixtures such as `murder_motel` stay in the Git checkout and are not
part of the crates.io package.

Pair it with the `fgk` CLI crate if you also want scaffolding,
manifest emission, and bundle packaging helpers.
