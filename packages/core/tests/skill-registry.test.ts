// SKILL.md registry tests (Sen P0-5).
// Run with: node --test packages/core/tests/skill-registry.test.ts
// (Node 24 runs .ts files directly through type stripping; no build step.)
// The registry reads real files: tests create real temp dirs with
// real SKILL.md files, and one test scans the real skills dir shipped
// in this repo (skills/morning-brief/SKILL.md). No read path is mocked.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  SkillRegistry,
  parseSkillFile,
  skillBody,
  validateSkill,
} from "../src/skill-registry.ts";

function tempRoot(): string {
  return mkdtempSync(join(tmpdir(), "sen-skill-test-"));
}

function writeSkill(
  root: string,
  dir: string,
  content: string,
): void {
  mkdirSync(join(root, dir), { recursive: true });
  writeFileSync(join(root, dir, "SKILL.md"), content, "utf8");
}

// --- Frontmatter parsing ---

test("parse: frontmatter name, description, triggers and body", () => {
  const { skill, errors } = parseSkillFile(
    `---\nname: brief\ndescription: Build the morning brief.\ntriggers:\n  - morning brief\n  - bản tin sáng\nallowed-tools: [read_memory, draft_email]\n---\n\n# Body\n\nDo the brief.\n`,
    "brief",
  );
  assert.deepEqual(errors, []);
  assert.equal(skill.name, "brief");
  assert.equal(skill.description, "Build the morning brief.");
  assert.deepEqual(skill.triggers, ["morning brief", "bản tin sáng"]);
  assert.deepEqual(skill.allowedTools, ["read_memory", "draft_email"]);
  assert.equal(skill.body, "\n# Body\n\nDo the brief.\n");
});

test("parse: inline trigger list and quoted values", () => {
  const { skill, errors } = parseSkillFile(
    `---\nname: "quoted-name"\ndescription: 'Quoted desc.'\ntriggers: [a, b, c]\n---\nBody.\n`,
    "fallback",
  );
  assert.deepEqual(errors, []);
  assert.equal(skill.name, "quoted-name");
  assert.deepEqual(skill.triggers, ["a", "b", "c"]);
});

test("parse: no frontmatter falls back to the directory name", () => {
  const { skill, errors } = parseSkillFile("# Just a body.\n", "plain");
  assert.equal(skill.name, "plain");
  assert.equal(skill.body, "# Just a body.\n");
  assert.ok(errors.some((e) => e.includes("no description")));
});

test("parse: unclosed frontmatter is body, like the CLI", () => {
  const { skill } = parseSkillFile(
    "---\nname: lost\n\nNo closing fence.\n",
    "lost",
  );
  assert.equal(skill.name, "lost");
  assert.ok(skill.body.startsWith("---\nname: lost"));
});

test("parse: a bad name and an empty body are reported", () => {
  const { errors } = parseSkillFile(
    "---\nname: bad name!\ndescription: x\n---\n",
    "bad name!",
  );
  assert.ok(errors.some((e) => e.includes("must be [a-zA-Z0-9_-]+")));
  assert.ok(errors.some((e) => e.includes("empty body")));
  assert.deepEqual(validateSkill({
    name: "ok",
    description: "d",
    triggers: [],
    allowedTools: [],
    body: "b",
  }), []);
});

// --- Scanning real dirs ---

