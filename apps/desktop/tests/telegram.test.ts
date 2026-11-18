// Telegram IPC + poll loop tests (P0-1).
// Run with: node --test apps/desktop/tests/telegram.test.ts
// The bridge transport is mocked for the wire tests; the loop tests
// inject a fake poll function and a fake chat. These tests lock the
// wire shape (nested `args`), the result validation, and the loop's
// guarantees: updates route in order, a failed round backs off 1s and
// retries, a failed update never kills the loop. They do NOT prove
// the Bot API itself: the client + poll offset are the
// telegram_client.rs suite in the harness, and a live bot is NOT RUN
// here (no token on this box).
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import {
  getTelegramStatus,
  pollTelegramOnce,
  sendTelegramMessage,
  startTelegramLoop,
  type TelegramPollResult,
  type TelegramUpdate,
} from "../src/chat/telegram.ts";
import type { ChatSession } from "../src/chat/chat.ts";
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

function update(updateId: number, chatId: number, text: string): TelegramUpdate {
  return { updateId, chatId, text, fromUsername: "zen" };
}

afterEach(() => {
  setBridgeTransport(null);
});

test("telegram: status, poll and send wire shapes", async () => {
  const seen: Array<{ command: string; args: unknown }> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, args });
    if (command === "sen_telegram_status") {
      return { configured: true, allowedChatIdSet: false, allowAnyChat: false };
    }
    if (command === "sen_telegram_poll_once") {
      return { updates: [update(1, 100, "hi")] };
    }
    if (command === "sen_telegram_send") {
      return { sent: true };
    }
    throw new Error("unexpected " + command);
  });
  const status = await getTelegramStatus(contract());
  assert.deepEqual(status, {
    configured: true,
    allowedChatIdSet: false,
    allowAnyChat: false,
  });
  const polled = await pollTelegramOnce(contract());
  assert.deepEqual(polled, { updates: [update(1, 100, "hi")] });
  const sent = await sendTelegramMessage(contract(), 100, "hello");
  assert.deepEqual(sent, { sent: true });
  assert.deepEqual(seen[0].args, { contract: contract() });
  assert.deepEqual(seen[1].args, { contract: contract() });
  assert.deepEqual(seen[2].args, {
    contract: contract(),
    args: { chatId: 100, text: "hello" },
  });
});

test("telegram: malformed poll result throws", async () => {
  setBridgeTransport(async () => ({
    updates: [{ updateId: "not a number", chatId: 100, text: "hi" }],
  }));
  await assert.rejects(() => pollTelegramOnce(contract()), /malformed/);
});

