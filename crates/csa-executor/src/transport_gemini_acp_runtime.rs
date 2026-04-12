use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use csa_resource::isolation_plan::DEFAULT_SANDBOX_TMPDIR;
use serde_json::{Map, Value};
use tracing::{debug, warn};

const GEMINI_RUNTIME_ROOT_DIR: &str = "cli-sub-agent-gemini";
const GEMINI_SESSION_RUNTIME_RELATIVE_PATH: &str = "runtime/gemini-home";
const CSA_FS_SANDBOXED_ENV: &str = "CSA_FS_SANDBOXED";
const CSA_SESSION_DIR_ENV: &str = "CSA_SESSION_DIR";
const GEMINI_RUNTIME_SETTINGS_PATHS: &[&str] =
    &[".gemini/settings.json", ".config/gemini-cli/settings.json"];
const GEMINI_RUNTIME_PINNED_PATH_BINARIES: &[&str] = &["node", "yarn", "gemini"];
const GEMINI_RUNTIME_SHIM_ENV_VARS: &[&str] = &["MISE_SHIM", "MISE_SHIMS_DIR"];
const GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH: &str = ".cache/mise";
const GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH: &str = ".local/state/mise";
const GEMINI_MIRROR_FILES: &[&str] = &[
    "oauth_creds.json",
    "google_accounts.json",
    "settings.json",
    "settings.json.orig",
    "trustedFolders.json",
    "trusted_hooks.json",
    "mcp-oauth-tokens-v2.json",
    "installation_id",
    "state.json",
];
const GEMINI_MIRROR_DIRS: &[&str] = &["extensions", "antigravity"];
const GEMINI_SELECTED_TYPE_OAUTH: &str = "oauth-personal";
const GEMINI_SELECTED_TYPE_API_KEY: &str = "gemini-api-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeminiAcpLaunch {
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
}

pub(crate) fn prepare_gemini_acp_runtime(
    env: &mut HashMap<String, String>,
    project_dir: Option<&Path>,
    session_dir: Option<&Path>,
    session_id: &str,
    base_args: &[String],
) -> Result<GeminiAcpLaunch> {
    let source_home = env
        .get("HOME")
        .cloned()
        .or_else(|| std::env::var("HOME").ok())
        .map(PathBuf::from);
    let runtime_session_dir = session_dir
        .map(Path::to_path_buf)
        .or_else(|| runtime_session_dir_from_env(env));
    let tmpdir = normalize_tmpdir_env(env);
    let runtime_home = resolve_runtime_home(runtime_session_dir.as_deref(), session_id, &tmpdir);
    seed_runtime_home(&runtime_home, source_home.as_deref())?;
    align_runtime_auth_selection(&runtime_home, env)?;

    let runtime_home_str = runtime_home.to_string_lossy().into_owned();
    env.insert("GEMINI_CLI_HOME".to_string(), runtime_home_str.clone());
    env.insert("HOME".to_string(), runtime_home_str.clone());
    env.insert(
        "XDG_CONFIG_HOME".to_string(),
        runtime_home.join(".config").to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        runtime_home.join(".cache").to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_STATE_HOME".to_string(),
        runtime_home
            .join(".local")
            .join("state")
            .to_string_lossy()
            .into_owned(),
    );
    env.insert(
        "MISE_CACHE_DIR".to_string(),
        runtime_home
            .join(GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH)
            .to_string_lossy()
            .into_owned(),
    );
    env.insert(
        "MISE_STATE_DIR".to_string(),
        runtime_home
            .join(GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH)
            .to_string_lossy()
            .into_owned(),
    );

    let inherited_path = env
        .get("PATH")
        .map(OsStr::new)
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));
    if let Some(path) = pin_non_shim_runtime_path(inherited_path.as_deref(), env, project_dir)? {
        env.insert("PATH".to_string(), path);
    }
    for key in GEMINI_RUNTIME_SHIM_ENV_VARS {
        env.insert((*key).to_string(), String::new());
    }

    let path_env = env
        .get("PATH")
        .map(OsStr::new)
        .map(std::ffi::OsString::from)
        .or(inherited_path);

    if let Some(launch) =
        resolve_non_shim_gemini_launch(path_env.as_deref(), env, project_dir, base_args)?
    {
        debug!(
            command = %launch.command,
            runtime_home = %runtime_home.display(),
            "prepared gemini ACP runtime using direct launch"
        );
        return Ok(launch);
    }

    debug!(
        runtime_home = %runtime_home.display(),
        "prepared gemini ACP runtime without direct launch override"
    );
    Ok(GeminiAcpLaunch {
        command: "gemini".to_string(),
        args: base_args.to_vec(),
    })
}

