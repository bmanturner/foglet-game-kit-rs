# Drop Dead Nebula — Admin, Operations, Diagnostics, Backup, and Recovery Deep Dive

Status: Draft v0.1  
Primary milestone relevance: design awareness M1, full operations M12  
Purpose: Define how a sysop can run, inspect, recover, and maintain a shared-world BBS door game.

---

## 1. Design Intent

Drop Dead Nebula should be fun to operate, not just fun to play.

Operations should:

- protect the shared world DB;
- expose health and stuck state;
- support recovery without direct DB surgery;
- make ticks and jobs inspectable;
- preserve audit trails;
- keep destructive actions explicit.

Operations should not:

- require sysops to understand every game rule;
- hide failed ticks/jobs until players complain;
- mutate state without audit;
- make admin powers look like normal gameplay.

---

## 2. Milestone Placement

| Milestone | Ops Role |
| --- | --- |
| M1 | Design/test awareness; no full admin UI. |
| M2 | Tick diagnostics become relevant. |
| M5 | Job repair/expiry diagnostics become relevant. |
| M8 | Social safety and Notice inspection matter. |
| M9–M10 | Outpost/corp recovery matters. |
| M12 | Admin/diagnostic tools become a milestone focus. |

---

## 3. Admin Surfaces

### 3.1 CLI / Maintenance Command

Best for:

- backups;
- world summaries;
- tick now;
- export diagnostics;
- repair dry-run.

### 3.2 In-Door Sysop Screen

Best for:

- quick health view;
- stuck player rescue;
- inspecting recent failures;
- non-destructive diagnostics.

### 3.3 Direct DB Access

Should be last resort, not normal operation.

---

## 4. World Health Checks

Health screen should report:

- world DB reachable;
- last successful tick;
- failed tick count;
- stuck running tasks;
- job rows in impossible states;
- players with invalid Presence;
- Markets with zero critical stock;
- outposts under-supplied;
- oversized Notice queues;
- recent admin actions.

Example:

```text
WORLD HEALTH — DROP DEAD NEBULA
DB: ok       Last tick: 14m ago       Failed ticks: 0
Warnings: 1 market empty, 0 stuck players, 2 expired jobs pending
```

---

## 5. Recovery Actions

Potential actions:

- rescue stuck Captain to safe Place;
- rerun failed tick;
- expire broken Contract;
- regenerate local jobs;
- restock critical Market minimally;
- archive spammy Notices;
- unlock stuck Presence;
- pause problematic deployable type;
- export world summary.

Rules:

- prefer dry-run first;
- require confirmation for mutation;
- append audit Event or admin log;
- explain player-facing consequences.

---

## 6. Backup and Restore

Design-level guidance:

- backup world DB when door is stopped or through safe backup API;
- include content seed/config with backup metadata;
- record game version/build metadata;
- document restore process;
- verify backup integrity.

Player saves and shared world DB must both be considered.

---

## 7. Tick Operations

Admin should see:

- due tick tasks;
- last run;
- next due;
- failure reason;
- retry status;
- catch-up backlog;
- digest count.

Actions:

- run due ticks once;
- disable noncritical tick temporarily;
- clear deadletter only with care;
- export tick report.

---

## 8. Social Safety Operations

Once player text exists:

- inspect reported message;
- archive/hide message;
- mute/block if supported;
- disable player mail/listing notes globally;
- export audit.

Do not require social tooling before free text launches, but do not launch free text without some safety posture.

---

## 9. Season Operations

M11+:

- start season;
- pause season;
- advance campaign phase manually if necessary;
- export season chronicle;
- end season;
- reset/partial reset if supported;
- preserve legacy records.

---

## 10. Admin UX Rules

- Mark sysop screens clearly.
- Diagnostics first, mutation second.
- Dangerous actions require confirmation.
- Avoid showing secrets from Foglet or host environment.
- Do not print logs to active TUI stdout.
- Keep admin actions auditable.

---

## 11. Recommended First Ops Slice

M12:

- world health summary;
- tick status;
- stuck player rescue;
- broken job listing;
- export summary;
- backup/restore documentation.

Earlier milestones should at least produce enough Events/errors to diagnose manually.

---

## 12. Open Questions

1. Should admin tools be CLI-first or in-door-first?
2. What admin actions are safe enough for in-door UI?
3. How are admin actions audited?
4. Should sysop role from Foglet unlock diagnostics automatically?
5. How much world repair should be automated?
6. How should backups handle per-user saves plus shared DB?
