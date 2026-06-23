use anyhow::Error;
use csa_config::ProjectConfig;
use csa_resource::{MemoryAdmissionError, memory_policy};
use csa_session::{
    MetaSessionState, NO_PROVIDER_LAUNCH_SCHEMA_VERSION, NoProviderLaunchDiagnostic,
    NoProviderLaunchMemoryDiagnostic,
};

use crate::resource_admission_soft_limit::MemorySoftLimitAdmissionError;
use crate::run_resource_overrides::RunResourceOverrides;

pub(crate) const HOST_MEMORY_ADMISSION_REASON: &str = "host_memory_admission";

pub(crate) struct NoProviderLaunchContext<'a> {
    pub(crate) session: &'a MetaSessionState,
    pub(crate) tool_name: &'a str,
    pub(crate) task_type: Option<&'a str>,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) resource_overrides: RunResourceOverrides,
}

pub(crate) fn diagnostic_from_error(
    ctx: NoProviderLaunchContext<'_>,
    error: &Error,
) -> Option<NoProviderLaunchDiagnostic> {
    if let Some(soft_limit) = error.downcast_ref::<MemorySoftLimitAdmissionError>() {
        return Some(from_soft_limit_admission(ctx, soft_limit));
    }
    if let Some(host_memory) = error.downcast_ref::<MemoryAdmissionError>() {
        return Some(from_host_memory_admission(ctx, host_memory));
    }
    None
}

fn from_soft_limit_admission(
    ctx: NoProviderLaunchContext<'_>,
    error: &MemorySoftLimitAdmissionError,
) -> NoProviderLaunchDiagnostic {
    base_diagnostic(
        &ctx,
        error.role(),
        MemorySoftLimitAdmissionError::TERMINATION_REASON,
        NoProviderLaunchMemoryDiagnostic {
            effective_memory_max_mb: Some(error.memory_max_mb()),
            soft_limit_percent: Some(error.soft_limit_percent()),
            soft_threshold_mb: Some(error.threshold_mb()),
            required_floor_mb: Some(error.required_threshold_mb()),
            required_memory_max_mb: Some(error.required_memory_max_mb()),
            ..Default::default()
        },
        error.guidance(),
    )
}

fn from_host_memory_admission(
    ctx: NoProviderLaunchContext<'_>,
    error: &MemoryAdmissionError,
) -> NoProviderLaunchDiagnostic {
    let effective_memory_max_mb = error.projected_spawn_mb.or_else(|| {
        ctx.resource_overrides
            .resolve_memory_max_mb(ctx.config, ctx.tool_name)
    });
    let soft_limit_percent = effective_memory_max_mb.map(|_| {
        ctx.config
            .and_then(|cfg| cfg.resources.soft_limit_percent)
            .unwrap_or(memory_policy::DEFAULT_SOFT_LIMIT_PERCENT)
    });
    let required_floor_mb =
        crate::resource_admission_soft_limit::codex_soft_limit_required_floor_mb(
            ctx.task_type,
            ctx.tool_name,
        );
    let required_memory_max_mb =
        required_floor_mb
            .zip(soft_limit_percent)
            .and_then(|(floor, percent)| {
                memory_policy::required_memory_max_for_soft_limit_mb(floor, percent)
            });
    let soft_threshold_mb = effective_memory_max_mb
        .zip(soft_limit_percent)
        .and_then(|(cap, percent)| memory_policy::soft_limit_threshold_mb(cap, percent));

    let memory = NoProviderLaunchMemoryDiagnostic {
        effective_memory_max_mb,
        soft_limit_percent,
        soft_threshold_mb,
        required_floor_mb,
        required_memory_max_mb,
        reserve_mb: Some(error.reserve_mb),
        available_memory_mb: Some(error.available_phys_mb),
        required_available_mb: error.required_available_mb,
        projected_spawn_mb: error.projected_spawn_mb,
        active_session_rss_mb: error.active_session_rss_mb,
        active_session_projected_mb: error.active_session_projected_mb,
        active_session_count: error.active_session_count,
        sampled_session_count: error.sampled_session_count,
    };
    let guidance = host_memory_guidance(ctx.task_type, ctx.tool_name, &memory);

    base_diagnostic(
        &ctx,
        role_from_task_type(ctx.task_type),
        error.kind.denial_class(),
        memory,
        guidance,
    )
}

fn base_diagnostic(
    ctx: &NoProviderLaunchContext<'_>,
    role: &str,
    denial_class: &str,
    memory: NoProviderLaunchMemoryDiagnostic,
    guidance: Vec<String>,
) -> NoProviderLaunchDiagnostic {
    NoProviderLaunchDiagnostic {
        schema_version: NO_PROVIDER_LAUNCH_SCHEMA_VERSION,
        session_id: ctx.session.meta_session_id.clone(),
        timestamp: chrono::Utc::now(),
        tool: ctx.tool_name.to_string(),
        role: role.to_string(),
        session_class: ctx.task_type.map(str::to_string),
        denial_class: denial_class.to_string(),
        no_provider_launch: true,
        provider_side_effects: false,
        head_sha: ctx.session.git_head_at_creation.clone(),
        scope: None,
        range: None,
        memory,
        guidance,
    }
}

pub(crate) fn role_from_task_type(task_type: Option<&str>) -> &'static str {
    match task_type {
        Some("reviewer_sub_session" | "review_fix_finding" | "review") => "reviewer",
        Some("run") | None => "writer",
        Some(_) => "session",
    }
}

