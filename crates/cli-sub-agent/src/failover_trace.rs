//! Failover trace: per-tool skip-reason categorisation for review/debate tier
//! failover (#1714).
//!
//! When a tiered `csa review` / `csa debate` falls back from its first-choice
//! reviewer to a later candidate, the orchestrator needs to know WHY each
//! intermediate tool was skipped. A quota-exhausted tool is a transient
//! condition; a `disabled` or undetected tool is a configuration issue. The
//! prior behaviour collapsed every skip into a single `429_quota_exhausted`
//! reason, which made codex look quota-exhausted when it was merely disabled or
//! missed by availability detection. This module records a categorised reason
//! per skipped/attempted tool and builds the `fallback_chain` surfaced in
//! `result.toml`.

use std::collections::HashSet;

use chrono::Utc;
use csa_core::types::{FallbackAttempt, ToolName, provider_for_tool_name};

/// Why a tier model/tool was skipped or failed during review/debate failover.
///
/// Each variant maps to a stable, machine-readable category surfaced in the
/// fallback chain so callers can distinguish quota exhaustion from disabled /
/// undetected / errored tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailoverSkipKind {
    /// Authentication is unavailable in non-interactive mode.
    AuthUnavailable,
    /// OAuth / subscription quota exhausted (e.g. gemini "monthly spending cap").
    OauthQuota,
    /// Provider returned an HTTP 429 / rate-limit marker.
    RateLimit429,
    /// Tool disabled in config (`[tools.<name>].enabled = false`).
    Disabled,
    /// Binary not found on PATH, or the availability probe missed it.
    AvailabilityDetectionMiss,
    /// Transport / spawn error before the model produced any output.
    TransportError,
    /// Model was attempted and returned an error (non-quota, non-transport).
    AttemptedAndErrored,
    /// Spec malformed: could not parse `tool/provider/model/thinking`.
    MalformedSpec,
}

