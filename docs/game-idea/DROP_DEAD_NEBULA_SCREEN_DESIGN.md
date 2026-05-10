# Drop Dead Nebula — Screen and UX Design

Status: Draft v0.1  
Companion docs: `DROP_DEAD_NEBULA.md`, `DROP_DEAD_NEBULA_GLOSSARY.md`, `DROP_DEAD_NEBULA_SYSTEMS.md`, `DROP_DEAD_NEBULA_MVP.md`, `DROP_DEAD_NEBULA_CONTENT_SEED.md`, `DROP_DEAD_NEBULA_GAME_KIT_WISHLIST.md`  
Purpose: Define the player-facing terminal screens for the MVP and sketch future UX directions for the more ambitious pursuits in Drop Dead Nebula.

---

## 1. UX North Star

Drop Dead Nebula should feel like a classic BBS door game that has learned modern UX discipline:

- fast keyboard-first navigation;
- readable at 80x24;
- obvious current status;
- no mystery about why a choice is unavailable;
- no wall of numbers without a clear next action;
- every session produces a compact story;
- ambitious systems appear as understandable boards, ledgers, charts, and reports.

The player should never wonder:

- Where am I?
- How many turns do I have?
- What can I do here?
- What changed because of my last action?
- How do I go back or quit?

---

## 2. Global Screen Rules

### 2.1 Terminal Assumptions

Primary design target:

- 80 columns x 24 rows.

Screens may become richer at larger sizes, but the baseline experience must not depend on color, mouse input, wide panels, or Unicode-heavy art.

### 2.2 Common Layout Pattern

Most screens should follow this shape:

```text
DROP DEAD NEBULA                         Turns 29/30  Credits 1,240
Captain Vanta | Rustbucket Mule | Ash Coil
────────────────────────────────────────────────────────────────────
[Screen title / local context]

Main content...

────────────────────────────────────────────────────────────────────
Hotkeys / hints / feedback line
```

### 2.3 Persistent Header

When practical, show:

- game title or compact title;
- Captain;
- Ship;
- current Place/Sector;
- Daily Turns remaining;
- credits;
- warning marker if cargo full, no turns, damaged, or wanted.

The full header can shrink on modal/detail screens.

### 2.4 Persistent Footer

The footer should usually show:

- `(↑/↓)` move selection;
- `(Enter)` choose;
- `(Esc/B)` back;
- `(?)` help;
- `(Q)` quit from safe screens;
- feedback text after the last action.

### 2.5 Disabled Choices

Disabled options remain visible if they teach the player something.

Good disabled examples:

```text
[ ] Buy Med Gel x10      Need 300 credits; you have 120
[ ] Travel to Red Maw    Requires 2 turns; you have 1
[ ] Complete Contract    Requires Med Gel in Cargo
```

Bad disabled examples:

```text
Complete Contract        unavailable
???                      locked
```

### 2.6 Confirmation Rules

Confirm only when an action is dangerous, irreversible, expensive, or likely accidental.

Confirm:

- abandon Contract;
- quit during unresolved dangerous scene;
- sell rare item;
- enter dangerous route;
- spend last Daily Turn on risky action;
- join/leave faction;
- deploy trap in lawful route.

Do not confirm:

- normal travel on safe route;
- opening screens;
- ordinary buy/sell of low-value goods in MVP;
- reading notices;
- viewing logs.

### 2.7 Result Feedback

Every meaningful action should produce immediate feedback.

Examples:

- `Bought 6 Med Gel for 300 credits.`
- `Spent 1 turn. Arrived at Mercy Relay.`
- `First Mercy Run complete. +180 credits.`
- `Cargo full: sell or jettison goods before buying more.`

### 2.8 Empty States

Empty states should be useful, not barren.

Examples:

- `No active Contracts. Check another Station or return after the next world tick.`
- `No Cargo in hold. Ash Coil Market has beginner freight.`
- `No unread Notices. The relay is quiet, which is rarely good news.`

---

## 3. MVP Screen Map

The MVP should include the minimum screens needed for the first trade-route slice.

```text
Title / Resume
   │
   ▼
Dashboard ───── Help
   │ │ │ │ │
   │ │ │ │ └── Log
   │ │ │ └──── Cargo
   │ │ └────── Jobs
   │ └──────── Market
   └────────── Travel
```

Required MVP screens:

1. Title / Resume
2. Dashboard
3. Travel
4. Market
5. Jobs / Contract Board
6. Cargo Manifest
7. Event Log
8. Help
9. Modal: confirmation / result detail where needed

The MVP does **not** need:

- inbox;
- faction screen;
- bounty board separate from jobs;
- combat screen;
- derelict local map;
- shipyard;
- outpost management;
- leaderboards;
- admin screens.

---

## 4. MVP Screen: Title / Resume

### Purpose

Set tone, identify the game, and get the player into the run quickly.

### UX Goal

A returning BBS caller should be one keypress from the dashboard.

### Wireframe

```text
╔════════════════════════════════════════════════════════════════════╗
║                         DROP DEAD NEBULA                           ║
║              Salvage, freight, and bad decisions                   ║
╚════════════════════════════════════════════════════════════════════╝

A dying relay corridor. A borrowed hauler. Thirty turns to matter.

Captain: Vanta
Ship:    Rustbucket Mule
Last:    Ash Coil

(R) Resume Captain
(N) New Local-Dev Captain       disabled if existing captain policy forbids
(H) Help
(Q) Quit to Foglet

────────────────────────────────────────────────────────────────────
Choose a key. Terminal baseline: 80x24.
```

### Required Elements

- title;
- one-line premise;
- resume/new affordance according to player state;
- quit path;
- no lore dump before play.

### MVP Behavior

- New player proceeds to Captain creation/resume default.
- Returning player resumes current Captain.
- No elaborate character creation in MVP.

### Future Enhancements

- season banner;
- last session summary;
- unread notices count;
- “continue recommended action” prompt;
- memorial line after ship loss.

