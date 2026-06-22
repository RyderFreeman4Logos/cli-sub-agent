use std::collections::HashMap;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;
use tracing::info;

pub(crate) const CSA_GIT_PUSH_ALLOWED_ENV: &str = csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY;
pub(crate) const CSA_RUN_GIT_PUSH_AUTHORIZED_ENV: &str =
    csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY;
const SHARED_CARGO_HOME: &str = "/usr/local/share/cargo";

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
    pub(crate) project_root: Option<&'a Path>,
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
        project_root,
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
    apply_rust_session_env_contract(&mut merged_env, project_root);
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

pub(crate) fn apply_rust_gate_env_contract(env: &mut HashMap<String, String>, project_root: &Path) {
    apply_rust_session_env_contract_inner(env, Some(project_root), false);
    ensure_project_env_path(
        env,
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        &project_root.join("target"),
    );
    ensure_project_env_path(
        env,
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        &project_root.join("target/cargo-install-root"),
    );
}

pub(crate) fn rust_session_writable_paths(env: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for key in [
        csa_core::env::CARGO_HOME_ENV_KEY,
        csa_core::env::RUSTUP_HOME_ENV_KEY,
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
    ] {
        let Some(value) = env.get(key).filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let path = PathBuf::from(value.as_str());
        if path.is_absolute() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn apply_rust_session_env_contract(env: &mut HashMap<String, String>, project_root: Option<&Path>) {
    apply_rust_session_env_contract_inner(env, project_root, true);
}

fn apply_rust_session_env_contract_inner(
    env: &mut HashMap<String, String>,
    project_root: Option<&Path>,
    materialize_cargo_install_root: bool,
) {
    let Some(home) = env_path(env, "HOME") else {
        return;
    };
    let cargo_home = preferred_cargo_home(&home, project_root);
    ensure_rust_env_path(env, csa_core::env::CARGO_HOME_ENV_KEY, &cargo_home);

    if materialize_cargo_install_root {
        let effective_cargo_home =
            env_path(env, csa_core::env::CARGO_HOME_ENV_KEY).unwrap_or(cargo_home);
        let cargo_install_root =
            preferred_cargo_install_root(project_root, effective_cargo_home.as_path());
        ensure_rust_env_path(
            env,
            csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
            &cargo_install_root,
        );
    }

    let rustup_home = preferred_rustup_home(env, &home);
    ensure_rust_env_path(env, csa_core::env::RUSTUP_HOME_ENV_KEY, &rustup_home);
    let effective_rustup_home =
        env_path(env, csa_core::env::RUSTUP_HOME_ENV_KEY).unwrap_or(rustup_home);
    if let Some(project_root) = project_root {
        maybe_prepend_real_rust_toolchain(env, project_root, &effective_rustup_home);
    }

    let mise_config_dir = home.join(".config/mise");
    ensure_rust_env_path(
        env,
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
        &mise_config_dir,
    );
}

fn preferred_cargo_home(home: &Path, project_root: Option<&Path>) -> PathBuf {
    let shared = Path::new(SHARED_CARGO_HOME);
    if shared.is_dir() && !csa_core::env::rust_state_path_needs_session_override(shared) {
        return shared.to_path_buf();
    }

    project_root
        .map(|root| root.join(".cargo-local"))
        .unwrap_or_else(|| home.join(".cargo"))
}

fn preferred_cargo_install_root(
    project_root: Option<&Path>,
    effective_cargo_home: &Path,
) -> PathBuf {
    project_root
        .map(|root| root.join("target/cargo-install-root"))
        .unwrap_or_else(|| effective_cargo_home.to_path_buf())
}

fn ensure_rust_env_path(env: &mut HashMap<String, String>, key: &str, fallback: &Path) {
    let effective = env_path(env, key)
        .filter(|current| !csa_core::env::rust_state_path_needs_session_override(current))
        .unwrap_or_else(|| fallback.to_path_buf());
    env.insert(key.to_string(), effective.to_string_lossy().into_owned());
}

fn ensure_project_env_path(env: &mut HashMap<String, String>, key: &str, fallback: &Path) {
    if let Some(value) = env.get(key).filter(|value| !value.trim().is_empty()) {
        if csa_core::env::rust_state_path_needs_session_override(Path::new(value.as_str())) {
            env.insert(key.to_string(), fallback.to_string_lossy().into_owned());
        }
        return;
    }

    if let Some(value) = std::env::var_os(key).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if csa_core::env::rust_state_path_needs_session_override(&path) {
            env.insert(key.to_string(), fallback.to_string_lossy().into_owned());
        }
        return;
    }

    env.insert(key.to_string(), fallback.to_string_lossy().into_owned());
}

fn preferred_rustup_home(env: &HashMap<String, String>, home: &Path) -> PathBuf {
    for candidate in mise_rust_home_candidates(env) {
        if candidate.join("settings.toml").is_file()
            && candidate.join("toolchains").is_dir()
            && !csa_core::env::rust_state_path_needs_session_override(&candidate)
        {
            return candidate;
        }
    }
    home.join(".rustup")
}

fn mise_rust_home_candidates(env: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for data_dir in env_path(env, csa_core::env::MISE_DATA_DIR_ENV_KEY)
        .into_iter()
        .chain(Some(PathBuf::from("/usr/local/share/mise")))
    {
        let stable = data_dir.join("installs/rust/stable");
        if !candidates.contains(&stable) {
            candidates.push(stable);
        }
    }
    candidates
}

fn maybe_prepend_real_rust_toolchain(
    env: &mut HashMap<String, String>,
    project_root: &Path,
    rustup_home: &Path,
) {
    let Some(toolchain_bin) = real_rust_toolchain_bin(project_root, rustup_home) else {
        return;
    };
    let current_path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let mut paths = std::env::split_paths(&current_path).collect::<Vec<_>>();
    if paths.iter().any(|path| path == &toolchain_bin) {
        return;
    }
    paths.insert(0, toolchain_bin);
    let Ok(joined) = std::env::join_paths(paths) else {
        return;
    };
    env.insert("PATH".to_string(), joined.to_string_lossy().into_owned());
}

fn real_rust_toolchain_bin(project_root: &Path, rustup_home: &Path) -> Option<PathBuf> {
    let channel = rust_toolchain_channel(project_root)?;
    let prefix = format!("{channel}-");
    let toolchains = rustup_home.join("toolchains");
    for entry in std::fs::read_dir(toolchains).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == channel || name.starts_with(&prefix) {
            let bin = entry.path().join("bin");
            if bin.join("cargo").is_file() {
                return Some(bin);
            }
        }
    }
    None
}

fn rust_toolchain_channel(project_root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(project_root.join("rust-toolchain.toml")).ok()?;
    let document = content.parse::<toml_edit::DocumentMut>().ok()?;
    document["toolchain"]["channel"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn env_path(env: &HashMap<String, String>, key: &str) -> Option<PathBuf> {
    env.get(key)
        .filter(|value| !value.trim().is_empty())
        .map(|value| PathBuf::from(value.as_str()))
        .or_else(|| {
            std::env::var_os(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
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
