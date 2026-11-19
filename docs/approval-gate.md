# Sen P0-2 — Cổng phê duyệt 3 tầng + nhật ký hoạt động

Status: MERGED into dev/scaffold via PR #71 (Sen P0-2 rev1), latest
update PR #100 (TS P1-4 rev2), head 780d257. Verified ĐẠT by SD Leader
on 16/11/2026.

## OBSERVABLE_SUCCESS

- A read-only or draft tool (`read_file`, `draft_message`, ...) runs
  through the gate at once once the policy allows it by name; with
  no allow rule it is refused (default-deny).
- A send / buy / sign / delete tool (`send_message`, `purchase`,
  `sign`, `delete_file`, ...) is refused with `APPROVAL_REQUIRED`
  until an approval is granted for that exact tool + idempotency
  key; after `sen_tool_approve` it passes the gate. It never runs
  on the sly while waiting.
- An approval belongs to one session: granted for session A, it
  never authorizes a call in session B, even with the same tool +
  idempotency key. The session id is the live session's id (the
  CLI `--session-id` UUID / SessionManager key), never invented
  by the gate.
- A grant can be taken back: `sen_tool_revoke` (frontend
  `revokeTool`) removes one approval, true when a live grant was
  removed, false when there was none. The next call with that
  tool + key waits for a fresh approval. Revoking never touches
  the activity log.
- A forbidden tool (securities trading, transfer between accounts,
  access to another account) is always refused with
  `APPROVAL_NEVER_DENIED`, even with an allow rule AND an approval.
- An unclassified tool is ask-first, never auto: it waits for an
  approval like any tier-2 tool.
- Every evaluation — allowed, denied, awaiting-approval — is
  recorded in the activity log with seq, timestamp, actor, tool,
  tier, decision, reason, and result; `sen_tool_activity_log`
  reads it back, oldest first. The log has no delete or update
  path: entries only ever get appended.
- The policy starts empty: with no rule at all, every tool is
  refused.

## Design

The gate decides permission; it does not execute anything. A call
that passes the gate still ends in `TOOL_EXECUTION_NOT_WIRED` on
the backend, because real tool execution is a later phase — and
that is honest: permission granted is not execution. The 6-step
execution gate order (`packages/core/src/execution-gate.ts`) is
unchanged and still asserted before the approval gate runs.

Evaluation order, fixed (same on both sides):

1. Tier `never` → deny (`APPROVAL_NEVER_DENIED`). No approval and
   no policy rule can override this.
2. Policy has no allow rule for the tool → deny
   (`TOOL_POLICY_DENIED`). Default-deny: only an explicit rule
   allows a tool.
3. Tier `ask-first` with no matching approval → ask-first
   (`APPROVAL_REQUIRED`). The approval must match the exact tool +
   idempotency key + session; a different key is a different call,
   and a grant from another session is no grant at all.
4. Otherwise → allow.

### The three tiers

| Tier | Tools | Behaviour |
| --- | --- | --- |
| 1 `auto` | read-only + drafts: `read_file`, `read_status`, `list_files`, `list_sessions`, `search_files`, `search_web`, `read_memory`, `get_status`, `draft_message`, `draft_email`, `draft_post`, `draft_note` | Runs at once when the policy allows it by name. |
| 2 `ask-first` | send / buy / sign / delete: `send_message`, `send_email`, `send_telegram`, `publish_post`, `purchase`, `buy`, `pay`, `sign`, `sign_document`, `delete_file`, `delete_message`, `delete_session`, `delete_note` | Must ask and wait for an explicit approval; without one it is refused, never run on the sly. |
| 3 `never` | `trade_securities`, `buy_stock`, `sell_stock`, `stock_trade`, `transfer_between_accounts`, `transfer_money`, `access_other_account`, `impersonate_account`, `login_as_other_user` | Always refused. No approval path exists. |

Unclassified tools (for example `edit_file`, or any new tool name)
are `ask-first`: they fail closed until someone classifies them
and writes an explicit allow rule.

### Where the logic lives

- `packages/core/src/approval-gate.ts` — pure TS: tier tables,
  classification, default-deny policy (`ToolPolicy`, empty by
  default), approval store, append-only activity log, and the
  `evaluateToolCall` / `gateToolCall` entry points. Exported from
  `packages/core/src/index.ts`.
- `apps/desktop/src-tauri/src/commands/approval_gate.rs` — pure
  Rust mirror (same tables, same order, same reasons). The two
  must stay in sync.
