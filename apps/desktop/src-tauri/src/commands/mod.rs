// Command module declarations, per the structure in prep B-2 section 2.
// registry.rs is the pure-Rust backend logic (statechart, contract check,
// record, CLI bridge handles); session.rs, tools.rs, events.rs and
// settings.rs are thin #[tauri::command] wrappers that delegate to it.
// cli_bin.rs, cli_settings.rs, cli_spawn.rs and stream_json.rs are the
// pure-Rust CLI bridge: binary resolution, local settings, process
// lifecycle, and stream-json event parsing.
pub mod cli_bin;
pub mod cli_settings;
pub mod cli_spawn;
pub mod events;
pub mod registry;
pub mod session;
pub mod settings;
pub mod stream_json;
pub mod tools;
