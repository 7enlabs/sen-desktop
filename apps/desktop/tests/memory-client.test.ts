// Memory client tests (P0-1).
// Run with: node --test apps/desktop/tests/memory-client.test.ts
// The bridge transport is mocked: these tests lock the wire shape the
// Rust handlers expect (nested `args`, command names) and the shape
// validation of every result. They do NOT prove the Rust side reads
// files: that is the memory_store.rs suite in the harness.
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
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

// --- CI P2-6: wire shape locked against memory.rs ---
// Same lesson as settings-client R2B-05 rev2: the v1 mock-transport
// tests only assert what the client sends, so a client that sends a
// flat payload while the handler waits for `args: <Args>` passes
// under node and fails on every real call ("missing required key
// args"). These tests read each handler signature out of memory.rs
// and assert the client payload against it, so they FAIL on a wrong
// payload and PASS only when the client sends the shape the handler
// waits for. They do NOT prove the Rust handlers read/write files:
// that evidence is the memory_store.rs suite in the harness.
function rustMemorySource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/memory.rs", import.meta.url),
    "utf8",
  );
}

function toCamelCase(snake: string): string {
  return snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

/// Expected wire shape for a command with an `args: <Args>` struct,
/// derived from the Rust handler: the struct-typed parameter name
/// (e.g. "args") plus the camelCase field names of its args struct.
function expectedWireShape(
  src: string,
  command: string,
): { paramKey: string; fields: string[] } {
  const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
  assert.ok(fnMatch, `Rust handler ${command} not found in memory.rs`);
  const argsMatch = fnMatch[1].match(/(\w+):\s*(\w+Args)/);
  assert.ok(argsMatch, `${command} declares no *Args parameter in memory.rs`);
  const paramKey = argsMatch[1];
  const structName = argsMatch[2];
  const structMatch = src.match(new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\}`));
  assert.ok(structMatch, `Rust struct ${structName} not found in memory.rs`);
  const structHead = src.slice(Math.max(0, (structMatch.index ?? 0) - 200), structMatch.index ?? 0);
  assert.match(structHead, /rename_all = "camelCase"/, `${structName} is not camelCase-renamed`);
  const fields = [...structMatch[1].matchAll(/pub (\w+):/g)].map((m) => toCamelCase(m[1]));
  assert.ok(fields.length > 0, `${structName} has no fields`);
  return { paramKey, fields };
}

/// The untagged ForgetTargetArgs enum: each variant name plus its
/// single field name, in source order.
function expectedForgetVariants(src: string): Array<[string, string]> {
  const enumMatch = src.match(/pub enum ForgetTargetArgs \{([\s\S]*?)\n\}/);
  assert.ok(enumMatch, "Rust enum ForgetTargetArgs not found in memory.rs");
  const variants = [...enumMatch[1].matchAll(/(\w+)\s*\{\s*(\w+):/g)].map(
    (m) => [m[1], m[2]] as [string, string],
  );
  assert.ok(variants.length > 0, "ForgetTargetArgs has no variants");
  return variants;
}

async function capturePayload(
  command: string,
  stubResult: unknown,
  call: () => Promise<unknown>,
): Promise<Record<string, unknown>> {
  let seen: Record<string, unknown> = {};
  setBridgeTransport(async (cmd, args) => {
    assert.equal(cmd, command);
    seen = args as Record<string, unknown>;
    return stubResult;
  });
  await call();
  setBridgeTransport(null);
  return seen;
}

test("memory-client add: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustMemorySource(), "sen_memory_add");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["scope", "text"]);
  const payload = await capturePayload(
    "sen_memory_add",
    { scope: "memory", index: 2, item: "likes tea" },
    () => addMemory(contract(), "memory", "likes tea"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { scope: "memory", text: "likes tea" });
  assert.ok(!("scope" in payload) && !("text" in payload), "payload must not carry flat scope/text keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("memory-client forget: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const src = rustMemorySource();
  const { paramKey, fields } = expectedWireShape(src, "sen_memory_forget");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["scope", "target"]);
  // The target is an untagged enum: exactly one of {number} / {text}.
  assert.deepEqual(expectedForgetVariants(src), [
    ["Number", "number"],
    ["Text", "text"],
  ]);
  const byNumber = await capturePayload(
    "sen_memory_forget",
    { scope: "memory", removed: "old fact" },
    () => forgetMemoryByNumber(contract(), "memory", 2),
  );
  assert.deepEqual(byNumber[paramKey], { scope: "memory", target: { number: 2 } });
  assert.ok(!("scope" in byNumber) && !("target" in byNumber), "payload must not carry flat scope/target keys");
  const byText = await capturePayload(
    "sen_memory_forget",
    { scope: "user", removed: "name is Zen" },
    () => forgetMemoryByText(contract(), "user", "Zen"),
  );
  assert.deepEqual(byText[paramKey], { scope: "user", target: { text: "Zen" } });
  assert.ok("contract" in byText, "payload must still carry the contract");
});

test("memory-client edit: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustMemorySource(), "sen_memory_edit");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["scope", "index", "text"]);
  const payload = await capturePayload(
    "sen_memory_edit",
    { scope: "memory", index: 1, item: "new fact" },
    () => editMemory(contract(), "memory", 1, "new fact"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { scope: "memory", index: 1, text: "new fact" });
  assert.ok(!("scope" in payload) && !("index" in payload) && !("text" in payload), "payload must not carry flat scope/index/text keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("memory-client set soul: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustMemorySource(), "sen_memory_set_soul");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["text"]);
  const payload = await capturePayload(
    "sen_memory_set_soul",
    { soul: "Thẳng thắn." },
    () => setSoul(contract(), "Thẳng thắn."),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { text: "Thẳng thắn." });
  assert.ok(!("text" in payload), "payload must not carry a flat text key");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("memory-client log: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustMemorySource(), "sen_memory_log");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["kind", "channel", "text"]);
  const payload = await capturePayload(
    "sen_memory_log",
    { logged: true },
    () => logMemory(contract(), "assistant", "telegram", "hello"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { kind: "assistant", channel: "telegram", text: "hello" });
  assert.ok(!("kind" in payload) && !("channel" in payload) && !("text" in payload), "payload must not carry flat kind/channel/text keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("memory-client read: no args key, contract only (CI P2-6)", async () => {
  const src = rustMemorySource();
  const fnMatch = src.match(/pub fn sen_memory_read\(([\s\S]*?)\) ->/);
  assert.ok(fnMatch, "Rust handler sen_memory_read not found in memory.rs");
  assert.doesNotMatch(fnMatch[1], /args:\s*\w+Args/, "sen_memory_read declares no args struct parameter in memory.rs");
  const payload = await capturePayload(
    "sen_memory_read",
    { memory: ["a"], user: ["u"], soul: "s", today: ["t"] },
    () => readMemory(contract()),
  );
  assert.deepEqual(Object.keys(payload), ["contract"]);
  assert.ok(!("args" in payload), "read takes no args struct, so the payload must not carry an args key");
});
