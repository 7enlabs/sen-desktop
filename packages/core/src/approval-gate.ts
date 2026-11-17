// Approval gate (Sen P0-2): 3-tier approval + activity log + default-deny
// tool policy. Tin 13895 (ZC Leader, 16/11): every tool that sends,
// pays, signs, or deletes goes through 3 tiers, default refuse.
//
// Tiers:
// 1. auto — read-only work and drafts only. Runs at once, no question.
// 2. ask-first — send, buy, sign, delete. Must ask and wait for an
//    explicit approval before running; without one the call is refused,
//    never run on the sly.
// 3. never — securities trading, transfers between accounts, access to
//    another person's account. Always refused, no approval path exists.
//
// Default-deny: the policy is an explicit allow-list, empty by default.
// A tool runs only when a rule allows it by name AND its tier allows
// it (auto: at once; ask-first: plus a matching approval). A tool with
// no classification is ask-first, never auto: an unknown tool can only
// run with an explicit allow rule AND an explicit approval.
//
// The activity log records EVERY evaluation (allowed, denied,
// awaiting-approval). It is append-only in execution: there is no
// delete or update API, entries keep a monotonic seq, and entries()
// hands out copies so a caller cannot rewrite history through the
// returned array.
//
// This module is pure TS (no I/O, no key, no network). The Rust mirror
// is apps/desktop/src-tauri/src/commands/approval_gate.rs — the two
// must stay in sync: same tier tables, same evaluation order, same
// error reasons. The backend (registry.rs tool_call) is the enforcing
// side; the frontend hook (packages/tools onToolCall) is the second
// layer, using this same module.

export type ApprovalTier = "auto" | "ask-first" | "never";

export type GateDecision = "allow" | "ask-first" | "deny";

export type ActivityResult = "allowed" | "denied" | "awaiting-approval";

/**
 * Tier 1 — read-only work and drafts. Nothing here sends, pays,
 * signs, or deletes anything, so it may run at once once the policy
 * allows it by name.
 */
export const AUTO_TOOLS: readonly string[] = [
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
];

/**
 * Tier 2 — send, buy, sign, delete. Needs an explicit approval for
 * the exact tool + idempotency key before it runs.
 */
export const ASK_FIRST_TOOLS: readonly string[] = [
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
];

/**
 * Tier 3 — never. No approval can override this list: an approval
 * record for one of these tools is ignored, and a policy allow rule
 * for one of them is ignored too.
 */
export const NEVER_TOOLS: readonly string[] = [
  "trade_securities",
  "buy_stock",
  "sell_stock",
  "stock_trade",
  "transfer_between_accounts",
  "transfer_money",
  "access_other_account",
  "impersonate_account",
  "login_as_other_user",
];

/**
 * Which tier a tool belongs to. Exact name match only; an unknown
 * tool is ask-first, never auto: new tools fail closed until someone
 * classifies them and writes an explicit allow rule.
 */
export function classifyTool(toolName: string): ApprovalTier {
  if ((NEVER_TOOLS as readonly string[]).includes(toolName)) {
    return "never";
  }
  if ((AUTO_TOOLS as readonly string[]).includes(toolName)) {
    return "auto";
  }
  return "ask-first";
}

export function describeTier(tier: ApprovalTier): string {
  switch (tier) {
    case "auto":
      return "auto: read-only or draft, runs at once when allowed";
    case "ask-first":
      return "ask-first: send / buy / sign / delete, needs approval";
    case "never":
      return "never: always refused, no approval path";
  }
}

/**
 * Tool policy: an explicit allow-list of tool names. Empty by
 * default, which denies everything (default-deny). A rule for a
 * never-tier tool is inert: it never makes that tool runnable.
 */
export interface ToolPolicy {
  allow: string[];
}

export function defaultToolPolicy(): ToolPolicy {
  return { allow: [] };
}

/** Validate a policy from unknown input (JSON from disk or IPC). */
export function validateToolPolicy(policy: unknown): string[] {
  if (!policy || typeof policy !== "object" || Array.isArray(policy)) {
    return ["tool policy must be an object"];
  }
  const raw = policy as Record<string, unknown>;
  if (!Array.isArray(raw.allow)) {
    return ["tool policy allow must be an array"];
  }
  const errors: string[] = [];
  const seen = new Set<string>();
  for (const name of raw.allow as unknown[]) {
    if (typeof name !== "string" || !name.trim()) {
      errors.push("tool policy allow entries must be non-empty strings");
      continue;
    }
    if (seen.has(name)) {
      errors.push(`tool policy allow duplicates tool: ${name}`);
    }
    seen.add(name);
  }
  return errors;
}

