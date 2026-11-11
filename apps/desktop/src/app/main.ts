// App entry: the Tauri webview loads this module from index.html, so
// loading it starts the app. The UI shell is still a stub (no real UI
// yet, no fake content), but the entry wires one real session through
// the backend instead of holding an uncalled stub.
//
// Wiring (SDK move, tin 9328/9459: the Pi SDK is out of the webview):
// - One SessionManager owns the session, created here and shared by
//   both steps below. Start and attach never use different managers.
// - One ActionContract covers the session lifecycle, built here and
//   closed over by both IPC calls.
// - The session is created through the backend only: createSession
//   (sen_session_create) makes the backend record first, then the local
//   record in this manager, exactly the order the BUG-02 fix established.
//   No SDK session is created anywhere in the webview: the Pi SDK
//   (createAgentSession, ModelRuntime) cannot run in a browser, and the
//   vite build failed on it (Rollup: "spawn" is not exported). The model
//   runs in the backend through the CLI core + stream-json bridge
//   (Phase 2, tin 9530): steer/followUp run the `zen` CLI for the
//   session, and model events flow from the backend as Tauri events
//   (sen-session-event) while a turn runs.
// - subscribeSession records the subscriber on the backend. Model events
//   flow from the backend (Tauri events) while a CLI turn runs; the
//   listener is listenSessionEvents in ../ipc/session-events.ts.
// - No model id, no key, no network call is configured here: the entry
//   names no model and holds no key. The model comes from local app
//   settings (sen_settings_get_model, default free model when nothing is
//   stored); the key reaches the CLI through the environment on the
//   backend side, never through the webview.
//
// Still open, NOT decided here:
// - This wiring is proven under node (tests) and by a successful vite
//   build after the SDK move, not yet in a real webview (no Rust
//   toolchain on this box, so tauri dev/build is not run here).
// - No UI supplies a model id at runtime yet; the model comes from local
//   app settings (sen_settings_set_model), default free model when
//   nothing is stored. There is no settings screen in this phase.
import { attachLiveSession } from "../ipc/live-client.ts";
import type { AttachedLiveSession } from "../ipc/live-client.ts";
import type { ActionContract } from "../ipc/types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";

export interface StartAppOptions {
  // Session id for the one app session. Defaults to
  // "app-live-session"; there is no UI yet to open a second one.
  sessionId?: string;
  // An existing manager to bind into. When omitted, the app creates one
  // and owns it for the whole session lifecycle.
  manager?: SessionManager;
}

export interface StartedApp {
  manager: SessionManager;
  sessionId: string;
  state: string;
  attached: AttachedLiveSession;
}

// The one contract for the app session lifecycle. The id and the
// idempotency key are unique per start, so a second app start never
// reuses the first start's key.
function appLiveContract(sessionId: string): ActionContract {
  const nonce = sessionId + "-" + Date.now();
  return {
    id: "app-live-" + nonce,
    capability: "sen.session",
    capabilityVersion: "1",
    actor: "app",
    purpose: "app live session",
    phase: "live",
    toolKind: "registered-tool",
    sideEffect: "no-side-effect",
    risk: "low",
    idempotencyKey: "app-live-" + nonce,
    timeoutMs: 120000,
    approvalRequired: false,
    auditRequired: false,
  };
}

// Starts the app: one session, wired through the backend registry
// first, then attached on the same manager. A second start on the same
// manager and session id refuses before any backend call
// (DUPLICATE_SESSION). Returns the manager, the session id, its state,
// and the attached session, so the caller (later: the UI) can steer,
// follow up, and close it once the backend runs the model through the
// CLI bridge.
export async function startApp(
  options: StartAppOptions = {},
): Promise<StartedApp> {
  const sessionId = options.sessionId ?? "app-live-session";
  const manager = options.manager ?? new SessionManager();
  if (manager.hasSession(sessionId)) {
    throw new Error("DUPLICATE_SESSION: Session already exists: " + sessionId);
  }
  const contract = appLiveContract(sessionId);
  // attachLiveSession creates the session (backend ack, then the local
  // record) and subscribes on the same manager; startApp does not create
  // it separately, or the second create would hit DUPLICATE_SESSION.
  const attached = await attachLiveSession(manager, contract, sessionId);
  return {
    manager,
    sessionId,
    state: attached.state,
    attached,
  };
}

// Boot: this module IS the app entry (index.html loads it), so loading
// it in a real webview starts the app. Under node (tests) there is no
// window, so nothing auto-starts there; tests call startApp directly.
// A boot failure is reported in the shell and on the console, never
// swallowed and never replaced with fake content.
if (typeof window !== "undefined") {
  startApp().catch((error: unknown) => {
    const message =
      error instanceof Error ? error.message : String(error);
    // eslint-disable-next-line no-console
    console.error("Sen failed to start a session: " + message);
    const root = document.getElementById("app");
    if (root) {
      root.textContent = "Sen failed to start: " + message;
    }
  });
}
