//! Passive evaluation of CSA session history.
//!
//! Scans session result/state files to produce aggregated reports on
//! failure patterns and token usage without modifying any session data.

mod scanner;

pub use scanner::{SessionSummary, scan_sessions};

use serde::{Deserialize, Serialize};

/// Top-level evaluation report for a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub project_key: String,
    pub period_days: u32,
    pub sessions_analyzed: usize,
    pub failure_patterns: Vec<FailurePattern>,
    pub token_stats: TokenStats,
}

/// A recurring failure pattern identified across sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailurePattern {
    pub category: FailCategory,
    pub count: usize,
    pub example_session_ids: Vec<String>,
    pub tool_involved: Option<String>,
}

/// Aggregated token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStats {
    pub total_input: u64,
    pub total_output: u64,
    pub avg_per_session: f64,
    pub estimated_cost_usd: f64,
}

/// Category of session failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FailCategory {
    Timeout,
    Signal,
    Error,
    TokenAnomaly,
    Unknown,
}

impl std::fmt::Display for FailCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "timeout"),
            Self::Signal => write!(f, "signal"),
            Self::Error => write!(f, "error"),
            Self::TokenAnomaly => write!(f, "token_anomaly"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Build an `EvalReport` from scanned sessions.
pub fn build_report(
    project_key: &str,
    period_days: u32,
    sessions: &[SessionSummary],
) -> EvalReport {
    let failure_patterns = identify_failure_patterns(sessions);
    let token_stats = compute_token_stats(sessions);

    EvalReport {
        project_key: project_key.to_string(),
        period_days,
        sessions_analyzed: sessions.len(),
        failure_patterns,
        token_stats,
    }
}

/// Identify recurring failure patterns from session summaries.
///
/// Groups failures by (category, tool) and also detects token anomalies
/// (sessions using > 2x the mean token count).
pub fn identify_failure_patterns(sessions: &[SessionSummary]) -> Vec<FailurePattern> {
    use std::collections::HashMap;

    // Group failures by (category, tool)
    let mut groups: HashMap<(FailCategory, Option<String>), Vec<String>> = HashMap::new();

    for s in sessions {
        // Skip successful sessions
        if s.status == "success" && s.exit_code == 0 {
            continue;
        }

        let cat = categorize_failure(&s.status, s.exit_code);
        let key = (cat, s.tool.clone());
        groups.entry(key).or_default().push(s.session_id.clone());
    }

    let mut patterns: Vec<FailurePattern> = groups
        .into_iter()
        .map(|((category, tool_involved), ids)| {
            let count = ids.len();
            let example_session_ids: Vec<String> = ids.into_iter().take(5).collect();
            FailurePattern {
                category,
                count,
                example_session_ids,
                tool_involved,
            }
        })
        .collect();

    // Token anomaly detection: sessions with total_tokens > 2x mean
    let token_anomalies = detect_token_anomalies(sessions);
    if !token_anomalies.is_empty() {
        patterns.push(FailurePattern {
            category: FailCategory::TokenAnomaly,
            count: token_anomalies.len(),
            example_session_ids: token_anomalies.into_iter().take(5).collect(),
            tool_involved: None,
        });
    }

    // Sort by count descending for readability
    patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.count));
    patterns
}

/// Detect sessions with token usage > 2x the mean.
fn detect_token_anomalies(sessions: &[SessionSummary]) -> Vec<String> {
    let token_values: Vec<(String, u64)> = sessions
        .iter()
        .filter_map(|s| s.total_tokens.map(|t| (s.session_id.clone(), t)))
        .collect();

    if token_values.is_empty() {
        return Vec::new();
    }

    let sum: u64 = token_values.iter().map(|(_, t)| t).sum();
    let mean = sum as f64 / token_values.len() as f64;
    let threshold = mean * 2.0;

    token_values
        .into_iter()
        .filter(|(_, tokens)| *tokens as f64 > threshold)
        .map(|(id, _)| id)
        .collect()
}

/// Categorize a session's failure based on status string and exit code.
fn categorize_failure(status: &str, exit_code: i32) -> FailCategory {
    match status {
        "timeout" => FailCategory::Timeout,
        "signal" => FailCategory::Signal,
        "failure" => FailCategory::Error,
        "success" => FailCategory::Unknown, // Not a failure
        _ => {
            // Fallback to exit code heuristics
            match exit_code {
                0 => FailCategory::Unknown,
                137 | 143 => FailCategory::Signal, // SIGKILL / SIGTERM
                124 => FailCategory::Timeout,      // coreutils timeout exit code
                _ => FailCategory::Error,
            }
        }
    }
}

