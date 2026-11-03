// SessionManager va AgentSessionRuntime: vong doi phien, theo prep B-2 muc 1 va diem 854.
// STUB PHAN SDK: createAgentSession cua Pi SDK 1.3.0 CHUA DUOC IMPORT THAT vi package CHUA CAI.
// Phan statechart duoi day la logic that theo diem 854, chua chay kiem tra tu dong.
import { RawLog } from "./raw-log.ts";

export type SessionState =
  | "Running"
  | "AwaitingApproval"
  | "Error"
  | "Completed"
  | "Cancelled"
  | "Failed";

const TERMINAL: SessionState[] = ["Completed", "Cancelled", "Failed"];

// Diem 854: AwaitingApproval bi tu choi thi ve Cancelled, khong treo.
// Error co du 2 duong: ve Running va sang Failed.
// Chi 3 trang thai ket thuc moi khong co duong ra.
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
    // CHUA CO: goi createAgentSession that cua Pi SDK 1.3.0 o day sau khi cai package va doi chieu dossier.
    this.states.set(sessionId, "Running");
    this.rawLog.append(sessionId, "session_create_stub", { note: "CHUA GOI SDK THAT" });
    return "Running";
  }

  transition(sessionId: string, to: SessionState): SessionState {
    const from = this.states.get(sessionId);
    if (!from) {
      throw new Error("Phien khong ton tai: " + sessionId);
    }
    if (TERMINAL.includes(from)) {
      throw new Error("Trang thai ket thuc khong co duong ra: " + from);
    }
    if (!canTransition(from, to)) {
      throw new Error("Chuyen trang thai khong hop le: " + from + " sang " + to);
    }
    this.states.set(sessionId, to);
    this.rawLog.append(sessionId, "session_transition", { from, to });
    return to;
  }

  // steer, follow-up, abort: chi la khung ten, chuyen tiep qua transition hoac SDK khi lap day.
  // Khong tu xu ly logic phien o tang nay ngoai statechart tren.
}
