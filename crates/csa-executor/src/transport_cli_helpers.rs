/// Extract the textual content from a claude `message` payload.
///
/// Claude emits `message.content` either as a plain string or as an array of
/// content blocks `[{"type": "text", "text": "..."}, ...]`.  We concatenate
/// all text blocks and ignore non-text blocks — they appear separately as
/// `tool_use` envelopes anyway.
fn extract_message_text(message: &Option<serde_json::Value>) -> Option<String> {
    let value = message.as_ref()?;
    if let Some(content) = value.get("content") {
        if let Some(s) = content.as_str() {
            return Some(s.to_string());
        }
        if let Some(arr) = content.as_array() {
            let mut buf = String::new();
            for block in arr {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    && let Some(text) = block.get("text").and_then(serde_json::Value::as_str)
                {
                    buf.push_str(text);
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
    }
    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
        return Some(text.to_string());
    }
    None
}
