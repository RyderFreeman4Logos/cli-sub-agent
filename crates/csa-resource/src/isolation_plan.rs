//! Isolation plan: combines resource and filesystem capabilities into a
//! single, builder-configured plan that executors can apply uniformly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;
use crate::sandbox::ResourceCapability;

// ---------------------------------------------------------------------------
// EnforcementMode (local copy)
// ---------------------------------------------------------------------------

/// Sandbox enforcement mode.
///
/// Mirrors `csa_config::EnforcementMode` but lives in `csa-resource` to avoid
/// a circular L1→L1 dependency.  The binary crate maps between the two.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    /// Require sandbox setup; abort if kernel support is missing.
    Required,
    /// Try to enforce limits; fall back gracefully if unavailable.
    BestEffort,
    /// Disable sandbox enforcement entirely.
    #[default]
    Off,
}

// ---------------------------------------------------------------------------
// IsolationPlan
// ---------------------------------------------------------------------------

/// Fully resolved isolation plan ready for executor consumption.
#[derive(Debug, Clone)]
pub struct IsolationPlan {
    /// Resource-level capability (cgroup / setrlimit / none).
    pub resource: ResourceCapability,
    /// Filesystem-level capability (bwrap / landlock / none).
    pub filesystem: FilesystemCapability,
    /// Paths the sandboxed process is allowed to write to.
    pub writable_paths: Vec<PathBuf>,
    /// Extra environment variables injected into the child process.
    pub env_overrides: HashMap<String, String>,
    /// Human-readable reasons when capabilities were downgraded.
    pub degraded_reasons: Vec<String>,
}

// ---------------------------------------------------------------------------
// IsolationPlanBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing an [`IsolationPlan`].
///
/// The `build()` method interprets the configured enforcement mode:
///
/// - **`BestEffort`** — uses the highest detected capability and records any
///   degradation reasons.
/// - **`Required`** — returns an error when filesystem isolation is `None`.
/// - **`Off`** — forces filesystem to `None`.
#[derive(Debug)]
pub struct IsolationPlanBuilder {
    enforcement_mode: EnforcementMode,
    resource: ResourceCapability,
    filesystem: FilesystemCapability,
    writable_paths: Vec<PathBuf>,
    env_overrides: HashMap<String, String>,
    degraded_reasons: Vec<String>,
}

impl IsolationPlanBuilder {
    /// Start a new builder with the given enforcement mode.
    pub fn new(enforcement_mode: EnforcementMode) -> Self {
        Self {
            enforcement_mode,
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::None,
            writable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
        }
    }

    /// Set the resource-level capability (cgroup / setrlimit / none).
    pub fn with_resource_capability(mut self, cap: ResourceCapability) -> Self {
        self.resource = cap;
        self
    }

    /// Set the filesystem-level capability (bwrap / landlock / none).
    pub fn with_filesystem_capability(mut self, cap: FilesystemCapability) -> Self {
        self.filesystem = cap;
        self
    }

    /// Add a single writable path to the plan.
    pub fn with_writable_path(mut self, path: PathBuf) -> Self {
        self.writable_paths.push(path);
        self
    }

    /// Apply per-tool default paths and environment overrides.
    ///
    /// Always adds `project_root` and `session_dir`.  Tool-specific config
    /// directories are appended based on `tool_name`.
    pub fn with_tool_defaults(
        mut self,
        tool_name: &str,
        project_root: &Path,
        session_dir: &Path,
    ) -> Self {
        self.writable_paths.push(project_root.to_path_buf());
        self.writable_paths.push(session_dir.to_path_buf());

        if let Some(home) = home_dir() {
            match tool_name {
                "claude-code" => {
                    self.writable_paths.push(home.join(".claude"));
                }
                "codex" => {
                    self.writable_paths.push(home.join(".codex"));
                }
                "gemini-cli" => {
                    self.writable_paths.push(home.join(".config/gemini-cli"));
                }
                "opencode" => {
                    self.writable_paths.push(home.join(".config/opencode"));
                }
                _ => {}
            }
        }
        self
    }

