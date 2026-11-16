// Proactive inbox (Sen P0-4): turn decision emails into proposals,
// and apply a proposal only through the P0-2 approval gate.
//
// A proposal never runs anything by itself. Applying one goes through
// gateToolCall (packages/core approval-gate): the evaluation is
// recorded in the ActivityLog exactly once per applyDecision call,
// and a non-allow decision throws with the same P0-2 error code
// (APPROVAL_REQUIRED / TOOL_POLICY_DENIED / APPROVAL_NEVER_DENIED)
// without doing anything. Even a gate pass does NOT execute: there
// is no Gmail sender and no calendar writer wired in this repo yet,
// so a pass returns { executed: false, reason: "EXECUTION_NOT_WIRED" }
// — permission granted is not execution, same honest line as P0-2.
//
// The one exception is action "ignore": it maps to no tool, so it
// needs no gate and writes no log entry — it only marks the proposal
// resolved locally (status "executed" means the local mark, not a
// tool run; the returned `executed` stays false because no tool ran).
//
// This module is pure TS (no I/O, no network, no key, no model call).
import {
  ActivityLog,
  ApprovalStore,
  gateToolCall,
  type GateEvaluation,
  type ToolPolicy,
} from "./approval-gate.ts";
import type { EmailItem, MorningBrief } from "./morning-brief.ts";

export type DecisionAction = "reply" | "draft-reply" | "archive" | "schedule" | "ignore";

export type ProposalStatus = "pending" | "approved" | "refused" | "executed";

export interface DecisionProposal {
  id: string;
  emailId: string;
  action: DecisionAction;
  summary: string;
  /** Tool the action would call, or null for "ignore" (no tool). */
  toolName: string | null;
  idempotencyKey: string;
  status: ProposalStatus;
}

/**
 * The tool each action would call. The tier comes from the P0-2
 * tables in approval-gate.ts: send_email is ask-first, draft_email
 * is auto, archive_email and create_calendar_event are unclassified
 * and therefore ask-first (fail closed until classified).
 */
export function toolNameForAction(action: DecisionAction): string | null {
  switch (action) {
    case "reply":
      return "send_email";
    case "draft-reply":
      return "draft_email";
    case "archive":
      return "archive_email";
    case "schedule":
      return "create_calendar_event";
    case "ignore":
      return null;
  }
}

export function proposalIdForEmail(emailId: string): string {
  return `proposal:${emailId}`;
}

export function idempotencyKeyFor(proposalId: string, toolName: string): string {
  return `${proposalId}:${toolName}`;
}

/**
 * One proposal per email that needs a decision. The default action
 * is "reply": the summary says plainly that the user must approve
 * before anything is sent — choosing a different action (draft,
 * archive, schedule, ignore) is the user's call at approval time,
 * not something this function decides for them. Proposal ids and
 * idempotency keys are deterministic from the email id, so building
 * twice from the same brief gives the same proposals.
 */
export function proposeDecisions(
  brief: MorningBrief,
  emails: EmailItem[],
): DecisionProposal[] {
  const byId = new Map(emails.map((email) => [email.id, email]));
  return brief.needsDecision.map((briefEmail) => {
    const email = byId.get(briefEmail.id) ?? briefEmail;
    const id = proposalIdForEmail(email.id);
    const toolName = toolNameForAction("reply");
    return {
      id,
      emailId: email.id,
      action: "reply" as const,
      summary: `Cần duyệt: ${email.subject} — từ ${email.from}`,
      toolName,
      idempotencyKey: idempotencyKeyFor(id, toolName as string),
      status: "pending" as const,
    };
  });
}

export interface ApplyDecisionInput {
  policy: ToolPolicy;
  approvals: ApprovalStore;
  log: ActivityLog;
  proposal: DecisionProposal;
  actor: string;
}

export interface ApplyDecisionResult {
  /** Whether the mapped tool ran. Always false in P0-4: no executor
   * is wired yet, and "ignore" runs no tool at all. */
  executed: boolean;
  reason: string;
  evaluation?: GateEvaluation;
}

/**
 * Apply one proposal. The proposal object is mutated in place
 * (status change only); the caller keeps the reference.
 *
 * - "ignore": no gate, no log entry, status becomes "executed"
 *   (local mark only). Returns executed: false.
 * - Any other action: evaluated through gateToolCall (logged exactly
 *   once). A non-allow decision throws with the P0-2 code and the
 *   proposal stays "pending", so the user can approve and retry.
 *   An allow decision sets status "approved" and returns
 *   { executed: false, reason: "EXECUTION_NOT_WIRED" } — nothing
 *   was sent, drafted, archived, or scheduled.
 */
export function applyDecision(input: ApplyDecisionInput): ApplyDecisionResult {
  const { proposal } = input;
  if (proposal.action === "ignore") {
    proposal.status = "executed";
    return { executed: false, reason: "IGNORED_LOCAL_ONLY" };
  }
  const toolName = proposal.toolName;
  if (toolName === null) {
    throw new Error("TOOL_POLICY_DENIED: proposal action has no tool to gate");
  }
  const evaluation = gateToolCall(input.policy, input.approvals, input.log, {
    actor: input.actor,
    tool: toolName,
    idempotencyKey: proposal.idempotencyKey,
  });
  if (evaluation.decision !== "allow") {
    if (evaluation.decision === "ask-first") {
      throw new Error("APPROVAL_REQUIRED: " + evaluation.reason);
    }
    if (evaluation.tier === "never") {
      throw new Error("APPROVAL_NEVER_DENIED: " + evaluation.reason);
    }
    throw new Error("TOOL_POLICY_DENIED: " + evaluation.reason);
  }
  proposal.status = "approved";
  return { executed: false, reason: "EXECUTION_NOT_WIRED", evaluation };
}
