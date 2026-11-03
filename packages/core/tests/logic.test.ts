// Tests for the 4 real logic files in packages/core.
// Run with: node --test packages/core/tests/logic.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// These tests cover the pure logic only. SDK calls, the Tauri app, and the
// real gate steps are stubs and are NOT covered here.
import test from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { SessionManager, canTransition } from "../src/session.ts";
import {
  GATE_ORDER,
  assertGateOrder,
  executeThroughGate,
} from "../src/execution-gate.ts";
import {
  TOKEN_ECONOMIZER_LONG_TURN_RATIO,
  TOKEN_ECONOMIZER_MAX_REDIRECT,
  shouldStopTurn,
} from "../src/token-economizer.ts";
import { resolveUnderRoot, setRootDir } from "../src/paths.ts";

// --- 1. Session statechart (point 854) ---

test("statechart: allowed transitions per point 854", () => {
  assert.equal(canTransition("Running", "AwaitingApproval"), true);
  assert.equal(canTransition("AwaitingApproval", "Running"), true);
  // AwaitingApproval, when rejected, goes to Cancelled, never hangs.
  assert.equal(canTransition("AwaitingApproval", "Cancelled"), true);
  assert.equal(canTransition("AwaitingApproval", "Error"), false);
  // Error has exactly 2 paths: back to Running and on to Failed.
  assert.equal(canTransition("Error", "Running"), true);
  assert.equal(canTransition("Error", "Failed"), true);
  assert.equal(canTransition("Error", "Cancelled"), false);
  // Terminal states have no outgoing path.
  for (const terminal of ["Completed", "Cancelled", "Failed"] as const) {
    for (const to of ["Running", "AwaitingApproval", "Error", "Completed", "Cancelled", "Failed"] as const) {
      assert.equal(canTransition(terminal, to), false);
    }
  }
});

test("SessionManager: lifecycle through the statechart", () => {
  const manager = new SessionManager();
  assert.equal(manager.create("s1"), "Running");
  assert.equal(manager.transition("s1", "AwaitingApproval"), "AwaitingApproval");
  assert.equal(manager.transition("s1", "Running"), "Running");
  assert.equal(manager.transition("s1", "Error"), "Error");
  assert.equal(manager.transition("s1", "Failed"), "Failed");
  // Terminal state has no outgoing transition.
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  // Unknown session throws.
  assert.throws(() => manager.transition("missing", "Running"), /does not exist/);
  // Illegal transition throws (AwaitingApproval has no path to Error).
  manager.create("s2");
  manager.transition("s2", "AwaitingApproval");
  assert.throws(() => manager.transition("s2", "Error"), /Illegal state transition/);
});

// --- 2. Execution gate order (6 fixed steps) ---

test("gate order: exact order passes, any drift fails", () => {
  assert.deepEqual(GATE_ORDER, [
    "protect",
    "lock-check",
    "irreversible-and-consent-check",
    "execute",
    "post-check",
    "record",
  ]);
  assert.doesNotThrow(() => assertGateOrder(GATE_ORDER));
  assert.throws(() => assertGateOrder(["protect"]), /missing step/);
  assert.throws(() => assertGateOrder([...GATE_ORDER].reverse()), /wrong order/);
});

test("executeThroughGate: still a stub, throws instead of executing", async () => {
  const contract = {
    actor: "test",
    approvalRequired: false,
    auditRequired: false,
    capability: "test",
    capabilityVersion: "1",
    id: "c1",
    idempotencyKey: "k1",
    phase: "test",
    purpose: "test",
    timeoutMs: 1000,
  };
  await assert.rejects(
    () => executeThroughGate(contract, async () => "ran"),
    /NOT FILLED IN YET/,
  );
});

// --- 3. Token economizer (point 856) ---

test("token economizer: constants and stop rules", () => {
  assert.equal(TOKEN_ECONOMIZER_MAX_REDIRECT, 1);
  assert.equal(TOKEN_ECONOMIZER_LONG_TURN_RATIO, 2.5);
  const base = {
    errorSignature: "",
    repeatedError: false,
    sameEditChurn: false,
    turnLength: 10,
    typicalTurnLength: 10,
  };
  // Clean signal: keep going.
  assert.equal(shouldStopTurn({ ...base, redirectCount: 0 }), false);
  // Redirect budget used up: stop.
  assert.equal(shouldStopTurn({ ...base, redirectCount: 1 }), true);
  // Same error repeated across tool calls: stop.
  assert.equal(shouldStopTurn({ ...base, redirectCount: 0, repeatedError: true }), true);
  // Long turn while churning the same edit: stop.
  assert.equal(
    shouldStopTurn({
      ...base,
      redirectCount: 0,
      sameEditChurn: true,
      turnLength: 26,
      typicalTurnLength: 10,
    }),
    true,
  );
  // Same-edit churn but turn not long enough: keep going.
  assert.equal(
    shouldStopTurn({
      ...base,
      redirectCount: 0,
      sameEditChurn: true,
      turnLength: 25,
      typicalTurnLength: 10,
    }),
    false,
  );
  // Zero typical length guards the ratio: keep going, never divide by zero.
  assert.equal(
    shouldStopTurn({
      ...base,
      redirectCount: 0,
      sameEditChurn: true,
      turnLength: 100,
      typicalTurnLength: 0,
    }),
    false,
  );
});

// --- 4. Single path variable ---

test("paths: no joining before root is set, join after it is set", () => {
  assert.throws(() => resolveUnderRoot("a"), /rootDir NOT SET/);
  setRootDir(join("tmp", "sen-root"));
  assert.equal(resolveUnderRoot("a", "b"), join("tmp", "sen-root", "a", "b"));
});
