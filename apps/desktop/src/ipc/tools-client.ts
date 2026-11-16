// Frontend client for the P0-2 approval gate (tin 13895).
// Two calls, both through the bridge like every other client:
// - approveTool: grant one approval (one tool + one idempotency key,
//   by a named granter). This is the tier-2 approval path: a
//   send / buy / sign / delete call waits for this before it runs.
// - getActivityLog: read back the append-only activity log.
// No key, token, or credential travels through either call.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";

export interface Approval {
  toolName: string;
  idempotencyKey: string;
  grantedBy: string;
  grantedAt: string;
}

export type ApprovalTier = "auto" | "ask-first" | "never";
export type GateDecision = "allow" | "ask-first" | "deny";
export type ActivityResult = "allowed" | "denied" | "awaiting-approval";

export interface ActivityEntry {
  seq: number;
  timestamp: string;
  actor: string;
  tool: string;
  tier: ApprovalTier;
  decision: GateDecision;
  reason: string;
  result: ActivityResult;
}

function asApproval(value: unknown): Approval {
  const res = value as Approval | null;
  if (
    !res ||
    typeof res.toolName !== "string" ||
    !res.toolName ||
    typeof res.idempotencyKey !== "string" ||
    !res.idempotencyKey ||
    typeof res.grantedBy !== "string" ||
    !res.grantedBy ||
    typeof res.grantedAt !== "string" ||
    !res.grantedAt
  ) {
    throw new Error(
      "Backend approval malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// Grant one approval for one tool + one idempotency key. A grant
/// for a never-tier tool is recorded but never honored by the gate.
export async function approveTool(
  contract: ActionContract,
  toolName: string,
  idempotencyKey: string,
  grantedBy: string,
): Promise<Approval> {
  const raw = await invoke("sen_tool_approve", {
    contract,
    args: { toolName, idempotencyKey, grantedBy },
  });
  return asApproval(raw);
}

function asActivityEntry(value: unknown): ActivityEntry {
  const res = value as ActivityEntry | null;
  if (
    !res ||
    typeof res.seq !== "number" ||
    typeof res.timestamp !== "string" ||
    typeof res.actor !== "string" ||
    typeof res.tool !== "string" ||
    !res.tool ||
    (res.tier !== "auto" && res.tier !== "ask-first" && res.tier !== "never") ||
    (res.decision !== "allow" &&
      res.decision !== "ask-first" &&
      res.decision !== "deny") ||
    typeof res.reason !== "string" ||
    (res.result !== "allowed" &&
      res.result !== "denied" &&
      res.result !== "awaiting-approval")
  ) {
    throw new Error(
      "Backend activity entry malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

/// Read back the append-only activity log (oldest first, seq order).
export async function getActivityLog(
  contract: ActionContract,
): Promise<ActivityEntry[]> {
  const raw = await invoke("sen_tool_activity_log", { contract });
  if (!Array.isArray(raw)) {
    throw new Error(
      "Backend activity log malformed, FAIL: " + JSON.stringify(raw),
    );
  }
  return raw.map((entry) => asActivityEntry(entry));
}