---

## 5. MVP Screen: Dashboard

### Purpose

The main command center.

### UX Goal

In five seconds, the player understands their status and best next action.

### Wireframe: Ash Coil Start

```text
DROP DEAD NEBULA                         Turns 30/30  Credits 1,000
Captain Vanta | Rustbucket Mule | Ash Coil
────────────────────────────────────────────────────────────────────
ASH COIL — refinery station / safe port
A refinery station wrapped around a cooling industrial core.

Signals:
 ! Mercy Relay requests medical freight.
 $ Ash Coil has Med Gel surplus.
 ? Blue Blind salvage licenses are not available in this slice.

Cargo: empty                              Hold: 0/20
Active Contract: none

(T)ravel   (M)arket   (J)obs   (C)argo   (L)og   (H)elp   (Q)uit
────────────────────────────────────────────────────────────────────
Tip: Open Jobs and accept First Mercy Run, then buy Med Gel.
```

### Wireframe: After Arriving at Mercy Relay

```text
DROP DEAD NEBULA                         Turns 29/30  Credits 700
Captain Vanta | Rustbucket Mule | Mercy Relay
────────────────────────────────────────────────────────────────────
MERCY RELAY — damaged comms station / med shortage
A half-repaired relay serving refugee traffic and emergency broadcasts.

Signals:
 ! First Mercy Run can be completed here.
 $ Mercy Relay is buying Med Gel above corridor average.

Cargo: Med Gel x6                         Hold: 6/20
Active Contract: First Mercy Run

(T)ravel   (M)arket   (J)obs   (C)argo   (L)og   (H)elp   (Q)uit
────────────────────────────────────────────────────────────────────
Feedback: Arrived at Mercy Relay. Spent 1 turn.
```

### Required Elements

- current Place name and kind;
- short description;
- actionable Signals;
- Cargo summary;
- active Contract summary;
- main hotkeys;
- feedback line.

### UX Notes

- Signals are the dashboard’s best UX tool. They should point toward useful actions without becoming a quest arrow.
- Dashboard should not show every stat. It should show what matters now.
- Keep one plain-language tip for new players in MVP.

### Future Enhancements

- unread Notices count;
- Heat warning;
- hull/fuel state;
- local NPC presence;
- faction state;
- region crisis banner;
- daily intel digest.

---

## 6. MVP Screen: Travel

### Purpose

Let the player choose a directed Route from the current Place.

### UX Goal

Make graph movement clear without requiring a visual map.

### Wireframe: From Ash Coil

```text
DROP DEAD NEBULA                         Turns 30/30  Credits 1,000
Captain Vanta | Rustbucket Mule | Ash Coil
────────────────────────────────────────────────────────────────────
TRAVEL FROM ASH COIL

Choose an outbound Route:

> Mercy Relay        Cost 1 turn   safe corridor
  Cinder Pocket      Cost 1 turn   mining rock / ore traffic

Known but unavailable:
  Dead Gate Verge    not reachable from Ash Coil

Route note:
Mercy Relay is buying Med Gel. First Mercy Run completes there.

────────────────────────────────────────────────────────────────────
(↑/↓) select  (Enter) travel  (B/Esc) back  (?) help
```

### Wireframe: From Blue Blind Later

```text
TRAVEL FROM BLUE BLIND

> Mercy Relay          Cost 1 turn   known return route
  Red Maw Approach     Cost 2 turns  warning: unstable anomaly edge

────────────────────────────────────────────────────────────────────
Entering dangerous routes will require confirmation in later slices.
```

### Required Elements

- current Place;
- outbound Routes only;
- destination display name;
- turn cost;
- short route note;
- disabled reason when applicable.

### MVP Behaviors

- Travel costs Daily Turns.
- Travel updates Presence.
- Travel updates Recall.
- Travel gives arrival feedback.

### Disabled Examples

```text
[ ] Mercy Relay       Cost 1 turn   Need 1 turn; you have 0
[ ] Dead Gate Verge   silent        Route unavailable in this slice
```

### Future Enhancements

- route hazard level;
- stale intel marker;
- fuel cost;
- scan shortcut;
- route preview modal;
- hidden route reveal;
- confirmation for dangerous routes;
- local route ASCII graph.

---

## 7. MVP Screen: Market

### Purpose

Support the buy-low/sell-high loop.

### UX Goal

Show what the station sells, what the player carries, and why a trade is good or blocked.

### Wireframe: Ash Coil Market

```text
DROP DEAD NEBULA                         Turns 30/30  Credits 1,000
Captain Vanta | Rustbucket Mule | Ash Coil
────────────────────────────────────────────────────────────────────
ASH COIL MARKET

Commodity          Buy     Sell    Stock   You   Note
> Med Gel           50      35      40      0     Mercy Relay wants this
  Reactor Coolant   80      55      20      0     industrial staple
  Low-Grade Ore     --      12      --      0     Ash Coil buys ore

Hold: 0/20       Credits: 1,000

(B)uy selected   (S)ell selected   (D)etails   (Esc) back
────────────────────────────────────────────────────────────────────
Tip: Buy Med Gel for the First Mercy Run.
```

### Wireframe: Mercy Relay Market

```text
MERCY RELAY MARKET

Commodity          Buy     Sell    Stock   You   Note
> Med Gel           --      75      6       6     emergency demand
  Reactor Coolant   --      95      2       0     relay cooling shortage
  Low-Grade Ore     --      10      --      0     no current demand

Hold: 6/20       Credits: 700

(S)ell selected   (D)etails   (Esc) back
────────────────────────────────────────────────────────────────────
Feedback: Mercy Relay buys Med Gel above Ash Coil price.
```

### Required Elements

- commodity list;
- buy price where station sells;
- sell price where station buys;
- stock;
- player-owned quantity;
- note or demand clue;
- credits;
- hold capacity;
- buy/sell actions.

### Trade Quantity Modal

