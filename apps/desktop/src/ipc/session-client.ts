// Frontend session client (bridge round): drives a session through the IPC
// bridge to the Rust handlers, then through the local SessionManager.
// This is the frontend half of the round-trip; the backend half is
// apps/desktop/src-tauri/src/commands/.
//
// SDK move (tin 9328/9459): the Pi SDK is out of the webview. No SDK
// session is created, bound, steered, or disposed anywhere in this file:
// the model runs in the backend through the CLI core + stream-json bridge
// (a later phase), and the frontend only talks to the backend through
// the IPC bridge. Until the CLI bridge lands, steer/followUp are refused
// by the backend with MODEL_NOT_WIRED, and that refusal surfaces here
// untouched — never mapped to a fake success.
//
// Order of truth, per phase:
// - create: backend record first, then the local SessionManager record.
//   No model runtime exists in this phase, by design (no default model
//   per message 4070, no key, no network call).
// - steer / follow_up: pure IPC. The backend gates on a Running session
//   and either runs the model (once the CLI bridge lands) or refuses
//   with MODEL_NOT_WIRED. The frontend records nothing locally on a
//   refusal: the local state stays Running and the backend log holds no
//   steer that never ran, so the two sides cannot diverge the way
//   BUG-06 described (backend ack without a model call).
// - abort: backend transition first (the record), then the local state
//   transition. No SDK abort or dispose exists anymore, so the BUG-05
//   failure mode (SDK abort throws after the backend already cancelled)
//   has no subject left; abort is a pure state transition on both sides.
// - tool_call: the backend validates + gates, then refuses with
//   TOOL_EXECUTION_NOT_WIRED (gate execute step + permission layer are
//   later phases). The error propagates, never a fake success.
// - events_subscribe: backend records the subscriber; the event stream
//   itself will flow from the backend (Tauri events) once the CLI bridge
//   lands. This call records the subscription only, it invents no events.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";

export interface BackendSessionResult {
  sessionId: string;
  state: string;
}

export interface BackendSubscribeResult {
  sessionId: string;
  subscriberId: number;
}

function asSessionResult(value: unknown, expectedId: string): BackendSessionResult {
  const res = value as BackendSessionResult | null;
  if (
    !res ||
    typeof res.sessionId !== "string" ||
    typeof res.state !== "string"
  ) {
    throw new Error(
      "Backend session result malformed, FAIL: " + JSON.stringify(value),
    );
  }
  if (res.sessionId !== expectedId) {
    throw new Error(
      "Backend session id mismatch, FAIL: expected " +
        expectedId +
        ", got " +
        res.sessionId,
    );
  }
  return res;
}

function asSubscribeResult(
  value: unknown,
  expectedId: string,
): BackendSubscribeResult {
  const res = value as BackendSubscribeResult | null;
  if (
    !res ||
    typeof res.sessionId !== "string" ||
    typeof res.subscriberId !== "number"
  ) {
    throw new Error(
      "Backend subscribe result malformed, FAIL: " + JSON.stringify(value),
    );
  }
  if (res.sessionId !== expectedId) {
    throw new Error(
      "Backend session id mismatch, FAIL: expected " +
        expectedId +
        ", got " +
        res.sessionId,
    );
  }
  return res;
}

export async function createSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
): Promise<BackendSessionResult> {
  const raw = await invoke("sen_session_create", {
    contract,
    session_id: sessionId,
  });
  const res = asSessionResult(raw, sessionId);
  const local = manager.create(sessionId);
  if (local !== res.state) {
    throw new Error(
      "Backend/local session state mismatch, FAIL: backend " +
        res.state +
        ", local " +
        local,
    );
  }
  return res;
}

export async function steerSession(
  contract: ActionContract,
  sessionId: string,
  text: string,
): Promise<BackendSessionResult> {
  const raw = await invoke("sen_session_steer", {
    contract,
    session_id: sessionId,
    text,
  });
  const res = asSessionResult(raw, sessionId);
  // The backend ack means the model call ran (once the CLI bridge
  // lands). A refusal (MODEL_NOT_WIRED until then) throws above, so no
  // local state is recorded for a steer that never ran.
  return res;
}

export async function followUpSession(
  contract: ActionContract,
  sessionId: string,
  text: string,
): Promise<BackendSessionResult> {
  const raw = await invoke("sen_session_follow_up", {
    contract,
    session_id: sessionId,
    text,
  });
  const res = asSessionResult(raw, sessionId);
  return res;
}

export async function abortSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
): Promise<BackendSessionResult> {
  const raw = await invoke("sen_session_abort", {
    contract,
    session_id: sessionId,
  });
  const res = asSessionResult(raw, sessionId);
  // Backend Cancelled first, then the local transition. No SDK abort or
  // dispose exists anymore, so nothing here can throw after the backend
  // ack and leave the two sides apart.
  const local = manager.transition(sessionId, "Cancelled");
  if (local !== res.state) {
    throw new Error(
      "Backend/local session state mismatch, FAIL: backend " +
        res.state +
        ", local " +
        local,
    );
  }
  return res;
}

export async function subscribeSession(
  contract: ActionContract,
  sessionId: string,
): Promise<BackendSubscribeResult> {
  const raw = await invoke("sen_events_subscribe", {
    contract,
    session_id: sessionId,
  });
  return asSubscribeResult(raw, sessionId);
}

export async function callTool(
  contract: ActionContract,
  toolName: string,
): Promise<unknown> {
  return invoke("sen_tool_call", { contract, tool_name: toolName });
}
