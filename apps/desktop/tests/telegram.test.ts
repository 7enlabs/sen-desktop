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
