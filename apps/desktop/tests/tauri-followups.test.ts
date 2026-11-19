// Follow-up tests (Tauri Rust commands, 19/11, ZC Leader tin 17093):
// the MCP delegate, the Gmail/calendar brief source, and the Rust
// SkillRegistry, as the frontend consumes them.
// Run with: node --test apps/desktop/tests/tauri-followups.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
//
// What these tests prove, end to end under node:
// - The three new IPC clients (mcp-delegate-client, brief-client,
//   skills-client) call the right Tauri commands with the right
//   wire args, validate the backend payloads, and surface backend
//   failures untouched — the Rust handlers themselves cannot run
//   under node, so a fake bridge transport stands in for them
//   (same pattern as the other *-client tests).
// - ChatSession with the Rust-backed options (async skillMatcher
//   over sen_skill_match, briefSource over sen_brief_source,
//   delegateToolCall over sen_mcp_delegate) routes a morning-brief
//   message to the brief flow, builds a real brief from the Rust
//   source data, runs /mcp through the Rust delegate, and still
//   sends unrelated messages to the model.
// - The webview import graph from main.ts reaches the new IPC
//   clients (they only import the bridge + types) and still reaches
//   NEITHER node-only module (mcp-client, skill-registry,
//   skill-matcher): the Rust commands own the fs/spawn now.
// They do NOT prove the Rust handlers themselves: those are proven
// by the Rust harness (cargo test in
// ~/workspace/sen-tauri-followups-harness-2026-11-19, 107 pass).
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import { ChatSession, type TranscriptEntry } from "../src/chat/chat.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import type { SenEvent } from "../src/ipc/session-events.ts";
import { fetchBriefSource } from "../src/ipc/brief-client.ts";
import { listSkills, matchSkill } from "../src/ipc/skills-client.ts";
import { delegateMcpTool } from "../src/ipc/mcp-delegate-client.ts";

const TEST_DIR = dirname(fileURLToPath(import.meta.url));

const UUID_SESSION = "11111111-2222-4333-8444-555555555555";

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

function resultEvent(text: string): SenEvent {
  return {
    type: "result",
    sessionId: UUID_SESSION,
    subtype: "success",
    isError: false,
    text,
    numTurns: 1,
    usage: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 0 },
    durationMs: 5,
    model: "test-model",
  };
}

function view(entries: TranscriptEntry[]) {
  return entries.map((entry) => ({
    role: entry.role,
    channel: entry.channel,
    text: entry.text,
  }));
}

function briefSourceData() {
  const now = Date.now();
  const iso = (ms: number) => new Date(ms).toISOString();
  return {
    emails: [
      {
        id: "m1",
        from: "sep@example.com",
        subject: "Vui lòng phê duyệt ngân sách",
        body: "Xem file đính kèm.",
        receivedAt: iso(now - 3600_000),
        dueAt: iso(now + 2 * 86400_000),
      },
    ],
    events: [
      {
        id: "e1",
        title: "Họp đội",
        start: iso(now + 3600_000),
        end: iso(now + 2 * 3600_000),
      },
      {
        id: "e2",
        title: "Họp khách",
        start: iso(now + 90 * 60_000),
        end: iso(now + 3 * 3600_000),
      },
    ],
  };
}

const MORNING_BRIEF_META = {
  name: "morning-brief",
  description:
    "Xây bản tin sáng từ email và lịch, rồi đề xuất quyết định chờ duyệt.",
  triggers: ["bản tin sáng", "morning brief", "hộp thư", "lịch hôm nay"],
  allowedTools: ["read_memory", "draft_email"],
  dir: "/skills/morning-brief",
  path: "/skills/morning-brief/SKILL.md",
};

