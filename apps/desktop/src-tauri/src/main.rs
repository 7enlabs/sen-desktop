// STUB - CHUA BIEN DICH, CHUA CHAY.
// Khoi tao Tauri, dang ky invoke commands. Ten command theo prep B-2 muc 2.
// Chi dien day API that sau khi chot version Tauri trong Cargo.toml.
mod commands;

fn main() {
    // CHUA CO: Tauri Builder that.
    // Danh sach command dang ky khi lap day:
    // sen_session_create, sen_session_steer, sen_session_follow_up,
    // sen_session_abort, sen_tool_call, sen_events_subscribe.
    let _registered: &[&str] = &[
        "sen_session_create",
        "sen_session_steer",
        "sen_session_follow_up",
        "sen_session_abort",
        "sen_tool_call",
        "sen_events_subscribe",
    ];
}
