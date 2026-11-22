# P0-1 Rust harness: how to run it

Branch: `feature/sen-chat-telegram-bridge`. Base: dev/scaffold `4608106`.

## Where the harness lives

`~/workspace/sen-p01-harness` — a scratch crate, NOT part of the repo and
never committed. It compiles the pure-Rust modules from the worktree via
`#[path]` (see its `src/lib.rs`), so their unit tests run on a box without
the Tauri system libraries (`gdk-3.0` / `webkit2gtk` are missing here, so
`cargo test` inside the Tauri crate itself is NOT RUN, as in earlier
rounds). Same pattern as the `sen-r2b-*-harness` crates.

The modules it covers, all from
`apps/desktop/src-tauri/src/commands/` in the worktree:

- `memory_store.rs` — file memory, context block, daily log.
- `telegram_client.rs` — update parsing, Bot API client, poll loop,
  allowlist, durable offset, error redaction, UTF-16 chunking.
- `registry.rs`, `cli_spawn.rs`, `cli_bin.rs`, `cli_settings.rs`,
  `stream_json.rs` — the older pure modules, regression cover only.

The Tauri wrapper files (`memory.rs`, `telegram.rs`, `session.rs`,
`settings.rs`) are NOT in the harness: they need the `tauri` crate. Their
wire shapes are locked by the TS client tests (`npm test`) instead.

## How to run it

```sh
cd ~/workspace/sen-p01-harness
cargo test
```

No env vars, no token, no network: the Telegram tests use a fake HTTP
transport, the memory tests use temp dirs. `cargo` is at
`~/.cargo/bin/cargo` on this box (not on the default `PATH`).

## Expected result

`117 passed; 0 failed` (measured 16/11, after the rev2 review fixes).

Breakdown by module: `cli_bin` 6, `cli_settings` 9, `cli_spawn` 29,
`memory_store` 14, `registry` 30, `stream_json` 12, `telegram_client` 17.

Note on the count: the 109 the Leader ran was the pre-fix total. The rev2
fixes added 8 regression tests (7 in `telegram_client`, 1 in
`memory_store`), so the total is now 117. If the count you see is 109,
you are running against the pre-fix worktree state; if it is higher, new
tests were added since — check `cargo test -- --list` for the breakdown.

## Known flakes (old, not from this change)

2 tests in `cli_spawn` failed once each in an earlier round and passed on
re-run (timing-sensitive spawn tests). If a `cli_spawn` test fails, run
`cargo test` again before concluding anything is broken, and record
whether the failure repeats.

## What it does NOT prove

- `cargo check` / `cargo build` of the Tauri crate: NOT RUN (missing
  `gdk-3.0`, same as earlier rounds).
- Live Tauri app / live webview: NOT RUN.
- Live model turn: NOT RUN (no `zen` CLI, no API key on this box).
- Live Telegram bot: NOT RUN (no bot token on this box). Poll/send are
  proven with a fake transport here and mocked IPC in the TS tests.
- The offset file path wiring in `main.rs` / `telegram.rs` (Tauri state)
  is NOT covered by the harness — it is Tauri code. Review it by reading
  `main.rs` setup and `telegram.rs` `poller_or_fail`.

## If the harness is missing

It is scratch, not in the repo: if the directory is gone, the tests in
`memory_store.rs` and `telegram_client.rs` still live in the worktree
(they are `#[cfg(test)]` modules in the files themselves) and can be run
by recreating the harness from `PLAN.md` in the harness directory — or by
running `cargo test` in the Tauri crate on a box that has the system
libraries.
