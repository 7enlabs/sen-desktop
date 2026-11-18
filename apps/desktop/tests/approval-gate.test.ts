// Approval gate tests (Sen P0-2, tin 13895).
// Run with: node --test apps/desktop/tests/approval-gate.test.ts
// Three layers, all runnable under node (no Rust toolchain needed):
// 1. The pure TS gate (packages/core/src/approval-gate.ts): tier
//    tables, default-deny policy, approvals, append-only log.
// 2. The tools hook (packages/tools/src/index.ts): the same gate
//    through the tool_call event hook, FAIL-loud on refusal.
// 3. The tools client (apps/desktop/src/ipc/tools-client.ts):
//    approve + activity-log calls invoke the right commands.
// These tests do NOT prove the Rust side: the Rust mirror suite is
// approval_gate.rs + registry.rs (cargo test, run on the Leader VM —
// cargo test in src-tauri needs gdk-3.0, not installed on this box),
// and real tool execution is NOT RUN anywhere yet (the gate passing
// still ends in TOOL_EXECUTION_NOT_WIRED on the backend).
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  ActivityLog,
  APPROVAL_TTL_MS,
  ApprovalStore,
  classifyTool,
  defaultToolPolicy,
  evaluateToolCall,
  gateToolCall,
  MAX_ACTIVITY_LOG_ENTRIES,
  validateToolPolicy,
  type ToolPolicy,
} from "../../../packages/core/src/approval-gate.ts";
import { onToolCall } from "../../../packages/tools/src/index.ts";
import {
  approveTool,
  getActivityLog,
  revokeTool,
} from "../src/ipc/tools-client.ts";
import { callTool } from "../src/ipc/session-client.ts";

function contract(): ActionContract {
  return {
    id: "c1",
    capability: "sen.session",
    capabilityVersion: "1",
    actor: "test",
    purpose: "test",
    phase: "test",
    toolKind: "registered-tool",
    sideEffect: "no-side-effect",
    risk: "low",
    idempotencyKey: "k1",
    timeoutMs: 1000,
    approvalRequired: false,
    auditRequired: false,
  };
}

function policyAllowing(tools: string[]): ToolPolicy {
  return { allow: [...tools] };
}

// Session ids are UUIDs, the same shape the CLI session id has
// (packages/core/src/mcp-client.ts: input.sessionId ?? randomUUID(),
// validated as a UUID). Two different sessions below stand in for
// two different live sessions.
const SESSION_ID = "11111111-1111-4111-8111-111111111111";
const OTHER_SESSION_ID = "22222222-2222-4222-8222-222222222222";

// --- 1. Pure gate: classification ---

test("gate: read-only work and drafts are auto", () => {
  for (const tool of [
    "read_file",
    "read_status",
    "list_files",
    "list_sessions",
    "search_files",
    "search_web",
    "read_memory",
    "get_status",
    "draft_message",
    "draft_email",
    "draft_post",
    "draft_note",
  ]) {
    assert.equal(classifyTool(tool), "auto", tool);
  }
});

test("gate: send, buy, sign, delete are ask-first", () => {
  for (const tool of [
    "send_message",
    "send_email",
    "send_telegram",
    "publish_post",
    "purchase",
    "buy",
    "pay",
    "sign",
    "sign_document",
    "delete_file",
    "delete_message",
    "delete_session",
    "delete_note",
  ]) {
    assert.equal(classifyTool(tool), "ask-first", tool);
  }
});

test("gate: the three forbidden kinds are never", () => {
  for (const tool of [
    "trade_securities",
    "buy_stock",
    "sell_stock",
    "stock_trade",
    "transfer_between_accounts",
    "transfer_money",
    "access_other_account",
    "impersonate_account",
    "login_as_other_user",
  ]) {
    assert.equal(classifyTool(tool), "never", tool);
  }
});

