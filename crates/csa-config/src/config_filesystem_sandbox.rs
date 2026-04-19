//! Filesystem sandbox configuration section (`[filesystem_sandbox]`).
//!
//! Controls filesystem isolation for child tool processes via bubblewrap
//! (bwrap) or Landlock LSM.  Lives in both project and global config;
//! project-level values override global-level via the standard TOML merge.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// (e.g. `"claude-code"`, `"gemini-cli"`).
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
