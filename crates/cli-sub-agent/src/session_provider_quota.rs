use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const PROVIDER_QUOTA_SCAN_MAX_BYTES: u64 = 64 * 1024;
const PROVIDER_QUOTA_DETAIL_MAX_CHARS: usize = 220;
const PROVIDER_QUOTA_RETRY_MAX_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderQuotaDisplay {
    pub(crate) summary: String,
    pub(crate) hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderQuotaCandidate {
    score: u32,
    tool: Option<String>,
    retry_after: Option<String>,
    detail: String,
}

pub(crate) fn provider_quota_display_for_result(
    session_dir: &Path,
    result: &csa_session::SessionResult,
) -> Option<ProviderQuotaDisplay> {
    if result.status.eq_ignore_ascii_case("success") || result.exit_code == 0 {
        return None;
    }
    provider_quota_display(
        session_dir,
        Some(result.tool.as_str()),
        Some(result.summary.as_str()),
    )
}

pub(crate) fn provider_quota_display_for_session_dir(
    session_dir: &Path,
) -> Option<ProviderQuotaDisplay> {
    let result = read_result_summary_from_dir(session_dir);
    if result.as_ref().is_some_and(|result| {
        result.status.eq_ignore_ascii_case("success") || result.exit_code == 0
    }) {
        return None;
    }
    provider_quota_display(
        session_dir,
        result.as_ref().map(|result| result.tool.as_str()),
        result.as_ref().map(|result| result.summary.as_str()),
    )
}

fn provider_quota_display(
    session_dir: &Path,
    tool: Option<&str>,
    raw_summary: Option<&str>,
) -> Option<ProviderQuotaDisplay> {
    let mut candidates = Vec::new();
    if let Some(candidate) = raw_summary.and_then(|summary| provider_quota_candidate(summary, tool))
    {
        candidates.push(candidate);
    }

    for path in provider_quota_scan_paths(session_dir) {
        let Some(text) = read_bounded_tail(&path) else {
            continue;
        };
        if let Some(candidate) = provider_quota_candidate(&text, tool) {
            candidates.push(candidate);
        }
    }

    let candidate = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.score)?;
    let tool_label = candidate
        .tool
        .as_deref()
        .or(tool)
        .map(provider_tool_label)
        .unwrap_or("provider");
    let issue = if tool_label == "Codex"
        && candidate
            .detail
            .to_ascii_lowercase()
            .contains("usage limit")
    {
        "Codex usage limit hit".to_string()
    } else {
        format!("{tool_label} quota/rate limit hit")
    };
    let mut summary = format!("provider quota exhausted: {issue}");
    if let Some(retry_after) = candidate.retry_after.as_deref() {
        summary.push_str("; retry_after=");
        summary.push_str(retry_after);
    } else if !candidate.detail.is_empty() {
        summary.push_str("; detail=");
        summary.push_str(&candidate.detail);
    }

    Some(ProviderQuotaDisplay {
        summary,
        hint: provider_quota_hint(tool_label).to_string(),
    })
}

fn read_result_summary_from_dir(session_dir: &Path) -> Option<csa_session::SessionResult> {
    let raw = fs::read_to_string(session_dir.join(csa_session::result::RESULT_FILE_NAME)).ok()?;
    toml::from_str::<csa_session::SessionResult>(&raw).ok()
}

fn provider_quota_scan_paths(session_dir: &Path) -> Vec<PathBuf> {
    [
        session_dir.join("stderr.log"),
        session_dir.join("stdout.log"),
        session_dir.join("output.log"),
        session_dir.join("output").join("full.md"),
        session_dir.join("output").join("acp-events.jsonl"),
    ]
    .into_iter()
    .collect()
}

