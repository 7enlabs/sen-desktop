// Entry point of packages/core. Only this package may import the Pi SDK, per
// prep B-2 section 3 point 4. The real SDK import lives in
// ./agent-session.ts (not re-exported here, so the install-free smoke
// import keeps working); it loads by direct import after pnpm install.
export { SessionManager, canTransition } from "./session.ts";
export type { SessionState } from "./session.ts";
export { RawLog } from "./raw-log.ts";
export type { RawEvent } from "./raw-log.ts";
export { GATE_ORDER, assertGateOrder, executeThroughGate } from "./execution-gate.ts";
export type { GateStep } from "./execution-gate.ts";
export { TOKEN_ECONOMIZER_MAX_REDIRECT, TOKEN_ECONOMIZER_LONG_TURN_RATIO, shouldStopTurn } from "./token-economizer.ts";
export type { TurnSignal } from "./token-economizer.ts";
export { setRootDir, resolveUnderRoot } from "./paths.ts";
