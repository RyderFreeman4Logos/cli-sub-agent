use super::{
    AttemptFailure, FailoverSkipKind, TierModelExclusion, build_review_fallback_chain,
    writer_family_diversity_warning,
};
use csa_core::types::ToolName;

/// Build a runtime attempt failure whose permanent-quota classification is
/// derived from the raw reason string (the build-time path: `quota_exhausted =
/// None`). Mirrors a failure that never carried a structured scheduler flag.
fn raw_failure(model_spec: &str, reason: &str) -> AttemptFailure {
    AttemptFailure {
        model_spec: model_spec.to_string(),
        reason: reason.to_string(),
        quota_exhausted: None,
    }
}

/// Build a runtime attempt failure carrying the scheduler's AUTHORITATIVE
/// permanent-quota flag, reproducing the production path where the scheduler
/// already decided permanent vs. transient before normalizing `reason`.
fn scheduler_failure(model_spec: &str, reason: &str, quota_exhausted: bool) -> AttemptFailure {
    AttemptFailure {
        model_spec: model_spec.to_string(),
        reason: reason.to_string(),
        quota_exhausted: Some(quota_exhausted),
    }
}

#[test]
fn classify_distinguishes_quota_rate_limit_transport_and_error() {
    assert_eq!(
        FailoverSkipKind::classify("gemini-cli reached its monthly spending cap"),
        FailoverSkipKind::OauthQuota
    );
    // A billing/spending-cap context marks PERMANENT exhaustion. (A bare
    // "quota" without billing context is transient — see the dedicated
    // generic-quota regression test below.)
    assert_eq!(
        FailoverSkipKind::classify("account billing limit exceeded; quota_exhausted"),
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
        FailoverSkipKind::classify("gemini_cli_crash"),
        FailoverSkipKind::TransportError
    );
    assert_eq!(
        FailoverSkipKind::classify("gemini_runtime_home_unavailable"),
        FailoverSkipKind::TransportError
    );
    assert_eq!(
        FailoverSkipKind::classify("model returned a malformed verdict"),
        FailoverSkipKind::AttemptedAndErrored
    );
}

// #1714 / #1736 classify() narrowing regression: a transient provider error
// that merely CONTAINS the word "quota" alongside a 429 / RESOURCE_EXHAUSTED
// marker must classify as the transient `RateLimit429`, NOT permanent
// `OauthQuota`. The prior `classify()` matched a generic "quota" substring
// first and mislabeled these as permanent (→ quota_exhausted=true), violating
// the `FallbackAttempt.quota_exhausted` contract.
#[test]
fn classify_transient_429_with_quota_word_is_rate_limit_not_oauth_quota() {
    let kind = FailoverSkipKind::classify("HTTP 429: quota exceeded");
    assert_eq!(kind, FailoverSkipKind::RateLimit429);
    assert!(
        !kind.is_quota(),
        "a 429 carrying the word 'quota' is a transient rate limit, not permanent exhaustion"
    );
}

#[test]
fn classify_resource_exhausted_quota_exhausted_without_billing_is_rate_limit() {
    // RESOURCE_EXHAUSTED + QUOTA_EXHAUSTED but NO billing context → transient.
    let kind = FailoverSkipKind::classify("status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED");
    assert_eq!(kind, FailoverSkipKind::RateLimit429);
    assert!(!kind.is_quota());
}

#[test]
fn classify_generic_quota_exceeded_without_billing_is_rate_limit() {
    // Bare "quota exceeded" without any hard-cap/billing marker is the
    // per-minute/per-model throttle class → transient.
    let kind = FailoverSkipKind::classify("quota exceeded for model gemini-3.1-pro");
    assert_eq!(kind, FailoverSkipKind::RateLimit429);
    assert!(!kind.is_quota());
}

#[test]
fn classify_monthly_spending_cap_is_permanent_oauth_quota() {
    let kind = FailoverSkipKind::classify("monthly spending cap reached");
    assert_eq!(kind, FailoverSkipKind::OauthQuota);
    assert!(
        kind.is_quota(),
        "a monthly spending cap is permanent quota exhaustion"
    );
}

