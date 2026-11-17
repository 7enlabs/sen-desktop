// Permission layer: tool_call event hook, judged by behavior, not by the
// shell syntax of one OS, per prep B-2 section 3 point 6.
// Every tool call must pass through this hook. A tool call that skips the hook
// means FAIL.
//
// P0-2: the hook is filled in with the 3-tier approval gate from
// packages/core (same module the Rust backend mirrors). Default-deny:
// without an explicit policy allow rule the call is refused, and a
// never-tier tool is refused even with a rule and an approval. An
// ask-first call without a matching approval throws APPROVAL_REQUIRED
// (the frontend shows the approval UI); it never runs on the sly.
// An allowed call returns its evaluation: permission granted, NOT
// executed — the tool itself still runs (or is refused) by the caller,
// and the backend registry is the enforcing side (its tool_call ends
// in TOOL_EXECUTION_NOT_WIRED after the gate passes, because real
// tool execution is a later phase).
// Minimal contract type at the tools layer; no backwards import from
// apps/desktop. The core import below is a relative path, the same
// style apps/desktop uses to import packages/core.
import {
  ActivityLog,
  ApprovalStore,
  defaultToolPolicy,
  evaluateToolCall,
  type ApprovalTier,
  type GateDecision,
  type ToolPolicy,
} from "../../core/src/approval-gate.ts";

export interface ToolContract {
  id: string;
  idempotencyKey: string;
  toolKind: string;
}

export interface ToolCallEvent {
  contract: ToolContract;
  toolName: string;
  /** Who is asking for the tool; "unknown-actor" when not given. */
  actor?: string;
  /** The live session asking for the tool (a UUID, the same id
   * the CLI session and the SessionManager use). Required: a
   * grant belongs to one session, so a call with no session id
   * can never match a grant and fails loud instead of running
   * on a grant from another session. */
  sessionId?: string;
}

export interface ToolGateContext {
  policy?: ToolPolicy;
  approvals?: ApprovalStore;
  /** When given, every evaluation (allow / deny / ask-first) is recorded. */
  log?: ActivityLog;
}

export interface ToolCallDecision {
  tier: ApprovalTier;
  decision: GateDecision;
  reason: string;
}

/**
 * The tool_call hook. Throws (FAIL-loud, "CODE: message" style) when
 * the call may not run; returns the evaluation when permission is
 * granted. Defaults: default-deny policy (empty allow-list) and no
 * approvals, so a bare hook call refuses everything except nothing —
 * a caller that wants a tool to run must pass an explicit policy.
 */
export function onToolCall(
  event: ToolCallEvent,
  gate: ToolGateContext = {},
): ToolCallDecision {
  if (!event.contract.idempotencyKey) {
    throw new Error("Tool call missing idempotency key, FAIL");
  }
  if (event.contract.toolKind !== "registered-tool" && event.contract.toolKind !== "authorized-execution-tool" && event.contract.toolKind !== "model-tool") {
    throw new Error("Illegal tool kind, FAIL");
  }
  if (!event.toolName.trim()) {
    throw new Error("Tool call missing tool name, FAIL");
  }
  if (!event.sessionId?.trim()) {
    throw new Error("Tool call missing session id, FAIL");
  }
  const policy = gate.policy ?? defaultToolPolicy();
  const approvals = gate.approvals ?? new ApprovalStore();
  const actor = event.actor ?? "unknown-actor";
  const evaluation = evaluateToolCall(policy, approvals, {
    actor,
    tool: event.toolName,
    idempotencyKey: event.contract.idempotencyKey,
    sessionId: event.sessionId,
  });
  if (gate.log) {
    gate.log.record({
      actor,
      tool: event.toolName,
      tier: evaluation.tier,
      decision: evaluation.decision,
      reason: evaluation.reason,
      result:
        evaluation.decision === "allow"
          ? "allowed"
          : evaluation.decision === "ask-first"
            ? "awaiting-approval"
            : "denied",
    });
  }
  if (evaluation.decision === "allow") {
    // An allow for an ask-first tool spends the approval (P1-4):
    // one grant authorizes exactly one call, so the same tool +
    // key cannot pass this hook again on an old approval.
    if (evaluation.tier === "ask-first") {
      approvals.consume(event.toolName, event.contract.idempotencyKey, event.sessionId);
    }
    return evaluation;
  }
  if (evaluation.decision === "ask-first") {
    throw new Error("APPROVAL_REQUIRED: " + evaluation.reason);
  }
  if (evaluation.tier === "never") {
    throw new Error("APPROVAL_NEVER_DENIED: " + evaluation.reason);
  }
  throw new Error("TOOL_POLICY_DENIED: " + evaluation.reason);
}
