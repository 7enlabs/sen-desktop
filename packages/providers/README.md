# @sen/providers (PROPOSED scope, see note below)

Pi version lock and model channel declarations for Sen Desktop.

## Role

- Holds the single locked Pi version: `PINNED_PI_VERSION = "1.5.0"`.
- Declares the ModelRuntime shape and custom provider channels.
- Default model: NONE, per decision message 4070. This package is only a
  channel-declaration shell; no model is attached, no key is stored, no
  network call is made.

## Status

STUB. ModelRuntime is not filled in. No SDK import here: only
packages/core may import the SDK.

## Model router (Sen P0-3)

`src/router.ts` is real, tested logic (see `docs/model-router.md`):
the router picks a (provider, model, base URL) route for each agent
chat turn from the free-only chain spark → openrouter-free → ollama,
with BYOK only on explicit user approval and a paid route never
usable. It holds no key and makes no network call; the model call
itself still runs through the CLI bridge. The Rust mirror lives in
`apps/desktop/src-tauri/src/commands/model_router.rs`.

## Package scope note

The `@sen/providers` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`providers` before merge. Do not publish under the scope until
confirmation exists.
