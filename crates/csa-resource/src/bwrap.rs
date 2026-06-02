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
    readable_paths: Vec<PathBuf>,
    ro_binds: Vec<(PathBuf, PathBuf)>,
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
    pub fn with_readable_path(&mut self, path: &Path) -> &mut Self {
        assert!(
            path.is_absolute(),
            "readable sandbox path must be absolute: {}",
            path.display()
        );
        assert!(
            path != Path::new("/tmp"),
            "readable sandbox path must not be /tmp itself; expose a specific sub-path instead"
        );
        assert!(
            path.exists(),
            "readable sandbox path must exist: {}",
            path.display()
        );
        self.readable_paths.push(path.to_path_buf());
        self
    }

    /// Add an extra read-only bind mount beyond the default `/ → /`.
    pub fn with_ro_bind(&mut self, src: &Path, dest: &Path) -> &mut Self {
        self.ro_binds.push((src.to_path_buf(), dest.to_path_buf()));
        self
    }

    /// Inject an environment variable into the sandboxed process.
    pub fn with_env(&mut self, key: &str, value: &str) -> &mut Self {
        self.env_vars.push((key.to_owned(), value.to_owned()));
        self
    }

    /// Consume the builder and produce a ready-to-spawn [`Command`].
    pub fn build(&self) -> Command {
        self.build_with_home(std::env::var_os("HOME").as_deref().map(Path::new))
    }

    fn build_with_home(&self, home: Option<&Path>) -> Command {
        let mut cmd = Command::new("bwrap");

        // Read-only root filesystem
        cmd.args(["--ro-bind", "/", "/"]);

        // Standard virtual filesystems MUST come before bind mounts so that
        // writable paths under /tmp are not hidden by the fresh tmpfs overlay.
        cmd.args(["--tmpfs", "/tmp"]);
        cmd.args(["--dev", "/dev"]);
        cmd.args(["--proc", "/proc"]);

        // Bind after tmpfs. /tmp itself is an explicit host tmpdir grant.
        let tmp_prefix = Path::new("/tmp");
        for path in &self.writable_paths {
            let s = path.to_string_lossy();
            if path == tmp_prefix {
                cmd.args(["--bind", &s, &s]);
            } else if path.starts_with(tmp_prefix) {
                if let Some(parent) = path.parent()
                    && parent != tmp_prefix
                {
                    let p = parent.to_string_lossy();
                    cmd.args(["--dir", &p]);
                }
                if !(path.is_file() || (!path.exists() && path.extension().is_some())) {
                    cmd.args(["--dir", &s]);
                }
                cmd.args(["--bind", &s, &s]);
            } else {
                cmd.args(["--bind", &s, &s]);
            }
        }

        // Read-only readable paths. For /tmp files, only create parent dirs.
        for path in &self.readable_paths {
            let s = path.to_string_lossy();
            assert!(
                path != tmp_prefix,
                "readable sandbox path must not be /tmp itself; expose a specific sub-path instead"
            );
            if path.starts_with(tmp_prefix)
                && let Some(parent) = path.parent()
                && parent != tmp_prefix
            {
                cmd.args(["--dir", &parent.to_string_lossy()]);
            }
            cmd.args(["--ro-bind", &s, &s]);
        }

        // Extra read-only bind mounts.  When the dest path differs from src
        // (remapped HOME), the mount target may not exist inside the sandbox
        // (e.g. Gemini runtime home only seeds gemini-cli config, not gh-aider).
        // Emit --dir for the dest parent so bubblewrap can create the mount point.
        for (src, dest) in self
            .ro_binds
            .iter()
            .cloned()
            .chain(self.implicit_ro_binds(home))
        {
            if src != dest
                && let Some(parent) = dest.parent()
            {
                cmd.args(["--dir", &parent.to_string_lossy()]);
            }
            cmd.args(["--ro-bind", &src.to_string_lossy(), &dest.to_string_lossy()]);
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

        cmd
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

    fn implicit_ro_binds(&self, home: Option<&Path>) -> impl Iterator<Item = (PathBuf, PathBuf)> {
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
                || self
                    .ro_binds
                    .iter()
                    .any(|(src, dest)| src == &gh_aider || dest == &sandbox_gh_aider);
            if gh_aider.exists() && !already_visible {
                ro_binds.push((gh_aider, sandbox_gh_aider));
            }
        }

        ro_binds.into_iter()
    }
}

/// Build a bwrap [`Command`] from an [`IsolationPlan`] if the plan calls
/// for bubblewrap filesystem isolation.
///
/// Returns `Some(Command)` when `plan.filesystem == FilesystemCapability::Bwrap`,
/// `None` otherwise.
pub fn from_isolation_plan(
    plan: &IsolationPlan,
    tool_binary: &str,
    tool_args: &[String],
) -> Option<Command> {
    if plan.filesystem != FilesystemCapability::Bwrap {
        return None;
    }

    let mut builder = BwrapCommandBuilder::new(tool_binary, tool_args);

    for path in &plan.writable_paths {
        // When readonly_project_root is set, mount the project root as
        // read-only instead of read-write.
        let is_project_root = plan.project_root.as_ref().is_some_and(|root| path == root);
        if plan.readonly_project_root && is_project_root {
            builder.with_ro_bind(path, path);
        } else {
            builder.with_writable_path(path);
        }
    }

    for path in &plan.readable_paths {
        builder.with_readable_path(path);
    }

    for (key, value) in &plan.env_overrides {
        if csa_core::env::is_startup_subtree_env_key(key) {
            continue;
        }
        builder.with_env(key, value);
    }

    Some(builder.build())
}

#[cfg(test)]
#[path = "bwrap_tests.rs"]
mod tests;
