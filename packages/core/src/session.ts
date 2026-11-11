// SessionManager and AgentSessionRuntime: session lifecycle, per prep B-2
// section 1 and point 854.
// SDK move (tin 9328/9459): the Pi SDK is out of the webview. This file
// keeps only the local statechart frame; create() below records local
// state only and makes no model call, and there is no SDK handle storage
// here anymore (bindSdkSession/getSdkSession are gone with the SDK
// modules). The model runs in the backend through the CLI core +
// stream-json bridge; the frontend mirrors the session state locally
// through this statechart and the IPC bridge.
// The statechart below is real logic per point 854 (see
// packages/core/tests/logic.test.ts).
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
    // BUG-04: an id that already exists - in any state, including a
    // terminal one - is a duplicate, exactly like the Rust registry
    // (registry.rs, ERR_DUPLICATE_SESSION). A terminal state has no
    // outgoing path, so re-creating it would resurrect a dead session;
    // re-creating a live one would silently overwrite its record and
    // log. Refuse before touching any state, so the old record and the
    // raw log stay exactly as they were, and no new record is made.
    if (this.states.has(sessionId)) {
      throw new Error(
        "DUPLICATE_SESSION: Session already exists: " + sessionId,
      );
    }
    // Local statechart frame only: no model call here. The model runs in
    // the backend through the CLI core + stream-json bridge.
    this.states.set(sessionId, "Running");
    this.rawLog.append(sessionId, "session_create_stub", { note: "LOCAL STATE ONLY, NO MODEL CALL" });
    return "Running";
  }

  // Read-only ownership check: does this manager already hold a record
  // (any state, including terminal) for this id? Callers that create a
  // session through another layer check this first, because create()
  // refuses duplicates (BUG-04, fixed in create() above) and a second
  // create would throw instead of silently overwriting.
  hasSession(sessionId: string): boolean {
    return this.states.has(sessionId);
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
