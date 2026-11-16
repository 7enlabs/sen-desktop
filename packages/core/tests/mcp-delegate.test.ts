// Delegate-to-CLI-core tests (Sen P0-5).
// Run with: node --test packages/core/tests/mcp-delegate.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The delegate spawns the REAL fake `zen` binary
// (tests/fixtures/mcp-fake-zen.mjs, executable with a node shebang)
// through the real child_process spawn; only the CLI boundary is
// faked. Env vars FAKE_ZEN_* steer the fake binary per test.
import test from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { dirname } from "node:path";
import {
  buildDelegateArgv,
  buildDelegatePrompt,
  delegateToolCallToCliCore,
  writeMcpConfigFile,
  type McpServerConfig,
} from "../src/mcp-client.ts";

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), "fixtures");
const FAKE_ZEN = join(FIXTURES, "mcp-fake-zen.mjs");
chmodSync(FAKE_ZEN, 0o755);

const SERVER: McpServerConfig = {
  command: "node",
  args: ["server.mjs"],
};

function withFakeZenEnv(
  mode: string | undefined,
  result: string | undefined,
  run: () => Promise<void>,
): Promise<void> {
  const saved: Record<string, string | undefined> = {};
  for (const key of ["FAKE_ZEN_MODE", "FAKE_ZEN_RESULT", "FAKE_ZEN_ARGV_FILE"]) {
    saved[key] = process.env[key];
  }
  if (mode === undefined) {
    delete process.env.FAKE_ZEN_MODE;
  } else {
    process.env.FAKE_ZEN_MODE = mode;
  }
  if (result === undefined) {
    delete process.env.FAKE_ZEN_RESULT;
  } else {
    process.env.FAKE_ZEN_RESULT = result;
  }
  return run().finally(() => {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  });
}

// --- Prompt and argv ---

test("delegate: the prompt names the server, tool, and exact args", () => {
  const prompt = buildDelegatePrompt("fake", "echo", { a: 1 });
  assert.ok(prompt.includes('"echo"'));
  assert.ok(prompt.includes('"fake"'));
  assert.ok(prompt.includes('{"a":1}'));
});

test("delegate: argv carries the bridge flags, --mcp-config, and the prompt last", () => {
  const argv = buildDelegateArgv(
    {
      serverName: "fake",
      serverConfig: SERVER,
      tool: "echo",
      arguments: { a: 1 },
      model: "m:free",
    },
    "/tmp/x/mcp-servers.json",
    "s-1",
  );
  assert.deepEqual(argv.slice(0, 8), [
    "-p",
    "--output-format",
    "stream-json",
    "--bare",
    "--setting-sources",
    "",
    "--mcp-config",
    "/tmp/x/mcp-servers.json",
  ]);
  assert.ok(argv.includes("--model"));
  assert.ok(argv.includes("--session-id"));
  assert.ok(argv.includes("s-1"));
  assert.equal(argv[argv.length - 1]?.includes('"echo"'), true);
  const resumed = buildDelegateArgv(
    {
      serverName: "fake",
      serverConfig: SERVER,
      tool: "echo",
      resume: true,
    },
    "/tmp/x/mcp-servers.json",
    "s-1",
  );
  assert.ok(resumed.includes("-r"));
  assert.ok(!resumed.includes("--session-id"));
});

test("delegate: the written config file parses as zen-mcp expects", () => {
  const { dir, path } = writeMcpConfigFile("fake", SERVER);
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as {
      mcpServers: Record<string, unknown>;
    };
    assert.deepEqual(parsed, { mcpServers: { fake: SERVER } });
    assert.ok(path.endsWith("mcp-servers.json"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// --- Real spawn of the fake zen binary ---

test("delegate: a success turn returns the result record text", async () => {
  const argvFile = join(mkdtempSync(join(tmpdir(), "sen-argv-")), "argv.json");
  await withFakeZenEnv(undefined, "echo: hello", async () => {
    process.env.FAKE_ZEN_ARGV_FILE = argvFile;
    const out = await delegateToolCallToCliCore({
      zenBin: FAKE_ZEN,
      sessionId: "s-delegate-1",
      serverName: "fake",
      serverConfig: SERVER,
      tool: "echo",
      arguments: { hello: "world" },
    });
    assert.equal(out.text, "echo: hello");
    assert.equal(out.sessionId, "s-delegate-1");
    assert.equal(out.exitCode, 0);
    const argv = JSON.parse(readFileSync(argvFile, "utf8")) as string[];
    assert.ok(argv.includes("--mcp-config"));
    const at = argv.indexOf("--mcp-config");
    assert.ok(argv[at + 1]?.endsWith("mcp-servers.json"));
    assert.ok(argv.includes("--session-id"));
    assert.ok(argv[argv.length - 1]?.includes('"echo"'));
  });
  rmSync(dirname(argvFile), { recursive: true, force: true });
});

test("delegate: an error result record fails as CLI_DELEGATE_ERROR", async () => {
  await withFakeZenEnv("error", undefined, async () => {
    await assert.rejects(
      () =>
        delegateToolCallToCliCore({
          zenBin: FAKE_ZEN,
          serverName: "fake",
          serverConfig: SERVER,
          tool: "echo",
        }),
      /CLI_DELEGATE_ERROR: delegated tool failed/,
    );
  });
});

test("delegate: no result record fails as CLI_NO_RESULT", async () => {
  await withFakeZenEnv("noresult", undefined, async () => {
    await assert.rejects(
      () =>
        delegateToolCallToCliCore({
          zenBin: FAKE_ZEN,
          serverName: "fake",
          serverConfig: SERVER,
          tool: "echo",
        }),
      /CLI_NO_RESULT/,
    );
  });
});

test("delegate: a hanging CLI is killed as CLI_TURN_TIMEOUT", async () => {
  await withFakeZenEnv("hang", undefined, async () => {
    await assert.rejects(
      () =>
        delegateToolCallToCliCore({
          zenBin: FAKE_ZEN,
          serverName: "fake",
          serverConfig: SERVER,
          tool: "echo",
          timeoutMs: 300,
        }),
      /CLI_TURN_TIMEOUT: no result record within 300ms/,
    );
  });
});

test("delegate: a missing zen binary fails as CLI_SPAWN_FAILED", async () => {
  await assert.rejects(
    () =>
      delegateToolCallToCliCore({
        zenBin: "/nonexistent/zen",
        serverName: "fake",
        serverConfig: SERVER,
        tool: "echo",
      }),
    /CLI_SPAWN_FAILED/,
  );
});

test("delegate: an invalid server config is refused before spawning", async () => {
  await assert.rejects(
    () =>
      delegateToolCallToCliCore({
        zenBin: FAKE_ZEN,
        serverName: "fake",
        serverConfig: {},
        tool: "echo",
      }),
    /MCP_CONFIG_INVALID/,
  );
});
