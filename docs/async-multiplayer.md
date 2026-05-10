# BBS-native async multiplayer

This is the operator- and game-author-facing reference for the v3
async multiplayer layer: mailbox notices, challenges, markets,
factions/shared goals, and bounties. It explains the model, the
durability story, and the helper surface each primitive ships with.

The contract this document satisfies lives in
[`SPEC_v3.md`](../SPEC_v3.md) — §3 (system overview), §4 (domain
model), §7 (operational requirements), §8 (test requirements), and
§10 (design biases). If anything below conflicts with the SPEC, the
SPEC wins.

For the underlying SQLite storage (file location, migrations, locking,
backup), see [`shared-world.md`](shared-world.md) — v3 reuses the
same per-game world DB and adds migrations starting at version 6.

## 1. Why mailbox multiplayer

v2 makes the world shared: events, players, leaderboards. v3 makes
players *aware of each other* without requiring them to be online at
the same time.

That is the BBS sweet spot. Leave a message, set a trap, buy out a
market, post a bounty, join a faction, challenge another caller —
and let the world react during the next login.

The design biases ([SPEC_v3 §10](../SPEC_v3.md#10-design-biases))
that follow from this:

- **Mailbox over live multiplayer.** Players read mail on login; they
  do not chat in real time.
- **Refresh-on-navigation over background polling.** Screens re-read
  the relevant table when the player opens them. There are no
  pollers, no background threads, no daemons.
- **Transactional state machines over ad-hoc flags.** Every lifecycle
  edge (challenge accept, market buy, faction contribute, bounty
  claim) is a single conditional `UPDATE … RETURNING` so a failure
  leaves no partial debit.
- **Game-defined payload JSON over over-generalised schemas.** The
  kit owns lifecycle invariants and durable storage. The game owns
  the shape of stake, result, item, reward, and metadata payloads.
- **Short bounded player-authored text over rich formatting.** Every
  player-typed field has a length cap enforced before the SQL
  round-trip, and rejected drafts never produce autoincrement gaps.

What v3 explicitly does *not* do is covered separately in §10.

## 2. The five primitives at a glance

| Primitive   | Migration | Module                       | Lifecycle                                      |
|-------------|-----------|------------------------------|------------------------------------------------|
| Notices     | v6        | [`notices`](#3-notices)      | unread → read → archived (idempotent edges)    |
| Challenges  | v7        | [`challenges`](#4-challenges)| open → accepted → resolved \| declined \| expired |
| Market      | v8        | [`market`](#5-market)        | active → exhausted (atomic buy decrement)      |
| Factions    | v9        | [`factions`](#6-factions)    | join/leave; goal active → completed            |
| Bounties    | v10       | [`bounties`](#7-bounties)    | open → claimed → completed; open\|claimed → expired |

All five are opt-in via `[multiplayer]` in `assets/game.toml`
([SPEC_v3 §5](../SPEC_v3.md#5-configuration-contract)). A v1 or v2
game that omits the block ships with every primitive disabled.

```toml
[multiplayer]
notices = true
challenges = true
market = true
factions = true
bounties = true
max_notice_body_chars = 1000
```

Each toggle defaults to `false` so opting into one primitive does not
implicitly light up the others.

## 3. Notices

[`SPEC_v3 §4.1`](../SPEC_v3.md#41-notice). Player-to-player and
system mail. The default inbox view hides archived notices; the
`idx_notices_inbox` partial index makes that view a reverse index
walk over `(recipient_player_id, created_at, id) WHERE archived_at IS NULL`.

Public surface (re-exported from `foglet_game`):

- `Notice`, `NoticeError`, `NOTICE_SUBJECT_MAX_CHARS`,
  `NOTICES_MIGRATION`.
- `WorldDb::send_notice(sender, recipient, kind, subject, body,
  expires_at, metadata, max_body_chars)` — single `INSERT … RETURNING`
  round-trip. Validates emptiness then char-count caps **before** the
  insert so a rejected draft produces no row.
- `WorldDb::inbox(recipient)` — newest-first, archived hidden.
- `WorldDb::mark_read(notice_id)` — idempotent via
  `SET read_at = COALESCE(read_at, CURRENT_TIMESTAMP)`. Second call
  preserves the original timestamp.
- `WorldDb::archive_notice(notice_id)` — same `COALESCE` shape.

System notices set `sender_player_id = None`. Subject and body are
length-bounded ([SPEC_v3 §4.1](../SPEC_v3.md#41-notice)) — body cap
is `max_notice_body_chars` from config, subject cap is the
kit-internal `NOTICE_SUBJECT_MAX_CHARS = 120`. Counts are Unicode
scalar values, not bytes.

## 4. Challenges

[`SPEC_v3 §4.2`](../SPEC_v3.md#42-challenge). Async duels. The
challenger posts an offer; the target sees it on next navigation and
accepts, declines, or lets it lapse.

```text
open ──► accepted ──► resolved
  │
  ├──► declined
  └──► expired (sweeper)
```

A schema-level `CHECK (state IN ('open','accepted','declined','resolved','expired'))`
locks the vocabulary so a typo in code fails at write time rather
than silently landing a corrupt row.

Public surface:

- `Challenge`, `ChallengeError`, `ChallengeState`,
  `CHALLENGES_MIGRATION`.
- `WorldDb::create_challenge(challenger, target, kind, stake,
  expires_at)` — entry state is always `open`; the caller cannot
  smuggle in a different starting state.
- `WorldDb::accept_challenge(id)` — single conditional
  `UPDATE … RETURNING` whose `WHERE` folds three checks:
  `id = ?`, `state = 'open'`, and a chronological deadline gate
  (`datetime(expires_at) > datetime('now')`). On no-match, a
  diagnostic SELECT maps the failure to typed `NotFound`, `Expired`,
  or `InvalidTransition` so UIs can render the right message.
- `WorldDb::decline_challenge(id)` — open → declined only. Decline
  does not gate on `expires_at`: a target can legitimately decline a
  lapsed open challenge before the sweeper runs.
- `WorldDb::resolve_challenge(id, result)` — accepted → resolved
  only. `result` is mandatory (rejected at the boundary as
  `EmptyResult`) so the resolved row never exists with a NULL result.
- `WorldDb::expire_open_challenges(now)` — sweeper that walks the
  partial `idx_challenges_open_expiring` index. `now` is a parameter,
  not `CURRENT_TIMESTAMP`, so callers control the cutoff. Returns the
  swept rows so the caller can append world events or render an
  "expired since last visit" notification.

The kit owns the lifecycle. Game code owns the shape of `stake` and
`result` JSON.

## 5. Market

[`SPEC_v3 §4.3`](../SPEC_v3.md#43-marketlisting). Shared listings.
Sellers post; buyers buy; the partial `idx_market_listings_active`
index over `(created_at, id) WHERE quantity > 0` means exhausted
listings fall out of the active view automatically the moment a buy
commits.

Public surface:

- `MarketListing`, `MarketError`, `MARKET_DISPLAY_NAME_MAX_CHARS`,
  `MARKET_LISTINGS_MIGRATION`.
- `WorldDb::create_listing(seller, item_key, display_name, price,
  quantity, expires_at, metadata)`. Schema-level
  `CHECK (price >= 0)` and `CHECK (quantity >= 0)` are the safety
  net behind the Rust-side validator. `price` is `INTEGER`; games
  that need fractional pricing scale into minor units at the
  boundary.
- `WorldDb::active_listings()` — `quantity > 0` and not lapsed,
  newest-first with an `id DESC` tiebreak so two listings that post
  in the same SQLite second do not flicker between renders.
- `WorldDb::buy_listing(listing_id, buyer, on_buyer_debit)` —
  **the only multi-statement transactional helper in v3**. Wraps a
  conditional decrement, the buyer-side debit callback, and a
  `market.buy` world event in a single `BEGIN … COMMIT`. If the
  callback returns `Err`, the transaction rolls back: listing
  quantity, buyer balance, and the world-event log are all
  unchanged.

Atomicity is the load-bearing contract: SPEC §4.3 mandates that buy
be atomic across quantity decrement, buyer callback, and event
append. Tests pin rollback by injecting a callback that returns
`Err`.

## 6. Factions and shared goals

[`SPEC_v3 §4.4`](../SPEC_v3.md#44-faction). Three tables: `factions`
(slugged definitions, seeded from `[[factions.seed]]`),
`faction_memberships` (player ↔ faction with role), and
`shared_goals` (target/current amount, optional faction scope).

Public surface:

- `Faction`, `FactionMembership`, `SharedGoal`, `SharedGoalState`,
  `FactionError`, `FactionsSection`, `FactionSeed`, plus three
  migrations.
- `WorldDb::seed_factions(seeds)` — idempotent upsert from config so
  game launches converge on the seeded roster.
- `WorldDb::join_faction(player, faction, role)` /
  `WorldDb::leave_faction(player, faction)` — both idempotent.
- `WorldDb::create_shared_goal(faction, key, target_amount)` —
  idempotent on `(faction, key)`.
- `WorldDb::contribute_to_goal(goal_id, contributor, amount,
  on_contribute)` — transactional. Wraps the `current_amount`
  increment, the contributor-side debit callback, the optional flip
  to `completed` when `current_amount >= target_amount`, and the
  optional `factions.goal_completed` world event in a single
  `BEGIN … COMMIT`. A callback that returns `Err` rolls back the
  goal increment.

A player MAY belong to multiple factions unless the game restricts
it; the kit does not impose mutual exclusion. Shared goal completion
appends a `faction.goal.completed` world event so faction screens
can render "case closed" banners on next refresh.

## 7. Bounties

[`SPEC_v3 §4.5`](../SPEC_v3.md#45-bounty). Posted jobs with a
JSON-shaped reward.

```text
open ──► claimed ──► completed
  │         │
  └─────────┴──► expired (sweeper)
```

Public surface:

- `Bounty`, `BountyError`, `BountyState`, `BOUNTIES_MIGRATION`.
- `WorldDb::post_bounty(poster, title, description, reward,
  expires_at)` — entry state is always `open`; single
  `INSERT … RETURNING`. Title and description are length-bounded.
- `WorldDb::claim_bounty(bounty_id, claimant)` — open → claimed
  only, with the same chronological deadline gate as
  `accept_challenge`. Two callers racing on the same open row
  serialise under SQLite's write lock: one wins; the loser surfaces
  `InvalidTransition { from: "claimed", to: Claimed }`.
- `WorldDb::complete_bounty(bounty_id)` — claimed → completed only.
  The SET list intentionally excludes `reward`,
  `claimed_by_player_id`, and `claimed_at` so the audit chain
  ("who held this bounty when it completed, and what was the
  reward") survives untouched. The kit does *not* gate on
  claimant — game code owns evidence validation per SPEC §4.5.
- `WorldDb::expire_bounties(now)` — sweeper that walks the partial
  `idx_bounties_expiring` index. Both `open` and `claimed` rows are
  eligible: a claimant who never completes their work should not pin
  the bounty open forever.

## 8. Player-authored text: bounds and sanitization

[`SPEC_v3 §7`](../SPEC_v3.md#7-operational-requirements) requires that
"player-authored text MUST be bounded and SHOULD be sanitized for
terminal display." Bounding is enforced by the kit at write time;
sanitization is enforced by the game at render time. The split is
deliberate: storage stays a faithful record of what the player typed
(an operator running `sqlite3` against the world DB sees the truth),
and rendering owns the responsibility of not corrupting another
player's terminal.

### 8.1 What is "player-authored"

Three primitives carry text that came directly from a player keystroke
and round-trips to another player's screen on next navigation. These
are the fields the kit enforces caps on:

| Primitive | Field        | Cap constant                          | Default | Configurable? |
|-----------|--------------|---------------------------------------|---------|---------------|
| Notices   | `subject`    | `NOTICE_SUBJECT_MAX_CHARS`            | 120     | Kit-internal  |
| Notices   | `body`       | `[multiplayer].max_notice_body_chars` | 1000    | Per-game      |
| Market    | `display_name` | `MARKET_DISPLAY_NAME_MAX_CHARS`     | 120     | Kit-internal  |
| Bounties  | `title`      | `BOUNTY_TITLE_MAX_CHARS`              | 120     | Kit-internal  |
| Bounties  | `description`| `BOUNTY_DESCRIPTION_MAX_CHARS`        | 1000    | Kit-internal  |

Three things that look player-authored but are *not*:

- **Faction slug, display name, and description.** Seeded from
  `[[factions.seed]]` in `assets/game.toml`, not typed by a player at
  runtime. Bounded by config-load validation, not by the v3 multiplayer
  layer.
- **Challenge `stake` and `result`, market `metadata`, bounty
  `reward`.** Game-defined JSON payloads. The kit treats them as opaque
  text and the game owns their shape. If a game lets players type into
  one of those payloads, the game owns the cap.
- **Notice `kind`, market `item_key`, bounty `reward` keys.** Short
  machine-readable namespaces chosen by the game author, not entered at
  runtime.

### 8.2 Where the cap is enforced

For every player-authored field above, validation runs **before** the
SQL round-trip, in this order:

1. Emptiness check (`subject.is_empty()` etc.) → typed `EmptySubject`
   / `EmptyBody` / `EmptyDisplayName` / `EmptyTitle` /
   `EmptyDescription` error.
2. Character-count check (`s.chars().count()`) → typed `…TooLong { max,
   actual }` error.

The cap is counted in **Unicode scalar values**, not bytes. A 4-emoji
body (16 UTF-8 bytes) passes a `chars().count() == 4` cap; the test
suite pins this explicitly so a regression to `len()` would fail
loudly. Counting in scalar values matches what authoring screens
display in their "120 / 137" character counters and avoids penalising
non-ASCII text.

A rejected draft never reaches SQLite. Three concrete consequences:

- No autoincrement gap (the `notices.id` sequence skips nothing because
  no `INSERT` ran).
- No partial index entry to clean up.
- No `world_events` row for the failed write.

The `actual` field on `…TooLong { max, actual }` is a UI affordance:
it lets the authoring screen render "you typed 137 characters; the
cap is 120" without re-counting in the caller.

### 8.3 Why caps live in code, not in `CHECK` constraints

A schema-level `CHECK (length(subject) <= 120)` would be hostile to
two things v3 wants to preserve:

- **Per-game tuning of the body cap.** `max_notice_body_chars` is
  configurable per-game; embedding it in a `CHECK` would require a
  migration whenever a game tightens or loosens its mail policy.
- **Typed errors at the boundary.** A `CHECK` failure surfaces as a
  generic `SQLITE_CONSTRAINT`; the kit-side validator returns
  `BodyTooLong { max, actual }` which UIs can render directly.

The schema does carry safety-net constraints for *non-text* invariants
(`market_listings.price >= 0`, `quantity >= 0`, the challenge state
vocabulary CHECK) where there is no per-game tuning story.

### 8.4 Terminal-display sanitization is the renderer's job

Storage is faithful: the kit does **not** strip control characters,
ANSI escapes, or zero-width codepoints from incoming player text. An
operator inspecting `notices.body` with `sqlite3` sees exactly what the
player typed, which is the right contract for moderation, audit, and
debugging.

Rendering is defensive: any TUI screen that draws player-authored text
to another player's terminal MUST pass the text through a sanitizer
before handing it to `ratatui`. The recommended sanitizer for v3
multiplayer screens:

- **Strip ASCII control characters** (`c.is_control()` returning
  `true`) except for tab and newline if the widget renders multi-line.
  An unstripped `ESC` (U+001B) followed by `[2J` would clear another
  player's screen on draw.
- **Strip the C1 control range** (U+0080 – U+009F). `crossterm` and
  `ratatui` route most output through their own escape pipeline, but a
  C1 byte arriving through a `Paragraph` widget can still reach the
  raw terminal on some emulators.
- **Replace zero-width and bidirectional override codepoints** (U+200B,
  U+200C, U+200D, U+200E, U+200F, U+202A–U+202E, U+2066–U+2069) with
  the Unicode replacement character. These are the codepoints that
  enable "reverse-character" homoglyph attacks on caller handles
  rendered next to a notice subject.
- **Truncate to the on-screen budget after sanitization**, not before.
  A widget with a 60-cell budget should call its truncator on the
  sanitized string so it never lands mid-escape.

The kit ships this sanitizer with the v3 multiplayer screens it adds
to Murder Motel (Tasks 10–13); a game that builds its own multiplayer
screens calls the same helper. Game-defined JSON payloads
(`stake`, `result`, `metadata`, `reward`) are sanitized by the same
helper if they include player-typed strings, but the game decides
which fields qualify because the kit treats those payloads as opaque.

### 8.5 What the kit does *not* do

- **No HTML/markdown stripping.** v3 is a terminal kit; player text is
  rendered as plain text. There is no markdown layer to escape.
- **No profanity filtering or content moderation.** That is a game
  policy choice. A game that wants moderation can wrap
  `WorldDb::send_notice` with its own pre-validation.
- **No rate limiting in the kit.** A spammer-shaped flow (one player
  flooding another's inbox) is rate-limited at the game layer, not the
  storage layer. The hooks the kit gives you for that are
  `WorldDb::inbox` (count recent senders) and the `created_at` column.

## 9. Reading the world without polling

The runtime contract ([SPEC_v3 §7](../SPEC_v3.md#7-operational-requirements))
is "no polling loops; async multiplayer updates are visible on
screen refresh/navigation."

In practice that means:

- Screens read the relevant table in their `on_enter` / draw path.
  Inbox renders by calling `WorldDb::inbox`; the marketplace renders
  by calling `WorldDb::active_listings`; the bounty board renders
  by selecting open bounties on demand. None of these spawn a
  background task.
- Sweepers (`expire_open_challenges`, `expire_bounties`) run from
  navigation hooks or login flows, not from a background thread.
  The `now` parameter makes them deterministic in tests.
- A two-player local-dev smoke test (e.g.
  `cargo run --example murder_motel -- --local-dev-user alice`
  followed by `--local-dev-user bob`) is sufficient to prove that
  one player's action is visible to the other on next navigation.

If a feature seems to want a daemon, a socket, or a poller — stop.
v3 is mailbox multiplayer; the contract is refresh-on-navigation.

## 10. What v3 is not

[`SPEC_v3 §2.2`](../SPEC_v3.md#22-non-goals) is the canonical list.
The headline exclusions:

- **No live sockets between running door sessions.** Players never
  share a connection.
- **No real-time combat, chat, or co-op.** Every interaction settles
  on a state-machine edge that another player observes on next
  login.
- **No Foglet board posting or profile badges.** Bounties are local
  to the game world and do not imply Foglet-side posts.
- **No cross-game identities.** Each game's world DB is its own
  scope.
- **No payment, external economy, or real-money mechanics.** Market
  prices are integer game-units only.

If a v4-shaped feature pulls toward any of those, append to
`DECISIONS.md` § Open Questions rather than implementing it under
the v3 contract.
