//! Tool process spawning: plain, sandboxed, and cgroup-wrapped.

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

use csa_resource::filesystem_sandbox::FilesystemCapability;
use csa_resource::isolation_plan::IsolationPlan;
use csa_resource::sandbox::ResourceCapability;

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
    spawn_tool_with_pre_exec(cmd, stdin_data, PreExecPolicy::Setsid, spawn_options, None).await
}

async fn spawn_tool_with_pre_exec(
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
    pre_exec_policy: PreExecPolicy,
    spawn_options: SpawnOptions,
    landlock_paths: Option<Vec<std::path::PathBuf>>,
) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if stdin_data.is_some() || spawn_options.keep_stdin_open {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.kill_on_drop(true);

    // Isolate child in its own process group, optionally apply rlimits,
    // and optionally apply Landlock filesystem restrictions.
    // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
    //         Landlock syscalls (landlock_create_ruleset, landlock_add_rule,
    //         landlock_restrict_self) are also safe in this context.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();

            // Resource isolation (rlimits / OOM score).
            match pre_exec_policy {
                PreExecPolicy::Setsid => {}
                PreExecPolicy::Rlimits {
                    memory_max_mb,
                    pids_max,
                } => {
                    csa_resource::rlimit::apply_rlimits(memory_max_mb, pids_max)
                        .map_err(std::io::Error::other)?;
                }
                PreExecPolicy::OomAdj => {
                    csa_resource::rlimit::apply_oom_score_adj().map_err(std::io::Error::other)?;
                }
            }

            // Filesystem isolation via Landlock (when requested).
            if let Some(ref paths) = landlock_paths {
                csa_resource::apply_landlock_rules(paths).map_err(std::io::Error::other)?;
            }

            Ok(())
        });
    }
    #[cfg(not(unix))]
    {
        let _ = pre_exec_policy;
        let _ = landlock_paths;
    }

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