- `apps/desktop/src-tauri/src/commands/registry.rs` — the
  enforcing side: `Registry` holds the policy (starts empty),
  the approvals, and the activity log; `tool_call()` validates
  the contract, rejects a contract with no session id
  (fail closed, `INVALID_CONTRACT`), asserts the 6-step gate
  order, runs the approval gate scoped to the contract's session,
  records the evaluation, then refuses (`APPROVAL_REQUIRED` /
  `APPROVAL_NEVER_DENIED` / `TOOL_POLICY_DENIED`) or — when the
  gate passes — fails with `TOOL_EXECUTION_NOT_WIRED`, because
  execution is not filled in yet. `allow_tool()` adds a rule,
  `approve_tool()` grants an approval in one session (empty
  session id fails closed), `revoke_tool()` removes one in one
  session (bool: a live grant was removed), `activity_log()` /
  `list_approvals()` read the state back (clones). The Rust
  grant carries `expiresAtSecs` (granted-at + 900 s) and
  `sessionId`, and is spent by `gate_tool_call` on the first
  allow, exactly like the TS side (`APPROVAL_TTL_MS`,
  `consume()`). The store key is session + tool + key, so the
  same tool + key in another session is a different grant.
- `apps/desktop/src-tauri/src/commands/tools.rs` — thin Tauri
  wrappers: `sen_tool_call` (contract now carries `sessionId`,
  the live session's id), `sen_tool_approve` (contract +
  `{ toolName, idempotencyKey, grantedBy }` under `args`, session
  from the contract), `sen_tool_revoke` (contract +
  `{ toolName, idempotencyKey }` under `args`, session from the
  contract, returns whether a live grant was removed),
  `sen_tool_activity_log` (contract, returns the entries).
  Registered in `main.rs`.
- `packages/tools/src/index.ts` — the `tool_call` event hook is
  filled in: it runs the same gate (contract checks first, then
  tier + policy + approval), records the evaluation in the log
  when one is passed in, throws FAIL-loud on refusal, and returns
  the evaluation on allow. A bare hook call (no policy passed)
  uses the default-deny policy, so it refuses everything.
- Frontend: `apps/desktop/src/ipc/tools-client.ts`
  (`approveTool`, `revokeTool`, `getActivityLog`, all validated
  like the other clients; no key field ever travels). The
  frontend `Approval` carries `expiresAtSecs` (epoch seconds,
  the Rust grant's expiry) and `sessionId` (the live session
  the grant belongs to; an approval without one is malformed).
  The contract builders in `apps/desktop/src/app/main.ts`
  (`appLiveContract`, `chatContract`) set `sessionId` from the
  session they were built for, so every contract the app sends
  already carries the live session's id.

### The activity log

One entry per evaluation, exactly once each:

| Field | Meaning |
| --- | --- |
| `seq` | Monotonic, never reused; the ordering authority. |
| `timestamp` | TS: ISO string. Rust: `epoch-<seconds>` (no chrono in the crate). |
| `actor` | Who asked for the tool (contract actor). |
| `tool` | Tool name. |
| `tier` | `auto` / `ask-first` / `never`. |
| `decision` | `allow` / `ask-first` / `deny`. |
| `reason` | Human-readable reason, same wording both sides. |
| `result` | `allowed` / `awaiting-approval` / `denied`. |

Append-only in execution: there is no delete, no update, and no
clear API on either side, `entries()` hands out copies, and a
grant revocation removes the grant only — never a log entry. The
log is in-memory per registry (like the session log): durable
storage of the log is a later phase, not this one.

### Approving a tier-2 call

1. The caller asks for the tool; the gate answers
   `APPROVAL_REQUIRED` and records `awaiting-approval`.
2. A person grants it: `sen_tool_approve` with the same tool name
   and the same idempotency key, plus who granted it
   (`approveTool` on the frontend).
3. The caller retries the same tool + key, in the same session;
   the gate finds the approval and allows it (recorded as
   `allowed`). That allow spends the approval (P1-4): a grant is
   single-use, and it expires 15 minutes after being granted
   (`APPROVAL_TTL_MS`, overridable per grant) — an old approval
   never authorizes a later call, and replaying the same tool +
   key waits for a fresh approval. A grant from another session
   never allows here, even with the same tool + key.
4. A different idempotency key is a different call: it waits for
   its own approval. Revoking a grant (`ApprovalStore.revoke`,
   `sen_tool_revoke` / `revokeTool`) makes the next call wait
   again.

## Verification skill — approval gate

