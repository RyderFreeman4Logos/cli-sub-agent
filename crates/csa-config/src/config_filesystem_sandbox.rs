//! Filesystem sandbox configuration section (`[filesystem_sandbox]`).
//!
//! Controls filesystem isolation for child tool processes via bubblewrap
//! (bwrap) or Landlock LSM.  Lives in both project and global config;
//! project-level values override global-level via the standard TOML merge.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::paths::{APP_NAME, LEGACY_APP_NAME};

/// Filesystem sandbox configuration.
///
/// # TOML example
///
/// ```toml
/// [filesystem_sandbox]
/// enforcement_mode = "best-effort"   # "best-effort" | "required" | "off"
/// extra_writable = ["/tmp/my-cache"]
/// extra_readable = ["/tmp/host-data.json"]
///
/// [filesystem_sandbox.tool_writable_overrides]
/// claude-code = ["/home/user/.special"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesystemSandboxConfig {
    /// Filesystem sandbox enforcement mode.
    ///
    /// - `"best-effort"` (default when absent): try bwrap/landlock, degrade gracefully.
    /// - `"required"`: abort if no filesystem isolation is available.
    /// - `"off"`: disable filesystem sandboxing entirely.
    ///
    /// `None` is treated as `"best-effort"` by the pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_mode: Option<String>,

    /// Additional writable paths granted to all tools beyond the defaults
    /// (project root, session dir, tool config dirs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_writable: Vec<PathBuf>,

    /// Additional readable paths granted to all tools as read-only binds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_readable: Vec<PathBuf>,

    /// Per-tool writable path overrides.  Keys are canonical tool names
    /// (e.g. `"claude-code"`, `"codex"`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_writable_overrides: HashMap<String, Vec<PathBuf>>,
}

impl FilesystemSandboxConfig {
    /// Returns `true` when all fields are at their default values.
    ///
    /// Used by `skip_serializing_if` to omit the section from TOML output
    /// when it carries no user-specified configuration.
    pub fn is_default(&self) -> bool {
        self.enforcement_mode.is_none()
            && self.extra_writable.is_empty()
            && self.extra_readable.is_empty()
            && self.tool_writable_overrides.is_empty()
    }

    /// Normalize legacy `XDG_RUNTIME_DIR` root writable entries to scoped child paths.
    ///
    /// The resource sandbox intentionally rejects writing to the entire runtime
    /// directory. Older user configs may still contain that root path, so config
    /// loading narrows it to known existing CSA-related children before runtime
    /// validation sees it.
    pub(crate) fn sanitize_legacy_xdg_runtime_root(&mut self) {
        sanitize_legacy_xdg_runtime_root_paths(
            &mut self.extra_writable,
            "filesystem_sandbox.extra_writable",
        );
        for (tool, paths) in &mut self.tool_writable_overrides {
            let context = format!("filesystem_sandbox.tool_writable_overrides.{tool}");
            sanitize_legacy_xdg_runtime_root_paths(paths, &context);
        }
    }
}

pub(crate) fn sanitize_legacy_xdg_runtime_root_paths(paths: &mut Vec<PathBuf>, context: &str) {
    let Some(runtime_root) = xdg_runtime_root() else {
        return;
    };
    let replacements = scoped_runtime_replacements(&runtime_root);
    let mut sanitized = Vec::with_capacity(paths.len().saturating_add(replacements.len()));
    let mut changed = false;

    for path in paths.drain(..) {
        if is_runtime_root_entry(&path, &runtime_root) {
            changed = true;
            if replacements.is_empty() {
                tracing::warn!(
                    path = %path.display(),
                    context,
                    "Dropping legacy XDG_RUNTIME_DIR root writable path; no scoped runtime child exists"
                );
                continue;
            }
            tracing::warn!(
                path = %path.display(),
                replacement_count = replacements.len(),
                context,
                "Narrowing legacy XDG_RUNTIME_DIR root writable path to scoped child path(s)"
            );
            for replacement in &replacements {
                push_unique_path(&mut sanitized, replacement.clone());
            }
        } else {
            push_unique_path(&mut sanitized, path);
        }
    }

    if changed {
        *paths = sanitized;
    }
}

fn xdg_runtime_root() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    if !runtime_dir.is_absolute() {
        return None;
    }
    let runtime_dir = comparable_path(&runtime_dir);
    if runtime_dir == Path::new("/") {
        return None;
    }
    Some(runtime_dir)
}

fn scoped_runtime_replacements(runtime_root: &Path) -> Vec<PathBuf> {
    [APP_NAME, LEGACY_APP_NAME, "just"]
        .into_iter()
        .map(|name| comparable_path(&runtime_root.join(name)))
        .filter(|path| path.exists())
        .fold(Vec::new(), |mut acc, path| {
            push_unique_path(&mut acc, path);
            acc
        })
}

