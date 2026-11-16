# Sen P0-4 — Bản tin sáng + hộp thư chủ động

Status: PRODUCER BUNDLE, 16/11/2026. Verified independently by
the seat (not the producer subagent) on 16/11: `npm test` 165 pass,
vite build green, plus an independent gate probe (see Evidence).
No merge, no push, no PR.

## OBSERVABLE_SUCCESS

- Each morning a brief is built from email + calendar data:
  real overlaps are listed with minutes overlapped, events that
  only touch (one ends exactly when the next starts) are not
  conflicts, deadlines inside the 7-day window are listed in time
  order, and emails needing a decision are listed (flagged, or
  matching the explicit marker list in code).
- Every decision email produces exactly one proposal, with a
  deterministic id (`proposal:<emailId>`) and idempotency key,
  default action `reply`, and a summary that says plainly the user
  must approve first.
- Applying a proposal goes through the P0-2 gate: with no allow
  rule it throws `TOOL_POLICY_DENIED`; with a rule but no approval
  it throws `APPROVAL_REQUIRED`; a never-tier tool throws
  `APPROVAL_NEVER_DENIED` even with a rule and an approval. Each
  evaluation is recorded in the ActivityLog exactly once.
- A gate pass still executes nothing: it returns
  `{ executed: false, reason: "EXECUTION_NOT_WIRED" }`, because
  there is no Gmail sender or calendar writer wired yet. No code
  path in P0-4 ever returns `executed: true`.
- The `ignore` action needs no gate and writes no log entry: it
  only marks the proposal resolved locally.
- The brief UI renders email/calendar text escaped: a hostile
  subject cannot inject markup.

## Design

The brief only reads; a proposal never runs by itself. All
proactive content is read-only until the user approves it, and
even then the action is not executed in this phase — approval
grants permission, execution is a later phase.

### Where the logic lives

- `packages/core/src/morning-brief.ts` — pure TS: types,
  `validateEmails` / `validateEvents` (error lists, same style as
  `validateToolPolicy` / `validateRouterConfig`),
  `buildMorningBrief`, `renderMorningBriefText`. Exported from
  `packages/core/src/index.ts`.
- `packages/core/src/proactive-inbox.ts` — pure TS: proposal
  type, `proposeDecisions`, `applyDecision` (gate via
  `gateToolCall` from `approval-gate.ts`). Exported from
  `packages/core/src/index.ts`.
- `apps/desktop/src/app/morning-brief-ui.ts` — render a brief as
  an HTML string with every value escaped. NOT wired into
  `apps/desktop/src/app/main.ts` in P0-4; wiring is a follow-up
  after review.

### The brief

`buildMorningBrief({ emails, events, now, windowDays = 7 })`
throws FAIL-loud on invalid input (bad ISO dates, empty ids,
duplicate ids, `start >= end`, negative `windowDays`) instead of
building a wrong brief. Dates are ISO strings compared in UTC;
"today" is the UTC calendar day of `now`; the window is
`[now, now + windowDays days]`, both ends inclusive. Output is
deterministic: every list is sorted by time, then id.

- Conflicts: pairs of events with overlap > 0 minutes, with the
  overlap in minutes and the overlap window. Touching events are
  not conflicts.
- Deadlines: an email `dueAt`, or an event `start`, inside the
  window; `{ kind, id, label, at }`, sorted by time then id.
- needsDecision: emails with `needsDecision: true`, or whose
  subject + body matches `DECISION_MARKERS` — an explicit, closed
  list in code (Vietnamese: duyệt, phê duyệt, quyết định, quyết,
  xác nhận, xin ý kiến, cho ý kiến, đồng ý, chốt, cần bạn quyết;
  English: approve, approval, confirm, decision, decide,
  sign off, action required, please review). Nothing outside the
  list counts, so a plain informational email never becomes a
  decision item.
- todayEvents: events starting on the day of `now`.
- unreadCount: the number of emails handed in. The input list is
  the unread inbox snapshot, so there is no separate read flag;
  the count is exactly the list length, nothing more.
- `renderMorningBriefText` renders plain text with real items
  only; empty sections say so in plain words, never a placeholder.

### Proposals and the approval gate