test("gate: an unclassified tool is ask-first, never auto", () => {
  assert.equal(classifyTool("some-new-tool"), "ask-first");
  assert.equal(classifyTool("edit_file"), "ask-first");
  assert.equal(classifyTool(""), "ask-first");
});

// --- 1. Pure gate: default-deny policy ---

test("gate: the default policy (empty allow-list) denies everything", () => {
  const policy = defaultToolPolicy();
  const approvals = new ApprovalStore();
  for (const tool of ["read_file", "draft_note", "send_message", "trade_securities"]) {
    const evaluation = evaluateToolCall(policy, approvals, {
      actor: "test",
      tool,
      idempotencyKey: "k1",
      sessionId: SESSION_ID,
    });
    assert.equal(evaluation.decision, "deny", tool);
  }
});

test("gate: an auto tool runs only with an explicit allow rule", () => {
  const approvals = new ApprovalStore();
  const allowed = evaluateToolCall(policyAllowing(["read_file"]), approvals, {
    actor: "test",
    tool: "read_file",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(allowed.tier, "auto");
  assert.equal(allowed.decision, "allow");
  assert.match(allowed.reason, /auto-tier and explicitly allowed/);
  const denied = evaluateToolCall(policyAllowing(["read_status"]), approvals, {
    actor: "test",
    tool: "read_file",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(denied.decision, "deny");
  assert.match(denied.reason, /default-deny/);
});

test("gate: ask-first waits for approval, then passes; a wrong key still waits", () => {
  const policy = policyAllowing(["send_message", "delete_file", "pay"]);
  const approvals = new ApprovalStore();
  for (const tool of ["send_message", "delete_file", "pay"]) {
    const waiting = evaluateToolCall(policy, approvals, {
      actor: "test",
      tool,
      idempotencyKey: "k1",
      sessionId: SESSION_ID,
    });
    assert.equal(waiting.decision, "ask-first", tool);
    assert.match(waiting.reason, /needs explicit approval/);
  }
  approvals.grant("send_message", "k1", "zen", SESSION_ID);
  const allowed = evaluateToolCall(policy, approvals, {
    actor: "test",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(allowed.decision, "allow");
  assert.match(allowed.reason, /approved by zen/);
  const wrongKey = evaluateToolCall(policy, approvals, {
    actor: "test",
    tool: "send_message",
    idempotencyKey: "k2",
    sessionId: SESSION_ID,
  });
  assert.equal(wrongKey.decision, "ask-first");
});

test("gate: an approval without a policy rule still denies", () => {
  const approvals = new ApprovalStore();
  approvals.grant("delete_file", "k1", "zen", SESSION_ID);
  const evaluation = evaluateToolCall(defaultToolPolicy(), approvals, {
    actor: "test",
    tool: "delete_file",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(evaluation.decision, "deny");
  assert.match(evaluation.reason, /default-deny/);
});

test("gate: never-tier is denied even with a rule and an approval", () => {
  const policy = policyAllowing([
    "trade_securities",
    "transfer_between_accounts",
    "access_other_account",
  ]);
  const approvals = new ApprovalStore();
  for (const tool of [
    "trade_securities",
    "transfer_between_accounts",
    "access_other_account",
  ]) {
    approvals.grant(tool, "k1", "zen", SESSION_ID);
    const evaluation = evaluateToolCall(policy, approvals, {
      actor: "test",
      tool,
      idempotencyKey: "k1",
      sessionId: SESSION_ID,
    });
    assert.equal(evaluation.tier, "never", tool);
    assert.equal(evaluation.decision, "deny", tool);
    assert.match(evaluation.reason, /no approval path/, tool);
  }
});

test("gate: policy validation rejects a non-array allow-list and blank entries", () => {
  assert.deepEqual(validateToolPolicy({ allow: ["read_file"] }), []);
  assert.ok(validateToolPolicy({ allow: "read_file" }).length > 0);
  assert.ok(validateToolPolicy(null).length > 0);
  assert.ok(
    validateToolPolicy({ allow: ["read_file", ""] }).some((e) =>
      e.includes("non-empty strings"),
    ),
  );
  assert.ok(
    validateToolPolicy({ allow: ["read_file", "read_file"] }).some((e) =>
      e.includes("duplicates tool"),
    ),
  );
});

// --- 1. Pure gate: approvals + activity log ---

test("gate: approval store grants, finds, revokes, lists", () => {
  const store = new ApprovalStore();
  assert.equal(store.find("send_message", "k1", SESSION_ID), undefined);
  const approval = store.grant("send_message", "k1", "zen", SESSION_ID);
  assert.equal(approval.toolName, "send_message");
  assert.equal(approval.grantedBy, "zen");
  assert.ok(approval.grantedAt.length > 0);
  assert.deepEqual(store.find("send_message", "k1", SESSION_ID), approval);
  assert.equal(store.find("send_message", "k2", SESSION_ID), undefined);
  assert.equal(store.list().length, 1);
  assert.equal(store.revoke("send_message", "k1", SESSION_ID), true);
  assert.equal(store.revoke("send_message", "k1", SESSION_ID), false);
  assert.equal(store.find("send_message", "k1", SESSION_ID), undefined);
  assert.throws(
    () => store.grant("", "k1", "zen", SESSION_ID),
    /non-empty tool/,
  );
});

// --- 1b. P1-4: grants expire and are single-use ---

test("gate: a grant carries an expiry (default 15 minutes, overridable)", () => {
  const store = new ApprovalStore();
  const grantedAt = "2026-11-17T10:00:00.000Z";
  const approval = store.grant("send_message", "k1", "zen", SESSION_ID, grantedAt);
  assert.equal(
    approval.expiresAt,
    new Date(Date.parse(grantedAt) + APPROVAL_TTL_MS).toISOString(),
  );
  assert.equal(APPROVAL_TTL_MS, 15 * 60 * 1000);
  const short = store.grant("send_message", "k2", "zen", SESSION_ID, grantedAt, { ttlMs: 1000 });
  assert.equal(short.expiresAt, "2026-11-17T10:00:01.000Z");
  const pinned = store.grant("send_message", "k3", "zen", SESSION_ID, grantedAt, {
    expiresAt: "2026-11-17T11:00:00.000Z",
  });
  assert.equal(pinned.expiresAt, "2026-11-17T11:00:00.000Z");
  assert.throws(() => store.grant("send_message", "k4", "zen", SESSION_ID, grantedAt, { ttlMs: 0 }), /ttlMs/);
  assert.throws(
    () => store.grant("send_message", "k5", "zen", SESSION_ID, grantedAt, { expiresAt: "not-a-date" }),
    /expiresAt/,
  );
});

test("gate: an expired grant reads as absent and never allows again", () => {
  const store = new ApprovalStore();
  const policy = policyAllowing(["send_message"]);
  // Granted with an expiry already in the past: dead on arrival.
  const past = new Date(Date.now() - 1000).toISOString();
  const pastGrantedAt = new Date(Date.now() - 2000).toISOString();
  store.grant("send_message", "k-exp", "boss", SESSION_ID, pastGrantedAt, { expiresAt: past });
  assert.equal(store.find("send_message", "k-exp", SESSION_ID), undefined);
  assert.equal(store.list().length, 0);
  const evaluation = evaluateToolCall(policy, store, {
    actor: "test",
    tool: "send_message",
    idempotencyKey: "k-exp",
    sessionId: SESSION_ID,
  });
  assert.equal(evaluation.decision, "ask-first");
});

test("gate: an approval is single-use through gateToolCall", () => {
  const log = new ActivityLog();
  const policy = policyAllowing(["send_message"]);
  const approvals = new ApprovalStore();
  approvals.grant("send_message", "k1", "zen", SESSION_ID);
  const first = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(first.decision, "allow");
  assert.equal(approvals.find("send_message", "k1", SESSION_ID), undefined);
  // Same tool + key again: the spent grant is gone, so the gate
  // waits for a fresh approval instead of allowing on the old one.
  const second = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(second.decision, "ask-first");
  assert.deepEqual(
    log.entries().map((e) => e.result),
    ["allowed", "awaiting-approval"],
  );
});

// --- 1c. P1-4 rev2: grants are bound to one session ---

test("gate: a grant is bound to its session and unusable from another", () => {
  const policy = policyAllowing(["send_message"]);
  const approvals = new ApprovalStore();
  approvals.grant("send_message", "k1", "zen", SESSION_ID);
  // Same tool + key, other session: the grant is not there.
  assert.equal(approvals.find("send_message", "k1", OTHER_SESSION_ID), undefined);
  assert.equal(approvals.consume("send_message", "k1", OTHER_SESSION_ID), undefined);
  const fromOther = evaluateToolCall(policy, approvals, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: OTHER_SESSION_ID,
  });
  assert.equal(fromOther.decision, "ask-first");
  // The owning session still allows, and the grant is still live
  // (the other session's lookups spent nothing).
  const fromOwner = evaluateToolCall(policy, approvals, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(fromOwner.decision, "allow");
  // The same tool + key granted in the other session is
  // independent: each session spends its own grant.
  approvals.grant("send_message", "k1", "zen", OTHER_SESSION_ID);
  const log = new ActivityLog();
  assert.equal(
    gateToolCall(policy, approvals, log, {
      actor: "tester",
      tool: "send_message",
      idempotencyKey: "k1",
      sessionId: SESSION_ID,
    }).decision,
    "allow",
  );
  assert.equal(
    gateToolCall(policy, approvals, log, {
      actor: "tester",
      tool: "send_message",
      idempotencyKey: "k1",
      sessionId: OTHER_SESSION_ID,
    }).decision,
    "allow",
  );
  assert.equal(approvals.list().length, 0);
});

test("gate: revoke is session-scoped", () => {
  const store = new ApprovalStore();
  store.grant("send_message", "k1", "zen", SESSION_ID);
  store.grant("send_message", "k1", "zen", OTHER_SESSION_ID);
  assert.equal(store.revoke("send_message", "k1", SESSION_ID), true);
  assert.equal(store.find("send_message", "k1", SESSION_ID), undefined);
  assert.notEqual(store.find("send_message", "k1", OTHER_SESSION_ID), undefined);
  assert.equal(store.revoke("send_message", "k1", SESSION_ID), false);
  assert.equal(store.list().length, 1);
});

// --- 1d. P2-5: the activity log is capped, FIFO, seq never reused ---
// The cap is exercised through a small constructor bound so the tests
// stay fast; the production default (MAX_ACTIVITY_LOG_ENTRIES) is
// asserted, not filled, because recording 10,000 entries in a test
// would prove nothing the small-cap tests do not already prove.
// Same shape as packages/core/tests/raw-log.test.ts.

function recordActivity(log: ActivityLog, i: number) {
  return log.record({
    actor: "tester",
    tool: "read_file",
    tier: "auto",
    decision: "allow",
    reason: `ok ${i}`,
    result: "allowed",
  });
}

test("gate: activity log default cap is 10000 entries", () => {
  assert.equal(MAX_ACTIVITY_LOG_ENTRIES, 10000);
  const log = new ActivityLog();
  for (let i = 0; i < 3; i += 1) recordActivity(log, i);
  assert.equal(log.length, 3);
  assert.deepEqual(
    log.entries().map((e) => e.seq),
    [1, 2, 3],
  );
});

test("gate: activity log non-positive cap is rejected at construction", () => {
  assert.throws(() => new ActivityLog(0), /maxEntries must be a positive integer/);
  assert.throws(() => new ActivityLog(-1), /maxEntries must be a positive integer/);
});

test("gate: activity log over the cap evicts oldest first and keeps the N newest in order", () => {
  const log = new ActivityLog(5);
  for (let i = 0; i < 7; i += 1) recordActivity(log, i);
  assert.equal(log.length, 5);
  // Oldest two (seq 1, 2) evicted; newest five kept in append order.
  assert.deepEqual(
    log.entries().map((e) => e.seq),
    [3, 4, 5, 6, 7],
  );
  assert.deepEqual(
    log.entries().map((e) => e.reason),
    ["ok 2", "ok 3", "ok 4", "ok 5", "ok 6"],
  );
});

test("gate: activity log seq keeps increasing monotonically after eviction, never reused", () => {
  const log = new ActivityLog(3);
  for (let i = 0; i < 3; i += 1) recordActivity(log, i);
  recordActivity(log, 3);
  recordActivity(log, 4);
  assert.deepEqual(
    log.entries().map((e) => e.seq),
    [3, 4, 5],
  );
  // The next entry continues at 6, not back at 1: the seq gap (1-2 gone)
  // is what makes eviction observable instead of silent.
  assert.equal(recordActivity(log, 5).seq, 6);
});

test("gate: grant requires a non-empty session id", () => {
  const store = new ApprovalStore();
  assert.throws(
    () => store.grant("send_message", "k1", "zen", ""),
    /non-empty session/,
  );
  assert.throws(
    () => store.grant("send_message", "k1", "zen", "   "),
    /non-empty session/,
  );
});

test("hook: an approval is single-use through onToolCall", () => {
  const approvals = new ApprovalStore();
  approvals.grant("send_message", "k1", "zen", SESSION_ID);
  const allowed = onToolCall(
    {
      contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
      toolName: "send_message",
      actor: "tester",
      sessionId: SESSION_ID,
    },
    { policy: policyAllowing(["send_message"]), approvals },
  );
  assert.equal(allowed.decision, "allow");
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "send_message",
          actor: "tester",
          sessionId: SESSION_ID,
        },
        { policy: policyAllowing(["send_message"]), approvals },
      ),
    /APPROVAL_REQUIRED/,
  );
});

test("hook: onToolCall requires a session id and scopes the grant to it", () => {
  const approvals = new ApprovalStore();
  approvals.grant("send_message", "k1", "zen", SESSION_ID);
  // No session id at all: fail loud, like a missing idempotency key.
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "send_message",
          actor: "tester",
        },
        { policy: policyAllowing(["send_message"]), approvals },
      ),
    /missing session id/,
  );
  // The grant's session allows; another session waits.
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "send_message",
          actor: "tester",
          sessionId: OTHER_SESSION_ID,
        },
        { policy: policyAllowing(["send_message"]), approvals },
      ),
    /APPROVAL_REQUIRED/,
  );
  const allowed = onToolCall(
    {
      contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
      toolName: "send_message",
      actor: "tester",
      sessionId: SESSION_ID,
    },
    { policy: policyAllowing(["send_message"]), approvals },
  );
  assert.equal(allowed.decision, "allow");
});

