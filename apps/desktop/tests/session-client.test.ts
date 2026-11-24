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
import { readFileSync } from "node:fs";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  abortSession,
  callTool,
  createSession,
  endSession,
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

test("session-client steer: unknown/version counts reach the frontend", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // A turn that saw 2 unknown records and 1 version mismatch.
    return {
      sessionId: a.sessionId,
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
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    return {
      sessionId: a.sessionId,
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
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // Old-shape result without the R2B-04 counts: malformed, FAIL.
    return { sessionId: a.sessionId, state: "Running" };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  await assert.rejects(() => steerSession(contract(), "s1", "hi"), /malformed/);
  setBridgeTransport(null);
});

// --- CI P2-6: wire shape locked against session.rs + events.rs ---
// The session handlers take FLAT parameters, not an `args: <Args>`
// struct: `session_id: String` and `text: String` travel as top-level
// payload keys next to `contract`. The wire keys are lowerCamelCase:
// Tauri v2 maps each Rust parameter name to its camelCase form
// (session_id -> sessionId, text -> text), so the client must send
// sessionId, never the Rust identifier session_id. Sen v0.1.0 sent
// session_id and every session call failed at the Tauri layer with
// "missing required key sessionId" — the failure mode is the mirror
// image of settings-client R2B-05 rev2: a client that nests these
// under `args` passes under node and fails on every real call
// ("missing required key sessionId"), and a client that sends the
// Rust identifier instead of the wire key fails the same way. These
// tests parse each handler signature out of session.rs / events.rs,
// map the declared names to wire keys, and assert the client payload
// carries exactly the flat params the handler waits for — no `args`
// key, no snake_case key — so they FAIL on a wrong payload and PASS
// only when the client sends the shape the handler waits for. They do
// NOT prove the Rust handlers run a turn: that evidence is the
// cli_spawn harness test plus the registry suite.
function rustSessionSource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/session.rs", import.meta.url),
    "utf8",
  );
}

function rustEventsSource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/events.rs", import.meta.url),
    "utf8",
  );
}

/// Flat wire params for a command, derived from the Rust handler
/// signature: every declared parameter except the Tauri-managed
/// `state` / `app` / `memory` slots and the `contract`. Declared
/// names are mapped to Tauri v2 wire keys (lowerCamelCase, e.g.
/// `session_id` -> `sessionId`); the payload is asserted against
/// the wire keys, never the Rust identifiers.
///
/// Tauri v2 lowerCamelCases each `#[tauri::command]` parameter name
/// for the JS side (no `rename_all` override on these handlers),
/// so the wire key is the camelCase form of the Rust name.
function toCamelCase(snake: string): string {
  return snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}
function expectedFlatParams(src: string, file: string, command: string): string[] {
  const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
  assert.ok(fnMatch, `Rust handler ${command} not found in ${file}`);
  const params = fnMatch[1];
  // Split on commas that are not inside <...> (State<'_, Mutex<...>>).
  const chunks: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of params) {
    if (ch === "<") depth += 1;
    if (ch === ">") depth -= 1;
    if (ch === "," && depth === 0) {
      chunks.push(cur);
      cur = "";
    } else {
      cur += ch;
    }
  }
  chunks.push(cur);
  const names = chunks
    .map((c) => c.trim().match(/^(\w+)\s*:/)?.[1])
    .filter((n): n is string => !!n);
  const wire = names.filter((n) => !["state", "app", "memory", "contract"].includes(n));
  assert.ok(wire.length > 0, `${command} declares no wire params in ${file}`);
  return wire.map(toCamelCase);
}

async function capturePayload(
  command: string,
  stubResult: unknown,
  call: () => Promise<unknown>,
): Promise<Record<string, unknown>> {
  let seen: Record<string, unknown> = {};
  setBridgeTransport(async (cmd, args) => {
    assert.equal(cmd, command);
    seen = args as Record<string, unknown>;
    return stubResult;
  });
  try {
    await call();
  } finally {
    setBridgeTransport(null);
  }
  return seen;
}

test("session-client capturePayload: a failing call still restores the real transport (CI P2-6)", async () => {
  // Same try/finally as approval-gate.test.ts: a call that throws
  // after the stub answered must not leave the stub installed for
  // the next test.
  await assert.rejects(
    capturePayload(
      "sen_session_create",
      sessionStubResult(),
      async () => {
        await createSession(new SessionManager(), contract(), "s1");
        throw new Error("boom after call");
      },
    ),
    /boom after call/,
  );
  // With the stub gone, the next call reaches the real Tauri invoke,
  // which throws under node ("window is not defined") — not the
  // stale stub's answer.
  await assert.rejects(
    () => createSession(new SessionManager(), contract(), "s1"),
    /window is not defined/,
  );
});

