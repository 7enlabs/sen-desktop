// Frontend client for the Rust SkillRegistry (Tauri follow-up 3,
// ZC Leader tin 17093): two calls, through the bridge like every
// other client, to sen_skills_list and sen_skill_match. The backend
// scans the skills dir on the Rust side and returns skill METADATA
// only — name, description, triggers, allowed tools, dir, path.
// The skill BODY never crosses IPC: the frontend matches on this
// metadata, and a skill body reaches the model only through the
// CLI core, which reads the same SKILL.md layout itself. A backend
// failure surfaces here untouched — never mapped to a fake skill.
import { invoke } from "./bridge.ts";
import type { ActionContract } from "./types.ts";

/** One skill's metadata, as the Rust SkillMeta serializes it. */
export interface SkillMeta {
  name: string;
  description: string;
  /** Trigger words, lowercased, deduped, in file order. */
  triggers: string[];
  /** Tool names this skill may use (empty means unrestricted here). */
  allowedTools: string[];
  /** Directory holding SKILL.md. */
  dir: string;
  /** Absolute path of the SKILL.md file. */
  path: string;
}

export interface SkillScanReport {
  loaded: SkillMeta[];
  failed: { path: string; errors: string[] }[];
}

function asSkillMeta(value: unknown): SkillMeta {
  const res = value as SkillMeta | null;
  if (
    !res ||
    typeof res.name !== "string" ||
    !res.name ||
    typeof res.description !== "string" ||
    !Array.isArray(res.triggers) ||
    res.triggers.some((t) => typeof t !== "string") ||
    !Array.isArray(res.allowedTools) ||
    res.allowedTools.some((t) => typeof t !== "string") ||
    typeof res.dir !== "string" ||
    typeof res.path !== "string"
  ) {
    throw new Error(
      "Backend skill metadata malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return res;
}

function asScanReport(value: unknown): SkillScanReport {
  const res = value as SkillScanReport | null;
  if (
    !res ||
    !Array.isArray(res.loaded) ||
    !Array.isArray(res.failed) ||
    res.failed.some(
      (f) =>
        !f ||
        typeof f.path !== "string" ||
        !Array.isArray(f.errors) ||
        f.errors.some((e) => typeof e !== "string"),
    )
  ) {
    throw new Error(
      "Backend skill scan report malformed, FAIL: " + JSON.stringify(value),
    );
  }
  return { loaded: res.loaded.map(asSkillMeta), failed: res.failed };
}

/** Scan the skills root and return the loaded metadata + failures. */
export async function listSkills(
  contract: ActionContract,
): Promise<SkillScanReport> {
  const raw = await invoke("sen_skills_list", { contract });
  return asScanReport(raw);
}

/** Return the best skill metadata for the query, or null when nothing matches. */
export async function matchSkill(
  contract: ActionContract,
  query: string,
): Promise<SkillMeta | null> {
  const raw = await invoke("sen_skill_match", { contract, query });
  if (raw === null) {
    return null;
  }
  return asSkillMeta(raw);
}
