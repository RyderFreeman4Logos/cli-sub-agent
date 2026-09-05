//! Bubblewrap command builder for filesystem sandboxing.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::filesystem_sandbox::FilesystemCapability;
use crate::isolation_plan::IsolationPlan;

/// Environment variable set inside the sandbox to signal filesystem isolation.
const CSA_FS_SANDBOXED_ENV: &str = "CSA_FS_SANDBOXED";

/// Builder for constructing a `bwrap` command with explicit read/write binds.
pub struct BwrapCommandBuilder {
    tool_binary: String,
    tool_args: Vec<String>,
    writable_paths: Vec<PathBuf>,
    readable_paths: Vec<crate::isolation_plan::ReadablePath>,
    ro_binds: Vec<crate::isolation_plan::ReadablePath>,
    env_vars: Vec<(String, String)>,
}

impl BwrapCommandBuilder {
    /// Create a new builder that will wrap the given tool binary and arguments.
    pub fn new(tool_binary: &str, tool_args: &[String]) -> Self {
        Self {
            tool_binary: tool_binary.to_owned(),
            tool_args: tool_args.to_vec(),
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            ro_binds: Vec::new(),
            env_vars: Vec::new(),
        }
    }

    /// Add a path that the sandboxed process may write to (bind-mounted rw).
    pub fn with_writable_path(&mut self, path: &Path) -> &mut Self {
        self.writable_paths.push(path.to_path_buf());
        self
    }

    /// Add a path that the sandboxed process may read (bind-mounted ro).
    ///
    /// A [`crate::isolation_plan::ReadablePath`] keeps its validated bind
    /// source. A bare path is pinned at this call.
    pub fn with_readable_path(
        &mut self,
        path: impl Into<crate::isolation_plan::ReadablePath>,
    ) -> &mut Self {
        self.readable_paths.push(path.into());
        self
    }

    /// Add an extra read-only bind mount beyond the default `/ → /`.
    pub fn with_ro_bind(&mut self, src: &Path, dest: &Path) -> &mut Self {
        self.ro_binds
            .push(crate::isolation_plan::ReadablePath::pinned_extra(
                dest.to_path_buf(),
                src.to_path_buf(),
            ));
        self
    }

    /// Inject an environment variable into the sandboxed process.
    pub fn with_env(&mut self, key: &str, value: &str) -> &mut Self {
        self.env_vars.push((key.to_owned(), value.to_owned()));
        self
    }

    /// Produce a ready-to-spawn command, or return a path validation/pinning error.
    pub fn build(&self) -> std::io::Result<Command> {
        self.build_with_home(std::env::var_os("HOME").as_deref().map(Path::new))
    }

