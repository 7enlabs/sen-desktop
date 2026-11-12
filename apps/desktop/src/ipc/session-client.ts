// Frontend session client (bridge round): drives a session through the IPC
// bridge to the Rust handlers, then through the local SessionManager.
// This is the frontend half of the round-trip; the backend half is
// apps/desktop/src-tauri/src/commands/.
//
// SDK move (tin 9328/9459): the Pi SDK is out of the webview. No SDK
// session is created, bound, steered, or disposed anywhere in this file:
// the model runs in the backend through the CLI core + stream-json bridge
// (Phase 2, tin 9530), and the frontend only talks to the backend through
// the IPC bridge. A backend failure surfaces here untouched — never
// mapped to a fake success.
//
// Order of truth, per phase:
// - create: backend record first, then the local SessionManager record.
//   The session's model comes from local app settings (default free
//   model when nothing is stored); no key is held anywhere here — the key
//   reaches the CLI through the environment on the backend side.
// - steer / follow_up: pure IPC. The backend gates on a Running session
//   and runs one CLI turn for it; the backend ack means the model call
//   ran to a result record. A failure throws above, so no local state is
//   recorded for a steer that never ran.
// - abort: backend transition first (the record), then the local state
//   transition. No SDK abort or dispose exists anymore, so the BUG-05
//   failure mode (SDK abort throws after the backend already cancelled)
//   has no subject left; abort is a pure state transition on both sides.
// - tool_call: the backend validates + gates, then refuses with
//   TOOL_EXECUTION_NOT_WIRED (gate execute step + permission layer are
//   later phases). The error propagates, never a fake success.
// - events_subscribe: backend records the subscriber; the event stream
//   flows from the backend (Tauri events) while a CLI turn runs — see
//   listenSessionEvents in ./session-events.ts. This call records the
//   subscription only, it invents no events.
// - events_unsubscribe (BUG-09): backend drops one subscriber and
//   returns the remaining count; close calls it before abort, and abort
//   itself resets the count to 0.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";

export interface BackendSessionResult {
  sessionId: string;
  state: string;
  // R2B-04: counts from the last turn the backend ran (0 when no turn
  // ran: create/abort). The backend always sends both numbers.
  unknownRecords: number;
  versionMismatches: number;
}

export interface BackendSubscribeResult {
  sessionId: string;
  subscriberId: number;
}

export interface BackendUnsubscribeResult {
  sessionId: string;
  subscriberCount: number;
}

function asSessionResult(value: unknown, expectedId: string): BackendSessionResult {
  const res = value as BackendSessionResult | null;
  if (
    !res ||
    typeof res.sessionId !== "string" ||
    typeof res.state !== "string" ||
    typeof res.unknownRecords !== "number" ||
    typeof res.versionMismatches !== "number"
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
  // The backend ack means the model call ran to a result record. A
  // failure throws above, so no local state is recorded for a steer that
  // never ran.
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
  // R2B-10: refuse before touching the backend when this manager holds
  // no record for the session. Without this check the backend would
  // cancel the session while its real owner still holds it Running,
  // and the local transition below would throw "Session does not exist"
  // after the backend already moved. The message keeps the
  // transition() wording so callers see the same refusal.
  if (!manager.hasSession(sessionId)) {
    throw new Error(
      "Session does not exist: " +
        sessionId +
        " (abort refused before backend call)",
    );
  }
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

function asUnsubscribeResult(
  value: unknown,
  expectedId: string,
): BackendUnsubscribeResult {
  const res = value as BackendUnsubscribeResult | null;
  if (
    !res ||
    typeof res.sessionId !== "string" ||
    typeof res.subscriberCount !== "number"
  ) {
    throw new Error(
      "Backend unsubscribe result malformed, FAIL: " + JSON.stringify(value),
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

// BUG-09 (#24): drops one backend subscriber, returns the remaining
// count. The backend count saturates at 0, so an extra unsubscribe is
// a harmless no-op, never an underflow.
export async function unsubscribeSession(
  contract: ActionContract,
  sessionId: string,
): Promise<BackendUnsubscribeResult> {
  const raw = await invoke("sen_events_unsubscribe", {
    contract,
    session_id: sessionId,
  });
  return asUnsubscribeResult(raw, sessionId);
}

export async function callTool(
  contract: ActionContract,
  toolName: string,
): Promise<unknown> {
  return invoke("sen_tool_call", { contract, tool_name: toolName });
}
