// Memory client tests (P0-1).
// Run with: node --test apps/desktop/tests/memory-client.test.ts
// The bridge transport is mocked: these tests lock the wire shape the
// Rust handlers expect (nested `args`, command names) and the shape
// validation of every result. They do NOT prove the Rust side reads
// files: that is the memory_store.rs suite in the harness.
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import {
  addMemory,
  editMemory,
  forgetMemoryByNumber,
  forgetMemoryByText,
  logMemory,
  readMemory,
  setSoul,
} from "../src/ipc/memory-client.ts";
import type { ActionContract } from "../src/ipc/types.ts";

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

afterEach(() => {
  setBridgeTransport(null);
});

test("memory-client read: command name and result shape", async () => {
  const seen: string[] = [];
  setBridgeTransport(async (command) => {
    seen.push(command);
    return { memory: ["a"], user: ["u"], soul: "s", today: ["t"] };
  });
  const result = await readMemory(contract());
  assert.deepEqual(seen, ["sen_memory_read"]);
  assert.deepEqual(result, {
    memory: ["a"],
    user: ["u"],
    soul: "s",
    today: ["t"],
  });
});

test("memory-client read: malformed result throws", async () => {
  setBridgeTransport(async () => ({ memory: "not an array" }));
  await assert.rejects(() => readMemory(contract()), /malformed/);
});

test("memory-client add: args nest under args, result shape", async () => {
  let captured: unknown = null;
  setBridgeTransport(async (command, args) => {
    captured = args;
    assert.equal(command, "sen_memory_add");
    return { scope: "memory", index: 2, item: "likes tea" };
  });
  const result = await addMemory(contract(), "memory", "likes tea");
  assert.deepEqual(captured, {
    contract: contract(),
    args: { scope: "memory", text: "likes tea" },
  });
  assert.deepEqual(result, { scope: "memory", index: 2, item: "likes tea" });
});

test("memory-client forget by number: target nests as {number}", async () => {
  let captured: unknown = null;
  setBridgeTransport(async (command, args) => {
    captured = args;
    assert.equal(command, "sen_memory_forget");
    return { scope: "memory", removed: "old fact" };
  });
  const result = await forgetMemoryByNumber(contract(), "memory", 2);
  assert.deepEqual(
    (captured as { args: unknown }).args,
    { scope: "memory", target: { number: 2 } },
  );
  assert.deepEqual(result, { scope: "memory", removed: "old fact" });
});

test("memory-client forget by text: target nests as {text}", async () => {
  let captured: unknown = null;
  setBridgeTransport(async (command, args) => {
    captured = args;
    assert.equal(command, "sen_memory_forget");
    return { scope: "user", removed: "name is Zen" };
  });
  const result = await forgetMemoryByText(contract(), "user", "Zen");
  assert.deepEqual(
    (captured as { args: unknown }).args,
    { scope: "user", target: { text: "Zen" } },
  );
  assert.deepEqual(result, { scope: "user", removed: "name is Zen" });
});

test("memory-client edit and set soul: args nest under args", async () => {
  const seen: Array<{ command: string; args: unknown }> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, args });
    if (command === "sen_memory_edit") {
      return { scope: "memory", index: 1, item: "new fact" };
    }
    return { soul: "Thẳng thắn." };
  });
  const edited = await editMemory(contract(), "memory", 1, "new fact");
  assert.deepEqual(edited, { scope: "memory", index: 1, item: "new fact" });
  const soul = await setSoul(contract(), "Thẳng thắn.");
  assert.deepEqual(soul, { soul: "Thẳng thắn." });
  assert.deepEqual(seen[0].args, {
    contract: contract(),
    args: { scope: "memory", index: 1, text: "new fact" },
  });
  assert.deepEqual(seen[1].args, {
    contract: contract(),
    args: { text: "Thẳng thắn." },
  });
});

test("memory-client log: args carry kind, channel, text", async () => {
  let captured: unknown = null;
  setBridgeTransport(async (command, args) => {
    captured = args;
    assert.equal(command, "sen_memory_log");
    return { logged: true };
  });
  const result = await logMemory(contract(), "assistant", "telegram", "hello");
  assert.deepEqual(
    (captured as { args: unknown }).args,
    { kind: "assistant", channel: "telegram", text: "hello" },
  );
  assert.deepEqual(result, { logged: true });
});
