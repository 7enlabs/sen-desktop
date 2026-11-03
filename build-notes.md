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

### 8.10 Cargo.lock round per message 4086 (03/11/2026, ZC Leader)

- Rust stable installed on the seat machine with rustup (profile minimal):
  cargo 1.99.1, rustc 1.99.1. This closes the missing-toolchain block in 8.9.1
  for lockfile generation on this machine.
- Cargo.toml (apps/desktop/src-tauri) now pins tauri = "=2.12.5" (exact patch
  resolved by cargo generate-lockfile on 03/11/2026; 406 packages locked).
- Cargo.lock generated alongside Cargo.toml: 4.300 lines, SHA256
  9544ab7bd9c973058de525834b54788f06e9e1b291735fa5af4a6046e31feb81,
  tauri entry version 2.12.5 from crates.io. Desktop-line patch was not
  cross-checked against a verified source in this round; 2.12.5 is the latest
  2.x resolved by cargo at generation time, so it is current by construction.
- cargo check (full default features) NOT PASSED on this machine: the
  gdk-sys build script fails because system library gdk-3.0 is not found
  (pkg-config: Package gdk-3.0 was not found). Two legitimate attempts to
  install the Tauri Linux prerequisites failed: (1) apt-get update hung over
  10 minutes on this VM with no progress; (2) apt-get install from cached
  lists fails because libwebkit2gtk-4.1-dev, libappindicator3-dev,
  librsvg2-dev and patchelf are not in the cached lists. No system package
  was installed.
- Code is still a STUB (main.rs does not wire the Tauri Builder); only the
  dependency pin + lockfile changed. Merge still needs cargo check evidence
  from a machine with GTK/WebKit (Leader VM fallback per message 4086).
  Independent verification: NOT DONE by design; the Leader verifies.

### 8.11 Fill-in round per messages 4128/4138 (03/11/2026, evening)

