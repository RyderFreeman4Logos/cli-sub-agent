//! Tool process spawning: plain, sandboxed, and cgroup-wrapped.

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

use csa_resource::cgroup::SandboxConfig;
use csa_resource::sandbox::{ResourceCapability, detect_resource_capability};

use super::{PreExecPolicy, SandboxHandle, SpawnOptions};

/// Spawn a tool process without waiting for it to complete.
///
/// - Spawns the command
/// - Captures stdout (piped)
/// - Captures stderr (piped, tee'd to parent stderr in `wait_and_capture`)
/// - Sets stdin mode:
///   - `Stdio::piped()` when `stdin_data` is provided
///   - `Stdio::null()` otherwise
/// - Isolates child in its own process group (via setsid)
/// - Enables kill_on_drop as safety net
/// - Returns the child process handle for PID access and later waiting
///
/// Use this when you need the PID before waiting (e.g., for resource monitoring).
/// Call `wait_and_capture()` after starting monitoring to complete execution.
pub async fn spawn_tool(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
) -> Result<tokio::process::Child> {
    spawn_tool_with_options(cmd, stdin_data, SpawnOptions::default()).await
}

/// Spawn a tool process with explicit spawn options.
pub async fn spawn_tool_with_options(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
) -> Result<tokio::process::Child> {
    spawn_tool_with_pre_exec(cmd, stdin_data, PreExecPolicy::Setsid, spawn_options).await
}

async fn spawn_tool_with_pre_exec(
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
    pre_exec_policy: PreExecPolicy,
    spawn_options: SpawnOptions,
) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if stdin_data.is_some() || spawn_options.keep_stdin_open {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.kill_on_drop(true);

    // Isolate child in its own process group and optionally apply rlimits.
    // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            match pre_exec_policy {
                PreExecPolicy::Setsid => Ok(()),
                PreExecPolicy::Rlimits {
                    memory_max_mb,
                    pids_max,
                } => csa_resource::rlimit::apply_rlimits(memory_max_mb, pids_max)
                    .map_err(std::io::Error::other),
                PreExecPolicy::OomAdj => {
                    csa_resource::rlimit::apply_oom_score_adj().map_err(std::io::Error::other)
                }
            }
        });
    }
    #[cfg(not(unix))]
    let _ = pre_exec_policy;

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            let stdin_write_timeout = spawn_options.stdin_write_timeout;
            tokio::spawn(async move {
                match tokio::time::timeout(stdin_write_timeout, async {
                    stdin.write_all(&data).await?;
                    stdin.shutdown().await?;
                    Ok::<_, std::io::Error>(())
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("stdin write error: {}", e),
                    Err(_) => warn!(
                        timeout_secs = stdin_write_timeout.as_secs(),
                        "stdin write timed out"
                    ),
                }
            });
        } else {
            warn!("stdin was requested but no piped stdin handle was available");
        }
    }

    Ok(child)
}

/// Spawn a tool process with optional resource sandbox.
///
/// When `sandbox` is `Some`, the child process is wrapped in resource
/// isolation based on the host's detected capability:
///
/// - **CgroupV2**: The tool binary is launched inside a systemd transient
///   scope via `systemd-run --user --scope`.  A [`CgroupScopeGuard`] is
///   returned that stops the scope on drop.
///
/// - **Setrlimit**: `RLIMIT_NPROC` is applied in the child via `pre_exec`.
///
/// - **None capability**: Falls through to normal `spawn_tool` behavior.
///
/// When `sandbox` is `None`, this delegates directly to [`spawn_tool`] with
/// no overhead — behavior is identical to the unsandboxed path.
///
/// [`CgroupScopeGuard`]: csa_resource::cgroup::CgroupScopeGuard
pub async fn spawn_tool_sandboxed(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    sandbox: Option<&SandboxConfig>,
    tool_name: &str,
    session_id: &str,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    let Some(config) = sandbox else {
        let child = spawn_tool_with_options(cmd, stdin_data, spawn_options).await?;
        return Ok((child, SandboxHandle::None));
    };

    match detect_resource_capability() {
        ResourceCapability::CgroupV2 => {
            spawn_with_cgroup(
                cmd,
                stdin_data,
                spawn_options,
                config,
                tool_name,
                session_id,
            )
            .await
        }
        ResourceCapability::Setrlimit => {
            let memory_max_mb = config.memory_max_mb;
            let pids_max = config.pids_max.map(u64::from);

            let child = spawn_tool_with_pre_exec(
                cmd,
                stdin_data,
                PreExecPolicy::Rlimits {
                    memory_max_mb,
                    pids_max,
                },
                spawn_options,
            )
            .await?;

            Ok((child, SandboxHandle::Rlimit))
        }
        ResourceCapability::None => {
            debug!("no sandbox capability detected; applying OOM score adj as fallback");
            let child =
                spawn_tool_with_pre_exec(cmd, stdin_data, PreExecPolicy::OomAdj, spawn_options)
                    .await?;
            Ok((child, SandboxHandle::None))
        }
    }
}

/// Spawn inside a systemd cgroup scope.
async fn spawn_with_cgroup(
    original_cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    config: &SandboxConfig,
    tool_name: &str,
    session_id: &str,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    let scope_cmd = csa_resource::cgroup::create_scope_command(tool_name, session_id, config);

    let mut tokio_cmd = Command::from(scope_cmd);
    tokio_cmd.arg(original_cmd.as_std().get_program());
    tokio_cmd.args(original_cmd.as_std().get_args());

    let envs: Vec<_> = original_cmd
        .as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|val| (k.to_owned(), val.to_owned())))
        .collect();
    for (key, val) in &envs {
        tokio_cmd.env(key, val);
    }

    if let Some(dir) = original_cmd.as_std().get_current_dir() {
        tokio_cmd.current_dir(dir);
    }

    let child = spawn_tool_with_options(tokio_cmd, stdin_data, spawn_options).await?;
    let guard = csa_resource::cgroup::CgroupScopeGuard::new(tool_name, session_id);

    debug!(
        scope = %guard.scope_name(),
        pid = child.id(),
        "spawned tool inside cgroup scope"
    );

    Ok((child, SandboxHandle::Cgroup(guard)))
}
