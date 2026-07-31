use std::path::Path;

use csa_core::types::ReviewDecision;
use csa_session::ReviewVerdictArtifact;

const UNAVAILABLE_REASON_KIND: &str = "provider_usage_limit";
const UNAVAILABLE_REASON_MAX_CHARS: usize = 360;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnavailableReason {
    kind: &'static str,
    message: String,
}

impl UnavailableReason {
    pub(crate) fn render(&self) -> String {
        format!("{}: {}", self.kind, self.message)
    }
}

pub(crate) fn review_unavailable_reason_label(session_dir: &Path) -> Option<String> {
    let artifact = read_review_verdict_artifact(session_dir)?;
    review_unavailable_reason(&artifact).map(|reason| reason.render())
}

pub(crate) fn review_unavailable_reason(
    artifact: &ReviewVerdictArtifact,
) -> Option<UnavailableReason> {
    if artifact.decision != ReviewDecision::Unavailable {
        return None;
    }

    let candidates = [
        artifact.failure_reason.as_deref(),
        artifact.primary_failure.as_deref(),
    ];
    if let Some(candidate) = candidates
        .into_iter()
        .flatten()
        .filter_map(provider_limit_candidate)
        .max_by_key(|candidate| candidate.score)
    {
        return Some(UnavailableReason {
            kind: UNAVAILABLE_REASON_KIND,
            message: candidate.message,
        });
    }

    // #2911: admission/provider launch failures must remain actionable even when
    // they are not usage-limit shaped (opaque UNAVAILABLE is not enough).
    candidates
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(|text| {
            let redacted = crate::review_failure_context::sanitize_review_surface_text(text);
            let compact = compact_visible_text(&redacted);
            UnavailableReason {
                kind: actionable_unavailable_kind(&compact),
                message: clip_chars(&compact, UNAVAILABLE_REASON_MAX_CHARS),
            }
        })
        .filter(|reason| !reason.message.is_empty())
}

fn actionable_unavailable_kind(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();
    if lower.contains("host_memory_admission")
        || lower.contains("memory admission")
        || lower.contains("active-session memory")
    {
        "host_memory_admission"
    } else if lower.contains("admission") {
        "admission"
    } else if lower.contains("api key")
        || lower.contains("api_key")
        || lower.contains("auth")
        || lower.contains("unauthorized")
    {
        "auth"
    } else if lower.contains("did not launch") || lower.contains("provider") {
        "provider_launch"
    } else {
        "unavailable"
    }
}

fn read_review_verdict_artifact(session_dir: &Path) -> Option<ReviewVerdictArtifact> {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    if !verdict_path.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&verdict_path).ok()?;
    serde_json::from_str::<ReviewVerdictArtifact>(&raw).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReasonCandidate {
    score: u32,
    message: String,
}

fn provider_limit_candidate(text: &str) -> Option<ReasonCandidate> {
    let score = provider_limit_score(text)?;
    let excerpt = excerpt_around_first_limit_signal(text)?;
    let redacted = crate::review_failure_context::sanitize_review_surface_text(&excerpt);
    let compact = compact_visible_text(&redacted);
    if compact.is_empty() {
        return None;
    }
    Some(ReasonCandidate {
        score,
        message: clip_chars(&compact, UNAVAILABLE_REASON_MAX_CHARS),
    })
}

fn provider_limit_score(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let mut score = 0_u32;
    for (needle, weight) in [
        ("usage limit", 12),
        ("monthly usage", 12),
        ("credit", 10),
        ("quota", 9),
        ("resource_exhausted", 8),
        ("resource exhausted", 8),
        ("rate limit", 7),
        ("rate-limit", 7),
        ("too many requests", 6),
        ("http 429", 6),
        ("status 429", 6),
        ("api_error_status\":429", 6),
        ("429", 4),
    ] {
        if lower.contains(needle) {
            score = score.saturating_add(weight);
        }
    }
    for (needle, weight) in [
        ("try again", 5),
        ("reset", 4),
        ("retry", 2),
        ("purchase", 2),
        ("billing", 2),
    ] {
        if lower.contains(needle) {
            score = score.saturating_add(weight);
        }
    }
    (score > 0).then_some(score)
}

fn excerpt_around_first_limit_signal(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let signal_index = [
        "usage limit",
        "monthly usage",
        "credit",
        "quota",
        "resource_exhausted",
        "resource exhausted",
        "rate limit",
        "rate-limit",
        "too many requests",
        "http 429",
        "status 429",
        "api_error_status\":429",
        "429",
    ]
    .into_iter()
    .filter_map(|needle| lower.find(needle))
    .min()?;
    let start = previous_excerpt_boundary(text, signal_index);
    let excerpt = text
        .get(start..)?
        .trim_start_matches([' ', '\t', '\r', '\n', ',', ';']);
    Some(excerpt.to_string())
}

