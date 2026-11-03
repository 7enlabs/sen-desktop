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
