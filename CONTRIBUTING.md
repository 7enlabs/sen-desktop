# Contributing to Sen Desktop

## Language

English only on GitHub: code comments, commit messages, PR descriptions,
and docs in this repo are written in English.

## Workflow

- Work on a branch; never push straight to a shared branch and never
  self-merge. Changes go through a pull request and a review.
- The reviewer verifies the change independently; the author never verifies
  their own work.
- Every PR description lists its claims per file path, so the reviewer can
  cross-check with `git diff --stat`.

## Rules that already bind this scaffold

- Pi SDK stays pinned at 1.3.0. Do not raise it without a new decision.
- No credentials, keys, or tokens in code, docs, or messages.
- No paid-tier fallback: if no verified free path exists, stop and report.
- No fake content: stubs stay marked STUB, unmet conditions stay listed in
  `build-notes.md`, and test results are only reported when actually run.
- The `@sen/*` package scope is a proposal pending confirmation; do not
  publish under it until confirmation exists.
- Names (window title, productName, identifier) are temporary pre-release
  names until the 10/11 name decision.

## Tests

Core logic lives in `packages/core/src` and its tests in
`packages/core/tests/logic.test.ts`. Run them with:

```sh
node --test packages/core/tests/logic.test.ts
```

Add a test case for every new pure-logic rule, and keep the stub parts
out of the test scope until they are filled in.
