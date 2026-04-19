//! Isolation plan: combines resource and filesystem capabilities into a
//! single, builder-configured plan that executors can apply uniformly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;
use crate::sandbox::ResourceCapability;

pub const DEFAULT_SANDBOX_TMPDIR: &str = "/tmp";

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
    /// Paths the sandboxed process may read via read-only bind mounts.
    pub readable_paths: Vec<PathBuf>,
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
    /// When true, the project root is mounted read-only instead of read-write.
    pub readonly_project_root: bool,
    /// Project root path, used by bwrap to decide bind mount mode.
    pub project_root: Option<PathBuf>,
    /// Soft memory limit as a percentage of `memory_max_mb`.
    /// When set, a background monitor sends SIGTERM when usage exceeds this.
    pub soft_limit_percent: Option<u8>,
    /// Polling interval for the memory monitor in seconds.
    pub memory_monitor_interval_seconds: Option<u64>,
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
    readable_paths: Vec<PathBuf>,
    env_overrides: HashMap<String, String>,
    degraded_reasons: Vec<String>,
    memory_max_mb: Option<u64>,
    memory_swap_max_mb: Option<u64>,
    pids_max: Option<u32>,
    readonly_project_root: bool,
    project_root: Option<PathBuf>,
    soft_limit_percent: Option<u8>,
    memory_monitor_interval_seconds: Option<u64>,
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
            readable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
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

    /// Add a single read-only readable path to the plan.
    pub fn with_readable_path(mut self, path: PathBuf) -> Self {
        self.readable_paths.push(path);
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

    /// Mount the project root as read-only instead of read-write.
    ///
    /// When enabled, the bwrap builder uses `--ro-bind` for the project root.
    /// Useful for tools that should only read project files, not modify them.
    pub fn with_readonly_project_root(mut self, readonly: bool) -> Self {
        self.readonly_project_root = readonly;
        self
    }

    /// Set the soft memory limit percentage for the memory monitor.
    pub fn with_soft_limit_percent(mut self, percent: Option<u8>) -> Self {
        self.soft_limit_percent = percent;
        self
    }

    /// Set the memory monitor polling interval in seconds.
    pub fn with_memory_monitor_interval(mut self, seconds: Option<u64>) -> Self {
        self.memory_monitor_interval_seconds = seconds;
        self
    }

    /// Apply per-tool default paths and environment overrides.
    ///
    /// Always adds `project_root`, `session_dir`, and common writable paths
    /// that all tools need (XDG state dir, mise cache). It also injects a
    /// writable `TMPDIR`: bwrap uses its private `/tmp`, while Landlock and
    /// unsandboxed paths use a session-owned `tmp/` subdirectory.
    /// Tool-specific config directories are appended based on `tool_name`.
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
        self.project_root = Some(project_root.to_path_buf());
        self.writable_paths.push(project_root.to_path_buf());
        self.writable_paths.push(session_dir.to_path_buf());
        let sandbox_tmpdir = sandbox_tmpdir_for_capability(self.filesystem, session_dir);
        self.env_overrides.insert(
            "TMPDIR".to_string(),
            sandbox_tmpdir.to_string_lossy().into_owned(),
        );

        // Submodule detection: if .git is a file (not a directory), the project
        // root is inside a git submodule.  Walk up to find the superproject root
        // (the nearest ancestor with a .git *directory*) and make the entire
        // superproject writable so the agent can access .git/modules/ and other
        // submodules.
        if let Some(superproject) = detect_superproject_root(project_root) {
            self.writable_paths.push(superproject);
        }

        if let Some(home) = home_dir() {
            // Common writable paths needed by all tools:
            // - XDG_STATE_HOME (~/.local/state): cargo compilation writes proc-macro
            //   artifacts here; without write access tools get "Read-only file system
            //   (os error 30)" on Rust compilation.
            let xdg_state = std::env::var("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".local/state"));
            // Only add paths that exist on the filesystem; bwrap --bind fails on
            // nonexistent paths.
            if xdg_state.exists() {
                self.writable_paths.push(xdg_state);
            }

            // mise cache: tools launched via mise shims (rustc, cargo, node) write
            // to ~/.cache/mise during startup and compilation. Without write access,
            // mise-managed toolchains fail with "Read-only file system".
            let mise_cache = home.join(".cache/mise");
            if mise_cache.exists() {
                self.writable_paths.push(mise_cache);
            }

            // Cargo home directory: cargo needs write access to registry/, git/,
            // and .package-cache (lock file).
            //
            // When CARGO_HOME is explicitly set to a non-default location, we
            // ONLY expose that directory — not ~/.cargo — to avoid leaking
            // credentials/config from the real cargo home.  For cold starts
            // where the directory doesn't exist yet, we add it anyway so bwrap
            // can create it (the parent must exist).
            let default_cargo_home = home.join(".cargo");
            if let Ok(cargo_home_env) = std::env::var("CARGO_HOME") {
                let cargo_home = PathBuf::from(&cargo_home_env);
                if cargo_home == default_cargo_home {
                    // CARGO_HOME points to the default — treat as if unset.
                    add_dir_or_creatable_parent(&mut self.writable_paths, &default_cargo_home);
                } else {
                    // CARGO_HOME points elsewhere — only expose that directory.
                    // Do NOT add ~/.cargo (may contain credentials/config).
                    add_dir_or_creatable_parent(&mut self.writable_paths, &cargo_home);
                }
            } else {
                add_dir_or_creatable_parent(&mut self.writable_paths, &default_cargo_home);
            }

            // RUSTUP_HOME: rustup needs write access for toolchain management
            // (downloading components, updating toolchains). Same pattern as
            // CARGO_HOME: when explicitly set elsewhere, don't expose ~/.rustup.
            let default_rustup = home.join(".rustup");
            if let Ok(rustup_home) = std::env::var("RUSTUP_HOME") {
                let rustup_path = PathBuf::from(&rustup_home);
                if rustup_path == default_rustup {
                    add_dir_or_creatable_parent(&mut self.writable_paths, &default_rustup);
                } else {
                    // RUSTUP_HOME points elsewhere — only expose that directory.
                    add_dir_or_creatable_parent(&mut self.writable_paths, &rustup_path);
                }
            } else {
                add_dir_or_creatable_parent(&mut self.writable_paths, &default_rustup);
            }

            // NOTE: mise-managed Rust toolchain paths are intentionally NOT added
            // as writable. Making the entire install dir writable (rustc, stdlib)
            // is an isolation regression. The cargo registry/git cache dirs are
            // already covered by the CARGO_HOME logic above — when mise sets
            // CARGO_HOME into the toolchain dir, those subdirs get write access.

            // Tool-specific config/data directories (only if they exist).
            let tool_dirs: &[&str] = match tool_name {
                "claude-code" => &[".claude"],
                "codex" => &[".codex"],
                "gemini-cli" => &[".gemini", ".config/gemini-cli"],
                "opencode" => &[".config/opencode"],
                _ => &[],
            };
            for rel in tool_dirs {
                let p = home.join(rel);
                if p.exists() {
                    self.writable_paths.push(p);
                }
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

        // Landlock operates in the calling thread via pre_exec. That makes it
        // incompatible with CgroupV2's `systemd-run --scope` wrapper: applying
        // Landlock there would sandbox the wrapper itself and break its D-Bus
        // connection to the user manager. Prefer Setrlimit so the actual tool
        // process still receives filesystem isolation.
        if self.resource == ResourceCapability::CgroupV2
            && self.filesystem == FilesystemCapability::Landlock
        {
            self.resource = ResourceCapability::Setrlimit;
            self.degraded_reasons.push(
                "landlock cannot be combined with cgroup wrapper; falling back to setrlimit resource isolation".into(),
            );
        }

        Ok(IsolationPlan {
            resource: self.resource,
            filesystem: self.filesystem,
            writable_paths: self.writable_paths,
            readable_paths: self.readable_paths,
            env_overrides: self.env_overrides,
            degraded_reasons: self.degraded_reasons,
            memory_max_mb: self.memory_max_mb,
            memory_swap_max_mb: self.memory_swap_max_mb,
            pids_max: self.pids_max,
            readonly_project_root: self.readonly_project_root,
            project_root: self.project_root,
            soft_limit_percent: self.soft_limit_percent,
            memory_monitor_interval_seconds: self.memory_monitor_interval_seconds,
        })
    }
}

fn sandbox_tmpdir_for_capability(filesystem: FilesystemCapability, session_dir: &Path) -> PathBuf {
    match filesystem {
        FilesystemCapability::Bwrap => PathBuf::from(DEFAULT_SANDBOX_TMPDIR),
        FilesystemCapability::Landlock | FilesystemCapability::None => session_dir.join("tmp"),
    }
}

/// Validate that all writable paths are subpaths of allowed parents.
///
/// Allowed parents: `project_root`, the user home directory, and `/tmp`.
/// Paths like `/`, `/etc`, `/usr`, `/var` are rejected.
///
/// # Errors
///
/// Returns an error listing any rejected paths that are not under an allowed
/// parent directory.
pub fn validate_writable_paths(paths: &[PathBuf], project_root: &Path) -> anyhow::Result<()> {
    validate_sandbox_paths(
        paths,
        project_root,
        PathValidationOptions {
            kind: "writable_paths",
            require_absolute: false,
            require_exists: false,
            reject_tmp_root: false,
            canonicalize_for_allowlist: false,
        },
    )
}

/// Validate that readable paths are safe to expose into the sandbox.
///
/// Read-only binds are stricter than writable paths: every path must be
/// absolute, must exist on disk, `/tmp` itself is forbidden, and symlinked
/// paths are validated against the canonical target to prevent bind-mounting a
/// safe-looking path that resolves somewhere outside the allowlist.
pub fn validate_readable_paths(paths: &[PathBuf], project_root: &Path) -> anyhow::Result<()> {
    validate_sandbox_paths(
        paths,
        project_root,
        PathValidationOptions {
            kind: "readable_paths",
            require_absolute: true,
            require_exists: true,
            reject_tmp_root: true,
            canonicalize_for_allowlist: true,
        },
    )
}

struct PathValidationOptions<'a> {
    kind: &'a str,
    require_absolute: bool,
    require_exists: bool,
    reject_tmp_root: bool,
    canonicalize_for_allowlist: bool,
}

fn validate_sandbox_paths(
    paths: &[PathBuf],
    project_root: &Path,
    options: PathValidationOptions<'_>,
) -> anyhow::Result<()> {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("/nonexistent"));
    let allowed_parents: [&Path; 3] = [project_root, home.as_path(), Path::new("/tmp")];
    let mut rejected = Vec::new();

    for path in paths {
        let path_for_allowlist = match validate_single_path(path, &allowed_parents, &options) {
            Ok(candidate) => candidate,
            Err(reason) => {
                rejected.push(format!("{} ({reason})", path.display()));
                continue;
            }
        };

        let is_allowed = allowed_parents
            .iter()
            .any(|parent| path_for_allowlist.starts_with(parent));
        if !is_allowed {
            rejected.push(format!(
                "{} (outside allowed roots: home, /tmp, project root)",
                path.display()
            ));
        }
    }

    if rejected.is_empty() {
        return Ok(());
    }

    anyhow::bail!(
        "{} validation failed: rejected paths [{}]. Allowed: subpaths of home dir, /tmp, or project root",
        options.kind,
        rejected.join(", ")
    );
}

