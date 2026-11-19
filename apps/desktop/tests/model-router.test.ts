// Model router tests (Sen P0-3).
// Run with: node --test apps/desktop/tests/model-router.test.ts
// Three layers, all runnable under node (no Rust toolchain needed):
// 1. The pure TS router (packages/providers/src/router.ts): default
//    chain, preference, BYOK gating, paid rejection, retryable codes.
// 2. The settings client router calls: get/set invoke the right
//    commands with the router args, and no key field ever travels.
// 3. The chat /model commands: /model lists the chain, /model <id>
//    switches the preferred route, an unknown id lists the valid ids,
//    and a model command never reaches the model (no steer call).
// These tests do NOT prove the Rust side: the Rust mirror suite is
// model_router.rs + registry.rs + cli_settings.rs (cargo test, run
// on the Leader VM — no Rust toolchain on this box), and a live model
// turn on a real provider is NOT RUN here.
import test from "node:test";
import assert from "node:assert/strict";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import {
  defaultRouterConfig,
  describeRoute,
  errorCodeOf,
  isRetryableErrorCode,
  nextRoute,
  resolveAttemptOrder,
  routeCostLabel,
  selectRoute,
  validateRouterConfig,
  type Route,
  type RouterConfig,
} from "../../../packages/providers/src/router.ts";
import {
  getRouterSetting,
  setRouterSetting,
  type RouterSettingResult,
} from "../src/ipc/settings-client.ts";
import { ChatSession } from "../src/chat/chat.ts";
import type { MemoryClient, RouterClient } from "../src/chat/chat.ts";
import type { SenEvent } from "../src/ipc/session-events.ts";
import { parseChatCommand } from "../src/chat/commands.ts";

// No-op memory for the /model tests: model commands never touch
// memory, but executeCommand still writes a memory-log line, so a
// fake keeps that write off the real bridge (no "window is not
// defined" noise under node).
const fakeMemory: MemoryClient = {
  read: async () => ({ memory: [], user: [], soul: "", today: [] }),
  add: async () => ({ scope: "memory", index: 1, item: "" }),
  forgetByNumber: async () => ({ scope: "memory", removed: "" }),
  forgetByText: async () => ({ scope: "memory", removed: "" }),
  edit: async () => ({ scope: "memory", index: 1, item: "" }),
  setSoul: async () => ({ soul: "" }),
  log: async () => {},
};

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

// --- 1. Pure router ---

test("router: default chain is spark, openrouter-free, ollama, all 0 dong", () => {
  const config = defaultRouterConfig();
  assert.deepEqual(
    config.routes.map((r) => r.id),
    ["spark", "openrouter-free", "ollama"],
  );
  for (const route of config.routes) {
    assert.equal(routeCostLabel(route), "0 đồng");
    assert.equal(route.paid ?? false, false);
  }
  assert.equal(config.routes[0].baseUrl, undefined);
  assert.equal(
    config.routes[1].baseUrl,
    "https://openrouter.ai/api/v1",
  );
  assert.equal(config.routes[2].baseUrl, "http://127.0.0.1:11434/v1");
  assert.deepEqual(validateRouterConfig(config), []);
  assert.equal(selectRoute(config)?.id, "spark");
  assert.equal(
    describeRoute(config.routes[0]),
    "spark · google/gemini-4-argon:free · 0 đồng",
  );
});

test("router: preferred route goes first, then the chain order", () => {
  const config = defaultRouterConfig();
  config.preferredRoute = "ollama";
  assert.deepEqual(
    resolveAttemptOrder(config).map((r) => r.id),
    ["ollama", "spark", "openrouter-free"],
  );
  config.preferredRoute = "no-such-route";
  assert.deepEqual(
    resolveAttemptOrder(config).map((r) => r.id),
    ["spark", "openrouter-free", "ollama"],
  );
});

test("router: fallback disabled means one attempt", () => {
  const config = defaultRouterConfig();
  config.fallbackEnabled = false;
  assert.equal(resolveAttemptOrder(config).length, 1);
});

