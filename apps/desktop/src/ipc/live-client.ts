// Frontend live-session attach (live session round): binds a real SDK
// session to a session record through the IPC bridge, and streams model
// events to the caller.
//
// Layering, kept from the bridge round:
// - The SDK handle is created by the app through packages/core (see
//   live-session.ts) and injected here as a factory. This file never
//   imports the Pi SDK, so the frontend keeps its boundary: core owns
//   the SDK, the bridge owns transport, this client owns order + guards.
// - The IPC bridge is still the only path to the backend: every backend
//   call below goes through ./session-client.ts, which validates the
//   contract before any transport call.
// - Stream events flow through the SDK handle subscribe, not through a
//   new backend command: sen_events_subscribe records the subscriber on
//   the backend, and the event objects arrive through the handle.
//
// Order, and what each step proves:
// - Handle factory first: creating a handle makes no model call, and a
//   factory failure (missing key, missing model, unknown model) refuses
//   BEFORE any backend call and before any local record exists.
// - Backend + local record next (createSession): backend ack first,
//   then the local Running record, with mismatch refused as before.
// - Bind next: only the created handle is bound, so steer/followUp keep
//   refusing through the guard on every path that did not bind.
// - Backend subscribe next, then event subscribe: the caller receives
//   model events (message_start / message_update / message_end) through
//   onEvent, in the order the handle emits them.
// - Close (closeLiveSession): unsubscribe first, then the existing abort
//   path (backend Cancelled + SDK abort + dispose + local Cancelled).
//   The known abort edge from the bridge round is unchanged: if the SDK
//   abort throws after the backend already cancelled, the local transition
//   is skipped, and reconciling that still needs the status query command
//   from a later phase.
// - Tool execution stays refused: this file adds no tool path, and
//   callTool still propagates TOOL_EXECUTION_NOT_WIRED from the backend.
import {
  abortSession,
  createSession,
  subscribeSession,
} from "./session-client.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";
import type { SenAgentSession } from "../../../../packages/core/src/agent-session.ts";

export interface LiveAttachOptions {
  // Creates the SDK handle for this session id. The app passes a factory
  // built on packages/core startOpenRouterLiveSession; tests pass a fake.
  createHandle: (sessionId: string) => Promise<SenAgentSession>;
  // Receives model events in emission order. Events are passed through
  // untouched; this client never interprets, filters, or rewrites them.
  onEvent?: (event: unknown) => void;
}

export interface AttachedLiveSession {
  sessionId: string;
  state: string;
  subscriberId: number;
  handle: SenAgentSession;
  unsubscribe: () => void;
}

export async function attachLiveSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
  options: LiveAttachOptions,
): Promise<AttachedLiveSession> {
  // Factory first: a failure here leaves no backend call and no local
  // record, so the guard stays intact and nothing looks live that is not.
  const handle = await options.createHandle(sessionId);
  try {
    const created = await createSession(manager, contract, sessionId);
    manager.bindSdkSession(sessionId, handle);
    const sub = await subscribeSession(contract, sessionId);
    const unsubscribe = handle.subscribe((event) => {
      options.onEvent?.(event);
    });
    return {
      sessionId,
      state: created.state,
      subscriberId: sub.subscriberId,
      handle,
      unsubscribe,
    };
  } catch (error) {
    // A failure after the handle exists must not leak it: dispose on a
    // best-effort basis, then rethrow the original error untouched.
    try {
      handle.dispose();
    } catch {
      // Dispose is cleanup only; the original error is the one that matters.
    }
    throw error;
  }
}

// Closes a live session: stops the event stream first, then the existing
// abort path (backend Cancelled + SDK abort + dispose + local Cancelled).
export async function closeLiveSession(
  manager: SessionManager,
  attached: AttachedLiveSession,
  contract: ActionContract,
  sessionId: string,
): Promise<unknown> {
  try {
    attached.unsubscribe();
  } finally {
    return abortSession(manager, attached.handle, contract, sessionId);
  }
}