/// Compute aggregate token statistics from session summaries.
fn compute_token_stats(sessions: &[SessionSummary]) -> TokenStats {
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut sessions_with_tokens: usize = 0;

    for s in sessions {
        if let Some(input) = s.total_input_tokens {
            total_input += input;
        }
        if let Some(output) = s.total_output_tokens {
            total_output += output;
        }
        if let Some(cost) = s.estimated_cost_usd {
            total_cost += cost;
        }
        if s.total_tokens.is_some() {
            sessions_with_tokens += 1;
        }
    }

    let total_all = total_input + total_output;
    let avg_per_session = if sessions_with_tokens > 0 {
        total_all as f64 / sessions_with_tokens as f64
    } else {
        0.0
    };

    TokenStats {
        total_input,
        total_output,
        avg_per_session,
        estimated_cost_usd: total_cost,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(
        id: &str,
        status: &str,
        exit_code: i32,
        tool: Option<&str>,
        tier: Option<&str>,
        total_tokens: Option<u64>,
    ) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            status: status.to_string(),
            exit_code,
            tool: tool.map(|s| s.to_string()),
            tier_name: tier.map(|s| s.to_string()),
            total_input_tokens: total_tokens.map(|t| t / 2),
            total_output_tokens: total_tokens.map(|t| t / 2),
            total_tokens,
            estimated_cost_usd: total_tokens.map(|t| t as f64 * 0.00001),
        }
    }

    #[test]
    fn test_identify_failure_patterns_groups_by_category_and_tool() {
        let sessions = vec![
            make_session("S1", "timeout", 124, Some("gemini-cli"), None, Some(5000)),
            make_session("S2", "timeout", 124, Some("gemini-cli"), None, Some(3000)),
            make_session("S3", "failure", 1, Some("codex"), None, Some(2000)),
            make_session("S4", "success", 0, Some("claude-code"), None, Some(1000)),
        ];

        let patterns = identify_failure_patterns(&sessions);

        // S4 is success, should not appear. S1+S2 = timeout/gemini-cli, S3 = error/codex
        let timeout_pattern = patterns
            .iter()
            .find(|p| p.category == FailCategory::Timeout)
            .expect("should have timeout pattern");
        assert_eq!(timeout_pattern.count, 2);
        assert_eq!(timeout_pattern.tool_involved.as_deref(), Some("gemini-cli"));

        let error_pattern = patterns
            .iter()
            .find(|p| p.category == FailCategory::Error)
            .expect("should have error pattern");
        assert_eq!(error_pattern.count, 1);
        assert_eq!(error_pattern.tool_involved.as_deref(), Some("codex"));
    }

    #[test]
    fn test_identify_failure_patterns_signal() {
        let sessions = vec![
            make_session("S1", "signal", 137, Some("claude-code"), None, Some(8000)),
            make_session("S2", "signal", 143, Some("codex"), None, Some(4000)),
        ];

        let patterns = identify_failure_patterns(&sessions);
        let signal_patterns: Vec<_> = patterns
            .iter()
            .filter(|p| p.category == FailCategory::Signal)
            .collect();
        // Two different tools, so two patterns
        assert_eq!(signal_patterns.len(), 2);
    }

    #[test]
    fn test_identify_failure_patterns_no_failures() {
        let sessions = vec![
            make_session("S1", "success", 0, Some("claude-code"), None, Some(1000)),
            make_session("S2", "success", 0, Some("gemini-cli"), None, Some(1000)),
        ];

        let patterns = identify_failure_patterns(&sessions);
        // No failures, so no failure patterns (token anomaly check: all equal = no anomaly)
        assert!(
            patterns.is_empty(),
            "expected no patterns, got: {:?}",
            patterns
        );
    }

    #[test]
    fn test_identify_failure_patterns_token_anomaly() {
        let sessions = vec![
            make_session("S1", "success", 0, Some("claude-code"), None, Some(1000)),
            make_session("S2", "success", 0, Some("claude-code"), None, Some(1000)),
            make_session("S3", "success", 0, Some("claude-code"), None, Some(1000)),
            // S4 has 10x the tokens — outlier
            make_session("S4", "success", 0, Some("claude-code"), None, Some(10000)),
        ];

        let patterns = identify_failure_patterns(&sessions);
        let anomaly = patterns
            .iter()
            .find(|p| p.category == FailCategory::TokenAnomaly);
        assert!(anomaly.is_some(), "should detect token anomaly");
        let anomaly = anomaly.unwrap();
        assert!(anomaly.example_session_ids.contains(&"S4".to_string()));
    }

    #[test]
    fn test_identify_failure_patterns_sorted_by_count() {
        let sessions = vec![
            make_session("S1", "timeout", 124, Some("a"), None, None),
            make_session("S2", "timeout", 124, Some("a"), None, None),
            make_session("S3", "timeout", 124, Some("a"), None, None),
            make_session("S4", "failure", 1, Some("b"), None, None),
        ];

        let patterns = identify_failure_patterns(&sessions);
        assert!(patterns.len() >= 2);
        // First pattern should have higher count
        assert!(patterns[0].count >= patterns[1].count);
    }

    #[test]
    fn test_compute_token_stats() {
        let sessions = vec![
            make_session("S1", "success", 0, None, None, Some(2000)),
            make_session("S2", "success", 0, None, None, Some(4000)),
            make_session("S3", "failure", 1, None, None, None), // no tokens
        ];

        let stats = compute_token_stats(&sessions);
        assert_eq!(stats.total_input, 1000 + 2000); // 2000/2 + 4000/2
        assert_eq!(stats.total_output, 1000 + 2000);
        // avg_per_session: total / sessions_with_tokens = 6000 / 2 = 3000
        assert!((stats.avg_per_session - 3000.0).abs() < 0.1);
    }

    #[test]
    fn test_build_report() {
        let sessions = vec![
            make_session(
                "S1",
                "success",
                0,
                Some("claude-code"),
                Some("tier-1"),
                Some(5000),
            ),
            make_session(
                "S2",
                "failure",
                1,
                Some("codex"),
                Some("tier-2"),
                Some(3000),
            ),
        ];

        let report = build_report("my-project", 7, &sessions);
        assert_eq!(report.project_key, "my-project");
        assert_eq!(report.period_days, 7);
        assert_eq!(report.sessions_analyzed, 2);
        assert!(!report.failure_patterns.is_empty());
        assert!(report.token_stats.total_input > 0);
    }
}
