# @sen/tools (PROPOSED scope, see note below)

Tool extensions and the permission layer for Sen Desktop.

## Role

- Hosts customTools and Sen extensions.
- Owns the tool_call event hook: every tool call must pass through it,
  judged by behavior, not by the shell syntax of one OS.
- The hook wires into the 6-step execution gate in packages/core when
  filled in.

## Status

FILLED IN (Sen P0-2): the hook evaluates the 3-tier approval gate
from packages/core (auto / ask-first / never, default-deny policy)
and records every evaluation in the activity log when one is passed
in. A refused call throws FAIL-loud (`APPROVAL_REQUIRED`,
`APPROVAL_NEVER_DENIED`, or `TOOL_POLICY_DENIED`); an allowed call
returns its evaluation — permission granted, not executed. The
backend registry is the enforcing side: its `tool_call` ends in
`TOOL_EXECUTION_NOT_WIRED` after the gate passes, because real tool
execution is a later phase. No narrow Mika mode is applied, no mode
switch mid-run, no disabling of the protection layer.

## Package scope note

The `@sen/tools` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`tools` before merge. Do not publish under the scope until confirmation
exists.
