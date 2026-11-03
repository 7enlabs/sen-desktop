// Hop dong goi IPC, theo diem 888 va prep B-2 muc 2. Day la kieu that, chua co chay that.
// Moi lan goi tool phai mang du hop dong nay. Thieu truong thi FAIL, khong goi tiep.

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
