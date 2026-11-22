// Node-side SkillRegistry factory (CI P2-9).
//
// Builds the matcher ChatSession takes as its `skillMatcher` option:
// a real SkillRegistry scanned from a real skills root on disk
// (the repo ships skills/morning-brief/SKILL.md).
//
// NOT imported by apps/desktop/src/app/main.ts, or by any module the
// webview entry reaches: SkillRegistry reads with node:fs, and the
// webview has no fs — importing this module there breaks vite build
// (probe 19/11: exit 1, "Module node:fs has been externalized for
// browser compatibility"). Tests import it directly. Production
// uses the Rust SkillRegistry instead (Tauri follow-up 3, 19/11:
// sen_skill_match, apps/desktop/src/ipc/skills-client.ts), which
// returns the same match on metadata only; this factory remains
// for node-side use and as the reference implementation the Rust
// port mirrors.
import {
  SkillRegistry,
  type Skill,
} from "../../../../packages/core/src/skill-registry.ts";

/** Scan `skillsRoot` once and return a matcher for chat requests. */
export function createSkillMatcher(
  skillsRoot: string,
): (query: string) => Skill | undefined {
  const registry = new SkillRegistry();
  registry.scan(skillsRoot);
  return (query: string) => registry.matchSkill(query);
}