#[test]
fn classify_quota_exhausted_with_billing_context_is_permanent_oauth_quota() {
    // QUOTA_EXHAUSTED WITH a billing context IS permanent (matches the canonical
    // gemini detector's billing-context rule).
    let kind = FailoverSkipKind::classify(
        "status: RESOURCE_EXHAUSTED; reason: QUOTA_EXHAUSTED; billing hard limit reached",
    );
    assert_eq!(kind, FailoverSkipKind::OauthQuota);
    assert!(kind.is_quota());
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

// #1714 field-contract regression: `FallbackAttempt.quota_exhausted` is
// documented as PERMANENT quota exhaustion (monthly / spending-cap class) "vs.
// transient rate limit", and the canonical scheduler table maps a plain HTTP
// 429 to `quota_exhausted = false`. So ONLY `OauthQuota` may flag quota
// exhaustion; a transient `RateLimit429` must NOT, even though it still carries
// its own `rate-limit-429` skip_reason category.
#[test]
fn only_permanent_quota_counts_as_quota_exhausted() {
    // Permanent (monthly / spending-cap) quota DOES flag quota exhaustion.
    assert!(FailoverSkipKind::OauthQuota.is_quota());
    // Transient HTTP 429 rate limit does NOT — it is not permanent exhaustion.
    assert!(!FailoverSkipKind::RateLimit429.is_quota());
    assert!(!FailoverSkipKind::Disabled.is_quota());
    assert!(!FailoverSkipKind::AvailabilityDetectionMiss.is_quota());
    assert!(!FailoverSkipKind::TransportError.is_quota());
    assert!(!FailoverSkipKind::AttemptedAndErrored.is_quota());
}

// Serialization-level guard for the same contract: a plain 429 attempt failure
// serializes `quota_exhausted = false` (mapped to the transient `rate-limit-429`
// skip reason), while a permanent monthly-cap exhaustion serializes
// `quota_exhausted = true`.
#[test]
fn plain_429_serializes_not_quota_exhausted_but_permanent_does() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[
            raw_failure(
                "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
                "HTTP 429 Too Many Requests; Retry-After: 30",
            ),
            raw_failure(
                "antigravity-cli/google/default/xhigh",
                "gemini-cli reached its monthly spending cap",
            ),
        ],
        None,
    );
    assert_eq!(chain.len(), 2);
    // Plain transient 429 → rate-limit-429 reason, NOT quota_exhausted.
    assert_eq!(chain[0].tool, "gemini-cli");
    assert_eq!(chain[0].skip_reason, "rate-limit-429");
    assert!(
        !chain[0].quota_exhausted,
        "a plain HTTP 429 is a transient rate limit, not permanent quota exhaustion"
    );
    // Permanent monthly-cap exhaustion → oauth-quota reason AND quota_exhausted.
    assert_eq!(chain[1].tool, "antigravity-cli");
    assert_eq!(chain[1].skip_reason, "oauth-quota");
    assert!(
        chain[1].quota_exhausted,
        "a monthly spending cap is permanent quota exhaustion"
    );
}

// #1714 PRODUCTION-path regression (the gap round 8 named): the scheduler maps a
// real permanent marker such as "monthly spending cap" to the NORMALIZED reason
// "QUOTA_EXHAUSTED" while keeping the structured `quota_exhausted = true`. The
// runtime path must carry that structured flag straight through to
// `FallbackAttempt.quota_exhausted` instead of re-parsing the normalized reason
// via `classify()` (which — correctly for the build-time path, per round 5 —
// maps a bare "QUOTA_EXHAUSTED" to the TRANSIENT `RateLimit429`). Re-parsing here
// would silently downgrade permanent exhaustion to transient and violate the
// `FallbackAttempt.quota_exhausted` contract.
#[test]
fn scheduler_normalized_quota_exhausted_preserves_permanent_flag() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        // Reproduces the scheduler output: human-readable marker already
        // collapsed to "QUOTA_EXHAUSTED", but `quota_exhausted = true` retained.
        &[scheduler_failure(
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "QUOTA_EXHAUSTED",
            true,
        )],
        None,
    );
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].tool, "gemini-cli");
    // The structured flag is honored: permanent, NOT downgraded to transient.
    assert!(
        chain[0].quota_exhausted,
        "scheduler-authoritative permanent quota must survive reason normalization"
    );
    assert_eq!(
        chain[0].skip_reason, "oauth-quota",
        "a scheduler-confirmed permanent quota uses the permanent skip_reason"
    );
}

