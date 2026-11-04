// Real Pi SDK wiring for the Sen Desktop fill-in round (messages 4128/4138).
// Only packages/core may import the Pi SDK, per prep B-2 section 3 point 4.
// This module imports createAgentSession from @earendil-works/pi-coding-agent
// for real. Verified in the install round: pi-coding-agent 1.5.0 (MIT, tin 4178 bump)
// exports createAgentSession as a function; pi-agent-core 1.5.0 does NOT
// export it (re-checked in the installed 1.5.0 types), so the import
// must come from pi-coding-agent.
// NO DEFAULT MODEL: per message 4070 the frame has no default model, no key,
// no network call. The SDK's own defaults would resolve a model from the
// user's ~/.pi/agent settings and auth files, so this wrapper REFUSES to call
// the SDK without an explicitly provided model runtime. LIVE SESSION ROUND
// (tin 4528/4542): the free channel decision is now OpenRouter free; the
// channel is built in ./model-channel.ts (in-memory credential store, key
// only from the OPENROUTER_API_KEY environment variable) and live sessions
// are started in ./live-session.ts. The guard below is unchanged, and a
// live session additionally requires an explicitly resolved model: no
// runtime, no model, no session.
// Session start default: no tools enabled (noTools "all"). Tools attach later
// explicitly through the execution gate, never at session creation.
import { createAgentSession } from "@earendil-works/pi-coding-agent";
import type {
  AgentSession,
  AgentSessionEventListener,
  ModelRuntime,
} from "@earendil-works/pi-coding-agent";

export const SDK_PACKAGE = "@earendil-works/pi-coding-agent";
export const SDK_PINNED_VERSION = "1.5.0";

// Proves the real import resolves, without calling the SDK, without a key,
// without a network call. Throws if the import ever stops being a function.
export function probeSdkWiring(): {
  sdkPackage: string;
  sdkVersion: string;
  createAgentSession: string;
} {
  if (typeof createAgentSession !== "function") {
    throw new Error("SDK wiring broken: createAgentSession is not a function.");
  }
  return {
    sdkPackage: SDK_PACKAGE,
    sdkVersion: SDK_PINNED_VERSION,
    createAgentSession: "function",
  };
}

export interface SenAgentSessionOptions {
  modelRuntime: ModelRuntime;
  cwd?: string;
}

// Creates a real SDK session ONLY with an explicitly provided model runtime.
// Without one, throws instead of letting the SDK silently resolve a model from
// user settings or auth files. No live session exists until a caller supplies
// a runtime; this round supplies none, by design.
export async function createSenAgentSession(
  options: SenAgentSessionOptions,
): Promise<SenAgentSession> {
  if (!options || !options.modelRuntime) {
    throw new Error(
      "No model channel attached: refusing to call createAgentSession " +
        "without an explicitly provided model runtime (no default model per " +
        "message 4070; SDK defaults would read user settings/auth files).",
    );
  }
  const result = await createAgentSession({
    modelRuntime: options.modelRuntime,
    cwd: options.cwd,
    noTools: "all",
  });
  return new SenAgentSession(result.session);
}

export interface LiveSenAgentSessionOptions {
  modelRuntime: ModelRuntime;
  // An SDK catalog model resolved explicitly (see ./model-channel.ts).
  // Unknown to this module by id on purpose: this wrapper never picks
  // one, it only refuses when none is given.
  model: unknown;
  cwd?: string;
  // Agent directory for the SDK session managers. When omitted, the SDK
  // default applies; the desktop app must pass Sen's own directory at
  // wiring time so a live session never depends on Pi defaults.
  agentDir?: string;
}

// Creates a real live SDK session ONLY with an explicitly provided model
// runtime AND an explicitly resolved model. Missing either refuses, so
// the SDK can never silently resolve a model from settings, auth files,
// or a first-available list. No tools are enabled at creation
// (noTools "all"); the gate execute step and permission layer are later
// phases, so no tool runs for real even on a live session.
export async function createLiveSenAgentSession(
  options: LiveSenAgentSessionOptions,
): Promise<SenAgentSession> {
  if (!options || !options.modelRuntime) {
    throw new Error(
      "No model channel attached: refusing to call createAgentSession " +
        "without an explicitly provided model runtime (no default model per " +
        "message 4070; SDK defaults would read user settings/auth files).",
    );
  }
  if (!options.model) {
    throw new Error(
      "No model specified: refusing to create a live session without an " +
        "explicitly resolved model (no default model per message 4070, no " +
        "first-available fallback). Resolve a model through ./model-channel.ts first.",
    );
  }
  const result = await createAgentSession({
    modelRuntime: options.modelRuntime,
    model: options.model as never,
    cwd: options.cwd,
    agentDir: options.agentDir,
    noTools: "all",
  });
  return new SenAgentSession(result.session);
}

// Thin wrapper mapping the Tauri command names (sen_session_steer,
// sen_session_follow_up, sen_session_abort, sen_events_subscribe) to the SDK
// session methods. It holds no model, no key, and makes no call on its own.
export class SenAgentSession {
  private readonly session: AgentSession;

  constructor(session: AgentSession) {
    this.session = session;
  }

  steer(text: string): Promise<void> {
    return this.session.steer(text);
  }

  followUp(text: string): Promise<void> {
    return this.session.followUp(text);
  }

  abort(): Promise<void> {
    return this.session.abort();
  }

  subscribe(listener: AgentSessionEventListener): () => void {
    return this.session.subscribe(listener);
  }

  dispose(): void {
    this.session.dispose();
  }
}
