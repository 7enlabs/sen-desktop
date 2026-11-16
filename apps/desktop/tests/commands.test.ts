// Chat command parser tests (P0-1).
// Run with: node --test apps/desktop/tests/commands.test.ts
// Pure function, no mocks: every command form and every
// not-a-command case.
import test from "node:test";
import assert from "node:assert/strict";
import { COMMAND_USAGE, parseChatCommand } from "../src/chat/commands.ts";

test("commands: /nhớ and /tôi add facts to the right scope", () => {
  assert.deepEqual(parseChatCommand("/nhớ thích trà"), {
    kind: "memory-add",
    scope: "memory",
    text: "thích trà",
  });
  assert.deepEqual(parseChatCommand("/tôi tên là Zen"), {
    kind: "memory-add",
    scope: "user",
    text: "tên là Zen",
  });
});

test("commands: /quên and bare quên forget by number or text", () => {
  assert.deepEqual(parseChatCommand("/quên 2"), {
    kind: "memory-forget",
    scope: "memory",
    target: { number: 2 },
  });
  assert.deepEqual(parseChatCommand("quên 2"), {
    kind: "memory-forget",
    scope: "memory",
    target: { number: 2 },
  });
  assert.deepEqual(parseChatCommand("/quên trà xanh"), {
    kind: "memory-forget",
    scope: "memory",
    target: { text: "trà xanh" },
  });
  assert.deepEqual(parseChatCommand("quên tôi 1"), {
    kind: "memory-forget",
    scope: "user",
    target: { number: 1 },
  });
  assert.deepEqual(parseChatCommand("/quên tôi tên cũ"), {
    kind: "memory-forget",
    scope: "user",
    target: { text: "tên cũ" },
  });
});

test("commands: /sửa edits by number, /sửa soul replaces the persona", () => {
  assert.deepEqual(parseChatCommand("/sửa 1 thích cà phê"), {
    kind: "memory-edit",
    scope: "memory",
    index: 1,
    text: "thích cà phê",
  });
  assert.deepEqual(parseChatCommand("/sửa tôi 2 tên mới"), {
    kind: "memory-edit",
    scope: "user",
    index: 2,
    text: "tên mới",
  });
  assert.deepEqual(parseChatCommand("/sửa soul Thẳng thắn, ít chữ."), {
    kind: "soul-set",
    text: "Thẳng thắn, ít chữ.",
  });
});

test("commands: /xem lists, /nhật ký shows today", () => {
  assert.deepEqual(parseChatCommand("/xem"), { kind: "memory-list" });
  assert.deepEqual(parseChatCommand("/nhật ký"), { kind: "memory-log" });
  assert.deepEqual(parseChatCommand("  /xem  "), { kind: "memory-list" });
});

test("commands: an unknown slash command answers with usage, not the model", () => {
  const parsed = parseChatCommand("/không rõ");
  assert.equal(parsed?.kind, "unknown");
  assert.ok(COMMAND_USAGE.includes("/nhớ"));
  assert.ok(COMMAND_USAGE.includes("/quên"));
  assert.deepEqual(parseChatCommand("/quên"), {
    kind: "unknown",
    text: "/quên",
  });
  assert.deepEqual(parseChatCommand("/sửa 1"), {
    kind: "unknown",
    text: "/sửa 1",
  });
});

test("commands: ordinary sentences are not commands", () => {
  // Bare "nhớ ..." starts ordinary sentences, so it needs the slash.
  assert.equal(parseChatCommand("nhớ hôm qua trời mưa"), null);
  assert.equal(parseChatCommand("nhớ mua sữa"), null);
  assert.equal(parseChatCommand("xin chào Sen"), null);
  assert.equal(parseChatCommand("tôi quên mật khẩu rồi"), null);
  assert.equal(parseChatCommand("  "), null);
  assert.equal(parseChatCommand(""), null);
});
