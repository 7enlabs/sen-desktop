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
import { readFileSync } from "node:fs";
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

test("session-events: unknown event types and malformed variants are dropped", async () => {
  // R2-16 / #65: the backend speaks only the 7 Sen variants that mirror
  // the CLI's system/assistant/user/stream_event/result records. A
  // payload with an unknown type, or a known type with the wrong fields,
  // must never reach onEvent — the old check (type is a string +
  // sessionId is a string) let all of these through.
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  send({ type: "bogus", sessionId: "s1", text: "no such variant" });
  send({ type: "message_start", sessionId: "s1" });
  send({ type: "message", sessionId: "s1" });
  send({ type: "message", sessionId: "s1", text: 7 });
  send({ type: "messageUpdate", sessionId: "s1" });
  send({ type: "toolResult", sessionId: "s1", toolUseId: 7, content: "ok", isError: false });
  send({ type: "system", sessionId: "s1" });
  send({
    type: "result",
    sessionId: "s1",
    subtype: "success",
    isError: false,
    text: "done",
    numTurns: 1,
    durationMs: 5,
    model: "m:free",
  });
  send({
    type: "toolResult",
    sessionId: "s1",
    toolUseId: "t1",
    content: "ok",
    isError: false,
  });
  send({ type: "message", sessionId: "s1", text: "ok" });
  assert.deepEqual(received, [
    { type: "toolResult", sessionId: "s1", toolUseId: "t1", content: "ok", isError: false },
    { type: "message", sessionId: "s1", text: "ok" },
  ]);
  setEventListen(null);
});

test("session-events: real Rust-serialized wire bytes are delivered for all variants", async () => {
  // V-01 / #65: the wire lock. These JSON strings are copied verbatim
  // from the Rust serialize output (serde_json::to_string of each
  // SenEvent variant — the Rust test
  // sen_event_serializes_with_camel_case_fields locks the producer
  // side). They are parsed, not hand-written as objects, so this test
  // fails if the backend ever goes back to snake_case fields: the old
  // hand-written camelCase payloads could not catch that, and the
  // validator dropped 100% of real events because of it.
  const wireJson = [
    '{"type":"sessionStart","sessionId":"s1","model":"m"}',
    '{"type":"message","sessionId":"s1","text":"hi"}',
    '{"type":"message","sessionId":"s1","text":"","toolCalls":[{"function":{"arguments":"{}","name":"Read"},"id":"c1","type":"function"}]}',
    '{"type":"messageUpdate","sessionId":"s1","delta":"hel"}',
    '{"type":"toolResult","sessionId":"s1","toolUseId":"t1","content":"ok","isError":false}',
    '{"type":"system","sessionId":"s1","subtype":"tool_progress"}',
    '{"type":"result","sessionId":"s1","subtype":"success","isError":false,"text":"done","numTurns":2,"usage":{"inputTokens":10,"outputTokens":5,"cacheReadInputTokens":1},"durationMs":123,"model":"m"}',
    '{"type":"unknown","sessionId":"s1","recordType":"future_thing","subtype":"x"}',
  ];
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  for (const json of wireJson) {
    send(JSON.parse(json) as unknown);
  }
  assert.equal(received.length, wireJson.length);
  assert.deepEqual(
    received.map((event) => event.type),
    [
      "sessionStart",
      "message",
      "message",
      "messageUpdate",
      "toolResult",
      "system",
      "result",
      "unknown",
    ],
  );
  const result = received[6];
  assert.equal(result.type, "result");
  if (result.type === "result") {
    assert.equal(result.numTurns, 2);
    assert.deepEqual(result.usage, {
      inputTokens: 10,
      outputTokens: 5,
      cacheReadInputTokens: 1,
    });
  }
  setEventListen(null);
});

test("session-events: event name matches the Rust backend constant", () => {
  assert.equal(SEN_SESSION_EVENT, "sen-session-event");
});

test("session-events: Rust-generated wire fixture passes the validator for all variants", async () => {
  // R2-01 / #65 follow-up: the in-repo link between the Rust producer
  // and this validator. The fixture (./fixtures/sen-events-wire.json)
  // is NOT hand-written: it is the real serde_json::to_string output
  // of each SenEvent variant, generated by the Rust test
  // sen_event_wire_fixture_matches_serde_output in
  // src-tauri/src/commands/stream_json.rs (SEN_REGEN_WIRE_FIXTURE=1
  // to regenerate; see tests/fixtures/README.md), and that same Rust
  // test asserts the committed fixture still matches serde output
  // byte-for-byte. So this test fails if either side drifts: a Rust
  // field back to snake_case breaks the Rust fixture test first, and
  // a validator that no longer accepts real wire bytes fails here.
  // The older "real Rust-serialized wire bytes" test above keeps its
  // copied literals as a second lock; this test is the generated one.
  const fixtureUrl = new URL(
    "./fixtures/sen-events-wire.json",
    import.meta.url,
  );
  const wireJson = JSON.parse(
    readFileSync(fixtureUrl, "utf8"),
  ) as string[];
  assert.equal(wireJson.length, 8);
  const received: SenEvent[] = [];
  const captured: { send?: SendFn } = {};
  setEventListen(fakeListen(captured));
  await listenSessionEvents("s1", (e) => received.push(e));
  const send = captured.send as SendFn;
  for (const json of wireJson) {
    // Parse each fixture entry exactly as it arrived on the wire:
    // no hand-built objects, no field renames in between.
    send(JSON.parse(json) as unknown);
  }
  assert.equal(received.length, wireJson.length);
  assert.deepEqual(
    received.map((event) => event.type),
    [
      "sessionStart",
      "message",
      "message",
      "messageUpdate",
      "toolResult",
      "system",
      "result",
      "unknown",
    ],
  );
  const result = received[6];
  assert.equal(result.type, "result");
  if (result.type === "result") {
    assert.equal(result.numTurns, 2);
    assert.deepEqual(result.usage, {
      inputTokens: 10,
      outputTokens: 5,
      cacheReadInputTokens: 1,
    });
  }
  setEventListen(null);
});
