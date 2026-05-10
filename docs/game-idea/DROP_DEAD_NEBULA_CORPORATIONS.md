# Drop Dead Nebula — Corporations and Charters Deep Dive

Status: Draft v0.1  
Primary milestone relevance: seeded M8, playable M10, campaign relevance M11  
Purpose: Define solo Charters, multiplayer Corporations, shared resources, permissions, corp tasks, and low-pop social infrastructure.

---

## 1. Design Intent

Corporations and Charters let players create durable identity beyond a single Captain.

They should:

- support shared goals without requiring realtime co-presence;
- work for one active player through solo Charters;
- give groups shared depots, bank, tasks, and outposts;
- create async collaboration and competition;
- integrate with Factions, Markets, Jobs, Outposts, Notices, and Leaderboards.

They should not:

- lock core progression behind multiplayer population;
- require complicated governance early;
- enable easy griefing/theft through unclear permissions;
- become a spreadsheet guild simulator before the economy supports it.

---

## 2. Terms

### Charter

A solo-compatible legal shell for one Captain and their NPC/back-office support.

Purpose:

- gives solo players access to corp-like systems;
- can later become a Corporation if more players join;
- avoids locking Outposts/depots behind population.

### Corporation

A multi-Captain organization with shared resources and permissions.

### Member

A player Captain in a Corporation or Charter.

### Role

Permission bundle inside a Corporation.

### Depot

Shared stockpile owned by Charter/Corporation.

### Corp Task

Internal job/request visible to members.

---

## 3. Milestone Placement

| Milestone | Corp/Charter Role |
| --- | --- |
| M0–M7 | Flavor only; do not build. |
| M8 | Seed NPC corp references and async social hooks. |
| M9 | Outposts/depots create infrastructure corps can later own. |
| M10 | Solo Charters and Corporations become playable. |
| M11 | Corps/Charters influence campaign logistics and Faction fronts. |
| M12 | Permissions, admin tools, and recovery are hardened. |

---

## 4. Why Charters Come First

A low-pop BBS cannot assume group play. The Charter is the solo version of durable organization.

A Charter can have:

- name;
- founder Captain;
- bank;
- depot;
- task board;
- outpost ownership;
- Faction alignment;
- Notices/reports;
- Leaderboard entry.

A Corporation is a Charter with multiple members and permissions.

This avoids duplicate systems.

---

## 5. Core Player-Facing Uses

### 5.1 Shared Bank

Stores credits for group/charter projects.

Use cases:

- outpost upgrades;
- shipyard discounts;
- shared goal contributions;
- bounty funding;
- emergency repair fund.

### 5.2 Depots

Owner-keyed inventory stockpiles.

Use cases:

- store commodities outside Ship;
- stage goods near faction goals;
- prepare for market shifts;
- supply other members;
- stock outposts.

### 5.3 Corp Task Board

Internal Job Board.

Examples:

- “Bring 20 Reactor Coolant to Cinder Cache.”
- “Scout Blue Blind after next tick.”
- “Sell surplus Ore at Ash Coil.”
- “Contribute Relay Coil to Mercy Repair Fund.”

### 5.4 Corp Notices

Async coordination:

- depot changed;
- member completed task;
- outpost attacked;
- market opportunity;
- Faction status changed.

### 5.5 Corp-Owned Outposts

Outposts can belong to Charter/Corp rather than one Captain.

---

## 6. Permission Model

Keep permissions simple at first.

### 6.1 Suggested Roles

**Founder**

- all permissions;
- cannot be removed except by admin/recovery policy.

**Officer**

- manage tasks;
- withdraw limited depot stock;
- spend bank within limit;
- invite members if enabled.

**Member**

- view corp screens;
- deposit goods;
- claim tasks;
- withdraw from public/member stock if allowed.

**Applicant / Guest**

- optional later role;
- minimal access.

### 6.2 Permission Categories

- view bank;
- spend bank;
- deposit inventory;
- withdraw inventory;
- manage depot access;
- create internal tasks;
- complete internal tasks;
- manage outpost projects;
- invite/remove members;
- change Faction alignment;
- post corp Notices.

### 6.3 Safety Rules

- Deposits should be easy.
- Withdrawals should be permissioned and logged.
- Bank spending should be capped or role-gated.
- Destructive changes require confirmation.
- All shared-resource changes create audit Events or corp log entries.

---

## 7. Corp Task Board

Corp tasks should reuse Job Board UX but remain internal.

Task states:

```text
open -> claimed -> completed
open -> cancelled
claimed -> abandoned
claimed -> expired optional
```

Task types:

- supply depot;
- move cargo;
- scout Place;
- clear hazard;
- contribute to Faction goal;
- stock Market listing;
- upgrade Outpost.

Rewards:

- corp reputation;
- credits from bank;
- title/recognition;
- share of profits;
- no reward, if purely cooperative.

---

## 8. Corporation and Faction Relationship

Corporations can align with Factions, but this should be reversible or at least carefully messaged.

Effects:

- access to faction jobs;
- price modifiers;
- outpost protection;
- faction enemies;
- campaign influence.

Avoid early hard locks. Use standing and alignment bands first.

---

## 9. Low-Pop Design

If only one human plays:

- Charter gives access to corp infrastructure;
- NPC workers can provide flavor reports;
- internal tasks become personal logistics reminders;
- Faction shared goals remain viable;
- corp leaderboards can include NPC/Charter entries if useful.

If population grows:

- same screens support multiple members;
- permissions become more important;
- corp competitions and leaderboards matter.

---

## 10. UX Screens

### Charter Overview

```text
VANTA CHARTER                             Members 1
Bank: 1,200 credits       Faction: Mourning Union +4
Depot: Cinder Cache       Tasks: 3 open

(T)asks  (D)epots  (B)ank  (O)utposts  (N)otices  (Esc) back
```

### Corp Depot

```text
CINDER CACHE DEPOT                         Access: members
Item                 Qty    Reserved   Note
> Reactor Coolant     18       10      Relay repair
  Low-Grade Ore       40        0      upgrade stock
  Med Gel              3        0      emergency

(D)eposit  (W)ithdraw  (R)eserve  (Esc) back
```

### Corp Task Detail

```text
TASK: Supply Relay Repair
Need: Reactor Coolant x10 at Mercy Relay
Reward: 120 credits from corp bank
State: open

(C)laim  (D)etails  (Esc) back
```

---

## 11. Integration Points

- **Inventory:** depots and bank-like resources.
- **Outposts:** corp-owned infrastructure.
- **Notices:** reports and coordination.
- **Contracts/Jobs:** internal task board.
- **Factions:** alignment and shared goals.
- **Leaderboards:** corp wealth, contribution, route control.
- **World Ticks:** production, upkeep, reports.
- **Admin:** recover abandoned corp/outpost state.

---

## 12. Recommended First Corp Slice

M10:

- Solo Charter creation;
- one Charter depot;
- deposit/withdraw with founder-only permissions;
- simple Charter task board;
- Charter-owned Outpost link if M9 exists;
- corp/charter Notice report.

Defer:

- multi-role complexity;
- player invitations;
- corp warfare;
- complex bank permissions;
- taxes/dividends.

---

## 13. Open Questions

1. Can a Captain belong to multiple Corporations?
2. Can a Charter become a Corporation without migration pain?
3. Should corp bank use credits field or ledger-like entries?
4. How much theft/risk should be allowed between members?
5. Can abandoned corps decay or be archived?
6. Should NPC corps exist as competitors from the start?
7. Should corp names require moderation/sanitization?
