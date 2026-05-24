use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct CodexTranscriptEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    agent_message: Option<AgentMessageText>,
    #[serde(default)]
    item: Option<CodexTranscriptItem>,
}

#[derive(Debug, Deserialize)]
struct CodexTranscriptItem {
    #[serde(default, rename = "type")]
    item_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentMessageText {
    Text(String),
    Object { text: String },
}

impl AgentMessageText {
    fn into_text(self) -> String {
        match self {
            Self::Text(text) | Self::Object { text } => text,
        }
    }
}

pub(crate) fn first_non_empty_line_is_thread_started(raw_output: &str) -> bool {
    let Some(line) = raw_output.lines().find(|line| !line.trim().is_empty()) else {
        return false;
    };

    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .is_some_and(|value| {
            value
                .as_object()
                .and_then(|object| object.get("type"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|event_type| event_type == "thread.started")
        })
}

pub(crate) fn extract_codex_json_event_text(raw_output: &str) -> Option<String> {
    let mut first_non_empty_was_json = None;
    let pieces: Vec<String> = raw_output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let event = serde_json::from_str::<CodexTranscriptEvent>(line).ok();
            if first_non_empty_was_json.is_none() {
                first_non_empty_was_json = Some(event.is_some());
            }
            event
        })
        .filter_map(|event| {
            if let Some(agent_message) = event.agent_message {
                return Some(agent_message.into_text());
            }
            if event.event_type == "item.completed" {
                return event.item.and_then(|item| {
                    (item.item_type == "agent_message")
                        .then_some(item.text)
                        .flatten()
                });
            }
            None
        })
        .collect();

    if pieces.is_empty() {
        first_non_empty_was_json.unwrap_or(false).then(String::new)
    } else {
        Some(pieces.join("\n"))
    }
}

pub(crate) fn render_codex_or_plain_output(raw: &[u8]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let raw_text = String::from_utf8_lossy(raw);
    let rendered = if first_non_empty_line_is_thread_started(raw_text.as_ref()) {
        extract_codex_json_event_text(raw_text.as_ref()).unwrap_or_else(|| raw_text.to_string())
    } else {
        raw_text.to_string()
    };
    (!rendered.is_empty()).then_some(rendered)
}

#[cfg(test)]
mod tests {
    use super::render_codex_or_plain_output;

    #[test]
    fn render_filters_codex_json_transcript() {
        let raw = [
            r#"{"type":"thread.started","thread_id":"thread_1"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"tool_result","text":"secret shell output"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"final summary"}}"#,
        ]
        .join("\n");

        let rendered =
            render_codex_or_plain_output(raw.as_bytes()).expect("expected rendered text");

        assert_eq!(rendered, "final summary");
        assert!(!rendered.contains("thread.started"));
        assert!(!rendered.contains("tool_result"));
    }

    #[test]
    fn render_preserves_plain_stdout() {
        let rendered =
            render_codex_or_plain_output(b"plain output\n").expect("expected rendered text");

        assert_eq!(rendered, "plain output\n");
    }
}