Feature map: entry `packages/core/src/approval-gate.ts` +
`commands/approval_gate.rs` + `commands/registry.rs::tool_call` |
actions: call an auto tool, call a tier-2 tool without/with
approval, call a forbidden tool with a rule and an approval, call
an unknown tool, read the log | observable: the error code
(`TOOL_POLICY_DENIED` / `APPROVAL_REQUIRED` /
`APPROVAL_NEVER_DENIED` / `TOOL_EXECUTION_NOT_WIRED`), the log
entries in seq order.

### 1. LAUNCH

- Worktree `~/workspace/sen-p02-approval-gate`, branch
  `feature/sen-approval-gate`, base HEAD `b6a9340` (P0-3 merge).
- `git log -1` at run time: see Evidence.
- Build/run: `npm test` (node 24, type stripping, no build step);
  `npm run build --prefix apps/desktop` (vite); Rust pure modules
  via the scratch harness `/tmp/sen-p02-rust-check` (see §4)
  because `cargo test` in `src-tauri` needs `gdk-3.0` (not
  installed).

### 2. DOCTOR

- Linux, node v24.22.1, rustc/cargo 1.99.1, no Rust system libs
  (gdk-3.0 missing — `cargo test` in src-tauri NOT RUN, see below).
- Fail early when: HEAD is not the P0-2 base, `node_modules`
  missing (`npm install --prefix apps/desktop` first; the baseline
  run without it fails on `@tauri-apps/api`).

### 3. DRIVE

| # | Command | Result |
| --- | --- | --- |
| 1 | `npm test` (baseline, before changes) | 117 pass, 0 fail |
| 2 | `node --test apps/desktop/tests/approval-gate.test.ts` | 19 pass, 0 fail |
| 3 | `npm test` (after changes) | 136 pass, 0 fail |
| 4 | `npm run build --prefix apps/desktop` | vite build green |
| 5 | `cargo test --offline` in `/tmp/sen-p02-rust-check` (pure modules) | 117 pass, 0 fail |
| 6 | `rustfmt --edition 2021 --check` on the 5 touched Rust files | clean |

### 4. EVIDENCE

- TS suite: 136 pass / 0 fail (117 baseline + 19 new in
  `apps/desktop/tests/approval-gate.test.ts`).
- Rust pure-module harness (`/tmp/sen-p02-rust-check`, same module
  sources copied from the worktree, deps serde 1.0.229 /
  serde_json 1.0.154 / libc 0.2.191 from the offline cargo cache):
  117 pass / 0 fail (100 baseline per the P0-3 harness + 17 new:
  12 in `approval_gate`, 5 in `registry`). The harness is scratch
  only (`/tmp`, not part of the bundle); the one fixture-path
  failure seen mid-run was a harness path artifact (the locator
  walks up to `apps/desktop/...`), fixed by mirroring the fixture
  path — the test itself passes in the repo layout.
- `git log -1` at delivery: base HEAD `b6a9340` (P0-3 merge);
  the P0-2 change is uncommitted on branch `feature/sen-approval-gate`
  (producer bundle, no commit — see the bundle README).
- `git diff --check`: clean (checked at bundle time).

### 5. CLEANUP

- Kept: the worktree (branch `feature/sen-approval-gate`,
  uncommitted changes), the bundle files in `~/workspace`, the
  scratch Rust harness in `/tmp/sen-p02-rust-check` (disposable;
  rebuildable from the worktree + Cargo.lock versions).
- Removed nothing from the repo. `apps/desktop/node_modules` and
  `apps/desktop/dist` are git-ignored build outputs, left in place.
- Evidence still readable after cleanup: this file + the bundle
  README + the test files themselves.

## NOT RUN (must run before merge)

- `cargo test` in `apps/desktop/src-tauri` (full crate incl. the
  Tauri wrappers `tools.rs`): BLOCKED on this box — `gdk-3.0`
  system library missing. Run on the Leader VM.
- `cargo clippy --workspace --all-targets`: same blocker.
- Windows/macOS runs (box is Linux only).
- Live model turn on a real provider: no live call was made; the
  gate path is proven only against the TS suite, the Rust harness,
  and mocked transports.
- Real tool execution: no tool runs for real anywhere yet — a gate
  pass still ends in `TOOL_EXECUTION_NOT_WIRED` on the backend by
  design (execution is a later phase).
- Independent verification (producer ≠ verifier): this bundle is
  the producer's work; a verifier other than the producer checks it.

## Known limitations (NOT hidden)

1. The activity log is in-memory per registry: restarting the app
   loses it. Durable storage of the log is a later phase.
