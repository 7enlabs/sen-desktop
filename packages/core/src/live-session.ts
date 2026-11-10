// Live session orchestration for the Sen Desktop live session round.
// Wires the local SessionManager to a real SDK session on the OpenRouter
// free channel (tin 4528/4542), then binds the handle. Only packages/core
// may import the Pi SDK, per prep B-2 section 3 point 4.
//
// Order, and what each step proves:
// - Local record first (SessionManager.create): the session exists in the
//   statechart as Running before any SDK call, same order as the bridge
//   round (backend record, then local record, then bind).
// - Runtime + explicit model next (./model-channel.ts): missing key,
//   empty model id, or unknown model id refuses here, before any SDK
//   session exists and before any network call.
// - SDK session next (./agent-session.ts, noTools "all"): creation makes
//   no model call. The first network step is a later steer/followUp.
// - Bind last: only a created handle is bound, so steer/followUp keep
//   refusing through the guard on every path that did not bind.
// - The raw-log payload carries provider + model id only. The key is
//   never logged, never stored, never returned.
// - On failure after the local record exists, the local state moves to
//   Failed (an allowed Running transition) and the error rethrows
//   untouched. A failed live start therefore never leaves a fake
//   Running-with-handle record behind, and the session id is terminal,
//   so it cannot be silently reused.
// - BUG-02 fix: the core-only order above never touched the backend, so
//   a live session was unknown to the backend registry and a later
//   sen_session_steer answered UNKNOWN_SESSION. The desktop layer now
//   passes registerBackendSession (see StartLiveSessionOptions): the
//   hook runs after the handle exists and BEFORE the bind, and it
//   creates the backend record first, then the local record in this
//   same manager. Core never sees the contract or the transport; the
//   hook closes over both on the desktop side. A failure before the
//   hook leaves no record anywhere; a failure after the handle exists
//   disposes it. One manager owns the session: the hook and the bind
//   both use options.manager, and attachLiveSession refuses a live
//   session whose manager is a different instance.
// - The duplicate-id guard below is local to this live path only:
//   SessionManager.create still overwrites silently (BUG-04, NOT fixed
//   here), so this file checks hasSession first and refuses before any
//   SDK work or backend call.
// - Tool execution stays refused: noTools "all" at creation, and the
//   gate execute step + permission layer are later phases (see
//   ./execution-gate.ts). A live session can chat and stream; it cannot
//   run a tool for real.
import {
  createLiveSenAgentSession,
  type SenAgentSession,
} from "./agent-session.ts";
import {
  OPENROUTER_PROVIDER_ID,
  createOpenRouterRuntime,
  resolveOpenRouterModel,
} from "./model-channel.ts";
import { SessionManager } from "./session.ts";

export interface StartLiveSessionOptions {
  sessionId: string;
  // Explicit OpenRouter model id, caller-supplied. Never defaulted.
  modelId: string;
  // Explicit key. When omitted, the key is resolved from the environment
  // record (process.env by default). Tests pass an explicit key.
  apiKey?: string;
  env?: Record<string, string | undefined>;
  cwd?: string;
  // Agent directory for the SDK session managers. The desktop app must
  // pass Sen's own directory at wiring time; tests pass a temp directory.
  agentDir?: string;
  // An existing manager to bind into. When omitted, a new one is created.
  manager?: SessionManager;
  // Backend registration hook, supplied by the desktop layer. The app
  // passes () => createSession(manager, contract, sessionId) from
  // apps/desktop/src/ipc/session-client.ts: backend ack through
  // sen_session_create first, then the local Running record in this
  // same manager. When omitted, the old core-only order stays (local
  // record first, no backend call), for core-only use and old tests.
  registerBackendSession?: (sessionId: string) => Promise<void>;
}

export interface LiveSession {
  manager: SessionManager;
  handle: SenAgentSession;
  sessionId: string;
  providerId: string;
  modelId: string;
  // True only when registerBackendSession ran and the backend record
  // exists. attachLiveSession skips its own createSession on true.
  backendRegistered: boolean;
}

export async function startOpenRouterLiveSession(
  options: StartLiveSessionOptions,
): Promise<LiveSession> {
  if (!options || !options.sessionId) {
    throw new Error("Live session needs a session id.");
  }
  if (!options.modelId || !options.modelId.trim()) {
    throw new Error(
      "No model specified: refusing to start a live session without an " +
        "explicit OpenRouter model id (no default model per message 4070).",
    );
  }
  const manager = options.manager ?? new SessionManager();
  // Refuse a reused id before any SDK work or backend call. The check
  // is local to this live path: SessionManager.create still overwrites
  // silently (BUG-04, NOT fixed here), so the guard lives here.
  if (manager.hasSession(options.sessionId)) {
    throw new Error(
      "Duplicate live session: manager already owns session " +
        options.sessionId +
        " (refusing before any SDK work or backend call).",
    );
  }
  const withBackend =
    typeof options.registerBackendSession === "function";
  // Core-only order stays when no hook is given: local record first,
  // then the handle, then the bind. With a hook, the local record is
  // created inside the hook (backend ack first, then local record),
  // after the handle exists and before the bind, so a handle failure
  // leaves no record anywhere.
  if (!withBackend) {
    manager.create(options.sessionId);
  }
  let handle: SenAgentSession | undefined;
  try {
    const runtime = await createOpenRouterRuntime({
      apiKey: options.apiKey,
      env: options.env,
    });
    const model = resolveOpenRouterModel(runtime, options.modelId);
    handle = await createLiveSenAgentSession({
      modelRuntime: runtime,
      model,
      cwd: options.cwd,
      agentDir: options.agentDir,
    });
    if (withBackend) {
      await options.registerBackendSession!(options.sessionId);
    }
    manager.bindSdkSession(options.sessionId, handle);
    manager.rawLog.append(options.sessionId, "session_live_started", {
      provider: OPENROUTER_PROVIDER_ID,
      model: options.modelId,
      note: "Live SDK session bound; key never logged",
    });
    return {
      manager,
      handle,
      sessionId: options.sessionId,
      providerId: OPENROUTER_PROVIDER_ID,
      modelId: options.modelId,
      backendRegistered: withBackend,
    };
  } catch (error) {
    if (handle) {
      try {
        handle.dispose();
      } catch {
        // Dispose is cleanup only; the original error rethrows below.
      }
    }
    try {
      manager.transition(options.sessionId, "Failed");
    } catch {
      // The transition can only fail if no local record exists yet
      // (hook path, failure before the hook) or the state already left
      // Running; the original error is the one that matters.
    }
    throw error;
  }
}
