// STUB - NOT COMPILED, NOT RUN.
// Tauri init, invoke command registration. Command names per prep B-2 section 2.
// Real APIs are filled in during the real SDK install round. Tauri version 2 is
// decided per Telegram message 4070; the remaining stub status is about missing
// implementation, not about an undecided version.
mod commands;

fn main() {
    // NOT HERE YET: real Tauri Builder.
    // Commands registered when filled in:
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
