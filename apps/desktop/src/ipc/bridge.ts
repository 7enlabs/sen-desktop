// IPC bridge wrapper - the only layer the frontend may call, per prep B-2 section 2.
// STUB, EXECUTION PART: the real Tauri invoke function is NOT HERE yet because
// the real implementation is not filled in yet. Tauri version 2 is decided per
// Telegram message 4070 (ZC Leader, 03/11/2026); the stub status is about
// missing implementation, not about an undecided version.
// Functions here only validate the contract and block shortcut call paths;
// they never execute a tool on their own.
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
    throw new Error("IPC contract missing fields, FAIL: " + missing.join(", "));
  }
}

export function requireRawLog(rawLogRef: string): void {
  // A1: every model call, including auxiliary calls, must have a raw log line.
  // Missing log means FAIL.
  if (!rawLogRef) {
    throw new Error("Missing raw log, FAIL per A1");
  }
}

export async function invoke(command: string, request: SessionRequest | ToolCallRequest): Promise<unknown> {
  assertContract(request.contract);
  // NOT HERE YET: call the real Tauri invoke here. The core-side SDK wiring
  // is real since the fill-in round (packages/core/src/agent-session.ts),
  // but the frontend Tauri runtime binding (@tauri-apps/api plus the app
  // bundler) is still missing, so this bridge still throws instead of
  // calling through. That binding belongs to the app-build step.
  const err: IpcError = {
    code: "NOT_IMPLEMENTED",
    message: "Tauri invoke NOT HERE YET: not implemented, no real call made.",
    rawLogRef: "",
  };
  throw new Error(err.code + ": " + err.message);
}
