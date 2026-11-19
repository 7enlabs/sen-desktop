// Tauri app entry: the backend lives in the lib crate (src/lib.rs),
// so `cargo clippy -D warnings` treats the command API as exported
// API instead of dead code in a bin crate. This binary only starts it.
fn main() {
    sen_desktop::run();
}