test("telegram loop: updates route to the chat in order, loop stops on stop()", async () => {
  const calls: Array<{ channel: string; text: string; chatId?: number }> = [];
  let polls = 0;
  const fakeChat = {
    send: async (channel: string, text: string, chatId?: number) => {
      calls.push({ channel, text, chatId });
    },
  } as unknown as ChatSession;
  const stop = startTelegramLoop(fakeChat, () => contract(), {
    pollOnce: async (): Promise<TelegramPollResult> => {
      // Yield to the macrotask queue each round, like the real
      // long-poll does: without this the loop spins on microtasks
      // alone and starves the test's timer.
      await new Promise((resolve) => setTimeout(resolve, 1));
      polls += 1;
      if (polls === 1) {
        return { updates: [update(1, 100, "one"), update(2, 100, "two")] };
      }
      return { updates: [] };
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  stop();
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(calls, [
    { channel: "telegram", text: "one", chatId: 100 },
    { channel: "telegram", text: "two", chatId: 100 },
  ]);
  assert.ok(polls >= 1);
});

test("telegram loop: a failed round backs off 1s and retries", async () => {
  const sleeps: number[] = [];
  let polls = 0;
  const fakeChat = {
    send: async () => {},
  } as unknown as ChatSession;
  const stop = startTelegramLoop(fakeChat, () => contract(), {
    pollOnce: async (): Promise<TelegramPollResult> => {
      // Yield to the macrotask queue each round, like the real
      // long-poll does (see the first loop test).
      await new Promise((resolve) => setTimeout(resolve, 1));
      polls += 1;
      if (polls === 1) {
        throw new Error("network down");
      }
      return { updates: [] };
    },
    sleep: async (ms: number) => {
      sleeps.push(ms);
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  stop();
  assert.deepEqual(sleeps, [1000]);
  assert.ok(polls >= 2);
});

test("telegram loop: a failed update does not kill the loop", async () => {
  const routed: string[] = [];
  let sends = 0;
  const fakeChat = {
    send: async (_channel: string, text: string) => {
      sends += 1;
      if (sends === 1) {
        throw new Error("chat send failed");
      }
      routed.push(text);
    },
  } as unknown as ChatSession;
  let polls = 0;
  const stop = startTelegramLoop(fakeChat, () => contract(), {
    pollOnce: async (): Promise<TelegramPollResult> => {
      // Yield to the macrotask queue each round, like the real
      // long-poll does (see the first loop test).
      await new Promise((resolve) => setTimeout(resolve, 1));
      polls += 1;
      if (polls === 1) {
        return { updates: [update(1, 100, "first"), update(2, 100, "second")] };
      }
      return { updates: [] };
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  stop();
  // The first update failed, the loop logged it and routed the second.
  assert.deepEqual(routed, ["second"]);
  assert.equal(sends, 2);
});

// --- CI P2-6: wire shape locked against telegram.rs ---
// Same lesson as settings-client R2B-05 rev2: the v1 mock-transport
// tests only assert what the client sends, so a client that sends a
// flat payload while the handler waits for `args: <Args>` passes
// under node and fails on every real call ("missing required key
// args"). These tests read each handler signature out of telegram.rs
// and assert the client payload against it, so they FAIL on a wrong
// payload and PASS only when the client sends the shape the handler
// waits for. They do NOT prove the Bot API itself: that evidence is
// the telegram_client.rs suite in the harness, and a live bot is NOT
// RUN here (no token on this box).
function rustTelegramSource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/telegram.rs", import.meta.url),
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
  assert.ok(fnMatch, `Rust handler ${command} not found in telegram.rs`);
  const argsMatch = fnMatch[1].match(/(\w+):\s*(\w+Args)/);
  assert.ok(argsMatch, `${command} declares no *Args parameter in telegram.rs`);
  const paramKey = argsMatch[1];
  const structName = argsMatch[2];
  const structMatch = src.match(new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\}`));
  assert.ok(structMatch, `Rust struct ${structName} not found in telegram.rs`);
  const structHead = src.slice(Math.max(0, (structMatch.index ?? 0) - 200), structMatch.index ?? 0);
  assert.match(structHead, /rename_all = "camelCase"/, `${structName} is not camelCase-renamed`);
  const fields = [...structMatch[1].matchAll(/pub (\w+):/g)].map((m) => toCamelCase(m[1]));
  assert.ok(fields.length > 0, `${structName} has no fields`);
  return { paramKey, fields };
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

test("telegram send: payload matches the Rust handler wire shape (CI P2-6)", async () => {
  const { paramKey, fields } = expectedWireShape(rustTelegramSource(), "sen_telegram_send");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["chatId", "text"]);
  const payload = await capturePayload(
    "sen_telegram_send",
    { sent: true },
    () => sendTelegramMessage(contract(), 100, "hello"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { chatId: 100, text: "hello" });
  assert.ok(!("chatId" in payload) && !("text" in payload), "payload must not carry flat chatId/text keys");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("telegram status and poll: no args key, contract only (CI P2-6)", async () => {
  const src = rustTelegramSource();
  for (const command of ["sen_telegram_status", "sen_telegram_poll_once"]) {
    const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
    assert.ok(fnMatch, `Rust handler ${command} not found in telegram.rs`);
    assert.doesNotMatch(fnMatch[1], /args:\s*\w+Args/, `${command} declares no args struct parameter in telegram.rs`);
  }
  const statusPayload = await capturePayload(
    "sen_telegram_status",
    { configured: true, allowedChatIdSet: false, allowAnyChat: false },
    () => getTelegramStatus(contract()),
  );
  assert.deepEqual(Object.keys(statusPayload), ["contract"]);
  assert.ok(!("args" in statusPayload), "status takes no args struct, so the payload must not carry an args key");
  const pollPayload = await capturePayload(
    "sen_telegram_poll_once",
    { updates: [] },
    () => pollTelegramOnce(contract()),
  );
  assert.deepEqual(Object.keys(pollPayload), ["contract"]);
  assert.ok(!("args" in pollPayload), "poll takes no args struct, so the payload must not carry an args key");
});
