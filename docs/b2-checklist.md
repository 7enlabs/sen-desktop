# Checklist for wiring Pi SDK 1.3.0 - scaffold B-2 status

Source: prep B-2 section 3. The status below is the real status of this
local bundle, not the desired status.

- [ ] 1. Pin exactly 1.3.0 for both packages: RECORDED in
  packages/core/package.json AND in pnpm-lock.yaml (specifier 1.3.0,
  resolved 1.3.0). Installed in a scratch copy only for the lockfile;
  node_modules is NOT part of the bundle. Registry re-checked during the
  install round: both packages exist at 1.3.0.
- [ ] 2. MIT license for both packages: RECORDED in package.json;
  CONFIRMED on the npm registry and in the installed package.json files
  (both packages: version 1.3.0, license MIT) during the install round.
- [ ] 3. apps/desktop ESM-only: type module RECORDED; NOT CHECKED by a
  build yet because the Tauri app is still a stub.
- [x] 4. packages/core wraps createAgentSession: DONE in the fill-in round
  (messages 4128/4138). packages/core/src/agent-session.ts imports
  createAgentSession from @earendil-works/pi-coding-agent 1.3.0 for real
  (NOT from pi-agent-core, which does not export it). The wrapper requires
  an explicitly provided model runtime and refuses without one: no default
  model per message 4070, no key, no network call, no live session yet.
  SessionManager binds an SDK handle through bindSdkSession(); the local
  statechart is unchanged. Wiring test: packages/core/tests/sdk-wiring.test.ts
  (runs after pnpm install; the default test script stays install-free).
- [ ] 5. packages/providers locks the Pi version: constant 1.3.0
  RECORDED; ModelRuntime NOT FILLED IN; default model NONE per decision
  message 4070; providers is only a channel-declaration shell.
- [ ] 6. packages/tools permission layer: tool_call hook frame EXISTS;
  the real permission logic is NOT FILLED IN.
- [ ] 7. Tauri invoke + IPC bridge: contract types and contract-check
  function EXIST; the real invoke is NOT HERE at the frontend binding level
  (missing @tauri-apps/api plus the app bundler); the core-side SDK wiring
  is real since the fill-in round (agent-session.ts). Tauri version 2
  decided major per message 4070; exact patch 2.12.5 + Cargo.lock DONE in
  the lockfile round; cargo check green on the Leader VM for the merged
  head (message 4124).
- [ ] 8. Raw event log: RawLog class EXISTS as real structure; NO REAL
  DATA yet; NO AfterDeleteBench closing line yet.
- [ ] 9. Single 6-step execution gate: order and order-check function
  EXIST; the real steps are NOT FILLED IN.
- [ ] 10. No banned modes applied: RECORDED as bans in code comments;
  CHECKED by grep in the condition round: no banned name appears as a
  dependency (core dependencies are only the two Pi packages); mentions in
  docs and comments are prohibitions, not usage.

Stop conditions, per prep section 3: stop at once if the SDK version is
not 1.3.0, the license is not MIT, a shortcut path outside the gate
exists, the raw log is missing, cua-driver is requested before the Z Red
gate on 10/11, or iOS is requested before a new decision. After two
failures at the same step, stop and re-plan.

Points A1-A4: A1 missing log means FAIL, recorded in the bridge. A2 model
calls only serve the default display, switched OFF; NO REAL off-switch
mechanism yet. A3 third-party gates and runtimes banned, NOT CHECKED. A4
per-shift token budget report with a separate auxiliary-call column, NOT
HERE. Open condition A1-A4: whether Pi SDK 1.3.0 has auxiliary calls of
this kind is NOT VERIFIED (the install round verified exports only, not
call behavior).
