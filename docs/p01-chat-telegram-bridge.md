# P0-1: desktop chat + Telegram bridge + file memory Phase 1

Branch: `feature/sen-chat-telegram-bridge`. Base: dev/scaffold `4608106`
(full `4608106e33c48b54f30e2b194749018d1d9c25b5`).

## Why this base (decided, with evidence)

Base is dev/scaffold, NOT main. Main `3c4e845` holds only 5 docs/config
files and no app code (no package.json, no apps/, no packages/). The app
code lives only on dev/scaffold. A branch from main would have nothing to
build on.

## What lands

- Desktop chat: a real chat UI (vanilla TS, no framework) in the Tauri
  webview. One session, started by `startApp`, shared by every channel.
- Telegram bridge: the chat layer drives poll rounds by calling
  `sen_telegram_poll_once` again after each round (1s backoff after an
  error). The Rust backend holds the poller (Bot API client + offset)
  in managed state and returns the new allowed text updates. The bot
  token lives only in the Rust backend (env `SEN_TELEGRAM_BOT_TOKEN`);
  it never crosses IPC into the webview, the same rule as `ZEN_API_KEY`.
  There is no background thread in the backend and no start/stop
  command: stopping is just not calling again, so no stop flag can
  race a 30s long-poll round. The poll offset is durable: it is written
  to `sen-memory/telegram-offset` before the in-memory offset moves, so
  a restart resumes instead of replaying up to 100 already-handled
  updates; an update id is also never emitted twice in one process.
  The bridge is fail closed: with neither `SEN_TELEGRAM_ALLOWED_CHAT_ID`
  nor the explicit `SEN_TELEGRAM_ALLOW_ANY_CHAT=1` opt-in set, no chat
  is served.
- One shared agent context: one CLI session id (the bridge resumes it with
  `-r`, so the model sees the full history), one transcript (both channels
  render from it), one memory store.
- File memory Phase 1, under the app data dir (`sen-memory/`):
  `MEMORY.md` (durable facts), `USER.md` (facts about the user), `SOUL.md`
  (persona text), `daily/YYYY-MM-DD.md` (event log, append-only).
- The assistant has its own name: Sen. Every turn prompt starts with a
  memory context block that names Sen and carries the memory files.
- The `quên` command deletes one memory item (by number or by text).

## How a turn runs

1. The user types in the desktop chat, or a Telegram message arrives.
2. The TS chat layer (`src/chat/chat.ts`) appends the user entry to the
   shared transcript and sends it through the IPC bridge
   (`sen_session_steer` first turn, `sen_session_follow_up` after).
3. The Rust backend (`session.rs`) prepends the memory context block to
   the prompt and runs the CLI turn on the session bridge (unchanged
   stream-json v1 contract: additive only, nothing in it is modified).
4. The model result flows back as `sen-session-event`s; the chat layer
   appends the assistant entry to the transcript and, when the turn came
   from Telegram, sends the reply to that Telegram chat.
5. User and assistant lines are appended to today's daily log.

Memory commands (`/nhớ`, `/tôi`, `/quên`, `/sửa`, `/xem`, `/nhật ký`)
never reach the model: the chat layer executes them locally against the
memory commands and replies with a confirmation in the transcript.

## Memory context block (prepended to every turn)

```
[Sen memory - YYYY-MM-DD]
Bạn là Sen, trợ lý cá nhân. Trả lời bằng tiếng Việt, trừ khi người dùng
viết bằng ngôn ngữ khác.
Tính cách (SOUL.md):
<SOUL.md content, or nothing when empty>
Về người dùng (USER.md):
- <fact>
...
Dữ kiện bền (MEMORY.md):
- <fact>
...
Hôm nay:
- HH:MM:SS [desktop] user: ...
- HH:MM:SS [telegram] Sen: ...
---
<user text>
```

Tradeoff, stated openly: the block is prepended to EVERY turn instead of
only when memory changes. That is always correct (the model never misses
an edit, including a `quên`), at the cost of repeating a small block in the
CLI history. Only the daily part is bounded (the last 10 lines): SOUL.md,
USER.md and MEMORY.md go in in full, so the block grows with those files.
A hash-versioned injection can replace this later if memory grows.

## Files

Rust (pure, unit-tested in the scratch harness — see
`docs/p01-harness.md` for where it lives and how to run it):
- `commands/memory_store.rs` — file layout, item ops, context block, log.
  `log()` flattens multi-line text to one line, so a pasted message can
  never inject fake lines into the daily log or the context block.
- `commands/telegram_client.rs` — update parsing, reqwest blocking client,
  poll loop, fail-closed chat allowlist, durable poll offset
  (`FileOffsetStore`, one number in `sen-memory/telegram-offset`) plus
  in-process dedupe by update id. Error strings are redacted (the token
  is replaced with `***`) before they leave the client, because reqwest's
  error Display embeds the request URL, token included. Replies are split
  by UTF-16 code units (4096), the unit Telegram counts — not Rust chars.