function noOpMemory() {
  return {
    read: async () => ({ memory: [], user: [], soul: "", today: [] }),
    add: async (_c: unknown, _s: never, text: string) => ({
      scope: "memory" as const,
      index: 1,
      item: text,
    }),
    forgetByNumber: async () => ({ scope: "memory" as const, removed: "" }),
    forgetByText: async () => ({ scope: "memory" as const, removed: "" }),
    edit: async (_c: unknown, _s: never, i: number, text: string) => ({
      scope: "memory" as const,
      index: i,
      item: text,
    }),
    setSoul: async (_c: unknown, text: string) => ({ soul: text }),
    log: async () => undefined,
  };
}

afterEach(() => {
  setBridgeTransport(null);
});

// --- brief-client ---

test("followups: fetchBriefSource calls sen_brief_source and validates the data", async () => {
  const commands: string[] = [];
  setBridgeTransport(async (command, args) => {
    commands.push(command);
    assert.equal(
      (args as unknown as { contract: ActionContract }).contract.id,
      "c1",
    );
    return briefSourceData();
  });
  const data = await fetchBriefSource(contract());
  assert.deepEqual(commands, ["sen_brief_source"]);
  assert.equal(data.emails[0]?.subject, "Vui lòng phê duyệt ngân sách");
  assert.equal(data.events.length, 2);
});

test("followups: fetchBriefSource rejects malformed backend data, never a fake brief", async () => {
  setBridgeTransport(async () => ({
    emails: [{ id: "m1" }],
    events: [],
  }));
  await assert.rejects(fetchBriefSource(contract()), /malformed/);
  // A backend failure propagates untouched.
  setBridgeTransport(async () => {
    throw new Error(
      "BRIEF_SOURCE_NOT_CONFIGURED: set SEN_GMAIL_ACCESS_TOKEN",
    );
  });
  await assert.rejects(
    fetchBriefSource(contract()),
    /BRIEF_SOURCE_NOT_CONFIGURED/,
  );
});

// --- skills-client ---

test("followups: listSkills and matchSkill call the Rust skill commands", async () => {
  const commands: string[] = [];
  setBridgeTransport(async (command, args) => {
    commands.push(command);
    if (command === "sen_skills_list") {
      return { loaded: [MORNING_BRIEF_META], failed: [] };
    }
    if (command === "sen_skill_match") {
      const { query } = args as unknown as { query: string };
      return query.includes("bản tin sáng") ? MORNING_BRIEF_META : null;
    }
    throw new Error("unexpected command " + command);
  });
  const report = await listSkills(contract());
  assert.equal(report.loaded.length, 1);
  assert.equal(report.loaded[0]?.name, "morning-brief");
  // Metadata carries no body: the key is absent, not empty.
  assert.equal("body" in (report.loaded[0] ?? {}), false);
  assert.deepEqual(report.failed, []);
  assert.equal(
    (await matchSkill(contract(), "cho tôi bản tin sáng"))?.name,
    "morning-brief",
  );
  assert.equal(await matchSkill(contract(), "kể chuyện cười"), null);
  assert.deepEqual(commands, [
    "sen_skills_list",
    "sen_skill_match",
    "sen_skill_match",
  ]);
});

test("followups: skills-client rejects malformed metadata", async () => {
  setBridgeTransport(async () => ({
    loaded: [{ name: "x" }],
    failed: [],
  }));
  await assert.rejects(listSkills(contract()), /malformed/);
  setBridgeTransport(async () => ({ name: 1 }));
  await assert.rejects(matchSkill(contract(), "x"), /malformed/);
});

// --- mcp-delegate-client ---

test("followups: delegateMcpTool calls sen_mcp_delegate with the chat session id", async () => {
  const seen: unknown[] = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, args });
    return { text: "echo: hello", sessionId: "delegate-uuid", exitCode: 0 };
  });
  const result = await delegateMcpTool(contract(), {
    sessionId: UUID_SESSION,
    serverName: "calc",
    serverConfig: { command: "node", args: ["server.mjs"] },
    tool: "echo",
    arguments: { hello: "world" },
  });
  assert.equal(result.text, "echo: hello");
  assert.equal(result.sessionId, "delegate-uuid");
  assert.equal(result.exitCode, 0);
  const call = seen[0] as {
    command: string;
    args: Record<string, unknown>;
  };
  assert.equal(call.command, "sen_mcp_delegate");
  assert.equal(call.args.session_id, UUID_SESSION);
  const wireArgs = call.args.args as Record<string, unknown>;
  assert.equal(wireArgs.serverName, "calc");
  assert.equal(wireArgs.tool, "echo");
  assert.deepEqual(wireArgs.arguments, { hello: "world" });
});

