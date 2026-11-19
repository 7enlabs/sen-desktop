// SKILL.md registry, Rust side (Tauri follow-up 3, ZC Leader tin
// 17093). PURE RUST, no Tauri dependency: compiles and tests
// standalone in the scratch harness.
//
// The webview cannot read the filesystem (no node:fs there), so the
// node-side SkillRegistry (packages/core/src/skill-registry.ts)
// could never be instantiated at boot — the production
// instantiation point is this module. It owns the scan: it reads
// <skills-root>/<name>/SKILL.md files, parses the same frontmatter
// the CLI registry reads (name, description, triggers,
// allowed-tools), and returns skill METADATA only — name,
// description, triggers, allowed tools, dir, path. The skill BODY
// never crosses IPC: the frontend only ever matches on metadata
// (see commands/skills.rs + apps/desktop/src/ipc/skills-client.ts),
// and a skill body reaches the model only through the CLI core,
// which reads the same SKILL.md layout itself.
//
// Port notes (parity with the TS registry, same file layout as
// ZenCore crates/zen-cli/src/skills.rs + crates/zen-ext/src/skills.rs
// at 8ac73fe):
// - Frontmatter delimited by `---` lines with `name:` and
//   `description:`; body after the closing fence. An unclosed
//   frontmatter means the whole file is body, same as the CLI.
// - `triggers:` / `allowed-tools:` take a `[a, b]` list, a comma
//   string, or `- item` lines. Unknown keys are ignored.
// - The directory name is the fallback name when frontmatter has
//   no name. A duplicate name fails the scan for that file (the
//   first file wins); a file with parse errors fails for that file.
//   Both are reported, never thrown, so one bad skill cannot hide
//   the rest. A missing root scans as empty; a root that exists but
//   is not a directory is an error.
// - Match scoring mirrors TS matchSkill exactly: a trigger word
//   appearing in the request (+10 each), the skill name appearing
//   (+5), a word of the description appearing (+1 each, capped at
//   +5). A query with no match scores 0 and matches nothing.
// - Word splitting: the TS split regex lists Vietnamese diacritics
//   explicitly; here a word char is any Unicode alphanumeric,
//   `_`, or `-` (char::is_alphanumeric covers the same Vietnamese
//   letters), which keeps the split identical on real requests
//   without a regex dependency.
use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

/// A skill name: [a-zA-Z0-9_-]+, same rule as the CLI registry.
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One registered skill's METADATA — everything the frontend needs
/// to match, and nothing more. No body: the body never crosses IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    /// Trigger words, lowercased, deduped, in file order.
    pub triggers: Vec<String>,
    /// Tool names this skill may use (empty means unrestricted here).
    pub allowed_tools: Vec<String>,
    /// Directory holding SKILL.md.
    pub dir: String,
    /// Absolute path of the SKILL.md file.
    pub path: String,
}

/// The parsed content of one SKILL.md file (name, description,
/// triggers, allowed tools, body). The body is kept here because
/// validation needs it (an empty body is an error), but it is NOT
/// part of SkillMeta and never leaves this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub body: String,
}

fn split_frontmatter(content: &str) -> (Option<Vec<String>>, String) {
    let mut lines = content.lines();
    match lines.next() {
        Some(first) if first.trim() == "---" => {}
        _ => return (None, content.to_string()),
    }
    let mut fm_lines: Vec<String> = Vec::new();
    // lines() drops the trailing newline; re-joining with "\n"
    // reproduces the body the TS split("\n") produces (the TS body
    // keeps a trailing "" element when the file ends with a newline,
    // which is whitespace-only and irrelevant to trim checks).
    let rest: Vec<&str> = lines.collect();
    for (i, line) in rest.iter().enumerate() {
        if line.trim() == "---" || line.trim() == "..." {
            let body = rest[i + 1..].join("\n");
            return (Some(fm_lines), body);
        }
        fm_lines.push(line.to_string());
    }
    // Unclosed frontmatter: the whole file is body (CLI behaviour).
    (None, content.to_string())
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}

