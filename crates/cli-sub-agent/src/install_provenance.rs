//! Shared PATH/install provenance checks for installation and `csa doctor install`.
//!
//! Safety contract: never execute a PATH-resolved binary whose content bytes
//! differ from the trusted build artifact. Hash first; only run `--version`
//! against the artifact (or, when bytes match, treat the artifact version as
//! authoritative and skip redundant shadow execution).

use anyhow::{Context, Result, bail};
use std::env;
use std::ffi::OsStr;
#[cfg(not(unix))]
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::audit::hash::hash_file;

/// Bound for artifact/`csa --version` probes (doctor + install verification).
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cap stdout/stderr growth so a hostile/hung artifact cannot exhaust memory.
const VERSION_PROBE_MAX_BYTES: usize = 64 * 1024;
/// Grace between SIGTERM and SIGKILL while the unreaped leader still anchors the PGID.
const VERSION_PROBE_TERM_GRACE: Duration = Duration::from_millis(100);
const VERSION_PROBE_POLL: Duration = Duration::from_millis(10);

/// Stable marker when a mismatched PATH binary was intentionally not executed.
pub(crate) const NOT_EXECUTED_MISMATCH: &str =
    "(not executed: PATH-resolved bytes differ from build artifact)";

/// Stable marker when full doctor would otherwise run an unverified PATH binary.
pub(crate) const NOT_EXECUTED_UNVERIFIED: &str =
    "(not executed: refuse to run unverified PATH-resolved binary)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallProvenanceStatus {
    Current,
    StaleShadow,
    UnsafeShadow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallProvenanceReport {
    pub(crate) status: InstallProvenanceStatus,
    pub(crate) path_resolved: PathBuf,
    pub(crate) intended_target: PathBuf,
    pub(crate) artifact: PathBuf,
    pub(crate) artifact_hash: String,
    pub(crate) resolved_hash: String,
    pub(crate) artifact_version: String,
    /// Version banner from PATH-resolved binary, or a not-executed marker.
    pub(crate) version_output: String,
}

impl InstallProvenanceReport {
    pub(crate) fn is_current(&self) -> bool {
        self.status == InstallProvenanceStatus::Current
    }

    pub(crate) fn status_str(&self) -> &'static str {
        match self.status {
            InstallProvenanceStatus::Current => "current",
            InstallProvenanceStatus::StaleShadow => "stale_shadow",
            InstallProvenanceStatus::UnsafeShadow => "unsafe_shadow",
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "status": self.status_str(),
            "path_resolved": self.path_resolved.display().to_string(),
            "intended_target": self.intended_target.display().to_string(),
            "artifact": self.artifact.display().to_string(),
            "artifact_sha256": self.artifact_hash,
            "path_resolved_sha256": self.resolved_hash,
            "artifact_version": self.artifact_version,
            "path_resolved_version": self.version_output,
            "current": self.is_current(),
        })
    }

    pub(crate) fn diagnostic(&self) -> String {
        let summary = match self.status {
            InstallProvenanceStatus::Current => "active binary matches the newly built artifact",
            InstallProvenanceStatus::StaleShadow => {
                "PATH resolves a different executable; refusing to report installation success"
            }
            InstallProvenanceStatus::UnsafeShadow => {
                "PATH resolves a different executable that is not writable; refusing to report installation success"
            }
        };
        format!(
            "CSA install provenance: {summary}\n  PATH-resolved executable: {}\n  intended install target: {}\n  build artifact: {}\n  artifact sha256: {}\n  PATH-resolved sha256: {}\n  artifact version/source commit: {}\n  PATH-resolved version/source commit: {}\n{}",
            self.path_resolved.display(),
            self.intended_target.display(),
            self.artifact.display(),
            self.artifact_hash,
            self.resolved_hash,
            self.artifact_version,
            self.version_output,
            if self.is_current() {
                "  status: current"
            } else {
                "  remediation: update PATH so the intended target is first, then rerun `just install`; CSA will not overwrite arbitrary PATH entries."
            },
        )
    }
}

/// Default intended install target for `just install` / doctor surfaces.
///
/// Unix: `/usr/local/bin/csa`. Windows: `LOCALAPPDATA\\csa\\csa.exe` when set,
/// otherwise a non-Unix placeholder (the release `just install` recipe is
/// Unix-oriented).
pub(crate) fn default_intended_target() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("csa")
            .join("csa.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/local/bin/csa")
    }
}