// Companion to the above on the transient side: a plain HTTP 429 that the
// scheduler classified as `quota_exhausted = false` must serialize the transient
// `quota_exhausted = false`, keeping the granular `rate-limit-429` category.
#[test]
fn scheduler_transient_429_stays_not_quota_exhausted() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[scheduler_failure(
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "HTTP 429",
            false,
        )],
        None,
    );
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].tool, "gemini-cli");
    assert!(
        !chain[0].quota_exhausted,
        "a scheduler-confirmed transient 429 is not permanent quota exhaustion"
    );
    assert_eq!(chain[0].skip_reason, "rate-limit-429");
}

// Defense-in-depth: even if the scheduler's transient marker is a string that
// `classify()` would (on the build-time path) treat as PERMANENT — e.g. an
// "oauth" substring — the structured `quota_exhausted = false` is authoritative
// on the runtime path and the flag stays transient. This guards against the
// inverse of the #1714 bug (spuriously upgrading a scheduler-transient failure).
#[test]
fn scheduler_false_flag_overrides_classify_permanent_string() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[scheduler_failure(
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
            "oauth token refresh throttled",
            false,
        )],
        None,
    );
    assert_eq!(chain.len(), 1);
    assert!(
        !chain[0].quota_exhausted,
        "scheduler is authoritative: a transient failure stays transient regardless of reason text"
    );
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
    let attempt_failures = vec![raw_failure(
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh",
        "gemini-cli hit HTTP 429",
    )];

    // claude-code (pos3) is the surviving selection; the three earlier tools
    // (all BEFORE the winner) are recorded, and the winner is omitted.
    let chain = build_review_fallback_chain(
        &ordered_specs,
        &exclusions,
        &attempt_failures,
        Some("claude-code/anthropic/sonnet-4.6/xhigh"),
    );

    // One entry per skipped/attempted tool, in tier-definition order; the
    // selected claude-code reviewer is NOT in the chain.
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].tool, "gemini-cli");
    assert_eq!(chain[0].skip_reason, "rate-limit-429");
    // A plain HTTP 429 is a transient rate limit, NOT permanent quota
    // exhaustion (the documented `quota_exhausted` contract reserves the flag
    // for the monthly / spending-cap class). The distinct `rate-limit-429`
    // skip_reason still records WHY gemini-cli was skipped.
    assert!(!chain[0].quota_exhausted);
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
        &[raw_failure(
            "codex/openai/gpt-5.5/high",
            "transport: server shut down unexpectedly",
        )],
        None,
    );
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].tool, "codex");
    assert_eq!(chain[0].skip_reason, "transport-error");
    assert!(!chain[0].quota_exhausted);
}

// #1714 after-winner contract regression: when the FIRST tier model wins, a
// LATER disabled/missing tier model was NEVER reached and must NOT appear in the
// persisted chain (the prior code emitted every exclusion found in the full tier
// order, regardless of where the winner sat). Reproduces a tier like
// `[claude-code (enabled, pos0), codex (disabled, pos1)]`: claude-code wins
// first, so codex must be omitted.
#[test]
fn build_chain_omits_after_winner_exclusions_when_first_model_wins() {
    let ordered_specs = vec![
        "claude-code/anthropic/sonnet-4.6/xhigh".to_string(),
        "codex/openai/gpt-5.5/high".to_string(),
    ];
    let exclusions = vec![TierModelExclusion {
        // codex is disabled, but it sits AFTER the winning claude-code so it was
        // never reached.
        model_spec: "codex/openai/gpt-5.5/high".to_string(),
        tool: Some(ToolName::Codex),
        kind: FailoverSkipKind::Disabled,
    }];

    let chain = build_review_fallback_chain(
        &ordered_specs,
        &exclusions,
        &[],
        Some("claude-code/anthropic/sonnet-4.6/xhigh"),
    );

    assert!(
        chain.is_empty(),
        "first-model win with no earlier skips must leave an empty chain; got {chain:?}"
    );
    assert!(
        chain.iter().all(|attempt| attempt.tool != "codex"),
        "a disabled model AFTER the winner was never reached and must not be recorded"
    );
}

