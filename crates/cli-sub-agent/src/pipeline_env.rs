use std::collections::HashMap;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;

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
    materialize_ambient_rust_env_inputs(&mut merged_env);
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
    force_project_env_path(
        env,
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        &project_root.join("target"),
    );
    force_project_env_path(
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
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
    ] {
        let Some(value) = env.get(key).filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let path = PathBuf::from(value.as_str());
        push_unique_absolute_path(&mut paths, path.clone());
        if key == csa_core::env::CARGO_HOME_ENV_KEY {
            for child in ["git", "registry"] {
                push_unique_absolute_path(&mut paths, path.join(child));
            }
        }
    }
    paths
}

fn push_unique_absolute_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_absolute() && !paths.contains(&path) {
        paths.push(path);
    }
}

fn materialize_ambient_rust_env_inputs(env: &mut HashMap<String, String>) {
    for key in [
        "HOME",
        csa_core::env::CARGO_HOME_ENV_KEY,
        csa_core::env::RUSTUP_HOME_ENV_KEY,
        csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
        csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
        csa_core::env::MISE_CONFIG_DIR_ENV_KEY,
        csa_core::env::MISE_DATA_DIR_ENV_KEY,
    ] {
        if env.contains_key(key) {
            continue;
        }
        let Some(value) = std::env::var_os(key).filter(|value| !value.is_empty()) else {
            continue;
        };
        env.insert(key.to_string(), value.to_string_lossy().into_owned());
    }
}

fn apply_rust_session_env_contract(env: &mut HashMap<String, String>, project_root: Option<&Path>) {
    apply_rust_session_env_contract_inner(env, project_root, true);
}

fn apply_rust_session_env_contract_inner(
    env: &mut HashMap<String, String>,
    project_root: Option<&Path>,
    materialize_cargo_install_root: bool,
) {
    let home = env_path(env, "HOME");
    let cargo_home = preferred_cargo_home(home.as_deref(), project_root);
    if let Some(cargo_home) = cargo_home.as_deref() {
        ensure_rust_env_path(env, csa_core::env::CARGO_HOME_ENV_KEY, cargo_home);
    }

    if materialize_cargo_install_root {
        if let Some(project_root) = project_root {
            force_project_env_path(
                env,
                csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
                &project_root.join("target/cargo-install-root"),
            );
            force_project_env_path(
                env,
                csa_core::env::CARGO_TARGET_DIR_ENV_KEY,
                &project_root.join("target"),
            );
        } else if let Some(effective_cargo_home) =
            env_path(env, csa_core::env::CARGO_HOME_ENV_KEY).or(cargo_home)
        {
            let cargo_install_root = preferred_cargo_install_root(effective_cargo_home.as_path());
            ensure_rust_env_path(
                env,
                csa_core::env::CARGO_INSTALL_ROOT_ENV_KEY,
                &cargo_install_root,
            );
        }
    }

    let Some(home) = home else {
        return;
    };
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

fn preferred_cargo_home(home: Option<&Path>, project_root: Option<&Path>) -> Option<PathBuf> {
    preferred_cargo_home_with_shared(home, project_root, Path::new(SHARED_CARGO_HOME))
}

fn preferred_cargo_home_with_shared(
    home: Option<&Path>,
    project_root: Option<&Path>,
    shared: &Path,
) -> Option<PathBuf> {
    if shared.is_dir() && !csa_core::env::rust_state_path_needs_session_override(shared) {
        return Some(shared.to_path_buf());
    }

    home.map(|home| home.join(".cargo"))
        .or_else(|| project_root.map(|root| root.join("target/cargo-home")))
}

fn preferred_cargo_install_root(effective_cargo_home: &Path) -> PathBuf {
    effective_cargo_home.to_path_buf()
}

fn ensure_rust_env_path(env: &mut HashMap<String, String>, key: &str, fallback: &Path) {
    let effective = env_path(env, key)
        .filter(|current| !csa_core::env::rust_state_path_needs_session_override(current))
        .unwrap_or_else(|| fallback.to_path_buf());
    env.insert(key.to_string(), effective.to_string_lossy().into_owned());
}

fn force_project_env_path(env: &mut HashMap<String, String>, key: &str, fallback: &Path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_cargo_home_without_shared_or_home_uses_target_backed_fallback() {
        let project = tempfile::tempdir().expect("tempdir");
        let shared = project.path().join("missing-shared-cache");

        let cargo_home = preferred_cargo_home_with_shared(None, Some(project.path()), &shared)
            .expect("target-backed fallback");

        assert_eq!(cargo_home, project.path().join("target/cargo-home"));
        assert_ne!(cargo_home, project.path().join(".cargo-local"));
        assert_ne!(cargo_home, Path::new("/usr/local"));
    }
}