```text
BUY MED GEL

Ash Coil price: 50 credits each
In stock:       40
You carry:      0
Hold space:     20
Credits:        1,000

Quantity: 6
Total:    300 credits

(←/→) adjust  (M)ax  (Enter) confirm  (Esc) cancel
```

### Disabled Examples

```text
[ ] Buy Med Gel x10     Need 500 credits; you have 120
[ ] Buy Reactor Coolant Cargo full: hold 20/20
[ ] Sell Ore            You have none
```

### UX Notes

- MVP can use simple fixed prices, but UI should already communicate buy/sell asymmetry.
- Show “You” quantity; players should not need to switch to Cargo to know what they carry.
- “Details” should include short flavor and trade role.

### Future Enhancements

- price trend arrows;
- stale recall comparison;
- black-market tabs;
- player listings;
- contract demand indicators;
- faction discounts;
- market event banners;
- bulk trade presets.

---

## 8. MVP Screen: Jobs / Contract Board

### Purpose

Give the player a clear objective.

### UX Goal

The Job Board should be more than a menu; it should tell the player what work is available, what state it is in, and what to do next.

### Wireframe: Ash Coil Before Accepting

```text
DROP DEAD NEBULA                         Turns 30/30  Credits 1,000
Captain Vanta | Rustbucket Mule | Ash Coil
────────────────────────────────────────────────────────────────────
ASH COIL JOB BOARD

> [Contract] First Mercy Run        available
             Deliver Med Gel to Mercy Relay
             Reward: 180 credits

Unavailable in this slice:
  [Bounty]   Blue Blind black box    salvage licenses closed
  [Faction]  Relay repair fund       faction offices closed

────────────────────────────────────────────────────────────────────
(A)ccept  (D)etails  (Esc) back
```

### Wireframe: Accepted

```text
ASH COIL JOB BOARD

> [Contract] First Mercy Run        accepted
             Destination: Mercy Relay
             Need: Med Gel in Cargo
             Reward: 180 credits

Next step:
Buy Med Gel at Ash Coil, then travel to Mercy Relay.

────────────────────────────────────────────────────────────────────
(D)etails  (Esc) back
```

### Wireframe: At Mercy Relay With Requirements Met

```text
MERCY RELAY JOB BOARD

> [Contract] First Mercy Run        ready to complete
             Deliver Med Gel to Mercy Relay
             Reward: 180 credits

Requirements:
 ✓ At Mercy Relay
 ✓ Med Gel in Cargo

────────────────────────────────────────────────────────────────────
(C)omplete  (D)etails  (Esc) back
```

### Required Elements

- opportunity type label;
- state;
- objective;
- destination if any;
- reward;
- required next step;
- disabled completion reasons.

### UX Notes

- Use the unified Job Board concept even in MVP, but only Contract is active.
- Showing inactive Bounty/Faction rows as “closed in this slice” is acceptable if it is clearly flavor, not broken functionality.
- The board should answer “what do I do next?”

### Future Enhancements

- mixed Contracts/Bounties/Challenges/Faction Goals;
- filters by type;
- expiry timers;
- accepted jobs tab;
- claim/accept/abandon/complete flows;
- notices linked to jobs;
- player-posted work.

---

## 9. MVP Screen: Cargo Manifest

### Purpose

Show what the Ship carries and how much space remains.

### UX Goal

Make Cargo constraints legible and support Market/Contract decisions.

### Wireframe

```text
DROP DEAD NEBULA                         Turns 29/30  Credits 700
Captain Vanta | Rustbucket Mule | Mercy Relay
────────────────────────────────────────────────────────────────────
CARGO MANIFEST — RUSTBUCKET MULE

Hold: 6/20 used

Commodity          Qty    Notes
> Med Gel           6     Mercy Relay demand / First Mercy Run
  Reactor Coolant   0     industrial staple
  Low-Grade Ore     0     refinery input

No modules installed in this slice.

────────────────────────────────────────────────────────────────────
(D)etails  (Esc) back
```

### Required Elements

- Ship name;
- capacity used/total;
- quantities;
- notes relevant to current Place or Contract;
- detail view for Commodity descriptions.

### Future Enhancements

- jettison with confirmation;
- contraband warning;
- mission cargo separation;
- damaged/spoiling cargo;
- cargo sorting/filtering;
- transfer to station/outpost/corp depot.

---

## 10. MVP Screen: Event Log

### Purpose

Show that the world remembers.

### UX Goal

The player should see clear, satisfying consequences without reading raw system logs.

### Wireframe

```text
DROP DEAD NEBULA                         Turns 29/30  Credits 1,330
Captain Vanta | Rustbucket Mule | Mercy Relay
────────────────────────────────────────────────────────────────────
RECENT EVENTS

Today
> Vanta completed First Mercy Run for Mercy Relay. +180 credits.
  Vanta arrived at Mercy Relay from Ash Coil.
  Vanta bought Med Gel at Ash Coil.

Earlier
  Welcome to the Ash Mercy Corridor.

────────────────────────────────────────────────────────────────────
(↑/↓) scroll  (Esc) back
```

### Required Elements

- recent Events;
- readable text;
- chronological grouping if useful;
- no private data leakage;
- empty state.

### Empty State

```text
No Events yet.

The corridor is quiet. That will change after your first delivery.
```

### Future Enhancements

- global vs personal filter;
- faction news;
- NPC actions;
- market reports;
- bounty completions;
- weekly chronicle;
- exportable season history.

---

## 11. MVP Screen: Help

### Purpose

Teach the first loop without overwhelming the player.

### UX Goal

Help should be short, contextual, and written like a dock clerk who wants the player to survive.

### Wireframe

```text
DROP DEAD NEBULA                         Help
────────────────────────────────────────────────────────────────────
FIRST RUN GUIDE

1. Open (J)obs at Ash Coil.
2. Accept First Mercy Run.
3. Open (M)arket and buy Med Gel.
4. Open (T)ravel and fly to Mercy Relay.
5. Sell or deliver the Med Gel.
6. Check the (L)og.
7. (Q)uit safely when done.

Terms:
Daily Turn  - your limited action budget.
Cargo       - goods in your Ship hold.
Contract    - paid work from a Station.
Route       - directed path to another Place.

────────────────────────────────────────────────────────────────────
(Esc) back
```