    fn build_with_home(&self, home: Option<&Path>) -> std::io::Result<Command> {
        let mut cmd = Command::new("bwrap");
        let extra_ro_binds: Vec<_> = self
            .ro_binds
            .iter()
            .cloned()
            .chain(self.implicit_ro_binds(home))
            .collect();

        for path in self.readable_paths.iter().chain(&extra_ro_binds) {
            validate_readonly_path(path)?;
        }

        // Read-only root filesystem
        cmd.args(["--ro-bind", "/", "/"]);

        // Standard virtual filesystems MUST come before bind mounts so that
        // writable paths under /tmp are not hidden by the fresh tmpfs overlay.
        cmd.args(["--tmpfs", "/tmp"]);
        cmd.args(["--dev", "/dev"]);
        cmd.args(["--proc", "/proc"]);

        // Bind after the fresh virtual filesystems. A writable path below a
        // replaced mount must keep its logical destination; otherwise its
        // canonical destination is hidden once the mount is installed.
        // Runtime writes nested under a later read-only overlay (Hermes logs /
        // SQLite sidecars) must follow that overlay or the overlay hides them.
        for path in &self.writable_paths {
            if !self.writable_nested_under_readonly_overlay(path) {
                Self::bind_writable_path(&mut cmd, path, self.pinned_writable_file(path));
            }
        }

        // Read-only readable paths. Writable grants already imply readability;
        // adding an overlapping read-only bind afterward would downgrade the
        // writable mount.
        #[cfg(unix)]
        let overlay_files: Vec<std::sync::Arc<std::fs::File>> = {
            let mut files = sandbox_bind_files_from_paths(&self.readable_paths);
            files.extend(
                extra_ro_binds
                    .iter()
                    .filter_map(crate::isolation_plan::ReadablePath::pinned_source_file),
            );
            files
        };
        for path in &self.readable_paths {
            if path.writable_bind()
                || self.is_covered_by_writable_path(path)
                || self.is_late_readonly_overlay(path)
            {
                continue;
            }
            Self::bind_readonly_path(&mut cmd, path);
        }

        for path in &self.writable_paths {
            if self.writable_nested_under_readonly_overlay(path) {
                Self::bind_writable_path(&mut cmd, path, self.pinned_writable_file(path));
            }
        }

        for path in &self.readable_paths {
            if self.is_late_readonly_overlay(path) {
                Self::bind_readonly_path(&mut cmd, path);
            }
        }

        // Extra read-only bind mounts.  When the dest path differs from src
        // (remapped HOME), the mount target may not exist inside the sandbox
        // (e.g. Gemini runtime home only seeds gemini-cli config, not gh-aider).
        // Emit --dir for the dest parent so bubblewrap can create the mount point.
        for path in &extra_ro_binds {
            Self::bind_extra_readonly_path(&mut cmd, path);
        }

        // Namespace configuration
        cmd.arg("--share-net");
        cmd.arg("--unshare-pid");
        cmd.arg("--die-with-parent");

        // Sandbox marker environment variable
        cmd.args(["--setenv", CSA_FS_SANDBOXED_ENV, "1"]);

        // User-supplied environment variables
        for (key, value) in &self.env_vars {
            cmd.args(["--setenv", key, value]);
        }

        // Separator and tool command
        cmd.arg("--");
        cmd.arg(&self.tool_binary);
        cmd.args(&self.tool_args);

        #[cfg(unix)]
        if !overlay_files.is_empty() {
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;
            // SAFETY: the closure only clears FD_CLOEXEC on descriptors this
            // command already holds so bwrap can inherit `--ro-bind-fd` sources.
            unsafe {
                cmd.pre_exec(move || {
                    for file in &overlay_files {
                        if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }

        Ok(cmd)
    }

    fn is_covered_by_writable_path(
        &self,
        readable_path: &crate::isolation_plan::ReadablePath,
    ) -> bool {
        if readable_path.overrides_writable_mount() {
            return false;
        }
        let readable_dest = if fresh_writable_mount_root(readable_path.requested()).is_some() {
            readable_path.requested().to_path_buf()
        } else {
            readable_path.bind_source().to_path_buf()
        };
        self.writable_paths.iter().any(|writable_path| {
            let writable_dest = effective_mount_destination(writable_path);
            readable_dest == writable_dest || readable_dest.starts_with(&writable_dest)
        })
    }

    fn is_late_readonly_overlay(&self, path: &crate::isolation_plan::ReadablePath) -> bool {
        path.overrides_writable_mount()
            && self.writable_paths.iter().any(|writable| {
                path.requested().starts_with(writable)
                    && path.requested() != writable
                    && self.writable_nested_under_readonly_overlay(writable)
            })
    }

    fn bind_readonly_path(cmd: &mut Command, path: &crate::isolation_plan::ReadablePath) {
        // Use the bind source pinned at validation/add time. Re-resolving here
        // would reopen the #3102 TOCTOU after a symlink replacement.
        let resolved = path.bind_source();
        let tmp_prefix = Path::new("/tmp");
        let dest_path = if path.overrides_writable_mount()
            || fresh_writable_mount_root(path.requested()).is_some()
        {
            path.requested()
        } else {
            resolved
        };
        if let Some(parent) = dest_path.parent()
            && parent != tmp_prefix
            && parent != Path::new("/")
        {
            cmd.args(["--dir", &parent.to_string_lossy()]);
        }
        #[cfg(unix)]
        if let Some(file) = path.pinned_source_file() {
            use std::os::fd::AsRawFd;
            cmd.args([
                "--ro-bind-fd",
                &file.as_raw_fd().to_string(),
                &dest_path.to_string_lossy(),
            ]);
            return;
        }
        cmd.args([
            "--ro-bind",
            &resolved.to_string_lossy(),
            &dest_path.to_string_lossy(),
        ]);
    }

    fn bind_extra_readonly_path(cmd: &mut Command, path: &crate::isolation_plan::ReadablePath) {
        let dest = path.requested();
        if path.bind_source() != dest
            && let Some(parent) = dest.parent()
            && parent != Path::new("/")
        {
            cmd.args(["--dir", &parent.to_string_lossy()]);
        }
        #[cfg(unix)]
        if let Some(file) = path.pinned_source_file() {
            use std::os::fd::AsRawFd;
            cmd.args([
                "--ro-bind-fd",
                &file.as_raw_fd().to_string(),
                &dest.to_string_lossy(),
            ]);
            return;
        }
        let src = resolve_for_bind(path.bind_source());
        cmd.args(["--ro-bind", &src.to_string_lossy(), &dest.to_string_lossy()]);
    }

    fn writable_nested_under_readonly_overlay(&self, path: &Path) -> bool {
        self.readable_paths.iter().any(|readable| {
            readable.overrides_writable_mount() && path.starts_with(readable.requested())
        })
    }

    fn pinned_writable_file(&self, path: &Path) -> Option<std::sync::Arc<std::fs::File>> {
        #[cfg(unix)]
        {
            self.readable_paths.iter().find_map(|readable| {
                (readable.writable_bind() && readable.requested() == path)
                    .then(|| readable.pinned_source_file())
                    .flatten()
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }

    fn bind_writable_path(
        cmd: &mut Command,
        path: &Path,
        pinned: Option<std::sync::Arc<std::fs::File>>,
    ) {
        #[cfg(unix)]
        if let Some(file) = pinned {
            use std::os::fd::AsRawFd;
            let dest = path.to_string_lossy();
            if let Some(parent) = path.parent()
                && parent != Path::new("/")
            {
                cmd.args(["--dir", &parent.to_string_lossy()]);
            }
            cmd.args(["--bind-fd", &file.as_raw_fd().to_string(), &dest]);
            return;
        }
        #[cfg(not(unix))]
        let _ = pinned;
        let resolved = resolve_for_bind(path);
        let s = resolved.to_string_lossy();
        let fresh_mount_root = fresh_writable_mount_root(path);
        // Elsewhere, bind at the resolved destination: bwrap cannot
        // create a mount target by walking a state-path symlink into an
        // autofs-backed CSA session-state root.
        let dest_path = if fresh_mount_root.is_some() {
            path
        } else {
            resolved.as_path()
        };
        let dest = dest_path.to_string_lossy();
        if let Some(mount_root) = fresh_mount_root {
            let mount_root = Path::new(mount_root);
            if path == mount_root {
                cmd.args(["--bind", &s, &dest]);
                return;
            }
            if let Some(parent) = path.parent()
                && parent != mount_root
            {
                let p = parent.to_string_lossy();
                cmd.args(["--dir", &p]);
            }
            if !(path.is_file() || (!path.exists() && path.extension().is_some())) {
                cmd.args(["--dir", &dest]);
            }
            cmd.args(["--bind", &s, &dest]);
        } else if let Some(parent) = dest_path.parent()
            && parent != Path::new("/")
        {
            // Ensure the resolved destination parent exists inside the
            // sandbox. Creating the logical parent can fail when it is a
            // symlink into an autofs-backed CSA session-state root.
            cmd.args(["--dir", &parent.to_string_lossy()]);
            cmd.args(["--bind", &s, &dest]);
        } else {
            cmd.args(["--bind", &s, &dest]);
        }
    }

    fn sandbox_home(&self, host_home: Option<&Path>) -> Option<PathBuf> {
        self.env_vars
            .iter()
            .rev()
            .find_map(|(key, value)| {
                (key == "HOME")
                    .then(|| PathBuf::from(value))
                    .filter(|path| path.is_absolute())
            })
            .or_else(|| host_home.map(Path::to_path_buf))
    }

    fn implicit_ro_binds(
        &self,
        home: Option<&Path>,
    ) -> impl Iterator<Item = crate::isolation_plan::ReadablePath> {
        let mut ro_binds = Vec::new();

        if let Some(home) = home {
            let gh_aider = home.join(".config/gh-aider");
            let sandbox_gh_aider = self
                .sandbox_home(Some(home))
                .unwrap_or_else(|| home.to_path_buf())
                .join(".config/gh-aider");
            // writable_paths are HOST paths — only compare against the HOST
            // gh_aider path.  Comparing sandbox_gh_aider against host writable
            // paths falsely matches when sandbox HOME is under a writable
            // session dir (common in Gemini ACP).
            let already_visible = self
                .writable_paths
                .iter()
                .any(|existing| existing == &gh_aider || gh_aider.starts_with(existing))
                || self.ro_binds.iter().any(|existing| {
                    existing.bind_source() == gh_aider || existing.requested() == sandbox_gh_aider
                });
            let absent = std::fs::symlink_metadata(&gh_aider)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            if !absent && !already_visible {
                ro_binds.push(
                    crate::isolation_plan::ReadablePath::pinned_home_config(gh_aider)
                        .for_sandbox_home(self.sandbox_home(Some(home)).as_deref()),
                );
            }
        }

        ro_binds.into_iter()
    }
}

/// Capture every extra source before capability checks and executor projection.
/// The returned ReadablePaths use the same descriptor owners as ordinary binds.
pub(crate) fn plan_extra_readonly_paths(
    writable_paths: &[PathBuf],
    readonly_project: Option<&Path>,
    home: Option<&Path>,
) -> std::io::Result<Vec<crate::isolation_plan::ReadablePath>> {
    let mut builder = BwrapCommandBuilder::new("", &[]);
    for path in writable_paths {
        if readonly_project == Some(path.as_path()) {
            builder.with_ro_bind(path, path);
        } else {
            builder.with_writable_path(path);
        }
    }
    let implicit: Vec<_> = builder.implicit_ro_binds(home).collect();
    builder.ro_binds.extend(implicit);
    for path in &builder.ro_binds {
        validate_readonly_path(path)?;
    }
    Ok(builder.ro_binds)
}

fn validate_readonly_path(path: &crate::isolation_plan::ReadablePath) -> std::io::Result<()> {
    if !path.requested().is_absolute() || path.requested() == Path::new("/tmp") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "readable sandbox path must be absolute and must not be /tmp itself",
        ));
    }
    path.validate_pin()
}

/// Build a bwrap [`Command`] from an [`IsolationPlan`] if the plan calls
/// for bubblewrap filesystem isolation.
///
/// Returns `Some(Command)` when `plan.filesystem == FilesystemCapability::Bwrap`,
/// `None` otherwise. Invalid or unpinnable read-only sources return an error.
pub fn from_isolation_plan(
    plan: &IsolationPlan,
    tool_binary: &str,
    tool_args: &[String],
) -> std::io::Result<Option<Command>> {
    if plan.filesystem != FilesystemCapability::Bwrap {
        return Ok(None);
    }

    let mut builder = BwrapCommandBuilder::new(tool_binary, tool_args);

    for path in &plan.writable_paths {
        // When readonly_project_root is set, mount the project root as
        // read-only instead of read-write.
        let is_project_root = plan.project_root.as_ref().is_some_and(|root| path == root);
        if plan.readonly_project_root && is_project_root {
            if !plan
                .readable_paths
                .iter()
                .any(|readable| readable.is_extra_bind() && readable.requested() == path)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "read-only project bind must be pinned by IsolationPlanBuilder",
                ));
            }
        } else {
            builder.with_writable_path(path);
        }
    }

    for path in &plan.readable_paths {
        if path.is_extra_bind() {
            builder
                .ro_binds
                .push(path.for_sandbox_home(plan.env_overrides.get("HOME").map(Path::new)));
        } else {
            builder.with_readable_path(path.clone());
        }
    }

    let mut env_overrides = plan.env_overrides.clone();
    csa_core::env::scrub_subtree_contract_env_map(&mut env_overrides);
    csa_core::env::strip_git_push_authorization_keys(&mut env_overrides);
    for (key, value) in &env_overrides {
        builder.with_env(key, value);
    }

    // Implicit sources were captured in the plan. Reopening them here would
    // detach their FDs from capability checks and reconstructed spawn commands.
    builder.build_with_home(None).map(Some)
}

/// Number of descriptors for all planned bind mounts, including extra mounts.
pub fn sandbox_bind_fd_count(plan: &IsolationPlan) -> usize {
    sandbox_bind_files(plan).len()
}

pub(crate) fn readable_bind_fd_count(
    readable_paths: &[crate::isolation_plan::ReadablePath],
) -> usize {
    sandbox_bind_files_from_paths(readable_paths).len()
}

#[cfg(unix)]
fn sandbox_bind_files_from_paths(
    readable_paths: &[crate::isolation_plan::ReadablePath],
) -> Vec<std::sync::Arc<std::fs::File>> {
    readable_paths
        .iter()
        .filter_map(crate::isolation_plan::ReadablePath::pinned_source_file)
        .collect()
}

#[cfg(unix)]
fn sandbox_bind_files(plan: &IsolationPlan) -> Vec<std::sync::Arc<std::fs::File>> {
    sandbox_bind_files_from_paths(&plan.readable_paths)
}

#[cfg(not(unix))]
fn sandbox_bind_files(_plan: &IsolationPlan) -> Vec<()> {
    Vec::new()
}

#[cfg(not(unix))]
fn sandbox_bind_files_from_paths(
    _readable_paths: &[crate::isolation_plan::ReadablePath],
) -> Vec<()> {
    Vec::new()
}

/// Keep `--ro-bind-fd` / `--bind-fd` descriptors open across `exec`.
///
/// ACP and cgroup paths reconstruct a command from program+args and must call
/// this on the final spawn command.
pub fn inherit_sandbox_bind_fds(cmd: &mut Command, plan: &IsolationPlan) {
    #[cfg(unix)]
    {
        let files = sandbox_bind_files(plan);
        if files.is_empty() {
            return;
        }
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure only clears FD_CLOEXEC on descriptors the plan
        // still holds so bwrap can inherit `--ro-bind-fd` / `--bind-fd` sources.
        unsafe {
            cmd.pre_exec(move || {
                for file in &files {
                    if libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, plan);
    }
}

/// Inherit bind FDs, or fail closed when the reconstructed command cannot.
pub fn try_inherit_sandbox_bind_fds(
    cmd: &mut Command,
    plan: &IsolationPlan,
) -> std::io::Result<()> {
    if sandbox_bind_fd_count(plan) == 0 {
        return Ok(());
    }
    let program = Path::new(cmd.get_program());
    if program == Path::new("systemd-run")
        || program.file_name() == Some(std::ffi::OsStr::new("systemd-run"))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bwrap bind-fd cannot pass through systemd-run; overlay --ro-bind-fd would be dropped",
        ));
    }
    inherit_sandbox_bind_fds(cmd, plan);
    Ok(())
}

/// Resolve the destination used for a bind mount. Fresh virtual filesystems
/// keep their logical paths because their resolved host paths are hidden.
fn effective_mount_destination(path: &Path) -> PathBuf {
    if fresh_writable_mount_root(path).is_some() {
        path.to_path_buf()
    } else {
        resolve_for_bind(path)
    }
}

/// Resolve a bind source path by following symlinks. bwrap operates in the
/// root namespace where symlink targets must be real paths — if `~/.claude`
/// is a symlink to `/ssd/.../.claude`, bwrap needs the resolved target.
fn resolve_for_bind(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Return the fresh writable virtual filesystem containing `path`.
///
/// `/proc` is deliberately excluded: it is a read-only process virtual
/// filesystem and cannot host writable bind destinations.
fn fresh_writable_mount_root(path: &Path) -> Option<&'static str> {
    ["/tmp", "/dev"]
        .into_iter()
        .find(|mount_root| path.starts_with(Path::new(mount_root)))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "bwrap_tests.rs"]
mod tests;