test("router: nextRoute walks the chain and never restarts at the first route (P2-2)", () => {
  // Mirror of next_route_never_restarts_at_the_first_route in
  // model_router.rs (PR #95): an unknown failed id used to return
  // the first route, which may be the route that just failed — the
  // fallback loop then retried it forever when the config changed
  // mid-turn. The caller snapshots the order once per turn with
  // resolveAttemptOrder and walks the snapshot (L2 follow-up to
  // PR #96); nextRoute itself takes no config.
  const order = resolveAttemptOrder(defaultRouterConfig());
  assert.equal(nextRoute(order, "spark")?.id, "openrouter-free");
  assert.equal(nextRoute(order, "openrouter-free")?.id, "ollama");
  assert.equal(nextRoute(order, "ollama"), undefined);
  assert.equal(nextRoute(order, "no-such-route"), undefined);
  assert.equal(nextRoute(order, ""), undefined);
});

test("router: nextRoute walks a snapshot order fixed at turn start (L2 follow-up to PR #96)", () => {
  // Mirror of next_route_in_walks_a_snapshot_order in
  // model_router.rs: the caller snapshots once with
  // resolveAttemptOrder, and a config change after the snapshot
  // (preferred route moved, a route dropped) cannot move routes
  // under the walk.
  const config = defaultRouterConfig();
  const order = resolveAttemptOrder(config);
  assert.equal(order.length, 3);
  assert.equal(nextRoute(order, "spark")?.id, "openrouter-free");
  assert.equal(nextRoute(order, "openrouter-free")?.id, "ollama");
  assert.equal(nextRoute(order, "ollama"), undefined);
  assert.equal(nextRoute(order, "no-such-route"), undefined);
  // Mutating the config after the snapshot leaves the walk alone.
  config.preferredRoute = "ollama";
  config.routes = config.routes.filter((r) => r.id !== "ollama");
  assert.equal(nextRoute(order, "spark")?.id, "openrouter-free");
  assert.equal(nextRoute(order, "openrouter-free")?.id, "ollama");
  // A reordered snapshot walks the reordered order: the caller,
  // not nextRoute, owns the snapshot.
  const reordered = [...order].reverse();
  assert.equal(nextRoute(reordered, "ollama")?.id, "openrouter-free");
});

test("router: BYOK is skipped without approval or without a key", () => {
  const byok: Route = {
    id: "byok-openrouter",
    provider: "byok",
    model: "anthropic/claude-sonnet-4",
    baseUrl: "https://openrouter.ai/api/v1",
    byokApproved: true,
    keyEnv: "OPENROUTER_API_KEY",
  };
  const config: RouterConfig = {
    routes: [...defaultRouterConfig().routes, byok],
    fallbackEnabled: true,
  };
  // Key absent: BYOK never attempted, even approved.
  assert.deepEqual(
    resolveAttemptOrder(config, false).map((r) => r.id),
    ["spark", "openrouter-free", "ollama"],
  );
  // Key present: BYOK joins the order after the free routes (cap 3
  // keeps the first three, so it only runs when preferred or when a
  // free route is missing — the point is it is never first by
  // accident).
  const preferred: RouterConfig = {
    ...config,
    preferredRoute: "byok-openrouter",
  };
  assert.equal(resolveAttemptOrder(preferred, true)[0].id, "byok-openrouter");
  assert.equal(
    routeCostLabel(byok),
    "key của bạn (BYOK)",
  );
});

test("router: validation rejects paid routes, unapproved BYOK, bad ids", () => {
  const config = defaultRouterConfig();
  const paid: Route = { ...config.routes[0], id: "paid-one", paid: true };
  assert.ok(
    validateRouterConfig({ ...config, routes: [...config.routes, paid] }).some(
      (e) => e.includes("paid routes are rejected"),
    ),
  );
  const byok: Route = {
    id: "byok-x",
    provider: "byok",
    model: "m",
    keyEnv: "SOME_KEY_ENV",
  };
  assert.ok(
    validateRouterConfig({ ...config, routes: [...config.routes, byok] }).some(
      (e) => e.includes("BYOK needs explicit user approval"),
    ),
  );
  const byokNoEnv: Route = { ...byok, byokApproved: true, keyEnv: undefined };
  assert.ok(
    validateRouterConfig({
      ...config,
      routes: [...config.routes, byokNoEnv],
    }).some((e) => e.includes("BYOK needs keyEnv")),
  );
  assert.ok(
    validateRouterConfig({ ...config, routes: [] }).some((e) =>
      e.includes("non-empty array"),
    ),
  );
  assert.ok(
    validateRouterConfig({
      ...config,
      routes: [...config.routes, config.routes[0]],
    }).some((e) => e.includes("duplicate route id")),
  );
  assert.ok(
    validateRouterConfig({ ...config, preferredRoute: "nope" }).some((e) =>
      e.includes("not a route id"),
    ),
  );
  const badUrl: Route = {
    ...config.routes[1],
    baseUrl: "not-a-url",
  };
  assert.ok(
    validateRouterConfig({
      ...config,
      routes: [config.routes[0], badUrl, config.routes[2]],
    }).some((e) => e.includes("http(s) URL")),
  );
});

