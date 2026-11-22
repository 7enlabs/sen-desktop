# Sen P0-3 — Model router (BYOK + free/local routes)

Status: MERGED into dev/scaffold via PR #70 (Sen P0-3 rev2), latest
update PR #71 (Sen P0-2 rev1 evidence touch), head 780d257. Verified
ĐẠT by SD Leader on 16/11/2026.

## OBSERVABLE_SUCCESS

- A desktop or Telegram chat turn runs on the router's active route
  and the UI status line shows `Model: <provider> · <model> · 0 đồng`
  (or `key của bạn (BYOK)` for a BYOK route).
- `/model` lists the chain with costs; `/model ollama` switches the
  preferred route for new sessions.
- A turn that fails with a retryable code (auth / rate-limit /
  no-result / timeout / spawn-failed / not-found / conflict) runs
  again on the next route in the chain and succeeds there; a
  non-retryable failure (validation / forbidden / abort / empty
  input) returns at once, never retried.
- No paid route is ever usable: validation rejects `paid: true`, and
  selection skips paid routes defensively. A BYOK route is usable
  only with explicit user approval AND its named key env var present.
- No key value is stored, logged, or sent anywhere: the settings
  file carries only the env var NAME (`keyEnv`).

## Design

The model call itself still runs through the CLI bridge (`zen -p
--output-format stream-json ... --model <model> [--base-url <url>]`,
see `apps/desktop/src-tauri/src/commands/cli_spawn.rs`). A route is
only a (provider, model, base URL) triple handed to that bridge. No
new HTTP client, no new provider SDK, no key handling was added.

Default chain, preference order (`packages/providers/src/router.ts`,
Rust mirror `apps/desktop/src-tauri/src/commands/model_router.rs` —
the two must stay in sync: same chain, same order, same validation,
same retryable codes):

| Route | Provider | Model | Base URL | Cost |
| --- | --- | --- | --- | --- |
| `spark` | spark (Pi/Muse Spark = zen CLI default) | `deepseek-v4.1-flash` | (CLI default) | 0 đồng |
| `free` | Free models | `deepseek-v4.1-flash` | `https://api.experientiallabs.ai/api/v1` | 0 đồng |
| `ollama` | local Ollama | `llama3.1` (starting value: change to a model actually pulled) | `http://127.0.0.1:11434/v1` | 0 đồng |

BYOK is NOT in the default chain. A `byok` route needs
`byokApproved: true` (route-level or config-level) AND its `keyEnv`
env var set and non-empty. The backend checks only presence
(`key_env_present`); the value is never read into a kept string.

### Where the logic lives

- `packages/providers/src/router.ts` — pure TS: default chain,
  validation, attempt order, retryable codes. Exported from
  `packages/providers/src/index.ts`.
- `apps/desktop/src-tauri/src/commands/model_router.rs` — pure Rust
  mirror (same rules, same tests).
- `apps/desktop/src-tauri/src/commands/cli_settings.rs` —
  `CliSettings.router: Option<RouterConfig>` in `cli-settings.json`.
  Old files without `router` load as `None` (legacy single route:
  stored model/base_url as implicit `spark`). An invalid stored
  router loads as `None`, never fails a session. `active_route()`
  resolves the route for a new session.
- `apps/desktop/src-tauri/src/commands/registry.rs` — `create()`
  wires the bridge from the active route; `route_id()` reports the
  session's route; `fallback_bridge()` swaps the bridge to the next
  route. `steer()` (test-only) retries across routes.
- `apps/desktop/src-tauri/src/commands/session.rs` — production
  `run_steer_turn` retries across routes on retryable errors
  (same loop as `steer()`); `SessionResult` carries `routeId` +
  `cost` so the frontend shows what actually ran.
- `apps/desktop/src-tauri/src/commands/settings.rs` —
  `sen_settings_get_router` / `sen_settings_set_router` (partial
  update, validated before storing; paid/unapproved-BYOK refused).
