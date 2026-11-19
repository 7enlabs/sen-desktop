// Wiring tests (CI P2-9): P0-4/P0-5 joined to the real app run path.
// Run with: node --test apps/desktop/tests/wiring-p2-9.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
//
// What these tests prove, end to end under node:
// - /brief builds a REAL brief with buildMorningBrief (not a render of
//   a hand-made object) and shows it as text in the transcript and as
//   HTML through onBrief; without a brief source it says so honestly.
// - A plain "bản tin sáng" message is routed by the REAL SkillRegistry
//   (scanned from the repo skills/ dir) to the same brief flow, and
//   never reaches the model; an unrelated message still does.
// - /mcp runs a REAL delegateToolCallToCliCore turn (fake `zen` binary
//   from the core fixtures, real spawn + stream-json parse) and the
//   result lands in the transcript; without a delegate or a server
//   config it says so honestly instead of faking a result.
// - The webview import graph from main.ts reaches the pure-TS brief
//   module and reaches NEITHER node-only module (mcp-client,
//   skill-registry): importing those into the webview entry breaks
//   vite build (probe 19/11: exit 1, node:fs/node:child_process
//   externalized for browser compatibility).
// They do NOT prove a real webview, a real `zen` binary, or a real
// Gmail/calendar fetch: the brief source and the delegate are injected
// here, and at boot main.ts passes neither (see main.ts comment).
import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import { ChatSession, type TranscriptEntry } from "../src/chat/chat.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import type { SenEvent } from "../src/ipc/session-events.ts";
import { parseChatCommand } from "../src/chat/commands.ts";
import { buildAndRenderMorningBrief } from "../src/app/morning-brief-ui.ts";
import { createSkillMatcher } from "../src/chat/skill-matcher.ts";
import { delegateToolCallToCliCore } from "../../../packages/core/src/mcp-client.ts";
import type {
  CalendarEvent,
  EmailItem,
} from "../../../packages/core/src/morning-brief.ts";
import type { Skill } from "../../../packages/core/src/skill-registry.ts";
import { startChatUI } from "../src/app/chat-ui.ts";

const TEST_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(TEST_DIR, "../../..");
const FAKE_ZEN = join(
  TEST_DIR,
  "../../../packages/core/tests/fixtures/mcp-fake-zen.mjs",
);
chmodSync(FAKE_ZEN, 0o755);

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

// Real brief data, built around the real current time so the window
// math in buildMorningBrief always sees the events as today + soon.
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

interface WiringHarness {
  chat: ChatSession;
  commands: string[];
  briefHtml: string[];
}

function makeChat(options: {
  briefSource?: () => Promise<{
    emails: EmailItem[];
    events: CalendarEvent[];
  }>;
  skillMatcher?: (query: string) => Promise<Skill | undefined>;
} = {}): WiringHarness {
  const commands: string[] = [];
  const briefHtml: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    // The memory client is a no-op fake: the brief and MCP paths
    // never touch memory, and the model-turn test below only needs
    // the log call not to throw.
    memory: {
      read: async () => ({ memory: [], user: [], soul: "", today: [] }),
      add: async (_c, _s, text) => ({ scope: "memory", index: 1, item: text }),
      forgetByNumber: async () => ({ scope: "memory", removed: "" }),
      forgetByText: async () => ({ scope: "memory", removed: "" }),
      edit: async (_c, _s, i, text) => ({ scope: "memory", index: i, item: text }),
      setSoul: async (_c, text) => ({ soul: text }),
      log: async () => undefined,
    },
    briefSource: options.briefSource,
    skillMatcher: options.skillMatcher,
    onBrief: (html) => {
      briefHtml.push(html);
    },
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command, args) => {
    commands.push(command);
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
  return { chat, commands, briefHtml };
}

afterEach(() => {
  setBridgeTransport(null);
  delete process.env.FAKE_ZEN_RESULT;
  delete process.env.FAKE_ZEN_MODE;
});

// --- Command parsing ---