pub(crate) fn inspect_current_path(
    artifact: &Path,
    intended_target: &Path,
) -> Result<InstallProvenanceReport> {
    // Preserve raw OS PATH bytes. Lossy UTF-8 conversion can invent U+FFFD
    // directory components and skip a real higher-priority non-UTF-8 shadow
    // that `execvp` / the shell would still resolve first.
    let path = env::var_os("PATH").context("PATH is not set")?;
    inspect_os(path.as_os_str(), artifact, intended_target)
}

/// Resolve the PATH-first executable named `csa` (for doctor diagnostics).
pub(crate) fn resolve_current_path() -> Result<PathBuf> {
    let path = env::var_os("PATH").context("PATH is not set")?;
    resolve_from_path_os(path.as_os_str())
}

/// UTF-8 PATH helper for tests that already hold a `str`.
#[cfg(test)]
pub(crate) fn inspect(
    path: &str,
    artifact: &Path,
    intended_target: &Path,
) -> Result<InstallProvenanceReport> {
    inspect_os(OsStr::new(path), artifact, intended_target)
}

/// Inspect with an OS-native PATH value (may contain non-UTF-8 directory bytes).
pub(crate) fn inspect_os(
    path: &OsStr,
    artifact: &Path,
    intended_target: &Path,
) -> Result<InstallProvenanceReport> {
    let path_resolved = resolve_from_path_os(path)?;
    let artifact_hash = hash_file(artifact)
        .with_context(|| format!("failed to hash artifact {}", artifact.display()))?;
    let resolved_hash = hash_file(&path_resolved).with_context(|| {
        format!(
            "failed to hash PATH-resolved executable {}",
            path_resolved.display()
        )
    })?;

    // Always version the trusted artifact only.
    let artifact_version = version_output(artifact)?;

    // Hash-first gate: never execute a PATH shadow whose bytes differ.
    // When bytes match, the artifact version is authoritative — skip redundant
    // shadow execution (same content cannot yield a different --version banner).
    let (status, version_output) = if artifact_hash == resolved_hash {
        (InstallProvenanceStatus::Current, artifact_version.clone())
    } else if is_writable(&path_resolved)? {
        (
            InstallProvenanceStatus::StaleShadow,
            NOT_EXECUTED_MISMATCH.to_string(),
        )
    } else {
        (
            InstallProvenanceStatus::UnsafeShadow,
            NOT_EXECUTED_MISMATCH.to_string(),
        )
    };

    Ok(InstallProvenanceReport {
        status,
        path_resolved,
        intended_target: intended_target.to_path_buf(),
        artifact: artifact.to_path_buf(),
        artifact_hash,
        resolved_hash,
        artifact_version,
        version_output,
    })
}

fn resolve_from_path_os(path: &OsStr) -> Result<PathBuf> {
    // Use OS-equivalent lookup: Unix effective execute checks (access/X_OK) and
    // Windows PATHEXT ordering via the `which` crate — not mode-bit heuristics.
    // Pass the raw OsStr so non-UTF-8 PATH components remain searchable, matching
    // shell / execvp resolution order for the `csa` basename.
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    which::which_in("csa", Some(path), cwd).with_context(|| "could not resolve `csa` from PATH")
}

/// Whether the *current process* has effective write access to `path`.
///
/// Uses `access(W_OK)` on Unix so owner/group/other mode bits, identity, and
/// ACLs are honored. Checking mode write bits alone mis-classifies root-owned
/// 0755 shadows as writable for unprivileged callers.
fn is_writable(path: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .with_context(|| format!("path contains interior NUL: {}", path.display()))?;
        // SAFETY: `access` only inspects the path string; no pointer aliasing.
        let rc = unsafe { libc::access(c_path.as_ptr(), libc::W_OK) };
        Ok(rc == 0)
    }
    #[cfg(not(unix))]
    {
        Ok(!fs::metadata(path)?.permissions().readonly())
    }
}

fn version_output(path: &Path) -> Result<String> {
    version_output_with_limits(path, VERSION_PROBE_TIMEOUT, VERSION_PROBE_MAX_BYTES)
}

/// Why the wait loop stopped. Group cleanup always runs afterward while the
/// unreaped leader still anchors the process group (Unix).
#[derive(Debug)]
enum ProbeStopReason {
    LeaderExited,
    TimedOut,
    OutputTooLarge,
    WaitError(std::io::Error),
}

/// Owns a version-probe child and enforces process-group cleanup ownership rules.
///
/// On Unix, negative-PGID signals are legal only while the group leader remains
/// unreaped. Reaping first can free the numeric PID/PGID for reuse by an
/// unrelated process group — never signal `-pgid` after the leader is reaped.
struct VersionProbeSession {
    /// Present until the leader has been reaped (or Drop fails closed).
    child: Option<Child>,
    /// Cached leader status when reaped early (non-Unix `try_wait` path).
    reaped_status: Option<ExitStatus>,
}

