// Tauri app entry (bridge round): real Builder, real command registration.
// Window size, title and identifier come from tauri.conf.json (1200x800,
// temporary title Pi Desktop until the 10/11 Sen-name decision, identifier
// dev.7enlab.sen-desktop, core Tauri permissions only, no plugin).
// No live session exists yet by design: session creation needs an
// explicitly provided model runtime in the frontend, which this phase
// deliberately has none. The frontend is bundled with Vite (PR #21), so
// `pnpm build` produces dist/; the backend wires up the commands.
mod commands;

use std::sync::Mutex;

use commands::registry::Registry;

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(Registry::new()))
        .invoke_handler(tauri::generate_handler![
            commands::session::sen_session_create,
            commands::session::sen_session_steer,
            commands::session::sen_session_follow_up,
            commands::session::sen_session_abort,
            commands::tools::sen_tool_call,
            commands::events::sen_events_subscribe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
