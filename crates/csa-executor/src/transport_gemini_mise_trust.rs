use std::fs;
use std::path::{Path, PathBuf};

pub(super) const GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH: &str = "mise/trusted-configs";

/// Check if the mise config at `mise_toml_path` is trusted on the host by
/// scanning the host mise filesystem trust DB.
///
/// Returns `Some(mise_toml_path)` when the host trust DB contains a direct
/// symlink entry that points at `project_root` or one of its ancestors.
pub(super) fn probe_host_mise_trust_db(
    project_root: &Path,
    mise_toml_path: &Path,
) -> Option<PathBuf> {
    let canonical_project_root = project_root.canonicalize().ok()?;
    let canonical_mise_toml_path = mise_toml_path.canonicalize().ok()?;
    let trust_db_dir = host_mise_trust_db_dir()?;
    let entries = fs::read_dir(trust_db_dir).ok()?;

    for entry in entries.flatten() {
        let entry_path = entry.path();
        let Ok(link_target) = fs::read_link(&entry_path) else {
            continue;
        };
        let resolved_target = resolve_trust_db_symlink_target(&entry_path, &link_target);
        if canonical_project_root.starts_with(&resolved_target) {
            return Some(canonical_mise_toml_path);
        }
    }

    None
}

fn host_mise_trust_db_dir() -> Option<PathBuf> {
    let xdg_state_home = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .map(|home| home.join(".local").join("state"))
        })?;
    Some(xdg_state_home.join(GEMINI_HOST_MISE_TRUST_DB_RELATIVE_PATH))
}

fn resolve_trust_db_symlink_target(entry_path: &Path, link_target: &Path) -> PathBuf {
    if link_target.is_absolute() {
        return link_target.to_path_buf();
    }

    entry_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(link_target)
}
