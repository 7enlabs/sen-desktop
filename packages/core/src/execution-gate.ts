// Cong thuc thi duy nhat, 6 chang theo thu tu co dinh, theo tin 901/931 qua prep B-2 muc 3.
// Thu tu: bao ve - kiem tra khoa - kiem tra hanh dong khong dao nguoc va dong y -
// thuc thi - kiem tra sau - ghi lai. Di duong vong hoac thieu chang thi FAIL.
// STUB: cac chang kiem tra that CHUA LAP DAY, ham nay chi giu thu tu va chan khi thieu chang.
// Kieu hop dong o day la ban sao toi thieu dung cho tang core, khong import nguoc tu apps/desktop.
// Khi lap day, phai doi chieu ban nay voi apps/desktop/src/ipc/types.ts de khong lech nhau.
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
    throw new Error("Cong thuc thi thieu chang, FAIL");
  }
  for (let i = 0; i < GATE_ORDER.length; i += 1) {
    if (steps[i] !== GATE_ORDER[i]) {
      throw new Error("Cong thuc thi sai thu tu, FAIL tai chang " + String(i));
    }
  }
}

export async function executeThroughGate(
  contract: ActionContract,
  run: () => Promise<unknown>,
): Promise<unknown> {
  // CHUA CO: logic bao ve, khoa, dong y, kiem tra sau, ghi lai that.
  // Ham nay chua duoc goi that o dau trong scaffold, chi giu khung thu tu.
  assertGateOrder(GATE_ORDER);
  void contract;
  void run;
  throw new Error("Cong thuc thi CHUA LAP DAY: khong thuc thi that.");
}
