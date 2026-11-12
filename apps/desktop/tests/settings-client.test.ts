// Settings client tests for the CLI bridge round (tin 9530 quyết 2).
// Run with: node --test apps/desktop/tests/settings-client.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The injected transport stands in for the Rust handlers, which cannot run
// under node. These tests prove the frontend half of the settings calls:
// get/set invoke the right commands with the model, the result shape is
// validated, and no key field ever travels in either direction. They do
// NOT prove the Rust handlers read/write the settings file: that evidence
// is the standalone cli_settings tests.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import { getBaseUrlSetting, getModelSetting, setBaseUrlSetting, setModelSetting } from "../src/ipc/settings-client.ts";

function contract(): ActionContract {
  return {
    id: "c1",
    capability: "sen.settings",
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

test("settings-client get: invokes sen_settings_get_model, returns the model", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command, args) => {
    calls.push(command);
    assert.ok(!("key" in args) && !("apiKey" in args));
    return { model: "google/gemini-4-argon:free" };
  });
  const res = await getModelSetting(contract());
  assert.deepEqual(res, { model: "google/gemini-4-argon:free" });
  assert.deepEqual(calls, ["sen_settings_get_model"]);
  setBridgeTransport(null);
});

test("settings-client set: invokes sen_settings_set_model with the model, no key", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, ...args });
    // Rev2: the model travels nested under "args", the parameter name
    // the Rust handler declares; a flat "model" key is rejected by Tauri.
    assert.deepEqual(args["args"], { model: "some/model:free" });
    assert.ok(!("model" in args) && !("key" in args) && !("apiKey" in args));
    return { model: "some/model:free" };
  });
  const res = await setModelSetting(contract(), "some/model:free");
  assert.deepEqual(res, { model: "some/model:free" });
  assert.equal(seen[0]["command"], "sen_settings_set_model");
  setBridgeTransport(null);
});

test("settings-client: a malformed result fails loudly", async () => {
  setBridgeTransport(async () => ({ model: "" }));
  await assert.rejects(() => getModelSetting(contract()), /malformed/);
  setBridgeTransport(async () => ({}));
  await assert.rejects(() => setModelSetting(contract(), "m:free"), /malformed/);
  setBridgeTransport(null);
});

// R2B-05: the base URL client mirrors the model client: get/set invoke
// the right commands with the URL, null means nothing stored, and no
// key field ever travels in either direction.
test("settings-client get base URL: invokes sen_settings_get_base_url, returns the URL", async () => {
  const calls: string[] = [];
  setBridgeTransport(async (command, args) => {
    calls.push(command);
    assert.ok(!("key" in args) && !("apiKey" in args));
    return { baseUrl: "https://example.invalid/v1" };
  });
  const res = await getBaseUrlSetting(contract());
  assert.deepEqual(res, { baseUrl: "https://example.invalid/v1" });
  assert.deepEqual(calls, ["sen_settings_get_base_url"]);
  setBridgeTransport(async () => ({ baseUrl: null }));
  assert.deepEqual(await getBaseUrlSetting(contract()), { baseUrl: null });
  setBridgeTransport(null);
});

test("settings-client set base URL: invokes sen_settings_set_base_url with the URL, no key", async () => {
  const seen: Array<Record<string, unknown>> = [];
  setBridgeTransport(async (command, args) => {
    seen.push({ command, ...args });
    // Rev2: the URL travels nested under "args", the parameter name
    // the Rust handler declares; a flat "baseUrl" key is rejected by Tauri.
    assert.deepEqual(args["args"], { baseUrl: "https://example.invalid/v1" });
    assert.ok(!("baseUrl" in args) && !("key" in args) && !("apiKey" in args));
    return { baseUrl: "https://example.invalid/v1" };
  });
  const res = await setBaseUrlSetting(contract(), "https://example.invalid/v1");
  assert.deepEqual(res, { baseUrl: "https://example.invalid/v1" });
  assert.equal(seen[0]["command"], "sen_settings_set_base_url");
  setBridgeTransport(null);
});