test("wiring: /brief parses to morning-brief, /mcp parses server/tool/args", () => {
  assert.deepEqual(parseChatCommand("/brief"), { kind: "morning-brief" });
  assert.deepEqual(parseChatCommand("/mcp calc echo {\"a\": 1}"), {
    kind: "mcp-call",
    server: "calc",
    tool: "echo",
    args: { a: 1 },
  });
  assert.deepEqual(parseChatCommand("/mcp calc echo"), {
    kind: "mcp-call",
    server: "calc",
    tool: "echo",
    args: {},
  });
  // Malformed args JSON is an unknown command (usage reply), never a
  // half-parsed call.
  assert.deepEqual(parseChatCommand("/mcp calc echo {bad"), {
    kind: "unknown",
    text: "/mcp calc echo {bad",
  });
  assert.deepEqual(parseChatCommand("/mcp calc"), {
    kind: "unknown",
    text: "/mcp calc",
  });
});

// --- Morning brief: build + render in one call ---

test("wiring: buildAndRenderMorningBrief builds a real brief and escapes hostile text", () => {
  const hostile = `<script>alert("x")</script>`;
  const html = buildAndRenderMorningBrief({
    emails: [
      {
        id: "m1",
        from: "evil@example.com",
        subject: hostile,
        body: hostile,
        receivedAt: "2026-11-16T06:00:00.000Z",
        needsDecision: true,
      },
    ],
    events: [
      {
        id: "e1",
        title: "Họp đội",
        start: "2026-11-16T09:00:00.000Z",
        end: "2026-11-16T10:00:00.000Z",
      },
    ],
    now: "2026-11-16T07:00:00.000Z",
  });
  assert.match(html, /Họp đội/);
  assert.match(html, /Vui lòng|evil@example\.com/);
  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;script&gt;/);
});

// --- /brief through the chat run path ---

test("wiring: /brief builds a real brief from the source, text + HTML, no model turn", async () => {
  const h = makeChat({ briefSource: async () => briefSourceData() });
  await h.chat.send("desktop", "/brief");
  assert.deepEqual(h.commands, []);
  const entries = view(h.chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.role, "system");
  // Real buildMorningBrief output: the overlapping pair is a
  // 30-minute conflict, the decision email is listed, today has
  // both events.
  assert.match(entries[0]?.text ?? "", /Họp đội/);
  assert.match(entries[0]?.text ?? "", /Vui lòng phê duyệt ngân sách/);
  assert.match(entries[0]?.text ?? "", /chồng lấn 30 phút/);
  assert.equal(h.briefHtml.length, 1);
  assert.match(h.briefHtml[0] ?? "", /Họp đội/);
  assert.match(h.briefHtml[0] ?? "", /Vui lòng phê duyệt ngân sách/);
});