fn parse_list_value(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    inner
        .split(',')
        .map(|item| unquote(item).trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Parse one SKILL.md file's text. The file name (its directory
/// name) is the fallback name when frontmatter has no name, so a
/// skill written for the CLI (name only in the directory) still
/// loads. Returns error strings; empty means valid. A file with
/// errors still yields a SkillFile with what could be read, so the
/// caller can report the file instead of silently dropping it.
pub fn parse_skill_file(content: &str, dir_name: &str) -> (SkillFile, Vec<String>) {
    let (frontmatter, body) = split_frontmatter(content);
    let mut errors: Vec<String> = Vec::new();
    let mut name = dir_name.to_string();
    let mut description = String::new();
    let mut triggers: Vec<String> = Vec::new();
    let mut allowed_tools: Vec<String> = Vec::new();
    if let Some(fm_lines) = frontmatter {
        let mut current_key: Option<&str> = None;
        let mut list_items: Vec<String> = Vec::new();
        let flush = |key: Option<&str>,
                     items: &mut Vec<String>,
                     triggers: &mut Vec<String>,
                     allowed_tools: &mut Vec<String>| {
            if key == Some("triggers") {
                *triggers = std::mem::take(items);
            } else if key == Some("allowed-tools") {
                *allowed_tools = std::mem::take(items);
            }
            items.clear();
        };
        for raw_line in &fm_lines {
            let line = raw_line.as_str();
            if let Some(list_item) = line.strip_prefix('-').and_then(|rest| {
                // A `- item` line only counts when it starts with
                // "- " (a dash followed by a space); a bare "-" or
                // a "-word" line is not a list item.
                rest.strip_prefix(' ')
            }) {
                if current_key.is_some() {
                    list_items.push(unquote(list_item).trim().to_string());
                    continue;
                }
            } else if line.trim_start().starts_with("- ") && current_key.is_some() {
                // Indented list items ("  - item").
                let item = line.trim_start()[2..].trim();
                list_items.push(unquote(item).to_string());
                continue;
            }
            // A `key: value` line (key chars [A-Za-z0-9_-]+).
            let key_value = (|| {
                let colon = line.find(':')?;
                let key = line[..colon].trim();
                if key.is_empty()
                    || !key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return None;
                }
                Some((key.to_ascii_lowercase(), line[colon + 1..].to_string()))
            })();
            let Some((key, value)) = key_value else {
                continue;
            };
            flush(
                current_key,
                &mut list_items,
                &mut triggers,
                &mut allowed_tools,
            );
            current_key = None;
            if key == "name" {
                if !value.trim().is_empty() {
                    name = unquote(&value);
                }
            } else if key == "description" {
                description = unquote(&value);
            } else if key == "triggers" || key == "allowed-tools" {
                current_key = if key == "triggers" {
                    Some("triggers")
                } else {
                    Some("allowed-tools")
                };
                if !value.trim().is_empty() {
                    list_items = parse_list_value(&value);
                }
            } else {
                // Unknown frontmatter keys are ignored: a skill
                // written for the CLI may carry keys Sen does not use.
                current_key = None;
            }
        }
        flush(
            current_key,
            &mut list_items,
            &mut triggers,
            &mut allowed_tools,
        );
    }
    if name.trim().is_empty() {
        errors.push("skill name is empty".to_string());
    } else if !is_valid_skill_name(&name) {
        errors.push(format!(
            "skill name {} must be [a-zA-Z0-9_-]+",
            serde_json::to_string(&name).unwrap_or_else(|_| name.clone())
        ));
    }
    if description.trim().is_empty() {
        errors.push(format!(
            "skill {} has no description",
            serde_json::to_string(&name).unwrap_or_else(|_| name.clone())
        ));
    }
    if body.trim().is_empty() {
        errors.push(format!(
            "skill {} has an empty body",
            serde_json::to_string(&name).unwrap_or_else(|_| name.clone())
        ));
    }
    let mut seen = std::collections::HashSet::new();
    triggers.retain(|t| seen.insert(t.to_ascii_lowercase()));
    let triggers: Vec<String> = triggers.iter().map(|t| t.to_ascii_lowercase()).collect();
    (
        SkillFile {
            name,
            description,
            triggers,
            allowed_tools,
            body,
        },
        errors,
    )
}

/// Validate a parsed skill (same checks parse_skill_file reports).
pub fn validate_skill(skill: &SkillFile) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    if skill.name.trim().is_empty() {
        errors.push("skill name is empty".to_string());
    } else if !is_valid_skill_name(&skill.name) {
        errors.push(format!(
            "skill name {} must be [a-zA-Z0-9_-]+",
            serde_json::to_string(&skill.name).unwrap_or_else(|_| skill.name.clone())
        ));
    }
    if skill.description.trim().is_empty() {
        errors.push(format!(
            "skill {} has no description",
            serde_json::to_string(&skill.name).unwrap_or_else(|_| skill.name.clone())
        ));
    }
    if skill.body.trim().is_empty() {
        errors.push(format!(
            "skill {} has an empty body",
            serde_json::to_string(&skill.name).unwrap_or_else(|_| skill.name.clone())
        ));
    }
    errors
}

