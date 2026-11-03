# Build notes - scaffold B-2, local bundle not pushed yet

Build date: 03/11/2026, during the work shift. Status: NOT DONE. No
independent verification exists.

## 1. Basis and fast head

- Purpose: Telegram message 4064 dated 03/11/2026, SD Leader opening the
  B-2 build gate in writing, tagging @sddeveloper_bot. Direction: build
  from the approved full-text prep file, through the PR fast gate, through
  ZC Leader review, never self-merge.
- Fast head of dev/scaffold: 4564a9c, per Leader verification in message
  4055 and repeated in message 4064.
- NOT RE-VERIFIED from an independent source: this workspace has no repo
  clone, this round opened no connection and ran no head-freshness check.
  The number 4564a9c here is only quoted from the Leader message, not a
  freshness-check result of this round.
- No push, no commit to a remote, no PR opened, no merge, nothing sent to
  Telegram in this round.

## 2. What was created

pnpm repo frame, ESM-only, per prep section 1:

- Root package.json, pnpm-workspace.yaml, .gitattributes (LF),
  .editorconfig, README.md.
- apps/desktop: package.json, src-tauri/Cargo.toml,
  src-tauri/tauri.conf.json, src-tauri/src/main.rs,
  src-tauri/src/commands/mod.rs, session.rs, tools.rs, events.rs,
  src/ipc/types.ts, src/ipc/bridge.ts, src/app/main.ts, src/app/index.html.
- packages/core: package.json pinning Pi SDK 1.3.0, src/index.ts,
  session.ts, raw-log.ts, execution-gate.ts, token-economizer.ts, paths.ts.
- packages/providers, tools, prompts, ui: each package has a package.json
  and src/index.ts.
- evals/README.md, docs/b2-checklist.md, build-notes.md (this file).

## 3. Which files hold real content, which are frames or stubs

Real in structure and pure logic, but NOT RUN, NOT BUILT, NO AUTOMATED
CHECK before the condition round (tests added in section 8 cover the 4
logic files only):

- apps/desktop/src/ipc/types.ts: action contract types from point 888,
  real in data fields.
- apps/desktop/src/ipc/bridge.ts: contract-check function and raw-log
  gate (A1), real in logic; the real Tauri invoke call is a STUB.
- packages/core/src/session.ts: point-854 statechart, real in the
  transition table; the real createAgentSession call is a STUB.
- packages/core/src/raw-log.ts: raw log class, real in structure, no real
  data yet, no AfterDeleteBench closing line.
- packages/core/src/execution-gate.ts: 6-step order, real; the real check
  steps are STUBs.
- packages/core/src/token-economizer.ts: point-856 stop-turn rule, real in
  constants and the judge function; the real detection is a STUB.
- packages/core/src/paths.ts: single path variable, real in mechanism, no
  real root yet.
- packages/core/package.json: pin 1.3.0, real as a declaration text,
  installed only in a scratch copy during the condition round (section 8).

Frames or stubs, no real runnable code:

- All of src-tauri: main.rs, commands/session.rs, tools.rs, events.rs,
  tauri.conf.json, Cargo.toml.
- apps/desktop/src/app/main.ts and index.html: UI shell not filled in.
- packages/providers/src/index.ts: ModelRuntime not filled in, default
  model NONE per message 4070.
- packages/tools/src/index.ts: tool_call hook frame, real permission logic
  not filled in.
- packages/prompts/src/index.ts and packages/ui/src/index.ts:
  ResourceLoader and renderer not filled in.
- evals: no real eval case yet.

## 4. Constraints kept

- Pi SDK pinned at 1.3.0, never raised to 1.4.0. No cua-driver, no iOS
  target, no F8 MCP, no bundled font, no TrustBoost, NVISO,
  agent-memory-guard Python, Decepticon.
- No OS-dependent shell command at the frame layer, no path joining by
  string, no home-directory assumption.
- No credential, key, token. No paid-tier probing. No package installed
  in the bundle itself (install ran only in a scratch copy, section 8).