test("wiring: /brief without a source says so honestly, no brief shown", async () => {
  const h = makeChat();
  await h.chat.send("desktop", "/brief");
  assert.deepEqual(h.commands, []);
  const entries = view(h.chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.match(entries[0]?.text ?? "", /Chưa nối nguồn email\/lịch/);
  assert.deepEqual(h.briefHtml, []);
});

// --- Skill routing: the real registry on the real skills/ dir ---

test("wiring: a morning-brief request is routed by the real SkillRegistry, never reaches the model", async () => {
  const matcher = createSkillMatcher(join(REPO_ROOT, "skills"));
  // The shipped skill really matches the Vietnamese request.
  assert.equal((await matcher("cho tôi bản tin sáng hôm nay"))?.name, "morning-brief");
  assert.equal((await matcher("kể chuyện cười"))?.name, undefined);
  const h = makeChat({
    briefSource: async () => briefSourceData(),
    skillMatcher: async (query) => matcher(query),
  });
  await h.chat.send("desktop", "cho tôi bản tin sáng hôm nay");
  assert.deepEqual(h.commands, []);
  const entries = view(h.chat.getTranscript());
  assert.deepEqual(entries.map((e) => e.role), ["user", "system"]);
  assert.match(entries[1]?.text ?? "", /Họp đội/);
  assert.match(entries[1]?.text ?? "", /chồng lấn 30 phút/);
  assert.equal(h.briefHtml.length, 1);
});

test("wiring: an unrelated message still runs a model turn with the matcher injected", async () => {
  const matcher = createSkillMatcher(join(REPO_ROOT, "skills"));
  const h = makeChat({
    briefSource: async () => briefSourceData(),
    skillMatcher: async (query) => matcher(query),
  });
  await h.chat.send("desktop", "kể chuyện cười");
  assert.deepEqual(h.commands, ["sen_session_steer"]);
  const entries = view(h.chat.getTranscript());
  assert.deepEqual(entries, [
    { role: "user", channel: "desktop", text: "kể chuyện cười" },
    { role: "assistant", channel: "desktop", text: "reply to kể chuyện cười" },
  ]);
  assert.deepEqual(h.briefHtml, []);
});

// --- /mcp through the chat run path, real delegate + fake zen ---

test("wiring: /mcp runs the real delegate and the result lands in the transcript", async () => {
  process.env.FAKE_ZEN_RESULT = "echo: hello";
  const commands: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: {
      read: async () => ({ memory: [], user: [], soul: "", today: [] }),
      add: async (_c, _s, text) => ({ scope: "memory", index: 1, item: text }),
      forgetByNumber: async () => ({ scope: "memory", removed: "" }),
      forgetByText: async () => ({ scope: "memory", removed: "" }),
      edit: async (_c, _s, i, text) => ({ scope: "memory", index: i, item: text }),
      setSoul: async (_c, text) => ({ soul: text }),
      log: async () => undefined,
    },
    mcpServers: { calc: { command: "node", args: ["server.mjs"] } },
    delegateToolCall: (input) =>
      delegateToolCallToCliCore({ ...input, zenBin: FAKE_ZEN }),
  });
  chat.bind(UUID_SESSION);
  setBridgeTransport(async (command) => {
    commands.push(command);
    throw new Error("unexpected command " + command);
  });
  await chat.send("desktop", "/mcp calc echo {\"hello\": \"world\"}");
  // No IPC at all: the delegate spawns the CLI directly.
  assert.deepEqual(commands, []);
  const entries = view(chat.getTranscript());
  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.role, "system");
  assert.match(entries[0]?.text ?? "", /echo: hello/);
});

test("wiring: /mcp without a delegate or a server config says so honestly, spawns nothing", async () => {
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: {
      read: async () => ({ memory: [], user: [], soul: "", today: [] }),
      add: async (_c, _s, text) => ({ scope: "memory", index: 1, item: text }),
      forgetByNumber: async () => ({ scope: "memory", removed: "" }),
      forgetByText: async () => ({ scope: "memory", removed: "" }),
      edit: async (_c, _s, i, text) => ({ scope: "memory", index: i, item: text }),
      setSoul: async (_c, text) => ({ soul: text }),
      log: async () => undefined,
    },
  });
  chat.bind(UUID_SESSION);
  await chat.send("desktop", "/mcp calc echo");
  let entries = view(chat.getTranscript());
  assert.match(entries[0]?.text ?? "", /chưa được nối|chưa nối/);
  // A delegate with no server config for the named server: same
  // honest refusal, and the delegate is never called.
  let delegateCalls = 0;
  const chat2 = new ChatSession({
    makeContract: () => contract(),
    memory: {
      read: async () => ({ memory: [], user: [], soul: "", today: [] }),
      add: async (_c, _s, text) => ({ scope: "memory", index: 1, item: text }),
      forgetByNumber: async () => ({ scope: "memory", removed: "" }),
      forgetByText: async () => ({ scope: "memory", removed: "" }),
      edit: async (_c, _s, i, text) => ({ scope: "memory", index: i, item: text }),
      setSoul: async (_c, text) => ({ soul: text }),
      log: async () => undefined,
    },
    mcpServers: {},
    delegateToolCall: async () => {
      delegateCalls += 1;
      return { text: "never", sessionId: UUID_SESSION, exitCode: 0 };
    },
  });
  chat2.bind(UUID_SESSION);
  await chat2.send("desktop", "/mcp calc echo");
  entries = view(chat2.getTranscript());
  assert.match(entries[0]?.text ?? "", /Chưa cấu hình MCP server/);
  assert.equal(delegateCalls, 0);
});