fn is_runtime_root_entry(path: &Path, runtime_root: &Path) -> bool {
    path.is_absolute() && comparable_path(path) == runtime_root
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    let comparable_candidate = comparable_path(&candidate);
    if paths
        .iter()
        .any(|existing| comparable_path(existing) == comparable_candidate)
    {
        return;
    }
    paths.push(candidate);
}

fn comparable_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| normalize_path_components(path))
}

fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
            | Component::Prefix(_)
            | Component::RootDir
            | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::{OsStr, OsString};

    #[test]
    fn test_default_is_default() {
        let cfg = FilesystemSandboxConfig::default();
        assert!(cfg.is_default());
    }

    #[test]
    fn test_with_enforcement_mode_not_default() {
        let cfg = FilesystemSandboxConfig {
            enforcement_mode: Some("required".to_string()),
            ..Default::default()
        };
        assert!(!cfg.is_default());
    }

    #[test]
    fn test_with_extra_writable_not_default() {
        let cfg = FilesystemSandboxConfig {
            extra_writable: vec![PathBuf::from("/tmp/extra")],
            ..Default::default()
        };
        assert!(!cfg.is_default());
    }

    #[test]
    fn test_roundtrip_toml() {
        let cfg = FilesystemSandboxConfig {
            enforcement_mode: Some("required".to_string()),
            extra_writable: vec![PathBuf::from("/opt/data")],
            extra_readable: vec![PathBuf::from("/tmp/foo.json")],
            tool_writable_overrides: HashMap::from([(
                "claude-code".to_string(),
                vec![PathBuf::from("/home/user/.claude-extra")],
            )]),
        };
        let toml_str = toml::to_string(&cfg).expect("serialize");
        let decoded: FilesystemSandboxConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(decoded.enforcement_mode, cfg.enforcement_mode);
        assert_eq!(decoded.extra_writable, cfg.extra_writable);
        assert_eq!(decoded.extra_readable, cfg.extra_readable);
        assert_eq!(decoded.tool_writable_overrides, cfg.tool_writable_overrides);
    }

    #[test]
    fn test_deserialize_extra_readable() {
        let decoded: FilesystemSandboxConfig = toml::from_str(
            r#"
enforcement_mode = "best-effort"
extra_readable = ["/tmp/foo.json"]
"#,
        )
        .expect("deserialize");

        assert_eq!(decoded.extra_readable, vec![PathBuf::from("/tmp/foo.json")]);
    }

    #[test]
    #[serial]
    fn test_sanitize_legacy_runtime_root_uses_existing_scoped_children() {
        let runtime_dir = tempfile::tempdir().expect("tempdir");
        let runtime_root = runtime_dir.path();
        let app_dir = runtime_root.join(APP_NAME);
        let just_dir = runtime_root.join("just");
        std::fs::create_dir_all(&app_dir).expect("create app dir");
        std::fs::create_dir_all(&just_dir).expect("create just dir");
        let _guard = ScopedEnvVar::set("XDG_RUNTIME_DIR", runtime_root.as_os_str());
        let mut cfg = FilesystemSandboxConfig {
            extra_writable: vec![runtime_root.to_path_buf(), app_dir.clone()],
            tool_writable_overrides: HashMap::from([(
                "codex".to_string(),
                vec![runtime_root.to_path_buf()],
            )]),
            ..Default::default()
        };

        cfg.sanitize_legacy_xdg_runtime_root();

        let expected_app_dir = comparable_path(&app_dir);
        let expected_just_dir = comparable_path(&just_dir);

        assert_eq!(
            cfg.extra_writable,
            vec![expected_app_dir.clone(), expected_just_dir.clone()]
        );
        assert_eq!(
            cfg.tool_writable_overrides.get("codex"),
            Some(&vec![expected_app_dir, expected_just_dir])
        );
    }

    #[test]
    #[serial]
    fn test_sanitize_legacy_runtime_root_drops_root_without_scoped_child() {
        let runtime_dir = tempfile::tempdir().expect("tempdir");
        let runtime_root = runtime_dir.path();
        let other_path = PathBuf::from("/tmp/csa-cache");
        let _guard = ScopedEnvVar::set("XDG_RUNTIME_DIR", runtime_root.as_os_str());
        let mut paths = vec![runtime_root.to_path_buf(), other_path.clone()];

        sanitize_legacy_xdg_runtime_root_paths(&mut paths, "test");

        assert_eq!(paths, vec![other_path]);
    }

    struct ScopedEnvVar {
        key: &'static str,
        original: Option<OsString>,
    }

    impl ScopedEnvVar {
        fn set(key: &'static str, value: &OsStr) -> Self {
            let original = std::env::var_os(key);
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }
    }

    impl Drop for ScopedEnvVar {
        fn drop(&mut self) {
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
