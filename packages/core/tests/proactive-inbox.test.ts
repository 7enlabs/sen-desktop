// Proactive inbox tests (Sen P0-4).
// Run with: node --test packages/core/tests/proactive-inbox.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// Fixtures are explicit test data in this file. These tests prove the
// proposal + gate path only: deterministic proposals, the P0-2 gate
// refusing without policy/approval, allowing with both, never-tier
// still denying with both, the log recording every outcome once, and
// ignore needing no gate. They do NOT prove a real send: a gate pass
// returns EXECUTION_NOT_WIRED, never executed: true (see
// docs/morning-brief.md).
import test from "node:test";
import assert from "node:assert/strict";
import {
  ActivityLog,
  ApprovalStore,
  buildMorningBrief,
  type EmailItem,
} from "../src/index.ts";
import {
  applyDecision,
  proposeDecisions,
  toolNameForAction,
  type DecisionProposal,
} from "../src/proactive-inbox.ts";

const NOW = "2026-11-16T07:00:00.000Z";

// Session ids are UUIDs, the same shape the CLI session id has
// (input.sessionId ?? randomUUID() in mcp-client.ts, validated as
// a UUID). Two ids stand in for two different live sessions.
const SESSION_ID = "11111111-1111-4111-8111-111111111111";
const OTHER_SESSION_ID = "22222222-2222-4222-8222-222222222222";

function briefWithDecisions() {
  const emails: EmailItem[] = [
    {
      id: "m1",
      from: "sep@example.com",
      subject: "Vui lòng phê duyệt ngân sách",
      body: "Xem file đính kèm.",
      receivedAt: "2026-11-16T06:00:00.000Z",
    },
    {
      id: "m2",
      from: "boss@example.com",
      subject: "Báo cáo tuần",
      body: "FYI, không có gì cần làm.",
      receivedAt: "2026-11-16T06:05:00.000Z",
    },
  ];
  const brief = buildMorningBrief({ emails, events: [], now: NOW });
  return { emails, brief };
}

function proposal(over: Partial<DecisionProposal> & { id: string }): DecisionProposal {
  return {
    emailId: "m1",
    action: "reply",
    summary: "Cần duyệt: test — từ a@example.com",
    toolName: "send_email",
    idempotencyKey: "proposal:m1:send_email",
    status: "pending",
    ...over,
  };
}

// --- Action -> tool mapping ---

test("proposals: every action maps to its tool, ignore maps to no tool", () => {
  assert.equal(toolNameForAction("reply"), "send_email");
  assert.equal(toolNameForAction("draft-reply"), "draft_email");
  assert.equal(toolNameForAction("archive"), "archive_email");
  assert.equal(toolNameForAction("schedule"), "create_calendar_event");
  assert.equal(toolNameForAction("ignore"), null);
});

// --- proposeDecisions ---

test("proposals: one proposal per decision email, deterministic ids and keys", () => {
  const { emails, brief } = briefWithDecisions();
  const first = proposeDecisions(brief, emails);
  const second = proposeDecisions(brief, emails);
  assert.deepEqual(first, second);
  // m2 needs no decision, so it gets no proposal.
  assert.equal(first.length, 1);
  assert.equal(first[0].id, "proposal:m1");
  assert.equal(first[0].emailId, "m1");
  assert.equal(first[0].action, "reply");
  assert.equal(first[0].toolName, "send_email");
  assert.equal(first[0].idempotencyKey, "proposal:m1:send_email");
  assert.equal(first[0].status, "pending");
  assert.match(first[0].summary, /Vui lòng phê duyệt ngân sách/);
  assert.match(first[0].summary, /sep@example\.com/);
  // The summary says the user must approve: the proposal never
  // claims it will send on its own.
  assert.match(first[0].summary, /Cần duyệt/);
});

test("proposals: no decision email means no proposal", () => {
  const brief = buildMorningBrief({
    emails: [
      {
        id: "m1",
        from: "a@example.com",
        subject: "Báo cáo tuần",
        body: "FYI.",
        receivedAt: "2026-11-16T06:00:00.000Z",
      },
    ],
    events: [],
    now: NOW,
  });
  assert.deepEqual(proposeDecisions(brief, []), []);
});

// --- applyDecision: refusals ---

