// Entry point of packages/core. SDK move (tin 9328/9459): the Pi SDK is
// out of this package — no Pi import lives here anymore. The model runs
// in the backend through the CLI core + stream-json bridge. This package
// keeps the session statechart, raw log, execution gate, token
// economizer, and paths.
export { SessionManager, canTransition } from "./session.ts";
export type { SessionState } from "./session.ts";
export { RawLog } from "./raw-log.ts";
export type { RawEvent } from "./raw-log.ts";
export { GATE_ORDER, assertGateOrder, executeThroughGate } from "./execution-gate.ts";
export type { GateStep } from "./execution-gate.ts";
export { TOKEN_ECONOMIZER_MAX_REDIRECT, TOKEN_ECONOMIZER_LONG_TURN_RATIO, shouldStopTurn } from "./token-economizer.ts";
export type { TurnSignal } from "./token-economizer.ts";
export { setRootDir, resolveUnderRoot } from "./paths.ts";
