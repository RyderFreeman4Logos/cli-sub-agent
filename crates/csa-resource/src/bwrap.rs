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

        // Writable bind mounts (after tmpfs). /tmp itself is skipped; /tmp
        // sub-paths get pre-created mount points inside the fresh tmpfs.
        let tmp_prefix = Path::new("/tmp");
        for path in &self.writable_paths {
            let s = path.to_string_lossy();
            if path == tmp_prefix {
                continue;
            } else if path.starts_with(tmp_prefix) {
                if let Some(parent) = path.parent()
                    && parent != tmp_prefix
                {
                    let p = parent.to_string_lossy();
                    cmd.args(["--dir", &p]);
                }
                cmd.args(["--dir", &s]);
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

    #[test]
    fn test_bwrap_gh_aider_bind_targets_overridden_sandbox_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_home = temp.path().join("host-home");
        let gh_aider = host_home.join(".config/gh-aider");
        std::fs::create_dir_all(&gh_aider).expect("create gh-aider config");

        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_env("HOME", "/sandbox/runtime-home");
        let cmd = builder.build_with_home(Some(&host_home));
        let args = command_args(&cmd);

        let found_bind = args.windows(3).any(|window| {
            window[0] == "--ro-bind"
                && window[1] == gh_aider.to_string_lossy()
                && window[2] == "/sandbox/runtime-home/.config/gh-aider"
        });

        assert!(
            found_bind,
            "gh-aider config should bind from the host path into the sandbox HOME; args: {args:?}"
        );

        // --dir must precede the bind to create the mount target inside sandbox
        let dir_pos = args
            .iter()
            .enumerate()
            .find(|(_, a)| *a == "--dir" || a.as_str() == "/sandbox/runtime-home/.config")
            .map(|(i, _)| i);
        let bind_pos = args
            .iter()
            .enumerate()
            .find(|(i, _)| {
                args.get(*i + 1).map(|s| s.as_str()) == Some(&gh_aider.to_string_lossy())
            })
            .map(|(i, _)| i);
        if let (Some(d), Some(b)) = (dir_pos, bind_pos) {
            assert!(
                d < b,
                "--dir for mount target must precede --ro-bind; args: {args:?}"
            );
        }
    }

    #[test]
    fn test_bwrap_gh_aider_bind_not_skipped_when_session_dir_writable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let host_home = temp.path().join("host-home");
        let gh_aider = host_home.join(".config/gh-aider");
        std::fs::create_dir_all(&gh_aider).expect("create gh-aider config");

        // Simulate Gemini layout: sandbox HOME is under a writable session dir
        let session_dir = temp.path().join("session");
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        let sandbox_home = session_dir.join("runtime/home");

        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_env("HOME", &sandbox_home.to_string_lossy());
        builder.writable_paths.push(session_dir);
        let cmd = builder.build_with_home(Some(&host_home));
        let args = command_args(&cmd);

        let found_bind = args.windows(3).any(|window| {
            window[0] == "--ro-bind"
                && window[1] == gh_aider.to_string_lossy()
                && window[2] == sandbox_home.join(".config/gh-aider").to_string_lossy()
        });

        assert!(
            found_bind,
            "gh-aider bind must NOT be skipped just because sandbox HOME is under a writable session dir; args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_from_isolation_plan_sets_tmpdir_override() {
        let mut env_overrides = HashMap::new();
        env_overrides.insert("TMPDIR".to_string(), "/tmp".to_string());
        let plan = IsolationPlan {
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::Bwrap,
            writable_paths: vec![PathBuf::from("/project")],
            readable_paths: Vec::new(),
            env_overrides,
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: Some(PathBuf::from("/project")),
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        };

        let cmd = from_isolation_plan(&plan, "/usr/bin/tool", &[]).expect("should produce command");
        let args = command_args(&cmd);
        let found_tmpdir_env = args
            .windows(3)
            .any(|window| window == ["--setenv", "TMPDIR", "/tmp"]);

        assert!(
            found_tmpdir_env,
            "TMPDIR must be pinned inside bwrap env overrides; args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_readonly_project_root() {
        let plan = IsolationPlan {
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::Bwrap,
            writable_paths: vec![PathBuf::from("/project"), PathBuf::from("/tmp/session")],
            readable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: true,
            project_root: Some(PathBuf::from("/project")),
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        };

        let cmd = from_isolation_plan(&plan, "/usr/bin/tool", &[]).expect("should produce command");
        let args = command_args(&cmd);

        // /project should appear after --ro-bind (not --bind)
        let ro_bind_positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--ro-bind")
            .map(|(i, _)| i)
            .collect();
        let project_after_ro = ro_bind_positions
            .iter()
            .any(|&pos| args.get(pos + 1).map(|s| s.as_str()) == Some("/project"));
        assert!(
            project_after_ro,
            "/project should be --ro-bind when readonly_project_root is true; args: {args:?}"
        );

        // /tmp/session should still be --bind (writable)
        let bind_positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--bind")
            .map(|(i, _)| i)
            .collect();
        let session_after_bind = bind_positions
            .iter()
            .any(|&pos| args.get(pos + 1).map(|s| s.as_str()) == Some("/tmp/session"));
        assert!(
            session_after_bind,
            "/tmp/session should be --bind (writable); args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_extra_writable_under_tmp_ordering() {
        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_writable_path(Path::new("/home/user/project"));
        builder.with_writable_path(Path::new("/tmp/foo"));
        let cmd = builder.build();
        let args = command_args(&cmd);

        // Locate key positions
        let pos = |needle: &str| args.iter().position(|a| a == needle);

        let tmpfs_pos = args
            .iter()
            .enumerate()
            .find(|(_, a)| *a == "--tmpfs")
            .map(|(i, _)| i)
            .expect("--tmpfs must be present");
        assert_eq!(
            args[tmpfs_pos + 1],
            "/tmp",
            "--tmpfs must be followed by /tmp"
        );

        // --tmpfs /tmp must appear BEFORE --bind /tmp/foo /tmp/foo
        let bind_tmp_foo_pos = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--bind")
            .find(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/foo"))
            .map(|(i, _)| i)
            .expect("--bind /tmp/foo must be present");
        assert!(
            tmpfs_pos < bind_tmp_foo_pos,
            "--tmpfs /tmp (pos {tmpfs_pos}) must come BEFORE --bind /tmp/foo (pos {bind_tmp_foo_pos}); args: {args:?}"
        );

        // --dir /tmp/foo must appear between --tmpfs and --bind for /tmp paths
        let dir_tmp_foo_pos = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--dir")
            .find(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/foo"))
            .map(|(i, _)| i)
            .expect("--dir /tmp/foo must be present for /tmp sub-paths");
        assert!(
            tmpfs_pos < dir_tmp_foo_pos && dir_tmp_foo_pos < bind_tmp_foo_pos,
            "--dir /tmp/foo (pos {dir_tmp_foo_pos}) must be between --tmpfs (pos {tmpfs_pos}) and --bind (pos {bind_tmp_foo_pos}); args: {args:?}"
        );

        // Non-/tmp writable path should NOT have --dir
        let dir_project = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--dir")
            .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/home/user/project"));
        assert!(
            !dir_project,
            "non-/tmp path /home/user/project should NOT have --dir; args: {args:?}"
        );

        // Non-/tmp writable path should still have --bind
        let bind_project = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--bind")
            .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/home/user/project"));
        assert!(
            bind_project,
            "/home/user/project should have --bind; args: {args:?}"
        );

        // Verify --ro-bind / / is still first
        assert_eq!(args[1], "--ro-bind");
        assert_eq!(args[2], "/");
        assert_eq!(args[3], "/");

        // Verify overall order: --ro-bind < --tmpfs < --bind < --
        let separator_pos = pos("--").expect("-- separator must be present");
        assert!(
            bind_tmp_foo_pos < separator_pos,
            "--bind must come before -- separator"
        );
    }

    #[test]
    fn test_bwrap_command_with_readable_tmp_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let readable = temp.path().join("foo.json");
        std::fs::write(&readable, "{}").expect("write readable file");

        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_readable_path(&readable);
        let cmd = builder.build();
        let args = command_args(&cmd);
        let readable_str = readable.to_string_lossy().into_owned();

        let tmpfs_pos = args
            .iter()
            .position(|arg| arg == "--tmpfs")
            .expect("--tmpfs must be present");
        let ro_bind_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind" && window[1] == readable_str && window[2] == readable_str
            })
            .expect("--ro-bind readable path must be present");

        assert_eq!(args[tmpfs_pos + 1], "/tmp");
        assert!(
            tmpfs_pos < ro_bind_pos,
            "readable --ro-bind must come after --tmpfs /tmp; args: {args:?}"
        );
    }

    #[test]
    #[should_panic(expected = "must not be /tmp itself")]
    fn test_bwrap_readable_tmp_root_rejected() {
        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_readable_path(Path::new("/tmp"));
    }

    #[test]
    fn test_bwrap_readable_and_writable_paths_after_tmpfs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let readable = temp.path().join("bar.txt");
        std::fs::write(&readable, "hello").expect("write readable file");

        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_writable_path(Path::new("/tmp/work"));
        builder.with_readable_path(&readable);
        let cmd = builder.build();
        let args = command_args(&cmd);
        let readable_str = readable.to_string_lossy().into_owned();

        let tmpfs_pos = args
            .iter()
            .position(|arg| arg == "--tmpfs")
            .expect("--tmpfs must be present");
        let writable_bind_pos = args
            .windows(3)
            .position(|window| window[0] == "--bind" && window[1] == "/tmp/work")
            .expect("writable bind should be present");
        let readable_bind_pos = args
            .windows(3)
            .position(|window| {
                window[0] == "--ro-bind" && window[1] == readable_str && window[2] == readable_str
            })
            .expect("readable ro-bind should be present");

        assert!(
            tmpfs_pos < writable_bind_pos,
            "writable bind must come after tmpfs; args: {args:?}"
        );
        assert!(
            tmpfs_pos < readable_bind_pos,
            "readable bind must come after tmpfs; args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_nested_tmp_path_creates_intermediate_dirs() {
        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_writable_path(Path::new("/tmp/deep/nested/dir"));
        let cmd = builder.build();
        let args = command_args(&cmd);

        // Must have --dir for intermediate parent
        let has_parent_dir = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--dir")
            .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested"));
        assert!(
            has_parent_dir,
            "nested /tmp path must have --dir for parent /tmp/deep/nested; args: {args:?}"
        );

        // Must have --dir for the path itself
        let has_path_dir = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--dir")
            .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested/dir"));
        assert!(
            has_path_dir,
            "nested /tmp path must have --dir for /tmp/deep/nested/dir; args: {args:?}"
        );

        // Must have --bind
        let has_bind = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--bind")
            .any(|(i, _)| args.get(i + 1).map(|s| s.as_str()) == Some("/tmp/deep/nested/dir"));
        assert!(
            has_bind,
            "/tmp/deep/nested/dir must have --bind; args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_bare_tmp_is_not_bind_mounted() {
        // writable_paths = ["/tmp"] should NOT produce --bind /tmp /tmp,
        // because --tmpfs /tmp already makes /tmp writable.  Bind-mounting
        // host /tmp would leak host temp files into the sandbox.
        let mut builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        builder.with_writable_path(Path::new("/tmp"));
        let cmd = builder.build();
        let args = command_args(&cmd);

        // --tmpfs /tmp must exist
        assert!(
            args.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/tmp"),
            "--tmpfs /tmp must be present; args: {args:?}"
        );

        // --bind /tmp /tmp must NOT exist
        let has_bind_tmp = args
            .windows(3)
            .any(|w| w[0] == "--bind" && w[1] == "/tmp" && w[2] == "/tmp");
        assert!(
            !has_bind_tmp,
            "bare /tmp must NOT be --bind mounted (would expose host /tmp); args: {args:?}"
        );
    }

    #[test]
    fn test_bwrap_auto_ro_binds_gh_aider_config_when_present() {
        let home = tempfile::tempdir().expect("tempdir");
        let gh_aider = home.path().join(".config/gh-aider");
        std::fs::create_dir_all(&gh_aider).expect("create gh-aider dir");

        let builder = BwrapCommandBuilder::new("/usr/bin/tool", &[]);
        let cmd = builder.build_with_home(Some(home.path()));
        let args = command_args(&cmd);

        assert!(
            args.windows(3).any(|window| {
                window[0] == "--ro-bind"
                    && window[1] == gh_aider.to_string_lossy()
                    && window[2] == gh_aider.to_string_lossy()
            }),
            "~/.config/gh-aider should be explicitly re-bound read-only so sandboxed gh commands can still read the aider auth config; args: {args:?}"
        );
    }
}