test("apply: no allow rule refuses with TOOL_POLICY_DENIED and logs once", () => {
  const p = proposal({ id: "proposal:m1" });
  const log = new ActivityLog();
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: [] },
        approvals: new ApprovalStore(),
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /TOOL_POLICY_DENIED/,
  );
  assert.equal(p.status, "pending");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "denied");
  assert.equal(log.entries()[0].tool, "send_email");
});

test("apply: allow rule but no approval refuses with APPROVAL_REQUIRED and logs once", () => {
  const p = proposal({ id: "proposal:m1" });
  const log = new ActivityLog();
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["send_email"] },
        approvals: new ApprovalStore(),
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /APPROVAL_REQUIRED/,
  );
  assert.equal(p.status, "pending");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "awaiting-approval");
});

test("apply: never-tier still denies even with an allow rule AND an approval", () => {
  const p = proposal({
    id: "proposal:m1",
    toolName: "trade_securities",
    idempotencyKey: "proposal:m1:trade_securities",
  });
  const approvals = new ApprovalStore();
  approvals.grant("trade_securities", "proposal:m1:trade_securities", "boss", SESSION_ID);
  const log = new ActivityLog();
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["trade_securities"] },
        approvals,
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /APPROVAL_NEVER_DENIED/,
  );
  assert.equal(p.status, "pending");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "denied");
  assert.equal(log.entries()[0].tier, "never");
});

// --- applyDecision: allow path (still no execution) ---

test("apply: allow rule + approval passes the gate but never executes", () => {
  const p = proposal({ id: "proposal:m1" });
  const approvals = new ApprovalStore();
  approvals.grant("send_email", "proposal:m1:send_email", "boss", SESSION_ID);
  const log = new ActivityLog();
  const result = applyDecision({
    policy: { allow: ["send_email"] },
    approvals,
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "EXECUTION_NOT_WIRED");
  assert.equal(result.evaluation?.decision, "allow");
  assert.equal(p.status, "approved");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "allowed");
});

test("apply: an approval from another session does not apply the proposal", () => {
  const p = proposal({ id: "proposal:m1" });
  const approvals = new ApprovalStore();
  approvals.grant("send_email", "proposal:m1:send_email", "boss", OTHER_SESSION_ID);
  const log = new ActivityLog();
  // The grant belongs to another session: this session still waits.
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["send_email"] },
        approvals,
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /APPROVAL_REQUIRED/,
  );
  assert.equal(p.status, "pending");
  // Granted for this session: the proposal applies.
  approvals.grant("send_email", "proposal:m1:send_email", "boss", SESSION_ID);
  const result = applyDecision({
    policy: { allow: ["send_email"] },
    approvals,
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "EXECUTION_NOT_WIRED");
  assert.equal(p.status, "approved");
});

test("apply: replaying an approved proposal refuses and never passes the gate again", () => {
  const p = proposal({ id: "proposal:m1" });
  const approvals = new ApprovalStore();
  approvals.grant("send_email", "proposal:m1:send_email", "boss", SESSION_ID);
  const log = new ActivityLog();
  const result = applyDecision({
    policy: { allow: ["send_email"] },
    approvals,
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "EXECUTION_NOT_WIRED");
  assert.equal(p.status, "approved");
  assert.equal(log.length, 1);
  // The same proposal again: the approval was spent on the first
  // apply, and the proposal itself is terminal — replay refuses
  // before the gate, and no second log entry is written.
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["send_email"] },
        approvals,
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /PROPOSAL_ALREADY_APPLIED/,
  );
  assert.equal(p.status, "approved");
  assert.equal(log.length, 1);
});

test("apply: an expired approval refuses with APPROVAL_REQUIRED and stays pending", () => {
  const p = proposal({ id: "proposal:m1" });
  const approvals = new ApprovalStore();
  const past = new Date(Date.now() - 1000).toISOString();
  approvals.grant("send_email", "proposal:m1:send_email", "boss", SESSION_ID, new Date(Date.now() - 2000).toISOString(), {
    expiresAt: past,
  });
  const log = new ActivityLog();
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["send_email"] },
        approvals,
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /APPROVAL_REQUIRED/,
  );
  assert.equal(p.status, "pending");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "awaiting-approval");
});

