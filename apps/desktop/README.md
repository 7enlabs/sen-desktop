# @sen/desktop (PROPOSED scope, see note below)

Tauri desktop app shell for Sen Desktop.

## Role

- Hosts the main window (1200x800) and the UI shell entry point
  (`src/app/main.ts`, `src/app/index.html`).
- Owns the IPC bridge (`src/ipc/bridge.ts`): the only layer the frontend
  may call. Every call carries the action contract from point 888
  (`src/ipc/types.ts`).
- Owns the Rust side (`src-tauri/`): command registration for
  sen_session_create, sen_session_steer, sen_session_follow_up,
  sen_session_abort, sen_tool_call, sen_events_subscribe.
- Holds no shared business logic; session logic lives in packages/core.

## Status

STUB. The UI shell is not filled in, the Tauri Builder is not wired, and
the real invoke call is not implemented. Tauri version 2 is decided per
Telegram message 4070; the stub status is about missing implementation,
not about an undecided version.

## Names: temporary until the 10/11 decision

- Window title: Pi Desktop (temporary).
- productName: Sen Desktop (temporary pre-release name).
- Identifier: dev.7enlab.sen-desktop (temporary, changeable before
  release).

Their unification is NOT decided here and waits for the 10/11 decision.

## Package scope note

The `@sen/desktop` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`desktop` before merge. Do not publish under the scope until confirmation
exists.
