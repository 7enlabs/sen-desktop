# @sen/prompts (PROPOSED scope, see note below)

Prompt templates and context files for Sen Desktop.

## Role

- Serves prompt templates and context files through a ResourceLoader.
- No prompt content is hard-coded into other packages; everything flows
  through the loader interface.

## Status

STUB. The ResourceLoader interface exists; no real loader and no real
template content exist yet, and no fake content is returned.

## Package scope note

The `@sen/prompts` name is a PROPOSAL pending SD Leader confirmation at
review time. If the scope is not agreed, the fallback is the unscoped name
`prompts` before merge. Do not publish under the scope until confirmation
exists.
