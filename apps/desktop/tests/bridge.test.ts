// Bridge round-trip tests (bridge round, 04/11/2026).
// Run with: node --test apps/desktop/tests/bridge.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected transport stands in for the Rust handlers, which cannot run
// under node. These tests prove the frontend half of the round-trip:
// contract validation before any transport call, command + args passthrough,
// error propagation without swallowing, the no-default-model guard before
// any backend call, and the local SessionManager transitions. They do NOT
// prove the Rust handlers run: that evidence is the standalone registry
// tests (rustc --test) plus cargo check on the Leader VM.
import test from "node:test";
import assert from "node:assert/strict";
import { invoke, setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  abortSession,
  callTool,
  createSession,
  followUpSession,
  steerSession,
  subscribeSession,
  unsubscribeSession,
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

test("bridge: contract validated before transport is called", async () => {
  const calls: Array<[string, unknown]> = [];
  setBridgeTransport(async (command, args) => {
    calls.push([command, args]);
    return {};
  });
  const bad = contract();
  bad.id = "";
  await assert.rejects(() => invoke("sen_session_create", { contract: bad }), /missing fields/);
  assert.equal(calls.length, 0);
  setBridgeTransport(null);
});

// BUG-16: assertContract must check phase, toolKind, sideEffect and risk,
// not just the original seven fields. A contract missing any one of them
// is rejected in the frontend and never reaches the transport (the Rust
// side would reject it as INVALID_CONTRACT, but the bridge comment promises
// a bad contract never reaches the backend at all).
for (const field of ["phase", "toolKind", "sideEffect", "risk"] as const) {
  test(`bridge BUG-16: contract missing ${field} is rejected before transport`, async () => {
    const calls: Array<[string, unknown]> = [];
    setBridgeTransport(async (command, args) => {
      calls.push([command, args]);
      return {};
    });
    const bad = contract();
    (bad as Record<string, unknown>)[field] = "";
    await assert.rejects(
      () => invoke("sen_session_create", { contract: bad }),
      /missing fields/,
    );
    assert.equal(calls.length, 0);
    setBridgeTransport(null);
  });
}

// BUG-16 parity: the Rust validate_contract also rejects toolKind /
// sideEffect values outside their allowed sets, so the frontend must
// reject them too instead of leaving them for the backend to catch.
test("bridge BUG-16: illegal toolKind is rejected before transport", async () => {
  const calls: Array<[string, unknown]> = [];
  setBridgeTransport(async (command, args) => {
    calls.push([command, args]);
    return {};
  });
  const bad = contract();
  (bad as Record<string, unknown>).toolKind = "unregistered-tool";
  await assert.rejects(
    () => invoke("sen_session_create", { contract: bad }),
    /illegal tool kind/,
  );
  assert.equal(calls.length, 0);
  setBridgeTransport(null);
});

test("bridge BUG-16: illegal sideEffect is rejected before transport", async () => {
  const calls: Array<[string, unknown]> = [];
  setBridgeTransport(async (command, args) => {
    calls.push([command, args]);
    return {};
  });
  const bad = contract();
  (bad as Record<string, unknown>).sideEffect = "maybe-reversible";
  await assert.rejects(
    () => invoke("sen_session_create", { contract: bad }),
    /illegal side effect class/,
  );
  assert.equal(calls.length, 0);
  setBridgeTransport(null);
});

test("bridge: valid call passes command and args through, returns result", async () => {
  const calls: Array<[string, unknown]> = [];
  setBridgeTransport(async (command, args) => {
    calls.push([command, args]);
    return { ok: true };
  });
  const args = { contract: contract(), session_id: "s1" };
  assert.deepEqual(await invoke("sen_session_create", args), { ok: true });
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], "sen_session_create");
  assert.deepEqual(calls[0][1], args);
  setBridgeTransport(null);
});

test("bridge: transport error propagates with code intact", async () => {
  setBridgeTransport(async () => {
    throw new Error("TOOL_EXECUTION_NOT_WIRED: no tool ran");
  });
  await assert.rejects(
    () => invoke("sen_tool_call", { contract: contract(), tool_name: "t" }),
    /TOOL_EXECUTION_NOT_WIRED: no tool ran/,
  );
  setBridgeTransport(null);
});

test("bridge: default transport is the real Tauri invoke, not a stub", async () => {
  setBridgeTransport(null);
  await assert.rejects(
    () => invoke("sen_session_create", { contract: contract(), session_id: "s1" }),
    (err: unknown) => {
      assert.ok(err instanceof Error);
      assert.match(err.message, /window is not defined/);
      assert.doesNotMatch(err.message, /NOT_IMPLEMENTED/);
      return true;
    },
  );
});

