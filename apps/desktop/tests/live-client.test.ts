// Live client tests for the CLI bridge round (tin 9530).
// Run with: node --test apps/desktop/tests/live-client.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected transport stands in for the Rust handlers, which cannot run
// under node. These tests prove the frontend half of the IPC live round:
// attach creates the backend + local record and subscribes, then installs
// the session-event listener (R2-16 / #65) so validated CLI events land in
// the session record and in onEvent; close aborts through the backend,
// steer/followUp are pure IPC whose backend ack means the model ran to a
// result record, and a backend CLI failure surfaces untouched with the
// local state still Running; tool execution is still refused. They do NOT
// prove the Rust handlers run or a real model answers: that evidence is
// the standalone registry tests, cargo check on the Leader VM, and the
// CLI bridge phase.
import test, { afterEach, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import { abortSession, callTool, followUpSession, steerSession } from "../src/ipc/session-client.ts";
import {
  setEventListen,
  SEN_SESSION_EVENT,
  type EventListen,
  type SenEvent,
} from "../src/ipc/session-events.ts";
import {
  attachLiveSession,
  closeLiveSession,
} from "../src/ipc/live-client.ts";
import { SessionManager } from "../../../packages/core/src/session.ts";

// Fake Tauri event listener: attachLiveSession always listens after
// subscribing (R2-16 / #65), so tests stand in for the backend here
// exactly like they do for the bridge transport. The fake mirrors real
// Tauri semantics: once unlistened, no further payload is delivered.
const capturedEvents: {
  send?: (payload: unknown) => void;
  unlistened?: boolean;
} = {};

function fakeEventListen(): EventListen {
  return async (event, handler) => {
    assert.equal(event, SEN_SESSION_EVENT);
    capturedEvents.unlistened = false;
    capturedEvents.send = (payload: unknown) => {
      if (capturedEvents.unlistened) {
        return;
      }
      handler({ payload, event, id: 1 });
    };
    return () => {
      capturedEvents.unlistened = true;
    };
  };
}

beforeEach(() => {
  capturedEvents.send = undefined;
  capturedEvents.unlistened = false;
  setEventListen(fakeEventListen());
});

afterEach(() => {
  setEventListen(null);
  setBridgeTransport(null);
});

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

function liveTransport() {
  const calls: string[] = [];
  setBridgeTransport(async (command, args) => {
    calls.push(command);
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
    }
    if (command === "sen_events_subscribe") {
      return { sessionId: a.session_id, subscriberId: 7 };
    }
    if (command === "sen_events_unsubscribe") {
      return { sessionId: a.session_id, subscriberCount: 0 };
    }
    if (command === "sen_session_abort") {
      return { sessionId: a.session_id, state: "Cancelled", unknownRecords: 0, versionMismatches: 0 };
    }
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  return calls;
}

test("live-client attach: success creates the backend + local record and subscribes", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const attached = await attachLiveSession(manager, contract(), "s1");
  assert.equal(attached.sessionId, "s1");
  assert.equal(attached.state, "Running");
  assert.equal(attached.subscriberId, 7);
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  // The local record is the create only: no SDK bind, no live start.
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub"],
  );
  // The session is Running and accepts the Cancelled transition.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("live-client attach: backend failure leaves no local record", async () => {
  setBridgeTransport(async () => {
    throw new Error("backend down");
  });
  const manager = new SessionManager();
  await assert.rejects(
    () => attachLiveSession(manager, contract(), "s1"),
    /backend down/,
  );
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("live-client close: backend + local Cancelled through the IPC bridge", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const attached = await attachLiveSession(manager, contract(), "s1");
  const res = await closeLiveSession(manager, contract(), "s1");
  assert.deepEqual(res, { sessionId: "s1", state: "Cancelled", unknownRecords: 0, versionMismatches: 0 });
  // BUG-09: close unsubscribes before abort, so the backend subscriber
  // count drops instead of leaking.
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
    "sen_events_unsubscribe",
    "sen_session_abort",
  ]);
  assert.equal(attached.sessionId, "s1");
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  setBridgeTransport(null);
});