    /// Consume the builder and produce an [`IsolationPlan`].
    ///
    /// # Errors
    ///
    /// Returns an error when `enforcement_mode` is `Required` but the
    /// filesystem capability is `None`.
    pub fn build(mut self) -> anyhow::Result<IsolationPlan> {
        match self.enforcement_mode {
            EnforcementMode::Off => {
                self.filesystem = FilesystemCapability::None;
            }
            EnforcementMode::Required => {
                if self.filesystem == FilesystemCapability::None {
                    anyhow::bail!("filesystem isolation required but no capability detected");
                }
            }
            EnforcementMode::BestEffort => {
                if self.filesystem == FilesystemCapability::None {
                    self.degraded_reasons
                        .push("no filesystem isolation available; proceeding without".into());
                }
                if self.resource == ResourceCapability::None {
                    self.degraded_reasons
                        .push("no resource isolation available; proceeding without".into());
                }
            }
        }

        Ok(IsolationPlan {
            resource: self.resource,
            filesystem: self.filesystem,
            writable_paths: self.writable_paths,
            env_overrides: self.env_overrides,
            degraded_reasons: self.degraded_reasons,
        })
    }
}

/// Portable home-directory lookup (avoids pulling in the `dirs` crate).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_best_effort_with_bwrap() {
        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_resource_capability(ResourceCapability::CgroupV2)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .build()
            .expect("BestEffort with Bwrap should succeed");

        assert_eq!(plan.resource, ResourceCapability::CgroupV2);
        assert_eq!(plan.filesystem, FilesystemCapability::Bwrap);
        assert!(plan.degraded_reasons.is_empty());
    }

    #[test]
    fn test_builder_best_effort_degradation() {
        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_resource_capability(ResourceCapability::None)
            .with_filesystem_capability(FilesystemCapability::None)
            .build()
            .expect("BestEffort should never fail");

        assert_eq!(plan.filesystem, FilesystemCapability::None);
        assert_eq!(plan.degraded_reasons.len(), 2);
        assert!(plan.degraded_reasons[0].contains("filesystem"));
        assert!(plan.degraded_reasons[1].contains("resource"));
    }

    #[test]
    fn test_builder_required_fails_without_capability() {
        let result = IsolationPlanBuilder::new(EnforcementMode::Required)
            .with_resource_capability(ResourceCapability::CgroupV2)
            .with_filesystem_capability(FilesystemCapability::None)
            .build();

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("filesystem isolation required"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_builder_off_forces_none() {
        let plan = IsolationPlanBuilder::new(EnforcementMode::Off)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_resource_capability(ResourceCapability::CgroupV2)
            .build()
            .expect("Off mode should always succeed");

        assert_eq!(
            plan.filesystem,
            FilesystemCapability::None,
            "Off mode must force filesystem to None"
        );
        // Resource capability is kept as-is (Off only governs filesystem).
        assert_eq!(plan.resource, ResourceCapability::CgroupV2);
    }

    #[test]
    fn test_tool_defaults_claude_code() {
        let project = PathBuf::from("/tmp/project");
        let session = PathBuf::from("/tmp/session");

        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults("claude-code", &project, &session)
            .build()
            .expect("should succeed");

        assert!(plan.writable_paths.contains(&project));
        assert!(plan.writable_paths.contains(&session));

        if let Some(home) = home_dir() {
            assert!(
                plan.writable_paths.contains(&home.join(".claude")),
                "claude-code defaults should include ~/.claude"
            );
        }
    }

    #[test]
    fn test_tool_defaults_codex() {
        let project = PathBuf::from("/tmp/project");
        let session = PathBuf::from("/tmp/session");

        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults("codex", &project, &session)
            .build()
            .expect("should succeed");

        assert!(plan.writable_paths.contains(&project));
        assert!(plan.writable_paths.contains(&session));

        if let Some(home) = home_dir() {
            assert!(
                plan.writable_paths.contains(&home.join(".codex")),
                "codex defaults should include ~/.codex"
            );
        }
    }
}
