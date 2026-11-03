// SessionManager and AgentSessionRuntime: session lifecycle, per prep B-2
// section 1 and point 854.
// STUB, SDK PART: createAgentSession from Pi SDK 1.3.0 is NOT IMPORTED FOR
// REAL yet because the package is NOT INSTALLED yet.
// The statechart below is real logic per point 854; no automated test ran on
// it before this round (see packages/core/tests/logic.test.ts).
import { RawLog } from "./raw-log.ts";

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
  readonly rawLog = new RawLog();

  create(sessionId: string): SessionState {
    // NOT HERE YET: call the real createAgentSession from Pi SDK 1.3.0 here
    // after installing the package and cross-checking the dossier.
    this.states.set(sessionId, "Running");
    this.rawLog.append(sessionId, "session_create_stub", { note: "REAL SDK CALL NOT MADE YET" });
    return "Running";
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
