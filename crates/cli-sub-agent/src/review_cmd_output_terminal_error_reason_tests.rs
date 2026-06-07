use super::terminal_tool_error_reason;

#[test]
fn terminal_tool_error_reason_survives_trailing_stream_noise() {
    let transcript = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"PASS"}}"#,
        r#"{"type":"result","subtype":"error_api","is_error":true,"result":"HTTP 403 Forbidden: authentication failed"}"#,
        "",
        r#"{"type":"item.completed","item":{"type":"tool_call","text":"ignored"}}"#,
        r#"{"type":"usage","input_tokens":1}"#,
        "Usage: provider emitted a diagnostic after the crash",
    ]
    .join("\n");

    assert_eq!(
        terminal_tool_error_reason(&transcript).as_deref(),
        Some("HTTP 403 Forbidden: authentication failed")
    );
}
