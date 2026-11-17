// ChatSession tests (P0-1): the shared desktop + Telegram session.
// Run with: node --test apps/desktop/tests/chat.test.ts
// The bridge transport is mocked (a steer/follow_up delivers its
// Result event synchronously, like the backend streaming a result
// record mid-turn, then acks), and the memory client is an in-memory
// fake. These tests lock: one transcript shared by both channels,
// steer first then follow_up, commands never reaching the model,
// the queue ordering turns, and failures becoming system entries
// instead of rejected sends. They do NOT prove the Rust side: the
// memory store is the memory_store.rs suite, the model turn is the
// live-client + bridge suites, and a live model turn is NOT RUN here.
//
// Note on assertions: TranscriptEntry carries id/ts (and
// telegramChatId, undefined for desktop entries), so tests compare
// projections (role/channel/text[/telegramChatId]), not whole entries.
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import {
  ChatSession,
  type ChatChannel,
  type MemoryClient,
  type TranscriptEntry,
} from "../src/chat/chat.ts";
import type {
  MemoryReadResult,
  MemoryScope,
} from "../src/ipc/memory-client.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import type { SenEvent } from "../src/ipc/session-events.ts";

const SESSION_ID = "s1";

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

function resultEvent(text: string, isError = false): SenEvent {
  return {
    type: "result",
    sessionId: SESSION_ID,
    subtype: "success",
    isError,
    text,
    numTurns: 1,
    usage: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 0 },
    durationMs: 5,
    model: "test-model",
  };
}

// Projection for assertions: the stable fields of an entry.
function view(entries: TranscriptEntry[]) {
  return entries.map((entry) => ({
    role: entry.role,
    channel: entry.channel,
    text: entry.text,
    ...(entry.telegramChatId !== undefined
      ? { telegramChatId: entry.telegramChatId }
      : {}),
  }));
}

// In-memory stand-in for the file memory: same interface, same
// exactly-one-match rule for forget-by-text, no files.
class FakeMemory implements MemoryClient {
  memory: string[] = [];
  user: string[] = [];
  soul = "Bạn là Sen, trợ lý của người dùng.";
  today: string[] = [];
  logs: Array<{ kind: string; channel: string; text: string }> = [];

  async read(_contract: ActionContract): Promise<MemoryReadResult> {
    return {
      memory: [...this.memory],
      user: [...this.user],
      soul: this.soul,
      today: [...this.today],
    };
  }

  private list(scope: MemoryScope): string[] {
    return scope === "memory" ? this.memory : this.user;
  }

  async add(_contract: ActionContract, scope: MemoryScope, text: string) {
    const list = this.list(scope);
    list.push(text);
    return { scope, index: list.length, item: text };
  }

  async forgetByNumber(
    _contract: ActionContract,
    scope: MemoryScope,
    index: number,
  ) {
    const list = this.list(scope);
    if (index < 1 || index > list.length) {
      throw new Error("no item " + index);
    }
    const removed = list.splice(index - 1, 1)[0];
    return { scope, removed };
  }

  async forgetByText(
    _contract: ActionContract,
    scope: MemoryScope,
    text: string,
  ) {
    const list = this.list(scope);
    const matches = list.filter((item) => item.includes(text));
    if (matches.length === 0) {
      throw new Error("không tìm thấy mục nào chứa “" + text + "”");
    }
    if (matches.length > 1) {
      throw new Error(
        "tìm thấy " + matches.length + " mục chứa “" + text + "”",
      );
    }
    const removed = matches[0];
    list.splice(list.indexOf(removed), 1);
    return { scope, removed };
  }

  async edit(
    _contract: ActionContract,
    scope: MemoryScope,
    index: number,
    text: string,
  ) {
    const list = this.list(scope);
    if (index < 1 || index > list.length) {
      throw new Error("no item " + index);
    }
    list[index - 1] = text;
    return { scope, index, item: text };
  }

  async setSoul(_contract: ActionContract, text: string) {
    this.soul = text;
    return { soul: text };
  }

  async log(
    _contract: ActionContract,
    kind: string,
    channel: string,
    text: string,
  ) {
    this.logs.push({ kind, channel, text });
    this.today.push(kind + ": " + text);
  }
}

interface ChatHarness {
  chat: ChatSession;
  memory: FakeMemory;
  sent: Array<{ chatId: number; text: string }>;
  commands: string[];
  texts: string[];
  fail: { steer: boolean; errorResult: boolean };
  transcripts: TranscriptEntry[][];
}

