// Session-client tests for R2B-04: the backend SessionResult carries the
// turn's unknown-record and version-mismatch counts to the frontend.
// Run with: node --test apps/desktop/tests/session-client.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected transport stands in for the Rust handlers, which cannot run
// under node. These tests prove the frontend half only: a fake transport
// returning counts is received unchanged by steer/followUp, create/abort
// carry zero counts (no turn ran), and a result missing the counts is
// refused as malformed instead of silently dropping them. They do NOT
// prove the Rust handlers emit the counts: that evidence is the
// standalone cli_spawn harness test (missing `v` counts as a mismatch)
// plus the session.rs wiring (Run and Duplicate both fill the counts
// from TurnResult).
import test from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  abortSession,
  createSession,
  followUpSession,
  steerSession,
} from "../src/ipc/session-client.ts";
import { SessionManager } from "../../../packages/core/src/session.ts";

function contract(): ActionContract {
  return {
    id: "c1",
    capability: "sen.session",
    capabilityVersion: "1",
    actor: "test",
    purpose: "test",
    phase: "test",
    toolKind: "registered-tool",
    sideEffect: "no-side-effect",
    risk: "low",
    idempotencyKey: "k1",
    timeoutMs: 1000,
    approvalRequired: false,
    auditRequired: false,
  };
}

test("session-client steer: unknown/version counts reach the frontend", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.session_id,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // A turn that saw 2 unknown records and 1 version mismatch.
    return {
      sessionId: a.session_id,
      state: "Running",
      unknownRecords: 2,
      versionMismatches: 1,
    };
  });
  const manager = new SessionManager();
  const created = await createSession(manager, contract(), "s1");
  assert.equal(created.unknownRecords, 0);
  assert.equal(created.versionMismatches, 0);
  const steered = await steerSession(contract(), "s1", "hi");
  assert.equal(steered.unknownRecords, 2);
  assert.equal(steered.versionMismatches, 1);
  const followed = await followUpSession(contract(), "s1", "again");
  assert.equal(followed.unknownRecords, 2);
  assert.equal(followed.versionMismatches, 1);
  setBridgeTransport(null);
});

test("session-client abort: no turn ran, counts are zero", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.session_id,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    return {
      sessionId: a.session_id,
      state: "Cancelled",
      unknownRecords: 0,
      versionMismatches: 0,
    };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  const res = await abortSession(manager, contract(), "s1");
  assert.equal(res.state, "Cancelled");
  assert.equal(res.unknownRecords, 0);
  assert.equal(res.versionMismatches, 0);
  setBridgeTransport(null);
});

test("session-client steer: result missing the counts is refused", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.session_id,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // Old-shape result without the R2B-04 counts: malformed, FAIL.
    return { sessionId: a.session_id, state: "Running" };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  await assert.rejects(() => steerSession(contract(), "s1", "hi"), /malformed/);
  setBridgeTransport(null);
});
