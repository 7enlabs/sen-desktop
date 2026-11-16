// MCP client tests (Sen P0-5).
// Run with: node --test packages/core/tests/mcp-client.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The client code itself is never mocked: the stdio tests spawn the
// real fake MCP server (tests/fixtures/mcp-fake-server.mjs) through
// the real StdioMcpTransport, and the HTTP tests talk to a real
// local HTTP server through the real HttpMcpTransport. Only the
// server boundary is faked.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import {
  ActivityLog,
  ApprovalStore,
} from "../src/approval-gate.ts";
import {
  McpClient,
  StdioMcpTransport,
  connectHttpMcp,
  connectStdioMcp,
  executeMcpTool,
  isHttpServerConfig,
  mcpConfigFileJson,
  validateMcpServerConfig,
  type McpServerConfig,
} from "../src/mcp-client.ts";

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "fixtures");
const FAKE_SERVER = join(FIXTURES, "mcp-fake-server.mjs");

function stdioConfig(): McpServerConfig {
  return { command: process.execPath, args: [FAKE_SERVER] };
}

async function connectedClient(timeoutMs = 5000) {
  return connectStdioMcp(stdioConfig(), { timeoutMs });
}

// --- Config validation ---

test("config: a stdio config and an HTTP config validate clean", () => {
  assert.deepEqual(validateMcpServerConfig(stdioConfig()), []);
  assert.deepEqual(
    validateMcpServerConfig({ url: "http://127.0.0.1:9/mcp" }),
    [],
  );
  assert.equal(isHttpServerConfig({ url: "http://127.0.0.1:9/mcp" }), true);
  assert.equal(isHttpServerConfig(stdioConfig()), false);
  assert.equal(
    isHttpServerConfig({ type: "http", url: "https://example.com/mcp" }),
    true,
  );
});

test("config: missing command and url, bad type, bad url are rejected", () => {
  assert.ok(validateMcpServerConfig({}).length > 0);
  assert.ok(validateMcpServerConfig({ type: "tcp" }).length > 0);
  assert.ok(
    validateMcpServerConfig({ type: "stdio" }).some((e) =>
      e.includes("command"),
    ),
  );
  assert.ok(
    validateMcpServerConfig({ type: "http" }).some((e) => e.includes("url")),
  );
  assert.ok(
    validateMcpServerConfig({ url: "ftp://example.com/x" }).some((e) =>
      e.includes("http(s)"),
    ),
  );
  assert.ok(validateMcpServerConfig("nope").length > 0);
});

test("config: the CLI config file shape matches zen-mcp", () => {
  const parsed = JSON.parse(
    mcpConfigFileJson("fake", { command: "x", args: ["--a"] }),
  ) as { mcpServers: Record<string, unknown> };
  assert.deepEqual(parsed, {
    mcpServers: { fake: { command: "x", args: ["--a"] } },
  });
});

// --- Protocol over stdio (real fake server process) ---

test("client: initialize + listTools returns the fake server tools", async () => {
  const client = await connectedClient();
  try {
    assert.equal(client.isInitialized, true);
    const tools = await client.listTools();
    assert.deepEqual(
      tools.map((t) => t.name),
      ["echo", "add", "fail", "slow", "read_file"],
    );
    assert.equal(tools[0]?.description, "Echo the arguments back as text");
  } finally {
    await client.close();
  }
});

test("client: callTool echo and add return real results", async () => {
  const client = await connectedClient();
  try {
    const echoed = await client.callTool("echo", { hello: "world" });
    assert.equal(echoed.isError, false);
    assert.equal(
      (echoed.content[0] as { text: string }).text,
      JSON.stringify({ hello: "world" }),
    );
    const sum = await client.callTool("add", { a: 19, b: 23 });
    assert.equal((sum.content[0] as { text: string }).text, "42");
  } finally {
    await client.close();
  }
});

test("client: a tool that reports isError returns the result, not a throw", async () => {
  const client = await connectedClient();
  try {
    const result = await client.callTool("fail", {});
    assert.equal(result.isError, true);
    assert.equal((result.content[0] as { text: string }).text, "boom");
  } finally {
    await client.close();
  }
});

test("client: an unknown tool fails as MCP_TOOL_ERROR", async () => {
  const client = await connectedClient();
  try {
    await assert.rejects(
      () => client.callTool("no-such-tool", {}),
      /MCP_TOOL_ERROR: server error -32602: Unknown tool/,
    );
  } finally {
    await client.close();
  }
});

test("client: ping works, and tools before initialize throw", async () => {
  const raw = new McpClient(new StdioMcpTransport(stdioConfig()));
  try {
    await assert.rejects(() => raw.listTools(), /MCP_NOT_INITIALIZED/);
    await raw.initialize();
    await raw.ping();
  } finally {
    await raw.close();
  }
});

test("client: a tool that never replies fails as MCP_TIMEOUT", async () => {
  const client = await connectStdioMcp(stdioConfig(), { timeoutMs: 300 });
  try {
    await assert.rejects(
      () => client.callTool("slow", {}),
      /MCP_TIMEOUT: no reply to tools\/call within 300ms/,
    );
  } finally {
    await client.close();
  }
});

test("client: a missing server binary fails as MCP_SERVER_EXITED", async () => {
  await assert.rejects(
    () =>
      connectStdioMcp(
        { command: "/nonexistent/sen-mcp-server" },
        { timeoutMs: 5000 },
      ),
    /MCP_SERVER_EXITED/,
  );
});

