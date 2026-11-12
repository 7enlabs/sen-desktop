// Frontend live-session attach (live session round): binds a session
// record to the backend through the IPC bridge.
//
// SDK move (tin 9328/9459): the Pi SDK is out of the webview. There is
// no SDK handle here anymore — no factory, no handle.subscribe, no
// handle.dispose. The model runs in the backend through the CLI core +
// stream-json bridge (a later phase), and model events flow from the
// backend (Tauri events) once that bridge lands. This file creates the
// backend + local record, records the subscription, and installs the
// frontend listener for those events (R2-16 / #65): the backend
// subscription alone delivered nothing to the app, so attach also
// listens on sen-session-event and records each validated event in the
// session's raw log.
//
// Layering, kept from the bridge round:
// - The IPC bridge is the only path to the backend: every backend call
//   below goes through ./session-client.ts, which validates the
//   contract before any transport call.
// - Tool execution stays refused: this file adds no tool path, and
//   callTool still propagates TOOL_EXECUTION_NOT_WIRED from the backend.
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  abortSession,
  createSession,
  subscribeSession,
  unsubscribeSession,
} from "./session-client.ts";
import {
  listenSessionEvents,
  type SenEvent,
} from "./session-events.ts";
import type { ActionContract } from "./types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";

export interface AttachedLiveSession {
  sessionId: string;
  state: string;
  subscriberId: number;
  // Stops the frontend event listener installed by attach. The caller
  // owns it: pass it to closeLiveSession when the session closes, so no
  // listener leaks past the session (like the backend subscriber,
  // BUG-09). Null when no listener was installed.
  unlisten: UnlistenFn | null;
}

export interface AttachLiveSessionOptions {
  // Optional extra sink for validated events, in addition to the
  // session record below. The listener is installed whether or not this
  // is given: the app must receive the backend's events, not only
  // subscribe on the backend (R2-16 / #65).
  onEvent?: (event: SenEvent) => void;
}

export async function attachLiveSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
  options: AttachLiveSessionOptions = {},
): Promise<AttachedLiveSession> {
  // Listen first, before any backend call: a session that cannot
  // receive events is broken, and listening first keeps attach
  // all-or-nothing — a listen failure leaves no backend session, no
  // subscription and no local record behind to roll back. No event can
  // arrive in the gap: the backend only emits during a turn, and a turn
  // cannot run before this session is created and subscribed below.
  // Each validated event is recorded in the session's raw log (kind
  // session_event), in arrival order, so the app receives the CLI's
  // system/assistant/user/result events (R2-16 / #65); the optional
  // onEvent gets the same validated events. listenSessionEvents
  // validates the variant + fields, so an unknown or malformed payload
  // never reaches the record or onEvent.
  const unlisten: UnlistenFn = await listenSessionEvents(sessionId, (event) => {
    manager.rawLog.append(sessionId, "session_event", event);
    options.onEvent?.(event);
  });
  try {
    // Backend + local record (createSession: backend ack, then the
    // local Running record, with mismatch refused), then the backend
    // subscription. A backend failure leaves no local record behind.
    const created = await createSession(manager, contract, sessionId);
    const sub = await subscribeSession(contract, sessionId);
    return {
      sessionId,
      state: created.state,
      subscriberId: sub.subscriberId,
      unlisten,
    };
  } catch (error) {
    // Create or subscribe failed after the listener was installed:
    // drop the listener so no orphan listens past the failed attach.
    if (unlisten) {
      unlisten();
    }
    throw error;
  }
}

// Closes a live session: drop the backend subscriber first (BUG-09:
// the backend count used to leak, because close never unsubscribed),
// then the backend + local Cancelled path (abortSession through the
// IPC bridge). Abort itself also resets the count to 0, so a direct
// abort that bypasses this path leaks nothing either. No SDK abort or
// dispose exists anymore, so the old finally-return swallow (BUG-12)
// has no subject left.
// V-04 (base 7158fe1): the join point is the 4th parameter, unlisten.
// No production caller passes it yet — the only callers are tests —
// because no UI/window close path exists on this base. When the UI
// builds one, it passes StartedApp.unlisten here; the order below
// (unlisten, unsubscribe, abort) is locked by the V-04 test in
// live-client.test.ts.
export async function closeLiveSession(
  manager: SessionManager,
  contract: ActionContract,
  sessionId: string,
  unlisten?: UnlistenFn | null,
): Promise<unknown> {
  // Stop receiving first, so no event is delivered after the session
  // closes; then drop the backend subscriber (BUG-09: the backend
  // count used to leak, because close never unsubscribed), then the
  // backend + local Cancelled path (abortSession through the IPC
  // bridge).
  if (unlisten) {
    unlisten();
  }
  await unsubscribeSession(contract, sessionId);
  return abortSession(manager, contract, sessionId);
}