pub(crate) fn gemini_runtime_home_from_env(env: &HashMap<String, String>) -> Option<PathBuf> {
    let runtime_root = runtime_root_from_env(env);
    let session_relative_path = Path::new(GEMINI_SESSION_RUNTIME_RELATIVE_PATH);
    let candidates = [
        env.get("GEMINI_CLI_HOME").map(PathBuf::from),
        env.get("HOME").map(PathBuf::from),
        env.get("XDG_CONFIG_HOME")
            .map(Path::new)
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(PathBuf::from),
        env.get("XDG_CACHE_HOME")
            .map(Path::new)
            .and_then(Path::parent)
            .map(PathBuf::from),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.starts_with(&runtime_root) || path.ends_with(session_relative_path))
}

fn resolve_runtime_home(session_dir: Option<&Path>, session_id: &str, tmpdir: &Path) -> PathBuf {
    if let Some(session_dir) = session_dir {
        return session_dir.join(GEMINI_SESSION_RUNTIME_RELATIVE_PATH);
    }

    tmpdir.join(GEMINI_RUNTIME_ROOT_DIR).join(session_id)
}

fn normalize_tmpdir_env(env: &mut HashMap<String, String>) -> PathBuf {
    if is_filesystem_sandboxed(env) {
        let resolved = PathBuf::from(DEFAULT_SANDBOX_TMPDIR);
        env.insert(
            "TMPDIR".to_string(),
            resolved.to_string_lossy().into_owned(),
        );
        return resolved;
    }

    let candidate = env
        .get("TMPDIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from));

    let resolved = match candidate {
        Some(path) if tmpdir_probe_writable(&path).is_ok() => path,
        Some(path) => {
            warn!(
                tmpdir = %path.display(),
                fallback = DEFAULT_SANDBOX_TMPDIR,
                "TMPDIR is not writable for Gemini ACP runtime; falling back to /tmp"
            );
            PathBuf::from(DEFAULT_SANDBOX_TMPDIR)
        }
        None => PathBuf::from(DEFAULT_SANDBOX_TMPDIR),
    };

    env.insert(
        "TMPDIR".to_string(),
        resolved.to_string_lossy().into_owned(),
    );
    resolved
}

fn runtime_session_dir_from_env(env: &HashMap<String, String>) -> Option<PathBuf> {
    env.get(CSA_SESSION_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn is_filesystem_sandboxed(env: &HashMap<String, String>) -> bool {
    env.get(CSA_FS_SANDBOXED_ENV)
        .is_some_and(|value| value == "1")
}

fn runtime_root_from_env(env: &HashMap<String, String>) -> PathBuf {
    env.get("TMPDIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SANDBOX_TMPDIR))
        .join(GEMINI_RUNTIME_ROOT_DIR)
}

fn tmpdir_probe_writable(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create TMPDIR candidate {}", path.display()))?;

    let probe_path = path.join(format!(
        ".csa-gemini-tmpdir-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
        .with_context(|| format!("failed to create TMPDIR probe {}", probe_path.display()))?;
    drop(file);
    let _ = fs::remove_file(&probe_path);
    Ok(())
}

fn seed_runtime_home(runtime_home: &Path, source_home: Option<&Path>) -> Result<()> {
    let gemini_dir = runtime_home.join(".gemini");
    fs::create_dir_all(&gemini_dir).with_context(|| {
        format!(
            "failed to create gemini runtime dir {}",
            gemini_dir.display()
        )
    })?;
    for directory in [
        runtime_home.join(".config").join("gemini-cli"),
        runtime_home.join(".cache"),
        runtime_home.join(GEMINI_RUNTIME_MISE_CACHE_RELATIVE_PATH),
        runtime_home.join(".local").join("state"),
        runtime_home.join(GEMINI_RUNTIME_MISE_STATE_RELATIVE_PATH),
    ] {
        fs::create_dir_all(&directory).with_context(|| {
            format!(
                "failed to create gemini runtime dir {}",
                directory.display()
            )
        })?;
    }

    for dir_name in ["history", "logs", "tmp"] {
        fs::create_dir_all(gemini_dir.join(dir_name)).with_context(|| {
            format!(
                "failed to create gemini runtime state dir {}",
                gemini_dir.join(dir_name).display()
            )
        })?;
    }

    let Some(source_home) = source_home else {
        return Ok(());
    };

    let source_gemini_dir = source_home.join(".gemini");
    let target_gemini_dir = runtime_home.join(".gemini");
    for file_name in GEMINI_MIRROR_FILES {
        copy_if_present(
            &source_gemini_dir.join(file_name),
            &target_gemini_dir.join(file_name),
        );
    }

    copy_tree_contents(
        &source_home.join(".config").join("gemini-cli"),
        &runtime_home.join(".config").join("gemini-cli"),
    );

    for dir_name in GEMINI_MIRROR_DIRS {
        mirror_directory_link(
            &source_gemini_dir.join(dir_name),
            &target_gemini_dir.join(dir_name),
        );
    }

    mirror_directory_link(&source_home.join(".agents"), &runtime_home.join(".agents"));

    Ok(())
}

fn align_runtime_auth_selection(runtime_home: &Path, env: &HashMap<String, String>) -> Result<()> {
    let Some(selected_type) = runtime_auth_selected_type(env) else {
        return Ok(());
    };

    for relative_path in GEMINI_RUNTIME_SETTINGS_PATHS {
        let settings_path = runtime_home.join(relative_path);
        write_runtime_selected_auth_type(&settings_path, selected_type)?;
    }

    debug!(
        runtime_home = %runtime_home.display(),
        selected_type,
        "aligned gemini runtime auth selection"
    );
    Ok(())
}

fn runtime_auth_selected_type(env: &HashMap<String, String>) -> Option<&'static str> {
    match env
        .get(csa_core::gemini::AUTH_MODE_ENV_KEY)
        .map(String::as_str)
    {
        Some(csa_core::gemini::AUTH_MODE_API_KEY) => Some(GEMINI_SELECTED_TYPE_API_KEY),
        Some(csa_core::gemini::AUTH_MODE_OAUTH) => Some(GEMINI_SELECTED_TYPE_OAUTH),
        _ if env.contains_key(csa_core::gemini::API_KEY_ENV) => Some(GEMINI_SELECTED_TYPE_API_KEY),
        _ => None,
    }
}

fn write_runtime_selected_auth_type(settings_path: &Path, selected_type: &str) -> Result<()> {
    let mut settings = load_runtime_settings(settings_path);
    let auth_settings = ensure_auth_settings_mut(&mut settings);
    auth_settings.insert(
        "selectedType".to_string(),
        Value::String(selected_type.to_string()),
    );
    auth_settings.insert(
        "enforcedType".to_string(),
        Value::String(selected_type.to_string()),
    );

    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create gemini runtime settings dir {}",
                parent.display()
            )
        })?;
    }

    let serialized =
        serde_json::to_string_pretty(&settings).context("failed to serialize gemini settings")?;
    fs::write(settings_path, format!("{serialized}\n")).with_context(|| {
        format!(
            "failed to write gemini runtime settings {}",
            settings_path.display()
        )
    })?;
    Ok(())
}

fn load_runtime_settings(settings_path: &Path) -> Value {
    let Ok(raw) = fs::read_to_string(settings_path) else {
        return Value::Object(Map::new());
    };

    match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            warn!(
                path = %settings_path.display(),
                error = %error,
                "failed to parse mirrored gemini settings; recreating runtime auth override"
            );
            Value::Object(Map::new())
        }
    }
}

