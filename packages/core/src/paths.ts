// Single path variable in packages/core, per prep B-2 section 1.
// No path joining by string, no assumption that the home directory is ~/.pi,
// no POSIX shell assumption, no shell-dependent command.
import { join } from "node:path";

let rootDir = "";

export function setRootDir(dir: string): void {
  rootDir = dir;
}

export function resolveUnderRoot(...parts: string[]): string {
  if (!rootDir) {
    throw new Error("rootDir NOT SET: no path joining before the single path variable is set.");
  }
  return join(rootDir, ...parts);
}