test("gate: the log records allow, deny, and ask-first with monotonic seq", () => {
  const log = new ActivityLog();
  assert.equal(log.length, 0);
  const policy = policyAllowing(["read_file", "send_message"]);
  const approvals = new ApprovalStore();
  const a = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "read_file",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(a.decision, "allow");
  const b = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(b.decision, "ask-first");
  const c = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "trade_securities",
    idempotencyKey: "k1",
    sessionId: SESSION_ID,
  });
  assert.equal(c.decision, "deny");
  assert.equal(log.length, 3);
  const entries = log.entries();
  assert.deepEqual(
    entries.map((e) => e.seq),
    [1, 2, 3],
  );
  assert.equal(entries[0].result, "allowed");
  assert.equal(entries[1].result, "awaiting-approval");
  assert.equal(entries[2].result, "denied");
  assert.equal(entries[2].tier, "never");
  assert.equal(entries[0].actor, "tester");
  assert.equal(log.entriesFor("send_message").length, 1);
  assert.equal(log.entriesFor("no-such-tool").length, 0);
  // Readers get copies: mutating the returned array changes nothing.
  const stolen = log.entries();
  stolen.length = 0;
  assert.equal(log.length, 3);
  // The log exposes no delete or update path at all.
  assert.equal(
    "delete" in log || "clear" in log || "update" in log,
    false,
  );
});