/// One failed file in a scan report: path + why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedSkill {
    pub path: String,
    pub errors: Vec<String>,
}

/// A scan report: skills loaded, in directory order, plus the files
/// that failed to load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub loaded: Vec<SkillMeta>,
    pub failed: Vec<FailedSkill>,
}

/// The registry: scan a skills root for child SKILL.md files, keep
/// the loaded metadata, answer get/list/match. Bodies are never
/// stored: parse_skill_file needs the body only for validation.
pub struct SkillRegistry {
    skills: HashMap<String, SkillMeta>,
    order: Vec<String>,
}

impl SkillRegistry {
    pub fn new() -> SkillRegistry {
        SkillRegistry {
            skills: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Skills in scan order.
    pub fn list(&self) -> Vec<SkillMeta> {
        self.order
            .iter()
            .filter_map(|name| self.skills.get(name).cloned())
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<SkillMeta> {
        self.skills.get(name).cloned()
    }

    pub fn size(&self) -> usize {
        self.skills.len()
    }

    /// Scan a skills root: every direct child directory holding a
    /// SKILL.md file. A missing root scans as empty (no skills, no
    /// failure) so a fresh install without a skills dir still works;
    /// a root that exists but is not a directory is an error.
    pub fn scan(&mut self, root: &Path) -> Result<ScanReport, String> {
        self.skills.clear();
        self.order.clear();
        let mut loaded: Vec<SkillMeta> = Vec::new();
        let mut failed: Vec<FailedSkill> = Vec::new();
        if !root.exists() {
            return Ok(ScanReport { loaded, failed });
        }
        if !root.is_dir() {
            return Err(format!(
                "SKILL_REGISTRY_INVALID: skills root is not a directory: {}",
                root.display()
            ));
        }
        let mut children: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(root)
            .map_err(|e| format!("cannot read skills root {}: {}", root.display(), e))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read skills root {}: {}", root.display(), e))?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                children.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        children.sort();
        for child in children {
            let path = root.join(&child).join("SKILL.md");
            if !path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    failed.push(FailedSkill {
                        path: path.to_string_lossy().to_string(),
                        errors: vec![format!("cannot read SKILL.md: {}", e)],
                    });
                    continue;
                }
            };
            let (parsed, mut errors) = parse_skill_file(&content, &child);
            if self.skills.contains_key(&parsed.name) {
                errors.push(format!(
                    "duplicate skill name {} (already loaded)",
                    serde_json::to_string(&parsed.name).unwrap_or_else(|_| parsed.name.clone())
                ));
            }
            if !errors.is_empty() {
                failed.push(FailedSkill {
                    path: path.to_string_lossy().to_string(),
                    errors,
                });
                continue;
            }
            let dir = path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let meta = SkillMeta {
                name: parsed.name.clone(),
                description: parsed.description.clone(),
                triggers: parsed.triggers.clone(),
                allowed_tools: parsed.allowed_tools.clone(),
                dir,
                path: path.to_string_lossy().to_string(),
            };
            self.order.push(meta.name.clone());
            self.skills.insert(meta.name.clone(), meta.clone());
            loaded.push(meta);
        }
        Ok(ScanReport { loaded, failed })
    }

    /// Pick the best skill for a request, or None when none matches.
    /// Scoring mirrors the TS matchSkill exactly: a trigger word
    /// appearing in the request (+10 each), the skill name appearing
    /// (+5), a word of the description appearing (+1 each, capped
    /// at +5). A query with no match scores 0 and matches nothing.
    pub fn match_skill(&self, query: &str) -> Option<SkillMeta> {
        let text = query.to_lowercase();
        let words = split_words(&text);
        let word_set: std::collections::HashSet<&str> = words.iter().map(|w| w.as_str()).collect();
        let mut best: Option<SkillMeta> = None;
        let mut best_score = 0;
        for meta in self.list() {
            let mut score = 0;
            for trigger in &meta.triggers {
                if text.contains(trigger.as_str()) {
                    score += 10;
                }
            }
            if text.contains(meta.name.to_lowercase().as_str()) {
                score += 5;
            }
            let desc_words = split_words(&meta.description.to_lowercase());
            let mut desc_hits = 0;
            let mut seen_desc: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for word in &desc_words {
                // Word length counts characters, like the TS
                // `word.length > 2` (UTF-16 units; same count for
                // BMP text), not bytes.
                if word.chars().count() > 2
                    && word_set.contains(word.as_str())
                    && seen_desc.insert(word.as_str())
                {
                    desc_hits += 1;
                }
            }
            score += desc_hits.min(5);
            if score > best_score {
                best_score = score;
                best = Some(meta);
            }
        }
        if best_score > 0 {
            best
        } else {
            None
        }
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Split text into words: a word char is any Unicode alphanumeric,
/// `_`, or `-` — the Rust spelling of the TS split regex (which
/// lists Vietnamese diacritics explicitly; is_alphanumeric covers
/// the same letters).
fn split_words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            current.push(ch);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    const MORNING_BRIEF: &str = r#"---
name: morning-brief
description: Xây bản tin sáng từ email và lịch, rồi đề xuất quyết định chờ duyệt.
triggers:
  - bản tin sáng
  - morning brief
  - hộp thư
  - quyết định chờ duyệt
  - lịch hôm nay
allowed-tools:
  - read_memory
  - draft_email
---

# Morning brief

Xây bản tin sáng từ ảnh chụp hộp thư chưa đọc và lịch.
"#;

    #[test]
    fn parse_reads_frontmatter_lists_and_body() {
        let (skill, errors) = parse_skill_file(MORNING_BRIEF, "morning-brief");
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(skill.name, "morning-brief");
        assert!(skill.description.contains("bản tin sáng"));
        assert_eq!(
            skill.triggers,
            vec![
                "bản tin sáng",
                "morning brief",
                "hộp thư",
                "quyết định chờ duyệt",
                "lịch hôm nay",
            ]
        );
        assert_eq!(skill.allowed_tools, vec!["read_memory", "draft_email"]);
        assert!(skill.body.contains("# Morning brief"));
    }

    #[test]
    fn parse_falls_back_to_dir_name_and_reports_errors() {
        // No name in frontmatter: the directory name wins.
        let content = "---\ndescription: Does a thing.\n---\n\nBody.\n";
        let (skill, errors) = parse_skill_file(content, "my-skill");
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(skill.name, "my-skill");
        // A bad name and an empty body are reported.
        let content = "---\nname: bad name!\ndescription: x\n---\n\n";
        let (skill, errors) = parse_skill_file(content, "dir");
        assert_eq!(skill.name, "bad name!");
        assert_eq!(errors.len(), 2, "errors: {:?}", errors);
        assert!(errors.iter().any(|e| e.contains("must be [a-zA-Z0-9_-]+")));
        assert!(errors.iter().any(|e| e.contains("empty body")));
        // No description is reported too.
        let content = "---\nname: ok-name\n---\n\nBody.\n";
        let (_, errors) = parse_skill_file(content, "dir");
        assert!(errors.iter().any(|e| e.contains("no description")));
        // Unclosed frontmatter: the whole file is body, so a file
        // with no closing fence and no description reports it.
        let content = "---\nname: ok-name\ndescription: x\n\nBody never closed.\n";
        let (skill, errors) = parse_skill_file(content, "dir");
        assert!(skill.body.contains("Body never closed"));
        assert!(errors.iter().any(|e| e.contains("no description")));
    }

    #[test]
    fn parse_accepts_inline_list_forms() {
        let content =
            "---\nname: s\ndescription: d\ntriggers: [a, b]\nallowed-tools: x, y\n---\n\nBody.\n";
        let (skill, errors) = parse_skill_file(content, "s");
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(skill.triggers, vec!["a", "b"]);
        assert_eq!(skill.allowed_tools, vec!["x", "y"]);
        // Triggers are lowercased and deduped, in file order.
        let content = "---\nname: s\ndescription: d\ntriggers: [Ban, ban, TIN]\n---\n\nBody.\n";
        let (skill, errors) = parse_skill_file(content, "s");
        assert!(errors.is_empty(), "errors: {:?}", errors);
        assert_eq!(skill.triggers, vec!["ban", "tin"]);
    }

    #[test]
    fn scan_loads_real_files_and_reports_bad_ones() {
        let root = std::env::temp_dir().join(format!("sen_skill_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_skill(&root, "morning-brief", MORNING_BRIEF);
        write_skill(
            &root,
            "broken",
            "---\nname: broken\ndescription: x\n---\n\n",
        );
        // Sorts after "morning-brief", so this duplicate name fails
        // (the first file wins in directory order).
        write_skill(
            &root,
            "z-duplicate",
            "---\nname: morning-brief\ndescription: dup\n---\n\nDup body.\n",
        );
        // A child dir with no SKILL.md is skipped silently.
        std::fs::create_dir_all(root.join("no-skill")).unwrap();
        let mut registry = SkillRegistry::new();
        let report = registry.scan(&root).unwrap();
        assert_eq!(report.loaded.len(), 1);
        assert_eq!(report.loaded[0].name, "morning-brief");
        // Metadata carries no body.
        let json = serde_json::to_string(&report.loaded[0]).unwrap();
        assert!(
            !json.contains("Morning brief"),
            "metadata must not carry the body"
        );
        assert_eq!(report.failed.len(), 2);
        assert!(report.failed.iter().any(|f| f.path.contains("broken")));
        assert!(report.failed.iter().any(|f| f.path.contains("z-duplicate")));
        assert!(report
            .failed
            .iter()
            .any(|f| f.errors.iter().any(|e| e.contains("duplicate"))));
        assert_eq!(registry.size(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_missing_root_is_empty_and_non_dir_root_errors() {
        let mut registry = SkillRegistry::new();
        let missing =
            std::env::temp_dir().join(format!("sen_skill_missing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let report = registry.scan(&missing).unwrap();
        assert!(report.loaded.is_empty() && report.failed.is_empty());
        let file = std::env::temp_dir().join(format!("sen_skill_file_{}", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        assert!(registry.scan(&file).is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn match_scores_triggers_name_and_description_like_ts() {
        let root = std::env::temp_dir().join(format!("sen_skill_match_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        write_skill(&root, "morning-brief", MORNING_BRIEF);
        let mut registry = SkillRegistry::new();
        registry.scan(&root).unwrap();
        assert_eq!(
            registry
                .match_skill("cho tôi bản tin sáng hôm nay")
                .map(|s| s.name),
            Some("morning-brief".to_string())
        );
        assert_eq!(registry.match_skill("kể chuyện cười"), None);
        assert_eq!(
            registry.match_skill("morning-brief").map(|s| s.name),
            Some("morning-brief".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn split_words_keeps_vietnamese_words_whole() {
        assert_eq!(
            split_words("bản tin sáng, lịch hôm nay!"),
            vec!["bản", "tin", "sáng", "lịch", "hôm", "nay"]
        );
        assert_eq!(
            split_words("morning-brief x_y"),
            vec!["morning-brief", "x_y"]
        );
    }
}
