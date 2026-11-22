#!/usr/bin/env node
// Fake `zen` binary for delegate tests (Sen P0-5). It records its
// argv to the file in FAKE_ZEN_ARGV_FILE, then prints stream-json
// records like the real CLI: an init record, then a result record.
// FAKE_ZEN_MODE controls the result:
// - unset: success, result text from FAKE_ZEN_RESULT (default "ok").
// - error: a result record with is_error true.
// - noresult: exit 0 with no result record at all.
// - hang: print the init record, then wait forever (timeout path;
//   the caller kills it).
// This file is test-only: it is the faked CLI boundary, never the
// code under test.
import { writeFileSync } from "node:fs";

const argvFile = process.env.FAKE_ZEN_ARGV_FILE;
if (argvFile) {
  writeFileSync(argvFile, JSON.stringify(process.argv.slice(2)), "utf8");
}

const mode = process.env.FAKE_ZEN_MODE ?? "";
const resultText = process.env.FAKE_ZEN_RESULT ?? "ok";

function out(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

out({
  type: "system",
  subtype: "init",
  session_id: "s1",
  cwd: "/tmp",
  model: "m:free",
  tools: [],
  mcp_servers: [{ name: "fake", status: "connected" }],
  permissionMode: "default",
  slash_commands: [],
  v: 1,
});

if (mode === "hang") {
  setInterval(() => {}, 1000);
} else if (mode === "noresult") {
  process.exit(0);
} else if (mode === "error") {
  out({
    type: "result",
    subtype: "error",
    is_error: true,
    result: "delegated tool failed",
    duration_ms: 1,
    num_turns: 1,
    usage: { input_tokens: 1, output_tokens: 1 },
    model: "m:free",
    session_id: "s1",
    v: 1,
  });
  process.exit(1);
} else {
  out({
    type: "result",
    subtype: "success",
    is_error: false,
    result: resultText,
    duration_ms: 1,
    num_turns: 1,
    usage: { input_tokens: 1, output_tokens: 1 },
    model: "m:free",
    session_id: "s1",
    v: 1,
  });
  process.exit(0);
}
