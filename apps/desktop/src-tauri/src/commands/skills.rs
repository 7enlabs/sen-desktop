// Real Tauri command handlers for the Rust SkillRegistry
// (Tauri follow-up 3, ZC Leader tin 17093). Thin wrappers only:
// they read the managed skills root and delegate to
// commands/skill_registry.rs, which owns the scan and the match.
// No registry logic lives here, and no skill BODY ever crosses
// IPC: both commands return metadata only (name, description,
// triggers, allowed tools, dir, path).
//
// The skills root is set once at boot (main.rs setup) to the app
// config dir's `skills/` folder, or to SEN_SKILLS_ROOT when that
// env var is set (dev override). A missing root scans as empty —
// the same rule as the node registry — so a fresh install without
// a skills dir still works, and /brief skill routing simply
// matches nothing until a skill is installed. Every call re-scans
// the root: the dir is tiny, and a fresh scan picks up a newly
// installed skill without an app restart.
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::State;

use super::session::ContractArgs;
use super::skill_registry::{ScanReport, SkillMeta, SkillRegistry};

/// Managed state: the skills root the Rust registry scans.
/// None until main.rs setup sets it.
pub struct SkillsState {
    pub root: Option<PathBuf>,
}

impl SkillsState {
    pub fn new() -> SkillsState {
        SkillsState { root: None }
    }
}

impl Default for SkillsState {
    fn default() -> Self {
        Self::new()
    }
}

fn scan(state: &SkillsState) -> Result<(SkillRegistry, ScanReport), String> {
    let root = state
        .root
        .clone()
        .ok_or_else(|| "SKILL_REGISTRY_INVALID: skills root is not configured".to_string())?;
    let mut registry = SkillRegistry::new();
    let report = registry.scan(&root)?;
    Ok((registry, report))
}

/// sen_skills_list: scan the skills root and return the loaded
/// skill metadata plus the files that failed to load. A missing
/// root scans as empty, never an error.
#[tauri::command]
pub fn sen_skills_list(
    skills: State<'_, Mutex<SkillsState>>,
    contract: ContractArgs,
) -> Result<ScanReport, String> {
    super::registry::validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let guard = skills.lock().map_err(|e| e.to_string())?;
    let (_, report) = scan(&guard)?;
    Ok(report)
}

/// sen_skill_match: scan the skills root and return the best skill
/// metadata for the query, or null when nothing matches. The
/// frontend matches on this metadata only; it never sees a body.
#[tauri::command]
pub fn sen_skill_match(
    skills: State<'_, Mutex<SkillsState>>,
    contract: ContractArgs,
    query: String,
) -> Result<Option<SkillMeta>, String> {
    super::registry::validate_contract(&contract.to_registry()).map_err(|e| e.to_wire())?;
    let guard = skills.lock().map_err(|e| e.to_string())?;
    let (registry, _) = scan(&guard)?;
    Ok(registry.match_skill(&query))
}
