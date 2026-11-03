// Permission layer: tool_call event hook, judged by behavior, not by the
// shell syntax of one OS, per prep B-2 section 3 point 6.
// Every tool call must pass through this hook. A tool call that skips the hook
// means FAIL.
// STUB: the real permission logic is NOT FILLED IN YET. No narrow Mika mode
// is applied, no mode switch mid-run, no disabling of the protection layer,
// per prep section 3 point 10.
// Minimal contract type at the tools layer; no backwards import from
// apps/desktop.
// When filled in, cross-check against apps/desktop/src/ipc/types.ts.
export interface ToolContract {
  id: string;
  idempotencyKey: string;
  toolKind: string;
}

export interface ToolCallEvent {
  contract: ToolContract;
  toolName: string;
}

export function onToolCall(event: ToolCallEvent): void {
  // NOT HERE YET: real behavior-based permission check. Only the minimal
  // contract check runs here.
  if (!event.contract.idempotencyKey) {
    throw new Error("Tool call missing idempotency key, FAIL");
  }
  if (event.contract.toolKind !== "registered-tool" && event.contract.toolKind !== "authorized-execution-tool" && event.contract.toolKind !== "model-tool") {
    throw new Error("Illegal tool kind, FAIL");
  }
  // Next step when filled in: wire this hook into the 6-step execution gate
  // in packages/core.
  throw new Error("Permission layer NOT FILLED IN YET: no tool runs for real.");
}
