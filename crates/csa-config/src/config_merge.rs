use anyhow::{Result, bail};

use crate::{ConvergenceCompletionPolicy, parse_project_convergence_completion_policy};

/// Warn about deprecated config keys that serde silently ignores.
pub(crate) fn warn_deprecated_keys(raw: &toml::Value, source: &str) {
    if let Some(resources) = raw.get("resources")
        && resources.get("min_free_swap_mb").is_some()
    {
        eprintln!(
            "warning: config '{source}': 'resources.min_free_swap_mb' is deprecated and ignored. \
                 Use 'resources.min_free_memory_mb' (physical MemAvailable threshold) instead."
        );
    }
    if let Some(session) = raw.get("session")
        && session.get("daemon_wait_seconds").is_some()
    {
        eprintln!(
            "warning: config '{source}': 'session.daemon_wait_seconds' is deprecated. \
             Move this to global '[kv_cache].long_poll_seconds'; CSA currently keeps the legacy key only for migration compatibility."
        );
    }
}

/// Reject project-level attempts to enable global-only tier bypass policy.
///
/// `[tier_policy].allow_force_bypass` grants permission to bypass configured
/// tiers with exact model specs and force flags. Only the global config may set
/// it; allowing a project overlay to set this would let a repository grant
/// itself the bypass.
pub(crate) fn reject_project_tier_policy(raw: &toml::Value, source: &str) -> Result<()> {
    let Some(tier_policy) = raw.get("tier_policy").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    if tier_policy.contains_key("allow_force_bypass") {
        bail!(
            "{source}: [tier_policy].allow_force_bypass is global-only. \
             Set it in ~/.config/cli-sub-agent/config.toml, not project .csa/config.toml."
        );
    }
    Ok(())
}

/// Reject project-level completion settings that would expand the global safety ceiling.
///
/// Unlike normal configuration fields, completion authority composes by intersection. The
/// project may set an already-permitted boolean to false or lower the retention cap, but cannot
/// enable an authority denied globally or retain evidence for longer than the global cap.
pub(crate) fn reject_project_convergence_completion_policy(
    global: Option<&toml::Value>,
    project: &toml::Value,
    source: &str,
) -> Result<()> {
    let Some(project_policy) = parse_project_convergence_completion_policy(project)? else {
        return Ok(());
    };

    let global_policy = global
        .and_then(|raw| raw.get("convergence_completion"))
        .map(|value| value.clone().try_into())
        .transpose()?
        .unwrap_or_else(ConvergenceCompletionPolicy::default);

    for (key, requested, globally_allowed) in [
        (
            "allow_execution",
            project_policy.allow_execution,
            global_policy.allow_execution,
        ),
        (
            "allow_provider_egress",
            project_policy.allow_provider_egress,
            global_policy.allow_provider_egress,
        ),
        (
            "allow_shell_commands",
            project_policy.allow_shell_commands,
            global_policy.allow_shell_commands,
        ),
        (
            "allow_credential_inheritance",
            project_policy.allow_credential_inheritance,
            global_policy.allow_credential_inheritance,
        ),
    ] {
        if requested == Some(true) && !globally_allowed {
            bail!(
                "{source}: [convergence_completion].{key} cannot expand the global safety ceiling"
            );
        }
    }

    if let Some(retention_days) = project_policy.max_retention_days
        && retention_days > global_policy.max_retention_days
    {
        bail!(
            "{source}: [convergence_completion].max_retention_days cannot exceed the global safety ceiling"
        );
    }
    Ok(())
}

/// Deep merge two TOML values. Overlay wins for non-table values.
/// Tables are merged recursively (project-level keys override user-level keys).
pub(crate) fn merge_toml_values(base: toml::Value, overlay: toml::Value) -> toml::Value {
    match (base, overlay) {
        (toml::Value::Table(mut base_map), toml::Value::Table(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged_val = match base_map.remove(&key) {
                    Some(base_val) => merge_toml_values(base_val, overlay_val),
                    None => overlay_val,
                };
                base_map.insert(key, merged_val);
            }
            toml::Value::Table(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// Project-only keys under `[review]`. These fields are meaningful only in
/// project config; values from global config are stripped before merge to
/// prevent accidental inheritance.
const REVIEW_PROJECT_ONLY_KEYS: &[&str] = &["gate_command", "gate_commands", "gate_timeout_secs"];

/// Strip project-only keys from the global `[review]` table before merge.
///
/// Some review config fields (e.g. `gate_command`, `gate_timeout_secs`) are
/// project-specific and must not be inherited from the global config.
/// If the global config sets them, emit a warning and remove them so the
/// merge only preserves values from the project config.
pub(crate) fn strip_review_project_only_from_global(global: &mut toml::Value) {
    let review_table = match global.get_mut("review").and_then(|t| t.as_table_mut()) {
        Some(t) => t,
        None => return,
    };

    for key in REVIEW_PROJECT_ONLY_KEYS {
        if review_table.remove(*key).is_some() {
            tracing::warn!(
                key = *key,
                "Global config sets review.{} which is project-only; ignoring global value",
                key
            );
        }
    }
}

/// Re-apply `tools.*.enabled = false` from the global config into a merged
/// TOML value.  This ensures that global disablement is a hard override:
/// project configs cannot set a globally-disabled tool back to `enabled = true`.
pub(crate) fn enforce_global_tool_disables(global: &toml::Value, merged: &mut toml::Value) {
    let global_tools = match global.get("tools").and_then(|t| t.as_table()) {
        Some(t) => t,
        None => return,
    };
    let merged_tools = match merged.get_mut("tools").and_then(|t| t.as_table_mut()) {
        Some(t) => t,
        None => return,
    };

    for (tool_name, global_tool_val) in global_tools {
        let globally_disabled =
            global_tool_val.get("enabled").and_then(|v| v.as_bool()) == Some(false);
        if !globally_disabled {
            continue;
        }
        // Force `enabled = false` in the merged config for this tool.
        if let Some(merged_tool) = merged_tools.get_mut(tool_name)
            && let Some(table) = merged_tool.as_table_mut()
        {
            table.insert("enabled".to_string(), toml::Value::Boolean(false));
        }
    }
}