// --- 2. Tools hook ---

test("hook: an allowed auto tool returns its evaluation", () => {
  const log = new ActivityLog();
  const decision = onToolCall(
    {
      contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
      toolName: "read_file",
      actor: "tester",
      sessionId: SESSION_ID,
    },
    { policy: policyAllowing(["read_file"]), log },
  );
  assert.equal(decision.decision, "allow");
  assert.equal(decision.tier, "auto");
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "allowed");
});

test("hook: ask-first without approval throws APPROVAL_REQUIRED and logs it", () => {
  const log = new ActivityLog();
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "send_message",
          actor: "tester",
          sessionId: SESSION_ID,
        },
        { policy: policyAllowing(["send_message"]), log },
      ),
    /APPROVAL_REQUIRED/,
  );
  assert.equal(log.length, 1);
  assert.equal(log.entries()[0].result, "awaiting-approval");
});

test("hook: never-tier throws APPROVAL_NEVER_DENIED even with a rule and an approval", () => {
  const approvals = new ApprovalStore();
  approvals.grant("trade_securities", "k1", "zen", SESSION_ID);
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "trade_securities",
          actor: "tester",
          sessionId: SESSION_ID,
        },
        { policy: policyAllowing(["trade_securities"]), approvals },
      ),
    /APPROVAL_NEVER_DENIED/,
  );
});

