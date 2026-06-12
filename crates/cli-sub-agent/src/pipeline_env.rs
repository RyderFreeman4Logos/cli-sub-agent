use std::collections::HashMap;
use std::path::Path;

use csa_config::ProjectConfig;
use tracing::info;

pub(crate) const CSA_GIT_PUSH_ALLOWED_ENV: &str = csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY;
pub(crate) const CSA_RUN_GIT_PUSH_AUTHORIZED_ENV: &str =
    csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY;

/// Resolve effective cooldown seconds from config or default.
pub(crate) fn resolve_cooldown_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .map(|c| c.session.cooldown_seconds)
        .unwrap_or(csa_config::DEFAULT_COOLDOWN_SECS)
}

pub(crate) struct MergedEnvRequest<'a> {
    pub(crate) extra_env: Option<&'a HashMap<String, String>>,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: Option<&'a csa_config::GlobalConfig>,
    pub(crate) tool_name: &'a str,
    pub(crate) current_depth: u32,
    pub(crate) pattern_internal: bool,
    pub(crate) allow_git_push: bool,
}

pub(crate) fn build_merged_env(request: MergedEnvRequest<'_>) -> HashMap<String, String> {
    let MergedEnvRequest {
        extra_env,
        config,
        global_config,
        tool_name,
        current_depth,
        pattern_internal,
        allow_git_push,
    } = request;
    let suppress = config
        .map(|c| c.should_suppress_notify(tool_name))
        .unwrap_or(true);

    let mut merged_env = extra_env.cloned().unwrap_or_default();
    csa_core::env::scrub_subtree_contract_env_map(&mut merged_env);
    csa_core::env::strip_git_push_authorization_keys(&mut merged_env);
    if !merged_env.contains_key("PATH")
        && let Some(path) = std::env::var_os("PATH")
    {
        merged_env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }
    if suppress {
        merged_env.insert("CSA_SUPPRESS_NOTIFY".to_string(), "1".to_string());
    }

    if let Some(limit_mb) = config.and_then(|c| c.sandbox_node_heap_limit_mb(tool_name)) {
        let heap_flag = format!("--max-old-space-size={limit_mb}");
        merged_env
            .entry("NODE_OPTIONS".to_string())
            .and_modify(|value| {
                if value.is_empty() {
                    *value = heap_flag.clone();
                } else {
                    value.push(' ');
                    value.push_str(&heap_flag);
                }
            })
            .or_insert(heap_flag);
    }

    if tool_name == "gemini-cli" || tool_name == "antigravity-cli" {
        let allow_degraded_mcp = global_config.is_none_or(|gc| gc.allow_degraded_mcp(tool_name));
        merged_env.insert(
            "CSA_GEMINI_ALLOW_DEGRADED_MCP".to_string(),
            if allow_degraded_mcp { "1" } else { "0" }.to_string(),
        );
        #[cfg(test)]
        merged_env.insert(
            "CSA_TEST_DISABLE_GEMINI_DIRECT_LAUNCH".to_string(),
            "1".to_string(),
        );
    }

    if tool_name == "openai-compat"
        && let Some(cfg) = config
        && let Some(tool_cfg) = cfg.tools.get("openai-compat")
    {
        if let Some(base_url) = tool_cfg
            .base_url
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            merged_env
                .entry("OPENAI_COMPAT_BASE_URL".to_string())
                .or_insert_with(|| base_url.clone());
        }
        if let Some(api_key) = tool_cfg
            .api_key
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            merged_env
                .entry("OPENAI_COMPAT_API_KEY".to_string())
                .or_insert_with(|| api_key.clone());
        }
    }

    merged_env.insert(
        csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
        current_depth.saturating_add(1).to_string(),
    );
    merged_env.insert(
        csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(),
        "1".to_string(),
    );
    // Propagate the pattern-internal marker to the leaf tool (and thence to any
    // nested `csa` it spawns) when this session is itself pattern-internal. The
    // key is NOT in the scrubbed subtree contract, so it survives the transport
    // env filter rather than being stripped (#1847).
    if pattern_internal {
        merged_env.insert(
            csa_core::env::CSA_PATTERN_INTERNAL_ENV_KEY.to_string(),
            "1".to_string(),
        );
    }
    if allow_git_push {
        merged_env.insert(CSA_GIT_PUSH_ALLOWED_ENV.to_string(), "true".to_string());
    }

    merged_env
}

pub(crate) fn apply_review_target_dir(project_root: &Path, tool_name: &str) {
    let repo_target_dir = project_root.join("target");
    if let Some(target_kind) = detect_project_target_kind(&repo_target_dir) {
        info!(
            project_target = %repo_target_dir.display(),
            tool = tool_name,
            target_kind,
            "honoring user ./target ({target_kind}), CARGO_TARGET_DIR untouched"
        );
        return;
    }

    info!(
        project_target = %repo_target_dir.display(),
        tool = tool_name,
        "no ./target present, CARGO_TARGET_DIR left at codex/cargo default"
    );
}

pub(crate) fn apply_task_target_dir_guards(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
) {
    if matches!(task_type, Some("review")) {
        apply_review_target_dir(project_root, tool_name);
    }
    apply_run_target_dir_guard(task_type, tool_name, project_root, merged_env);
}

pub(crate) fn apply_run_target_dir_guard(
    task_type: Option<&str>,
    tool_name: &str,
    project_root: &Path,
    merged_env: &mut HashMap<String, String>,
) {
    if !matches!(task_type, Some("run")) || tool_name != "codex" {
        return;
    }

    let _ = merged_env;

    let repo_target_dir = project_root.join("target");
    let user_configured_target = std::fs::symlink_metadata(&repo_target_dir).is_ok();

    if user_configured_target {
        info!(
            project_target = %repo_target_dir.display(),
            "Run session: ./target already configured by user (detected symlink/dir), leaving CARGO_TARGET_DIR untouched"
        );
        return;
    }

    info!(
        project_target = %repo_target_dir.display(),
        "Run session: no user-configured ./target, leaving codex default CARGO_TARGET_DIR behavior (no CSA override)"
    );
}

fn detect_project_target_kind(repo_target_dir: &Path) -> Option<&'static str> {
    let metadata = std::fs::symlink_metadata(repo_target_dir).ok()?;
    if metadata.file_type().is_symlink() {
        return Some("symlink");
    }
    if metadata.is_dir() {
        return Some("dir");
    }
    None
}