impl VersionProbeSession {
    fn new(child: Child) -> Self {
        Self {
            child: Some(child),
            reaped_status: None,
        }
    }

    fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    /// Signal remaining probe processes while the leader is still unreaped.
    ///
    /// - When the leader already exited (Unix WNOWAIT path): SIGKILL the group
    ///   immediately to reap descendants that may still hold stdout/stderr FDs.
    /// - When the leader is still running (timeout / oversized output): SIGTERM,
    ///   grace window (leader still unreaped), then SIGKILL.
    /// - No-op if the leader was already reaped (non-Unix `try_wait` path).
    fn terminate_group_while_owned(&mut self, leader_already_exited: bool) {
        let Some(child) = self.child.as_mut() else {
            // Already reaped — never signal a negative PGID after ownership loss.
            return;
        };
        let pid = child.id();
        if pid <= 1 {
            return;
        }

        #[cfg(unix)]
        {
            let pgid = -(pid as libc::pid_t);
            if !leader_already_exited {
                // SAFETY: leader is still unreaped, so its PID remains the process-group
                // anchor created via `process_group(0)`. Negative PGID targets that group.
                let _ = unsafe { libc::kill(pgid, libc::SIGTERM) };
                std::thread::sleep(VERSION_PROBE_TERM_GRACE);
            }
            // Leader still unreaped — safe for final group KILL (no post-reap -pgid).
            let kill_rc = unsafe { libc::kill(pgid, libc::SIGKILL) };
            if kill_rc != 0 {
                // ESRCH: group already empty. Other errors: fall back to exact child kill.
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    let _ = child.kill();
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = leader_already_exited;
            let _ = child.kill();
        }
    }

    fn reap_leader(&mut self) -> Result<ExitStatus> {
        if let Some(status) = self.reaped_status {
            return Ok(status);
        }
        let mut child = self
            .child
            .take()
            .context("version probe leader already reaped")?;
        let status = child
            .wait()
            .context("failed to reap version probe leader")?;
        self.reaped_status = Some(status);
        Ok(status)
    }

    /// Terminate remaining group members (while owned), then reap the leader.
    fn finish(mut self, stop: &ProbeStopReason) -> Result<ExitStatus> {
        let leader_already_exited = matches!(stop, ProbeStopReason::LeaderExited);
        self.terminate_group_while_owned(leader_already_exited);
        self.reap_leader()
    }
}

impl Drop for VersionProbeSession {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        // Fail-closed cleanup: escalate as if the probe is still live.
        self.terminate_group_while_owned(false);
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

/// Bounded `--version` probe: timeout, dual-stream size cap, process-group lifecycle.
///
/// Trusted paths only (callers enforce). Never mutates PATH entries.
///
/// Lifecycle:
/// 1. Spawn in a new process group (Unix) so descendants share the leader PGID.
/// 2. Poll until leader exit / timeout / stdout|stderr size breach — on Unix,
///    detect exit with `waitid(WNOWAIT)` so the leader stays unreaped.
/// 3. SIGTERM → grace → SIGKILL the process group while the leader is still owned.
/// 4. Reap the leader exactly once; never signal a negative PGID after reaping.
/// 5. Enforce both stream caps on the captured output (tempfile-backed; no reader threads).
fn version_output_with_limits(path: &Path, timeout: Duration, max_bytes: usize) -> Result<String> {
    let mut stdout_file =
        tempfile::tempfile().context("failed to allocate version probe stdout buffer")?;
    let stderr_file =
        tempfile::tempfile().context("failed to allocate version probe stderr buffer")?;

    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            stdout_file
                .try_clone()
                .context("failed to clone version probe stdout")?,
        ))
        .stderr(Stdio::from(
            stderr_file
                .try_clone()
                .context("failed to clone version probe stderr")?,
        ));

    // Isolate process group so cleanup can terminate the probe and grandchildren.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to run {} --version", path.display()))?;
    let mut session = VersionProbeSession::new(child);

    let stop = poll_version_probe(&mut session, &stdout_file, &stderr_file, timeout, max_bytes);

    // Always clean the group while ownership holds, including the success path
    // where the leader already exited but descendants may retain stdout/stderr FDs.
    let status = session.finish(&stop).with_context(|| {
        format!(
            "failed to terminate/reap version probe for {}",
            path.display()
        )
    })?;