test("hook: no policy means default-deny, and the contract checks still fail loud", () => {
  assert.throws(
    () =>
      onToolCall({
        contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
        toolName: "read_file",
        sessionId: SESSION_ID,
      }),
    /TOOL_POLICY_DENIED/,
  );
  assert.throws(
    () =>
      onToolCall({
        contract: { id: "c1", idempotencyKey: "", toolKind: "registered-tool" },
        toolName: "read_file",
      }),
    /missing idempotency key/,
  );
  assert.throws(
    () =>
      onToolCall({
        contract: { id: "c1", idempotencyKey: "k1", toolKind: "bad-kind" },
        toolName: "read_file",
      }),
    /Illegal tool kind/,
  );
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "  ",
        },
        { policy: policyAllowing(["read_file"]) },
      ),
    /missing tool name/,
  );
});

// --- 3. Tools client ---

test("tools-client approve: invokes sen_tool_approve with the grant under args", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, ...args });
    return {
      toolName: "send_message",
      idempotencyKey: "k1",
      grantedBy: "zen",
      grantedAt: "epoch-1",
      expiresAtSecs: 1793123456,
    };
  });
  try {
    const approval = await approveTool(contract(), "send_message", "k1", "zen");
    assert.equal(approval.grantedBy, "zen");
    assert.equal(approval.expiresAtSecs, 1793123456);
    assert.equal(seen[0]["command"], "sen_tool_approve");
    assert.deepEqual(seen[0]["args"], {
      toolName: "send_message",
      idempotencyKey: "k1",
      grantedBy: "zen",
    });
  } finally {
    setBridgeTransport(null);
  }
});