Rust (thin Tauri wrappers, same pattern as session.rs / settings.rs):
- `commands/memory.rs` — `sen_memory_read/add/forget/edit/set_soul/log`.
- `commands/telegram.rs` — `sen_telegram_status/poll_once/send`.
  Polling is TS-driven (one `poll_once` per round); there is no
  start/stop command and no background thread in the backend.
- `commands/session.rs` — `run_steer_turn` prepends the memory prompt.
- `commands/mod.rs`, `main.rs` — registration + setup (memory dir from the
  app data dir; Telegram state).
- `Cargo.toml` — `reqwest 0.12` (`blocking`, `json`, `rustls-tls`). Version
  0.12, not 0.13: 0.13 renamed `rustls-tls` to `rustls` and needs
  `aws-lc-rs`, which is not in the local cargo cache; 0.12 uses ring,
  which is cached.

TypeScript:
- `src/ipc/memory-client.ts` — IPC wrappers for the memory commands.
- `src/chat/commands.ts` — chat command parser.
- `src/chat/chat.ts` — shared transcript, send queue, command execution,
  Telegram reply routing.
- `src/chat/telegram.ts` — Telegram IPC wrappers + poll-loop driver
  (one `poll_once` per round, 1s backoff after an error).
- `src/app/chat-ui.ts` — DOM chat UI.
- `src/app/main.ts` — boot: startApp, bind the chat session, start the UI,
  start the Telegram poll when configured.
- `index.html` — chat shell (no stub text, no fake content).

## Wire shapes (Tauri commands)

All commands validate the full IPC action contract first, like the
session/settings commands. Multi-arg commands nest their own args under
`args` (the R2B-05 rev2 lesson: a flat payload is rejected with
"missing required key args").

- `sen_memory_read {contract}` ->
  `{memory: string[], user: string[], soul: string, today: string[]}`
- `sen_memory_add {contract, args: {scope: "memory"|"user", text}}` ->
  `{scope, index, item}`
- `sen_memory_forget {contract, args: {scope, target: {number} |
  {text}}}` -> `{scope, removed: string}`. A text target must match
  exactly one item; otherwise the command fails and reports the match count.
- `sen_memory_edit {contract, args: {scope, index, text}}` ->
  `{scope, index, item}`
- `sen_memory_set_soul {contract, args: {text}}` -> `{soul}`
- `sen_memory_log {contract, args: {kind: "user"|"assistant"|"memory",
  channel: "desktop"|"telegram"|"local", text}}` -> `{logged: true}`
- `sen_telegram_status {contract}` ->
  `{configured: boolean, allowedChatIdSet: boolean, allowAnyChat: boolean}`
- `sen_telegram_poll_once {contract}` ->
  `{updates: [{updateId, chatId, text, fromUsername}]}`
- `sen_telegram_send {contract, args: {chatId: number, text}}` ->
  `{sent: true}`

## Configuration (operator, runtime only, never in the repo)

- `SEN_TELEGRAM_BOT_TOKEN` — the bot token. Without it the Telegram
  bridge reports not-configured and the desktop chat still works.
- `SEN_TELEGRAM_ALLOWED_CHAT_ID` — the one Telegram chat that may use
  the assistant. When it is unset AND `SEN_TELEGRAM_ALLOW_ANY_CHAT` is
  not set to `1`/`true`, the bridge serves NOBODY (fail closed): a bot
  whose operator never set an allowlist cannot be found and driven by a
  stranger. Set the allowlist in production.
- `SEN_TELEGRAM_ALLOW_ANY_CHAT` — set to `1` or `true` only as a deliberate
  operator opt-in to serve any chat that finds the bot. This is the
  recorded acceptance the fail-closed default requires; anything else
  keeps the bridge closed to every chat.
- Poll offset file — `sen-memory/telegram-offset` under the app data dir,
  one number, written before the in-memory offset moves. Deleting it
  resets the poll to offset 0 (the next start re-fetches up to 100 recent
  updates; the in-process dedupe does not cover a restart).
- Model: the CLI's free default (`google/gemini-4-argon:free`) unless the
  operator stored another model with `sen_settings_set_model`. No paid
  model is configured anywhere in this change.

## Not verified on this box (honest list)

- Live Tauri app / live webview: no Tauri system libraries here.
- Live model turn: no `zen` CLI and no API key on this box. Turns are
  proven with the fake-CLI pattern the bridge round established.
- Live Telegram bot: no bot token on this box. Poll/send are proven with a
  fake HTTP transport in Rust tests and mocked IPC in TS tests.
- `cargo check` / `cargo build` of the Tauri crate: NOT RUN (gdk-sys, same
  as earlier rounds). Pure modules are proven in the scratch harness;
  wrapper wire shapes are locked by the TS client tests.