function sessionStubResult() {
  return {
    sessionId: "s1",
    state: "Running",
    unknownRecords: 0,
    versionMismatches: 0,
  };
}

test("session-client create: flat params match the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustSessionSource(), "session.rs", "sen_session_create"), ["sessionId"]);
  const manager = new SessionManager();
  const payload = await capturePayload(
    "sen_session_create",
    sessionStubResult(),
    () => createSession(manager, contract(), "s1"),
  );
  assert.deepEqual(payload, { contract: contract(), sessionId: "s1" });
  assert.ok(!("args" in payload), "create takes flat params, so the payload must not carry an args key");
  assert.ok(!("session_id" in payload), "payload must use the Tauri v2 wire key sessionId, not the Rust identifier session_id");
});

test("session-client steer: flat params match the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustSessionSource(), "session.rs", "sen_session_steer"), ["sessionId", "text"]);
  const payload = await capturePayload(
    "sen_session_steer",
    sessionStubResult(),
    () => steerSession(contract(), "s1", "hi"),
  );
  assert.deepEqual(payload, { contract: contract(), sessionId: "s1", text: "hi" });
  assert.ok(!("args" in payload), "steer takes flat params, so the payload must not carry an args key");
});

test("session-client follow_up: flat params match the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustSessionSource(), "session.rs", "sen_session_follow_up"), ["sessionId", "text"]);
  const payload = await capturePayload(
    "sen_session_follow_up",
    sessionStubResult(),
    () => followUpSession(contract(), "s1", "again"),
  );
  assert.deepEqual(payload, { contract: contract(), sessionId: "s1", text: "again" });
  assert.ok(!("args" in payload), "follow_up takes flat params, so the payload must not carry an args key");
});

test("session-client abort: flat params match the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustSessionSource(), "session.rs", "sen_session_abort"), ["sessionId"]);
  const manager = new SessionManager();
  setBridgeTransport(async (command, args) => {
    const a = args as { sessionId?: string };
    return {
      sessionId: a.sessionId,
      state: command === "sen_session_create" ? "Running" : "Cancelled",
      unknownRecords: 0,
      versionMismatches: 0,
    };
  });
  await createSession(manager, contract(), "s1");
  let abortPayload: Record<string, unknown> = {};
  setBridgeTransport(async (cmd, args) => {
    assert.equal(cmd, "sen_session_abort");
    abortPayload = args as Record<string, unknown>;
    return { sessionId: "s1", state: "Cancelled", unknownRecords: 0, versionMismatches: 0 };
  });
  await abortSession(manager, contract(), "s1");
  setBridgeTransport(null);
  assert.deepEqual(abortPayload, { contract: contract(), sessionId: "s1" });
  assert.ok(!("args" in abortPayload), "abort takes flat params, so the payload must not carry an args key");
});

test("session-client subscribe/unsubscribe: flat params match the Rust handler (CI P2-6)", async () => {
  const events = rustEventsSource();
  assert.deepEqual(expectedFlatParams(events, "events.rs", "sen_events_subscribe"), ["sessionId"]);
  assert.deepEqual(expectedFlatParams(events, "events.rs", "sen_events_unsubscribe"), ["sessionId"]);
  const subPayload = await capturePayload(
    "sen_events_subscribe",
    { sessionId: "s1", subscriberId: 1 },
    () => subscribeSession(contract(), "s1"),
  );
  assert.deepEqual(subPayload, { contract: contract(), sessionId: "s1" });
  assert.ok(!("args" in subPayload), "subscribe takes flat params, so the payload must not carry an args key");
  const unsubPayload = await capturePayload(
    "sen_events_unsubscribe",
    { sessionId: "s1", subscriberCount: 0 },
    () => unsubscribeSession(contract(), "s1"),
  );
  assert.deepEqual(unsubPayload, { contract: contract(), sessionId: "s1" });
  assert.ok(!("args" in unsubPayload), "unsubscribe takes flat params, so the payload must not carry an args key");
});