function makeChat(): ChatHarness {
  const memory = new FakeMemory();
  const sent: Array<{ chatId: number; text: string }> = [];
  const commands: string[] = [];
  const texts: string[] = [];
  const fail = { steer: false, errorResult: false };
  const transcripts: TranscriptEntry[][] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory,
    sendTelegram: async (chatId, text) => {
      sent.push({ chatId, text });
    },
    onTranscript: (entries) => {
      transcripts.push([...entries]);
    },
  });
  chat.bind(SESSION_ID);
  setBridgeTransport(async (command, args) => {
    commands.push(command);
    if (command === "sen_session_steer" || command === "sen_session_follow_up") {
      if (fail.steer) {
        throw new Error("boom");
      }
      const { text } = args as unknown as { session_id: string; text: string };
      texts.push(text);
      // Deliver the Result event synchronously, mid-turn, like the
      // backend streaming a result record before the ack.
      chat.handleEvent(
        resultEvent(
          fail.errorResult ? "HTTP 401: bad key" : "reply to " + text,
          fail.errorResult,
        ),
      );
      return {
        sessionId: SESSION_ID,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    throw new Error("unexpected command " + command);
  });
  return { chat, memory, sent, commands, texts, fail, transcripts };
}

afterEach(() => {
  setBridgeTransport(null);
});

test("chat: desktop send runs one steer turn, transcript in order", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "hi");
  assert.deepEqual(h.commands, ["sen_session_steer"]);
  assert.deepEqual(h.texts, ["hi"]);
  assert.deepEqual(view(h.chat.getTranscript()), [
    { role: "user", channel: "desktop", text: "hi" },
    { role: "assistant", channel: "desktop", text: "reply to hi" },
  ]);
  assert.equal(h.chat.isBusy(), false);
  // The turn was logged (user + assistant) in the daily log.
  assert.deepEqual(
    h.memory.logs.map((entry) => entry.kind),
    ["user", "assistant"],
  );
});

test("chat: second send uses follow_up, first turn used steer", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "one");
  await h.chat.send("desktop", "two");
  assert.deepEqual(h.commands, [
    "sen_session_steer",
    "sen_session_follow_up",
  ]);
  assert.deepEqual(
    view(h.chat.getTranscript()).map((entry) => entry.text),
    ["one", "reply to one", "two", "reply to two"],
  );
});

test("chat: /nhớ runs locally, never reaches the model", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/nhớ thích trà");
  assert.deepEqual(h.commands, []);
  assert.deepEqual(h.memory.memory, ["thích trà"]);
  // A command is not echoed as a user entry: only its confirmation
  // lands in the transcript (the raw command text goes to the daily
  // log, kind "memory").
  assert.deepEqual(view(h.chat.getTranscript()), [
    {
      role: "system",
      channel: "desktop",
      text: 'Đã ghi nhớ: "thích trà" (mục 1).',
    },
  ]);
  assert.ok(
    h.memory.logs.some(
      (entry) => entry.kind === "memory" && entry.text.includes("thích trà"),
    ),
  );
});

test("chat: /quên by number removes the item, locally", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/nhớ thích trà");
  await h.chat.send("desktop", "/nhớ thích cà phê");
  await h.chat.send("desktop", "/quên 1");
  assert.deepEqual(h.commands, []);
  assert.deepEqual(h.memory.memory, ["thích cà phê"]);
  const entries = h.chat.getTranscript();
  const last = entries[entries.length - 1];
  assert.equal(last.role, "system");
  assert.equal(last.text, 'Đã quên mục 1: "thích trà".');
});

test("chat: /quên by text with no match reports the failure, keeps memory", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/nhớ thích trà");
  await h.chat.send("desktop", "/quên bánh mì");
  assert.deepEqual(h.commands, []);
  assert.deepEqual(h.memory.memory, ["thích trà"]);
  const entries = h.chat.getTranscript();
  const last = entries[entries.length - 1];
  assert.equal(last.role, "system");
  assert.match(last.text, /Không làm được/);
  assert.match(last.text, /bánh mì/);
});

test("chat: /xem lists facts numbered, /nhật ký shows today", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/nhớ thích trà");
  await h.chat.send("desktop", "/tôi tên là Zen");
  await h.chat.send("desktop", "/xem");
  let entries = h.chat.getTranscript();
  let listed = entries[entries.length - 1];
  assert.equal(listed.role, "system");
  assert.match(listed.text, /1\. thích trà/);
  assert.match(listed.text, /1\. tên là Zen/);
  await h.chat.send("desktop", "chào");
  await h.chat.send("desktop", "/nhật ký");
  entries = h.chat.getTranscript();
  const logged = entries[entries.length - 1];
  assert.equal(logged.role, "system");
  assert.match(logged.text, /user: chào/);
});