/** One explicit approval: granted by a person, for one tool call.
 * A grant is bounded two ways (Sen P1-4): it expires
 * APPROVAL_TTL_MS after it is granted, and it is spent the first
 * time a gate allows a call with it — a second call with the same
 * tool + key waits for a fresh approval. */
export interface Approval {
  toolName: string;
  idempotencyKey: string;
  grantedBy: string;
  grantedAt: string;
  /** ISO timestamp after which the grant no longer counts. */
  expiresAt: string;
}

/**
 * Default lifetime of a grant: 15 minutes. Long enough for a person
 * to approve in the UI and the call to run at once; short enough
 * that an approval from long ago can never authorize a later run.
 * A caller that needs a different window passes ttlMs/expiresAt
 * to grant(); the default is the fail-closed choice, not a
 * per-call decision.
 */
export const APPROVAL_TTL_MS = 15 * 60 * 1000;

/** In-memory approval grants. A grant matches one tool + one key,
 * lives at most APPROVAL_TTL_MS, and is spent on first use. */
export class ApprovalStore {
  private grants = new Map<string, Approval>();

  private static key(toolName: string, idempotencyKey: string): string {
    return `${toolName}\n${idempotencyKey}`;
  }

  private static isExpired(approval: Approval, nowMs: number): boolean {
    const expiresMs = Date.parse(approval.expiresAt);
    return Number.isNaN(expiresMs) || expiresMs <= nowMs;
  }

  grant(
    toolName: string,
    idempotencyKey: string,
    grantedBy: string,
    grantedAt: string = new Date().toISOString(),
    options: { ttlMs?: number; expiresAt?: string } = {},
  ): Approval {
    if (!toolName.trim() || !idempotencyKey.trim() || !grantedBy.trim()) {
      throw new Error(
        "Approval needs a non-empty tool, idempotency key, and granter, FAIL",
      );
    }
    if (options.ttlMs !== undefined) {
      if (!Number.isFinite(options.ttlMs) || options.ttlMs <= 0) {
        throw new Error("Approval ttlMs must be a positive number, FAIL");
      }
    }
    let expiresAt = options.expiresAt;
    if (expiresAt !== undefined && Number.isNaN(Date.parse(expiresAt))) {
      throw new Error("Approval expiresAt must be a valid ISO date, FAIL");
    }
    if (expiresAt === undefined) {
      const grantedMs = Date.parse(grantedAt);
      const baseMs = Number.isNaN(grantedMs) ? Date.now() : grantedMs;
      expiresAt = new Date(
        baseMs + (options.ttlMs ?? APPROVAL_TTL_MS),
      ).toISOString();
    }
    const approval: Approval = { toolName, idempotencyKey, grantedBy, grantedAt, expiresAt };
    this.grants.set(ApprovalStore.key(toolName, idempotencyKey), approval);
    return { ...approval };
  }

  find(toolName: string, idempotencyKey: string): Approval | undefined {
    const key = ApprovalStore.key(toolName, idempotencyKey);
    const found = this.grants.get(key);
    if (!found) {
      return undefined;
    }
    // An expired grant counts as absent, and is dropped so it can
    // never be honored later: expiry is enforced on read, not only
    // on grant.
    if (ApprovalStore.isExpired(found, Date.now())) {
      this.grants.delete(key);
      return undefined;
    }
    return { ...found };
  }

  /**
   * Spend a grant: return it only if it is still live, and remove
   * it either way, so one approval authorizes exactly one allowed
   * call. Returns undefined for an absent or expired grant.
   */
  consume(toolName: string, idempotencyKey: string): Approval | undefined {
    const found = this.find(toolName, idempotencyKey);
    if (!found) {
      return undefined;
    }
    this.grants.delete(ApprovalStore.key(toolName, idempotencyKey));
    return found;
  }

  /** Revoking a grant removes the grant, never a log entry. */
  revoke(toolName: string, idempotencyKey: string): boolean {
    return this.grants.delete(ApprovalStore.key(toolName, idempotencyKey));
  }

  list(): Approval[] {
    const nowMs = Date.now();
    const live: Approval[] = [];
    for (const [key, approval] of this.grants) {
      if (ApprovalStore.isExpired(approval, nowMs)) {
        this.grants.delete(key);
        continue;
      }
      live.push({ ...approval });
    }
    return live;
  }
}