fn read_bounded_tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(PROVIDER_QUOTA_SCAN_MAX_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.take(PROVIDER_QUOTA_SCAN_MAX_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.is_empty() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if let Some(newline) = (start > 0).then(|| text.find('\n')).flatten() {
        text = text[newline + 1..].to_string();
    }
    Some(text)
}

fn provider_quota_candidate(text: &str, tool: Option<&str>) -> Option<ProviderQuotaCandidate> {
    let score = provider_quota_score(text)?;
    let detail = provider_quota_detail(text)?;
    let retry_after = retry_after_snippet(&detail).or_else(|| retry_after_snippet(text));
    let inferred_tool = infer_provider_tool(text).or_else(|| tool.map(ToOwned::to_owned));
    Some(ProviderQuotaCandidate {
        score,
        tool: inferred_tool,
        retry_after,
        detail: clip_chars(
            &compact_visible_text(&csa_session::redact_text_content(&detail)),
            PROVIDER_QUOTA_DETAIL_MAX_CHARS,
        ),
    })
}

fn provider_quota_score(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let mut score = 0_u32;
    for (needle, weight) in [
        ("you've hit your usage limit", 30),
        ("you have hit your usage limit", 30),
        ("usage_limit_exceeded", 24),
        ("usage limit", 20),
        ("monthly usage", 16),
        ("quota_exhausted", 14),
        ("quota exhausted", 14),
        ("quota exceeded", 12),
        ("rate_limit_exceeded", 10),
        ("rate limit", 9),
        ("too many requests", 8),
        ("http 429", 8),
        ("status 429", 8),
        ("429", 4),
    ] {
        if lower.contains(needle) {
            score = score.saturating_add(weight);
        }
    }
    for (needle, weight) in [
        ("try again at", 8),
        ("try again", 5),
        ("retry after", 5),
        ("reset", 4),
        ("purchase", 2),
        ("credit", 2),
        ("billing", 2),
    ] {
        if lower.contains(needle) {
            score = score.saturating_add(weight);
        }
    }
    (score >= 8).then_some(score)
}

fn provider_quota_detail(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let signal_index = [
        "you've hit your usage limit",
        "you have hit your usage limit",
        "usage_limit_exceeded",
        "usage limit",
        "monthly usage",
        "quota_exhausted",
        "quota exhausted",
        "quota exceeded",
        "rate_limit_exceeded",
        "rate limit",
        "too many requests",
        "http 429",
        "status 429",
        "429",
    ]
    .into_iter()
    .filter_map(|needle| lower.find(needle))
    .min()?;
    let start = previous_detail_boundary(text, signal_index);
    let tail = text
        .get(start..)?
        .trim_start_matches([' ', '\t', '\r', '\n', ',', ';']);
    Some(tail.to_string())
}

fn retry_after_snippet(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for marker in [
        "try again at",
        "retry after",
        "quota will reset",
        "reset at",
    ] {
        let Some(index) = lower.find(marker) else {
            continue;
        };
        let snippet = text.get(index..)?;
        let end = snippet_end(snippet);
        let compact = compact_visible_text(&csa_session::redact_text_content(&snippet[..end]));
        if !compact.is_empty() {
            return Some(clip_chars(&compact, PROVIDER_QUOTA_RETRY_MAX_CHARS));
        }
    }
    None
}

fn previous_detail_boundary(text: &str, before: usize) -> usize {
    let Some(prefix) = text.get(..before) else {
        return 0;
    };
    prefix
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| matches!(ch, '\n' | '\r' | ';').then_some(idx + ch.len_utf8()))
        .unwrap_or(0)
}

fn snippet_end(text: &str) -> usize {
    for (idx, ch) in text.char_indices() {
        if matches!(ch, '\n' | '\r' | ';') {
            return idx;
        }
        if ch == '.' {
            let after = idx + ch.len_utf8();
            if text[after..].starts_with(' ') || after == text.len() {
                return after;
            }
        }
    }
    text.len()
}

fn compact_visible_text(text: &str) -> String {
    let mut compact = String::with_capacity(text.len().min(PROVIDER_QUOTA_DETAIL_MAX_CHARS));
    let mut last_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_control() {
            if !last_was_space {
                compact.push(' ');
                last_was_space = true;
            }
            continue;
        }
        compact.push(ch);
        last_was_space = false;
    }
    compact.trim().to_string()
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut clipped = text.chars().take(keep).collect::<String>();
    clipped = clipped.trim_end().to_string();
    clipped.push_str("...");
    clipped
}

fn infer_provider_tool(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("codex") || lower.contains("chatgpt.com/codex") {
        return Some("codex".to_string());
    }
    if lower.contains("gemini") {
        return Some("gemini-cli".to_string());
    }
    None
}

fn provider_tool_label(tool: &str) -> &'static str {
    match tool {
        "codex" => "Codex",
        "gemini-cli" | "antigravity-cli" => "Gemini",
        "claude-code" => "Claude Code",
        "opencode" => "OpenCode",
        _ => "provider",
    }
}

fn provider_quota_hint(tool_label: &str) -> &'static str {
    match tool_label {
        "Codex" => {
            "do not retry CSA-Codex sessions until cooldown expires unless using a different configured provider."
        }
        "Gemini" => {
            "do not retry Gemini-backed sessions until quota resets unless using a different configured provider."
        }
        _ => {
            "do not retry this provider until quota/cooldown clears unless using a different configured provider."
        }
    }
}