// --- Wire-key regression: Sen v0.1.0 "missing required key sessionId" ---
// Tauri v2 maps each Rust flat parameter to its lowerCamelCase wire key
// (session_id -> sessionId, tool_name -> toolName). Sen v0.1.0 sent the
// Rust identifiers instead, so every session call failed at the Tauri
// layer with "invalid args sessionId for command sen_session_create:
// missing required key sessionId" — before any Rust handler ran. The
// CI P2-6 tests above pin each payload against the Rust signature;
// this test pins the wire keys directly, so a future edit that reverts
// any call to snake_case fails here under node instead of at runtime.
test("session-client wire keys are camelCase (regression: v0.1.0 missing required key sessionId)", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push(args as Record<string, unknown>);
    // abort lands on Cancelled locally; the other calls land on Running.
    return command === "sen_session_abort"
      ? { ...sessionStubResult(), state: "Cancelled" }
      : sessionStubResult();
  });
  try {
    const manager = new SessionManager();
    await createSession(manager, contract(), "s1");
    await steerSession(contract(), "s1", "hi");
    await followUpSession(contract(), "s1", "again");
    await abortSession(manager, contract(), "s1");
    // endSession needs a live local record: the abort above moved the
    // first manager's session to Cancelled (terminal), so use a fresh one.
    const manager2 = new SessionManager();
    manager2.create("s1");
    setBridgeTransport(async (_command, args) => {
      seen.push(args as Record<string, unknown>);
      return endStubResult();
    });
    await endSession(manager2, contract(), "s1");
    setBridgeTransport(async (_command, args) => {
      seen.push(args as Record<string, unknown>);
      return { sessionId: "s1", subscriberId: 1 };
    });
    await subscribeSession(contract(), "s1");
    setBridgeTransport(async (_command, args) => {
      seen.push(args as Record<string, unknown>);
      return { sessionId: "s1", subscriberCount: 0 };
    });
    await unsubscribeSession(contract(), "s1");
    setBridgeTransport(async (_command, args) => {
      seen.push(args as Record<string, unknown>);
      return undefined;
    });
    await callTool(contract(), "read_file");
  } finally {
    setBridgeTransport(null);
  }
  // 7 session calls + 1 tool call, in call order.
  assert.equal(seen.length, 8);
  for (const payload of seen) {
    assert.ok(
      "sessionId" in payload || "toolName" in payload,
      "payload carries a camelCase wire key: " + JSON.stringify(payload),
    );
    assert.ok(!("session_id" in payload), "payload must not carry the Rust identifier session_id: " + JSON.stringify(payload));
    assert.ok(!("tool_name" in payload), "payload must not carry the Rust identifier tool_name: " + JSON.stringify(payload));
  }
  assert.deepEqual(Object.keys(seen[0] ?? {}).sort(), ["contract", "sessionId"]);
  assert.deepEqual(Object.keys(seen[7] ?? {}).sort(), ["contract", "toolName"]);
});

// --- sen_session_end (T11, Q1-Q9) ---

function endStubResult() {
  return {
    sessionId: "s1",
    state: "Completed",
    alreadyEnded: false,
    endedFrom: "Running",
    turnsCompleted: 1,
    approvalsRevoked: 2,
  };
}

test("session-client end: flat params match the Rust handler (CI P2-6)", async () => {
  assert.deepEqual(expectedFlatParams(rustSessionSource(), "session.rs", "sen_session_end"), ["sessionId"]);
  const manager = new SessionManager();
  manager.create("s1");
  const payload = await capturePayload(
    "sen_session_end",
    endStubResult(),
    () => endSession(manager, contract(), "s1"),
  );
  assert.deepEqual(payload, { contract: contract(), sessionId: "s1" });
  assert.ok(!("args" in payload), "end takes flat params, so the payload must not carry an args key");
  assert.ok(!("session_id" in payload), "payload must use the Tauri v2 wire key sessionId, not the Rust identifier session_id");
});

test("session-client end: receipt fields reach the frontend, local lands on Completed", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    return endStubResult();
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  const res = await endSession(manager, contract(), "s1");
  assert.equal(res.state, "Completed");
  assert.equal(res.alreadyEnded, false);
  assert.equal(res.endedFrom, "Running");
  assert.equal(res.turnsCompleted, 1);
  assert.equal(res.approvalsRevoked, 2);
  // The local record is terminal now: no transition out of it.
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  setBridgeTransport(null);
});

test("session-client end: result missing the receipt fields is refused", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // End result without the receipt fields: malformed, FAIL.
    return { sessionId: a.sessionId, state: "Completed" };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  await assert.rejects(() => endSession(manager, contract(), "s1"), /malformed/);
  setBridgeTransport(null);
});

test("session-client end (R2B-10): foreign manager refuses before any backend call", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command) => {
    calls.push(command);
    return endStubResult();
  });
  const foreign = new SessionManager();
  await assert.rejects(
    () => endSession(foreign, contract(), "s1"),
    /does not exist/,
  );
  assert.deepEqual(calls, []);
  setBridgeTransport(null);
});

test("session-client end: backend/local mismatch is refused", async () => {
  setBridgeTransport(async (command, args) => {
    const a = args as { sessionId?: string };
    if (command === "sen_session_create") {
      return {
        sessionId: a.sessionId,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    // Backend says Cancelled while the local transition goes to
    // Completed: the two sides disagree, FAIL.
    return { ...endStubResult(), state: "Cancelled" };
  });
  const manager = new SessionManager();
  await createSession(manager, contract(), "s1");
  await assert.rejects(() => endSession(manager, contract(), "s1"), /mismatch/);
  setBridgeTransport(null);
});