impl FailoverSkipKind {
    /// Stable, machine-readable category string. This is what the orchestrator
    /// parses out of the fallback chain.
    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::AuthUnavailable => "auth_unavailable",
            Self::OauthQuota => "oauth-quota",
            Self::RateLimit429 => "rate-limit-429",
            Self::Disabled => "disabled",
            Self::AvailabilityDetectionMiss => "availability-detection-miss",
            Self::TransportError => "transport-error",
            Self::AttemptedAndErrored => "attempted-and-errored",
            Self::MalformedSpec => "malformed-spec",
        }
    }

    /// Whether this skip represents PERMANENT quota exhaustion (drives
    /// `FallbackAttempt.quota_exhausted`). Only `OauthQuota` — the monthly /
    /// spending-cap class — counts, matching the documented field contract on
    /// [`csa_core::types::FallbackAttempt::quota_exhausted`] ("permanent quota
    /// exhaustion vs. transient rate limit") and the canonical scheduler
    /// classification (`csa-scheduler` rate_limit table: a plain HTTP 429 maps
    /// to `quota_exhausted = false`). A transient `RateLimit429`, a disabled, or
    /// an undetected tool is NOT permanent quota exhaustion. `RateLimit429`
    /// still carries its own distinct `rate-limit-429` `skip_reason`, so the
    /// per-tool failover categorisation from #1714 is preserved — only this
    /// boolean is narrowed to the permanent class.
    pub(crate) const fn is_quota(self) -> bool {
        matches!(self, Self::OauthQuota)
    }

    /// Classify a free-text failover reason (from `detect_rate_limit` or attempt
    /// stderr) into a stable category. Used for tools that were ACTUALLY
    /// attempted and produced an error — distinct from build-time exclusions
    /// whose kind is known structurally.
    ///
    /// Quota classification mirrors the documented `FallbackAttempt.quota_exhausted`
    /// contract (permanent monthly / spending-cap class vs. transient rate limit)
    /// and the canonical scheduler `rate_limit` table. PERMANENT `OauthQuota` is
    /// matched NARROWLY via the shared
    /// [`csa_core::gemini::detect_permanent_quota_exhaustion_pattern`] helper,
    /// which keys off genuine hard-cap markers ("monthly spending cap",
    /// "billing", "spending limit", `QUOTA_EXHAUSTED` WITH a billing context),
    /// plus a bare `oauth` marker. Transient markers — a plain "429" / "rate
    /// limit" / "too many requests" / "RESOURCE_EXHAUSTED" / generic "quota"
    /// (e.g. "quota exceeded", `QUOTA_EXHAUSTED` WITHOUT billing context) — fall
    /// through to `RateLimit429`. The transient check is ordered FIRST so a
    /// string carrying BOTH a transient marker and the word "quota" (e.g. "HTTP
    /// 429: quota exceeded") classifies as transient `RateLimit429`, not
    /// permanent `OauthQuota`.
    pub(crate) fn classify(reason: &str) -> Self {
        let lower = reason.to_ascii_lowercase();
        // Permanent hard-cap quota (monthly / spending-cap / billing) is matched
        // narrowly first via the canonical detector, then a bare `oauth` marker.
        // A generic "quota" substring is deliberately NOT treated as permanent
        // here — that is the transient class handled below.
        if lower.contains("auth_unavailable") || lower.contains("manual authorization is required")
        {
            Self::AuthUnavailable
        } else if csa_core::gemini::detect_permanent_quota_exhaustion_pattern(&lower).is_some()
            || lower.contains("oauth")
        {
            Self::OauthQuota
        } else if lower.contains("429")
            || lower.contains("rate limit")
            || lower.contains("rate-limit")
            || lower.contains("resource_exhausted")
            || lower.contains("resource exhausted")
            || lower.contains("too many requests")
            || lower.contains("quota")
        {
            Self::RateLimit429
        } else if lower.contains("transport")
            || lower.contains("server shut down")
            || lower.contains("spawn")
            || lower.contains("broken pipe")
            || lower.contains("gemini_cli_crash")
            || lower.contains("gemini_runtime_home_unavailable")
            || lower.contains("tool_crash")
            || lower.contains("unexpected critical error")
            || lower.contains("enospc")
            || lower.contains("no space left on device")
        {
            Self::TransportError
        } else {
            Self::AttemptedAndErrored
        }
    }
}

/// A tier model that was excluded at candidate-build time, with the structural
/// reason it never entered the failover candidate list. Recorded in tier
/// definition order by [`crate::run_helpers::evaluate_tier_models`].
#[derive(Debug, Clone)]
pub(crate) struct TierModelExclusion {
    /// Full model spec (e.g. `codex/openai/gpt-5.5/high`).
    pub(crate) model_spec: String,
    /// Parsed tool, if the spec's tool segment was recognised.
    pub(crate) tool: Option<ToolName>,
    /// Why this model was excluded.
    pub(crate) kind: FailoverSkipKind,
}

fn tool_segment(spec: &str) -> &str {
    spec.split('/').next().unwrap_or(spec)
}

fn exclusion_attempt(exclusion: &TierModelExclusion) -> FallbackAttempt {
    FallbackAttempt {
        tool: exclusion.tool.as_ref().map_or_else(
            || tool_segment(&exclusion.model_spec).to_string(),
            |t| t.as_str().to_string(),
        ),
        model_spec: Some(exclusion.model_spec.clone()),
        skip_reason: exclusion.kind.category().to_string(),
        quota_exhausted: exclusion.kind.is_quota(),
        timestamp: Utc::now(),
    }
}

/// A tier model that was actually ATTEMPTED at runtime and errored, with the
/// normalized failover reason and — for failures originating from a scheduler
/// [`csa_scheduler::RateLimitDetected`] — the authoritative permanent-quota
/// flag the scheduler already computed.
#[derive(Debug, Clone)]
pub(crate) struct AttemptFailure {
    /// Full model spec (e.g. `gemini-cli/google/gemini-3.1-pro-preview/xhigh`).
    pub(crate) model_spec: String,
    /// Normalized failover reason (e.g. the scheduler's `"QUOTA_EXHAUSTED"`).
    pub(crate) reason: String,
    /// Scheduler-authoritative permanent-quota flag, when known. `Some` for
    /// runtime rate-limit detections (the scheduler decided permanent vs.
    /// transient before normalizing `reason`); `None` when only a raw reason
    /// string is available and the kind must be derived via [`FailoverSkipKind::classify`].
    pub(crate) quota_exhausted: Option<bool>,
}