test("tools-client revoke: invokes sen_tool_revoke and returns its boolean", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, ...args });
    return true;
  });
  try {
    assert.equal(await revokeTool(contract(), "send_message", "k1"), true);
    assert.equal(seen[0]["command"], "sen_tool_revoke");
    assert.deepEqual(seen[0]["args"], {
      toolName: "send_message",
      idempotencyKey: "k1",
    });
  } finally {
    setBridgeTransport(null);
  }
});

test("tools-client revoke: a non-boolean answer fails loudly", async () => {
  setBridgeTransport(async () => "yes");
  try {
    await assert.rejects(() => revokeTool(contract(), "send_message", "k1"), /malformed/);
  } finally {
    setBridgeTransport(null);
  }
});

test("tools-client activity log: invokes sen_tool_activity_log and validates entries", async () => {
  setBridgeTransport(async (command) => {
    assert.equal(command, "sen_tool_activity_log");
    return [
      {
        seq: 1,
        timestamp: "epoch-1",
        actor: "tester",
        tool: "read_file",
        tier: "auto",
        decision: "allow",
        reason: "auto-tier",
        result: "allowed",
      },
      {
        seq: 2,
        timestamp: "epoch-2",
        actor: "tester",
        tool: "trade_securities",
        tier: "never",
        decision: "deny",
        reason: "never-tier",
        result: "denied",
      },
    ];
  });
  try {
    const entries = await getActivityLog(contract());
    assert.equal(entries.length, 2);
    assert.equal(entries[0].seq, 1);
    assert.equal(entries[1].tier, "never");
  } finally {
    setBridgeTransport(null);
  }
});

