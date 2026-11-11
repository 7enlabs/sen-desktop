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
import { setBridgeTransport } from "../src/ipc/bridge.ts";
import type { ActionContract } from "../src/ipc/types.ts";
import { getModelSetting, setModelSetting } from "../src/ipc/settings-client.ts";

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
    assert.equal(args["model"], "some/model:free");
    assert.ok(!("key" in args) && !("apiKey" in args));
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
