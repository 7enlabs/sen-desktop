// IPC bridge wrapper - the only layer the frontend may call, per prep B-2 section 2.
// BRIDGE ROUND: invoke() now calls the real Tauri invoke from
// @tauri-apps/api/core (Tauri 2, decided per Telegram message 4070).
// The NOT_IMPLEMENTED stub is gone. Outside a Tauri webview (for example
// under node in tests) the real invoke throws a runtime error
// ("window is not defined") instead of a stub error; that is proof the
// binding is attempted, not proof of a running app.
// Contract validation runs BEFORE the transport call: a bad contract never
// reaches the backend. Transport errors propagate untouched, never mapped
// to a fake success and never swallowed.
// The transport is injectable ONLY for tests (setBridgeTransport): the app
// path always uses the real Tauri invoke. A test transport stands in for
// the Rust handlers, which cannot run under node.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { ActionContract } from "./types.ts";

export function assertContract(contract: ActionContract): void {
  const missing: string[] = [];
  if (!contract.id) missing.push("id");
  if (!contract.capability) missing.push("capability");
  if (!contract.capabilityVersion) missing.push("capabilityVersion");
  if (!contract.actor) missing.push("actor");
  if (!contract.purpose) missing.push("purpose");
  if (!contract.phase) missing.push("phase");
  if (!contract.toolKind) missing.push("toolKind");
  if (!contract.sideEffect) missing.push("sideEffect");
  if (!contract.risk) missing.push("risk");
  if (!contract.idempotencyKey) missing.push("idempotencyKey");
  if (!contract.timeoutMs) missing.push("timeoutMs");
  if (missing.length > 0) {
    throw new Error("IPC contract missing fields, FAIL: " + missing.join(", "));
  }
  // Parity with validate_contract in
  // apps/desktop/src-tauri/src/commands/registry.rs: the frontend must
  // reject at least everything the backend rejects, so a bad contract
  // never reaches the backend. An illegal tool kind or side effect class
  // is rejected here, not left for the Rust side to catch.
  if (
    contract.toolKind !== "model-tool" &&
    contract.toolKind !== "registered-tool" &&
    contract.toolKind !== "authorized-execution-tool"
  ) {
    throw new Error("IPC contract illegal tool kind, FAIL");
  }
  if (
    contract.sideEffect !== "no-side-effect" &&
    contract.sideEffect !== "reversible" &&
    contract.sideEffect !== "irreversible"
  ) {
    throw new Error("IPC contract illegal side effect class, FAIL");
  }
}

export function requireRawLog(rawLogRef: string): void {
  // A1: every model call, including auxiliary calls, must have a raw log line.
  // Missing log means FAIL.
  if (!rawLogRef) {
    throw new Error("Missing raw log, FAIL per A1");
  }
}

// Args for one Tauri command: the full action contract plus the command's
// own parameters. Wire names are snake_case for the Rust parameters
// (session_id, text, tool_name); the contract object itself stays camelCase
// and the Rust side renames it with serde rename_all camelCase.
export interface BridgeArgs {
  contract: ActionContract;
  [key: string]: unknown;
}

export type BridgeTransport = (command: string, args: BridgeArgs) => Promise<unknown>;

const realTransport: BridgeTransport = (command, args) =>
  tauriInvoke(command, args);

let transport: BridgeTransport = realTransport;

// Test-only injection point. Pass null to restore the real Tauri invoke.
// Production code never calls this; tests use it to stand in for the Rust
// handlers, which cannot run under node.
export function setBridgeTransport(next: BridgeTransport | null): void {
  transport = next ?? realTransport;
}

export async function invoke(command: string, args: BridgeArgs): Promise<unknown> {
  assertContract(args.contract);
  return transport(command, args);
}