test("apply: auto-tier draft needs an allow rule but no approval", () => {
  const p = proposal({
    id: "proposal:m1",
    action: "draft-reply",
    toolName: "draft_email",
    idempotencyKey: "proposal:m1:draft_email",
  });
  const log = new ActivityLog();
  const result = applyDecision({
    policy: { allow: ["draft_email"] },
    approvals: new ApprovalStore(),
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "EXECUTION_NOT_WIRED");
  assert.equal(p.status, "approved");
});

test("apply: unclassified archive_email is ask-first until approved", () => {
  const p = proposal({
    id: "proposal:m1",
    action: "archive",
    toolName: "archive_email",
    idempotencyKey: "proposal:m1:archive_email",
  });
  const log = new ActivityLog();
  assert.throws(
    () =>
      applyDecision({
        policy: { allow: ["archive_email"] },
        approvals: new ApprovalStore(),
        log,
        proposal: p,
        actor: "test",
        sessionId: SESSION_ID,
      }),
    /APPROVAL_REQUIRED/,
  );
  const approvals = new ApprovalStore();
  approvals.grant("archive_email", "proposal:m1:archive_email", "boss", SESSION_ID);
  const result = applyDecision({
    policy: { allow: ["archive_email"] },
    approvals,
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "EXECUTION_NOT_WIRED");
  assert.equal(log.length, 2);
});

// --- Log records all three outcomes ---

test("apply: the log records denied, awaiting-approval, and allowed, one entry each", () => {
  const log = new ActivityLog();
  const approvals = new ApprovalStore();
  const policy = { allow: ["send_email"] };
  const denied = proposal({ id: "proposal:d1", idempotencyKey: "proposal:d1:send_email" });
  const waiting = proposal({ id: "proposal:d2", idempotencyKey: "proposal:d2:send_email" });
  const allowed = proposal({ id: "proposal:d3", idempotencyKey: "proposal:d3:send_email" });
  approvals.grant("send_email", "proposal:d3:send_email", "boss", SESSION_ID);

  assert.throws(
    () => applyDecision({ policy: { allow: [] }, approvals, log, proposal: denied, actor: "test", sessionId: SESSION_ID }),
    /TOOL_POLICY_DENIED/,
  );
  assert.throws(
    () => applyDecision({ policy, approvals, log, proposal: waiting, actor: "test", sessionId: SESSION_ID }),
    /APPROVAL_REQUIRED/,
  );
  const result = applyDecision({ policy, approvals, log, proposal: allowed, actor: "test", sessionId: SESSION_ID });
  assert.equal(result.executed, false);

  assert.equal(log.length, 3);
  assert.deepEqual(
    log.entries().map((e) => e.result),
    ["denied", "awaiting-approval", "allowed"],
  );
  assert.deepEqual(
    log.entries().map((e) => e.seq),
    [1, 2, 3],
  );
});

// --- ignore: no gate, no log ---

test("apply: ignore needs no gate and writes no log entry", () => {
  const p = proposal({
    id: "proposal:m1",
    action: "ignore",
    toolName: null,
    idempotencyKey: "proposal:m1:ignore",
  });
  const log = new ActivityLog();
  const result = applyDecision({
    policy: { allow: [] },
    approvals: new ApprovalStore(),
    log,
    proposal: p,
    actor: "test",
    sessionId: SESSION_ID,
  });
  assert.equal(result.executed, false);
  assert.equal(result.reason, "IGNORED_LOCAL_ONLY");
  assert.equal(result.evaluation, undefined);
  // Local-only mark: the proposal is resolved, no tool ran, no gate
  // evaluation was recorded.
  assert.equal(p.status, "executed");
  assert.equal(log.length, 0);
});

test("apply: executed is never true, for any action", () => {
  const approvals = new ApprovalStore();
  approvals.grant("send_email", "proposal:m1:send_email", "boss", SESSION_ID);
  approvals.grant("draft_email", "proposal:m1:draft_email", "boss", SESSION_ID);
  const log = new ActivityLog();
  const cases: DecisionProposal[] = [
    proposal({ id: "proposal:m1" }),
    proposal({ id: "proposal:m1", action: "draft-reply", toolName: "draft_email", idempotencyKey: "proposal:m1:draft_email" }),
    proposal({ id: "proposal:m1", action: "ignore", toolName: null, idempotencyKey: "proposal:m1:ignore" }),
  ];
  const policies = [
    { allow: ["send_email"] },
    { allow: ["draft_email"] },
    { allow: [] as string[] },
  ];
  cases.forEach((p, i) => {
    const result = applyDecision({
      policy: policies[i],
      approvals,
      log,
      proposal: p,
      actor: "test",
      sessionId: SESSION_ID,
    });
    assert.equal(result.executed, false, p.action);
  });
});