// --- Brief panel in the chat UI (fake DOM: node has none) ---

interface FakeEl {
  tag: string;
  children: FakeEl[];
  className: string;
  textContent: string;
  innerHTML: string;
  hidden: boolean;
  value: string;
  scrollTop: number;
  scrollHeight: number;
  append(...nodes: FakeEl[]): void;
  removeChild(node: FakeEl): void;
  addEventListener(): void;
  setAttribute(): void;
  readonly firstChild: FakeEl | null;
}

function fakeEl(tag: string): FakeEl {
  const el: FakeEl = {
    tag,
    children: [],
    className: "",
    textContent: "",
    innerHTML: "",
    hidden: false,
    value: "",
    scrollTop: 0,
    scrollHeight: 0,
    append(...nodes: FakeEl[]) {
      el.children.push(...nodes);
    },
    removeChild(node: FakeEl) {
      el.children = el.children.filter((c) => c !== node);
    },
    addEventListener() {},
    setAttribute() {},
    get firstChild() {
      return el.children[0] ?? null;
    },
  };
  return el;
}

test("wiring: chat UI showBrief fills the brief panel with escaped HTML", () => {
  const savedDocument = (globalThis as { document?: unknown }).document;
  (globalThis as { document?: unknown }).document = {
    createElement: (tag: string) => fakeEl(tag),
  };
  try {
    const root = fakeEl("div");
    const chat = new ChatSession({
      makeContract: () => contract(),
      memory: {
        read: async () => ({ memory: [], user: [], soul: "", today: [] }),
        add: async (_c, _s, text) => ({ scope: "memory", index: 1, item: text }),
        forgetByNumber: async () => ({ scope: "memory", removed: "" }),
        forgetByText: async () => ({ scope: "memory", removed: "" }),
        edit: async (_c, _s, i, text) => ({ scope: "memory", index: i, item: text }),
        setSoul: async (_c, text) => ({ soul: text }),
        log: async () => undefined,
      },
    });
    const ui = startChatUI(root as never, chat);
    const panel = root.children.find((c) => c.className === "morning-brief");
    assert.ok(panel, "expected a morning-brief panel in the UI");
    assert.equal(panel.hidden, true);
    const html = buildAndRenderMorningBrief({
      emails: [],
      events: [
        {
          id: "e1",
          title: "Họp đội",
          start: "2026-11-16T09:00:00.000Z",
          end: "2026-11-16T10:00:00.000Z",
        },
      ],
      now: "2026-11-16T07:00:00.000Z",
    });
    ui.showBrief(html);
    assert.equal(panel.hidden, false);
    assert.match(panel.innerHTML, /Họp đội/);
  } finally {
    if (savedDocument === undefined) {
      delete (globalThis as { document?: unknown }).document;
    } else {
      (globalThis as { document?: unknown }).document = savedDocument;
    }
  }
});

// --- Webview import graph: brief in, node-only modules out ---

// Only VALUE imports count here: `import type` is erased before the
// bundle runs, so a type-only import of a node-only module cannot
// break vite build — but a value import of one does (see the probe
// in the header comment). Strip type-only import/export statements
// first; they may span lines, so match to the closing `;`.
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

test("wiring: webview graph reaches the pure-TS brief module, not the node-only MCP/skill modules", () => {
  const reached = walkGraph(join(TEST_DIR, "../src/app/main.ts"));
  assert.ok(
    reached.some((f) => f.endsWith("packages/core/src/morning-brief.ts")),
    "expected morning-brief.ts in the webview graph (value import)",
  );
  assert.ok(
    !reached.some((f) => f.endsWith("packages/core/src/mcp-client.ts")),
    "mcp-client.ts must stay out of the webview graph (node:child_process breaks vite build)",
  );
  assert.ok(
    !reached.some((f) => f.endsWith("packages/core/src/skill-registry.ts")),
    "skill-registry.ts must stay out of the webview graph (node:fs breaks vite build)",
  );
});