test("followups: delegateMcpTool rejects malformed results and surfaces backend errors", async () => {
  setBridgeTransport(async () => ({ text: 1 }));
  await assert.rejects(
    delegateMcpTool(contract(), {
      sessionId: UUID_SESSION,
      serverName: "calc",
      serverConfig: { command: "node" },
      tool: "echo",
    }),
    /malformed/,
  );
  setBridgeTransport(async () => {
    throw new Error("CLI_DELEGATE_ERROR: tool exploded");
  });
  await assert.rejects(
    delegateMcpTool(contract(), {
      sessionId: UUID_SESSION,
      serverName: "calc",
      serverConfig: { command: "node" },
      tool: "echo",
    }),
    /CLI_DELEGATE_ERROR/,
  );
});

// --- ChatSession over the Rust commands ---

test("followups: /brief through the Rust source builds a real brief, no model turn", async () => {
  const commands: string[] = [];
  const briefHtml: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: noOpMemory(),
    briefSource: () => fetchBriefSource(contract()),
    onBrief: (html) => {
      briefHtml.push(html);
    },
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command, args) => {
    commands.push(command);
    if (command === "sen_brief_source") {
      return briefSourceData();
    }
    if (command === "sen_session_steer" || command === "sen_session_follow_up") {
      const { text } = args as unknown as { session_id: string; text: string };
      chat.handleEvent(resultEvent("reply to " + text));
      return {
        sessionId: UUID_SESSION,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    throw new Error("unexpected command " + command);
  });
  await chat.send("desktop", "/brief");
  assert.deepEqual(commands, ["sen_brief_source"]);
  const entries = view(chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.match(entries[0]?.text ?? "", /Họp đội/);
  assert.match(entries[0]?.text ?? "", /chồng lấn 30 phút/);
  assert.equal(briefHtml.length, 1);
});

test("followups: /brief without a token still refuses honestly (no fake brief)", async () => {
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: noOpMemory(),
    briefSource: () => fetchBriefSource(contract()),
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command) => {
    if (command === "sen_brief_source") {
      throw new Error(
        "BRIEF_SOURCE_NOT_CONFIGURED: set SEN_GMAIL_ACCESS_TOKEN",
      );
    }
    throw new Error("unexpected command " + command);
  });
  await chat.send("desktop", "/brief");
  const entries = view(chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.match(entries[0]?.text ?? "", /BRIEF_SOURCE_NOT_CONFIGURED/);
});

test("followups: a morning-brief message is routed by the Rust matcher, never reaches the model", async () => {
  const commands: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: noOpMemory(),
    briefSource: () => fetchBriefSource(contract()),
    skillMatcher: async (query) =>
      (await matchSkill(contract(), query)) ?? undefined,
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command, args) => {
    commands.push(command);
    if (command === "sen_skill_match") {
      const { query } = args as unknown as { query: string };
      return query.includes("bản tin sáng") ? MORNING_BRIEF_META : null;
    }
    if (command === "sen_brief_source") {
      return briefSourceData();
    }
    if (command === "sen_session_steer" || command === "sen_session_follow_up") {
      const { text } = args as unknown as { session_id: string; text: string };
      chat.handleEvent(resultEvent("reply to " + text));
      return {
        sessionId: UUID_SESSION,
        state: "Running",
        unknownRecords: 0,
        versionMismatches: 0,
      };
    }
    throw new Error("unexpected command " + command);
  });
  await chat.send("desktop", "cho tôi bản tin sáng hôm nay");
  assert.deepEqual(commands, ["sen_skill_match", "sen_brief_source"]);
  const entries = view(chat.getTranscript());
  assert.deepEqual(entries.map((e) => e.role), ["user", "system"]);
  assert.match(entries[1]?.text ?? "", /chồng lấn 30 phút/);
  // An unrelated message still runs a model turn (matcher returns null).
  await chat.send("desktop", "kể chuyện cười");
  assert.deepEqual(commands, [
    "sen_skill_match",
    "sen_brief_source",
    "sen_skill_match",
    "sen_session_steer",
  ]);
});

