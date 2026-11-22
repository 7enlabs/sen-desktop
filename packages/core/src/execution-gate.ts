// Single execution gate, 6 steps in fixed order, per messages 901/931 via
// prep B-2 section 3.
// Order: protect - lock check - irreversible-action and consent check -
// execute - post check - record. A shortcut path or a missing step means FAIL.
// STUB: the real check steps are NOT FILLED IN YET; this function only keeps
// the order and blocks when a step is missing.
// The contract type here is a minimal copy for the core layer; it must not
// import backwards from apps/desktop.
// When filled in, cross-check this copy against
// apps/desktop/src/ipc/types.ts so the two do not drift apart.
export interface ActionContract {
  id: string;
  capability: string;
  capabilityVersion: string;
  actor: string;
  purpose: string;
  phase: string;
  idempotencyKey: string;
  timeoutMs: number;
  approvalRequired: boolean;
  auditRequired: boolean;
}

export type GateStep =
  | "protect"
  | "lock-check"
  | "irreversible-and-consent-check"
  | "execute"
  | "post-check"
  | "record";

export const GATE_ORDER: GateStep[] = [
  "protect",
  "lock-check",
  "irreversible-and-consent-check",
  "execute",
  "post-check",
  "record",
];

export function assertGateOrder(steps: GateStep[]): void {
  if (steps.length !== GATE_ORDER.length) {
    throw new Error("Execution gate missing step, FAIL");
  }
  for (let i = 0; i < GATE_ORDER.length; i += 1) {
    if (steps[i] !== GATE_ORDER[i]) {
      throw new Error("Execution gate wrong order, FAIL at step " + String(i));
    }
  }
}

export async function executeThroughGate(
  contract: ActionContract,
  run: () => Promise<unknown>,
): Promise<unknown> {
  // NOT HERE YET: real protect, lock, consent, post-check, and record logic.
  // This function is not really called anywhere in the scaffold yet; it only
  // keeps the order frame.
  assertGateOrder(GATE_ORDER);
  void contract;
  void run;
  throw new Error("Execution gate NOT FILLED IN YET: no real execution.");
}
