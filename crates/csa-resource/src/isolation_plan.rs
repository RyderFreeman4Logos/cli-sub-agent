//! Isolation plan: combines resource and filesystem capabilities into a
//! single, builder-configured plan that executors can apply uniformly.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::filesystem_sandbox::FilesystemCapability;
use crate::sandbox::ResourceCapability;

pub const DEFAULT_SANDBOX_TMPDIR: &str = "/tmp";

#[path = "isolation_plan_claude.rs"]
mod claude_paths;
#[path = "isolation_plan_codex.rs"]
mod codex_paths;
#[path = "isolation_plan_runtime_path.rs"]
mod runtime_path;
#[path = "isolation_plan_rust_env.rs"]
mod rust_env;
#[path = "isolation_plan_validation.rs"]
mod validation;
#[cfg(test)]
use runtime_path::is_xdg_runtime_child_path;
use runtime_path::{detect_superproject_root, home_dir, is_sensitive_system_path};
pub use validation::{
    canonicalize_through_existing_ancestors, resolve_writable_paths, validate_readable_paths,
    validate_writable_paths,
};

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
    /// When true, the sandbox exposes D-Bus user bus and systemd private socket
    /// as readable paths, allowing the sandboxed process to communicate with
    /// the user session daemon manager (#2404).
    ///
    /// This is a named capability — it must be explicitly requested via
    /// [`IsolationPlanBuilder::with_user_daemon_ipc`]. The implicit auto-detection
    /// based on writable runtime children has been removed in favor of this
    /// explicit opt-in.
    pub user_daemon_ipc: bool,
}

impl IsolationPlan {
    /// Add a writable directory, pre-creating it when its parent exists so
    /// bwrap bind sources are present on cold start.
    pub fn add_writable_dir_or_creatable_parent(&mut self, dir: &Path) -> bool {
        add_dir_or_creatable_parent(&mut self.writable_paths, dir)
    }
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
    user_daemon_ipc: bool,
    required_writable_dirs: Vec<codex_paths::RequiredWritableDir>,
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
            user_daemon_ipc: false,
            required_writable_dirs: Vec::new(),
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

    /// Enable the `user-daemon-ipc` named sandbox capability (#2404).
    ///
    /// When enabled, the sandbox exposes the D-Bus user bus socket
    /// (`$XDG_RUNTIME_DIR/bus`) and systemd private socket
    /// (`$XDG_RUNTIME_DIR/systemd/private`) as readable paths, allowing
    /// the sandboxed process to communicate with the user session daemon
    /// manager (e.g. for `systemctl --user restart`).
    ///
    /// This capability MUST be explicitly requested — it is never auto-detected.
    /// Usage should be recorded in session audit artifacts.
    pub fn with_user_daemon_ipc(mut self) -> Self {
        self.user_daemon_ipc = true;
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
        self.apply_tool_defaults(tool_name, project_root, session_dir, None);
        self
    }

    /// Apply per-tool defaults with optional configured tool state dirs.
    ///
    /// `tool_state_dirs` maps canonical state names such as `"codex"` and
    /// `"claude"` to host paths.  Environment variables recognized by the
    /// underlying tools still take precedence over this table.
    pub fn with_tool_defaults_and_state_dirs(
        mut self,
        tool_name: &str,
        project_root: &Path,
        session_dir: &Path,
        tool_state_dirs: Option<&HashMap<String, PathBuf>>,
    ) -> Self {
        self.apply_tool_defaults(tool_name, project_root, session_dir, tool_state_dirs);
        self
    }

