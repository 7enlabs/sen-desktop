// App entry: the Tauri webview loads this module from index.html, so
// loading it starts the app. The UI shell is still a stub (no real UI
// yet, no fake content), but the entry now wires one real live session
// instead of holding an uncalled stub.
//
// Wiring (BUG-03 fix, stacked on the BUG-02 fix, issue #10):
// - One SessionManager owns the session, created here and shared by
//   both steps below. Start and attach never use different managers.
// - One ActionContract covers the session lifecycle, built here and
//   closed over by the backend hook.
// - startOpenRouterLiveSession runs with registerBackendSession:
//   () => createSession(manager, contract, sessionId), so the backend
//   record (sen_session_create) exists before the local record and the
//   bind, exactly the order the BUG-02 fix established. Core never sees
//   the contract or the transport; the hook closes over both here.
// - attachLiveSession receives the whole LiveSession from the factory,
//   so it sees backendRegistered and skips its own createSession
//   (calling it again would hit DUPLICATE_SESSION on the backend).
// - The model id is ALWAYS caller-supplied, never defaulted (message
//   4070): without one, start refuses before any backend call. The key
//   comes from an explicit caller-supplied apiKey or from the
//   OPENROUTER_API_KEY environment record inside packages/core; it is
//   never read from a file here, never stored, never logged.
// - No network call happens here: starting a live session only builds
//   the runtime, resolves the model, and binds the handle. The first
//   network step is a later steer/followUp on the attached session.
//
// Still open, NOT decided here (see the bundle README):
// - Vite bundles this module now (bundler round, tin 9313), so a real
//   webview can load the built dist/index.html. This wiring is proven
//   under node and by a successful vite build, not yet in a real webview
//   (no Rust toolchain on this box, so tauri dev/build is not run here).
// - No UI or config source supplies modelId/apiKey at runtime yet, so
//   the load-time boot below refuses with a clear message until one
//   exists. The refusal is surfaced in the shell, never swallowed.
import { attachLiveSession } from "../ipc/live-client.ts";
import type { AttachedLiveSession } from "../ipc/live-client.ts";
import { createSession } from "../ipc/session-client.ts";
import type { ActionContract } from "../ipc/types.ts";
import { SessionManager } from "../../../../packages/core/src/session.ts";
import {
  startOpenRouterLiveSession,
  type LiveSession,
} from "../../../../packages/core/src/live-session.ts";

export interface StartAppOptions {
  // Session id for the one app live session. Defaults to
  // "app-live-session"; there is no UI yet to open a second one.
  sessionId?: string;
  // Explicit OpenRouter model id. Required: never defaulted, never
  // fuzzy-matched. Without one, startApp refuses before any backend call.
  modelId?: string;
  // Explicit key. When omitted, packages/core resolves it from the
  // OPENROUTER_API_KEY environment record. Tests pass an explicit key.
  apiKey?: string;
  env?: Record<string, string | undefined>;
  cwd?: string;
  // Agent directory for the SDK session managers. The desktop app must
  // pass Sen's own directory at wiring time; tests pass a temp directory.
  agentDir?: string;
  // An existing manager to bind into. When omitted, the app creates one
  // and owns it for the whole session lifecycle.
  manager?: SessionManager;
  // Receives model events in emission order, passed through untouched.
  onEvent?: (event: unknown) => void;
}

export interface StartedApp {
  manager: SessionManager;
  live: LiveSession;
  attached: AttachedLiveSession;
}

// The one contract for the app live session lifecycle. The id and the
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
    timeoutMs: 30000,
    approvalRequired: false,
    auditRequired: false,
  };
}

// Starts the app: one live session, wired through the backend registry
// first (BUG-02 order), then attached to the same manager. Returns the
// manager, the live session, and the attached session, so the caller
// (later: the UI) can steer, follow up, and close it.
export async function startApp(
  options: StartAppOptions = {},
): Promise<StartedApp> {
  const sessionId = options.sessionId ?? "app-live-session";
  const manager = options.manager ?? new SessionManager();
  const contract = appLiveContract(sessionId);
  const live = await startOpenRouterLiveSession({
    sessionId,
    modelId: options.modelId ?? "",
    apiKey: options.apiKey,
    env: options.env,
    cwd: options.cwd,
    agentDir: options.agentDir,
    manager,
    registerBackendSession: (id) => createSession(manager, contract, id),
  });
  const attached = await attachLiveSession(manager, contract, sessionId, {
    createHandle: async () => live,
    onEvent: options.onEvent,
  });
  return { manager, live, attached };
}

// Boot: this module IS the app entry (index.html loads it), so loading
// it in a real webview starts the app. Under node (tests) there is no
// window, so nothing auto-starts there; tests call startApp directly.
// A boot failure is reported in the shell and on the console, never
// swallowed and never replaced with fake content: with no model id or
// key configured yet, the refusal below is the honest boot state.
if (typeof window !== "undefined") {
  startApp().catch((error: unknown) => {
    const message =
      error instanceof Error ? error.message : String(error);
    // eslint-disable-next-line no-console
    console.error("Sen failed to start a live session: " + message);
    const root = document.getElementById("app");
    if (root) {
      root.textContent = "Sen failed to start: " + message;
    }
  });
}
