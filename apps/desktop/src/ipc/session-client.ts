// Frontend session client (bridge round): drives a session through the IPC
// bridge to the Rust handlers, then through the local SessionManager and
// the packages/core wrapper. This is the frontend half of the round-trip;
// the backend half is apps/desktop/src-tauri/src/commands/.
//
// Order of truth, per phase:
// - create: backend record first, then the local SessionManager record.
//   No SDK session is created: no model runtime exists in this phase, by
//   design (no default model per message 4070, no key, no network call).
// - steer / follow_up: the no-default-model guard runs FIRST, in the
//   frontend, before any backend call. Without a SDK handle bound in the
//   SessionManager the call refuses with the same message family as the
//   core guard, and the transport is never called. With a handle: backend
//   gate ack, then the real SDK steer/followUp call on the handle.
//   BUG-06: if the SDK call throws after the backend ack, the local
//   session transitions to Failed and the SDK error is rethrown, so the
//   failure is recorded locally instead of leaving local Running while
//   the backend log already holds the steer intent. The backend log
//   entry stays: it records the intent, and an ack never implied a
//   model call happened. The remaining backend-Running vs local-Failed
//   gap still needs a status query command, which belongs to a later
//   phase.
// - abort: backend transition first (the record), then the SDK abort +
//   dispose when a handle is bound, then the local state transition.
//   BUG-05: if the SDK abort throws after the backend already
//   cancelled, dispose still runs and the local transition still runs,
//   so local ends Cancelled like the backend and no new divergence is
//   created; the SDK error is then rethrown so the caller still learns
//   the abort was not clean. There is still no status query command to
//   reconcile a divergence that predates this fix, which belongs to a
//   later phase.
// - tool_call: the backend validates + gates, then refuses with
//   TOOL_EXECUTION_NOT_WIRED (gate execute step + permission layer are
//   later phases). The error propagates, never a fake success.
// - events_subscribe: backend records the subscriber; the stream itself
//   flows through the frontend SDK handle when bound.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";
import type { SenAgentSession } from "../../../../packages/core/src/agent-session.ts";

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

function requireBoundHandle(
  manager: SessionManager,
  handle: SenAgentSession | undefined,
  sessionId: string,
): SenAgentSession {
  if (!handle) {
    throw new Error(
      "No model channel attached: refusing to steer/follow-up without a " +
        "bound SDK session (no default model per message 4070; the " +
        "transport is never called on this path).",
    );
  }
  if (manager.getSdkSession(sessionId) !== handle) {
    throw new Error(
      "Session handle not bound in SessionManager, FAIL: bind it with " +
        "bindSdkSession before steering.",
    );
  }
  return handle;
}

export async function steerSession(
  manager: SessionManager,
  handle: SenAgentSession | undefined,
  contract: ActionContract,
  sessionId: string,
  text: string,
): Promise<BackendSessionResult> {
  const bound = requireBoundHandle(manager, handle, sessionId);
  const raw = await invoke("sen_session_steer", {
    contract,
    session_id: sessionId,
    text,
  });
  const res = asSessionResult(raw, sessionId);
  await runSdkCall(manager, sessionId, () => bound.steer(text));
  return res;
}

export async function followUpSession(
  manager: SessionManager,
  handle: SenAgentSession | undefined,
  contract: ActionContract,
  sessionId: string,
  text: string,
): Promise<BackendSessionResult> {
  const bound = requireBoundHandle(manager, handle, sessionId);
  const raw = await invoke("sen_session_follow_up", {
    contract,
    session_id: sessionId,
    text,
  });
  const res = asSessionResult(raw, sessionId);
  await runSdkCall(manager, sessionId, () => bound.followUp(text));
  return res;
}

// BUG-06: the backend acked the steer/follow-up (its log already holds
// the intent) before the SDK call ran, so only this side knows the model
// call failed. When it throws, move the local session to Failed - a
// legal Running -> Failed transition per the point-854 statechart - and
// rethrow the original SDK error, never a replacement. The caller still
// learns the steer did not run, and the local record no longer pretends
// the session is healthy Running. A transition error (local already
// terminal from a divergence that predates this fix) must not mask the
// SDK error: the session is already recorded terminal, so the SDK error
// stays the reported one.
async function runSdkCall(
  manager: SessionManager,
  sessionId: string,
  run: () => Promise<void>,
): Promise<void> {
  try {
    await run();
  } catch (error) {
    try {
      manager.transition(sessionId, "Failed");
    } catch {
      // Local already terminal: nothing new to record, keep the SDK error.
    }
    throw error;
  }
}

export async function abortSession(
  manager: SessionManager,
  handle: SenAgentSession | undefined,
  contract: ActionContract,
  sessionId: string,
): Promise<BackendSessionResult> {
  const raw = await invoke("sen_session_abort", {
    contract,
    session_id: sessionId,
  });
  const res = asSessionResult(raw, sessionId);
  // BUG-05: the backend already said Cancelled, so a throwing SDK abort
  // must not skip the cleanup that follows it. Run dispose even when
  // abort throws, run the local transition even when either throws, and
  // only then rethrow the SDK error. The caller still learns the abort
  // was not clean (the error is not swallowed), but local state ends
  // Cancelled, matching the backend, instead of staying Running with a
  // leaked handle. No backend state is invented here: the Cancelled
  // comes from the backend ack above, not from guessing.
  let sdkError: unknown;
  let hasSdkError = false;
  if (handle) {
    try {
      await handle.abort();
    } catch (error) {
      sdkError = error;
      hasSdkError = true;
    }
    try {
      handle.dispose();
    } catch (error) {
      if (!hasSdkError) {
        sdkError = error;
        hasSdkError = true;
      }
    }
  }
  const local = manager.transition(sessionId, "Cancelled");
  if (local !== res.state) {
    throw new Error(
      "Backend/local session state mismatch, FAIL: backend " +
        res.state +
        ", local " +
        local,
    );
  }
  if (hasSdkError) {
    throw sdkError;
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