- `apps/desktop/src-tauri/src/commands/cli_spawn.rs` —
  `CliBridge::has_started()` / `mark_session_started()`: a fallback
  bridge resumes the CLI session with `-r` when the failed bridge
  already ran a turn to a result (history kept); otherwise it starts
  a fresh CLI session and the turn runs on the prompt + memory block
  alone (known limitation, see below).
- Frontend: `apps/desktop/src/ipc/settings-client.ts` (router
  get/set, no key field), `apps/desktop/src/ipc/session-client.ts`
  (`routeId`/`cost`, optional for pre-P0-3 backends),
  `apps/desktop/src/chat/commands.ts` + `chat.ts` (`/model`,
  `/model <id>`, status line via `onRouterStatus` /
  `refreshRouterStatus()`), `apps/desktop/src/app/chat-ui.ts`
  (`setRouterStatus`), `apps/desktop/src/app/main.ts` (wiring).

### Fallback rules

- Retryable (provider problem, another route may succeed):
  `CLI_AUTH`, `CLI_RATE_LIMIT`, `CLI_NO_RESULT`,
  `CLI_TURN_TIMEOUT`, `CLI_SPAWN_FAILED`, `CLI_NOT_FOUND`,
  `CLI_CONFLICT`.
- Never retried: `CLI_VALIDATION`, `CLI_FORBIDDEN`,
  `CLI_TURN_ABORTED`, `CLI_EMPTY_TEXT`, `CLI_INVALID_RECORD`,
  `CLI_TRUNCATED`, `DUPLICATE_STEER`.
- At most one attempt per route, capped at 3 attempts per turn
  (`MAX_ATTEMPTS`); `fallbackEnabled: false` means one attempt.
- The in-flight idempotency key stays held across fallback
  attempts inside one steer, so a retry with the same key after a
  fully failed steer still executes (BUG-10, unchanged).
- BUG-06 kept: a steer that never runs to a result leaves no log
  entry — `fallback_bridge` pushes no log entry; the successful
  turn logs its own entry in `complete_steer`.
- P0-3 supersedes one pre-router test reading: `steer_failed_turn_
  does_not_record_key` assumed a retryable failure ends the steer.
  With fallback the same steer succeeds on the next route; the
  BUG-10 rule is now pinned by the R2B-02 error-turn test and by
  the new `steer_non_retryable_failure_does_not_record_key` test.
  See the test comments in `registry.rs`.

### Known limitations (NOT hidden)

1. Fallback after a first-turn failure (no result yet) starts a
   fresh CLI session: that turn has no CLI history, only the prompt
   + memory block. Fallback after a later turn resumes with `-r`
   and keeps history.
2. The Ollama default model (`llama3.1`) is a starting value: it
   only works when that model is actually pulled in Ollama. Change
   it with `/model` + a stored route, or via the router settings.
3. `zen --version` proves the CLI release line (0.1.0), not the
   exact source rev — same limit as the existing bridge gate.
4. No live model turn on a real provider was run here (see NOT RUN).

## Verification skill — model router

Feature map: entry `packages/providers/src/router.ts` +
`commands/model_router.rs` + `commands/registry.rs::steer` /
`commands/session.rs::run_steer_turn` | actions: list routes,
switch preferred route, run a turn, fail a turn on route 1 |
observable: status line text, `/model` reply, `routeId` in the
session result, retry on the next route, no retry on
non-retryable codes.

### 1. LAUNCH