2. Approvals are in-memory grants, not persisted: restarting the
   app loses them, so a tier-2 call after a restart waits again.
3. The tool policy is in-memory too (starts empty on every start):
   there is no settings command yet to store allow rules. The
   frontend approval UI (showing a pending call and granting it)
   is also a later phase; the wire path (`sen_tool_approve` +
   `approveTool`) already exists.
4. No live model turn on a real provider was run here (see NOT RUN).

## Handoff

| Check | Verdict | Evidence |
| --- | --- | --- |
| `npm test` (TS, incl. 19 new) | PASS | 136 pass / 0 fail |
| `npm run build --prefix apps/desktop` | PASS | vite build green, 21.80 kB js |
| Rust pure modules (`cargo test`, scratch harness) | PASS | 117 pass / 0 fail |
| `rustfmt --check` (5 touched Rust files) | PASS | clean |
| `cargo test` in src-tauri (full crate) | NOT RUN | gdk-3.0 missing on this box |
| `cargo clippy` | NOT RUN | same blocker |
| Live provider turn | NOT RUN | no live call made (free-only rule) |
| Real tool execution | NOT RUN | gate pass still ends in TOOL_EXECUTION_NOT_WIRED by design |
| Independent review | NOT RUN | producer bundle; a verifier other than the producer checks it |

Risks left: the Tauri wrappers (`tools.rs` approve/log commands)
compile only under the full crate — a typo there would surface
only on the Leader VM's `cargo test`. The TS side of those
commands is covered by mocked-transport tests, and the shared gate
logic is covered by the Rust `approval_gate` + `registry` tests,
but the wrapper code path itself is uncompiled here.

## P1-4 Rust session — 19/11/2026 (tin 16891, ZC Leader)

The rev2 follow-up: the Rust side is now session-scoped too,
closing the "Chưa làm" item rev2 left open.

1. Wire shape: `ActionContract` carries `sessionId` (TS type:
   optional on the shared type, set by both app contract
   builders from the live session; Rust `ContractArgs.session_id`
   with a serde default, so an older frontend payload still
   deserializes and then fails closed instead of failing to
   parse). `validate_contract` is unchanged on purpose: a
   settings/memory read has no session to scope to. Only the
   tool path requires a session — `validate_tool_session`
   (`registry.rs`) rejects an empty one with `INVALID_CONTRACT`
   before any grant, spend, or evaluation.
2. Store: `Approval.sessionId`, keyed `(session, tool, key)` —
   the same tool + key in another session is a different grant.
   `grant` / `find` / `consume` / `revoke` and
   `evaluate_tool_call` / `gate_tool_call` all take the session;
   argument order mirrors the TS side (`grant(tool, key, by,
   session)`, `find(tool, key, session)`). `Registry`:
   `approve_tool` / `revoke_tool` take the session and return
   `Result` (empty session fails closed); `tool_call` passes
   `contract.session_id` to the gate. `sen_tool_approve` /
   `sen_tool_revoke` take the session from the contract.
   Frontend `Approval.sessionId` is validated in `asApproval`.
3. TTL: `APPROVAL_TTL_SECS` stays 900 (15 minutes) — confirmed
   by the Leader in the same message (tin 16891), no longer the
   seat's own choice.

Evidence (this box, 19/11, rebased onto 389d525, CI P2-6):
`npm test` 257 pass / 0 fail on re-run (256 baseline at
389d525, measured 256/256 clean here before the patch, + 1 new TS
test — an approval without a session id is malformed; the
first full run after the patch was 256 pass / 1 fail on the
CI P2-6 wire-shape test (stub lacked `sessionId`, see below);
after that 1-line stub fix the next run was 256 pass / 1 fail
on the pre-existing flaky `MCP_TIMEOUT` test, then 257 / 0 on
re-run);
`npm run typecheck` (`tsc --noEmit`) clean;
`npm run build --prefix apps/desktop` vite green, built in
363 ms; Rust pure-module harness
(`~/workspace/sen-p1-ts4-rust-session-check`, 8 modules,
serde 1.0.229 / serde_json 1.0.154 / libc 0.2.191 from the
offline cache) 159 pass / 0 fail (154 baseline at 389d525 + 5 new:
grant in session A cannot authorize session B, same tool +
key in two sessions are two grants, tool_call approval scoped
to session, tool_call without session id fails closed, revoke
scoped to session); `rustfmt --check` clean on the 4 touched
Rust files; `git diff --check` clean. New tests were written
first and failed on the unmodified source (the Rust tests
failed to compile — 28 errors: new signatures missing; the TS
test failed with "Missing expected rejection"), then passed
after the fix. Rebase onto 389d525 (CI P2-6) needed one
reconciliation, not a silent drop: the CI P2-6 wire-shape test
stubbed a backend approval with no `sessionId`, which TS4's
`asApproval` now rejects as malformed — the stub now carries
`sessionId: SESSION_ID`, modelling the real post-TS4 backend,
while the wire-shape assertions themselves are untouched.

