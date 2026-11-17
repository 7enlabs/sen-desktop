// Frontend client for the P0-2 approval gate (tin 13895).
// Three calls, all through the bridge like every other client:
// - approveTool: grant one approval (one tool + one idempotency key,
//   by a named granter). This is the tier-2 approval path: a
//   send / buy / sign / delete call waits for this before it runs.
// - revokeTool: revoke one approval (one tool + one idempotency
//   key); the next call with that tool + key waits again.
// - getActivityLog: read back the append-only activity log.
// No key, token, or credential travels through any call.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";

export interface Approval {
  toolName: string;
  idempotencyKey: string;
  grantedBy: string;
  grantedAt: string;
  /** Epoch seconds after which the grant no longer counts (P1-4:
   * the Rust grant expires APPROVAL_TTL_SECS after it is granted). */
  expiresAtSecs: number;
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
    !res.grantedAt ||
    typeof res.expiresAtSecs !== "number" ||
    !Number.isFinite(res.expiresAtSecs)
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

/// Revoke one approval (one tool + one idempotency key): the next
/// call with that tool + key waits for a fresh approval. Returns
/// true when a live grant was removed, false when there was none
/// (already spent, expired, or never granted). Revoking a grant
/// never touches the activity log: the log has no delete path.
export async function revokeTool(
  contract: ActionContract,
  toolName: string,
  idempotencyKey: string,
): Promise<boolean> {
  const raw = await invoke("sen_tool_revoke", {
    contract,
    args: { toolName, idempotencyKey },
  });
  if (typeof raw !== "boolean") {
    throw new Error(
      "Backend revoke answer malformed, FAIL: " + JSON.stringify(raw),
    );
  }
  return raw;
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