// --- Approval-gated execution ---

test("gate: default-deny refuses before the tool runs and logs once", async () => {
  const client = await connectedClient();
  const log = new ActivityLog();
  try {
    await assert.rejects(
      () =>
        executeMcpTool(client, { log }, {
          actor: "sen",
          server: "fake",
          tool: "echo",
          arguments: { hello: "world" },
          idempotencyKey: "k1",
        }),
      /TOOL_POLICY_DENIED/,
    );
    assert.equal(log.length, 1);
    assert.equal(log.entries()[0]?.result, "denied");
  } finally {
    await client.close();
  }
});

test("gate: an auto-tier tool with an allow rule runs, no approval needed", async () => {
  const client = await connectedClient();
  const log = new ActivityLog();
  try {
    const { result } = await executeMcpTool(
      client,
      { policy: { allow: ["read_file"] }, log },
      {
        actor: "sen",
        server: "fake",
        tool: "read_file",
        arguments: { path: "README.md" },
        idempotencyKey: "k2",
      },
    );
    assert.equal(result.isError, false);
    assert.equal(log.length, 1);
    assert.equal(log.entries()[0]?.result, "allowed");
  } finally {
    await client.close();
  }
});

test("gate: an ask-first tool needs an approval, then runs", async () => {
  const client = await connectedClient();
  const approvals = new ApprovalStore();
  const log = new ActivityLog();
  const input = {
    actor: "sen",
    server: "fake",
    tool: "echo",
    arguments: { hello: "world" },
    idempotencyKey: "k3",
  };
  try {
    await assert.rejects(
      () =>
        executeMcpTool(client, { policy: { allow: ["echo"] }, approvals, log }, input),
      /APPROVAL_REQUIRED/,
    );
    approvals.grant("echo", "k3", "zen");
    const { result } = await executeMcpTool(
      client,
      { policy: { allow: ["echo"] }, approvals, log },
      input,
    );
    assert.equal(result.isError, false);
    assert.equal(log.length, 2);
    assert.deepEqual(
      log.entries().map((e) => e.result),
      ["awaiting-approval", "allowed"],
    );
  } finally {
    await client.close();
  }
});

test("gate: a never-tier tool is denied even with a rule and an approval", async () => {
  const client = await connectedClient();
  const approvals = new ApprovalStore();
  approvals.grant("trade_securities", "k4", "zen");
  try {
    await assert.rejects(
      () =>
        executeMcpTool(
          client,
          { policy: { allow: ["trade_securities"] }, approvals },
          {
            actor: "sen",
            server: "fake",
            tool: "trade_securities",
            arguments: {},
            idempotencyKey: "k4",
          },
        ),
      /APPROVAL_NEVER_DENIED/,
    );
  } finally {
    await client.close();
  }
});

// --- HTTP transport (real local HTTP server) ---

function startHttpFake(): Promise<{
  url: string;
  close: () => Promise<void>;
}> {
  return new Promise((resolve) => {
    const server = createServer((req: IncomingMessage, res: ServerResponse) => {
      let body = "";
      req.on("data", (chunk: Buffer) => {
        body += chunk.toString("utf8");
      });
      req.on("end", () => {
        let msg: { id?: unknown; method?: string; params?: Record<string, unknown> };
        try {
          msg = JSON.parse(body) as typeof msg;
        } catch {
          res.writeHead(400);
          res.end("bad json");
          return;
        }
        const id = msg.id;
        const reply = (result: unknown) =>
          ({ jsonrpc: "2.0", id, result });
        const replyError = (code: number, message: string) =>
          ({ jsonrpc: "2.0", id, error: { code, message } });
        let payload: unknown;
        if (msg.method === "initialize") {
          payload = reply({
            protocolVersion: "2024-11-05",
            capabilities: {},
            serverInfo: { name: "sen-fake-http", version: "0.0.0" },
          });
        } else if (msg.method === "tools/list") {
          payload = reply({
            tools: [
              { name: "echo", description: "Echo", inputSchema: { type: "object" } },
            ],
          });
        } else if (msg.method === "tools/call") {
          // Answer tools/call as an SSE stream: the transport must
          // parse data: lines, not only plain JSON bodies.
          res.writeHead(200, { "content-type": "text/event-stream" });
          res.end(
            `data: ${JSON.stringify(reply({ content: [{ type: "text", text: JSON.stringify(msg.params?.arguments ?? {}) }], isError: false }))}\n\n`,
          );
          return;
        } else if (msg.method === "ping") {
          payload = reply({});
        } else {
          payload = replyError(-32601, "Method not found");
        }
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(payload));
      });
    });
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port =
        typeof address === "object" && address ? address.port : 0;
      resolve({
        url: `http://127.0.0.1:${port}/mcp`,
        close: () =>
          new Promise<void>((done) => {
            server.close(() => done());
          }),
      });
    });
  });
}

test("client: HTTP transport speaks JSON and SSE replies", async () => {
  const fake = await startHttpFake();
  const client = await connectHttpMcp({ url: fake.url }, { timeoutMs: 5000 });
  try {
    const tools = await client.listTools();
    assert.deepEqual(tools.map((t) => t.name), ["echo"]);
    const result = await client.callTool("echo", { a: 1 });
    assert.equal(
      (result.content[0] as { text: string }).text,
      JSON.stringify({ a: 1 }),
    );
    await client.ping();
  } finally {
    await client.close();
    await fake.close();
  }
});