test("followups: /mcp through the Rust delegate lands the result in the transcript", async () => {
  const commands: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: noOpMemory(),
    mcpServers: { calc: { command: "node", args: ["server.mjs"] } },
    delegateToolCall: (input) =>
      delegateMcpTool(contract(), {
        sessionId: UUID_SESSION,
        serverName: input.serverName,
        serverConfig: input.serverConfig,
        tool: input.tool,
        arguments: input.arguments,
      }),
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command) => {
    commands.push(command);
    if (command === "sen_mcp_delegate") {
      return { text: "echo: hello", sessionId: "delegate-uuid", exitCode: 0 };
    }
    throw new Error("unexpected command " + command);
  });
  await chat.send("desktop", '/mcp calc echo {"hello": "world"}');
  assert.deepEqual(commands, ["sen_mcp_delegate"]);
  const entries = view(chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.role, "system");
  assert.match(entries[0]?.text ?? "", /echo: hello/);
});

// --- Webview import graph: new IPC clients in, node-only modules out ---

function staticImports(source: string): string[] {
  const stripped = source
    .replace(/import\s+type\b[^;]*;/g, "")
    .replace(/export\s+type\b[^;]*;/g, "");
  const found: string[] = [];
  const fromRe = /(?:import|export)[^'"]*?from\s*["']([^"']+)["']/g;
  const sideEffectRe = /import\s*["']([^"']+)["']/g;
  let m: RegExpExecArray | null;
  while ((m = fromRe.exec(stripped)) !== null) {
    found.push(m[1]);
  }
  while ((m = sideEffectRe.exec(stripped)) !== null) {
    found.push(m[1]);
  }
  return [...new Set(found)];
}

function walkGraph(entry: string): string[] {
  const reached: string[] = [];
  const queue: string[] = [entry];
  const seen = new Set<string>();
  while (queue.length > 0) {
    const file = queue.shift()!;
    if (seen.has(file)) {
      continue;
    }
    seen.add(file);
    reached.push(file);
    const source = readFileSync(file, "utf8");
    for (const specifier of staticImports(source)) {
      if (!specifier.startsWith(".")) {
        continue;
      }
      const base = resolve(dirname(file), specifier);
      for (const candidate of [base, base + ".ts", join(base, "index.ts")]) {
        if (existsSync(candidate) && candidate.endsWith(".ts")) {
          queue.push(candidate);
        }
      }
    }
  }
  return reached;
}

test("followups: webview graph reaches the new IPC clients, not the node-only modules", () => {
  const reached = walkGraph(join(TEST_DIR, "../src/app/main.ts"));
  for (const client of [
    "ipc/brief-client.ts",
    "ipc/skills-client.ts",
    "ipc/mcp-delegate-client.ts",
  ]) {
    assert.ok(
      reached.some((f) => f.endsWith(client)),
      `expected ${client} in the webview graph (value import)`,
    );
  }
  for (const banned of [
    "packages/core/src/mcp-client.ts",
    "packages/core/src/skill-registry.ts",
    "chat/skill-matcher.ts",
  ]) {
    assert.ok(
      !reached.some((f) => f.endsWith(banned)),
      `${banned} must stay out of the webview graph (node:fs / node:child_process breaks vite build)`,
    );
  }
});
