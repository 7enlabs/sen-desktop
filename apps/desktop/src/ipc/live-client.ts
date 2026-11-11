// Frontend live-session attach (live session round): binds a session
// record to the backend through the IPC bridge.
//
// SDK move (tin 9328/9459): the Pi SDK is out of the webview. There is
// no SDK handle here anymore — no factory, no handle.subscribe, no
// handle.dispose. The model runs in the backend through the CLI core +
// stream-json bridge (a later phase), and model events will flow from
// the backend (Tauri events) once that bridge lands. This file only
// creates the backend + local record and records the subscription.
//
// Layering, kept from the bridge round:
// - The IPC bridge is the only path to the backend: every backend call
//   below goes through ./session-client.ts, which validates the
//   contract before any transport call.
// - Tool execution stays refused: this file adds no tool path, and
//   callTool still propagates TOOL_EXECUTION_NOT_WIRED from the backend.
import {
  abortSession,
  createSession,
  subscribeSession,
} from "./session-client.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";

export interface AttachedLiveSession {
  sessionId: string;
  state: string;
  subscriberId: number;
}

export async function attachLiveSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
): Promise<AttachedLiveSession> {
  // Backend + local record first (createSession: backend ack, then the
  // local Running record, with mismatch refused), then the backend
  // subscription. A backend failure leaves no local record behind.
  const created = await createSession(manager, contract, sessionId);
  const sub = await subscribeSession(contract, sessionId);
  return {
    sessionId,
    state: created.state,
    subscriberId: sub.subscriberId,
  };
}

// Closes a live session: the backend + local Cancelled path (abortSession
// through the IPC bridge). No SDK abort or dispose exists anymore, so the
// old finally-return swallow (BUG-12) has no subject left.
export async function closeLiveSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
): Promise<unknown> {
  return abortSession(manager, contract, sessionId);
}
