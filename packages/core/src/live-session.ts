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
}

export interface LiveSession {
  manager: SessionManager;
  handle: SenAgentSession;
  sessionId: string;
  providerId: string;
  modelId: string;
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
  manager.create(options.sessionId);
  try {
    const runtime = await createOpenRouterRuntime({
      apiKey: options.apiKey,
      env: options.env,
    });
    const model = resolveOpenRouterModel(runtime, options.modelId);
    const handle = await createLiveSenAgentSession({
      modelRuntime: runtime,
      model,
      cwd: options.cwd,
      agentDir: options.agentDir,
    });
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
    };
  } catch (error) {
    try {
      manager.transition(options.sessionId, "Failed");
    } catch {
      // The transition can only fail if the state already left Running;
      // the original error is the one that matters, so it rethrows below.
    }
    throw error;
  }
}