test("router: retryable codes are provider problems only", () => {
  for (const code of [
    "CLI_AUTH",
    "CLI_RATE_LIMIT",
    "CLI_NO_RESULT",
    "CLI_TURN_TIMEOUT",
    "CLI_SPAWN_FAILED",
    "CLI_NOT_FOUND",
    "CLI_CONFLICT",
  ]) {
    assert.equal(isRetryableErrorCode(code), true, code);
  }
  for (const code of [
    "CLI_VALIDATION",
    "CLI_FORBIDDEN",
    "CLI_TURN_ABORTED",
    "CLI_EMPTY_TEXT",
    "CLI_INVALID_RECORD",
    "CLI_TRUNCATED",
    "DUPLICATE_STEER",
    "",
  ]) {
    assert.equal(isRetryableErrorCode(code), false, code);
  }
  assert.equal(errorCodeOf("CLI_AUTH: bad key"), "CLI_AUTH");
  assert.equal(errorCodeOf("no colon here"), "");
});

// --- 2. Settings client router calls ---

function routerResult(): RouterSettingResult {
  return {
    routes: defaultRouterConfig().routes.map((r) => ({
      id: r.id,
      provider: r.provider,
      model: r.model,
      baseUrl: r.baseUrl ?? null,
    })),
    preferredRoute: null,
    fallbackEnabled: true,
    byokApproved: false,
    byokKeyPresent: false,
    activeRouteId: "spark",
  };
}

test("settings-client router get: invokes sen_settings_get_router, no key", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command, args) => {
    calls.push(command);
    assert.ok(!("key" in args) && !("apiKey" in args));
    return routerResult();
  });
  try {
    const res = await getRouterSetting(contract());
    assert.equal(res.activeRouteId, "spark");
    assert.deepEqual(calls, ["sen_settings_get_router"]);
  } finally {
    setBridgeTransport(null);
  }
});

test("settings-client router set: preferred route travels under args, no key", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, ...args });
    return { ...routerResult(), preferredRoute: "ollama", activeRouteId: "ollama" };
  });
  try {
    const res = await setRouterSetting(contract(), { preferredRoute: "ollama" });
    assert.equal(res.activeRouteId, "ollama");
    assert.deepEqual(seen[0]["args"], { preferredRoute: "ollama" });
    assert.ok(!("key" in seen[0]) && !("apiKey" in seen[0]));
  } finally {
    setBridgeTransport(null);
  }
});

test("settings-client router get: malformed result fails loudly", async () => {
  setBridgeTransport(async () => ({ routes: [] }));
  try {
    await assert.rejects(
      () => getRouterSetting(contract()),
      /malformed/,
    );
  } finally {
    setBridgeTransport(null);
  }
});

// --- 3. Chat /model commands ---

test("commands: /model parses to show, /model <id> parses to set", () => {
  assert.deepEqual(parseChatCommand("/model"), { kind: "model-show" });
  assert.deepEqual(parseChatCommand("/model ollama"), {
    kind: "model-set",
    routeId: "ollama",
  });
  assert.deepEqual(parseChatCommand("/model  spark  "), {
    kind: "model-set",
    routeId: "spark",
  });
});

class FakeRouter implements RouterClient {
  public sets: string[] = [];
  private status: RouterSettingResult;
  constructor(status: RouterSettingResult) {
    this.status = status;
  }
  async getRouter(): Promise<RouterSettingResult> {
    return this.status;
  }
  async setPreferredRoute(
    _contract: ActionContract,
    routeId: string,
  ): Promise<RouterSettingResult> {
    this.sets.push(routeId);
    this.status = {
      ...this.status,
      preferredRoute: routeId,
      activeRouteId: routeId,
    };
    return this.status;
  }
}