test("live-client close: a session that was never created refuses, no local record invented", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  await assert.rejects(
    () => closeLiveSession(manager, contract(), "s1"),
    /does not exist/,
  );
  assert.deepEqual(transportCalls, [
    "sen_events_unsubscribe",
    "sen_session_abort",
  ]);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("live-client steer/followUp: pure IPC, backend ack returned, local stays Running", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  await attachLiveSession(manager, contract(), "s1");
  const steered = await steerSession(contract(), "s1", "hi");
  assert.deepEqual(steered, { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 });
  const followed = await followUpSession(contract(), "s1", "again");
  assert.deepEqual(followed, { sessionId: "s1", state: "Running", unknownRecords: 0, versionMismatches: 0 });
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
    "sen_session_steer",
    "sen_session_follow_up",
  ]);
  // No local state was recorded for the steers: the session is still
  // Running and accepts the Cancelled transition.
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub"],
  );
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("live-client steer: backend CLI failure surfaces, local stays Running", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  await attachLiveSession(manager, contract(), "s1");
  // The backend runs the CLI; a turn that ends with no result record
  // fails loudly. The failure must surface untouched — never a fake
  // success — and the local session must stay Running: no steer ran to a
  // result, so no Failed transition is recorded.
  setBridgeTransport(async (command, args) => {
    transportCalls.push(command);
    const a = args as { session_id?: string };
    if (command === "sen_session_steer") {
      throw new Error(
        "CLI_NO_RESULT: CLI turn ended with no result record (exited general error (1)).",
      );
    }
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  await assert.rejects(
    () => steerSession(contract(), "s1", "hi"),
    /CLI_NO_RESULT/,
  );
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub"],
  );
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("live-client abort: pure IPC, backend + local Cancelled", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  await attachLiveSession(manager, contract(), "s1");
  const res = await abortSession(manager, contract(), "s1");
  assert.deepEqual(res, { sessionId: "s1", state: "Cancelled", unknownRecords: 0, versionMismatches: 0 });
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
    "sen_session_abort",
  ]);
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  setBridgeTransport(null);
});

test("live-client abort: backend refusal surfaces, local stays Running", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  await attachLiveSession(manager, contract(), "s1");
  setBridgeTransport(async (command, args) => {
    transportCalls.push(command);
    const a = args as { session_id?: string };
    if (command === "sen_session_abort") {
      throw new Error("backend down");
    }
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  await assert.rejects(
    () => abortSession(manager, contract(), "s1"),
    /backend down/,
  );
  // The refusal left the local session Running: it still transitions.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("live-client: tool execution is still refused in the live phase", async () => {
  setBridgeTransport(async () => {
    throw new Error("TOOL_EXECUTION_NOT_WIRED: no tool ran");
  });
  await assert.rejects(
    () => callTool(contract(), "my-tool"),
    /TOOL_EXECUTION_NOT_WIRED/,
  );
  setBridgeTransport(null);
});

test("live-client attach: validated session events are recorded in the session log", async () => {
  // R2-16 / #65: subscribing on the backend alone delivered nothing
  // to the app — no code ever listened. Attach now installs the
  // listener and records each validated event in the session's raw log
  // (kind session_event), in order; other sessions' events, unknown
  // types and malformed payloads never reach the record.
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const attached = await attachLiveSession(manager, contract(), "s1");
  assert.equal(typeof attached.unlisten, "function");
  const send = capturedEvents.send as (payload: unknown) => void;
  send({ type: "message", sessionId: "other", text: "not mine" });
  send({ type: "bogus", sessionId: "s1", text: "no such variant" });
  send({ type: "message", sessionId: "s1" });
  send({ type: "message", sessionId: "s1", text: "hello" });
  send({
    type: "result",
    sessionId: "s1",
    subtype: "success",
    isError: false,
    text: "done",
    numTurns: 1,
    usage: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 0 },
    durationMs: 5,
    model: "m:free",
  });
  const events = manager.rawLog.eventsFor("s1");
  assert.deepEqual(
    events.map((event) => event.kind),
    ["session_create_stub", "session_event", "session_event"],
  );
  assert.deepEqual(events[1].payload, {
    type: "message",
    sessionId: "s1",
    text: "hello",
  });
  assert.equal((events[2].payload as SenEvent).type, "result");
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  setBridgeTransport(null);
});