- Worktree `~/workspace/sen-p03-model-router`, branch
  `feature/sen-model-router`, base HEAD `0f10901` (PR #69 merge).
- `git log -1` at run time: see Evidence.
- Build/run: `npm test` (node 24, type stripping, no build step);
  `npm run build --prefix apps/desktop` (vite); Rust pure modules
  via the scratch harness `/tmp/sen-rust-check` (see §4) because
  `cargo test` in `src-tauri` needs `gdk-3.0` (not installed).

### 2. DOCTOR

- Linux, node v24.22.1, rustc/cargo 1.99.1, no Rust system libs
  (gdk-3.0 missing — `cargo test` in src-tauri NOT RUN, see below).
- Fail early when: HEAD is not the P0-3 commit, `node_modules`
  missing (`npm install --prefix apps/desktop` first; the baseline
  run without it fails on `@tauri-apps/api`).

### 3. DRIVE

| # | Command | Result |
| --- | --- | --- |
| 1 | `npm test` (baseline, before changes) | 102 pass, 0 fail |
| 2 | `node --test apps/desktop/tests/model-router.test.ts` | 15 pass, 0 fail |
| 3 | `npm test` (after changes) | 117 pass, 0 fail |
| 4 | `npm run build --prefix apps/desktop` | vite build green |
| 5 | `cargo test --offline` in `/tmp/sen-rust-check` (pure modules) | 100 pass, 0 fail |
| 6 | `rustfmt --edition 2021 --check` on the 8 touched Rust files | clean |

### 4. EVIDENCE

- TS suite: 117 pass / 0 fail (102 baseline + 15 new in
  `apps/desktop/tests/model-router.test.ts`).
- Rust pure-module harness (`/tmp/sen-rust-check`, same module
  sources copied from the worktree, deps serde 1.0.229 /
  serde_json 1.0.154 / libc 0.2.191 from the offline cargo cache):
  100 pass / 0 fail, including 7 new `model_router` tests, 3 new
  `cli_settings` router tests, and 3 new + 2 updated `registry`
  fallback tests. The harness is scratch only (`/tmp`, not part of
  the bundle); the one fixture-path failure seen mid-run was a
  harness path artifact (the locator walks up to
  `apps/desktop/...`), fixed by mirroring the fixture path — the
  test itself passes in the repo layout.
- `git log -1` at delivery: base HEAD `0f10901` (PR #69 merge);
  the P0-3 change is uncommitted on branch `feature/sen-model-router`
  (producer bundle, no commit — see the bundle README).
- `git diff --check`: clean (checked at bundle time).

### 5. CLEANUP

- Kept: the worktree (branch `feature/sen-model-router`, uncommitted
  changes), the bundle files in `~/workspace`, the scratch Rust
  harness in `/tmp/sen-rust-check` (disposable; rebuildable from the
  worktree + Cargo.lock versions).
- Removed nothing from the repo. `apps/desktop/node_modules` and
  `apps/desktop/dist` are git-ignored build outputs, left in place.
- Evidence still readable after cleanup: this file + the bundle
  README + the test files themselves.

## NOT RUN (must run before merge)

- `cargo test` in `apps/desktop/src-tauri` (full crate incl. the
  Tauri wrappers `session.rs`/`settings.rs`): BLOCKED on this box
  — `gdk-3.0` system library missing. Run on the Leader VM.
- `cargo clippy --workspace --all-targets`: same blocker.
- Windows/macOS runs (box is Linux only).
- Live model turn on a real provider (free / Ollama /
  Spark): no live call was made; the fallback path is proven only
  against fake `zen` binaries in the Rust tests.
- Independent verification (producer ≠ verifier): this bundle is
  the producer's work; a verifier other than the producer checks it.

## Handoff

| Check | Verdict | Evidence |
| --- | --- | --- |
| `npm test` (TS, incl. 15 new) | PASS | 117 pass / 0 fail |
| `npm run build --prefix apps/desktop` | PASS | vite build green, 21.41 kB js |
| Rust pure modules (`cargo test`, scratch harness) | PASS | 100 pass / 0 fail |
| `rustfmt --check` (8 touched Rust files) | PASS | clean |
| `cargo test` in src-tauri (full crate) | NOT RUN | gdk-3.0 missing on this box |
| `cargo clippy` | NOT RUN | same blocker |
| Live provider turn | NOT RUN | no live call made (free-only rule) |
| Independent review | NOT RUN | producer bundle; seat verifies |

Risks left: the Tauri wrappers (`session.rs` fallback loop,
`settings.rs` router commands) compile only under the full crate
— a typo there would surface only on the Leader VM's `cargo test`.
The TS side of those commands is covered by mocked-transport tests,
and the shared loop logic is covered by the Rust `Registry::steer`
tests, but the wrapper code path itself is uncompiled here. The
fallback-history note (§ Known limitations 1) is a real behaviour
gap for first-turn failures, not a bug: the turn still succeeds,
just without CLI history.
