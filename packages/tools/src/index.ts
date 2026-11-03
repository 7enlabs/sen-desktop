// Lop permission: hook su kien tool_call, xet theo hanh vi chu khong theo cu phap shell
// cua mot he, theo prep B-2 muc 3 diem 6.
// Moi tool call phai di qua hook nay. Tool khong qua hook thi FAIL.
// STUB: logic xet quyen that CHUA LAP DAY. Khong ap che do hoi hep cua Mika,
// khong doi che do giua chung, khong vo hieu lop bao ve, theo prep muc 3 diem 10.
// Kieu hop dong toi thieu o tang tools, khong import nguoc tu apps/desktop.
// Khi lap day, phai doi chieu voi apps/desktop/src/ipc/types.ts.
export interface ToolContract {
  id: string;
  idempotencyKey: string;
  toolKind: string;
}

export interface ToolCallEvent {
  contract: ToolContract;
  toolName: string;
}

export function onToolCall(event: ToolCallEvent): void {
  // CHUA CO: xet quyen that theo hanh vi. Chi kiem tra hop dong toi thieu o day.
  if (!event.contract.idempotencyKey) {
    throw new Error("Tool call thieu idempotency key, FAIL");
  }
  if (event.contract.toolKind !== "registered-tool" && event.contract.toolKind !== "authorized-execution-tool" && event.contract.toolKind !== "model-tool") {
    throw new Error("Loai tool khong hop le, FAIL");
  }
  // Buoc tiep theo khi lap day: noi hook nay vao cong thuc thi 6 chang o packages/core.
  throw new Error("Lop permission CHUA LAP DAY: khong cho tool chay that.");
}
