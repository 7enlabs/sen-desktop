// Tests for the RawLog retention cap (V-03, Sen issue #65 follow-up).
// Run with: node --test packages/core/tests/raw-log.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The cap is exercised through a small constructor bound so the tests stay
// fast; the production default (MAX_RAW_LOG_EVENTS) is asserted, not
// filled, because appending 10,000 events in a test would be slow and
// prove nothing the small-cap tests do not already prove.
import test from "node:test";
import assert from "node:assert/strict";
import { RawLog, MAX_RAW_LOG_EVENTS } from "../src/raw-log.ts";

test("RawLog: default cap is 10000 events", () => {
  assert.equal(MAX_RAW_LOG_EVENTS, 10000);
  const log = new RawLog();
  for (let i = 0; i < 3; i += 1) log.append("s1", "session_event", { i });
  assert.equal(log.eventsFor("s1").length, 3);
  assert.deepEqual(
    log.eventsFor("s1").map((e) => e.seq),
    [1, 2, 3],
  );
});

test("RawLog: under the cap, append/eventsFor/seq behave exactly as before", () => {
  const log = new RawLog(5);
  const a = log.append("s1", "session_create_stub", { note: "x" });
  const b = log.append("s1", "session_transition", { from: "Running", to: "Error" });
  const c = log.append("s2", "session_event", { text: "hi" });
  assert.deepEqual([a.seq, b.seq, c.seq], [1, 2, 3]);
  assert.deepEqual(
    log.eventsFor("s1").map((e) => e.kind),
    ["session_create_stub", "session_transition"],
  );
  assert.deepEqual(
    log.eventsFor("s2").map((e) => e.kind),
    ["session_event"],
  );
  assert.deepEqual(log.eventsFor("missing"), []);
});

test("RawLog: non-positive cap is rejected at construction", () => {
  assert.throws(() => new RawLog(0), /maxEvents must be a positive integer/);
  assert.throws(() => new RawLog(-1), /maxEvents must be a positive integer/);
});

test("RawLog: over the cap, evicts oldest first and keeps the N newest in order", () => {
  const log = new RawLog(5);
  for (let i = 0; i < 7; i += 1) {
    log.append("s1", "session_event", { i });
  }
  const kept = log.eventsFor("s1");
  assert.equal(kept.length, 5);
  // Oldest two (seq 1, 2) evicted; newest five kept in append order.
  assert.deepEqual(
    kept.map((e) => e.seq),
    [3, 4, 5, 6, 7],
  );
  assert.deepEqual(
    kept.map((e) => (e.payload as { i: number }).i),
    [2, 3, 4, 5, 6],
  );
});

test("RawLog: seq keeps increasing monotonically after eviction, never reused", () => {
  const log = new RawLog(3);
  for (let i = 0; i < 3; i += 1) log.append("s1", "session_event", { i });
  log.append("s1", "session_event", { i: 3 });
  log.append("s1", "session_event", { i: 4 });
  assert.deepEqual(
    log.eventsFor("s1").map((e) => e.seq),
    [3, 4, 5],
  );
  // The next event continues at 6, not back at 1: the seq gap (1-2 gone)
  // is what makes eviction observable instead of silent.
  assert.equal(log.append("s1", "session_event", { i: 5 }).seq, 6);
});

test("RawLog: eviction is global FIFO across sessions, not per-session", () => {
  const log = new RawLog(4);
  log.append("a", "session_event", { i: 0 });
  log.append("a", "session_event", { i: 1 });
  log.append("b", "session_event", { i: 2 });
  log.append("b", "session_event", { i: 3 });
  log.append("a", "session_event", { i: 4 });
  // Oldest event overall (session a, seq 1) is evicted, even though
  // session a still has newer events.
  assert.deepEqual(
    log.eventsFor("a").map((e) => e.seq),
    [2, 5],
  );
  assert.deepEqual(
    log.eventsFor("b").map((e) => e.seq),
    [3, 4],
  );
});
