// Live smoke test for the live session round (tin 4528/4542).
// Run ONLY by hand with: node --test packages/core/tests/live-smoke.test.ts
// (script test:live-smoke). This file is NOT run by any default test
// script, so the regular suites stay offline-clean.
//
// The smoke test runs ONLY when BOTH environment variables are present:
// OPENROUTER_API_KEY (a real key, supplied by the operator at runtime,
// never committed, never logged) and SEN_LIVE_SMOKE_MODEL (an explicit
// OpenRouter model id, supplied by the operator; there is no default).
// When either is missing, the single test below passes as an explicit
// SKIP with its reason, and NO network call is made, NO result is faked,
// and NO key is borrowed or created. A skipped smoke test is reported as
// skipped in the handover, never as a passed live run.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createOpenRouterRuntime, resolveOpenRouterModel } from "../src/model-channel.ts";
import { createLiveSenAgentSession } from "../src/agent-session.ts";

const KEY = process.env.OPENROUTER_API_KEY;
const MODEL_ID = process.env.SEN_LIVE_SMOKE_MODEL;

test("live smoke: real OpenRouter free model, one short steer", async () => {
  if (!KEY || !MODEL_ID) {
    // Explicit skip: the handover must report this as SKIPPED with the
    // reason (no live key and/or no explicit smoke model in env), never
    // as evidence that a live run happened.
    console.log(
      "LIVE SMOKE SKIPPED: OPENROUTER_API_KEY and/or SEN_LIVE_SMOKE_MODEL " +
        "not set in the environment; no network call made, no result faked.",
    );
    assert.ok(true);
    return;
  }
  const base = await mkdtemp(join(tmpdir(), "sen-live-smoke-"));
  const runtime = await createOpenRouterRuntime({ apiKey: KEY });
  const model = resolveOpenRouterModel(runtime, MODEL_ID);
  const handle = await createLiveSenAgentSession({
    modelRuntime: runtime,
    model,
    cwd: join(base, "cwd"),
    agentDir: join(base, "agent"),
  });
  const seen: string[] = [];
  let settled = false;
  const done = new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      if (!settled) {
        settled = true;
        void handle.abort().catch(() => {});
        reject(new Error("Live smoke timed out waiting for message_end."));
      }
    }, 120000);
    handle.subscribe((event) => {
      const type = (event as { type?: string }).type;
      if (type) {
        seen.push(type);
      }
      if (type === "message_end" && !settled) {
        settled = true;
        clearTimeout(timer);
        resolve();
      }
    });
  });
  try {
    await handle.steer("Reply with exactly: ok");
    await done;
  } finally {
    try {
      await handle.abort();
    } catch {
      // Abort after a completed or timed-out run is cleanup only.
    }
    handle.dispose();
  }
  assert.ok(
    seen.includes("message_end"),
    "Expected a message_end event, got: " + seen.join(","),
  );
});
