// SKILL.md registry (Sen P0-5): a local registry of skills read
// from SKILL.md files on disk.
//
// Layout mirrors the ZenCore CLI core (crates/zen-cli/src/skills.rs
// + crates/zen-ext/src/skills.rs, ZenCore 8ac73fe): one skill per
// directory, `<skills-root>/<name>/SKILL.md`, frontmatter delimited
// by `---` lines with `name:` and `description:`, body after the
// closing fence. An unclosed frontmatter means the whole file is
// body, same as the CLI. Two Sen-local extras: `triggers:` (a list
// or comma string of words that select the skill for a request)
// and `allowed-tools:` (a list mirroring zen-core SkillInfo
// allowed_tools). The registry reads real files with node:fs; tests
// scan real temp dirs with real SKILL.md files, never a mocked
// read path.
//
// The registry selects and hands the skill BODY to the caller; it
// does NOT run the skill, does NOT call a model, and does NOT spawn
// an agent. Running a skill is the CLI core's job (its
// SubAgentExecutor reads the same SKILL.md layout and substitutes
// ${ZEN_SKILL_DIR} with the skill directory - this module does the
// same substitution in skillBody so a shared skill reads the same
// in both places).
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

/** A skill name: [a-zA-Z0-9_-]+, same rule as the CLI registry. */
export const SKILL_NAME_PATTERN = /^[a-zA-Z0-9_-]+$/;

/** One registered skill. */
export interface Skill {
  name: string;
  description: string;
  /** Trigger words, lowercased, deduped, in file order. */
  triggers: string[];
  /** Tool names this skill may use (empty means unrestricted here). */
  allowedTools: string[];
  /** Full body text after the frontmatter, ${ZEN_SKILL_DIR} kept raw. */
  body: string;
  /** Directory holding SKILL.md; body is read from <dir>/SKILL.md. */
  dir: string;
  /** Absolute path of the SKILL.md file. */
  path: string;
}

export interface SkillFile {
  name: string;
  description: string;
  triggers: string[];
  allowedTools: string[];
  body: string;
}

function splitFrontmatter(
  content: string,
): { frontmatter: string[] | null; body: string } {
  const lines = content.split("\n");
  if (lines[0]?.trim() !== "---") {
    return { frontmatter: null, body: content };
  }
  const fmLines: string[] = [];
  for (let i = 1; i < lines.length; i += 1) {
    const line = lines[i] as string;
    if (line.trim() === "---" || line.trim() === "...") {
      const body = lines.slice(i + 1).join("\n");
      return { frontmatter: fmLines, body };
    }
    fmLines.push(line);
  }
  // Unclosed frontmatter: the whole file is body (CLI behaviour).
  return { frontmatter: null, body: content };
}

function unquote(value: string): string {
  const trimmed = value.trim();
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function parseListValue(value: string): string[] {
  const trimmed = value.trim();
  if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
    return trimmed
      .slice(1, -1)
      .split(",")
      .map((item) => unquote(item))
      .map((item) => item.trim())
      .filter((item) => item !== "");
  }
  return trimmed
    .split(",")
    .map((item) => unquote(item))
    .map((item) => item.trim())
    .filter((item) => item !== "");
}

/**
 * Parse one SKILL.md file's text. The file name (its directory
 * name) is the fallback name when frontmatter has no name, so a
 * skill written for the CLI (name only in the directory) still
 * loads. Returns error strings; empty means valid. A file with
 * errors still yields a SkillFile with what could be read, so the
 * caller can report the file instead of silently dropping it.
 */
export function parseSkillFile(
  content: string,
  dirName: string,
): { skill: SkillFile; errors: string[] } {
  const { frontmatter, body } = splitFrontmatter(content);
  const errors: string[] = [];
  let name = dirName;
  let description = "";
  let triggers: string[] = [];
  let allowedTools: string[] = [];
  if (frontmatter) {
    let currentKey: string | null = null;
    let listItems: string[] = [];
    const flushList = () => {
      if (currentKey === "triggers") {
        triggers = listItems;
      } else if (currentKey === "allowed-tools") {
        allowedTools = listItems;
      }
      listItems = [];
    };
    for (const rawLine of frontmatter) {
      const line = rawLine;
      const listMatch = line.match(/^\s*-\s+(.*)$/);
      if (listMatch && currentKey) {
        listItems.push(unquote(listMatch[1] as string).trim());
        continue;
      }
      const keyMatch = line.match(/^\s*([A-Za-z0-9_-]+)\s*:\s*(.*)$/);
      if (!keyMatch) {
        continue;
      }
      flushList();
      const key = (keyMatch[1] as string).toLowerCase();
      const value = keyMatch[2] as string;
      currentKey = null;
      if (key === "name") {
        if (value.trim()) {
          name = unquote(value);
        }
      } else if (key === "description") {
        description = unquote(value);
      } else if (key === "triggers" || key === "allowed-tools") {
        currentKey = key;
        if (value.trim()) {
          listItems = parseListValue(value);
        }
      } else {
        // Unknown frontmatter keys are ignored: a skill written
        // for the CLI may carry keys Sen does not use.
        currentKey = null;
      }
    }
    flushList();
  }
  if (!name.trim()) {
    errors.push("skill name is empty");
  } else if (!SKILL_NAME_PATTERN.test(name)) {
    errors.push(`skill name ${JSON.stringify(name)} must be [a-zA-Z0-9_-]+`);
  }
  if (!description.trim()) {
    errors.push(`skill ${JSON.stringify(name)} has no description`);
  }
  if (!body.trim()) {
    errors.push(`skill ${JSON.stringify(name)} has an empty body`);
  }
  return {
    skill: {
      name,
      description,
      triggers: [...new Set(triggers.map((t) => t.toLowerCase()))],
      allowedTools,
      body,
    },
    errors,
  };
}

