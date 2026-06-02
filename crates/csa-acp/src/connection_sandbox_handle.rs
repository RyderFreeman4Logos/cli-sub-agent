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
