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
// own parameters. Wire names are camelCase: Tauri v2 maps each Rust
// parameter to its lowerCamelCase wire key (session_id -> sessionId,
// tool_name -> toolName, text -> text), so the frontend must send
// sessionId / toolName, never the Rust identifiers. The contract object
// itself is camelCase too, and the Rust side renames it with serde
// rename_all camelCase (ContractArgs). Sending session_id was the Sen
// v0.1.0 bug: Tauri waited for sessionId and failed every session call
// with "missing required key sessionId".
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
  // P1-2: enforce contract.timeoutMs. Before this fix invoke() returned
  // the transport promise bare, so a hung backend call hung forever and
  // leaked: the chat waiter timed out and rejected, but the steer
  // promise stayed in-flight, and the next turn on the same session
  // then failed with CLI_TURN_IN_PROGRESS. The timeout rejects here;
  // the timer is always cleared when the transport settles first, and
  // a late transport rejection is swallowed so it never surfaces as an
  // unhandled rejection after the timeout already won the race.
  // The transport itself cannot be cancelled (Tauri invoke takes no
  // AbortSignal), so this bounds the frontend wait only; the backend
  // turn cap is a separate concern (Rust side).
  const timeoutMs = args.contract.timeoutMs;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      reject(
        new Error(
          `IPC_TIMEOUT: ${command} did not respond within ${timeoutMs}ms (contract ${args.contract.id})`,
        ),
      );
    }, timeoutMs);
  });
  const result = transport(command, args);
  result.catch(() => undefined);
  try {
    return await Promise.race([result, timeout]);
  } finally {
    clearTimeout(timer);
  }
}
