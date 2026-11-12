// Tauri app entry (CLI bridge round): real Builder, real command registration.
// Window size, title and identifier come from tauri.conf.json (1200x800,
// title Sen Desktop per the 10/11 Sen-name decision, identifier
// dev.7enlab.sen-desktop, core Tauri permissions only, no plugin).
// The model runs in the backend through the CLI core + stream-json bridge
// (Phase 2): session creation records the session and wires a CLI bridge
// handle for it; steer/follow_up run the zen CLI for that session. The
// frontend is bundled with Vite (PR #21), so there is a distributable
// build; the webview itself has not run live yet.
mod commands;

use std::sync::Mutex;

use commands::registry::Registry;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(Registry::new()))
        .setup(|app| {
            // Local CLI settings live in the app config dir
            // (cli-settings.json, model id + optional base URL, never a
            // key). When the dir is unavailable the registry keeps no
            // settings dir and new sessions use the CLI's free default
            // model.
            if let Ok(dir) = app.path().app_config_dir() {
                if let Some(state) = app.try_state::<Mutex<Registry>>() {
                    if let Ok(mut registry) = state.lock() {
                        registry.set_settings_dir(dir);
                    }
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
            commands::events::sen_events_subscribe,
            commands::events::sen_events_unsubscribe,
            commands::settings::sen_settings_get_model,
            commands::settings::sen_settings_set_model,
            commands::settings::sen_settings_get_base_url,
            commands::settings::sen_settings_set_base_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