test("tools-client activity log: a malformed entry fails loudly", async () => {
  setBridgeTransport(async () => [{ seq: 1, tool: "read_file" }]);
  try {
    await assert.rejects(() => getActivityLog(contract()), /malformed/);
  } finally {
    setBridgeTransport(null);
  }
});

// --- 4. Tools client wire shape (CI P2-6) ---
// Same lesson as settings-client R2B-05 rev2: the v1 mock-transport
// tests only assert what the client sends, so a client that sends a
// flat payload while the handler waits for `args: <Args>` (or a
// nested `args` while the handler waits for a flat `tool_name`)
// passes under node and fails on every real call ("missing required
// key args" / "missing required key tool_name"). These tests read
// each handler signature out of tools.rs and assert the client
// payload against it, so they FAIL on a wrong payload and PASS only
// when the client sends the shape the handler waits for. They do NOT
// prove the Rust side gates or logs: that evidence is the
// approval_gate.rs + registry.rs suite (cargo test, run on the
// Leader VM), and real tool execution is NOT RUN anywhere yet.
function rustToolsSource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/tools.rs", import.meta.url),
    "utf8",
  );
}

function toCamelCase(snake: string): string {
  return snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

/// Expected wire shape for a command with an `args: <Args>` struct,
/// derived from the Rust handler: the struct-typed parameter name
/// (e.g. "args") plus the camelCase field names of its args struct.
function expectedWireShape(
  src: string,
  command: string,
): { paramKey: string; fields: string[] } {
  const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
  assert.ok(fnMatch, `Rust handler ${command} not found in tools.rs`);
  // tools.rs declares `contract: ContractArgs` BEFORE `args: <Args>`
  // (approve/revoke), so take the first *Args parameter that is not
  // the contract: `contract` is the wire contract, not the args struct.
  const argsMatch = [...fnMatch[1].matchAll(/(\w+):\s*(\w+Args)/g)].find(
    (m) => m[1] !== "contract",
  );
  assert.ok(argsMatch, `${command} declares no *Args parameter in tools.rs`);
  const paramKey = argsMatch[1];
  const structName = argsMatch[2];
  const structMatch = src.match(new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\}`));
  assert.ok(structMatch, `Rust struct ${structName} not found in tools.rs`);
  const structHead = src.slice(Math.max(0, (structMatch.index ?? 0) - 200), structMatch.index ?? 0);
  assert.match(structHead, /rename_all = "camelCase"/, `${structName} is not camelCase-renamed`);
  const fields = [...structMatch[1].matchAll(/pub (\w+):/g)].map((m) => toCamelCase(m[1]));
  assert.ok(fields.length > 0, `${structName} has no fields`);
  return { paramKey, fields };
}

/// Flat wire params for a command, derived from the Rust handler
/// signature: every declared parameter except the Tauri-managed
/// `state` slot and the `contract`. Param names are asserted as
/// declared (snake_case, e.g. `tool_name`).
function expectedFlatParams(src: string, command: string): string[] {
  const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
  assert.ok(fnMatch, `Rust handler ${command} not found in tools.rs`);
  const params = fnMatch[1];
  // Split on commas that are not inside <...> (State<'_, Mutex<...>>).
  const chunks: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of params) {
    if (ch === "<") depth += 1;
    if (ch === ">") depth -= 1;
    if (ch === "," && depth === 0) {
      chunks.push(cur);
      cur = "";
    } else {
      cur += ch;
    }
  }
  chunks.push(cur);
  const names = chunks
    .map((c) => c.trim().match(/^(\w+)\s*:/)?.[1])
    .filter((n): n is string => !!n);
  const wire = names.filter((n) => !["state", "contract"].includes(n));
  assert.ok(wire.length > 0, `${command} declares no wire params in tools.rs`);
  return wire;
}

async function capturePayload(
  command: string,
  stubResult: unknown,
  call: () => Promise<unknown>,
): Promise<Record<string, unknown>> {
  let seen: Record<string, unknown> = {};
  setBridgeTransport(async (cmd, args) => {
    assert.equal(cmd, command);
    seen = args as Record<string, unknown>;
    return stubResult;
  });
  try {
    await call();
  } finally {
    setBridgeTransport(null);
  }
  return seen;
}

test("tools-client approve: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustToolsSource(), "sen_tool_approve");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["toolName", "idempotencyKey", "grantedBy"]);
  const payload = await capturePayload(
    "sen_tool_approve",
    {
      toolName: "send_message",
      idempotencyKey: "k1",
      grantedBy: "zen",
      grantedAt: "epoch-1",
      expiresAtSecs: 1793123456,
    },
    () => approveTool(contract(), "send_message", "k1", "zen"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], {
    toolName: "send_message",
    idempotencyKey: "k1",
    grantedBy: "zen",
  });
  assert.ok(!("toolName" in payload) && !("idempotencyKey" in payload) && !("grantedBy" in payload), "payload must not carry flat toolName/idempotencyKey/grantedBy keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("tools-client revoke: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustToolsSource(), "sen_tool_revoke");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["toolName", "idempotencyKey"]);
  const payload = await capturePayload(
    "sen_tool_revoke",
    true,
    () => revokeTool(contract(), "send_message", "k1"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], {
    toolName: "send_message",
    idempotencyKey: "k1",
  });
  assert.ok(!("toolName" in payload) && !("idempotencyKey" in payload), "payload must not carry flat toolName/idempotencyKey keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("tools-client call: flat tool_name param matches the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustToolsSource(), "sen_tool_call"), ["tool_name"]);
  const payload = await capturePayload(
    "sen_tool_call",
    undefined,
    () => callTool(contract(), "read_file"),
  );
  assert.deepEqual(payload, { contract: contract(), tool_name: "read_file" });
  assert.ok(!("args" in payload), "call takes a flat tool_name, so the payload must not carry an args key");
  assert.ok(!("toolName" in payload), "payload must use the declared snake_case tool_name, not camelCase toolName");
});

test("tools-client activity log: no args key, contract only (CI P2-6)", async () => {
  const src = rustToolsSource();
  const fnMatch = src.match(/pub fn sen_tool_activity_log\(([\s\S]*?)\) ->/);
  assert.ok(fnMatch, "Rust handler sen_tool_activity_log not found in tools.rs");
  assert.doesNotMatch(fnMatch[1], /args:\s*\w+Args/, "sen_tool_activity_log declares no args struct parameter in tools.rs");
  const payload = await capturePayload(
    "sen_tool_activity_log",
    [],
    () => getActivityLog(contract()),
  );
  assert.deepEqual(Object.keys(payload), ["contract"]);
  assert.ok(!("args" in payload), "activity log takes no args struct, so the payload must not carry an args key");
});