fn previous_excerpt_boundary(text: &str, before: usize) -> usize {
    let prefix = match text.get(..before) {
        Some(prefix) => prefix,
        None => return 0,
    };
    prefix
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| matches!(ch, '\n' | '\r' | ';' | ',').then_some(idx + ch.len_utf8()))
        .unwrap_or(0)
}

fn compact_visible_text(text: &str) -> String {
    let mut compact = String::with_capacity(text.len().min(UNAVAILABLE_REASON_MAX_CHARS));
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use csa_session::Severity;

    use super::*;

    fn unavailable_artifact(
        primary_failure: Option<&str>,
        failure_reason: Option<&str>,
    ) -> ReviewVerdictArtifact {
        ReviewVerdictArtifact {
            schema_version: 1,
            session_id: "01TESTUNAVAILABLE".to_string(),
            timestamp: Utc::now(),
            decision: ReviewDecision::Unavailable,
            verdict_legacy: "UNAVAILABLE".to_string(),
            severity_counts: BTreeMap::from([
                (Severity::Critical, 0),
                (Severity::High, 0),
                (Severity::Medium, 0),
                (Severity::Low, 0),
            ]),
            review_mode: None,
            routed_to: None,
            primary_failure: primary_failure.map(str::to_string),
            failure_reason: failure_reason.map(str::to_string),
            prior_round_refs: Vec::new(),
            review_iterations: None,
            fix_rounds: None,
            diff_size: None,
            large_diff_warning: false,
            large_diff_warning_threshold: None,
            large_diff_warning_changed_lines: None,
            no_provider_launch: None,
        }
    }

    #[test]
    fn review_unavailable_reason_surfaces_codex_usage_limit_hint() {
        let artifact = unavailable_artifact(
            Some("HTTP 429"),
            Some(
                "codex/openai/gpt-5.5/xhigh=You've hit your usage limit. Visit \
                 https://chatgpt.com/codex/settings/usage to purchase more credits or try again \
                 at Jun 20th, 2026 6:48 PM.",
            ),
        );

        let reason = review_unavailable_reason(&artifact).expect("usage-limit reason");

        assert_eq!(reason.kind, "provider_usage_limit");
        assert!(reason.message.contains("You've hit your usage limit."));
        assert!(
            reason
                .message
                .contains("try again at Jun 20th, 2026 6:48 PM")
        );
    }

    #[test]
    fn review_unavailable_reason_redacts_and_bounds_provider_output() {
        let long_tail = " provider-debug".repeat(80);
        let api_field = concat!("api", "_", "key");
        let fake_value = concat!("sk", "-", "sec", "...", "6789");
        let artifact = unavailable_artifact(
            Some("HTTP 429"),
            Some(&format!(
                "provider stderr: You've hit your usage limit. {api_field}={fake_value} \
                 retry after reset.{long_tail}",
            )),
        );

        let reason = review_unavailable_reason(&artifact).expect("usage-limit reason");

        assert!(!reason.message.contains(fake_value));
        assert!(reason.message.contains("[REDACTED]"));
        assert!(reason.message.chars().count() <= UNAVAILABLE_REASON_MAX_CHARS);
    }

    #[test]
    fn review_unavailable_reason_surfaces_admission_reason() {
        let artifact = unavailable_artifact(
            Some(
                "host_memory_admission: reviewer provider did not launch; no alternate configured reviewer candidate was available",
            ),
            Some("codex=host_memory_admission: reviewer provider did not launch"),
        );

        let reason = review_unavailable_reason(&artifact).expect("admission reason");
        assert_eq!(reason.kind, "host_memory_admission");
        assert!(reason.message.contains("host_memory_admission"));
        assert!(reason.message.contains("did not launch"));
    }

    #[test]
    fn review_unavailable_reason_surfaces_auth_failure() {
        let artifact = unavailable_artifact(
            Some("api_key_invalid"),
            Some("gemini-cli tool failure: API Key not found"),
        );

        let reason = review_unavailable_reason(&artifact).expect("auth reason");
        assert_eq!(reason.kind, "auth");
        assert!(reason.message.contains("API Key not found") || reason.message.contains("api_key"));
    }

    #[test]
    fn review_unavailable_reason_relativizes_absolute_paths() {
        let artifact = unavailable_artifact(
            Some("provider_launch"),
            Some(
                "provider failed reading /home/alice/private-project/config.toml while launching reviewer",
            ),
        );

        let reason = review_unavailable_reason(&artifact).expect("path reason");
        assert!(!reason.message.contains("/home/alice"));
        assert!(reason.message.contains("private-project/config.toml"));
        assert!(reason.message.contains("provider failed reading"));
    }

    #[test]
    fn review_unavailable_reason_ignores_non_unavailable_verdict() {
        let mut artifact = unavailable_artifact(Some("HTTP 429"), Some("usage limit"));
        artifact.decision = ReviewDecision::Pass;

        assert_eq!(review_unavailable_reason(&artifact), None);
    }
}