/// Build a [`FallbackAttempt`] for a runtime attempt failure.
///
/// When the scheduler already computed an authoritative `quota_exhausted` flag
/// (`failure.quota_exhausted = Some(_)`), that flag is carried STRAIGHT THROUGH
/// to [`FallbackAttempt::quota_exhausted`] rather than being re-derived from the
/// normalized `reason`. This is the #1714 fix: the scheduler maps a permanent
/// marker such as "monthly spending cap" to `reason = "QUOTA_EXHAUSTED"` while
/// keeping `quota_exhausted = true`; re-parsing that normalized reason via
/// [`FailoverSkipKind::classify`] would (correctly, per the build-time contract)
/// map a bare `"QUOTA_EXHAUSTED"` to the transient `RateLimit429`, silently
/// downgrading permanent exhaustion. The `skip_reason` category still reflects
/// the failure kind: a permanent quota uses `oauth-quota`; a transient runtime
/// failure keeps the granular `classify`-derived category (rate-limit-429 /
/// transport-error / attempted-and-errored) but is FORCED non-quota, since the
/// scheduler is authoritative on the quota determination.
///
/// When `failure.quota_exhausted` is `None` (no `RateLimitDetected` — only a raw
/// reason string), the kind and the quota flag are both derived from
/// [`FailoverSkipKind::classify`], unchanged from the build-time path.
fn failure_attempt(failure: &AttemptFailure) -> FallbackAttempt {
    let (skip_reason, quota_exhausted) = match failure.quota_exhausted {
        Some(true) => (FailoverSkipKind::OauthQuota.category().to_string(), true),
        Some(false) => {
            // Scheduler is authoritative: NOT permanent quota. Keep the granular
            // classify-derived category for auditability, but never let it flag
            // quota exhaustion regardless of what the lossy reason string implies.
            (
                FailoverSkipKind::classify(&failure.reason)
                    .category()
                    .to_string(),
                false,
            )
        }
        None => {
            let kind = FailoverSkipKind::classify(&failure.reason);
            (kind.category().to_string(), kind.is_quota())
        }
    };
    FallbackAttempt {
        tool: tool_segment(&failure.model_spec).to_string(),
        model_spec: Some(failure.model_spec.clone()),
        skip_reason,
        quota_exhausted,
        timestamp: Utc::now(),
    }
}