### Required Elements

- first-run steps;
- core terms;
- controls;
- quit reminder.

### Future Enhancements

- glossary browser;
- contextual help per screen;
- tutorial checklist;
- “suggest next action” assistant panel;
- relay AI mentor.

---

## 12. MVP Modals and Result Panels

### 12.1 Generic Result Panel

```text
┌──────────────────────────────────────────────┐
│ First Mercy Run Complete                     │
│                                              │
│ Mercy Relay takes the Med Gel before the     │
│ dock clamps finish cycling. Someone in the   │
│ med bay cheers through a broken speaker.     │
│                                              │
│ Reward: +180 credits                         │
│ Event recorded in Log.                       │
│                                              │
│                  (Any key)                   │
└──────────────────────────────────────────────┘
```

### 12.2 Quit Confirmation

Only needed if quitting from a questionable state. From the safe dashboard, direct quit is acceptable.

```text
┌──────────────────────────────────────────────┐
│ Quit to Foglet?                              │
│                                              │
│ Your Captain will resume at Mercy Relay.     │
│                                              │
│ (Y) Quit safely      (N/Esc) Stay            │
└──────────────────────────────────────────────┘
```

### 12.3 Disabled Explanation Modal

Usually the feedback line is enough. Use a modal only if the explanation is multi-step.

```text
┌──────────────────────────────────────────────┐
│ Cannot Complete Contract                     │
│                                              │
│ First Mercy Run requires:                    │
│ ✓ Be at Mercy Relay                          │
│ ✗ Carry Med Gel                              │
│                                              │
│ Buy Med Gel at Ash Coil or retrieve it from  │
│ your Cargo if you stored it elsewhere later. │
│                                              │
│                  (Any key)                   │
└──────────────────────────────────────────────┘
```

---

## 13. MVP Happy Path Screen Flow

```text
Title
  Resume
Dashboard at Ash Coil
  Jobs
    Accept First Mercy Run
Dashboard
  Market
    Buy Med Gel
Dashboard
  Travel
    Mercy Relay
Arrival feedback
Dashboard at Mercy Relay
  Market
    Sell Med Gel
Dashboard
  Jobs
    Complete First Mercy Run
Completion modal
Dashboard
  Log
Dashboard
  Quit
```

### UX Success Criteria

- No step requires guessing hidden commands.
- Each screen suggests the next likely action.
- The player can back out of every non-final action.
- The player can complete the loop without reading external docs.
- The player sees at least one satisfying result panel or feedback line.

---

## 14. Future UX Principle: Add Depth Without Adding Confusion

Each ambitious pursuit should enter the game through a familiar pattern:

- a board;
- a ledger;
- a chart;
- a manifest;
- a report;
- a modal choice;
- a compact map.

Avoid dumping complex systems directly into the dashboard. The dashboard should advertise opportunities; specialized screens should handle complexity.

---

# Future Pursuit Sketches

The following sections sketch screen ideas for the larger game. These are not MVP requirements.

---

## 15. Pursuit: Living Galaxy / Daily Intel

### Player Fantasy

The world moved while you were away.

### Screen: Daily Intel Packet

```text
DROP DEAD NEBULA                         Day 18 Intel
────────────────────────────────────────────────────────────────────
WHILE YOU WERE OUT

Markets
 $ Mercy Relay demand for Med Gel cooled slightly.
 $ Cinder Pocket ore output rose after a quiet shift.

Routes
 ! Blue Blind reports fresh wreck signatures.
 ! Red Maw Approach instability increased.

People
 @ Brass Jory undercut coolant prices at Ash Coil.
 @ Marshal Senn posted a warning about forged transponders.

Jobs
 + 2 new Contracts available in the Ash Mercy Corridor.
 + 1 Bounty posted near Blue Blind.

────────────────────────────────────────────────────────────────────
(J)obs  (M)arket  (L)og  (Enter) continue
```

### UX Notes

- Summarize, do not spam raw tick output.
- Group by category.
- Let the player jump directly to relevant screens.
- Use symbols consistently: `$` market, `!` hazard, `@` actor, `+` opportunity.

### Game-Kit Mapping

- World Ticks;
- Event Log;
- Notices;
- Market;
- Bounties/Contracts;
- NPC simulation.

---

## 16. Pursuit: Notice Inbox and Mail

### Player Fantasy

The relay has messages: some useful, some threatening, some from people who know what you did.

### Screen: Inbox

```text
DROP DEAD NEBULA                         Inbox 3 unread
────────────────────────────────────────────────────────────────────
INBOX

> * [Faction] Mercy Relay thanks relief captains
  * [NPC] Brass Jory: stay off my route
    [System] First Mercy Run completed
    [Market] Ash Coil coolant surplus updated

────────────────────────────────────────────────────────────────────
(Enter) read  (A)rchive  (R)eply if allowed  (Esc) back
```

### Screen: Notice Detail

```text
FROM: Brass Jory
SUBJ: stay off my route
────────────────────────────────────────────────────────────────────
Cute little mercy run, Captain.

Do it twice and people start calling it a business.
Do it three times and I start calling it mine.

— Jory

────────────────────────────────────────────────────────────────────
(A)rchive  (M)ark unread  (Esc) back
```

### UX Notes

- Inbox is a consequence surface, not a chat room first.
- Replies should wait until bounded player text and moderation rules are ready.
- NPC messages should be short and punchy.

### Game-Kit Mapping

- Notices/Mail;
- bounded player-authored text later;
- NPC memory;
- Event Log.

---

## 17. Pursuit: Unified Job Board

### Player Fantasy

Every station has work, but not all work is the same.