/// Spawn a tool process with optional dual-axis isolation.
///
/// When `isolation` is `Some`, the child process is wrapped in up to two
/// independent isolation layers derived from the [`IsolationPlan`]:
///
/// ## Resource axis (`plan.resource`)
///
/// - **CgroupV2**: The tool binary is launched inside a systemd transient
///   scope via `systemd-run --user --scope`.  A [`CgroupScopeGuard`] is
///   returned that stops the scope on drop.
///
/// - **Setrlimit**: `RLIMIT_NPROC` is applied in the child via `pre_exec`.
///
/// - **None**: Falls through to OOM score adjustment as a last resort.
///
/// ## Filesystem axis (`plan.filesystem`)
///
/// - **Bwrap**: The command is wrapped with `bwrap(1)` via
///   [`csa_resource::bwrap::from_isolation_plan()`], providing read-only root
///   with selective writable bind mounts.
///
/// - **Landlock**: Reserved for Phase C (currently a no-op placeholder).
///
/// - **None**: No filesystem isolation applied.
///
/// When `isolation` is `None`, this delegates directly to [`spawn_tool`] with
/// no overhead — behavior is identical to the unsandboxed path.
///
/// [`CgroupScopeGuard`]: csa_resource::cgroup::CgroupScopeGuard
pub async fn spawn_tool_sandboxed(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    isolation: Option<&IsolationPlan>,
    tool_name: &str,
    session_id: &str,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    let Some(plan) = isolation else {
        let child = spawn_tool_with_options(cmd, stdin_data, spawn_options).await?;
        return Ok((child, SandboxHandle::None));
    };

    // --- Filesystem axis: wrap the command if needed ---
    //
    // Landlock paths are captured here and applied in pre_exec later,
    // since Landlock operates on the calling thread (not via a wrapper binary).
    let mut landlock_paths: Option<Vec<std::path::PathBuf>> = None;

    let cmd = match plan.filesystem {
        FilesystemCapability::Bwrap => {
            let tool_binary = cmd.as_std().get_program().to_string_lossy().to_string();
            let tool_args: Vec<String> = cmd
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();

            if let Some(bwrap_cmd) =
                csa_resource::bwrap::from_isolation_plan(plan, &tool_binary, &tool_args)
            {
                let mut wrapped = Command::from(bwrap_cmd);
                // Propagate environment from original command.
                for (key, val) in cmd.as_std().get_envs() {
                    if let Some(v) = val {
                        wrapped.env(key, v);
                    }
                }
                if let Some(dir) = cmd.as_std().get_current_dir() {
                    wrapped.current_dir(dir);
                }
                debug!("wrapped tool command with bwrap filesystem sandbox");
                wrapped
            } else {
                warn!("bwrap requested but from_isolation_plan returned None; proceeding without");
                cmd
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
            cmd
        }
        FilesystemCapability::None => cmd,
    };

    let has_bwrap = plan.filesystem == FilesystemCapability::Bwrap;

    let has_landlock = landlock_paths.is_some();

    // --- Resource axis: apply resource isolation ---
    match plan.resource {
        ResourceCapability::CgroupV2 => {
            spawn_with_cgroup(
                cmd,
                stdin_data,
                spawn_options,
                plan,
                tool_name,
                session_id,
                FsSandboxParams {
                    _has_bwrap: has_bwrap,
                    landlock_paths,
                },
            )
            .await
        }
        ResourceCapability::Setrlimit => {
            let child = spawn_tool_with_pre_exec(
                cmd,
                stdin_data,
                PreExecPolicy::Rlimits {
                    memory_max_mb: plan.memory_max_mb.unwrap_or(0),
                    pids_max: plan.pids_max.map(u64::from),
                },
                spawn_options,
                landlock_paths,
            )
            .await?;

            let handle = if has_bwrap {
                SandboxHandle::Bwrap
            } else if has_landlock {
                SandboxHandle::Landlock
            } else {
                SandboxHandle::Rlimit
            };
            Ok((child, handle))
        }
        ResourceCapability::None => {
            debug!("no resource capability in isolation plan; applying OOM score adj as fallback");
            let child = spawn_tool_with_pre_exec(
                cmd,
                stdin_data,
                PreExecPolicy::OomAdj,
                spawn_options,
                landlock_paths,
            )
            .await?;

            let handle = if has_bwrap {
                SandboxHandle::Bwrap
            } else if has_landlock {
                SandboxHandle::Landlock
            } else {
                SandboxHandle::None
            };
            Ok((child, handle))
        }
    }
}

/// Filesystem isolation parameters for cgroup spawn.
struct FsSandboxParams {
    _has_bwrap: bool,
    landlock_paths: Option<Vec<std::path::PathBuf>>,
}

/// Spawn inside a systemd cgroup scope.
async fn spawn_with_cgroup(
    original_cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    plan: &IsolationPlan,
    tool_name: &str,
    session_id: &str,
    fs_sandbox: FsSandboxParams,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    let cgroup_config = csa_resource::cgroup::SandboxConfig {
        memory_max_mb: plan.memory_max_mb.unwrap_or(4096),
        memory_swap_max_mb: plan.memory_swap_max_mb,
        pids_max: plan.pids_max.or(Some(512)),
    };

    let scope_cmd =
        csa_resource::cgroup::create_scope_command(tool_name, session_id, &cgroup_config);

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

    // When Landlock is requested alongside cgroup, apply it via pre_exec
    // on the systemd-run wrapper command (which eventually exec's the tool).
    let child = if fs_sandbox.landlock_paths.is_some() {
        spawn_tool_with_pre_exec(
            tokio_cmd,
            stdin_data,
            PreExecPolicy::Setsid,
            spawn_options,
            fs_sandbox.landlock_paths,
        )
        .await?
    } else {
        spawn_tool_with_options(tokio_cmd, stdin_data, spawn_options).await?
    };
    let guard = csa_resource::cgroup::CgroupScopeGuard::new(tool_name, session_id);

    debug!(
        scope = %guard.scope_name(),
        pid = child.id(),
        "spawned tool inside cgroup scope"
    );

    // Cgroup guard needs to live for cleanup regardless of filesystem isolation.
    Ok((child, SandboxHandle::Cgroup(guard)))
}
