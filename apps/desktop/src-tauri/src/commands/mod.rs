// Command module declarations, per the structure in prep B-2 section 2.
// registry.rs is the pure-Rust backend logic (statechart, contract check,
// record, CLI bridge handles); session.rs, tools.rs, events.rs and
// settings.rs are thin #[tauri::command] wrappers that delegate to it.
// cli_bin.rs, cli_settings.rs, cli_spawn.rs and stream_json.rs are the
// pure-Rust CLI bridge: binary resolution, local settings, process
// lifecycle, and stream-json event parsing.
// P0-1 adds memory_store.rs (pure file memory) + memory.rs (its
// wrappers) and telegram_client.rs (pure Bot API client + poll loop) +
// telegram.rs (its wrappers).
// Tauri follow-ups (19/11, ZC Leader tin 17093): mcp_delegate.rs
// (pure MCP delegate turn) + mcp.rs (its wrapper), skill_registry.rs
// (pure Rust SkillRegistry, metadata only) + skills.rs (its
// wrappers), brief_sources.rs (pure Gmail/Calendar fetch) +
// brief.rs (its wrapper).
pub mod approval_gate;
pub mod atomic_file;
pub mod brief;
pub mod brief_sources;
pub mod cli_bin;
pub mod cli_settings;
pub mod cli_spawn;
pub mod events;
pub mod mcp;
pub mod mcp_delegate;
pub mod memory;
pub mod memory_store;
pub mod model_router;
pub mod registry;
pub mod session;
pub mod settings;
pub mod skill_registry;
pub mod skills;
pub mod stream_json;
pub mod telegram;
pub mod telegram_client;
pub mod tools;