### Screen: Mixed Job Board

```text
MERCY RELAY JOB BOARD                    Turns 22/30
────────────────────────────────────────────────────────────────────
Type        Job                              State       Reward
> Contract  Coolant for the Antennas         available   220 cr
  Bounty    Black Box at Blue Blind          open        500 cr
  Faction   Relay Repair Fund                43%         access
  Challenge Beat Brass Jory's run            expires 1d  wager

Details:
Coolant for the Antennas
Deliver Reactor Coolant to Mercy Relay before tomorrow's tick.

────────────────────────────────────────────────────────────────────
(A)ccept/(C)laim  (D)etails  (F)ilter  (Esc) back
```

### UX Notes

- Type labels prevent lifecycle confusion.
- Action label changes by type: Accept Contract, Claim Bounty, Open Challenge, Contribute Goal.
- Filters matter once the board grows.

### Game-Kit Mapping

- Contracts if added;
- Bounties;
- Challenges;
- Faction/shared goals;
- Job Board wishlist surface.

---

## 18. Pursuit: Star Chart and Place Recall

### Player Fantasy

Your map is yours. It remembers what you saw, not necessarily what is true now.

### Screen: Star Chart

```text
STAR CHART — ASH MERCY CORRIDOR          Recall view
────────────────────────────────────────────────────────────────────
Known Places

> Ash Coil          Station     seen today      safe
  Mercy Relay       Relay       seen today      med demand
  Cinder Pocket     Mining      seen 2d ago     ore source
  Blue Blind        Wreck Field rumor only      salvage locked
  Red Maw Approach  Anomaly     unknown         danger

Routes from selected:
 Ash Coil -> Mercy Relay       cost 1   safe corridor
 Ash Coil -> Cinder Pocket     cost 1   ore traffic

────────────────────────────────────────────────────────────────────
(T)ravel if reachable  (N)otes  (S)can later  (Esc) back
```

### UX Notes

- Distinguish known, rumored, stale, and unknown.
- Never pretend stale data is current.
- Keep graph representation textual until a visual map is worth it.

### Game-Kit Mapping

- Places;
- Routes;
- Presence;
- Place Recall;
- future scan/intel systems.

---

## 19. Pursuit: Dynamic Markets and Trade Intelligence

### Player Fantasy

Markets are alive, readable, and exploitable if you pay attention.

### Screen: Advanced Market

```text
ASH COIL MARKET                           Credits 4,820 Hold 8/35
────────────────────────────────────────────────────────────────────
Commodity        Buy   Sell  Stock Trend  You  Signal
> Med Gel         62    37    12    ↑↑     0    audit soon
  Coolant         74    51    80    ↓      2    surplus
  Ore             --    18    --    →      0    refinery input
  Relay Coils     310   190   3     ↑      0    rare

Intel:
Mercy Relay last seen buying Med Gel at 91, 1 day ago.

────────────────────────────────────────────────────────────────────
(B)uy  (S)ell  (I)ntel  (R)ecall prices  (Esc) back
```

### UX Notes

- Trend arrows are hints, not formulas.
- “Last seen” market recall gives value to intel without overwhelming players.
- Advanced pricing should remain explainable in broad terms.

### Game-Kit Mapping

- Market;
- owner-keyed inventory;
- Place Recall snapshots;
- World Ticks;
- Event Log.

---

## 20. Pursuit: Salvage / Derelict Boarding

### Player Fantasy

The money is inside the dead ship, but so is the reason it died.

### Screen: Salvage Site Approach

```text
BLUE BLIND WRECK FIELD                    Turns 18/30 Hull 82%
────────────────────────────────────────────────────────────────────
WRECK SIGNATURE: DDN-44 "Choirless Bell"

Status: partially intact freighter
Risk:   moderate radiation / unknown drone activity
Value:  black box, spare parts, sealed cargo
Time:   1 turn to board, more turns inside

Options:
> Board the derelict
  Scan first                         cost 1 turn
  Mark for later                     no cost
  Leave

────────────────────────────────────────────────────────────────────
(Enter) choose  (Esc) back
```

### Screen: Local Derelict Map

```text
CHOIRLESS BELL — DECK 1                  Turns 16/30 Oxygen stable
────────────────────────────────────────────────────────────────────
###########
#A..#..C..#      A Airlock     ✓ safe
#.#.#.##.#       C Cargo Bay   ? unopened
#.#...#B.#       B Bridge      ! hazard ping
#D#####..#       D Drone Nest  ! avoid?
###########

You are at: Airlock
Cargo: 10/20

────────────────────────────────────────────────────────────────────
(N/S/E/W) move  (S)can room  (L)oot  (R)eturn to ship
```

### Screen: Salvage Choice Modal

```text
SEALED CARGO CRATE

Inside: Relay Coils x2
Value:  high
Mass:   6 cargo
Risk:   claim beacon may alert original owner

Your hold: 16/20

(T)ake  (J)ettison other cargo  (L)eave it  (Esc) back
```

### UX Notes

- Local maps should be small and readable.
- Salvage choices should create cargo/turn/risk tradeoffs.
- Always provide a clear return-to-ship action.
- Hazards should telegraph enough to feel fair.

### Game-Kit Mapping

- v1 maps;
- prompts;
- owner-keyed inventory;
- turns;
- random tables;
- event log;
- bounties.

---

## 21. Pursuit: Random Events

### Player Fantasy

The route was supposed to be quiet. It is not.

### Screen: Travel Event

```text
ION SQUALL                                Ash Coil -> Mercy Relay
────────────────────────────────────────────────────────────────────
The route lights blue, then white. Your panels lag half a second
behind your hands. Mercy Relay is still ahead, but the Mule is
starting to complain.

Choose:
> Ride it out                 no extra turn, minor hull risk
  Stabilize the jump          spend 1 turn, avoid damage
  Dump coolant into baffles   consume Reactor Coolant, safe arrival

────────────────────────────────────────────────────────────────────
(Enter) choose  (D)etails
```