fn host_memory_guidance(
    task_type: Option<&str>,
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
) -> Vec<String> {
    match role_from_task_type(task_type) {
        "reviewer" => {
            let mut guidance = vec![
                "Host memory admission failed before the reviewer provider launched; this is infrastructure no-verdict, not PASS/FAIL.".to_string(),
                "Host admission uses physical MemAvailable only; swap/combined memory is diagnostic and is not counted because launching a reviewer into swap can still OOM or terminate before a verdict.".to_string(),
            ];
            if let Some(retry) = suggested_host_memory_retry(tool_name, memory) {
                guidance.push(retry);
            } else {
                guidance.push("Retry the same cap with a lower --min-free-memory-mb only when the host reserve is intentionally conservative; otherwise wait/free memory or use another configured reviewer.".to_string());
            }
            guidance.push(
                "If no reviewer fallback runs, fail closed and use native/manual review after one bounded retry."
                    .to_string(),
            );
            guidance
        }
        "writer" => vec![
            "Host memory admission failed before the writer provider launched; no provider-side worktree mutation occurred.".to_string(),
            "Host admission uses physical MemAvailable only; swap/combined memory is diagnostic and is not counted toward the pre-spawn gate.".to_string(),
            "Lower only the reserve when safe, wait/free memory, or reduce the projected spawn cap without dropping below the role soft-limit floor.".to_string(),
            "For dirty-work recovery, avoid repeated CSA retries in the same memory envelope; after one same-class retry, switch to documented recovery/manual fallback.".to_string(),
        ],
        _ => vec![
            "Host memory admission failed before the provider launched; retry after freeing memory or lowering only safe resource overrides.".to_string(),
        ],
    }
}

fn suggested_host_memory_retry(
    tool_name: &str,
    memory: &NoProviderLaunchMemoryDiagnostic,
) -> Option<String> {
    let required_cap_mb = memory.required_memory_max_mb?;
    let required_floor_mb = memory.required_floor_mb?;
    let soft_limit_percent = memory.soft_limit_percent?;
    let available_mb = memory.available_memory_mb?;
    let suggested_cap_mb = round_up_to_100(required_cap_mb);
    let suggested_soft_threshold_mb =
        memory_policy::soft_limit_threshold_mb(suggested_cap_mb, soft_limit_percent)?;
    if suggested_soft_threshold_mb < required_floor_mb || available_mb <= suggested_cap_mb {
        return None;
    }

    let max_reserve_mb = available_mb.saturating_sub(suggested_cap_mb);
    let current_reserve_mb = memory.reserve_mb.unwrap_or(max_reserve_mb);
    let preferred_reserve_mb = current_reserve_mb.min(6_000);
    let suggested_reserve_mb = if max_reserve_mb >= preferred_reserve_mb {
        preferred_reserve_mb
    } else {
        round_down_to_100(max_reserve_mb)
    };
    if suggested_reserve_mb == 0 {
        return None;
    }

    let host_required_mb = suggested_cap_mb.saturating_add(suggested_reserve_mb);
    if host_required_mb > available_mb {
        return None;
    }

    Some(format!(
        "Suggested bounded retry for {tool_name}: --memory-max-mb {suggested_cap_mb} \
         --min-free-memory-mb {suggested_reserve_mb} (minimum cap for \
         soft_limit_percent={soft_limit_percent} is {required_cap_mb}MB; suggested soft \
         threshold {suggested_soft_threshold_mb}MB >= required floor {required_floor_mb}MB; \
         host required {host_required_mb}MB <= physical available {available_mb}MB)."
    ))
}

fn round_up_to_100(value: u64) -> u64 {
    value.saturating_add(99) / 100 * 100
}

fn round_down_to_100(value: u64) -> u64 {
    value / 100 * 100
}

pub(crate) fn enrich_review_diagnostic(
    diagnostic: &mut NoProviderLaunchDiagnostic,
    head_sha: &str,
    scope: &str,
) {
    if diagnostic.head_sha.as_deref().is_none_or(str::is_empty) && !head_sha.is_empty() {
        diagnostic.head_sha = Some(head_sha.to_string());
    }
    diagnostic.scope = Some(scope.to_string());
    diagnostic.range = scope.strip_prefix("range:").map(str::to_string);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_memory_reviewer_guidance_suggests_soft_limit_safe_retry_pair() {
        let memory = NoProviderLaunchMemoryDiagnostic {
            effective_memory_max_mb: Some(10_000),
            soft_limit_percent: Some(90),
            soft_threshold_mb: Some(9_000),
            required_floor_mb: Some(8_192),
            required_memory_max_mb: Some(9_103),
            reserve_mb: Some(9_000),
            available_memory_mb: Some(17_033),
            required_available_mb: Some(19_000),
            projected_spawn_mb: Some(10_000),
            ..Default::default()
        };

        let guidance = host_memory_guidance(Some("reviewer_sub_session"), "codex", &memory);
        let joined = guidance.join("\n");

        assert!(joined.contains("physical MemAvailable only"));
        assert!(joined.contains("swap/combined memory is diagnostic"));
        assert!(joined.contains("--memory-max-mb 9200 --min-free-memory-mb 6000"));
        assert!(joined.contains("minimum cap for soft_limit_percent=90 is 9103MB"));
        assert!(joined.contains("soft threshold 8280MB >= required floor 8192MB"));
        assert!(joined.contains("host required 15200MB <= physical available 17033MB"));
    }
}