/** Validate a parsed skill (same checks parseSkillFile reports). */
export function validateSkill(skill: SkillFile): string[] {
  const errors: string[] = [];
  if (!skill.name.trim()) {
    errors.push("skill name is empty");
  } else if (!SKILL_NAME_PATTERN.test(skill.name)) {
    errors.push(
      `skill name ${JSON.stringify(skill.name)} must be [a-zA-Z0-9_-]+`,
    );
  }
  if (!skill.description.trim()) {
    errors.push(`skill ${JSON.stringify(skill.name)} has no description`);
  }
  if (!skill.body.trim()) {
    errors.push(`skill ${JSON.stringify(skill.name)} has an empty body`);
  }
  return errors;
}

/**
 * Body with ${ZEN_SKILL_DIR} replaced by the skill directory,
 * exactly like the CLI SubAgentExecutor does when it hands a skill
 * to a subagent. Callers that pass the body to the CLI core should
 * use this form so a shared skill reads the same in both places.
 */
export function skillBody(skill: Skill): string {
  return skill.body.replaceAll("${ZEN_SKILL_DIR}", skill.dir);
}

export interface ScanReport {
  /** Skills loaded, in directory order. */
  loaded: Skill[];
  /** Files that failed to load: path + why. */
  failed: { path: string; errors: string[] }[];
}

/**
 * The registry: scan a skills root for child SKILL.md files, keep the
 * loaded skills, answer get/list/match. A duplicate name fails
 * the scan for that file (the first file wins); a file with parse
 * errors fails for that file. Both are reported in the returned
 * report, never thrown, so one bad skill cannot hide the rest.
 */
export class SkillRegistry {
  private skills = new Map<string, Skill>();

  /** Skills in scan order. */
  list(): Skill[] {
    return [...this.skills.values()];
  }

  get(name: string): Skill | undefined {
    return this.skills.get(name);
  }

  get size(): number {
    return this.skills.size;
  }

  /** Load one SKILL.md file by path; throws on unreadable files. */
  loadFile(path: string, dirName?: string): { skill: Skill; errors: string[] } {
    const content = readFileSync(path, "utf8");
    const dir = path.slice(0, path.length - "/SKILL.md".length);
    const { skill, errors } = parseSkillFile(
      content,
      dirName ?? dir.split("/").pop() ?? "",
    );
    const full: Skill = { ...skill, dir, path };
    return { skill: full, errors };
  }

  /**
   * Scan a skills root: every direct child directory holding a
   * SKILL.md file. A missing root scans as empty (no skills, no
   * failure) so a fresh install without a skills dir still works;
   * a root that exists but is not a directory throws FAIL-loud.
   */
  scan(root: string): ScanReport {
    this.skills.clear();
    const loaded: Skill[] = [];
    const failed: { path: string; errors: string[] }[] = [];
    if (!existsSync(root)) {
      return { loaded, failed };
    }
    if (!statSync(root).isDirectory()) {
      throw new Error(
        `SKILL_REGISTRY_INVALID: skills root is not a directory: ${root}`,
      );
    }
    const children = readdirSync(root, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
    for (const child of children) {
      const path = join(root, child, "SKILL.md");
      if (!existsSync(path)) {
        continue;
      }
      let parsed: { skill: Skill; errors: string[] };
      try {
        parsed = this.loadFile(path, child);
      } catch (error) {
        failed.push({
          path,
          errors: [
            `cannot read SKILL.md: ${error instanceof Error ? error.message : String(error)}`,
          ],
        });
        continue;
      }
      const errors = [...parsed.errors];
      if (this.skills.has(parsed.skill.name)) {
        errors.push(
          `duplicate skill name ${JSON.stringify(parsed.skill.name)} (already loaded)`,
        );
      }
      if (errors.length > 0) {
        failed.push({ path, errors });
        continue;
      }
      this.skills.set(parsed.skill.name, parsed.skill);
      loaded.push(parsed.skill);
    }
    return { loaded, failed };
  }

  /**
   * Pick the best skill for a request, or undefined when none
   * matches. Scoring, highest first: a trigger word appearing in
   * the request (+10 each), the skill name appearing (+5), a word
   * of the description appearing (+1 each, capped at +5). A query
   * with no match scores 0 and matches nothing: an unrelated
   * request must not pull in a skill.
   */
  matchSkill(query: string): Skill | undefined {
    const text = query.toLowerCase();
    const words = text.split(/[^a-z0-9_àáảãạăắằẳẵặâấầẩẫậèéẻẽẹêếềểễệìíỉĩịòóỏõọôốồổỗộơớờởỡợùúủũụưứừửữựỳýỷỹỵđ-]+/).filter(Boolean);
    const wordSet = new Set(words);
    let best: Skill | undefined;
    let bestScore = 0;
    for (const skill of this.skills.values()) {
      let score = 0;
      for (const trigger of skill.triggers) {
        if (text.includes(trigger)) {
          score += 10;
        }
      }
      if (text.includes(skill.name.toLowerCase())) {
        score += 5;
      }
      const descWords = skill.description
        .toLowerCase()
        .split(/[^a-z0-9_àáảãạăắằẳẵặâấầẩẫậèéẻẽẹêếềểễệìíỉĩịòóỏõọôốồổỗộơớờởỡợùúủũụưứừửữựỳýỷỹỵđ-]+/)
        .filter(Boolean);
      let descHits = 0;
      for (const word of new Set(descWords)) {
        if (word.length > 2 && wordSet.has(word)) {
          descHits += 1;
        }
      }
      score += Math.min(descHits, 5);
      if (score > bestScore) {
        bestScore = score;
        best = skill;
      }
    }
    return bestScore > 0 ? best : undefined;
  }
}
