//! Spawn and sandbox logic for ACP connections.
//!
//! Extracted from `connection.rs` to keep module sizes manageable.
//! This module handles process spawning (plain, sandboxed, cgroup, rlimit)
//! while `connection.rs` retains session/prompt operations.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    time::{Duration, Instant},
};

use tokio::{io::AsyncReadExt, process::Command, task::LocalSet};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{debug, warn};

use csa_resource::filesystem_sandbox::FilesystemCapability;
use csa_resource::isolation_plan::IsolationPlan;
use csa_resource::sandbox::ResourceCapability;

use crate::{
    client::{AcpClient, SessionEventStore},
    error::{AcpError, AcpResult},
};

use super::AcpConnection;

/// Holds sandbox resources that must live as long as the ACP child process.
///
/// Mirrors [`csa_process::SandboxHandle`] for the ACP transport path.
///
/// # Signal semantics
///
/// - **`Cgroup`**: The ACP process runs inside a systemd transient scope.
///   On drop, the guard calls `systemctl --user stop <scope>`, sending
///   `SIGTERM` to all processes in the scope.
///
/// - **`Rlimit`**: `RLIMIT_NPROC` was applied in the child's `pre_exec`.
///   This is a marker variant indicating rlimit-based PID isolation is active.
///
/// - **`Bwrap`**: Bubblewrap filesystem sandbox is active.
///
/// - **`None`**: No sandbox active.
pub enum AcpSandboxHandle {
    /// cgroup scope guard -- dropped to stop the scope.
    Cgroup(csa_resource::cgroup::CgroupScopeGuard),
    /// Bubblewrap filesystem sandbox is active.
    Bwrap,
    /// Landlock LSM filesystem sandbox is active.
    Landlock,
    /// `RLIMIT_NPROC` was applied in child via `pre_exec`.
    Rlimit,
    /// No sandbox active.
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpConnectionOptions {
    /// Timeout for ACP initialization/session setup operations.
    pub init_timeout: Duration,
    /// Grace period between SIGTERM and SIGKILL for forced termination.
    pub termination_grace_period: Duration,
}

impl Default for AcpConnectionOptions {
    fn default() -> Self {
        Self {
            init_timeout: Duration::from_secs(120),
            termination_grace_period: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AcpSpawnRequest<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub working_dir: &'a Path,
    pub env: &'a HashMap<String, String>,
    pub options: AcpConnectionOptions,
}

#[derive(Debug, Clone)]
pub struct AcpSandboxRequest<'a> {
    pub isolation_plan: &'a IsolationPlan,
    pub tool_name: &'a str,
    pub session_id: &'a str,
    pub env_overrides: Option<&'a HashMap<String, String>>,
}

#[derive(Debug)]
struct PreparedSandboxCommand {
    effective_command: String,
    effective_args: Vec<String>,
    effective_env: HashMap<String, String>,
    landlock_paths: Option<Vec<PathBuf>>,
    has_bwrap: bool,
}

impl AcpSandboxHandle {
    /// Check if the OOM killer was triggered in the sandbox scope.
    ///
    /// Only meaningful for the [`Cgroup`](Self::Cgroup) variant; returns
    /// `false` for all others.  Must be called **before** the handle is
    /// dropped, as the cgroup scope is stopped on drop.
    pub fn check_oom_killed(&self) -> bool {
        self.check_oom_killed_with_signal(None)
    }

    /// Check whether the cgroup scope was OOM-killed, falling back to a
    /// SIGKILL-based inference when systemd has already GC'd the failed scope.
    pub fn check_oom_killed_with_signal(&self, exit_signal: Option<i32>) -> bool {
        match self {
            Self::Cgroup(guard) => guard.check_oom_killed_with_signal(exit_signal),
            _ => false,
        }
    }

    /// Produce an actionable OOM diagnosis string, if applicable.
    ///
    /// Returns `Some(hint)` when the cgroup OOM killer was triggered,
    /// including peak/limit memory info and configuration advice.
    pub fn oom_diagnosis(&self) -> Option<String> {
        self.oom_diagnosis_with_signal(None)
    }

    /// Produce an actionable OOM diagnosis string, using the child exit signal
    /// as a fallback hint when the failed scope has already been collected.
    pub fn oom_diagnosis_with_signal(&self, exit_signal: Option<i32>) -> Option<String> {
        match self {
            Self::Cgroup(guard) => guard.oom_diagnosis_with_signal(exit_signal),
            _ => None,
        }
    }

