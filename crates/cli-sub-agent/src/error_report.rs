use anyhow::Error;

const META_SESSION_MARKER: &str = "meta_session_id=";

pub(crate) fn render_user_facing_error(err: &Error) -> String {
    let mut session_id = None;
    let mut messages = Vec::new();

    for cause in err.chain() {
        let message = cause.to_string();
        if let Some(extracted) = parse_meta_session_id(&message) {
            session_id.get_or_insert(extracted.to_string());
            continue;
        }
        if messages.last() != Some(&message) {
            messages.push(message);
        }
    }

    let primary = messages.first().cloned().unwrap_or_else(|| err.to_string());
    let mut rendered = format!("Error: {primary}");

    if messages.len() > 1 {
        rendered.push_str("\n\nCaused by:");
        for cause in messages.iter().skip(1) {
            rendered.push_str(&format!("\n  - {cause}"));
        }
    }

    if let Some(session_id) = session_id {
        rendered.push_str(&format!("\n\nSession ID: {session_id}"));
    }

    rendered
}

fn parse_meta_session_id(message: &str) -> Option<&str> {
    let suffix = message.trim().strip_prefix(META_SESSION_MARKER)?;
    let end = suffix
        .find(|ch: char| !ch.is_ascii_alphanumeric())
        .unwrap_or(suffix.len());
    let session_id = &suffix[..end];
    (!session_id.is_empty()).then_some(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_user_facing_error_skips_meta_wrapper_and_keeps_session_id() {
        let err = anyhow::anyhow!("ACP subprocess spawn failed: No such file or directory")
            .context("meta_session_id=01KTESTSESSIONABCDE123456");

        let rendered = render_user_facing_error(&err);

        assert!(
            rendered.starts_with("Error: ACP subprocess spawn failed"),
            "unexpected rendered error: {rendered}"
        );
        assert!(
            rendered.contains("Session ID: 01KTESTSESSIONABCDE123456"),
            "session id should be preserved: {rendered}"
        );
        assert!(
            !rendered
                .lines()
                .next()
                .unwrap_or_default()
                .contains("meta_session_id="),
            "first line should not be opaque metadata: {rendered}"
        );
    }

    #[test]
    fn render_user_facing_error_includes_non_meta_cause_chain() {
        let err = anyhow::anyhow!("No such file or directory")
            .context("Failed to load transcript")
            .context("meta_session_id=01KTESTSESSIONABCDE123456");

        let rendered = render_user_facing_error(&err);

        assert!(
            rendered.contains("Error: Failed to load transcript"),
            "top-level actionable context should be surfaced: {rendered}"
        );
        assert!(
            rendered.contains("Caused by:\n  - No such file or directory"),
            "root cause should remain visible: {rendered}"
        );
    }
}