| Action | Tool | Tier (P0-2 tables) |
| --- | --- | --- |
| `reply` | `send_email` | ask-first: needs an allow rule + an approval |
| `draft-reply` | `draft_email` | auto: needs an allow rule only |
| `archive` | `archive_email` | unclassified, therefore ask-first (fail closed) |
| `schedule` | `create_calendar_event` | unclassified, therefore ask-first (fail closed) |
| `ignore` | (no tool) | no gate, no log entry, local mark only |

`proposeDecisions(brief, emails)` makes exactly one proposal per
email in `brief.needsDecision`, default action `reply`. The
summary names the real subject and sender and says the user must
approve; choosing a different action is the user's call at
approval time, not something the proposer decides.

`applyDecision({ policy, approvals, log, proposal, actor })`:

1. `ignore`: sets `proposal.status` to `"executed"` (the local
   mark only — no tool ran) and returns
   `{ executed: false, reason: "IGNORED_LOCAL_ONLY" }`. No gate
   call, no log entry.
2. Any other action: evaluates through `gateToolCall` (logged
   exactly once). A non-allow decision throws with the P0-2 code
   (`APPROVAL_REQUIRED` / `TOOL_POLICY_DENIED` /
   `APPROVAL_NEVER_DENIED`) and the proposal stays `"pending"`,
   so the user can approve and retry.
3. An allow decision sets `proposal.status` to `"approved"` and
   returns `{ executed: false, reason: "EXECUTION_NOT_WIRED",
   evaluation }`. Nothing was sent, drafted, archived, or
   scheduled.

### No design deviation

None. The build follows the assigned design exactly: read-only
brief, gate-mandatory apply, `EXECUTION_NOT_WIRED`, no wiring
into `main.ts`, no new dependency, no network, no key, no paid
model call anywhere.

### Follow-ups (not this bundle)

- Gmail / calendar fetchers feeding real `EmailItem` /
  `CalendarEvent` data into `buildMorningBrief`.
- Real executors for `send_email`, `draft_email`,
  `archive_email`, `create_calendar_event` — only then can a gate
  pass return `executed: true`, and only behind the same gate.
- A Rust mirror of the brief/proposal logic, like the P0-2/P0-3
  mirrors, if the backend needs it.
- Wiring `renderMorningBriefHtml` into `main.ts` and an approval
  UI for proposals.
- Durable storage for the brief and the proposals (in-memory
  only in this phase, like the P0-2 log and approvals).

## Verification skill — morning brief + proactive inbox

Feature map: entry `packages/core/src/morning-brief.ts` +
`packages/core/src/proactive-inbox.ts` +
`apps/desktop/src/app/morning-brief-ui.ts` | actions: build a
brief, propose decisions, apply a proposal with/without policy
and approval, ignore a proposal, render the brief as text/HTML |
observable: conflicts/deadlines/decisions in the brief, the
proposal ids, the thrown error code, the log entries in seq
order, `{ executed: false, reason: "EXECUTION_NOT_WIRED" }`,
escaped HTML.

### 1. LAUNCH

