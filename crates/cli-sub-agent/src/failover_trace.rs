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
    /// Excluded by an explicit tool whitelist (requested-tool / heterogeneity filter).
    WhitelistFiltered,
}

impl FailoverSkipKind {
    /// Stable, machine-readable category string (kebab-case). This is what the
    /// orchestrator parses out of the fallback chain.
    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::OauthQuota => "oauth-quota",
            Self::RateLimit429 => "rate-limit-429",
            Self::Disabled => "disabled",
            Self::AvailabilityDetectionMiss => "availability-detection-miss",
            Self::TransportError => "transport-error",
            Self::AttemptedAndErrored => "attempted-and-errored",
            Self::MalformedSpec => "malformed-spec",
            Self::WhitelistFiltered => "whitelist-filtered",
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
        if csa_core::gemini::detect_permanent_quota_exhaustion_pattern(&lower).is_some()
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

fn failure_attempt(model_spec: &str, raw_reason: &str) -> FallbackAttempt {
    let kind = FailoverSkipKind::classify(raw_reason);
    FallbackAttempt {
        tool: tool_segment(model_spec).to_string(),
        model_spec: Some(model_spec.to_string()),
        skip_reason: kind.category().to_string(),
        quota_exhausted: kind.is_quota(),
        timestamp: Utc::now(),
    }
}

/// Build the per-tool failover chain for a review/debate run.
///
/// `ordered_specs` is the tier's model list in definition order; it drives a
/// coherent, deterministic trace. `exclusions` are models filtered out at
/// candidate-build time; `attempt_failures` are `(model_spec, raw_reason)` pairs
/// for candidates that were actually tried and errored. Models present in
/// neither collection (the selected reviewer, or specs never reached) are
/// omitted — the chain records only skips and failures. Entries not covered by
/// `ordered_specs` (e.g. the global-fallback path, which has no tier list) are
/// appended so nothing is silently dropped.
pub(crate) fn build_review_fallback_chain(
    ordered_specs: &[String],
    exclusions: &[TierModelExclusion],
    attempt_failures: &[(String, String)],
) -> Vec<FallbackAttempt> {
    let mut chain = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();

    for spec in ordered_specs {
        if let Some(exclusion) = exclusions.iter().find(|e| &e.model_spec == spec) {
            chain.push(exclusion_attempt(exclusion));
            emitted.insert(spec.clone());
        } else if let Some((found_spec, reason)) = attempt_failures.iter().find(|(s, _)| s == spec)
        {
            chain.push(failure_attempt(found_spec, reason));
            emitted.insert(spec.clone());
        }
    }

    for exclusion in exclusions {
        if emitted.insert(exclusion.model_spec.clone()) {
            chain.push(exclusion_attempt(exclusion));
        }
    }
    for (spec, reason) in attempt_failures {
        if emitted.insert(spec.clone()) {
            chain.push(failure_attempt(spec, reason));
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