/// Build the per-tool failover chain for a review/debate run.
///
/// `ordered_specs` is the tier's model list in actual execution order; it drives
/// a coherent, deterministic trace. `exclusions` are models filtered out at
/// candidate-build time; `attempt_failures` are [`AttemptFailure`] records for
/// candidates that were actually tried and errored (each carrying the scheduler's
/// authoritative `quota_exhausted` flag when known). Models present in
/// neither collection (the selected reviewer, or specs never reached) are
/// omitted — the chain records only skips and failures. Entries not covered by
/// `ordered_specs` (e.g. the global-fallback path, which has no tier list) are
/// appended so nothing is silently dropped.
///
/// `selected_model_spec` is the WINNING model (the reviewer/debater that
/// succeeded), if any. When it is `Some` and present in `ordered_specs`, only
/// build-time exclusions and tier-ordered failures STRICTLY BEFORE its index are
/// emitted: the winner itself and any tier specs AFTER it were never reached, so
/// recording them would falsely imply a failover past the winner (#1714 — this
/// also stops [`writer_family_diversity_warning`] from firing for a first-choice
/// success). Runtime `attempt_failures` are always preserved (they are genuine
/// attempts), as are entries outside `ordered_specs` (the global-fallback path).
/// `None` (e.g. the all-models-failed path, where there is no winner) preserves
/// the full chain.
pub(crate) fn build_review_fallback_chain(
    ordered_specs: &[String],
    exclusions: &[TierModelExclusion],
    attempt_failures: &[AttemptFailure],
    selected_model_spec: Option<&str>,
) -> Vec<FallbackAttempt> {
    let mut chain = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();

    // Index of the winning model in tier order, if it is a tier spec. Tier specs
    // at or after this index were never reached and must be omitted.
    let selected_index =
        selected_model_spec.and_then(|sel| ordered_specs.iter().position(|spec| spec == sel));
    // A tier spec is "after the winner" (never reached) when its position is
    // >= the winner's index. Such specs are excluded from every pass below.
    let reached_in_tier_order = |spec: &str| -> bool {
        match (selected_index, ordered_specs.iter().position(|s| s == spec)) {
            (Some(winner_idx), Some(spec_idx)) => spec_idx < winner_idx,
            // No winner in tier order, or spec not in tier order: not gated here
            // (out-of-tier entries are handled by the trailing append loops).
            _ => true,
        }
    };

    for (spec_idx, spec) in ordered_specs.iter().enumerate() {
        if let Some(winner_idx) = selected_index
            && spec_idx >= winner_idx
        {
            // The winner and everything after it in tier order was not reached.
            continue;
        }
        if let Some(exclusion) = exclusions.iter().find(|e| &e.model_spec == spec) {
            chain.push(exclusion_attempt(exclusion));
            emitted.insert(spec.clone());
        } else if let Some(failure) = attempt_failures.iter().find(|f| &f.model_spec == spec) {
            chain.push(failure_attempt(failure));
            emitted.insert(spec.clone());
        }
    }

    for exclusion in exclusions {
        // Skip after-winner tier exclusions (never reached); out-of-tier
        // exclusions still flow through.
        if !reached_in_tier_order(&exclusion.model_spec) {
            continue;
        }
        if emitted.insert(exclusion.model_spec.clone()) {
            chain.push(exclusion_attempt(exclusion));
        }
    }
    for failure in attempt_failures {
        // Runtime attempt failures are genuine attempts and are always recorded,
        // even on the global-fallback path that has no tier order. (A spec that
        // both succeeded AND is listed as a failure cannot occur: a winning model
        // is never pushed as a TierAttemptFailure.)
        if emitted.insert(failure.model_spec.clone()) {
            chain.push(failure_attempt(failure));
        }
    }

    chain
}

/// Ask 3 (#1714): when a tiered review falls back to a reviewer in the SAME
/// model family as the writer/parent, heterogeneous review diversity is lost.
///
/// Returns a non-fatal warning string to attach to the result. This is
/// warn-only by design: it does NOT change the verdict, preserving #1657's
/// guarantee that a clean single-family review stays merge-able. Returns `None`
/// when the fallback chain does not show a skipped different-family reviewer,
/// when the writer family is unknown (so we cannot claim diversity was lost),
/// or when the writer/reviewer families differ.
pub(crate) fn writer_family_diversity_warning(
    writer_tool: Option<&str>,
    final_reviewer: ToolName,
    fallback_chain: &[FallbackAttempt],
) -> Option<String> {
    let writer = writer_tool?;
    let writer_family = provider_for_tool_name(writer)?;
    let reviewer_family = final_reviewer.model_family();
    if writer_family != reviewer_family {
        return None;
    }
    let skipped_different_family = fallback_chain.iter().any(|attempt| {
        provider_for_tool_name(&attempt.tool)
            .is_some_and(|skipped_family| skipped_family != reviewer_family)
    });
    if !skipped_different_family {
        return None;
    }
    Some(format!(
        "review fell back to {} (family {reviewer_family}), same family as the writer \
         ({writer}) — heterogeneous review diversity lost (#1714); treat findings as single-family",
        final_reviewer.as_str(),
    ))
}

#[cfg(test)]
#[path = "failover_trace_tests.rs"]
mod tests;
