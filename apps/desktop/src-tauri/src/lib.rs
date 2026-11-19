// Sen Desktop backend as a library crate.
//
// The Tauri commands live in `commands/` and form the crate's public
// API: the desktop wires a few of them in `run()` (main.rs) and the
// unit tests exercise the rest. They live in a lib, not in the bin, so
// `cargo clippy -D warnings` does not flag the API surface as dead
// code: in a bin crate every `pub` item the binary itself never calls
// is dead code, while in a lib crate it is exported API.
pub mod commands;

use std::sync::Mutex;

use commands::memory_store::MemoryStore;
use commands::registry::Registry;
use commands::skills::SkillsState;
use commands::telegram::TelegramState;
use tauri::Manager;

// Tauri app entry (CLI bridge round): real Builder, real command registration.
// Window size, title and identifier come from tauri.conf.json (1200x800,
// title Sen Desktop per the 10/11 Sen-name decision, identifier
// dev.7enlab.sen-desktop, core Tauri permissions only, no plugin).
// The model runs in the backend through the CLI core + stream-json bridge
// (Phase 2): session creation records the session and wires a CLI bridge
// handle for it; steer/follow_up run the zen CLI for that session. The
// frontend is bundled with Vite (PR #21), so there is a distributable
// build; the webview itself has not run live yet.
//
// P0-1 adds two managed states:
// - Mutex<Option<MemoryStore>>: the file memory (MEMORY.md, USER.md,
//   SOUL.md, daily logs) under the app data dir + "sen-memory/". None
//   when the dir is unavailable: memory commands then fail loudly and
//   turns run on the bare text.
// - TelegramState: the Telegram poller (client + offset), built
//   lazily from the environment on first use. Its pieces live behind
//   their own locks inside the struct (Rust P1-3), so it is managed
//   directly, not wrapped in another Mutex: a poll round must never
//   hold one lock across its 30s long-poll. No token is stored
//   anywhere else, and no command ever returns one.
// - Mutex<SkillsState>: the skills root the Rust SkillRegistry
//   scans (Tauri follow-up 3, 19/11). Set in setup to the app
//   config dir's `skills/` folder (SEN_SKILLS_ROOT overrides);
//   None until then, and the skills commands fail loudly.
pub fn run() {
    tauri::Builder::default()
        .manage(Mutex::new(Registry::new()))
        .manage(Mutex::new(None::<MemoryStore>))
        .manage(TelegramState::new())
        .manage(Mutex::new(SkillsState::new()))
        .setup(|app| {
            // Local CLI settings live in the app config dir
            // (cli-settings.json, model id + optional base URL, never a
            // key). When the dir is unavailable the registry keeps no
            // settings dir and new sessions use the CLI's free default
            // model.
            if let Ok(dir) = app.path().app_config_dir() {
                if let Some(state) = app.try_state::<Mutex<Registry>>() {
                    if let Ok(mut registry) = state.lock() {
                        registry.set_settings_dir(dir.clone());
                    }
                }
                // Skills root for the Rust SkillRegistry (Tauri
                // follow-up 3): <config dir>/skills. SEN_SKILLS_ROOT
                // overrides it (dev override, e.g. the repo's
                // skills/ dir). A missing dir scans as empty, so a
                // fresh install still works.
                if let Some(state) = app.try_state::<Mutex<SkillsState>>() {
                    if let Ok(mut skills) = state.lock() {
                        let root = std::env::var("SEN_SKILLS_ROOT")
                            .ok()
                            .filter(|v| !v.trim().is_empty())
                            .map(std::path::PathBuf::from)
                            .unwrap_or_else(|| dir.join("skills"));
                        skills.root = Some(root);
                    }
                }
            }
            // File memory (P0-1) lives in the app data dir, not the
            // config dir: it is user data (facts, persona, daily logs),
            // not app settings. When the dir is unavailable the memory
            // state stays None and memory commands fail loudly.
            if let Ok(dir) = app.path().app_data_dir() {
                if let Some(state) = app.try_state::<Mutex<Option<MemoryStore>>>() {
                    if let Ok(mut memory) = state.lock() {
                        *memory = Some(MemoryStore::new(dir.join("sen-memory")));
                    }
                }
                // The Telegram poll offset is durable state too: one
                // number in sen-memory/telegram-offset, written before
                // the in-memory offset moves, so a restart resumes
                // instead of replaying handled updates. When the dir
                // is unavailable the offset stays memory-only.
                if let Some(state) = app.try_state::<TelegramState>() {
                    state.set_offset_path(dir.join("sen-memory").join("telegram-offset"));
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session::sen_session_create,
            commands::session::sen_session_steer,
            commands::session::sen_session_follow_up,
            commands::session::sen_session_abort,
            commands::tools::sen_tool_call,
            commands::tools::sen_tool_approve,
            commands::tools::sen_tool_revoke,
            commands::tools::sen_tool_activity_log,
            commands::events::sen_events_subscribe,
            commands::events::sen_events_unsubscribe,
            commands::settings::sen_settings_get_model,
            commands::settings::sen_settings_set_model,
            commands::settings::sen_settings_get_base_url,
            commands::settings::sen_settings_set_base_url,
            commands::settings::sen_settings_get_router,
            commands::settings::sen_settings_set_router,
            commands::memory::sen_memory_read,
            commands::memory::sen_memory_add,
            commands::memory::sen_memory_forget,
            commands::memory::sen_memory_edit,
            commands::memory::sen_memory_set_soul,
            commands::memory::sen_memory_log,
            commands::telegram::sen_telegram_status,
            commands::telegram::sen_telegram_poll_once,
            commands::telegram::sen_telegram_send,
            commands::mcp::sen_mcp_delegate,
            commands::skills::sen_skills_list,
            commands::skills::sen_skill_match,
            commands::brief::sen_brief_source,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
