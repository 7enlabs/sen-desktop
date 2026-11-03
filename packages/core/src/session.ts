// SessionManager and AgentSessionRuntime: session lifecycle, per prep B-2
// section 1 and point 854.
// SDK PART, fill-in round (messages 4128/4138): the real createAgentSession
// import lives in ./agent-session.ts (pi-coding-agent 1.3.0). This file keeps
// the local statechart frame; create() below still records local state only
// and makes no SDK call. An SDK-backed session attaches through
// bindSdkSession(), which needs an explicitly provided model runtime (see
// agent-session.ts); without one, no live session exists, by design.
// The type-only import below is erased at runtime, so this file stays
// loadable without node_modules installed.
// The statechart below is real logic per point 854 (see
// packages/core/tests/logic.test.ts).
import { RawLog } from "./raw-log.ts";
import type { SenAgentSession } from "./agent-session.ts";

export type SessionState =
  | "Running"
  | "AwaitingApproval"
  | "Error"
  | "Completed"
  | "Cancelled"
  | "Failed";

const TERMINAL: SessionState[] = ["Completed", "Cancelled", "Failed"];

// Point 854: AwaitingApproval, when rejected, goes to Cancelled, never hangs.
// Error has exactly 2 paths: back to Running and on to Failed.
// Only the 3 terminal states have no outgoing path.
const ALLOWED: Record<SessionState, SessionState[]> = {
  Running: ["AwaitingApproval", "Error", "Completed", "Cancelled", "Failed"],
  AwaitingApproval: ["Running", "Cancelled"],
  Error: ["Running", "Failed"],
  Completed: [],
  Cancelled: [],
  Failed: [],
};

export function canTransition(from: SessionState, to: SessionState): boolean {
  return ALLOWED[from].includes(to);
}

export class SessionManager {
  private states = new Map<string, SessionState>();
  private sdkSessions = new Map<string, SenAgentSession>();
  readonly rawLog = new RawLog();

  create(sessionId: string): SessionState {
    // Local statechart frame only: no SDK call here. An SDK-backed session
    // attaches through bindSdkSession() with an explicit model runtime.
    this.states.set(sessionId, "Running");
    this.rawLog.append(sessionId, "session_create_stub", { note: "LOCAL STATE ONLY, NO SDK CALL" });
    return "Running";
  }

  // Binds a real SDK session handle (created via createSenAgentSession in
  // ./agent-session.ts with an explicit model runtime) to a local session id.
  // Binding alone makes no model call; it only records the link and the log.
  bindSdkSession(sessionId: string, handle: SenAgentSession): SessionState {
    const state = this.states.get(sessionId);
    if (!state) {
      throw new Error("Session does not exist: " + sessionId);
    }
    this.sdkSessions.set(sessionId, handle);
    this.rawLog.append(sessionId, "session_sdk_bound", { note: "SDK session bound; binding alone makes no model call" });
    return state;
  }

  getSdkSession(sessionId: string): SenAgentSession | undefined {
    return this.sdkSessions.get(sessionId);
  }

  transition(sessionId: string, to: SessionState): SessionState {
    const from = this.states.get(sessionId);
    if (!from) {
      throw new Error("Session does not exist: " + sessionId);
    }
    if (TERMINAL.includes(from)) {
      throw new Error("Terminal state has no outgoing transition: " + from);
    }
    if (!canTransition(from, to)) {
      throw new Error("Illegal state transition: " + from + " to " + to);
    }
    this.states.set(sessionId, to);
    this.rawLog.append(sessionId, "session_transition", { from, to });
    return to;
  }

  // steer, follow-up, abort: name frames only, forwarding through transition
  // or the SDK when filled in.
  // No session logic of its own at this layer beyond the statechart above.
}