    fn apply_tool_defaults(
        &mut self,
        tool_name: &str,
        project_root: &Path,
        session_dir: &Path,
        tool_state_dirs: Option<&HashMap<String, PathBuf>>,
    ) {
        self.project_root = Some(project_root.to_path_buf());
        self.writable_paths.push(project_root.to_path_buf());
        self.writable_paths.push(session_dir.to_path_buf());
        let sandbox_tmpdir =
            runtime_path::sandbox_tmpdir_for_capability(self.filesystem, session_dir);
        self.env_overrides.insert(
            "TMPDIR".to_string(),
            sandbox_tmpdir.to_string_lossy().into_owned(),
        );
        if !matches!(self.filesystem, FilesystemCapability::Bwrap) {
            add_dir_or_creatable_parent(&mut self.writable_paths, &sandbox_tmpdir);
        }

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
            if let Ok(cargo_home_env) = std::env::var(csa_core::env::CARGO_HOME_ENV_KEY) {
                let cargo_home =
                    rust_env::resolve_rust_state_path(&cargo_home_env, &default_cargo_home);
                if cargo_home == default_cargo_home {
                    // CARGO_HOME points to the default — treat as if unset.
                    add_dir_or_creatable_parent(&mut self.writable_paths, &default_cargo_home);
                    // Override the env var when the original pointed at a
                    // read-only system prefix (e.g. /usr/local) so the child
                    // process uses the writable default instead (#2607).
                    rust_env::insert_env_override_if_needed(
                        &mut self.env_overrides,
                        csa_core::env::CARGO_HOME_ENV_KEY,
                        &cargo_home_env,
                        &default_cargo_home,
                    );
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
            if let Ok(rustup_home) = std::env::var(csa_core::env::RUSTUP_HOME_ENV_KEY) {
                let rustup_path = rust_env::resolve_rust_state_path(&rustup_home, &default_rustup);
                if rustup_path == default_rustup {
                    add_dir_or_creatable_parent(&mut self.writable_paths, &default_rustup);
                    // Same env override as CARGO_HOME (#2607).
                    rust_env::insert_env_override_if_needed(
                        &mut self.env_overrides,
                        csa_core::env::RUSTUP_HOME_ENV_KEY,
                        &rustup_home,
                        &default_rustup,
                    );
                } else {
                    // RUSTUP_HOME points elsewhere — only expose that directory.
                    add_dir_or_creatable_parent(&mut self.writable_paths, &rustup_path);
                }
            } else {
                add_dir_or_creatable_parent(&mut self.writable_paths, &default_rustup);
            }

            // RUSTUP_TOOLCHAIN: ambient value is inherited by the child
            // process normally. Explicit tool configuration via extra_env
            // overrides ambient values through build_merged_env(). We do not
            // add RUSTUP_TOOLCHAIN to env_overrides here because bwrap
            // --setenv is applied after the merged execution_env and would
            // override configured values (#2661).

            // Do not make mise-managed toolchain install dirs writable; only the
            // registry/git cache subdirs above need write access.

            // Expose existing CODEX_HOME for every sandboxed parent so nested
            // Codex CSA children inherit a writable source path.
            codex_paths::add_codex_home_for_tool(
                tool_name,
                &home,
                tool_state_dirs,
                &mut self.writable_paths,
                &mut self.required_writable_dirs,
            );

            // Expose the claude home (~/.claude or $CLAUDE_CONFIG_DIR) writable
            // for every sandboxed parent so a nested claude-code CSA child can
            // create ~/.claude/session-env/<id> instead of hitting EROFS under a
            // read-only HOME. Mirrors the codex helper above; see
            // isolation_plan_claude.rs for the #1683 root cause and the ~/.codex
            // symmetry justification.
            claude_paths::add_claude_home_for_tool(
                tool_name,
                &home,
                tool_state_dirs,
                &mut self.writable_paths,
                &mut self.required_writable_dirs,
            );

            match tool_name {
                "gemini-cli" => [".gemini", ".config/gemini-cli"]
                    .into_iter()
                    .map(|rel| home.join(rel))
                    .filter(|path| path.exists())
                    .for_each(|path| self.writable_paths.push(path)),
                "opencode" => {
                    let p = home.join(".config/opencode");
                    if p.exists() {
                        self.writable_paths.push(p);
                    }
                }
                _ => {}
            }
        }
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

        self.add_runtime_daemon_socket_readable_paths();

        codex_paths::validate_required_writable_dirs(
            self.filesystem,
            &self.required_writable_dirs,
            &self.writable_paths,
        )?;

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
            user_daemon_ipc: self.user_daemon_ipc,
        })
    }

    fn add_runtime_daemon_socket_readable_paths(&mut self) {
        if self.filesystem != FilesystemCapability::Bwrap {
            return;
        }
        // #2404: D-Bus socket exposure is now an explicit named capability,
        // not an implicit side-effect of having writable runtime children.
        if !self.user_daemon_ipc {
            return;
        }
        let Some(runtime_root) = runtime_path::xdg_runtime_root() else {
            return;
        };

        for socket_path in runtime_path::runtime_daemon_socket_paths(&runtime_root) {
            if !socket_path.exists() || self.path_already_exposed(&socket_path) {
                continue;
            }
            self.readable_paths.push(socket_path);
        }
    }

    #[allow(dead_code)]
    fn has_writable_runtime_child(&self, runtime_root: &Path) -> bool {
        self.writable_paths.iter().any(|path| {
            let comparable = runtime_path::canonicalize_or_fallback(path);
            comparable.starts_with(runtime_root) && comparable != runtime_root
        })
    }

    fn path_already_exposed(&self, path: &Path) -> bool {
        self.readable_paths
            .iter()
            .any(|candidate| path == candidate)
            || self
                .writable_paths
                .iter()
                .any(|candidate| path.starts_with(candidate))
    }
}

fn add_dir_or_creatable_parent(paths: &mut Vec<PathBuf>, dir: &Path) -> bool {
    runtime_path::add_dir_or_creatable_parent(paths, dir)
}

#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "isolation_plan_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "isolation_plan_path_tests.rs"]
mod path_tests;

#[cfg(test)]
#[path = "isolation_plan_rust_env_tests.rs"]
mod rust_env_tests;

#[cfg(test)]
#[path = "isolation_plan_tmpdir_tests.rs"]
mod tmpdir_tests;

#[cfg(test)]
#[path = "isolation_plan_claude_tests.rs"]
mod claude_tests;

#[cfg(test)]
#[path = "isolation_plan_daemon_ipc_tests.rs"]
mod daemon_ipc_tests;
