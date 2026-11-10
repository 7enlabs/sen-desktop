// Live client tests for the live session round (tin 4528/4542).
// Run with: node --test apps/desktop/tests/live-client.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected transport stands in for the Rust handlers, which cannot run
// under node, and the injected handle factory stands in for the core live
// session builder. These tests prove the frontend half of the live round:
// a factory failure refuses before any backend call, the success path
// binds the handle and streams events in order, close unsubscribes then
// aborts, a backend failure disposes the handle, and tool execution is
// still refused. They do NOT prove the Rust handlers run or a real model
// answers: that evidence is the core live-session suite, the standalone
// registry tests, cargo check on the Leader VM, and the env-gated smoke.
import test from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import { abortSession, callTool } from "../src/ipc/session-client.ts";
import {
  attachLiveSession,
  closeLiveSession,
} from "../src/ipc/live-client.ts";
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

function fakeHandle() {
  const calls: string[] = [];
  const handle = {
    calls,
    steer: async (text: string) => {
      calls.push("steer:" + text);
    },
    followUp: async (text: string) => {
      calls.push("followUp:" + text);
    },
    abort: async () => {
      calls.push("abort");
    },
    subscribe: (listener: (event: unknown) => void) => {
      calls.push("subscribe");
      listener({ type: "message_start" });
      listener({ type: "message_update", delta: "hi" });
      listener({ type: "message_end" });
      return () => {
        calls.push("unsubscribe");
      };
    },
    dispose: () => {
      calls.push("dispose");
    },
  };
  return handle;
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

test("live-client attach: factory failure refuses before any backend call", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command) => {
    calls.push(command);
    return {};
  });
  const manager = new SessionManager();
  await assert.rejects(
    () =>
      attachLiveSession(manager, contract(), "s1", {
        createHandle: async () => {
          throw new Error("No model channel attached: no live session");
        },
      }),
    /No model channel attached/,
  );
  assert.deepEqual(calls, []);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("live-client attach: success binds the handle and streams events in order", async () => {
  const transportCalls = liveTransport();
  const events: unknown[] = [];
  const manager = new SessionManager();
  const handle = fakeHandle();
  const attached = await attachLiveSession(manager, contract(), "s1", {
    createHandle: async () => handle as never,
    onEvent: (event) => {
      events.push(event);
    },
  });
  assert.equal(attached.sessionId, "s1");
  assert.equal(attached.state, "Running");
  assert.equal(attached.subscriberId, 7);
  assert.equal(manager.getSdkSession("s1"), handle);
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
  ]);
  assert.deepEqual(events, [
    { type: "message_start" },
    { type: "message_update", delta: "hi" },
    { type: "message_end" },
  ]);
  assert.deepEqual(handle.calls, ["subscribe"]);
  setBridgeTransport(null);
});

test("live-client close: unsubscribe first, then backend + SDK abort + local Cancelled", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  const handle = fakeHandle();
  const attached = await attachLiveSession(manager, contract(), "s1", {
    createHandle: async () => handle as never,
  });
  const res = await closeLiveSession(manager, attached, contract(), "s1");
  assert.deepEqual(res, { sessionId: "s1", state: "Cancelled" });
  assert.deepEqual(transportCalls, [
    "sen_session_create",
    "sen_events_subscribe",
    "sen_session_abort",
  ]);
  assert.deepEqual(handle.calls, [
    "subscribe",
    "unsubscribe",
    "abort",
    "dispose",
  ]);
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  setBridgeTransport(null);
});

test("live-client attach: backend failure disposes the handle, no local record", async () => {
  setBridgeTransport(async () => {
    throw new Error("backend down");
  });
  const manager = new SessionManager();
  const handle = fakeHandle();
  await assert.rejects(
    () =>
      attachLiveSession(manager, contract(), "s1", {
        createHandle: async () => handle as never,
      }),
    /backend down/,
  );
  assert.deepEqual(handle.calls, ["dispose"]);
  assert.throws(() => manager.transition("s1", "Running"), /does not exist/);
  setBridgeTransport(null);
});

test("live-client abort: SDK abort throws, dispose still runs and local still Cancelled, error surfaces (BUG-05)", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  manager.create("s1");
  const calls: string[] = [];
  const handle = {
    abort: async () => {
      calls.push("abort");
      throw new Error("SDK abort exploded");
    },
    dispose: () => {
      calls.push("dispose");
    },
  };
  manager.bindSdkSession("s1", handle as never);
  await assert.rejects(
    () => abortSession(manager, handle as never, contract(), "s1"),
    /SDK abort exploded/,
  );
  // Dispose ran despite the throwing abort, and local followed the
  // backend ack to Cancelled instead of staying Running.
  assert.deepEqual(calls, ["abort", "dispose"]);
  assert.deepEqual(
    manager.rawLog.eventsFor("s1").map((event) => event.kind),
    ["session_create_stub", "session_sdk_bound", "session_transition"],
  );
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  assert.deepEqual(transportCalls, ["sen_session_abort"]);
  setBridgeTransport(null);
});

test("live-client abort: dispose throws after a clean abort, local still Cancelled, error surfaces (BUG-05)", async () => {
  const transportCalls = liveTransport();
  const manager = new SessionManager();
  manager.create("s1");
  const calls: string[] = [];
  const handle = {
    abort: async () => {
      calls.push("abort");
    },
    dispose: () => {
      calls.push("dispose");
      throw new Error("SDK dispose exploded");
    },
  };
  manager.bindSdkSession("s1", handle as never);
  await assert.rejects(
    () => abortSession(manager, handle as never, contract(), "s1"),
    /SDK dispose exploded/,
  );
  assert.deepEqual(calls, ["abort", "dispose"]);
  assert.throws(() => manager.transition("s1", "Running"), /Terminal state/);
  assert.deepEqual(transportCalls, ["sen_session_abort"]);
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