/** One recorded tool-call evaluation. Seq is the ordering authority. */
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

/**
 * Append-only activity log. There is deliberately no delete, no
 * update, and no clear: entries only ever get appended, seq never
 * reuses a number, and readers get copies.
 */
export class ActivityLog {
  private entriesList: ActivityEntry[] = [];
  private nextSeq = 1;

  record(input: {
    actor: string;
    tool: string;
    tier: ApprovalTier;
    decision: GateDecision;
    reason: string;
    result: ActivityResult;
    timestamp?: string;
  }): ActivityEntry {
    const entry: ActivityEntry = {
      seq: this.nextSeq,
      timestamp: input.timestamp ?? new Date().toISOString(),
      actor: input.actor,
      tool: input.tool,
      tier: input.tier,
      decision: input.decision,
      reason: input.reason,
      result: input.result,
    };
    this.nextSeq += 1;
    this.entriesList.push(entry);
    return { ...entry };
  }

  entries(): ActivityEntry[] {
    return this.entriesList.map((e) => ({ ...e }));
  }

  entriesFor(tool: string): ActivityEntry[] {
    return this.entriesList
      .filter((e) => e.tool === tool)
      .map((e) => ({ ...e }));
  }

  get length(): number {
    return this.entriesList.length;
  }
}

export interface ToolCallInput {
  actor: string;
  tool: string;
  idempotencyKey: string;
}

export interface GateEvaluation {
  tier: ApprovalTier;
  decision: GateDecision;
  reason: string;
}

function resultOf(decision: GateDecision): ActivityResult {
  switch (decision) {
    case "allow":
      return "allowed";
    case "ask-first":
      return "awaiting-approval";
    case "deny":
      return "denied";
  }
}

/**
 * Pure evaluation, in fixed order: never-tier first (no override),
 * then the default-deny policy, then the approval check for
 * ask-first tools. No side effect on live grants, no logging: use
 * gateToolCall for the evaluated + recorded + consuming path.
 * (find() drops grants that already expired, so an expired grant
 * reads as absent here too; a live grant is never spent by this
 * function — only the caller that acts on an allow spends it.)
 */
export function evaluateToolCall(
  policy: ToolPolicy,
  approvals: ApprovalStore,
  input: ToolCallInput,
): GateEvaluation {
  const tier = classifyTool(input.tool);
  if (tier === "never") {
    return {
      tier,
      decision: "deny",
      reason:
        `tool "${input.tool}" is never-tier (securities trading / ` +
        `transfer between accounts / access to another account): ` +
        `always denied, no approval path`,
    };
  }
  if (!policy.allow.includes(input.tool)) {
    return {
      tier,
      decision: "deny",
      reason:
        `default-deny: no explicit allow rule for tool "${input.tool}"`,
    };
  }
  if (tier === "ask-first") {
    const approval = approvals.find(input.tool, input.idempotencyKey);
    if (!approval) {
      return {
        tier,
        decision: "ask-first",
        reason:
          `tool "${input.tool}" needs explicit approval before it runs`,
      };
    }
    return {
      tier,
      decision: "allow",
      reason:
        `tool "${input.tool}" is ask-first, explicitly allowed and ` +
        `approved by ${approval.grantedBy}`,
    };
  }
  return {
    tier,
    decision: "allow",
    reason: `tool "${input.tool}" is auto-tier and explicitly allowed`,
  };
}

/**
 * Evaluate one tool call AND record it in the activity log: allowed,
 * denied, and awaiting-approval are all recorded, exactly once each.
 * "Allow" here means permission granted, not executed: the caller
 * still has to run (or refuse to run) the tool itself.
 * An allow for an ask-first tool spends the approval that allowed
 * it (ApprovalStore.consume): one grant authorizes exactly one
 * call, so replaying the same tool + key waits for a fresh
 * approval. Auto-tier allows spend nothing (no approval exists).
 */
export function gateToolCall(
  policy: ToolPolicy,
  approvals: ApprovalStore,
  log: ActivityLog,
  input: ToolCallInput,
): GateEvaluation {
  const evaluation = evaluateToolCall(policy, approvals, input);
  log.record({
    actor: input.actor,
    tool: input.tool,
    tier: evaluation.tier,
    decision: evaluation.decision,
    reason: evaluation.reason,
    result: resultOf(evaluation.decision),
  });
  if (evaluation.decision === "allow" && evaluation.tier === "ask-first") {
    approvals.consume(input.tool, input.idempotencyKey);
  }
  return evaluation;
}