NOT RUN (same blocker as P0-2/rev2): `cargo test` in
`apps/desktop/src-tauri` (full crate incl. `tools.rs`,
`registry.rs`, `session.rs`, `main.rs`) — `gdk-3.0` missing
on this box (`gdk-sys` build error, `gdk-3.0.pc` not found;
`find / -name gdk-3.0.pc` returns nothing); run on the Leader
VM. That leaves the Tauri wrappers (`sen_tool_approve` /
`sen_tool_revoke` session plumbing, `ContractArgs.session_id`
deserialization) uncompiled anywhere — reviewed by reading,
not by running. `cargo clippy`: same blocker.

## P1-4 rev2 — 17/11/2026 (tin 14959, ZC Leader)

Three additions on top of the rev1 TS patch (expiry + single-use
on the TS side only):

1. Rust mirror (`approval_gate.rs`): `Approval` carries
   `expiresAtSecs` (granted-at + `APPROVAL_TTL_SECS` = 900 s,
   same 15-minute window as `APPROVAL_TTL_MS`); `find()` /
   `list()` drop expired grants; `consume()` spends a grant;
   `gate_tool_call` spends it on the first allow for an
   ask-first tool. `grant_with_ttl` / `grant_with_expiry` are
   the explicit-window / test paths. `evaluate_tool_call` and
   `gate_tool_call` now take `&mut ApprovalStore` (find drops
   expired grants); `registry.rs::tool_call` passes
   `&mut self.approvals`, `list_approvals` is `&mut self`.
2. Session binding (TS only): `Approval.sessionId`, store key
   `sessionId + tool + key`; `grant` / `find` / `consume` /
   `revoke` and `ToolCallInput.sessionId` are session-scoped.
   The session id is the live session's id — the SessionManager
   key (`packages/core/src/session.ts`), the CLI `--session-id`
   UUID (`mcp-client.ts`: `input.sessionId ?? randomUUID()`).
   `onToolCall` throws when the event carries no session id;
   `executeMcpTool` and `applyDecision` take it as a required
   input. The Rust side was NOT session-scoped yet at rev2 time:
   the `ActionContract` on the `sen_tool_call` path carried no
   session id — done 19/11, see "P1-4 Rust session" above.
3. Revoke as a Tauri command: `sen_tool_revoke` in `tools.rs`
   (contract validated first, non-empty tool + key, returns
   whether a live grant was removed), `Registry::revoke_tool`,
   registered in `main.rs`, frontend `revokeTool` in
   `tools-client.ts`. Revoking never touches the activity log.

Evidence (this box, 17/11): `npm test` 227 pass / 1 fail — the
1 fail is the pre-existing flaky `MCP_TIMEOUT` test in
`mcp-client.test.ts:158` (also failed on the unmodified
baseline, passes on re-run in isolation); `npm run build
--prefix apps/desktop` vite green, 22.59 kB js; Rust pure
module harness (`/tmp/sen-p1-ts4-rust-check`, `approval_gate.rs`
only, serde 1.0.229 from the offline cache) 16 pass / 0 fail
(12 carried + 4 new); `rustfmt --check` clean on the 4
touched Rust files; `git diff --check` clean. New tests were
written first and failed on the unmodified source (the 2 TS
session tests failed; the Rust tests failed to compile —
`consume` / `APPROVAL_TTL_SECS` / `expires_at_secs` missing;
the `approval-gate.test.ts` file failed to load —
`revokeTool` export missing), then passed after the fix.
TTL 15 minutes is still the seat's own choice, not yet
confirmed by the Leader.

NOT RUN (same blocker as P0-2): `cargo test` in
`apps/desktop/src-tauri` (full crate incl. `tools.rs`,
`registry.rs`, `main.rs`) — `gdk-3.0` missing on this box;
run on the Leader VM. That leaves the 2 new `registry.rs`
tests (`tool_call_approval_is_single_use`,
`revoke_tool_removes_the_grant_and_the_next_call_waits`) and
the `sen_tool_revoke` wrapper uncompiled here — they were
reviewed by reading, not by running.
