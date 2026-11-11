// Session events tests for the CLI bridge round (tin 9530 quyết 4).
// Run with: node --test apps/desktop/tests/session-events.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected listener stands in for the Tauri backend, which cannot run
// under node. These tests prove the frontend half of the event flow:
// listenSessionEvents subscribes to the sen-session-event channel, drops
// events for other sessions and malformed payloads, delivers this
// session's events (including "unknown" record types) in order, and
// returns the real unlisten function. They do NOT prove the Rust backend
// emits: that evidence is the standalone stream_json / cli_spawn tests.
import test from "node:test";
import assert from "node:assert/strict";
import {
  listenSessionEvents,
  setEventListen,
  SEN_SESSION_EVENT,
  type EventListen,
  type SenEvent,
} from "../src/ipc/session-events.ts";

type SendFn = (payload: unknown) => void;

// Captures the subscribed handler so the test can send payloads through
// it like the Tauri backend would.
function fakeListen(captured: { send?: SendFn }): EventListen {
  return async (event, handler) => {
    assert.equal(event, SEN_SESSION_EVENT);
    captured.send = (payload: unknown) => {
      handler({ payload, event, id: 1 });
    };
    let unlistened = false;
    return () => {
      unlistened = true;
      assert.ok(unlistened);
    };
  };
}

test("session-events: only this session's events are delivered", async () => {
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  const unlisten = await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  send({ type: "message", sessionId: "other", text: "not mine" });
  send({ type: "message", sessionId: "s1", text: "hello" });
  send({ type: "messageUpdate", sessionId: "s1", delta: "hel" });
  assert.deepEqual(received, [
    { type: "message", sessionId: "s1", text: "hello" },
    { type: "messageUpdate", sessionId: "s1", delta: "hel" },
  ]);
  unlisten();
  setEventListen(null);
});

test("session-events: unknown record types are delivered, not dropped", async () => {
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  send({
    type: "unknown",
    sessionId: "s1",
    recordType: "future_thing",
    subtype: "x",
  });
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
  assert.equal(received.length, 2);
  assert.equal(received[0].type, "unknown");
  assert.equal(received[1].type, "result");
  setEventListen(null);
});

test("session-events: malformed payloads are dropped, never crash", async () => {
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  send(null);
  send("garbage");
  send({ type: "message" });
  send({ type: "message", sessionId: "s1", text: "ok" });
  assert.deepEqual(received, [
    { type: "message", sessionId: "s1", text: "ok" },
  ]);
  setEventListen(null);
});

test("session-events: event name matches the Rust backend constant", () => {
  assert.equal(SEN_SESSION_EVENT, "sen-session-event");
});