test("chat /model: lists the chain with costs, never steers", async () => {
  const router = new FakeRouter(routerResult());
  const statuses: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: fakeMemory,
    router,
    onRouterStatus: (text) => statuses.push(text),
  });
  chat.bind("s1");
  await chat.send("desktop", "/model");
  const entries = chat.getTranscript();
  assert.equal(entries.length, 1);
  assert.equal(entries[0].role, "system");
  assert.match(entries[0].text, /● spark · google\/gemini-4-argon:free · 0 đồng/);
  assert.match(entries[0].text, /○ ollama · llama3\.1 · 0 đồng/);
  assert.match(entries[0].text, /Fallback|thử đường tiếp theo/);
  assert.deepEqual(router.sets, []);
});

test("chat /model <id>: switches the preferred route and updates the status line", async () => {
  const router = new FakeRouter(routerResult());
  const statuses: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: fakeMemory,
    router,
    onRouterStatus: (text) => statuses.push(text),
  });
  chat.bind("s1");
  await chat.send("desktop", "/model ollama");
  assert.deepEqual(router.sets, ["ollama"]);
  assert.deepEqual(statuses, [
    "Model: ollama · llama3.1 · 0 đồng",
  ]);
  const entries = chat.getTranscript();
  assert.match(entries[0].text, /Đã chọn đường ollama/);
  assert.match(entries[0].text, /Phiên đang chạy giữ đường cũ/);
});

// P0-3 rev2 (F1): after a fallback the status line must show the
// route that actually ran, not the preferred route that failed. The
// fake backend steers on spark's preference but acks with
// routeId openrouter-free (the fallback route); the line must follow
// the ack, enriched with that route's provider/model from the chain.
test("chat turn: status line follows the route that actually ran after fallback", async () => {
  const router = new FakeRouter(routerResult());
  const statuses: string[] = [];
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: fakeMemory,
    router,
    onRouterStatus: (text) => statuses.push(text),
  });
  chat.bind("s1");
  setBridgeTransport(async (command, args) => {
    assert.equal(command, "sen_session_steer");
    const { text } = args as unknown as { session_id: string; text: string };
    // Deliver the Result event synchronously, mid-turn, like the
    // backend streaming a result record before the ack.
    const event: SenEvent = {
      type: "result",
      sessionId: "s1",
      subtype: "success",
      isError: false,
      text: "reply to " + text,
      numTurns: 1,
      usage: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 0 },
      durationMs: 5,
      model: "google/gemini-4-argon:free",
    };
    chat.handleEvent(event);
    return {
      sessionId: "s1",
      state: "Running",
      unknownRecords: 0,
      versionMismatches: 0,
      routeId: "openrouter-free",
      cost: "0 đồng",
    };
  });
  try {
    await chat.send("desktop", "hi");
  } finally {
    setBridgeTransport(null);
  }
  // The preferred/active route is still spark, but the turn ran on
  // openrouter-free: the line must name the route that ran.
  assert.deepEqual(statuses, [
    "Model: openrouter-free · google/gemini-4-argon:free · 0 đồng",
  ]);
  const entries = chat.getTranscript();
  assert.equal(entries[1].role, "assistant");
  assert.equal(entries[1].text, "reply to hi");
});

test("chat /model <unknown>: lists valid ids and sets nothing", async () => {
  const router = new FakeRouter(routerResult());
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: fakeMemory,
    router,
  });
  chat.bind("s1");
  await chat.send("desktop", "/model nope");
  assert.deepEqual(router.sets, []);
  const entries = chat.getTranscript();
  assert.match(entries[0].text, /Không có đường "nope"/);
  assert.match(entries[0].text, /spark, openrouter-free, ollama/);
});

test("chat /model without a router: reports the CLI free default, never fakes a list", async () => {
  const chat = new ChatSession({
    makeContract: () => contract(),
    memory: fakeMemory,
  });
  chat.bind("s1");
  await chat.send("desktop", "/model");
  const entries = chat.getTranscript();
  assert.match(entries[0].text, /chưa được nối/);
  assert.match(entries[0].text, /đường miễn phí của CLI/);
  await chat.refreshRouterStatus();
});