// Companion to the above: an after-winner same-family exclusion must NOT trigger
// the heterogeneous-diversity warning, because no actual failover past the
// winner occurred (the false-warning half of the #1714 bug).
#[test]
fn diversity_warning_silent_for_after_winner_same_family_exclusion() {
    let ordered_specs = vec![
        "claude-code/anthropic/sonnet-4.6/xhigh".to_string(),
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
    ];
    // gemini-cli (a DIFFERENT family) is disabled but sits AFTER the winning
    // claude-code, so it was never reached.
    let exclusions = vec![TierModelExclusion {
        model_spec: "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        tool: Some(ToolName::GeminiCli),
        kind: FailoverSkipKind::Disabled,
    }];
    let chain = build_review_fallback_chain(
        &ordered_specs,
        &exclusions,
        &[],
        Some("claude-code/anthropic/sonnet-4.6/xhigh"),
    );

    // The after-winner gemini exclusion is gone, so there is no skipped
    // different-family reviewer to drive the warning.
    assert!(
        writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, &chain)
            .is_none(),
        "no real failover past the first-choice winner → no diversity warning"
    );
}

// Ask 3 (#1714): warn (do NOT fail) when failover lands on the writer's family.
#[test]
fn diversity_warning_fires_on_same_family_failover() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[raw_failure(
            "codex/openai/gpt-5/high",
            "HTTP 429 Too Many Requests",
        )],
        None,
    );
    let warning =
        writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, &chain);
    assert!(warning.is_some());
    let text = warning.unwrap();
    assert!(text.contains("claude-code"));
    assert!(text.contains("#1714"));
}

#[test]
fn diversity_warning_fires_on_build_time_same_family_fallback() {
    let ordered_specs = vec![
        "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        "claude-code/anthropic/claude-sonnet/high".to_string(),
    ];
    let exclusions = vec![TierModelExclusion {
        model_spec: "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        tool: Some(ToolName::GeminiCli),
        kind: FailoverSkipKind::Disabled,
    }];
    // claude-code (pos1) won; the BEFORE-winner gemini-cli exclusion is recorded,
    // so the same-family diversity warning still fires.
    let chain = build_review_fallback_chain(
        &ordered_specs,
        &exclusions,
        &[],
        Some("claude-code/anthropic/claude-sonnet/high"),
    );

    let warning =
        writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, &chain);

    assert!(warning.is_some());
    assert!(
        warning
            .unwrap()
            .contains("heterogeneous review diversity lost")
    );
}

#[test]
fn diversity_warning_silent_when_writer_and_reviewer_families_differ() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[raw_failure(
            "codex/openai/gpt-5/high",
            "HTTP 429 Too Many Requests",
        )],
        None,
    );
    assert!(
        writer_family_diversity_warning(Some("claude-code"), ToolName::GeminiCli, &chain).is_none()
    );
}

#[test]
fn diversity_warning_silent_without_failover() {
    assert!(
        writer_family_diversity_warning(Some("claude-code"), ToolName::ClaudeCode, &[]).is_none()
    );
}

#[test]
fn diversity_warning_silent_when_writer_unknown() {
    let chain = build_review_fallback_chain(
        &[],
        &[],
        &[raw_failure(
            "codex/openai/gpt-5/high",
            "HTTP 429 Too Many Requests",
        )],
        None,
    );
    assert!(writer_family_diversity_warning(None, ToolName::ClaudeCode, &chain).is_none());
    // Unrecognised writer tool name → cannot assert diversity loss.
    assert!(
        writer_family_diversity_warning(Some("mystery-tool"), ToolName::ClaudeCode, &chain)
            .is_none()
    );
}
