# sen-events-wire.json — generated wire fixture (R2-01, #65 follow-up)

- What it is: a JSON array of 8 strings. Each string is the exact
  `serde_json::to_string` output of one `SenEvent` variant (sessionStart,
  message, message with toolCalls, messageUpdate, toolResult, system,
  result, unknown), in that order.
- Do NOT hand-edit it. It is generated from Rust and cross-checked on
  both sides:
  - Rust: `sen_event_wire_fixture_matches_serde_output` in
    `apps/desktop/src-tauri/src/commands/stream_json.rs` asserts the
    committed file still matches real serde output byte-for-byte.
  - TS: `session-events: Rust-generated wire fixture passes the
    validator for all variants` in `apps/desktop/tests/
    session-events.test.ts` reads this file verbatim and feeds each
    entry through the real `listenSessionEvents` validator.
- Regenerate (only after an intentional wire change, together with the
  TS lock test's literals):
  - On this box (Tauri crate cannot build here, no gdk-3.0), via the
    standalone harness:
    `cd ~/workspace/goals/telegram-group-exchange-with-zencode-team/hidden_files/sen-issue65-harness && SEN_REGEN_WIRE_FIXTURE=1 cargo test --offline sen_event_wire_fixture_matches_serde_output`
  - Where the Tauri crate builds (CI / a machine with gdk):
    `cd apps/desktop/src-tauri && SEN_REGEN_WIRE_FIXTURE=1 cargo test --offline sen_event_wire_fixture_matches_serde_output`
  - Then run the full checks again: `node --test` (repo root) and
    `cargo test --offline` (harness), and commit the regenerated file.
- If either test fails on this file, the wire drifted: regenerate it
  from Rust, never patch the JSON by hand to make a test pass.