### UX Notes

- Events are short choice moments, not hidden dice rolls only.
- Choices should reveal cost categories: turn, cargo, hull, heat, reputation.
- Quiet/no-event travel remains common enough that the game does not become exhausting.

### Game-Kit Mapping

- deterministic random table helper;
- prompts;
- turns;
- inventory;
- event log.

---

## 22. Pursuit: Combat and Interdiction

### Player Fantasy

You do not always win by firing. Sometimes you win by leaving with enough cargo to matter.

### Screen: Interdiction Encounter

```text
PIRATE INTERDICTION                       Red Maw Approach
────────────────────────────────────────────────────────────────────
A Dust Clan cutter burns across your bow.

Enemy:   Knife Sermon, light raider
Threat:  moderate
Demand:  120 credits or 3 cargo
Your:    Hull 82%, Shields weak, Cargo 14/20

Choose response:
> Run cold                   evade, risk engine damage
  Pay toll                   lose credits/cargo, avoid fight
  Fight                      spend turn, risk hull/cargo
  Bluff as Port Authority    heat/faction check
  Dump decoy beacon          consume deployable

────────────────────────────────────────────────────────────────────
(Enter) choose  (V)iew odds  (Esc) not available
```

### Screen: Combat Result

```text
RESULT: ESCAPED

The Mule screams, sheds a heat panel, and drops through the static.
The Knife Sermon does not follow.

Losses:
- Hull -8%
- 1 Daily Turn spent
- Event recorded privately

────────────────────────────────────────────────────────────────────
(Any key)
```

### UX Notes

- Combat should be decision-first, not stat-wall-first.
- Always show likely stakes.
- Avoid hard death as common result.
- Let non-combat builds survive through bribes, stealth, cargo loss, or favors.

### Game-Kit Mapping

- prompts;
- turns;
- inventory transfers;
- event log;
- notices;
- bounties later.

---

## 23. Pursuit: Smuggling, Heat, and Customs

### Player Fantasy

The best routes are profitable because they are watched.

### Screen: Customs Inspection

```text
CUSTOMS PING                              Mercy Relay Approach
────────────────────────────────────────────────────────────────────
Port Authority wants your manifest.

Heat:        WANTED-1
Contraband:  possible, hidden hold quality poor
Patrol:      routine inspection

Choose:
> Submit manifest            safe if clean, risky if not
  Bribe inspector            costs credits, may increase future heat
  Spoof transponder          module check
  Run dark                   spend turn, risk interdiction

────────────────────────────────────────────────────────────────────
(Enter) choose  (C)argo manifest  (H)eat details
```

### Screen: Heat Details

```text
HEAT

Global:        clean
Port Authority: suspicious
Mercy Relay:   watched
Dust Clans:    nobody

Recent causes:
- spoofed transponder near Mercy Relay
- carried restricted coils through Ash Coil customs

Heat fades through time, favors, bribes, or good behavior.
```

### UX Notes

- Heat must be visible enough to feel fair.
- Players should know when they are taking illegal risk.
- Smuggling should offer multiple playstyles: stealth, bribery, speed, social access.

### Game-Kit Mapping

- inventory metadata;
- random events;
- notices;
- event log;
- future relationship/reputation primitive.

---

## 24. Pursuit: Factions and Shared Goals

### Player Fantasy

You are not just earning credits. You are helping decide who owns the corridor.

### Screen: Faction Overview

```text
FACTIONS — ASH MERCY CORRIDOR
────────────────────────────────────────────────────────────────────
Faction                    Standing   Local Influence   Status
> Mourning Union              +3            42%          rebuilding
  Helix Cartel                +0            31%          buying favors
  Port Authority Black Office -1            18%          suspicious
  Saints of Vacuum            ?             9%           listening

Selected: Mourning Union
They repair what everyone else writes off.

────────────────────────────────────────────────────────────────────
(G)oals  (J)obs  (R)eputation details  (Esc) back
```

### Screen: Shared Goal

```text
MOURNING UNION GOAL — RELAY REPAIR FUND
────────────────────────────────────────────────────────────────────
Goal: Stabilize Mercy Relay antenna spine
Progress: 43%  [########...........]

Needed:
 ✓ Med Gel relief delivered
 - Reactor Coolant: 18 / 40
 - Relay Coils: 1 / 5
 - Credits: 2,200 / 5,000

Reward if completed:
- safer Mercy route
- better relay notices
- Union repair discount

────────────────────────────────────────────────────────────────────
(C)ontribute  (J)obs for this goal  (Esc) back
```

### UX Notes

- Factions should show concrete consequences, not abstract meters only.
- Shared goals need “what changes if we finish this?”
- Let solo players contribute meaningfully; scale goals for low population.

### Game-Kit Mapping

- factions;
- shared goals;
- inventory transfer;
- event log;
- notices;
- world ticks.

---

## 25. Pursuit: NPC Captains and Rival Memory

### Player Fantasy

The galaxy has other captains even when no humans are online.

### Screen: Local Contacts / Captains

```text
LOCAL SIGNALS — MERCY RELAY
────────────────────────────────────────────────────────────────────
Captains and contacts recently active here:

> Brass Jory              merchant rival       coolant route
  Moth-9                  salvage android      Blue Blind rumor
  Marshal Ivo Senn        Port Authority       customs patrol
  Sister Static           relay fragment       wants old signals

Selected: Brass Jory
Known for undercutting beginner routes and pretending it is charity.
Relationship: annoyed

────────────────────────────────────────────────────────────────────
(T)alk if docked  (H)istory  (J)obs linked  (Esc) back
```

### Screen: NPC History

```text
BRASS JORY — HISTORY

Recent:
- undercut coolant prices at Ash Coil
- warned you off the Mercy route
- lost a cargo sled near Blue Blind, allegedly

Memory:
- noticed your First Mercy Run
- considers you small but inconvenient
```

### UX Notes