test("session-client create: backend ack + local Running, mismatch refused", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { session_id?: string };
    assert.equal(command, "sen_session_create");
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  const manager = new SessionManager();
  const res = await createSession(manager, contract(), "s1");
  assert.equal(res.sessionId, "s1");
  assert.equal(res.state, "Running");
  assert.equal(manager.rawLog.eventsFor("s1").length, 1);
  setBridgeTransport(async () => ({ sessionId: "other", state: "Running", unknownRecords: 0, versionMismatches: 0 }));
  const manager2 = new SessionManager();
  await assert.rejects(() => createSession(manager2, contract(), "s2"), /mismatch/);
  assert.throws(() => manager2.transition("s2", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("session-client steer: pure IPC, backend ack returned, no local state change", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command) => {
    calls.push(command);
    return { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  const res = await steerSession(contract(), "s1", "hi");
  assert.equal(res.state, "Running");
  assert.deepEqual(calls, ["sen_session_create", "sen_session_steer"]);
  // No local state was recorded for the steer: the session is still
  // Running and accepts the Cancelled transition.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("session-client steer/followUp: pure IPC calls in order, args passed through", async () => {
  const calls: Array<[string, unknown]> = [];
  setBridgeTransport(async (command, args) => {
    calls.push([command, args]);
    return { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  const steered = await steerSession(contract(), "s1", "hi");
  assert.equal(steered.state, "Running");
  const followed = await followUpSession(contract(), "s1", "again");
  assert.equal(followed.state, "Running");
  assert.deepEqual(
    calls.map(([c]) => c),
    ["sen_session_create", "sen_session_steer", "sen_session_follow_up"],
  );
  const steerArgs = calls[1][1] as Record<string, unknown>;
  assert.equal(steerArgs["session_id"], "s1");
  assert.equal(steerArgs["text"], "hi");
  setBridgeTransport(null);
});

test("session-client steer: backend CLI failure surfaces, local stays Running", async () => {
  setBridgeTransport(async (command) => {
    if (command === "sen_session_create") {
      return { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 };
    }
    throw new Error(
      "CLI_NO_RESULT: CLI turn ended with no result record (exited general error (1)).",
    );
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  await assert.rejects(
    () => steerSession(contract(), "s1", "hi"),
    /CLI_NO_RESULT/,
  );
  // The refusal recorded nothing locally: the session is still Running.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("session-client abort: backend + local Cancelled through IPC", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command) => {
    calls.push(command);
    if (command === "sen_session_create") {
      return { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 };
    }
    return { sessionId: "s1", state: "Cancelled", unknownRecords: 0, versionMismatches: 0 };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  const res = await abortSession(manager, contract(), "s1");
  assert.equal(res.state, "Cancelled");
  assert.deepEqual(calls, ["sen_session_create", "sen_session_abort"]);
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  setBridgeTransport(null);
});

test("session-client tool call: backend refusal propagates, never faked", async () => {
  setBridgeTransport(async () => {
    throw new Error("TOOL_EXECUTION_NOT_WIRED: no tool ran");
  });
  await assert.rejects(() => callTool(contract(), "my-tool"), /TOOL_EXECUTION_NOT_WIRED/);
  setBridgeTransport(null);
});

test("session-client subscribe: returns subscriber id, mismatch refused", async () => {
  setBridgeTransport(async () => ({ sessionId: "s1", subscriberId: 1 }));
  const res = await subscribeSession(contract(), "s1");
  assert.equal(res.sessionId, "s1");
  assert.equal(res.subscriberId, 1);
  setBridgeTransport(async () => ({ sessionId: "other", subscriberId: 1 }));
  await assert.rejects(() => subscribeSession(contract(), "s1"), /mismatch/);
  setBridgeTransport(null);
});

test("session-client unsubscribe (BUG-09): returns remaining count, mismatch refused", async () => {
  setBridgeTransport(async () => ({ sessionId: "s1", subscriberCount: 1 }));
  const res = await unsubscribeSession(contract(), "s1");
  assert.equal(res.sessionId, "s1");
  assert.equal(res.subscriberCount, 1);
  setBridgeTransport(async () => ({ sessionId: "other", subscriberCount: 1 }));
  await assert.rejects(() => unsubscribeSession(contract(), "s1"), /mismatch/);
  setBridgeTransport(async () => ({ sessionId: "s1" }));
  await assert.rejects(() => unsubscribeSession(contract(), "s1"), /malformed/);
  setBridgeTransport(null);
});
