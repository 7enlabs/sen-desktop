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
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  ActivityLog,
  ApprovalStore,
  classifyTool,
  defaultToolPolicy,
  evaluateToolCall,
  gateToolCall,
  validateToolPolicy,
  type ToolPolicy,
} from "../../../packages/core/src/approval-gate.ts";
import { onToolCall } from "../../../packages/tools/src/index.ts";
import {
  approveTool,
  getActivityLog,
} from "../src/ipc/tools-client.ts";

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
  });
  assert.equal(allowed.tier, "auto");
  assert.equal(allowed.decision, "allow");
  assert.match(allowed.reason, /auto-tier and explicitly allowed/);
  const denied = evaluateToolCall(policyAllowing(["read_status"]), approvals, {
    actor: "test",
    tool: "read_file",
    idempotencyKey: "k1",
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
    });
    assert.equal(waiting.decision, "ask-first", tool);
    assert.match(waiting.reason, /needs explicit approval/);
  }
  approvals.grant("send_message", "k1", "zen");
  const allowed = evaluateToolCall(policy, approvals, {
    actor: "test",
    tool: "send_message",
    idempotencyKey: "k1",
  });
  assert.equal(allowed.decision, "allow");
  assert.match(allowed.reason, /approved by zen/);
  const wrongKey = evaluateToolCall(policy, approvals, {
    actor: "test",
    tool: "send_message",
    idempotencyKey: "k2",
  });
  assert.equal(wrongKey.decision, "ask-first");
});

test("gate: an approval without a policy rule still denies", () => {
  const approvals = new ApprovalStore();
  approvals.grant("delete_file", "k1", "zen");
  const evaluation = evaluateToolCall(defaultToolPolicy(), approvals, {
    actor: "test",
    tool: "delete_file",
    idempotencyKey: "k1",
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
    approvals.grant(tool, "k1", "zen");
    const evaluation = evaluateToolCall(policy, approvals, {
      actor: "test",
      tool,
      idempotencyKey: "k1",
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
  assert.equal(store.find("send_message", "k1"), undefined);
  const approval = store.grant("send_message", "k1", "zen");
  assert.equal(approval.toolName, "send_message");
  assert.equal(approval.grantedBy, "zen");
  assert.ok(approval.grantedAt.length > 0);
  assert.deepEqual(store.find("send_message", "k1"), approval);
  assert.equal(store.find("send_message", "k2"), undefined);
  assert.equal(store.list().length, 1);
  assert.equal(store.revoke("send_message", "k1"), true);
  assert.equal(store.revoke("send_message", "k1"), false);
  assert.equal(store.find("send_message", "k1"), undefined);
  assert.throws(
    () => store.grant("", "k1", "zen"),
    /non-empty tool/,
  );
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
  });
  assert.equal(a.decision, "allow");
  const b = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "send_message",
    idempotencyKey: "k1",
  });
  assert.equal(b.decision, "ask-first");
  const c = gateToolCall(policy, approvals, log, {
    actor: "tester",
    tool: "trade_securities",
    idempotencyKey: "k1",
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
  approvals.grant("trade_securities", "k1", "zen");
  assert.throws(
    () =>
      onToolCall(
        {
          contract: { id: "c1", idempotencyKey: "k1", toolKind: "registered-tool" },
          toolName: "trade_securities",
          actor: "tester",
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
    };
  });
  try {
    const approval = await approveTool(contract(), "send_message", "k1", "zen");
    assert.equal(approval.grantedBy, "zen");
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
