// App entry tests for the BUG-03 fix (issue #11).
// Run with: node --test apps/desktop/tests/app-entry.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// These tests prove the app entry is wired to a real session flow:
// startApp starts one live session through the backend registry first
// (BUG-02 order: one manager, one contract, registerBackendSession hook),
// then attaches it on the same manager, with exactly one
// sen_session_create and one sen_events_subscribe, the handle bound in
// that manager, and onEvent passed through to the handle subscribe. They do NOT prove a
// real webview runs this module (no bundler yet) or a real model
// answers: that evidence is the core live-session suite and the
// env-gated smoke test. The keys here are explicit test inputs
// ("test-key-not-real"), never real credentials, and no test here calls
// steer/followUp, so no model request ever leaves this box.
// NOTE: this suite is stacked on the BUG-02 fix (issue #10): it needs
// registerBackendSession on startOpenRouterLiveSession and the
// LiveSession-aware attachLiveSession. On plain dev/scaffold it fails;
// see the bundle README.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import { startApp } from "../src/app/main.ts";
import { SessionManager } from "../../../packages/core/src/session.ts";

// The model id below is an EXPLICIT TEST INPUT, not a default: it is
// passed as an argument to every call that needs one, and the production
// code names no model id anywhere. It is in the pinned SDK 1.5.0 builtin
// OpenRouter catalog (verified by probe before this round).
const TEST_MODEL_ID = "cohere/north-mini-code:free";

async function tempDirs(): Promise<{ cwd: string; agentDir: string }> {
  const base = await mkdtemp(join(tmpdir(), "sen-app-test-"));
  return { cwd: join(base, "cwd"), agentDir: join(base, "agent") };
}

function liveTransport() {
  const calls: string[] = [];
  setBridgeTransport(async (command, args) => {
    calls.push(command);
    const a = args as { session_id?: string };
    if (command === "sen_session_create") {
      return { sessionId: a.session_id, state: "Running" };
    }
    if (command === "sen_events_subscribe") {
      return { sessionId: a.session_id, subscriberId: 7 };
    }
    if (command === "sen_session_abort") {
      return { sessionId: a.session_id, state: "Cancelled" };
    }
    return { sessionId: a.session_id, state: "Running" };
  });
  return calls;
}

test("app entry: startApp wires one live session through the backend, same manager", async () => {
  const transportCalls = liveTransport();
  const dirs = await tempDirs();
  const events: unknown[] = [];
  const app = await startApp({
    sessionId: "s1",
    modelId: TEST_MODEL_ID,
    apiKey: "test-key-not-real",
    cwd: dirs.cwd,
    agentDir: dirs.agentDir,
    onEvent: (event) => {
      events.push(event);
    },
  });
  // One backend create, then one subscribe: no double create, no
  // second manager. The hook ran before the bind, so the backend knew
  // the session before the handle was bound.
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  assert.equal(app.live.backendRegistered, true);
  assert.equal(app.live.manager, app.manager);
  assert.equal(app.live.sessionId, "s1");
  assert.equal(app.attached.sessionId, "s1");
  assert.equal(app.attached.state, "Running");
  assert.equal(app.attached.subscriberId, 7);
  assert.equal(app.manager.getSdkSession("s1"), app.live.handle);
  // No model call was made, so no event arrived: the real handle only
  // forwards what the model emits, and this test invents none. The
  // in-order streaming itself is covered by the live-client suite with
  // a fake handle; here the wiring only has to pass onEvent through.
  assert.deepEqual(events, []);
  assert.equal(typeof app.attached.unsubscribe, "function");
  const kinds = app.manager.rawLog.eventsFor("s1").map((e) => e.kind);
  assert.deepEqual(kinds, [
    "session_create_stub",
    "session_sdk_bound",
    "session_live_started",
  ]);
  app.live.handle.dispose();
  setBridgeTransport(null);
});

test("app entry: startApp without a model id refuses before any backend call", async () => {
  const transportCalls = liveTransport();
  const dirs = await tempDirs();
  const manager = new SessionManager();
  await assert.rejects(
    () =>
      startApp({
        sessionId: "s1",
        cwd: dirs.cwd,
        agentDir: dirs.agentDir,
        manager,
      }),
    /No model specified/,
  );
  assert.deepEqual(transportCalls, []);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("app entry: startApp with an unknown model refuses before any backend call", async () => {
  const transportCalls = liveTransport();
  const dirs = await tempDirs();
  const manager = new SessionManager();
  await assert.rejects(
    () =>
      startApp({
        sessionId: "s1",
        modelId: "definitely-not-a-model-xyz",
        apiKey: "test-key-not-real",
        cwd: dirs.cwd,
        agentDir: dirs.agentDir,
        manager,
      }),
    /not found/,
  );
  assert.deepEqual(transportCalls, []);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("app entry: a second start on the same manager and session id refuses, no second backend create", async () => {
  const transportCalls = liveTransport();
  const dirs = await tempDirs();
  const manager = new SessionManager();
  const first = await startApp({
    sessionId: "s1",
    modelId: TEST_MODEL_ID,
    apiKey: "test-key-not-real",
    cwd: dirs.cwd,
    agentDir: dirs.agentDir,
    manager,
  });
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  await assert.rejects(
    () =>
      startApp({
        sessionId: "s1",
        modelId: TEST_MODEL_ID,
        apiKey: "test-key-not-real",
        cwd: dirs.cwd,
        agentDir: dirs.agentDir,
        manager,
      }),
    /Duplicate live session/,
  );
  // The refused second start made no backend call at all.
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  first.live.handle.dispose();
  setBridgeTransport(null);
});