    /// Query peak memory usage (in MB) from the cgroup scope.
    ///
    /// Must be called **before** the handle is dropped, as the cgroup scope
    /// is stopped on drop and the metric becomes unavailable.
    pub fn memory_peak_mb(&self) -> Option<u64> {
        match self {
            Self::Cgroup(guard) => guard.memory_peak_mb(),
            _ => None,
        }
    }

    /// Return the scope name if this is a cgroup sandbox.
    pub fn scope_name(&self) -> Option<&str> {
        match self {
            Self::Cgroup(guard) => Some(guard.scope_name()),
            _ => None,
        }
    }
}

impl AcpConnection {
    fn merge_sandbox_env(
        base_env: &HashMap<String, String>,
        sandbox_env_overrides: Option<&HashMap<String, String>>,
    ) -> HashMap<String, String> {
        let mut merged_env = base_env.clone();
        if let Some(overrides) = sandbox_env_overrides {
            merged_env.extend(
                overrides
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        merged_env
    }

    fn merged_bwrap_isolation_plan(
        plan: &IsolationPlan,
        sandbox_env_overrides: Option<&HashMap<String, String>>,
    ) -> IsolationPlan {
        let mut merged_plan = plan.clone();
        if let Some(overrides) = sandbox_env_overrides {
            merged_plan.env_overrides.extend(
                overrides
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        merged_plan
    }

    fn prepare_sandbox_command(
        request: AcpSpawnRequest<'_>,
        sandbox: &AcpSandboxRequest<'_>,
    ) -> PreparedSandboxCommand {
        let plan = sandbox.isolation_plan;
        let effective_env = Self::merge_sandbox_env(request.env, sandbox.env_overrides);

        // --- Filesystem axis: optionally wrap the command with bwrap ---
        // Landlock paths are captured here and applied in pre_exec later,
        // since Landlock operates on the calling thread (not via a wrapper binary).
        let mut landlock_paths: Option<Vec<PathBuf>> = None;

        let (effective_command, effective_args, has_bwrap) = match plan.filesystem {
            FilesystemCapability::Bwrap => {
                let tool_args: Vec<String> = request.args.to_vec();
                let bwrap_plan = Self::merged_bwrap_isolation_plan(plan, sandbox.env_overrides);
                if let Some(bwrap_cmd) = csa_resource::bwrap::from_isolation_plan(
                    &bwrap_plan,
                    request.command,
                    &tool_args,
                ) {
                    let program = bwrap_cmd.get_program().to_string_lossy().to_string();
                    let args: Vec<String> = bwrap_cmd
                        .get_args()
                        .map(|a| a.to_string_lossy().to_string())
                        .collect();
                    debug!("wrapped ACP command with bwrap filesystem sandbox");
                    (program, args, true)
                } else {
                    warn!(
                        "bwrap requested but from_isolation_plan returned None; proceeding without"
                    );
                    (request.command.to_owned(), request.args.to_vec(), false)
                }
            }
            FilesystemCapability::Landlock => {
                debug!("Landlock filesystem isolation will be applied in pre_exec");
                // Filter out project_root when readonly_project_root is set,
                // mirroring the bwrap --ro-bind behavior.
                let paths = if plan.readonly_project_root {
                    plan.writable_paths
                        .iter()
                        .filter(|p| plan.project_root.as_ref().is_none_or(|root| *p != root))
                        .cloned()
                        .collect()
                } else {
                    plan.writable_paths.clone()
                };
                landlock_paths = Some(paths);
                (request.command.to_owned(), request.args.to_vec(), false)
            }
            FilesystemCapability::None => {
                (request.command.to_owned(), request.args.to_vec(), false)
            }
        };

        PreparedSandboxCommand {
            effective_command,
            effective_args,
            effective_env,
            landlock_paths,
            has_bwrap,
        }
    }

    /// Spawn an ACP process without resource sandboxing.
    pub async fn spawn(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> AcpResult<Self> {
        Self::spawn_with_options(
            command,
            args,
            working_dir,
            env,
            AcpConnectionOptions::default(),
        )
        .await
    }

    /// Spawn an ACP process with explicit connection options.
    pub async fn spawn_with_options(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
        let cmd = Self::build_cmd(command, args, working_dir, env);
        Self::spawn_with_cmd(cmd, working_dir, options).await
    }

    /// Spawn an ACP process with optional dual-axis isolation.
    ///
    /// When `sandbox` is `Some`, the process is wrapped in up to two
    /// independent isolation layers derived from the [`IsolationPlan`]:
    ///
    /// ## Resource axis (`plan.resource`)
    ///
    /// - **CgroupV2**: Launched inside a systemd transient scope.
    /// - **Setrlimit**: `RLIMIT_NPROC` applied via `pre_exec`.
    /// - **None**: OOM score adjustment as last resort.
    ///
    /// ## Filesystem axis (`plan.filesystem`)
    ///
    /// - **Bwrap**: The ACP binary is wrapped with `bwrap(1)` via
    ///   [`csa_resource::bwrap::from_isolation_plan()`].
    /// - **Landlock**: Reserved for Phase C (no-op).
    /// - **None**: No filesystem isolation.
    ///
    /// When `sandbox` is `None`, behavior is identical to [`Self::spawn`].
    ///
    /// Returns the connection and an [`AcpSandboxHandle`] that must be kept
    /// alive for the duration of the child process.
    pub async fn spawn_sandboxed(
        request: AcpSpawnRequest<'_>,
        sandbox: Option<AcpSandboxRequest<'_>>,
    ) -> AcpResult<(Self, AcpSandboxHandle)> {
        let Some(sandbox) = sandbox else {
            let conn = Self::spawn_with_options(
                request.command,
                request.args,
                request.working_dir,
                request.env,
                request.options,
            )
            .await?;
            return Ok((conn, AcpSandboxHandle::None));
        };

        let plan = sandbox.isolation_plan;
        let PreparedSandboxCommand {
            effective_command,
            effective_args,
            effective_env,
            mut landlock_paths,
            has_bwrap,
        } = Self::prepare_sandbox_command(request, &sandbox);

        // --- Resource axis: apply resource isolation ---
        match plan.resource {
            ResourceCapability::CgroupV2 => {
                if landlock_paths.is_some() {
                    return Err(AcpError::ConfigError(
                        "invalid isolation plan: Landlock cannot be combined with CgroupV2; degrade to Setrlimit before spawning ACP"
                            .to_string(),
                    ));
                }

                // Build systemd-run wrapper command, then append the
                // (possibly bwrap-wrapped) ACP binary + args.
                let cgroup_config = csa_resource::cgroup::SandboxConfig {
                    memory_max_mb: plan.memory_max_mb.unwrap_or(4096),
                    memory_swap_max_mb: plan.memory_swap_max_mb,
                    pids_max: plan.pids_max.or(Some(512)),
                };
                let scope_cmd = csa_resource::cgroup::create_scope_command_with_env(
                    sandbox.tool_name,
                    sandbox.session_id,
                    &cgroup_config,
                    &effective_env,
                );
                let mut cmd = Command::from(scope_cmd);
                cmd.arg(&effective_command);
                cmd.args(&effective_args);
                cmd.current_dir(request.working_dir)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.kill_on_drop(true);

                // SAFETY: setsid() is async-signal-safe and runs before exec in child.
                #[cfg(unix)]
                {
                    unsafe {
                        cmd.pre_exec(move || {
                            libc::setsid();
                            Ok(())
                        });
                    }
                }

                for var in Self::STRIPPED_ENV_VARS {
                    cmd.env_remove(var);
                }
                for (key, value) in &effective_env {
                    cmd.env(key, value);
                }

                let has_landlock = matches!(plan.filesystem, FilesystemCapability::Landlock);
                let conn = Self::spawn_with_cmd(cmd, request.working_dir, request.options).await?;
                let guard = csa_resource::cgroup::CgroupScopeGuard::new(
                    sandbox.tool_name,
                    sandbox.session_id,
                    &cgroup_config,
                );
                debug!(
                    scope = %guard.scope_name(),
                    has_landlock,
                    "ACP process spawned inside cgroup scope"
                );
                Ok((conn, AcpSandboxHandle::Cgroup(guard)))
            }
            ResourceCapability::Setrlimit => {
                let mut cmd = Self::build_cmd_base(
                    &effective_command,
                    &effective_args,
                    request.working_dir,
                    &effective_env,
                );

                // Apply setsid + rlimits + optional Landlock in a single pre_exec hook.
                // SAFETY: setsid(), setrlimit, and Landlock syscalls are async-signal-safe.
                #[cfg(unix)]
                {
                    let rlimit_memory = plan.memory_max_mb.unwrap_or(0);
                    let rlimit_pids = plan.pids_max.map(u64::from);
                    let ll_paths = landlock_paths.take();
                    unsafe {
                        cmd.pre_exec(move || {
                            libc::setsid();
                            csa_resource::rlimit::apply_rlimits(rlimit_memory, rlimit_pids)
                                .map_err(std::io::Error::other)?;
                            if let Some(ref paths) = ll_paths {
                                csa_resource::apply_landlock_rules(paths)
                                    .map_err(std::io::Error::other)?;
                            }
                            Ok(())
                        });
                    }
                }

                let has_landlock = matches!(plan.filesystem, FilesystemCapability::Landlock);
                let conn =
                    Self::spawn_with_cmd_raw(cmd, request.working_dir, request.options).await?;

                Ok((
                    conn,
                    if has_bwrap {
                        AcpSandboxHandle::Bwrap
                    } else if has_landlock {
                        AcpSandboxHandle::Landlock
                    } else {
                        AcpSandboxHandle::Rlimit
                    },
                ))
            }
            ResourceCapability::None => {
                let has_landlock = landlock_paths.is_some();
                if has_bwrap || has_landlock {
                    // Filesystem sandbox active but no resource isolation.
                    let mut cmd = Self::build_cmd_base(
                        &effective_command,
                        &effective_args,
                        request.working_dir,
                        &effective_env,
                    );

                    // SAFETY: setsid(), OOM adj, and Landlock syscalls are
                    //         async-signal-safe and run before exec.
                    #[cfg(unix)]
                    {
                        let ll_paths = landlock_paths.take();
                        unsafe {
                            cmd.pre_exec(move || {
                                libc::setsid();
                                csa_resource::rlimit::apply_oom_score_adj()
                                    .map_err(std::io::Error::other)?;
                                if let Some(ref paths) = ll_paths {
                                    csa_resource::apply_landlock_rules(paths)
                                        .map_err(std::io::Error::other)?;
                                }
                                Ok(())
                            });
                        }
                    }

                    let conn =
                        Self::spawn_with_cmd_raw(cmd, request.working_dir, request.options).await?;
                    let handle = if has_bwrap {
                        AcpSandboxHandle::Bwrap
                    } else {
                        AcpSandboxHandle::Landlock
                    };
                    Ok((conn, handle))
                } else {
                    debug!("no sandbox capability detected; spawning ACP without isolation");
                    let conn = Self::spawn_with_options(
                        request.command,
                        request.args,
                        request.working_dir,
                        &effective_env,
                        request.options,
                    )
                    .await?;
                    Ok((conn, AcpSandboxHandle::None))
                }
            }
        }
    }

    /// Build a standard ACP command with piped stdio and `setsid` pre-exec.
    fn build_cmd(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Self::build_cmd_base(command, args, working_dir, env);

        // Isolate ACP child in its own process group so timeout kill can
        // terminate the full subtree.
        // SAFETY: setsid() runs in pre-exec before Rust runtime exists in child.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        cmd
    }

    /// Build a standard ACP command with piped stdio and environment.
    ///
    /// Strips inherited environment variables that cause the spawned ACP
    /// adapter (e.g. `claude-code-acp`) to fail.  The parent Claude Code
    /// process sets `CLAUDECODE=1` for recursion detection, which makes
    /// any child Claude Code instance refuse to start.
    fn build_cmd_base(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Safety net: ensure child process is cleaned up if AcpConnection is dropped
        // without explicit kill() (e.g., during panic). Not the primary shutdown mechanism —
        // explicit kill() in transport.rs handles normal cleanup.
        cmd.kill_on_drop(true);

        // Strip parent-process env vars that interfere with the ACP child.
        // CLAUDECODE=1 triggers recursion detection in claude-code, causing
        // immediate exit with "unset the CLAUDECODE environment variable".
        // CLAUDE_CODE_ENTRYPOINT is parent-specific context, not relevant.
        for var in Self::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }

        for (key, value) in env {
            cmd.env(key, value);
        }

        cmd
    }

    /// Shared connection setup from a pre-built command.
    pub(crate) async fn spawn_with_cmd(
        cmd: Command,
        working_dir: &Path,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
        Self::spawn_with_cmd_raw(cmd, working_dir, options).await
    }

    /// Core spawn logic: takes a fully configured command, spawns it, and
    /// sets up the ACP protocol connection over stdin/stdout.
    pub(crate) async fn spawn_with_cmd_raw(
        mut cmd: Command,
        working_dir: &Path,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
        let mut child = cmd.spawn().map_err(AcpError::SpawnFailed)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stdin pipe".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stdout pipe".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stderr pipe".to_string())
        })?;

        let local_set = LocalSet::new();
        let events = Rc::new(RefCell::new(SessionEventStore::default()));
        let last_activity = Rc::new(RefCell::new(Instant::now()));
        let client = AcpClient::new(events.clone(), last_activity.clone());
        let stderr_buf = Rc::new(RefCell::new(String::new()));

        let connection = local_set
            .run_until(async {
                let outgoing = stdin.compat_write();
                let incoming = stdout.compat();
                let (conn, io_task) = agent_client_protocol::ClientSideConnection::new(
                    client,
                    outgoing,
                    incoming,
                    |fut| {
                        tokio::task::spawn_local(fut);
                    },
                );

                tokio::task::spawn_local(async move {
                    if let Err(err) = io_task.await {
                        warn!(error = %err, "ACP I/O loop terminated");
                    }
                });

                let stderr_buf_clone = stderr_buf.clone();
                let activity_clone = last_activity.clone();
                tokio::task::spawn_local(async move {
                    let mut reader = stderr;
                    let mut buf = vec![0_u8; 4096];
                    loop {
                        match reader.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                *activity_clone.borrow_mut() = Instant::now();
                                let text = String::from_utf8_lossy(&buf[..n]);
                                stderr_buf_clone.borrow_mut().push_str(&text);
                            }
                            Err(err) => {
                                warn!(error = %err, "failed to read ACP stderr stream");
                                break;
                            }
                        }
                    }
                });

                conn
            })
            .await;

        Ok(Self::new_from_parts(
            local_set,
            connection,
            child,
            events,
            last_activity,
            stderr_buf,
            working_dir.to_path_buf(),
            options,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_setenv(args: &[String], key: &str, value: &str) -> bool {
        args.windows(3)
            .any(|window| window[0] == "--setenv" && window[1] == key && window[2] == value)
    }

    #[test]
    fn prepare_sandbox_command_merges_runtime_env_overrides_into_bwrap_invocation() {
        let request_env = HashMap::from([
            ("HOME".to_string(), "/home/original".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        let sandbox_env_overrides = HashMap::from([
            (
                "HOME".to_string(),
                "/tmp/cli-sub-agent-gemini/01TEST".to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                "/tmp/cli-sub-agent-gemini/01TEST/.local/state".to_string(),
            ),
            (
                "MISE_CACHE_DIR".to_string(),
                "/tmp/cli-sub-agent-gemini/01TEST/.cache/mise".to_string(),
            ),
        ]);
        let isolation_plan = IsolationPlan {
            resource: ResourceCapability::None,
            filesystem: FilesystemCapability::Bwrap,
            writable_paths: vec![
                PathBuf::from("/project"),
                PathBuf::from("/tmp/cli-sub-agent-gemini/01TEST"),
            ],
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: Some(PathBuf::from("/project")),
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        };
        let args = vec!["--acp".to_string()];
        let request = AcpSpawnRequest {
            command: "/usr/bin/gemini",
            args: &args,
            working_dir: Path::new("/project"),
            env: &request_env,
            options: AcpConnectionOptions::default(),
        };
        let sandbox = AcpSandboxRequest {
            isolation_plan: &isolation_plan,
            tool_name: "gemini-cli",
            session_id: "01TEST",
            env_overrides: Some(&sandbox_env_overrides),
        };

        let prepared = AcpConnection::prepare_sandbox_command(request, &sandbox);

        assert_eq!(prepared.effective_command, "bwrap");
        assert_eq!(
            prepared.effective_env.get("HOME"),
            Some(&"/tmp/cli-sub-agent-gemini/01TEST".to_string()),
            "scope env should see the Gemini runtime HOME override"
        );
        assert_eq!(
            prepared.effective_env.get("XDG_STATE_HOME"),
            Some(&"/tmp/cli-sub-agent-gemini/01TEST/.local/state".to_string())
        );
        assert!(
            has_setenv(
                &prepared.effective_args,
                "HOME",
                "/tmp/cli-sub-agent-gemini/01TEST",
            ),
            "bwrap args must include runtime HOME override: {:?}",
            prepared.effective_args
        );
        assert!(
            has_setenv(
                &prepared.effective_args,
                "XDG_STATE_HOME",
                "/tmp/cli-sub-agent-gemini/01TEST/.local/state",
            ),
            "bwrap args must include XDG_STATE_HOME override: {:?}",
            prepared.effective_args
        );
        assert!(
            has_setenv(
                &prepared.effective_args,
                "MISE_CACHE_DIR",
                "/tmp/cli-sub-agent-gemini/01TEST/.cache/mise",
            ),
            "bwrap args must include mise cache override: {:?}",
            prepared.effective_args
        );
    }
}
