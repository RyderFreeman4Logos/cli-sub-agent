use std::fs;
use std::path::Path;

use anyhow::Result;

pub(super) fn display_if_present(
    session_dir: &Path,
    unavailable_reason: Option<&str>,
    json: bool,
) -> Result<bool> {
    let Some(summary) = read_pre_exec_result_summary(session_dir) else {
        return Ok(false);
    };

    if json {
        let payload = serde_json::json!({
            "section": "summary",
            "source": "result.toml",
            "content": summary,
            "tokens": csa_session::estimate_tokens(&summary),
            "unavailable_reason": unavailable_reason,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("{summary}");
        if let Some(reason) = unavailable_reason {
            println!("Unavailable reason: {reason}");
        }
    }
    Ok(true)
}

fn read_pre_exec_result_summary(session_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(session_dir.join(csa_session::result::RESULT_FILE_NAME)).ok()?;
    let result: csa_session::SessionResult = toml::from_str(&raw).ok()?;
    let summary = crate::session_summary_text::human_session_summary(session_dir, &result.summary)?;
    let summary = compact_summary_line(&summary)?;
    summary.starts_with("pre-exec:").then_some(summary)
}

fn compact_summary_line(summary: &str) -> Option<String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }

    const MAX_CHARS: usize = 500;
    let mut compact = summary.replace(['\r', '\n'], " ");
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS).collect::<String>();
        compact.push_str("...");
    }
    Some(compact)
}