- NPCs should be legible personalities, not invisible simulation math.
- Use notices and event log to surface their actions.
- Relationship states should be plain-language before numeric.

### Game-Kit Mapping

- world ticks;
- notices;
- event log;
- future relationship primitive;
- presence.

---

## 26. Pursuit: Bounties

### Player Fantasy

Someone wants a thing done badly enough to pay publicly.

### Screen: Bounty Detail

```text
BOUNTY — BLACK BOX AT BLUE BLIND
────────────────────────────────────────────────────────────────────
Issuer: Mercy Relay salvage desk
Target: DDN-44 "Choirless Bell" black box
Place:  Blue Blind wreck field
Reward: 500 credits + relay favor
State:  open
Expires: 2 days

Requirements:
- Board the derelict
- Recover black box
- Return it to Mercy Relay

Risks:
- unknown drone activity
- cargo space required

────────────────────────────────────────────────────────────────────
(C)laim  (M)ap target  (Esc) back
```

### UX Notes

- Bounties should feel more target-specific than Contracts.
- Show target, proof, reward, expiry, and risks.
- Claiming should not hide all competition unless the design says exclusive.

### Game-Kit Mapping

- bounties;
- job board aggregation;
- salvage;
- inventory;
- event log;
- notices.

---

## 27. Pursuit: Async Challenges

### Player Fantasy

Rivalry without simultaneous login.

### Screen: Challenge Offer

```text
CHALLENGE — BRASS JORY'S MERCY RUN
────────────────────────────────────────────────────────────────────
Brass Jory claims your route is beginner luck.

Challenge:
Earn more profit than Jory on the Ash Coil ⇄ Mercy Relay corridor
before tomorrow's tick.

Stake:
- 150 credits
- winner gets event-log bragging rights

Your current profit on route: 0
Jory's posted mark:        310

────────────────────────────────────────────────────────────────────
(A)ccept  (D)ecline  (Rules)  (Esc) back
```

### UX Notes

- Challenges must state how they resolve.
- Stakes must be clear.
- Players should not need the other player online.
- NPC challenges are excellent low-pop content.

### Game-Kit Mapping

- challenges;
- notices;
- leaderboards;
- market/travel metrics;
- world ticks.

---

## 28. Pursuit: Leaderboards

### Player Fantasy

The BBS remembers who mattered.

### Screen: Leaderboards

```text
LEADERBOARDS                              Season: Frontier Scramble
────────────────────────────────────────────────────────────────────
Board: Contracts Completed

Rank  Captain             Score       Note
 1    Brass Jory          12          loud about it
 2    Vanta               3           rising
 3    Moth-9              2           salvage only

Your rank: #2

Boards: (C)ontracts  (R)ichest  (S)alvage  (F)action  (W)anted
────────────────────────────────────────────────────────────────────
(←/→) board  (Esc) back
```

### UX Notes

- Show player rank even if not in top N.
- Include playful notes sparingly.
- Avoid leaderboards that punish new players too harshly; seasonal boards help.

### Game-Kit Mapping

- leaderboards;
- event log;
- season metadata later.

---

## 29. Pursuit: Shipyard and Modules

### Player Fantasy

The Ship becomes an expression of career.

### Screen: Ship Status

```text
RUSTBUCKET MULE                           Hull 82%  Hold 20
────────────────────────────────────────────────────────────────────
Stats
Cargo      20
Hull       82 / 100
Fuel       7
Stealth    poor
Scanner    basic

Installed Modules
> Patchwork Cargo Rack      +5 hold, ugly welds
  Basic Scanner             reveals common route hazards
  Empty Slot

Services at Ash Coil:
(R)epair hull    (I)nstall module    (B)uy hulls    (Esc) back
```

### UX Notes

- Compare modules clearly before purchase.
- Show what a module changes in plain language.
- Avoid giant stat screens; group by career relevance.

### Game-Kit Mapping

- inventory;
- market;
- capacity helper;
- prompts;
- per-captain/ship state.

---

## 30. Pursuit: Outposts and Colonies

### Player Fantasy

Stop just moving through the nebula. Own a piece of it.

### Screen: Outpost Overview

```text
OUTPOST — CINDER CACHE                    Owner: Vanta Charter
────────────────────────────────────────────────────────────────────
Status: rough dock
Security: low
Storage: 18/80
Production: none
Access: private

Stockpile
> Reactor Coolant x8
  Low-Grade Ore x30
  Med Gel x2

Projects
  Upgrade Dock        needs Ore 40, Credits 800
  Install Beacon      needs Relay Coil 1

────────────────────────────────────────────────────────────────────
(D)eposit  (W)ithdraw  (P)rojects  (A)ccess  (Esc) back
```

### UX Notes

- Outposts need a simple overview first, details second.
- Show storage, access, security, projects.
- Avoid colony spreadsheet syndrome in terminal UI.

### Game-Kit Mapping

- places;
- owner-keyed inventory;
- world ticks;
- shared goals/projects;
- contracts.

---

## 31. Pursuit: Corporations and Charters

### Player Fantasy

Even a solo captain can form a charter; multiple humans can build something bigger.

### Screen: Charter / Corporation

```text
VANTA CHARTER                             Members 1
────────────────────────────────────────────────────────────────────
Purpose: independent relief freight and salvage claims
Bank:    1,200 credits
Depots:  Cinder Cache
Standing: Mourning Union +4

Tasks
> Stock Cinder Cache with Reactor Coolant
  Scout Blue Blind route
  Pay dock license at Ash Coil

Permissions
Solo charter: all permissions held by founder.

────────────────────────────────────────────────────────────────────
(T)asks  (D)epots  (B)ank  (N)otices  (Esc) back
```

### UX Notes

- A Charter keeps corp-like systems usable on low-pop BBSes.
- Multi-member corporations can grow from the same UI.
- Permissions should be simple and visible.

### Game-Kit Mapping

- notices;
- inventory;
- leaderboards;
- possible future group primitive;
- factions/shared goals patterns.

