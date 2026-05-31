use super::{
    FailoverSkipKind, TierModelExclusion, build_review_fallback_chain,
    writer_family_diversity_warning,
};
use csa_core::types::ToolName;

#[test]
fn classify_distinguishes_quota_rate_limit_transport_and_error() {
    assert_eq!(
        FailoverSkipKind::classify("gemini-cli reached its monthly spending cap"),
        FailoverSkipKind::OauthQuota
    );
    assert_eq!(
        FailoverSkipKind::classify("permanent quota exhaustion detected"),
        FailoverSkipKind::OauthQuota
    );
    assert_eq!(
        FailoverSkipKind::classify("HTTP 429 Too Many Requests"),
        FailoverSkipKind::RateLimit429
    );
    assert_eq!(
        FailoverSkipKind::classify("RESOURCE_EXHAUSTED from provider"),
        FailoverSkipKind::RateLimit429
    );
    assert_eq!(
        FailoverSkipKind::classify("acp server shut down unexpectedly"),
        FailoverSkipKind::TransportError
    );
    assert_eq!(
        FailoverSkipKind::classify("model returned a malformed verdict"),
        FailoverSkipKind::AttemptedAndErrored
    );
}

#[test]
fn category_strings_are_stable_and_distinct() {
    let kinds = [
        FailoverSkipKind::OauthQuota,
        FailoverSkipKind::RateLimit429,
        FailoverSkipKind::Disabled,
        FailoverSkipKind::AvailabilityDetectionMiss,
        FailoverSkipKind::TransportError,
        FailoverSkipKind::AttemptedAndErrored,
        FailoverSkipKind::MalformedSpec,
        FailoverSkipKind::WhitelistFiltered,
    ];
    let categories: Vec<&str> = kinds.iter().map(|k| k.category()).collect();
    let unique: std::collections::HashSet<&str> = categories.iter().copied().collect();
    assert_eq!(unique.len(), kinds.len(), "categories must be distinct");
    assert_eq!(FailoverSkipKind::Disabled.category(), "disabled");
    assert_eq!(
        FailoverSkipKind::AvailabilityDetectionMiss.category(),
        "availability-detection-miss"
    );
}

#[test]
fn only_quota_kinds_count_as_quota_exhausted() {
    assert!(FailoverSkipKind::OauthQuota.is_quota());
    assert!(FailoverSkipKind::RateLimit429.is_quota());
    assert!(!FailoverSkipKind::Disabled.is_quota());
    assert!(!FailoverSkipKind::AvailabilityDetectionMiss.is_quota());
    assert!(!FailoverSkipKind::TransportError.is_quota());
    assert!(!FailoverSkipKind::AttemptedAndErrored.is_quota());
}

// DONE-WHEN (Ask 1): the chain records, for EACH tool it skips or attempts, a
// per-tool categorised reason. Reproduces the #1714 scenario: gemini-cli is
// attempted and 429s, antigravity-cli is disabled, codex is disabled (NOT
// quota), and claude-code is the surviving selection (absent from the chain).
#[test]
fn build_chain_records_per_tool_reasons_for_multi_skip() {
    let ordered_specs = vec![
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        "antigravity-cli/google/default/xhigh".to_string(),
        "codex/openai/gpt-5.5/high".to_string(),
        "claude-code/anthropic/sonnet-4.6/xhigh".to_string(),
    ];
    let exclusions = vec![
        TierModelExclusion {
            model_spec: "antigravity-cli/google/default/xhigh".to_string(),
            tool: Some(ToolName::AntigravityCli),
            kind: FailoverSkipKind::Disabled,
        },
        TierModelExclusion {
            model_spec: "codex/openai/gpt-5.5/high".to_string(),
            tool: Some(ToolName::Codex),
            kind: FailoverSkipKind::Disabled,
        },
    ];
    let attempt_failures = vec![(
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        "gemini-cli hit HTTP 429".to_string(),
    )];

    let chain = build_review_fallback_chain(&ordered_specs, &exclusions, &attempt_failures);

    // One entry per skipped/attempted tool, in tier-definition order; the
    // selected claude-code reviewer is NOT in the chain.
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].tool, "gemini-cli");
    assert_eq!(chain[0].skip_reason, "rate-limit-429");
    assert!(chain[0].quota_exhausted);
    assert_eq!(chain[1].tool, "antigravity-cli");
    assert_eq!(chain[1].skip_reason, "disabled");
    assert!(!chain[1].quota_exhausted);
    // The crux of #1714: codex is recorded explicitly as `disabled`, NOT
    // collapsed into a quota reason — so the orchestrator can tell codex was
    // never attempted due to config, not quota exhaustion.
    assert_eq!(chain[2].tool, "codex");
    assert_eq!(chain[2].skip_reason, "disabled");
    assert!(!chain[2].quota_exhausted);
    assert!(
        chain.iter().all(|a| a.tool != "claude-code"),
        "selected reviewer must not appear as a skip"
    );
}

#[test]
fn build_chain_appends_entries_outside_tier_order() {
    // Global-fallback path: no tier model list, so ordered_specs is empty but
    // attempt failures must still be recorded.
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[(
            "codex/openai/gpt-5.5/high".to_string(),
            "transport: server shut down unexpectedly".to_string(),
        )],
    );
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].tool, "codex");
    assert_eq!(chain[0].skip_reason, "transport-error");
    assert!(!chain[0].quota_exhausted);
}

// Ask 3 (#1714): warn (do NOT fail) when failover lands on the writer's family.
#[test]
fn diversity_warning_fires_on_same_family_failover() {
    let warning = writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, true);
    assert!(warning.is_some());
    let text = warning.unwrap();
    assert!(text.contains("claude-code"));
    assert!(text.contains("#1714"));
}

#[test]
fn diversity_warning_silent_when_families_differ() {
    assert!(
        writer_family_diversity_warning(Some("claude-code"), ToolName::GeminiCli, true).is_none()
    );
}

#[test]
fn diversity_warning_silent_without_failover() {
    assert!(
        writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, false).is_none()
    );
}

#[test]
fn diversity_warning_silent_when_writer_unknown() {
    assert!(writer_family_diversity_warning(None, ToolName::ClaudeCode, true).is_none());
    // Unrecognised writer tool name → cannot assert diversity loss.
    assert!(
        writer_family_diversity_warning(Some("mystery-tool"), ToolName::ClaudeCode, true).is_none()
    );
}
