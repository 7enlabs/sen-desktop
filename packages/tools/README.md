# @sen/tools (PROPOSED scope, see note below)

Tool extensions and the permission layer for Sen Desktop.

## Role

- Hosts customTools and Sen extensions.
- Owns the tool_call event hook: every tool call must pass through it,
  judged by behavior, not by the shell syntax of one OS.
- The hook wires into the 6-step execution gate in packages/core when
  filled in.

## Status

STUB. The hook frame and the minimal contract check exist; the real
permission logic is not filled in, and no tool runs for real through it
yet. No narrow Mika mode is applied, no mode switch mid-run, no disabling
of the protection layer.

## Package scope note

The `@sen/tools` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`tools` before merge. Do not publish under the scope until confirmation
exists.
