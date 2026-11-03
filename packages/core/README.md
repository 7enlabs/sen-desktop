# @sen/core (PROPOSED scope, see note below)

Core session and execution logic for Sen Desktop. Only this package may
import the Pi SDK.

## Role

- `session.ts`: SessionManager and the point-854 session statechart.
- `raw-log.ts`: raw event log, the source of truth; derived summaries
  must be rebuildable from it.
- `execution-gate.ts`: the single 6-step execution gate in fixed order
  (protect, lock check, irreversible-and-consent check, execute,
  post-check, record).
- `token-economizer.ts`: the point-856 stop-turn rule (at most 1 redirect,
  long-turn ratio 2.5).
- `paths.ts`: the single path variable; no path joining by string.
- Pi SDK pinned at 1.3.0 (`@earendil-works/pi-agent-core` and
  `@earendil-works/pi-coding-agent`), MIT license on both.

## Status

- Real logic: statechart table, gate order + order check, stop-turn judge
  function, path variable. Covered by `tests/logic.test.ts` (6 tests,
  run green with node).
- Still stubs: the real createAgentSession import, the real gate steps,
  the real detection function. Verified in the install round:
  createAgentSession is exported by pi-coding-agent, not by pi-agent-core;
  the fill-in round must import it from pi-coding-agent.

## Tests

```sh
node --test packages/core/tests/logic.test.ts
```

## Package scope note

The `@sen/core` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`core` before merge. Do not publish under the scope until confirmation
exists.
