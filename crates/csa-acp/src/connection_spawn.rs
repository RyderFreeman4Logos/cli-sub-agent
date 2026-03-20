//! Spawn and sandbox logic for ACP connections.
//!
//! Extracted from `connection.rs` to keep module sizes manageable.
//! This module handles process spawning (plain, sandboxed, cgroup, rlimit)
//! while `connection.rs` retains session/prompt operations.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
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
}

impl AcpConnection {
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

        // --- Filesystem axis: optionally wrap the command with bwrap ---
        let (effective_command, effective_args, has_bwrap) = match plan.filesystem {
            FilesystemCapability::Bwrap => {
                let tool_args: Vec<String> = request.args.to_vec();
                if let Some(bwrap_cmd) =
                    csa_resource::bwrap::from_isolation_plan(plan, request.command, &tool_args)
                {
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
                debug!("Landlock filesystem isolation requested; not yet implemented (Phase C)");
                (request.command.to_owned(), request.args.to_vec(), false)
            }
            FilesystemCapability::None => {
                (request.command.to_owned(), request.args.to_vec(), false)
            }
        };

        // --- Resource axis: apply resource isolation ---
        match plan.resource {
            ResourceCapability::CgroupV2 => {
                // Build systemd-run wrapper command, then append the
                // (possibly bwrap-wrapped) ACP binary + args.
                let cgroup_config = csa_resource::cgroup::SandboxConfig {
                    memory_max_mb: 4096,
                    memory_swap_max_mb: Some(0),
                    pids_max: Some(512),
                };
                let scope_cmd = csa_resource::cgroup::create_scope_command(
                    sandbox.tool_name,
                    sandbox.session_id,
                    &cgroup_config,
                );
                let mut cmd = Command::from(scope_cmd);
                cmd.arg(&effective_command);
                cmd.args(&effective_args);
                cmd.current_dir(request.working_dir)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());

                // SAFETY: setsid() is async-signal-safe, runs before exec in child.
                #[cfg(unix)]
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }

                for var in Self::STRIPPED_ENV_VARS {
                    cmd.env_remove(var);
                }
                for (key, value) in request.env {
                    cmd.env(key, value);
                }

                let conn = Self::spawn_with_cmd(cmd, request.working_dir, request.options).await?;
                let guard = csa_resource::cgroup::CgroupScopeGuard::new(
                    sandbox.tool_name,
                    sandbox.session_id,
                );
                debug!(
                    scope = %guard.scope_name(),
                    "ACP process spawned inside cgroup scope"
                );
                Ok((conn, AcpSandboxHandle::Cgroup(guard)))
            }
            ResourceCapability::Setrlimit => {
                let mut cmd = Self::build_cmd_base(
                    &effective_command,
                    &effective_args,
                    request.working_dir,
                    request.env,
                );

                // Apply setsid + rlimits in a single pre_exec hook.
                // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
                #[cfg(unix)]
                unsafe {
                    cmd.pre_exec(move || {
                        libc::setsid();
                        csa_resource::rlimit::apply_rlimits(0, None).map_err(std::io::Error::other)
                    });
                }

                let conn =
                    Self::spawn_with_cmd_raw(cmd, request.working_dir, request.options).await?;

                Ok((
                    conn,
                    if has_bwrap {
                        AcpSandboxHandle::Bwrap
                    } else {
                        AcpSandboxHandle::Rlimit
                    },
                ))
            }
            ResourceCapability::None => {
                if has_bwrap {
                    // Filesystem sandbox active but no resource isolation.
                    let mut cmd = Self::build_cmd_base(
                        &effective_command,
                        &effective_args,
                        request.working_dir,
                        request.env,
                    );

                    // SAFETY: setsid() is async-signal-safe, runs before exec.
                    #[cfg(unix)]
                    unsafe {
                        cmd.pre_exec(|| {
                            libc::setsid();
                            csa_resource::rlimit::apply_oom_score_adj()
                                .map_err(std::io::Error::other)
                        });
                    }

                    let conn =
                        Self::spawn_with_cmd_raw(cmd, request.working_dir, request.options).await?;
                    Ok((conn, AcpSandboxHandle::Bwrap))
                } else {
                    debug!("no sandbox capability detected; spawning ACP without isolation");
                    let conn = Self::spawn_with_options(
                        request.command,
                        request.args,
                        request.working_dir,
                        request.env,
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