test("settings-client: a malformed base URL result fails loudly", async () => {
  setBridgeTransport(async () => ({ baseUrl: 42 }));
  await assert.rejects(() => getBaseUrlSetting(contract()), /malformed/);
  setBridgeTransport(async () => ({}));
  await assert.rejects(() => setBaseUrlSetting(contract(), "https://example.invalid/v1"), /malformed/);
  setBridgeTransport(null);
});

// R2B-05 rev2: the set payloads must match the wire shape the Rust
// handlers actually declare. Tauri 2.12.5 (tauri-macros 2.7.4) decodes
// each command parameter from the payload key named after that
// parameter (camelCase by default); a missing key rejects the call
// before any handler logic runs ("command ... missing required key
// args"). The v1 client sent a flat payload ({ contract, baseUrl })
// while the handler declares `args: SetBaseUrlArgs`, so a real app
// always rejected the set call - and the v1 mock-transport tests could
// not catch it because they only asserted what the client sent (they
// encoded the wrong flat shape). These tests read the handler
// signature out of settings.rs and assert the client payload against
// it, so they FAIL on the v1 payload and PASS only when the client
// sends the shape the handler waits for.
function rustSettingsSource(): string {
  return readFileSync(
    new URL("../src-tauri/src/commands/settings.rs", import.meta.url),
    "utf8",
  );
}

function toCamelCase(snake: string): string {
  return snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
}

/// Expected wire shape for a set command, derived from the Rust
/// handler: the struct-typed parameter name (e.g. "args") plus the
/// camelCase field names of its args struct.
function expectedWireShape(
  src: string,
  command: string,
): { paramKey: string; fields: string[] } {
  const fnMatch = src.match(new RegExp(`pub fn ${command}\\(([\\s\\S]*?)\\) ->`));
  assert.ok(fnMatch, `Rust handler ${command} not found in settings.rs`);
  const argsMatch = fnMatch[1].match(/(\w+):\s*(\w+Args)/);
  assert.ok(argsMatch, `${command} declares no *Args parameter in settings.rs`);
  const paramKey = argsMatch[1];
  const structName = argsMatch[2];
  const structMatch = src.match(new RegExp(`pub struct ${structName} \\{([\\s\\S]*?)\\}`));
  assert.ok(structMatch, `Rust struct ${structName} not found in settings.rs`);
  const structHead = src.slice(Math.max(0, (structMatch.index ?? 0) - 200), structMatch.index ?? 0);
  assert.match(structHead, /rename_all = "camelCase"/, `${structName} is not camelCase-renamed`);
  const fields = [...structMatch[1].matchAll(/pub (\w+):/g)].map((m) => toCamelCase(m[1]));
  assert.ok(fields.length > 0, `${structName} has no fields`);
  return { paramKey, fields };
}

async function captureSetPayload(
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

test("settings-client set base URL: payload matches the Rust handler wire shape (R2B-05 rev2)", async () => {
  const { paramKey, fields } = expectedWireShape(rustSettingsSource(), "sen_settings_set_base_url");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["baseUrl"]);
  const payload = await captureSetPayload(
    "sen_settings_set_base_url",
    { baseUrl: "https://example.invalid/v1" },
    () => setBaseUrlSetting(contract(), "https://example.invalid/v1"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { baseUrl: "https://example.invalid/v1" });
  assert.ok(!("baseUrl" in payload), "payload must not carry a flat baseUrl key");
  assert.ok("contract" in payload, "payload must still carry the contract");
});

test("settings-client set model: payload matches the Rust handler wire shape (R2B-05 rev2)", async () => {
  const { paramKey, fields } = expectedWireShape(rustSettingsSource(), "sen_settings_set_model");
  assert.equal(paramKey, "args");
  assert.deepEqual(fields, ["model"]);
  const payload = await captureSetPayload(
    "sen_settings_set_model",
    { model: "some/model:free" },
    () => setModelSetting(contract(), "some/model:free"),
  );
  assert.ok(paramKey in payload, `payload is missing the "${paramKey}" key the Rust handler waits for`);
  assert.deepEqual(payload[paramKey], { model: "some/model:free" });
  assert.ok(!("model" in payload), "payload must not carry a flat model key");
  assert.ok("contract" in payload, "payload must still carry the contract");
});
