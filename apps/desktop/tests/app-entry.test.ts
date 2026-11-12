// App entry tests for the SDK move (tin 9328/9459).
// Run with: node --test apps/desktop/tests/app-entry.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// These tests prove the app entry is wired to a real session flow through
// the backend only: startApp creates one session through the backend
// registry first (BUG-02 order: backend ack, then the local record), then
// attaches it on the same manager, with exactly one sen_session_create
// and one sen_events_subscribe. No SDK session is created anywhere in
// the webview: there is no model id, no key, and no handle in this API.
// They do NOT prove a real webview runs this module or a real model
// answers: the webview proof is the vite build (npm run build) plus the
// webview-no-sdk suite, and the model proof waits for the CLI bridge.
import test, { afterEach, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import { startApp } from "../src/app/main.ts";
import { SessionManager } from "../../../packages/core/src/session.ts";
import {
  setEventListen,
  SEN_SESSION_EVENT,
  type EventListen,
  type SenEvent,
} from "../src/ipc/session-events.ts";

// Fake Tauri event listener: startApp attaches a live session, which
// always listens after subscribing (R2-16 / #65), so tests stand in for
// the backend here exactly like they do for the bridge transport.
const capturedEvents: { send?: (payload: unknown) => void } = {};

function fakeEventListen(): EventListen {
  return async (event, handler) => {
    assert.equal(event, SEN_SESSION_EVENT);
    capturedEvents.send = (payload: unknown) => {
      handler({ payload, event, id: 1 });
    };
    return () => undefined;
  };
}

beforeEach(() => {
  capturedEvents.send = undefined;
  setEventListen(fakeEventListen());
});

afterEach(() => {
  setEventListen(null);
  setBridgeTransport(null);
});

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
    if (command === "sen_session_abort") {
      return { sessionId: a.session_id, state: "Cancelled", unknownRecords: 0, versionMismatches: 0 };
    }
    return { sessionId: a.session_id, state: "Running", unknownRecords: 0, versionMismatches: 0 };
  });
  return calls;
}

test("app entry: startApp wires one session through the backend, same manager", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const app = await startApp({ sessionId: "s1", manager });
  // One backend create, then one subscribe: no double create, no
  // second manager. No SDK session, no handle, no model id, no key.
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  assert.equal(app.manager, manager);
  assert.equal(app.sessionId, "s1");
  assert.equal(app.state, "Running");
  assert.equal(app.attached.sessionId, "s1");
  assert.equal(app.attached.state, "Running");
  assert.equal(app.attached.subscriberId, 7);
  // The local record is the create only: no SDK bind, no live start.
  const kinds = app.manager.rawLog.eventsFor("s1").map((e) => e.kind);
  assert.deepEqual(kinds, ["session_create_stub"]);
  // The session is Running and accepts the Cancelled transition.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("app entry: startApp without a session id generates a UUID default id", async () => {
  const transportCalls = liveTransport();
  const app = await startApp({});
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  // R2B-01: the default id must be a UUID, because the CLI rejects
  // "--session-id app-live-session" with exit 4 (not a valid UUID).
  const uuidPattern =
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  assert.match(app.sessionId, uuidPattern);
  assert.equal(app.attached.sessionId, app.sessionId);
  assert.equal(app.state, "Running");
  assert.equal(app.attached.subscriberId, 7);
  setBridgeTransport(null);
});

test("app entry: a second start on the same manager and session id refuses, no second backend call", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const first = await startApp({ sessionId: "s1", manager });
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  assert.equal(first.sessionId, "s1");
  await assert.rejects(
    () => startApp({ sessionId: "s1", manager }),
    /DUPLICATE_SESSION/,
  );
  // The refused second start made no backend call at all.
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  // Exactly one local record exists: it still transitions.
  assert.equal(manager.transition("s1", "Cancelled"), "Cancelled");
  setBridgeTransport(null);
});

test("app entry: a backend failure leaves no local record", async () => {
  setBridgeTransport(async () => {
    throw new Error("backend down");
  });
  const manager = new SessionManager();
  await assert.rejects(
    () => startApp({ sessionId: "s1", manager }),
    /backend down/,
  );
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("app entry: startApp receives live events into the session record and onEvent", async () => {
  // R2-16 / #65: the app entry wires one session through the backend
  // AND listens for its events. A validated message event lands in the
  // session record and in the caller's onEvent; an unknown-type event
  // reaches neither.
  const received: SenEvent[] = [];
  liveTransport();
  const manager = new SessionManager();
  const app = await startApp({
    sessionId: "s1",
    manager,
    onEvent: (event) => received.push(event),
  });
  assert.equal(typeof app.unlisten, "function");
  const send = capturedEvents.send as (payload: unknown) => void;
  send({ type: "bogus", sessionId: "s1", text: "no such variant" });
  send({ type: "message", sessionId: "s1", text: "hello" });
  send({
    type: "sessionStart",
    sessionId: "s1",
    model: "m:free",
  });
  assert.deepEqual(received, [
    { type: "message", sessionId: "s1", text: "hello" },
    { type: "sessionStart", sessionId: "s1", model: "m:free" },
  ]);
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub", "session_event", "session_event"],
  );
  setBridgeTransport(null);
});
