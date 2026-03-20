//! Bubblewrap command builder for filesystem sandboxing.
//!
//! Constructs a `bwrap` invocation that wraps a tool binary inside a
//! read-only root filesystem with selective writable bind mounts,
//! PID isolation, and parent-death signalling.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::filesystem_sandbox::FilesystemCapability;
use crate::isolation_plan::IsolationPlan;

/// Environment variable set inside the sandbox to signal filesystem isolation.
const CSA_FS_SANDBOXED_ENV: &str = "CSA_FS_SANDBOXED";

/// Builder for constructing a `bwrap` (bubblewrap) command.
///
/// Default configuration:
/// - `--ro-bind / /` — read-only root filesystem
/// - `--tmpfs /tmp` — writable scratch space
/// - `--dev /dev` — device nodes
/// - `--proc /proc` — process information
/// - `--share-net` — keep network access
/// - `--unshare-pid` — PID namespace isolation
/// - `--die-with-parent` — child dies when parent exits
/// - `--setenv CSA_FS_SANDBOXED 1` — sandbox marker
pub struct BwrapCommandBuilder {
    tool_binary: String,
    tool_args: Vec<String>,
    writable_paths: Vec<PathBuf>,
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
            ro_binds: Vec::new(),
            env_vars: Vec::new(),
        }
    }

    /// Add a path that the sandboxed process may write to (bind-mounted rw).
    pub fn with_writable_path(&mut self, path: &Path) -> &mut Self {
        self.writable_paths.push(path.to_path_buf());
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
        let mut cmd = Command::new("bwrap");

        // Read-only root filesystem
        cmd.args(["--ro-bind", "/", "/"]);

        // Writable bind mounts
        for path in &self.writable_paths {
            let s = path.to_string_lossy();
            cmd.args(["--bind", &s, &s]);
        }

        // Extra read-only bind mounts
        for (src, dest) in &self.ro_binds {
            cmd.args(["--ro-bind", &src.to_string_lossy(), &dest.to_string_lossy()]);
        }

        // Standard virtual filesystems
        cmd.args(["--tmpfs", "/tmp"]);
        cmd.args(["--dev", "/dev"]);
        cmd.args(["--proc", "/proc"]);

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
        builder.with_writable_path(path);
    }

    for (key, value) in &plan.env_overrides {
        builder.with_env(key, value);
    }

    Some(builder.build())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::sandbox::ResourceCapability;

    /// Helper: extract the full argument list from a Command via Debug output.
    fn command_args(cmd: &Command) -> Vec<String> {
        let debug = format!("{cmd:?}");
        // Debug format: "bwrap" "--ro-bind" "/" "/" ...
        // Parse quoted strings out of the debug representation.
        debug
            .split('"')
            .enumerate()
            .filter_map(|(i, s)| if i % 2 == 1 { Some(s.to_owned()) } else { None })
            .collect()
    }

    #[test]
    fn test_bwrap_command_basic() {
        let builder = BwrapCommandBuilder::new("/usr/bin/tool", &["--flag".into(), "arg".into()]);
        let cmd = builder.build();
        let args = command_args(&cmd);

        // Program is bwrap
        assert_eq!(args[0], "bwrap");

        // Core args present
        assert!(args.contains(&"--ro-bind".to_owned()));
        assert!(args.contains(&"--tmpfs".to_owned()));
        assert!(args.contains(&"--dev".to_owned()));
        assert!(args.contains(&"--proc".to_owned()));
        assert!(args.contains(&"--share-net".to_owned()));
        assert!(args.contains(&"--unshare-pid".to_owned()));
        assert!(args.contains(&"--die-with-parent".to_owned()));

        // Separator and tool binary
        assert!(args.contains(&"--".to_owned()));
        assert!(args.contains(&"/usr/bin/tool".to_owned()));
        assert!(args.contains(&"--flag".to_owned()));
        assert!(args.contains(&"arg".to_owned()));
    }

    #[test]
    fn test_bwrap_command_with_writable_paths() {
        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_writable_path(Path::new("/home/user/project"));
        builder.with_writable_path(Path::new("/tmp/session"));
        let cmd = builder.build();
        let args = command_args(&cmd);

        // Count --bind occurrences (writable bind mounts)
        let bind_positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--bind")
            .map(|(i, _)| i)
            .collect();

        assert_eq!(
            bind_positions.len(),
            2,
            "expected 2 writable --bind mounts, got {bind_positions:?}"
        );

        assert!(args.contains(&"/home/user/project".to_owned()));
        assert!(args.contains(&"/tmp/session".to_owned()));
    }

    #[test]
    fn test_bwrap_from_isolation_plan_bwrap() {
        let plan = IsolationPlan {
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::Bwrap,
            writable_paths: vec![PathBuf::from("/project")],
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
        };

        let result = from_isolation_plan(&plan, "/usr/bin/tool", &["run".into()]);
        assert!(result.is_some(), "Bwrap plan should produce Some(Command)");

        let cmd = result.unwrap();
        let args = command_args(&cmd);
        assert!(args.contains(&"/project".to_owned()));
        assert!(args.contains(&"/usr/bin/tool".to_owned()));
    }

    #[test]
    fn test_bwrap_from_isolation_plan_none() {
        let plan = IsolationPlan {
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::None,
            writable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
        };

        let result = from_isolation_plan(&plan, "/usr/bin/tool", &[]);
        assert!(result.is_none(), "Non-Bwrap plan should produce None");
    }

    #[test]
    fn test_bwrap_env_passthrough() {
        let builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        let cmd = builder.build();
        let args = command_args(&cmd);

        // Find --setenv CSA_FS_SANDBOXED 1 sequence
        let setenv_positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--setenv")
            .map(|(i, _)| i)
            .collect();

        let found_sandbox_env = setenv_positions.iter().any(|&pos| {
            args.get(pos + 1).map(|s| s.as_str()) == Some("CSA_FS_SANDBOXED")
                && args.get(pos + 2).map(|s| s.as_str()) == Some("1")
        });

        assert!(
            found_sandbox_env,
            "CSA_FS_SANDBOXED=1 must be set via --setenv; args: {args:?}"
        );
    }
}
