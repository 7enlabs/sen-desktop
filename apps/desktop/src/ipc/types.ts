// IPC call contract, per point 888 and prep B-2 section 2. These are real
// types; no real call runs yet.
// Every tool call must carry this full contract. A missing field means FAIL,
// and the call does not proceed.

export type ToolKind = "model-tool" | "registered-tool" | "authorized-execution-tool";

export type SideEffectClass = "no-side-effect" | "reversible" | "irreversible";

export interface ActionContract {
  id: string;
  capability: string;
  capabilityVersion: string;
  actor: string;
  purpose: string;
  phase: string;
  toolKind: ToolKind;
  sideEffect: SideEffectClass;
  risk: string;
  idempotencyKey: string;
  timeoutMs: number;
  approvalRequired: boolean;
  auditRequired: boolean;
  /** The live session this contract acts for (P1-4 Rust session,
   * 19/11): the SessionManager key / CLI --session-id UUID, set by
   * the frontend contract builder from the session it created.
   * Optional on the shared type because settings/memory reads have
   * no session to scope to; the tool path (sen_tool_call /
   * sen_tool_approve / sen_tool_revoke) rejects a contract without
   * one, fail closed. */
  sessionId?: string;
}

export interface SessionRequest {
  contract: ActionContract;
}

export interface ToolCallRequest {
  contract: ActionContract;
  toolName: string;
}

export interface IpcError {
  code: string;
  message: string;
  rawLogRef: string;
}