test("chat: two sends queue in order, never interleave", async () => {
  const h = makeChat();
  const first = h.chat.send("desktop", "one");
  const second = h.chat.send("desktop", "two");
  await Promise.all([first, second]);
  assert.deepEqual(h.commands, [
    "sen_session_steer",
    "sen_session_follow_up",
  ]);
  assert.deepEqual(
    view(h.chat.getTranscript()).map((entry) => entry.text),
    ["one", "reply to one", "two", "reply to two"],
  );
});

test("chat: Telegram send shares the transcript, reply goes back to the same chat", async () => {
  const h = makeChat();
  await h.chat.send("telegram", "hi", 100);
  assert.deepEqual(h.commands, ["sen_session_steer"]);
  assert.deepEqual(view(h.chat.getTranscript()), [
    { role: "user", channel: "telegram", telegramChatId: 100, text: "hi" },
    {
      role: "assistant",
      channel: "telegram",
      telegramChatId: 100,
      text: "reply to hi",
    },
  ]);
  assert.deepEqual(h.sent, [{ chatId: 100, text: "reply to hi" }]);
  // The next Telegram turn follows up the same session, and the
  // desktop sees the same transcript.
  await h.chat.send("telegram", "again", 100);
  assert.deepEqual(h.commands[1], "sen_session_follow_up");
  assert.equal(h.chat.getTranscript().length, 4);
});

test("chat: a failed steer becomes a system entry, and Telegram gets the error text", async () => {
  const h = makeChat();
  h.fail.steer = true;
  await h.chat.send("telegram", "hi", 100);
  let entries = h.chat.getTranscript();
  let last = entries[entries.length - 1];
  assert.equal(last.role, "system");
  assert.match(last.text, /Sen không trả lời được: boom/);
  assert.deepEqual(h.sent, [
    { chatId: 100, text: "Sen không trả lời được: boom" },
  ]);
  // send() itself never rejects: awaiting a second failed send
  // resolves, and its failure is another system entry.
  await h.chat.send("desktop", "still here");
  entries = h.chat.getTranscript();
  last = entries[entries.length - 1];
  assert.equal(last.text.includes("boom"), true);
});

test("chat: an is_error result becomes a system entry, never an assistant reply", async () => {
  const h = makeChat();
  h.fail.errorResult = true;
  await h.chat.send("desktop", "hi");
  assert.deepEqual(view(h.chat.getTranscript()), [
    { role: "user", channel: "desktop", text: "hi" },
    {
      role: "system",
      channel: "desktop",
      text: "Sen gặp lỗi: HTTP 401: bad key",
    },
  ]);
  // The error text is not logged as an assistant line in the daily
  // log: only the user line was logged.
  assert.deepEqual(
    h.memory.logs.map((entry) => entry.kind),
    ["user"],
  );
});

test("chat: an is_error Telegram turn reports the error, never sends it as a reply", async () => {
  const h = makeChat();
  h.fail.errorResult = true;
  await h.chat.send("telegram", "hi", 100);
  assert.deepEqual(view(h.chat.getTranscript()), [
    { role: "user", channel: "telegram", telegramChatId: 100, text: "hi" },
    {
      role: "system",
      channel: "telegram",
      telegramChatId: 100,
      text: "Sen gặp lỗi: HTTP 401: bad key",
    },
  ]);
  // Telegram gets the labelled error notice, not the raw error text
  // as if it were the model's reply.
  assert.deepEqual(h.sent, [
    { chatId: 100, text: "Sen gặp lỗi: HTTP 401: bad key" },
  ]);
});

test("chat: send before bind reports no session, runs nothing", async () => {
  const memory = new FakeMemory();
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory,
  });
  await chat.send("desktop", "hi");
  // No session: no user entry is recorded, only the system notice.
  assert.deepEqual(view(chat.getTranscript()), [
    {
      role: "system",
      channel: "desktop",
      text: "Chưa có phiên làm việc.",
    },
  ]);
});

test("chat: unknown slash command answers with usage, bare nhớ goes to the model", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/không rõ");
  assert.deepEqual(h.commands, []);
  let entries = h.chat.getTranscript();
  let last = entries[entries.length - 1];
  assert.equal(last.role, "system");
  assert.match(last.text, /\/nhớ/);
  await h.chat.send("desktop", "nhớ hôm qua trời mưa");
  assert.deepEqual(h.commands, ["sen_session_steer"]);
});

test("chat: transcript snapshots are copies, onTranscript fires per change", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "hi");
  // user entry + assistant entry: two transcript updates.
  assert.equal(h.transcripts.length, 2);
  assert.deepEqual(view(h.transcripts[0]), [
    { role: "user", channel: "desktop", text: "hi" },
  ]);
  const first = h.chat.getTranscript();
  first.push({
    id: 99,
    channel: "desktop",
    role: "system",
    text: "mutated",
    ts: new Date().toISOString(),
  });
  assert.equal(h.chat.getTranscript().length, 2);
});