## 5. Still open and unmet conditions

- Fast head of dev/scaffold NOT RE-VERIFIED from an independent source
  (see section 1).
- Full text of frame B-S2 NOT HERE. Full text of decision range B-S1 to
  B-S8 NOT HERE. If they appear later, cross-check every prep item at
  once; fix any drift before merge.
- Full text of output ticket message 1059, checklist message 965, early
  application message 972 NOT HERE, only captions seen.
- Tauri exact patch + Cargo.lock: STILL PENDING, no Rust toolchain on
  this machine (see section 8). Major version 2 is decided per
  message 4070.
- Pi SDK install round (section 8) verified the export location of
  createAgentSession, but the real import in session.ts is NOT FILLED IN.
- Open conditions A1-A4 NOT VERIFIED: whether Pi SDK 1.3.0 has auxiliary
  calls of that kind.
- F8 MCP waits for the full-text cross-check of B-1 and STU-2. F5
  cua-driver postponed until after the Z Red gate on 10/11.
- This bundle is NOT BUILT as an app and NOT RUN as an app. Any claim
  that it runs is wrong if added.

## 6. Update per the parameter decisions in message 4070 (round at 18:40 on 03/11/2026)

- ZC Leader message 4070 decided: Tauri version 2; identifier
  dev.7enlab.sen-desktop (temporary); minimum permissions: core Tauri
  only, no plugin added; main window 1200x800, temporary title Pi Desktop
  until the 10/11 Sen-name verification; default model NONE, providers
  only a channel-declaration shell.
- Fresh head: ZC Leader verified directly at 18:35 in message 4070, the
  dev/scaffold branch still at 4564a9c, matching the prep. The seat has
  NOT RE-VERIFIED it from an independent source; the head number here is
  quoted from the Leader message.
- Changed in the bundle, still exactly 35 files at that point, no file
  added: Cargo.toml records Tauri 2 (exact patch + lockfile deferred to
  the real SDK install round after structural review); tauri.conf.json
  records the identifier, the 1200x800 window, the Pi Desktop title, and
  core-only permissions; index.html changed its title to Pi Desktop;
  providers/index.ts dropped the default-model field and records that there
  is NO model.
- Still NOT BUILT, NO PACKAGE INSTALLED in the bundle, NO REAL SDK
  WIRED. The sequence from message 4070 stands: Leader structural review
  first, then the seat installs the real SDK + build check, only then a
  merge decision.
- Old open points in section 5 partly closed: Tauri version (major),
  identifier, permissions, desktop shell, default model are decided per
  message 4070. Full text of B-S2, range B-S1 to B-S8, output ticket 1059,
  checklist 965, early application 972 still NOT HERE; the cross-check
  clause when the originals appear still stands.

## 7. Next steps needing Leader review

1. Leader pushes this bundle to a new branch + opens a PR for structural
   review (never straight to dev/scaffold, never self-merge). Origin head
   of dev/scaffold per Leader verification in message 4070 is 4564a9c.
2. Structural parameters decided in message 4070 (see section 6); if the
   originals appear later, cross-check at once and fix drift before merge.
3. Leader supplies the full text of B-S2, range B-S1 to B-S8, output
   ticket message 1059, checklist message 965, early application message
   972 for cross-check before the build is fixed.
4. After those items arrive: install Pi SDK 1.3.0, cross-check the MIT
   license, fill in the real SDK import, fill in the real gate steps, then
   build and run checks.
5. Submit through the PR fast gate for ZC Leader review, never self-merge.
   Verification must be done by someone else, not by the producer of this
   bundle.

## 8. Pre-merge condition round per Leader structural review (message 4077)

Work date: 03/11/2026, evening round. Result per condition; every claim
below is producer-run evidence, NOT independent verification. The seat
re-verifies before any merge decision.

### 8.1 Condition 1: stale "Tauri version undecided" strings - FIXED

Fixed in 4 places found in one sweep (the review named 3; the sweep found
one more with the same stale claim):