fn ensure_auth_settings_mut(settings: &mut Value) -> &mut Map<String, Value> {
    let root = ensure_object_mut(settings);
    let security = root
        .entry("security".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let security = ensure_object_mut(security);
    let auth = security
        .entry("auth".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    ensure_object_mut(auth)
}

fn ensure_object_mut(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value
        .as_object_mut()
        .expect("value must be an object after initialization")
}

fn copy_if_present(source: &Path, target: &Path) {
    if !source.exists() || target.exists() {
        return;
    }

    if let Some(parent) = target.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        warn!(
            source = %source.display(),
            target = %target.display(),
            error = %error,
            "failed to create parent directory for mirrored gemini file"
        );
        return;
    }

    if let Err(error) = fs::copy(source, target) {
        warn!(
            source = %source.display(),
            target = %target.display(),
            error = %error,
            "failed to mirror gemini runtime file"
        );
    }
}

fn copy_tree_contents(source: &Path, target: &Path) {
    if !source.exists() {
        return;
    }

    let Ok(entries) = fs::read_dir(source) else {
        warn!(source = %source.display(), "failed to enumerate gemini config directory");
        return;
    };

    for entry in entries.flatten() {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else {
            warn!(source = %source_path.display(), "failed to inspect gemini config entry");
            continue;
        };

        if file_type.is_dir() {
            if let Err(error) = fs::create_dir_all(&target_path) {
                warn!(
                    source = %source_path.display(),
                    target = %target_path.display(),
                    error = %error,
                    "failed to create mirrored gemini config directory"
                );
                continue;
            }
            copy_tree_contents(&source_path, &target_path);
            continue;
        }

        copy_if_present(&source_path, &target_path);
    }
}

fn mirror_directory_link(source: &Path, target: &Path) {
    if !source.exists() || target.exists() {
        return;
    }

    #[cfg(unix)]
    {
        if let Some(parent) = target.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            warn!(
                source = %source.display(),
                target = %target.display(),
                error = %error,
                "failed to create parent directory for mirrored gemini symlink"
            );
            return;
        }

        if let Err(error) = std::os::unix::fs::symlink(source, target) {
            warn!(
                source = %source.display(),
                target = %target.display(),
                error = %error,
                "failed to mirror gemini runtime directory as symlink"
            );
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (source, target);
    }
}

fn pin_non_shim_runtime_path(
    path_env: Option<&OsStr>,
    env: &HashMap<String, String>,
    project_dir: Option<&Path>,
) -> Result<Option<String>> {
    let Some(path_env) = path_env else {
        return Ok(None);
    };

    let original_entries: Vec<PathBuf> = std::env::split_paths(path_env).collect();
    let mut pinned_dirs = Vec::new();
    for binary in GEMINI_RUNTIME_PINNED_PATH_BINARIES {
        if let Some(dir) = find_direct_tool_dir(binary, Some(path_env), env, project_dir)?
            && !pinned_dirs.contains(&dir)
        {
            pinned_dirs.push(dir);
        }
    }

    if pinned_dirs.is_empty() {
        return Ok(None);
    }

    let mut merged_entries = pinned_dirs;
    for entry in original_entries {
        if !merged_entries.contains(&entry) {
            merged_entries.push(entry);
        }
    }

    let joined = std::env::join_paths(merged_entries)
        .context("failed to join pinned Gemini runtime PATH")?;
    Ok(Some(joined.to_string_lossy().into_owned()))
}

fn find_direct_tool_dir(
    name: &str,
    path_env: Option<&OsStr>,
    env: &HashMap<String, String>,
    project_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    Ok(find_direct_tool_path(name, path_env, env, project_dir)?
        .and_then(|path| path.parent().map(PathBuf::from)))
}

fn find_path_entry(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;

    for directory in std::env::split_paths(path_env) {
        let candidate = directory.join(name);
        if !candidate.is_file() {
            continue;
        }

        return Some(candidate.canonicalize().unwrap_or(candidate));
    }

    None
}

fn find_direct_tool_path(
    name: &str,
    path_env: Option<&OsStr>,
    env: &HashMap<String, String>,
    project_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if let Some(candidate) = find_non_mise_path_entry(name, path_env) {
        return Ok(Some(candidate));
    }

    resolve_mise_which_path(name, path_env, env, project_dir)
}

fn find_non_mise_path_entry(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;

    for directory in std::env::split_paths(path_env) {
        let candidate = directory.join(name);
        if !candidate.is_file() {
            continue;
        }

        let canonical = candidate.canonicalize().unwrap_or(candidate);
        if canonical
            .file_name()
            .is_some_and(|file_name| file_name == OsStr::new("mise"))
        {
            continue;
        }

        return Some(canonical);
    }

    None
}

fn resolve_mise_which_path(
    name: &str,
    path_env: Option<&OsStr>,
    env: &HashMap<String, String>,
    project_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let Some(mise_path) = find_path_entry("mise", path_env) else {
        return Ok(None);
    };

    let mut command = Command::new(&mise_path);
    if let Some(project_dir) = project_dir {
        command.arg("-C").arg(project_dir);
    }
    command.arg("which").arg(name);
    for key in [
        "HOME",
        "PATH",
        "TMPDIR",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "MISE_CACHE_DIR",
        "MISE_STATE_DIR",
    ] {
        if let Some(value) = env.get(key) {
            command.env(key, value);
        }
    }

    let output = command.output().with_context(|| {
        format!("failed to resolve Gemini runtime binary `{name}` via mise which")
    })?;
    if !output.status.success() {
        debug!(
            binary = name,
            status = ?output.status.code(),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "mise which did not resolve a direct Gemini runtime binary"
        );
        return Ok(None);
    }

    let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if resolved.is_empty() {
        return Ok(None);
    }

    let resolved_path = PathBuf::from(resolved);
    if !resolved_path.exists() {
        return Ok(None);
    }

    Ok(Some(resolved_path.canonicalize().unwrap_or(resolved_path)))
}

fn resolve_non_shim_gemini_launch(
    path_env: Option<&OsStr>,
    env: &HashMap<String, String>,
    project_dir: Option<&Path>,
    base_args: &[String],
) -> Result<Option<GeminiAcpLaunch>> {
    let Some(gemini_candidate) = find_direct_tool_path("gemini", path_env, env, project_dir)?
    else {
        return Ok(None);
    };

    if gemini_candidate
        .extension()
        .is_some_and(|extension| extension == "js")
        || is_node_script(&gemini_candidate)
    {
        let Some(node_candidate) = find_direct_tool_path("node", path_env, env, project_dir)?
        else {
            return Ok(None);
        };

        let mut args = vec![
            "--no-warnings=DEP0040".to_string(),
            gemini_candidate.to_string_lossy().into_owned(),
        ];
        args.extend(base_args.iter().cloned());
        return Ok(Some(GeminiAcpLaunch {
            command: node_candidate.to_string_lossy().into_owned(),
            args,
        }));
    }

    Ok(Some(GeminiAcpLaunch {
        command: gemini_candidate.to_string_lossy().into_owned(),
        args: base_args.to_vec(),
    }))
}

fn is_node_script(path: &Path) -> bool {
    let Ok(contents) = fs::read(path) else {
        return false;
    };

    let Some(first_line) = contents.split(|byte| *byte == b'\n').next() else {
        return false;
    };
    let Ok(first_line) = std::str::from_utf8(first_line) else {
        return false;
    };
    first_line.contains("node")
}

#[cfg(all(test, unix))]
mod tests {
    include!("transport_gemini_acp_runtime_tests_tail.rs");
}
