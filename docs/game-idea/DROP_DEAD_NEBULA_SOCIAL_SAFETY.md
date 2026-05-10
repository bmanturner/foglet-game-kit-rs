# Drop Dead Nebula — Player Text, Social Safety, and Moderation Deep Dive

Status: Draft v0.1  
Primary milestone relevance: Notices M2, bounded player mail M8, corps/listings M10, operations M12  
Purpose: Define safe handling of player-authored text and asynchronous social systems.

---

## 1. Design Intent

Player text can make a BBS world feel alive, but it must be bounded and sysop-friendly.

Social features should:

- support async collaboration and rivalry;
- keep text short and terminal-safe;
- avoid moderation burden where possible;
- provide clear ownership and reporting/audit trails;
- work without realtime chat.

They should not:

- become an unmoderated chat platform;
- allow terminal escape/control abuse;
- require rich formatting;
- block core gameplay if disabled.

---

## 2. Player-Authored Text Surfaces

Potential surfaces:

- player mail;
- bounty blurbs;
- challenge messages;
- corp bulletins;
- outpost names/mottos;
- dead drop labels;
- market listing notes;
- ship names/epitaphs;
- rumors if enabled.

---

## 3. Milestone Placement

| Milestone | Social/Text Role |
| --- | --- |
| M0 | Ship/Captain display names may exist but can be simple. |
| M2 | System Notices, no free player text required. |
| M5 | Job/Bounty text mostly system-authored. |
| M8 | Player mail/Challenges may introduce bounded text. |
| M9 | Dead drops/outpost labels may need text rules. |
| M10 | Corp bulletins and listings need sanitization. |
| M12 | Admin/moderation tools and policies mature. |

---

## 4. Text Safety Rules

All player-authored text should be:

- length-bounded;
- line-bounded;
- terminal-control sanitized;
- safe to wrap;
- stored with author and timestamp;
- optionally archivable/hidden by recipient;
- never required for mechanical completion unless system-authored fallback exists.

No raw ANSI/control sequences.

---

## 5. Suggested Limits

Initial conservative limits:

- ship name: 32 chars;
- Captain display alias if allowed: 32 chars;
- mail subject: 60 chars;
- mail body: 500–1000 chars;
- bounty/challenge note: 240 chars;
- corp bulletin: 1000 chars;
- outpost motto: 120 chars;
- market listing note: 160 chars.

These are design suggestions, not implementation constants.

---

## 6. Social Safety UX

Inbox actions:

- read;
- archive;
- mark unread;
- block/mute later if needed;
- report later if supported.

Text entry should show:

- remaining characters;
- validation errors;
- preview if formatting/wrapping changes.

---

## 7. Moderation Posture

MVP can avoid free player text. Add it only when:

- sanitizer exists;
- sysop can inspect/report;
- text is not required for play;
- abuse can be mitigated by archive/block/report or disabling the feature.

Sysop options later:

- disable player mail;
- disable player listing notes;
- require pre-authored ship names only;
- purge/hide specific messages;
- export audit report.

---

## 8. Async Social Design

Prefer structured social actions before free text:

- challenge templates;
- preset bounty types;
- corp task templates;
- reaction choices;
- faction contribution messages;
- system-generated summaries.

Free text can add flavor later.

---

## 9. Integration Points

- **Notices:** player mail and reports.
- **Challenges:** optional taunts/messages.
- **Bounties:** player-authored blurbs if allowed.
- **Corporations:** bulletins and tasks.
- **Outposts:** names/mottos/access notes.
- **Market listings:** optional notes.
- **Admin:** inspection and moderation.

---

## 10. Recommended First Social Slice

M8:

- system-authored NPC/player Challenge notices;
- optional short player reply templates, not free text;
- bounded player mail only if sanitizer/admin posture is ready.

M10:

- corp bulletins/listing notes after moderation rules are established.

---

## 11. Open Questions

1. Should player-to-player mail be enabled by default?
2. Should sysops be able to disable all free text?
3. Do we need block/mute before player mail?
4. Should ship/outpost names be moderated?
5. Should reports be in-game or sysop-side only?
6. How much text history is retained?
