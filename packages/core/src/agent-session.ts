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
// the SDK without an explicitly provided model runtime. A live session
// therefore needs a feature-time free channel decision; this round wires the
// call path and proves the import, it does not open a live session.
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
