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
    /// Maximum physical memory in MB for cgroup `MemoryMax`.
    pub memory_max_mb: Option<u64>,
    /// Maximum swap in MB for cgroup `MemorySwapMax`.
    pub memory_swap_max_mb: Option<u64>,
    /// Maximum number of PIDs for cgroup `TasksMax` or `RLIMIT_NPROC`.
    pub pids_max: Option<u32>,
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
///
/// Resource and filesystem enforcement can be set independently via
/// [`with_filesystem_enforcement`].  When filesystem enforcement is not
/// explicitly set, it inherits the resource enforcement mode.
#[derive(Debug)]
pub struct IsolationPlanBuilder {
    enforcement_mode: EnforcementMode,
    fs_enforcement_mode: Option<EnforcementMode>,
    resource: ResourceCapability,
    filesystem: FilesystemCapability,
    writable_paths: Vec<PathBuf>,
    env_overrides: HashMap<String, String>,
    degraded_reasons: Vec<String>,
    memory_max_mb: Option<u64>,
    memory_swap_max_mb: Option<u64>,
    pids_max: Option<u32>,
}

impl IsolationPlanBuilder {
    /// Start a new builder with the given enforcement mode.
    pub fn new(enforcement_mode: EnforcementMode) -> Self {
        Self {
            enforcement_mode,
            fs_enforcement_mode: None,
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::None,
            writable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
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

    /// Set an independent enforcement mode for the filesystem axis.
    ///
    /// When set, the filesystem axis uses this mode instead of the
    /// resource enforcement mode.  This allows e.g. resource `Off` +
    /// filesystem `Required`.
    pub fn with_filesystem_enforcement(mut self, mode: EnforcementMode) -> Self {
        self.fs_enforcement_mode = Some(mode);
        self
    }

    /// Set resource limits for cgroup and rlimit enforcement.
    pub fn with_resource_limits(
        mut self,
        memory_max_mb: Option<u64>,
        memory_swap_max_mb: Option<u64>,
        pids_max: Option<u32>,
    ) -> Self {
        self.memory_max_mb = memory_max_mb;
        self.memory_swap_max_mb = memory_swap_max_mb;
        self.pids_max = pids_max;
        self
    }

    /// Apply per-tool default paths and environment overrides.
    ///
    /// Always adds `project_root` and `session_dir`.  Tool-specific config
    /// directories are appended based on `tool_name`.
    ///
    /// When `project_root` is inside a git submodule (`.git` is a file, not a
    /// directory), the superproject root is discovered by walking ancestors and
    /// added as writable.  This ensures the sandbox allows writes to
    /// `.git/modules/<submodule>/` which git requires for staging and commits.
    pub fn with_tool_defaults(
        mut self,
        tool_name: &str,
        project_root: &Path,
        session_dir: &Path,
    ) -> Self {
        self.writable_paths.push(project_root.to_path_buf());
        self.writable_paths.push(session_dir.to_path_buf());

        // Submodule detection: if .git is a file (not a directory), the project
        // root is inside a git submodule.  Walk up to find the superproject root
        // (the nearest ancestor with a .git *directory*) and make the entire
        // superproject writable so the agent can access .git/modules/ and other
        // submodules.
        if let Some(superproject) = detect_superproject_root(project_root) {
            self.writable_paths.push(superproject);
        }

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
    /// Returns an error when filesystem enforcement is `Required` but the
    /// filesystem capability is `None`.
    pub fn build(mut self) -> anyhow::Result<IsolationPlan> {
        // Filesystem enforcement: use dedicated override if set, otherwise
        // inherit from the resource enforcement mode.
        let fs_mode = self.fs_enforcement_mode.unwrap_or(self.enforcement_mode);

        match fs_mode {
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
            }
        }

        // Resource enforcement: handled separately.
        match self.enforcement_mode {
            EnforcementMode::BestEffort => {
                if self.resource == ResourceCapability::None {
                    self.degraded_reasons
                        .push("no resource isolation available; proceeding without".into());
                }
            }
            EnforcementMode::Off | EnforcementMode::Required => {
                // Required for resources is checked upstream in pipeline_sandbox.
                // Off is a no-op for the resource axis (capabilities are kept as-is
                // because cgroup limits don't need explicit disabling here).
            }
        }

        Ok(IsolationPlan {
            resource: self.resource,
            filesystem: self.filesystem,
            writable_paths: self.writable_paths,
            env_overrides: self.env_overrides,
            degraded_reasons: self.degraded_reasons,
            memory_max_mb: self.memory_max_mb,
            memory_swap_max_mb: self.memory_swap_max_mb,
            pids_max: self.pids_max,
        })
    }
}

/// Portable home-directory lookup (avoids pulling in the `dirs` crate).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Detect whether `project_root` is inside a git submodule and return the
/// superproject root if so.
///
/// A git submodule has a `.git` **file** (not directory) containing a
/// `gitdir:` pointer.  We walk ancestors looking for the nearest directory
/// that has a `.git` *directory* — that is the superproject root.
///
/// Returns `None` when `project_root` is not a submodule (`.git` is a
/// directory or does not exist).
fn detect_superproject_root(project_root: &Path) -> Option<PathBuf> {
    let dot_git = project_root.join(".git");

    // Only trigger when .git is a file (submodule marker).
    if !dot_git.is_file() {
        return None;
    }

    // Walk ancestors (skip project_root itself) looking for a .git directory.
    for ancestor in project_root.ancestors().skip(1) {
        if ancestor.join(".git").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }

    None
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
    fn test_submodule_detection_adds_superproject_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let superproject = tmp.path().join("monorepo");
        let submodule = superproject.join("crates").join("sub-crate");

        // Superproject has a .git directory
        std::fs::create_dir_all(superproject.join(".git")).expect("create .git dir");
        // Submodule has a .git file (not directory)
        std::fs::create_dir_all(&submodule).expect("create submodule dir");
        std::fs::write(
            submodule.join(".git"),
            "gitdir: ../../.git/modules/crates/sub-crate\n",
        )
        .expect("write .git file");

        let session = tmp.path().join("session");
        std::fs::create_dir_all(&session).expect("create session dir");

        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults("claude-code", &submodule, &session)
            .build()
            .expect("should succeed");

        assert!(
            plan.writable_paths.contains(&superproject),
            "superproject root should be in writable_paths, got: {:?}",
            plan.writable_paths
        );
        assert!(
            plan.writable_paths.contains(&submodule),
            "submodule (project_root) should still be in writable_paths"
        );
    }

    #[test]
    fn test_non_submodule_does_not_add_superproject() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("project");

        // Normal repo: .git is a directory
        std::fs::create_dir_all(project.join(".git")).expect("create .git dir");

        let session = tmp.path().join("session");
        std::fs::create_dir_all(&session).expect("create session dir");

        let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults("claude-code", &project, &session)
            .build()
            .expect("should succeed");

        // Only project + session + ~/.claude should be present (no superproject)
        let non_tool_paths: Vec<_> = plan
            .writable_paths
            .iter()
            .filter(|p| *p == &project || *p == &session)
            .collect();
        assert_eq!(
            non_tool_paths.len(),
            2,
            "should only have project + session as base writable paths"
        );
    }

    #[test]
    fn test_submodule_no_superproject_found() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orphan = tmp.path().join("orphan");

        // .git is a file but no ancestor has a .git directory
        std::fs::create_dir_all(&orphan).expect("create dir");
        std::fs::write(orphan.join(".git"), "gitdir: ../somewhere\n").expect("write .git file");

        let result = detect_superproject_root(&orphan);
        assert!(
            result.is_none(),
            "should return None when no superproject found"
        );
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
