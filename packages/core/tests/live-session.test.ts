// Live session tests for the live session round (tin 4528/4542).
// Run with: node --test packages/core/tests/live-session.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// These tests use the REAL Pi SDK (1.5.0) and the REAL builtin OpenRouter
// catalog, with an in-memory credential store and no network call:
// runtime creation passes refreshOnCreate false + allowModelNetwork false,
// and no test here calls steer/followUp on a real session, so no model
// request ever leaves this box. The keys in this file are explicit test
// inputs ("test-key-not-real"), never real credentials, and one test
// proves the key never lands in the raw log. The real-network smoke test
// lives separately in live-smoke.test.ts and runs only with an explicit
// OPENROUTER_API_KEY + SEN_LIVE_SMOKE_MODEL in the environment.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  OPENROUTER_PROVIDER_ID,
  createOpenRouterRuntime,
  listOpenRouterFreeModels,
  resolveOpenRouterApiKey,
  resolveOpenRouterModel,
} from "../src/model-channel.ts";
import {
  SenAgentSession,
  createLiveSenAgentSession,
} from "../src/agent-session.ts";
import { startOpenRouterLiveSession } from "../src/live-session.ts";
import { SessionManager } from "../src/session.ts";

// The model id below is an EXPLICIT TEST INPUT, not a default: it is
// passed as an argument to every call that needs one, and the production
// code names no model id anywhere. It is in the pinned SDK 1.5.0 builtin
// OpenRouter catalog (verified by probe before this round).
const TEST_MODEL_ID = "cohere/north-mini-code:free";

async function tempDirs(): Promise<{ cwd: string; agentDir: string }> {
  const base = await mkdtemp(join(tmpdir(), "sen-live-test-"));
  return { cwd: join(base, "cwd"), agentDir: join(base, "agent") };
}

test("model-channel: missing or blank key refuses, no fallback", () => {
  assert.throws(() => resolveOpenRouterApiKey({}), /not configured/);
  assert.throws(
    () => resolveOpenRouterApiKey({ OPENROUTER_API_KEY: "   " }),
    /not configured/,
  );
  assert.equal(
    resolveOpenRouterApiKey({ OPENROUTER_API_KEY: "  k-test  " }),
    "k-test",
  );
});

test("model-channel: runtime builds offline with an explicit key, openrouter present", async () => {
  const runtime = await createOpenRouterRuntime({ apiKey: "test-key-not-real" });
  assert.ok(runtime.getProvider(OPENROUTER_PROVIDER_ID));
  assert.equal(runtime.hasConfiguredAuth(OPENROUTER_PROVIDER_ID), true);
  const free = listOpenRouterFreeModels(runtime);
  assert.ok(free.includes(TEST_MODEL_ID));
});

test("model-channel: model resolution is exact, no fallback", async () => {
  const runtime = await createOpenRouterRuntime({ apiKey: "test-key-not-real" });
  assert.throws(() => resolveOpenRouterModel(runtime, ""), /No model specified/);
  assert.throws(
    () => resolveOpenRouterModel(runtime, "definitely-not-a-model-xyz"),
    /not found/,
  );
  const model = resolveOpenRouterModel(runtime, TEST_MODEL_ID) as {
    id: string;
  };
  assert.equal(model.id, TEST_MODEL_ID);
});

test("live session creation refuses without runtime or without model", async () => {
  await assert.rejects(
    () => createLiveSenAgentSession({} as never),
    /No model channel attached/,
  );
  const runtime = await createOpenRouterRuntime({ apiKey: "test-key-not-real" });
  await assert.rejects(
    () =>
      createLiveSenAgentSession({
        modelRuntime: runtime,
        model: undefined,
      } as never),
    /No model specified/,
  );
});

test("live session creation with an explicit model creates a real SDK session, no network call", async () => {
  const runtime = await createOpenRouterRuntime({ apiKey: "test-key-not-real" });
  const model = resolveOpenRouterModel(runtime, TEST_MODEL_ID);
  const dirs = await tempDirs();
  const handle = await createLiveSenAgentSession({
    modelRuntime: runtime,
    model,
    cwd: dirs.cwd,
    agentDir: dirs.agentDir,
  });
  assert.equal(typeof handle.steer, "function");
  assert.equal(typeof handle.followUp, "function");
  assert.equal(typeof handle.abort, "function");
  assert.equal(typeof handle.subscribe, "function");
  // Subscribe + dispose only: no steer/followUp is called here, so no
  // model request is made and no key is ever used on the network.
  const off = handle.subscribe(() => {});
  assert.equal(typeof off, "function");
  off();
  handle.dispose();
});

test("startOpenRouterLiveSession binds a live handle and logs provider + model, never the key", async () => {
  const dirs = await tempDirs();
  const manager = new SessionManager();
  const live = await startOpenRouterLiveSession({
    sessionId: "s1",
    modelId: TEST_MODEL_ID,
    apiKey: "test-key-not-real-2",
    cwd: dirs.cwd,
    agentDir: dirs.agentDir,
    manager,
  });
  assert.equal(live.sessionId, "s1");
  assert.equal(live.providerId, OPENROUTER_PROVIDER_ID);
  assert.equal(live.modelId, TEST_MODEL_ID);
  assert.equal(manager.getSdkSession("s1"), live.handle);
  const events = manager.rawLog.eventsFor("s1");
  const kinds = events.map((event) => event.kind);
  // bindSdkSession appends session_sdk_bound by design (session.ts), so
  // the full order is create, bind, live-started.
  assert.deepEqual(kinds, [
    "session_create_stub",
    "session_sdk_bound",
    "session_live_started",
  ]);
  const started = events[2].payload as Record<string, unknown>;
  assert.equal(started["provider"], OPENROUTER_PROVIDER_ID);
  assert.equal(started["model"], TEST_MODEL_ID);
  assert.doesNotMatch(JSON.stringify(events), /test-key-not-real-2/);
  live.handle.dispose();
});

test("startOpenRouterLiveSession failure leaves local state Failed, never a bound handle", async () => {
  const dirs = await tempDirs();
  const manager = new SessionManager();
  await assert.rejects(
    () =>
      startOpenRouterLiveSession({
        sessionId: "s1",
        modelId: "definitely-not-a-model-xyz",
        apiKey: "test-key-not-real",
        cwd: dirs.cwd,
        agentDir: dirs.agentDir,
        manager,
      }),
    /not found/,
  );
  assert.equal(manager.getSdkSession("s1"), undefined);
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
});

test("SenAgentSession subscribe forwards stream events in order", () => {
  const received: unknown[] = [];
  const fake = {
    steer: async () => {},
    followUp: async () => {},
    abort: async () => {},
    subscribe: (listener: (event: unknown) => void) => {
      listener({ type: "message_start" });
      listener({ type: "message_update", delta: "hi" });
      listener({ type: "message_end" });
      return () => {};
    },
    dispose: () => {},
  };
  const handle = new SenAgentSession(fake as never);
  const off = handle.subscribe((event) => {
    received.push(event);
  });
  off();
  handle.dispose();
  assert.deepEqual(received, [
    { type: "message_start" },
    { type: "message_update", delta: "hi" },
    { type: "message_end" },
  ]);
});