- Scope: real SDK import + createAgentSession wiring on top of merged
  scaffold (dev/scaffold head a868774, PR #2 merged). No push, no merge,
  no PR action by the seat; delivery goes through the Leader as before.
- API findings from pi-coding-agent 1.3.0 (MIT), read from the installed
  package types: createAgentSession(options?) takes all-optional options,
  BUT its defaults resolve a model from the user's ~/.pi/agent settings and
  auth files, and ModelRuntime.create() would read auth the same way. The
  wrapper therefore REFUSES to call the SDK without an explicitly provided
  model runtime: no default model per message 4070, no key, no network call,
  no live session in this round. AgentSession methods steer / followUp /
  abort / subscribe map exactly to the Tauri commands sen_session_steer,
  sen_session_follow_up, sen_session_abort, sen_events_subscribe.
- Changes: 2 new files (packages/core/src/agent-session.ts,
  packages/core/tests/sdk-wiring.test.ts), 5 modified files (session.ts,
  bridge.ts comment, root package.json, docs/b2-checklist.md, README.md).
  Zero Rust changes: the Rust code is byte-identical to the VM-green head
  9e5ef82, so the VM check result for that head still applies to it.
- Design decisions: core index.ts does NOT re-export agent-session.ts, so
  the install-free smoke import keeps working; SDK wiring loads by direct
  import after pnpm install. Session start default is noTools "all": tools
  attach later explicitly through the execution gate, never at creation.
- Verification run in this round (all real runs, no fabricated result):
  pnpm install --frozen-lockfile in a scratch copy (8.3s first, 1.7s
  re-check on the final bundle); sdk-wiring tests 4 pass 0 fail (real SDK
  import resolves, no-runtime guard throws, method mapping delegates,
  SessionManager binding keeps the statechart); logic tests 6 pass 0 fail
  in the scratch copy AND in the bundle dir without node_modules;
  install-free smoke import of core index, providers, tools, prompts, ui,
  bridge, main all clean; cargo metadata --locked exit 0 (tauri 2.12.5,
  407 nodes including the root package); scans clean (no long dash, no
  credential value in new/changed files).
- Still blocked / still open, stated plainly: full cargo check on this box
  was NOT re-run (Rust unchanged; the known gdk-sys / missing system lib
  block stands) - the new branch still needs cargo check --locked green on
  the Leader VM per message 4138 acceptance. Rust command handlers
  (session.rs, tools.rs, events.rs, main.rs) are still stubs and are the
  next wiring step after this TS-side round. The frontend Tauri runtime
  binding (@tauri-apps/api plus the app bundler) is still missing, so
  bridge.invoke() still throws NOT_IMPLEMENTED. No live session exists by
  design: no model channel attached yet per message 4070. Name unification
  at 10/11 is still open and does not block this round.
- Independent verification: NOT DONE by design; the seat verifies before
  any group handover, and the Leader verifies before any PR.

### 8.12 Pi bump round per message 4178 (03/11/2026, night, Zen order)
- Order: drop the pi-coding-agent 1.3.0 pin; Sen follows newest upstream Pi
  like the CLI line (currently 1.5.0). Separate PR into dev/scaffold,
  SD Leader review, no self-merge. If createAgentSession changes signature,
  report at once, do not force-fit.
- Verified from npm registry: newest @earendil-works/pi-coding-agent is
  1.5.0, license MIT. packages/core/package.json now pins pi-coding-agent
  1.5.0; pnpm-lock.yaml regenerated (install "Done in 16.6s", specifier
  1.5.0, resolved 1.5.0).
- pi-agent-core: direct pin UNCHANGED at 1.3.0 (tin 4178 names only
  pi-coding-agent; no order to move core). Registry newest for core is
  also 1.5.0, reported for a later Leader decision. Note: pi-coding-agent
  1.5.0 pulls pi-agent-core 1.5.0 as its own transitive dependency, so
  the lockfile lists both core 1.3.0 (direct) and 1.5.0 (transitive); the
  direct import in packages/core still resolves to core 1.3.0.
- Signature verdict: NO CHANGE affecting the wrapper. Checked against the
  installed 1.5.0 type declarations: createAgentSession still exported
  from pi-coding-agent, still takes CreateAgentSessionOptions (cwd?,
  modelRuntime?, noTools "all" | "builtin") and still returns
  { session: AgentSession }; AgentSession still has steer, followUp,
  abort, subscribe, dispose; ModelRuntime and AgentSessionEventListener
  still exported. The wrapper call (modelRuntime, cwd, noTools "all")
  and the no-default-model guard are untouched.
- Files changed vs merged PR #3 state (7 modified, 0 added, 0 deleted):
  packages/core/package.json, pnpm-lock.yaml,
  packages/core/src/agent-session.ts (SDK_PINNED_VERSION 1.5.0),
  packages/core/src/session.ts (version comment),
  packages/core/tests/sdk-wiring.test.ts (pin assertions 1.5.0),
  README.md (status + pin lines), build-notes.md (this section).
  docs/b2-checklist.md left untouched as the 1.3.0 fill-in round record.
- Evidence: logic tests 6 pass 0 fail installed AND install-free;
  sdk-wiring tests 4 pass 0 fail after install (real 1.5.0 import
  resolves, probe reports 1.5.0, no-runtime guard throws, method mapping
  delegates, SessionManager binding keeps the statechart); cargo metadata
  --locked exit 0 (tauri 2.12.5, 407 packages).
- Still open, stated plainly: full cargo check on this box NOT re-run
  (Rust code unchanged; known gdk-sys / missing system lib block stands)
  - the new branch still needs cargo check --locked green on the Leader
  VM. Rust handlers still stubs, bridge still NOT_IMPLEMENTED, no live
  session by design, name decision 10/11 still open. No PR opened from
  here; the Leader pushes and opens the separate PR.