- Root package.json build script: the echo no longer says the Tauri
  version is undecided; it says the build is not ready and Tauri 2 is
  decided per message 4070.
- apps/desktop/src/ipc/bridge.ts: header comment, invoke comment, and
  error message rewritten; the stub reason is now "not implemented yet",
  not "version undecided".
- apps/desktop/src/app/main.ts: the shell-technology open point rewritten
  as decided (Tauri 2 per message 4070); the file is still a stub shell.
- apps/desktop/src-tauri/src/main.rs: the "after the Tauri version is
  decided" comment rewritten the same way.
- Sweep proof after the fix: grep for "CHUA CHOT" and "chua chot" across
  the bundle returns no match in code or comments (the phrase survives
  only in this notes file as a historical quote of what was fixed).

### 8.2 Condition 2: @sen/* scope is a proposal only - KEPT, NOT DECIDED

- No package renamed. Names stay `@sen/desktop`, `@sen/core`,
  `@sen/providers`, `@sen/tools`, `@sen/prompts`, `@sen/ui`.
- Root README.md gained a "Package scope: PROPOSAL, pending
  confirmation" section: the scope waits for SD Leader confirmation at
  review time; the fallback, if not agreed, is unscoped names (desktop,
  core, providers, tools, prompts, ui) before merge.
- Every per-package README repeats the same scope note for its own
  package. No package is published under the scope until confirmation
  exists.

### 8.3 Condition 3: section 1 frame files - ADDED

Added, all in English: .gitignore, LICENSE (MIT, copyright holder
"7EN Labs" taken from the org name; the Leader may correct the holder
line), CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, CHANGELOG.md,
plus one README per package: apps/desktop, packages/core,
packages/providers, packages/tools, packages/prompts, packages/ui.
File count went from 35 to 49 (35 translated + 12 new frame files +
1 test file + pnpm-lock.yaml).

### 8.4 Condition 4: lockfiles + frozen-lockfile evidence - HALF DONE

Toolchain check first (real commands, this machine):

- node v24.22.1 at /usr/bin/node: PRESENT.
- npm: PRESENT (via the image tooling path).
- pnpm: MISSING as an installed command; ran through npx as
  pnpm@10.0.0 (10.0.0, matching the packageManager field), free public
  registry, no paid path.
- cargo: MISSING. rustc: MISSING. No Rust toolchain on this machine,
  matching the earlier standing note. No Rust toolchain was installed in
  this round.

pnpm side, DONE with real evidence:

- Scratch copy at /tmp/sen-desktop-b2-install (bundle copy, not the
  bundle itself). Command: `npx -y pnpm@10.0.0 install`. Result: 119
  packages, 118 downloaded, done in 14.1s, exit 0. One warning: 1
  deprecated subdependency (node-domexception@1.0.0). Two build scripts
  ignored by pnpm policy (@google/genai, protobufjs); no rebuild run,
  no behavior depends on them here.
- pnpm-lock.yaml (lockfileVersion 9.0, 37,339 bytes) copied into the
  bundle. It pins `@earendil-works/pi-agent-core` specifier 1.3.0 and
  `@earendil-works/pi-coding-agent` specifier 1.3.0, both resolved at
  1.3.0.
- Frozen-lockfile proof: `npx -y pnpm@10.0.0 install --frozen-lockfile`
  in the scratch copy returned "Already up to date", exit 0.
- node_modules is NOT in the bundle and NOT in the handover archive;
  .gitignore excludes it.

Cargo side, BLOCKED, nothing fabricated:

- No Cargo.lock was written, because generating one needs cargo plus
  declared Tauri dependencies, and Cargo.toml still declares no
  dependencies on purpose (the review sequence keeps the patch choice for
  the install round). The exact Tauri patch is therefore still
  unselected, and the lockfile is still missing.
- Concrete next step: on a box with a Rust toolchain, fill in the Tauri
  dependencies at exact 2.x patches, run `cargo generate-lockfile` (or a
  full build), and commit the resulting Cargo.lock. Do not hand-write a
  Cargo.lock.