- Worktree `~/workspace/sen-p04-morning-brief`, branch
  `feature/sen-morning-brief`, base HEAD `05f71c1b786d29880b44c5551e19b30177efaa16`
  (P0-2 approval gate, PR #71).
- `git log -1` at run time: see Evidence.
- Build/run: `npm test` (node 24, type stripping, no build step);
  `npm run build --prefix apps/desktop` (vite).

### 2. DOCTOR

- Linux, node v24.22.1, no Rust system libs (no Rust touched in
  this bundle — pure TS + a render-only frontend module).
- Fail early when: HEAD is not the P0-4 base, `node_modules`
  missing.

### 3. DRIVE

| # | Command | Result |
| --- | --- | --- |
| 1 | `npm test` (baseline, before changes) | 136 pass, 0 fail |
| 2 | `npm test` (after changes) | 165 pass, 0 fail |
| 3 | `node --test packages/core/tests/morning-brief.test.ts packages/core/tests/proactive-inbox.test.ts apps/desktop/tests/morning-brief-ui.test.ts` | 29 pass, 0 fail |
| 4 | `npm run build --prefix apps/desktop` | vite build green, 21.80 kB js |

### 4. EVIDENCE

- TS suite: 165 pass / 0 fail (136 baseline + 29 new: 13 in
  `packages/core/tests/morning-brief.test.ts`, 12 in
  `packages/core/tests/proactive-inbox.test.ts`, 4 in
  `apps/desktop/tests/morning-brief-ui.test.ts`).
- `git log -1` at delivery: base HEAD `05f71c1` (P0-2 merge);
  the P0-4 change is uncommitted on branch
  `feature/sen-morning-brief` (producer bundle, no commit).
- `git diff --check`: clean (checked at bundle time).
- Files changed/created (8 files, `main.ts` untouched):
  - created `packages/core/src/morning-brief.ts`
  - created `packages/core/src/proactive-inbox.ts`
  - created `packages/core/tests/morning-brief.test.ts`
  - created `packages/core/tests/proactive-inbox.test.ts`
  - created `apps/desktop/src/app/morning-brief-ui.ts`
  - created `apps/desktop/tests/morning-brief-ui.test.ts`
  - modified `packages/core/src/index.ts` (exports only)

### 5. CLEANUP

- Kept: the worktree (branch `feature/sen-morning-brief`,
  uncommitted changes).
- Removed nothing from the repo. `apps/desktop/node_modules`
  and `apps/desktop/dist` are git-ignored build outputs, left in
  place.
- Evidence still readable after cleanup: this file + the test
  files themselves.

## NOT RUN (must run before merge)

- Rust mirror + `cargo test` in `apps/desktop/src-tauri`:
  NOT RUN — no Rust code was touched in this bundle (pure TS),
  so there is no mirror to test; a mirror is a follow-up, and
  `cargo test` itself is BLOCKED on this box (gdk-3.0 missing).
  Run on the Leader VM when a mirror lands.
- `cargo clippy --workspace --all-targets`: NOT RUN, same
  blocker (no Rust touched).
- Test PC (Windows): NOT RUN — box is Linux only.
- CI: NOT RUN — no PR opened (producer bundle, no push).
- Live Gmail/calendar fetch: NOT RUN — no fetcher is wired yet;
  the brief is proven only against explicit test fixtures.
- Real tool execution (send/draft/archive/schedule): NOT RUN —
  a gate pass still ends in `EXECUTION_NOT_WIRED` by design.
- Independent verification (producer ≠ verifier): DONE by the
  seat on 16/11, after the producer delivery — `npm test` re-run
  165 pass / 0 fail, vite build re-run green, and an independent
  probe (`/tmp/p04-verify-probe.mjs`, not a producer test) confirmed
  end-to-end: 1 conflict of 30 min (touching events excluded),
  needsDecision only for the "duyệt" email, proposal
  `proposal:e1` / `send_email`, apply without approval throws
  `APPROVAL_REQUIRED` and stays `pending`, apply with approval
  returns `{ executed: false, reason: "EXECUTION_NOT_WIRED" }`,
  never-tier `trade_securities` throws `APPROVAL_NEVER_DENIED`
  despite allow rule + approval, ActivityLog holds 3 entries.

## Handoff

| Check | Verdict | Evidence |
| --- | --- | --- |
| `npm test` (TS, incl. 29 new) | PASS | 165 pass / 0 fail |
| `npm run build --prefix apps/desktop` | PASS | vite build green, 21.80 kB js |
| Rust mirror + `cargo test` in src-tauri | NOT RUN | no Rust touched; gdk-3.0 missing on this box |
| `cargo clippy` | NOT RUN | same blocker |
| Test PC (Windows) | NOT RUN | box is Linux only |
| CI | NOT RUN | no PR opened |
| Live Gmail/calendar fetch | NOT RUN | no fetcher wired; fixtures only |
| Real tool execution | NOT RUN | gate pass still ends in EXECUTION_NOT_WIRED by design |
| Independent review | PASS | seat re-ran `npm test` 165/165 + vite build green + independent gate probe (see Evidence) |

Risks left: the marker list is a heuristic — an email can match
a marker without truly needing a decision (e.g. "please review"
in a FYI), or need one without matching any marker; the flag
`needsDecision: true` from the fetcher is the reliable path and
the marker list is only a fallback. `archive_email` and
`create_calendar_event` are unclassified and therefore ask-first;
if they are later classified auto, their proposals would pass
with a rule alone — that reclassification needs its own review.
The brief compares dates in UTC; a user in another timezone
could see "today" shift at the day boundary — a timezone-aware
day is a follow-up, not a silent fix.
