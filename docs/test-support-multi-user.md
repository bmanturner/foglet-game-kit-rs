# Multi-User Test Support

The multi-user harness is test-only and gated behind the
`test-support` Cargo feature:

```bash
cargo test -p foglet_game --features test-support
```

Builder API:

```rust
let harness = MultiUserHarness::builder()
    .add_user("alice", FogletRole::User)
    .add_user("bob", FogletRole::Mod)
    .build()?;
```

The harness creates one temp world DB and one save root per user. Use
`context_for(handle)` or `with_user(handle, f)` when testing screen or
runtime code that needs a `GameContext`. Use `world_db()` for direct
shared-world setup and assertions.

Async-BBS patterns:

- Assert shared state by having one user mutate the world DB, then query
  it from another user's perspective.
- Assert privacy by comparing player-scoped rows such as recall,
  notices, or events.
- `assert_event_visible_to` treats global events as visible and hides
  other users' player-scoped events.
- `assert_notice_for` is available only when the harness is built with
  notices enabled.

The harness is not a scripted terminal driver and does not simulate
real-time multiplayer. It is for deterministic local tests over one
shared SQLite world DB.
