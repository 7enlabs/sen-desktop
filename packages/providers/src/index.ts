// STUB - Khoa version Pi tai day.
// Version Pi duoc khoa la 1.3.0, theo prep B-2 muc 3 diem 1 va 5.
// Model mac dinh: KHONG CO, theo chot tin 4070 (dung anti-job B-1: khong noi model/API that).
// packages/providers chi la vo khai bao kenh, chua gan model, chua key, chua goi mang.
// Khong import SDK o day: chi packages/core duoc import SDK, theo prep muc 3 diem 4.
export const PINNED_PI_VERSION = "1.3.0";

export interface ModelRuntime {
  pinnedPiVersion: string;
}

// CHUA CO: ModelRuntime that. Ham duoi day chi bao chua lap day, khong tra ve runtime gia.
export function createModelRuntime(): ModelRuntime {
  throw new Error("ModelRuntime CHUA LAP DAY: khong co model mac dinh theo tin 4070.");
}
