//! Bind-descriptor inheritance for reconstructed sandbox commands.

use std::path::Path;
use std::process::Command;

use super::sandbox_bind_fd_count;
#[cfg(unix)]
use super::sandbox_bind_files;
use crate::isolation_plan::IsolationPlan;

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
