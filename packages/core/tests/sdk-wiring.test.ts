// SDK wiring tests for the fill-in round (messages 4128/4138).
// Run AFTER pnpm install: this file imports the real Pi SDK through
// packages/core/src/agent-session.ts. The default bundle test script
// (logic.test.ts) stays install-free on purpose; run this file from an
// installed copy, e.g. after pnpm install --frozen-lockfile:
//   node --test packages/core/tests/sdk-wiring.test.ts
// These tests prove the import and the no-default-model guard. They open no
// live session, use no key, and make no network call.
import test from "node:test";
import assert from "node:assert/strict";
import {
  SDK_PACKAGE,
  SDK_PINNED_VERSION,
  SenAgentSession,
  createSenAgentSession,
  probeSdkWiring,
} from "../src/agent-session.ts";
import { SessionManager } from "../src/session.ts";

test("SDK wiring probe: real import resolves, pinned version", () => {
  const probe = probeSdkWiring();
  assert.equal(probe.sdkPackage, "@earendil-works/pi-coding-agent");
  assert.equal(probe.sdkVersion, "1.5.0");
  assert.equal(probe.createAgentSession, "function");
  assert.equal(SDK_PACKAGE, "@earendil-works/pi-coding-agent");
  assert.equal(SDK_PINNED_VERSION, "1.5.0");
});

test("createSenAgentSession refuses without an explicit model runtime", async () => {
  await assert.rejects(
    () => createSenAgentSession({} as never),
    /No model channel attached/,
  );
  await assert.rejects(
    () => createSenAgentSession({ modelRuntime: undefined } as never),
    /No model channel attached/,
  );
});

test("SenAgentSession maps steer/followUp/abort/subscribe to the SDK session", async () => {
  const calls: string[] = [];
  const fake = {
    steer: async (text: string) => {
      calls.push("steer:" + text);
    },
    followUp: async (text: string) => {
      calls.push("followUp:" + text);
    },
    abort: async () => {
      calls.push("abort");
    },
    subscribe: () => {
      calls.push("subscribe");
      return () => {};
    },
    dispose: () => {
      calls.push("dispose");
    },
  };
  const handle = new SenAgentSession(fake as never);
  await handle.steer("hello");
  await handle.followUp("again");
  await handle.abort();
  const off = handle.subscribe(() => {});
  off();
  handle.dispose();
  assert.deepEqual(calls, [
    "steer:hello",
    "followUp:again",
    "abort",
    "subscribe",
    "dispose",
  ]);
});

test("SessionManager binds an SDK session without changing the statechart", () => {
  const manager = new SessionManager();
  assert.equal(manager.create("s1"), "Running");
  const fake = {
    steer: async () => {},
    followUp: async () => {},
    abort: async () => {},
    subscribe: () => () => {},
    dispose: () => {},
  };
  assert.equal(manager.bindSdkSession("s1", fake as never), "Running");
  assert.equal(manager.getSdkSession("s1"), fake);
  assert.throws(
    () => manager.bindSdkSession("missing", fake as never),
    /does not exist/,
  );
});