fn validate_single_path(
    path: &Path,
    allowed_parents: &[&Path],
    options: &PathValidationOptions<'_>,
) -> anyhow::Result<PathBuf> {
    if path == Path::new("/") {
        anyhow::bail!("root path is forbidden");
    }
    if options.reject_tmp_root && path == Path::new("/tmp") {
        anyhow::bail!("/tmp itself is forbidden; expose a specific sub-path instead");
    }
    if options.require_absolute && !path.is_absolute() {
        anyhow::bail!("path must be absolute");
    }
    if options.require_exists && !path.exists() {
        anyhow::bail!("path must exist");
    }

    if !options.canonicalize_for_allowlist {
        return Ok(path.to_path_buf());
    }

    let canonical = path.canonicalize()?;
    let canonical_allowed = allowed_parents
        .iter()
        .any(|parent| canonical.starts_with(parent));
    if !canonical_allowed {
        anyhow::bail!(
            "resolved path {} escapes allowed roots",
            canonical.display()
        );
    }
    Ok(canonical)
}

/// Add `dir` to `paths` if it exists, otherwise pre-create it when its
/// parent exists (bwrap `--bind` requires the source path to exist).
///
/// Rejects paths under sensitive system directories (`/etc`, `/var/lib`,
/// `/boot`, `/sbin`, etc.) to prevent env vars like `CARGO_HOME` from
/// escaping the sandbox boundary.
fn add_dir_or_creatable_parent(paths: &mut Vec<PathBuf>, dir: &Path) {
    if is_sensitive_system_path(dir) {
        tracing::warn!(
            path = %dir.display(),
            "rejecting writable path under sensitive system directory"
        );
        return;
    }

    if dir.exists() {
        paths.push(dir.to_path_buf());
    } else if dir.parent().is_some_and(|p| p.exists()) {
        // Pre-create the directory so bwrap --bind can mount it.
        // On cold starts (fresh CARGO_HOME/RUSTUP_HOME) the dir won't
        // exist yet; bwrap requires the source path to be present.
        match std::fs::create_dir_all(dir) {
            Ok(()) => paths.push(dir.to_path_buf()),
            Err(e) => tracing::warn!(
                path = %dir.display(),
                error = %e,
                "failed to pre-create directory for sandbox writable mount, skipping"
            ),
        }
    }
}

/// Reject paths under sensitive system directories that should never be
/// writable inside a sandbox.  Allows legitimate paths like home dirs,
/// `/tmp`, `/usr/local/share/mise`, etc.
fn is_sensitive_system_path(path: &Path) -> bool {
    /// Prefixes that indicate sensitive system directories.
    const SENSITIVE_PREFIXES: &[&str] = &[
        "/etc", "/var/lib", "/var/log", "/var/run", "/boot", "/sbin", "/bin", "/lib", "/lib64",
        "/sys", "/proc", "/dev", "/run",
    ];

    for prefix in SENSITIVE_PREFIXES {
        if path.starts_with(prefix) {
            return true;
        }
    }
    // Reject bare root path
    path == Path::new("/")
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

#[cfg(test)]
#[path = "isolation_plan_tests.rs"]
mod tests;
