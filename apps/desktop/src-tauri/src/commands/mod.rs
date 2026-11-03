// Command module declarations, per the structure in prep B-2 section 2.
// registry.rs is the pure-Rust backend logic (statechart, contract check,
// record); session.rs, tools.rs and events.rs are thin #[tauri::command]
// wrappers that delegate to it.
pub mod events;
pub mod registry;
pub mod session;
pub mod tools;
