// IPC bridge wrapper - lop duy nhat frontend duoc goi, theo prep B-2 muc 2.
// STUB PHAN THUC THI: ham invoke that cua Tauri CHUA CO vi version Tauri CHUA CHOT.
// Ham o day chi kiem tra hop dong va chan duong goi tat, khong tu thuc thi tool.
import type { ActionContract, IpcError, SessionRequest, ToolCallRequest } from "./types.ts";

export function assertContract(contract: ActionContract): void {
  const missing: string[] = [];
  if (!contract.id) missing.push("id");
  if (!contract.capability) missing.push("capability");
  if (!contract.capabilityVersion) missing.push("capabilityVersion");
  if (!contract.actor) missing.push("actor");
  if (!contract.purpose) missing.push("purpose");
  if (!contract.idempotencyKey) missing.push("idempotencyKey");
  if (!contract.timeoutMs) missing.push("timeoutMs");
  if (missing.length > 0) {
    throw new Error("Hop dong IPC thieu truong, FAIL: " + missing.join(", "));
  }
}

export function requireRawLog(rawLogRef: string): void {
  // A1: moi loi goi model ke ca phu deu phai co dong log tho. Thieu log thi FAIL.
  if (!rawLogRef) {
    throw new Error("Thieu log tho, FAIL theo A1");
  }
}

export async function invoke(command: string, request: SessionRequest | ToolCallRequest): Promise<unknown> {
  assertContract(request.contract);
  // CHUA CO: goi Tauri invoke that o day sau khi chot version Tauri.
  const err: IpcError = {
    code: "NOT_IMPLEMENTED",
    message: "Tauri invoke CHUA CO: version Tauri CHUA CHOT, khong goi that.",
    rawLogRef: "",
  };
  throw new Error(err.code + ": " + err.message);
}