test("scan: real SKILL.md files load; bad and duplicate files fail", () => {
  const root = tempRoot();
  try {
    writeSkill(root, "brief", `---\nname: brief\ndescription: The brief.\ntriggers: [morning brief]\n---\nBody brief.\n`);
    writeSkill(root, "other", `---\nname: other\ndescription: Other skill.\n---\nBody other.\n`);
    writeSkill(root, "bad", `---\nname: bad name!\ndescription: Bad.\n---\nBody bad.\n`);
    writeSkill(root, "dup", `---\nname: brief\ndescription: Duplicate name.\n---\nBody dup.\n`);
    mkdirSync(join(root, "empty-dir"));
    const registry = new SkillRegistry();
    const report = registry.scan(root);
    assert.deepEqual(
      report.loaded.map((s) => s.name),
      ["brief", "other"],
    );
    assert.equal(registry.size, 2);
    assert.equal(registry.get("brief")?.description, "The brief.");
    assert.equal(report.failed.length, 2);
    assert.ok(report.failed.some((f) => f.path.replace(/\\/g, "/").endsWith("bad/SKILL.md")));
    assert.ok(
      report.failed.some((f) =>
        f.errors.some((e) => e.includes("duplicate skill name")),
      ),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("scan: a missing root is empty, a non-directory root throws", () => {
  const registry = new SkillRegistry();
  assert.deepEqual(registry.scan(join(tempRoot(), "no-such-dir")), {
    loaded: [],
    failed: [],
  });
  const file = join(tempRoot(), "not-a-dir");
  writeFileSync(file, "x", "utf8");
  assert.throws(
    () => registry.scan(file),
    /SKILL_REGISTRY_INVALID: skills root is not a directory/,
  );
  rmSync(file, { force: true });
});

test("scan: the shipped morning-brief skill loads from the repo", () => {
  const skillsDir = fileURLToPath(new URL("../../../skills", import.meta.url));
  const registry = new SkillRegistry();
  const report = registry.scan(skillsDir);
  assert.deepEqual(report.failed, []);
  const skill = registry.get("morning-brief");
  assert.ok(skill);
  assert.equal(
    skill?.description,
    "Xây bản tin sáng từ email và lịch, rồi đề xuất quyết định chờ duyệt.",
  );
  assert.ok(skill?.triggers.includes("bản tin sáng"));
  assert.ok(skill?.body.includes("buildMorningBrief"));
});

// --- Matching and body substitution ---

test("match: triggers win, the name matches, unrelated matches nothing", () => {
  const root = tempRoot();
  try {
    writeSkill(root, "brief", `---\nname: morning-brief\ndescription: Build the morning brief from email.\ntriggers: [bản tin sáng, morning brief]\n---\nBody.\n`);
    writeSkill(root, "other", `---\nname: notes\ndescription: Keep notes.\ntriggers: [ghi chú]\n---\nBody.\n`);
    const registry = new SkillRegistry();
    registry.scan(root);
    assert.equal(
      registry.matchSkill("cho tôi bản tin sáng hôm nay")?.name,
      "morning-brief",
    );
    assert.equal(
      registry.matchSkill("dùng skill morning-brief đi")?.name,
      "morning-brief",
    );
    assert.equal(registry.matchSkill("ghi chú họp đội")?.name, "notes");
    assert.equal(registry.matchSkill("thời tiết hôm nay thế nào"), undefined);
    assert.equal(registry.matchSkill(""), undefined);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("match: the shipped skill matches a Vietnamese request", () => {
  const skillsDir = fileURLToPath(new URL("../../../skills", import.meta.url));
  const registry = new SkillRegistry();
  registry.scan(skillsDir);
  assert.equal(
    registry.matchSkill("cho tôi bản tin sáng hôm nay")?.name,
    "morning-brief",
  );
  assert.equal(registry.matchSkill("mở nhạc lên")?.name, undefined);
});

test("body: ${ZEN_SKILL_DIR} is replaced with the skill directory", () => {
  const root = tempRoot();
  try {
    writeSkill(root, "demo", `---\nname: demo\ndescription: Demo.\n---\nLook in \${ZEN_SKILL_DIR}/data.\n`);
    const registry = new SkillRegistry();
    registry.scan(root);
    const skill = registry.get("demo");
    assert.ok(skill);
    assert.equal(
      skillBody(skill),
      `Look in ${join(root, "demo")}/data.\n`,
    );
    assert.ok(skill.body.includes("${ZEN_SKILL_DIR}"));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
