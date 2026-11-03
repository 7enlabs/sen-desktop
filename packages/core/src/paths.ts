// Mot bien duong dan duy nhat o packages/core, theo prep B-2 muc 1.
// Khong ghep duong dan bang chuoi, khong gia dinh thu muc nha la ~/.pi,
// khong gia dinh shell POSIX, khong dung lenh phu thuoc shell.
import { join } from "node:path";

let rootDir = "";

export function setRootDir(dir: string): void {
  rootDir = dir;
}

export function resolveUnderRoot(...parts: string[]): string {
  if (!rootDir) {
    throw new Error("CHUA DAT rootDir: khong ghep duong dan khi chua co mot bien duong dan.");
  }
  return join(rootDir, ...parts);
}