test("live-client attach: onEvent receives the same validated events", async () => {
  const received: SenEvent[] = [];
  const manager = new SessionManager();
  liveTransport();
  await attachLiveSession(manager, contract(), "s1", {
    onEvent: (event) => received.push(event),
  });
  const send = capturedEvents.send as (payload: unknown) => void;
  send({ type: "bogus", sessionId: "s1", text: "no such variant" });
  send({ type: "message", sessionId: "s1", text: "hello" });
  send({
    type: "system",
    sessionId: "s1",
    subtype: "tool_progress",
  });
  assert.deepEqual(received, [
    { type: "message", sessionId: "s1", text: "hello" },
    { type: "system", sessionId: "s1", subtype: "tool_progress" },
  ]);
  // The record got the same two events, not the bogus one.
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub", "session_event", "session_event"],
  );
  setBridgeTransport(null);
});

test("live-client attach: a listen failure fails attach with no backend call", async () => {
  // R2-16 / #65: attach listens before any backend call, so a session
  // that cannot receive events never gets created or subscribed — the
  // failure surfaces untouched and no orphan record or subscriber is
  // left behind.
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  setEventListen(async () => {
    throw new Error("listen down");
  });
  await assert.rejects(
    () => attachLiveSession(manager, contract(), "s1"),
    /listen down/,
  );
  assert.deepEqual(transportCalls, []);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setEventListen(null);
  setBridgeTransport(null);
});

test("live-client close: unlisten stops the frontend listener before backend unsubscribe", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const attached = await attachLiveSession(manager, contract(), "s1");
  const send = capturedEvents.send as (payload: unknown) => void;
  send({ type: "message", sessionId: "s1", text: "before close" });
  assert.equal(capturedEvents.unlistened, false);
  await closeLiveSession(manager, contract(), "s1", attached.unlisten);
  assert.equal(capturedEvents.unlistened, true);
  // No event is delivered after the listener stopped: the only new log
  // entry is the Cancelled transition from the abort itself.
  send({ type: "message", sessionId: "s1", text: "after close" });
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub", "session_event", "session_transition"],
  );
  // Backend order is unchanged: unsubscribe, then abort (BUG-09).
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
    "sen_events_unsubscribe",
    "sen_session_abort",
  ]);
  setBridgeTransport(null);
});

test("live-client close (V-04): unlisten runs BEFORE backend unsubscribe and abort", async () => {
  // V-04 (base 7158fe1): no production close path exists yet, so this
  // locks the join contract itself — closeLiveSession must stop the
  // frontend listener first, then unsubscribe, then abort. A future UI
  // close path passes StartedApp.unlisten as the 4th argument; the order
  // inside closeLiveSession is what this test pins, not a caller that
  // does not exist yet. The order array records the unlisten call on the
  // same timeline as the backend commands, so a swap fails this test.
  const order: string[] = [];
  setBridgeTransport(async (command, args) => {
    order.push(command);
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
    }
    if (command === "sen_events_subscribe") {
      return { sessionId: a.session_id, subscriberId: 7 };
    }
    if (command === "sen_events_unsubscribe") {
      return { sessionId: a.session_id, subscriberCount: 0 };
    }
    if (command === "sen_session_abort") {
      return { sessionId: a.session_id, state: "Cancelled", unknownRecords: 0, versionMismatches: 0 };
    }
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  const manager = new SessionManager();
  const attached = await attachLiveSession(manager, contract(), "s1");
  const unlisten = attached.unlisten;
  assert.equal(typeof unlisten, "function");
  await closeLiveSession(manager, contract(), "s1", () => {
    order.push("unlisten");
    if (unlisten) {
      unlisten();
    }
  });
  assert.deepEqual(order, [
    "sen_session_create",
    "sen_events_subscribe",
    "unlisten",
    "sen_events_unsubscribe",
    "sen_session_abort",
  ]);
  setBridgeTransport(null);
});
