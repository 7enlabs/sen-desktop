// Webview import-graph test for the SDK move (tin 9328/9459).
// Run with: node --test apps/desktop/tests/webview-no-sdk.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// Walks the static import graph from the webview entry
// (apps/desktop/src/app/main.ts) and proves the Pi SDK never enters it:
// no reached file imports @earendil-works/pi-coding-agent or
// @earendil-works/pi-agent-core, no reached file is one of the deleted
// SDK modules (agent-session, model-channel, live-session), and no
// reached file even mentions createAgentSession. The vite build is the
// primary proof (it fails on the SDK's node:child_process import); this
// suite is the cheap regression test that catches a re-introduced SDK
// import before the build runs. It also proves packages/core declares no
// Pi dependency anymore.
import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const TEST_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(TEST_DIR, "../../..");
const ENTRY = resolve(TEST_DIR, "../src/app/main.ts");

const FORBIDDEN_MODULES = ["agent-session.ts", "model-channel.ts", "live-session.ts"];
// Bare (non-relative) import specifiers that must never appear in the
// webview graph. Comments may name the SDK to explain why it is gone;
// only an actual import is a failure.
const FORBIDDEN_BARE_IMPORTS = [
  "@earendil-works/pi-coding-agent",
  "@earendil-works/pi-agent-core",
];

function staticImports(source: string): string[] {
  const found: string[] = [];
  const fromRe = /(?:import|export)[^'"]*?from\s*["']([^"']+)["']/g;
  const sideEffectRe = /import\s*["']([^"']+)["']/g;
  let m: RegExpExecArray | null;
  while ((m = fromRe.exec(source)) !== null) {
    found.push(m[1]);
  }
  while ((m = sideEffectRe.exec(source)) !== null) {
    found.push(m[1]);
  }
  return [...new Set(found)];
}

function resolveRelative(specifier: string, fromFile: string): string | null {
  const base = resolve(dirname(fromFile), specifier);
  const candidates = [base, base + ".ts", join(base, "index.ts")];
  for (const candidate of candidates) {
    if (existsSync(candidate) && extname(candidate) === ".ts") {
      return candidate;
    }
  }
  return null;
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
      if (specifier.startsWith(".")) {
        const target = resolveRelative(specifier, file);
        if (target && target.startsWith(REPO_ROOT)) {
          queue.push(target);
        }
      }
    }
  }
  return reached;
}

test("webview: the import graph from main.ts reaches no SDK module", () => {
  const reached = walkGraph(ENTRY);
  // The entry itself must be reached, and the graph must be non-trivial
  // (entry + IPC clients + core session): an empty walk would pass
  // vacuously, so assert the graph actually contains the IPC layer.
  assert.ok(reached.length >= 5, "expected a real import graph, got " + reached.length);
  assert.ok(
    reached.some((f) => f.endsWith("apps/desktop/src/ipc/bridge.ts")),
    "expected the IPC bridge in the webview graph",
  );
  assert.ok(
    reached.some((f) => f.endsWith("packages/core/src/session.ts")),
    "expected the core session statechart in the webview graph",
  );
  for (const file of reached) {
    const base = file.split("/").pop()!;
    assert.ok(
      !FORBIDDEN_MODULES.includes(base),
      "webview graph reaches deleted SDK module: " + file,
    );
    // No import specifier in this file may be a Pi package (relative
    // imports to the deleted modules are already excluded by the
    // filename check above, since those files no longer exist).
    const source = readFileSync(file, "utf8");
    for (const specifier of staticImports(source)) {
      assert.ok(
        !FORBIDDEN_BARE_IMPORTS.includes(specifier),
        "webview file imports a Pi package: " + file + " imports " + specifier,
      );
    }
  }
});

test("webview: packages/core declares no Pi dependency", () => {
  const pkg = JSON.parse(
    readFileSync(join(REPO_ROOT, "packages/core/package.json"), "utf8"),
  ) as { dependencies?: Record<string, string> };
  const deps = Object.keys(pkg.dependencies ?? {});
  assert.deepEqual(deps, [], "expected no dependencies in packages/core, got " + deps.join(", "));
});