### 8.5 Condition 5: tests for the 4 real logic files - ADDED, RUN GREEN

- New file: packages/core/tests/logic.test.ts, 6 tests, node:test +
  node:assert/strict, run with plain node (type stripping, no build
  step). Command: `node --test packages/core/tests/logic.test.ts`.
- First run: 5 pass, 1 fail. The failure was a WRONG TEST EXPECTATION,
  not a code bug: the test asserted Running to Failed is illegal, but the
  point-854 table allows it. The test was corrected to assert the truly
  illegal AwaitingApproval to Error path; no production code changed for
  the test.
- Second run (bundle) and third run (installed scratch copy): 6 pass,
  0 fail, both times. Coverage: statechart table + SessionManager
  lifecycle, gate order exactness + stub throw, economizer constants and
  stop rules (including the zero-typical-length guard), path variable
  unset/set behavior.
- What the tests do NOT prove: SDK behavior, the Tauri app, the real
  gate steps, the permission logic, rendering, or prompts. Those parts
  are still stubs and are not covered.

### 8.6 Condition 6: name unification - PENDING DECISION 10/11, NOT DECIDED

- Nothing renamed. Window title stays Pi Desktop, productName stays Sen
  Desktop, identifier stays dev.7enlab.sen-desktop.
- Every place that mentions a name now marks it temporary and points at
  the 10/11 decision: root README section "Names: DECISION PENDING
  10/11", tauri.conf.json note, index.html comment, apps/desktop README.
- The 10/11 decision still has to settle the single final name. Until
  then, no file claims a final name.

### 8.7 Condition 7: English only on GitHub - DONE for the 35 PR files

- All 35 original files had their comments and descriptions translated
  from unaccented Vietnamese to English. Code logic is unchanged except
  error-message wording (translated to English as part of the same pass;
  the FAIL markers are kept).
- New files from condition 3 and the test file were written in English
  directly.
- Sweep proof after the pass: grep for the old Vietnamese markers
  ("CHUA", "chua", "khong", "duoc", "thieu", "theo prep") returns no
  match in the bundle source, docs, or configs, except historical quotes
  inside this notes file (sections 8.1 and 8.7 themselves) and the
  Vietnamese-free proper names (Pi, Sen, Tauri).
- index.html lang attribute changed from "vi" to "en". Window title
  "Pi Desktop" is unchanged (temporary name, condition 6).

### 8.8 Real SDK install finding: createAgentSession lives in pi-coding-agent

During the install round, both Pi packages were imported for real in the
scratch copy (no key, no network call, import only):

- @earendil-works/pi-agent-core 1.3.0, license MIT: exports Agent,
  agentLoop, runAgentLoop, runToolCall, setDefaultStreamFn, streamProxy,
  and others. It does NOT export createAgentSession.
- @earendil-works/pi-coding-agent 1.3.0, license MIT: exports
  createAgentSession (function), createAgentSessionFromServices,
  createAgentSessionRuntime, AgentSession, AgentSessionRuntime,
  SessionManager, ModelRuntime, and others.
- Consequence for the fill-in round: session.ts must import
  createAgentSession from pi-coding-agent, not from pi-agent-core. This
  closes the open point in section 5 about which package holds the call.
  The import itself is NOT filled in by this round; the stub stands.

### 8.9 What is still blocked after this round

1. Cargo.lock + Tauri exact patch: blocked on a missing Rust toolchain
   (cargo/rustc absent). Needs a box with Rust.
2. Tauri app build (cargo build / tauri build): blocked on the same cause
   plus the undeclared dependencies.
3. Real SDK wiring (import + gate steps + permission logic + invoke):
   not started; needs the fill-in round after structural review.
4. Independent verification: NOT DONE by design. This round's test and
   install outputs are producer-run evidence; the seat re-verifies.
5. Name unification: waits for the 10/11 decision.
6. Scope confirmation: waits for SD Leader confirmation at review.