---

## 32. Pursuit: Mines, Traps, and Route Control

### Player Fantasy

The route remembers what you left behind.

### Screen: Deployable Control

```text
ROUTE CONTROL — BLUE BLIND                Deployables: 2 active
────────────────────────────────────────────────────────────────────
Your deployables nearby:

> Sensor Ghost       Blue Blind -> Red Maw     expires 2d
  Decoy Wreck        Blue Blind                triggered 0 times

Route hazards known:
  Pirate pressure    moderate
  Mine density       unknown

Actions:
(D)eploy  (S)weep  (R)ecall hazard intel  (Esc) back
```

### UX Notes

- Area denial can become griefy; always show legality/safety constraints.
- Give counterplay: scan, sweep, avoid, bribe, use drones.
- Reports should arrive through Notices when traps trigger.

### Game-Kit Mapping

- inventory;
- routes/places;
- notices;
- event log;
- world ticks;
- bounded player text if labels allowed.

---

## 33. Pursuit: Admin and Diagnostics UX

### Player Fantasy

Not player fantasy: sysop trust and recovery.

### Screen: World Health, Sysop Only

```text
WORLD HEALTH — DROP DEAD NEBULA           sysop
────────────────────────────────────────────────────────────────────
World DB:          ok
Last tick:         14 minutes ago
Failed ticks:      0
Stuck Contracts:   0
Players stranded:  0
Markets empty:     1 warning

Warnings
> Mercy Relay Med Gel stock at zero for 3 days

Actions
(V)iew warning  (T)ick now  (E)xport summary  (Esc) back
```

### UX Notes

- Admin screens must be clearly marked.
- Destructive actions need confirmation.
- Prefer diagnostics before repair actions.
- Audit everything.

### Game-Kit Mapping

- role/security mapping;
- world DB;
- event log;
- admin diagnostics wishlist.

---

## 34. Navigation Model Across Full Game

### 34.1 Top-Level Dashboard Actions, Mature Game

```text
(T)ravel   (M)arket   (J)obs   (C)argo   (S)hip   (I)nbox
(F)action  (R)ecall   (L)og    (O)utpost (B)oards (H)elp
(Q)uit
```

### 34.2 Avoid Hotkey Overload

As the game grows, do not put every action on the dashboard. Use categories:

- Jobs contains Contracts, Bounties, Challenges, Faction Goals.
- Ship contains repair, modules, hulls.
- Recall contains star chart, notes, intel.
- Boards contains leaderboards and public news.
- Outpost appears only when relevant.

### 34.3 Back Behavior

- `Esc` always backs out unless a modal requires explicit choice.
- `Q` from safe top-level screens quits.
- Dangerous scenes use `R`eturn, `F`lee, or explicit options rather than silent `Esc` escape.

---

## 35. Visual Language

### 35.1 Symbols

Use a small, consistent symbol set:

```text
! warning / hazard / urgent
$ market / price / profit
? unknown / rumor / stale intel
@ person / NPC / player
+ new opportunity / gain
- loss / cost
✓ requirement met
✗ requirement missing
* unread / important
```

If Unicode rendering is uncertain, provide ASCII fallbacks:

```text
[!] [$] [?] [@] [+] [-] [OK] [NO] [*]
```

### 35.2 Tone

Copy should be:

- terse;
- flavorful;
- actionable;
- slightly grimy;
- never so cute that it obscures mechanics.

Good:

> Mercy Relay buys Med Gel above corridor average.

Also good:

> The med bay is paying panic prices.

Bad:

> A complex healthcare supply-demand modifier is active.

### 35.3 Color

Color may enhance but never carry meaning alone.

- warnings can be red/yellow but still use `!`;
- profit can be green but still use `$`;
- disabled choices should include reason text.

---

## 36. Screen Acceptance Checklist

Before any screen is considered ready, answer:

- Does it work at 80x24?
- Does it have an obvious back path?
- Does it explain disabled choices?
- Does it show the relevant current state?
- Does it avoid leaking implementation terminology?
- Does it give immediate feedback after action?
- Does it avoid requiring color?
- Does it avoid long unwrapped text?
- Does it use glossary terms consistently?
- Does it map to existing game-kit primitives where appropriate?

---

## 37. Recommended Screen Build Order

Design order:

1. Dashboard
2. Travel
3. Market
4. Jobs
5. Cargo
6. Event Log
7. Help
8. Title/Resume
9. Result/confirmation modals

Reason:

- Dashboard, Travel, Market, and Jobs define the core loop.
- Cargo and Log prove state visibility.
- Help and Title polish onboarding.
- Modals can be standardized once the main flows are understood.

---

## 38. Open UX Questions

1. Should the dashboard always show Signals, or only when there are meaningful items?
2. Should the Market default to buy/sell table or separate buy and sell tabs?
3. Should Contract completion consume Med Gel directly or accept a prior sale as proof?
4. Should the Job Board show future locked systems in MVP, or hide them entirely?
5. Should Travel show unreachable known Places, or only outbound Routes?
6. How much flavor text is too much for returning players?
7. Should first-run guidance disappear after Contract completion?
8. Should Event Log default to personal Events or global Events once multiplayer begins?
9. Should Star Chart be available in MVP or wait until Recall matters more?
10. Should the dashboard include Ship hull/fuel in MVP if those systems are not active?

---

## 39. UX Recommendation for the MVP

For the first playable version, optimize for clarity over atmosphere:

- show Signals on Dashboard;
- make Jobs explicitly point to Med Gel and Mercy Relay;
- make Market notes explain why Med Gel matters;
- make Travel route notes remind the player of Contract destination;
- make Contract completion produce a satisfying modal;
- make Log visibly record success;
- hide or clearly mark unbuilt future systems.

The MVP succeeds if a new caller can complete First Mercy Run without help and still glimpse the bigger dream through names like Blue Blind, Saint Vex Drift, Red Maw Approach, and Dead Gate Verge.