    match stop {
        ProbeStopReason::TimedOut => {
            bail!(
                "{} --version timed out after {}ms",
                path.display(),
                timeout.as_millis()
            );
        }
        ProbeStopReason::OutputTooLarge => {
            bail!(
                "{} --version produced more than {} bytes of output",
                path.display(),
                max_bytes
            );
        }
        ProbeStopReason::WaitError(error) => {
            return Err(error)
                .with_context(|| format!("failed waiting for {} --version", path.display()));
        }
        ProbeStopReason::LeaderExited => {}
    }

    if !status.success() {
        bail!("{} --version exited with {}", path.display(), status);
    }

    // Post-completion dual-stream cap: descendants may have written after the
    // leader closed its own FDs but before group cleanup completed.
    if file_len_exceeds(&stdout_file, max_bytes)? || file_len_exceeds(&stderr_file, max_bytes)? {
        bail!(
            "{} --version produced more than {} bytes of output",
            path.display(),
            max_bytes
        );
    }

    let mut stdout_bytes = Vec::new();
    stdout_file
        .seek(SeekFrom::Start(0))
        .context("failed to rewind version probe stdout")?;
    stdout_file
        .read_to_end(&mut stdout_bytes)
        .context("failed to read version probe stdout")?;
    if stdout_bytes.len() > max_bytes {
        bail!(
            "{} --version produced more than {} bytes of output",
            path.display(),
            max_bytes
        );
    }

    String::from_utf8(stdout_bytes)
        .map(|value| value.trim().to_string())
        .context("csa --version returned non-UTF-8 output")
}

fn poll_version_probe(
    session: &mut VersionProbeSession,
    stdout_file: &std::fs::File,
    stderr_file: &std::fs::File,
    timeout: Duration,
    max_bytes: usize,
) -> ProbeStopReason {
    let deadline = Instant::now() + timeout;
    loop {
        match probe_leader_state(session) {
            Ok(LeaderPoll::Exited) => return ProbeStopReason::LeaderExited,
            Ok(LeaderPoll::Running) => {}
            Err(error) => return ProbeStopReason::WaitError(error),
        }

        match (
            file_len_exceeds(stdout_file, max_bytes),
            file_len_exceeds(stderr_file, max_bytes),
        ) {
            (Ok(true), _) | (_, Ok(true)) => return ProbeStopReason::OutputTooLarge,
            (Err(error), _) | (_, Err(error)) => {
                // Treat stat failures as wait-path errors so callers fail closed.
                return ProbeStopReason::WaitError(std::io::Error::other(error.to_string()));
            }
            (Ok(false), Ok(false)) => {}
        }

        if Instant::now() >= deadline {
            return ProbeStopReason::TimedOut;
        }
        std::thread::sleep(
            VERSION_PROBE_POLL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

enum LeaderPoll {
    Running,
    Exited,
}

/// Inspect leader liveness without releasing process-group ownership on Unix.
fn probe_leader_state(session: &mut VersionProbeSession) -> std::io::Result<LeaderPoll> {
    #[cfg(unix)]
    {
        let Some(child) = session.child_mut() else {
            // Already reaped — treat as exited (should not happen on Unix poll path).
            return Ok(LeaderPoll::Exited);
        };
        // WNOWAIT leaves the exited leader waitable so its PID remains the PGID anchor.
        leader_exited_without_reaping(child.id()).map(|exited| {
            if exited {
                LeaderPoll::Exited
            } else {
                LeaderPoll::Running
            }
        })
    }
    #[cfg(not(unix))]
    {
        // No process-group ownership model: try_wait reaps the direct child.
        let wait_result = match session.child.as_mut() {
            None => return Ok(LeaderPoll::Exited),
            Some(child) => child.try_wait()?,
        };
        match wait_result {
            Some(status) => {
                // Consume the handle after try_wait reaped the child.
                session.child = None;
                session.reaped_status = Some(status);
                Ok(LeaderPoll::Exited)
            }
            None => Ok(LeaderPoll::Running),
        }
    }
}

#[cfg(unix)]
fn leader_exited_without_reaping(pid: u32) -> std::io::Result<bool> {
    if pid <= 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid version probe PID {pid}"),
        ));
    }
    loop {
        // SAFETY: zeroed siginfo is the documented no-state-change baseline for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: info points to writable storage; WNOWAIT keeps the child unreaped.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        // SAFETY: waitid initialized info on success; si_pid == 0 means no change.
        let exited_pid = unsafe { info.si_pid() };
        return Ok(exited_pid != 0);
    }
}

fn file_len_exceeds(file: &std::fs::File, max_bytes: usize) -> Result<bool> {
    let len = file
        .metadata()
        .context("failed to stat version probe output")?
        .len();
    Ok(len as usize > max_bytes)
}

#[cfg(all(test, unix))]
#[path = "install_provenance_tests.rs"]
mod install_provenance_tests;
