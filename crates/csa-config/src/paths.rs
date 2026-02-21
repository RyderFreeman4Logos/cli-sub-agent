use std::path::PathBuf;

/// Canonical XDG app name used for all new path writes.
pub const APP_NAME: &str = "cli-sub-agent";
/// Legacy XDG app name kept for backward-compatible reads and migration.
pub const LEGACY_APP_NAME: &str = "csa";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XdgPathPair {
    pub label: &'static str,
    pub new_path: PathBuf,
    pub legacy_path: PathBuf,
}

fn project_config_dir(app_name: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", app_name).map(|dirs| dirs.config_dir().to_path_buf())
}

fn project_state_dir(app_name: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", app_name).map(|dirs| {
        dirs.state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_path_buf()
    })
}

fn choose_read_path(new_path: PathBuf, legacy_path: PathBuf) -> PathBuf {
    if new_path.exists() {
        new_path
    } else if legacy_path.exists() {
        legacy_path
    } else {
        new_path
    }
}

fn runtime_dir_for_name(app_name: &str, runtime_root: Option<&str>, uid: u32) -> PathBuf {
    if let Some(runtime_root) = runtime_root {
        return PathBuf::from(runtime_root).join(app_name);
    }
    PathBuf::from("/tmp").join(format!("{app_name}-{uid}"))
}

fn effective_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` has no preconditions and returns caller effective UID.
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Resolve config directory for reads:
/// prefer canonical path, fallback to legacy if canonical does not exist.
pub fn config_dir() -> Option<PathBuf> {
    let new_path = project_config_dir(APP_NAME)?;
    let legacy_path = project_config_dir(LEGACY_APP_NAME)?;
    Some(choose_read_path(new_path, legacy_path))
}

/// Canonical config directory for writes (always new path).
pub fn config_dir_write() -> Option<PathBuf> {
    project_config_dir(APP_NAME)
}

/// Resolve state directory for reads:
/// prefer canonical path, fallback to legacy if canonical does not exist.
pub fn state_dir() -> Option<PathBuf> {
    let new_path = project_state_dir(APP_NAME)?;
    let legacy_path = project_state_dir(LEGACY_APP_NAME)?;
    Some(choose_read_path(new_path, legacy_path))
}

/// Canonical state directory for writes (always new path).
pub fn state_dir_write() -> Option<PathBuf> {
    project_state_dir(APP_NAME)
}

/// Resolve runtime directory for reads:
/// prefer canonical path, fallback to legacy if canonical does not exist.
pub fn runtime_dir() -> PathBuf {
    let runtime_root = std::env::var("XDG_RUNTIME_DIR").ok();
    let uid = effective_uid();
    let new_path = runtime_dir_for_name(APP_NAME, runtime_root.as_deref(), uid);
    let legacy_path = runtime_dir_for_name(LEGACY_APP_NAME, runtime_root.as_deref(), uid);
    choose_read_path(new_path, legacy_path)
}

/// Canonical runtime directory for writes (always new path).
pub fn runtime_dir_write() -> PathBuf {
    let runtime_root = std::env::var("XDG_RUNTIME_DIR").ok();
    runtime_dir_for_name(APP_NAME, runtime_root.as_deref(), effective_uid())
}

pub fn legacy_config_dir() -> Option<PathBuf> {
    project_config_dir(LEGACY_APP_NAME)
}

pub fn legacy_state_dir() -> Option<PathBuf> {
    project_state_dir(LEGACY_APP_NAME)
}

pub fn legacy_runtime_dir() -> PathBuf {
    let runtime_root = std::env::var("XDG_RUNTIME_DIR").ok();
    runtime_dir_for_name(LEGACY_APP_NAME, runtime_root.as_deref(), effective_uid())
}

pub fn state_dir_fallback() -> PathBuf {
    std::env::temp_dir().join(format!("{APP_NAME}-state"))
}

pub fn xdg_path_pairs() -> Vec<XdgPathPair> {
    let mut pairs = Vec::new();
    if let (Some(new_path), Some(legacy_path)) = (config_dir_write(), legacy_config_dir()) {
        pairs.push(XdgPathPair {
            label: "config",
            new_path,
            legacy_path,
        });
    }
    if let (Some(new_path), Some(legacy_path)) = (state_dir_write(), legacy_state_dir()) {
        pairs.push(XdgPathPair {
            label: "state",
            new_path,
            legacy_path,
        });
    }
    pairs.push(XdgPathPair {
        label: "runtime",
        new_path: runtime_dir_write(),
        legacy_path: legacy_runtime_dir(),
    });
    pairs
}

fn is_symlink_to(legacy_path: &std::path::Path, new_path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(legacy_path) else {
        return false;
    };
    if !metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(target) = std::fs::read_link(legacy_path) else {
        return false;
    };
    target == new_path
}

pub fn legacy_paths_requiring_migration() -> Vec<XdgPathPair> {
    xdg_path_pairs()
        .into_iter()
        .filter(|pair| {
            if !pair.legacy_path.exists() {
                return false;
            }
            if !pair.new_path.exists() {
                return true;
            }
            !is_symlink_to(&pair.legacy_path, &pair.new_path)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{APP_NAME, LEGACY_APP_NAME, choose_read_path, runtime_dir_for_name};
    use std::path::PathBuf;

    #[test]
    fn choose_read_path_prefers_new_when_present() {
        let temp = tempfile::tempdir().expect("tempdir");
        let new_path = temp.path().join("new");
        let legacy_path = temp.path().join("legacy");
        std::fs::create_dir_all(&new_path).expect("create new path");
        std::fs::create_dir_all(&legacy_path).expect("create legacy path");

        let chosen = choose_read_path(new_path.clone(), legacy_path);
        assert_eq!(chosen, new_path);
    }

    #[test]
    fn choose_read_path_falls_back_to_legacy_when_new_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let new_path = temp.path().join("new");
        let legacy_path = temp.path().join("legacy");
        std::fs::create_dir_all(&legacy_path).expect("create legacy path");

        let chosen = choose_read_path(new_path, legacy_path.clone());
        assert_eq!(chosen, legacy_path);
    }

    #[test]
    fn runtime_dir_for_name_uses_xdg_runtime_dir_when_present() {
        let path = runtime_dir_for_name(APP_NAME, Some("/run/user/1000"), 1000);
        assert_eq!(path, PathBuf::from("/run/user/1000").join(APP_NAME));
    }

    #[test]
    fn runtime_dir_for_name_falls_back_to_tmp_with_uid() {
        let path = runtime_dir_for_name(LEGACY_APP_NAME, None, 1234);
        assert_eq!(path, PathBuf::from("/tmp").join("csa-1234"));
    }
}
