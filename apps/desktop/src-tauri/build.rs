// Standard Tauri v2 build script: generates the app context that
// tauri::generate_context!() in main.rs embeds. Required for a real
// Builder-based app; without it the context macro has no config.
fn main() {
    tauri_build::build()
}
