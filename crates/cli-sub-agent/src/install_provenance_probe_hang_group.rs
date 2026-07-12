//! Hang-group fixture helpers for version-probe lifecycle tests.
//!
//! Keeps ownership-unknown fixtures RAII-safe: armed cleanup kills the process
//! group on unwind, and happy-path asserts use heartbeat + starttime identity
//! instead of treating zombies or PID reuse as live writers.
//!
//! Signal rule (AGENTS Rust 018 Rule 5): never issue `kill(-pgid)` / exact-PID
//! kill from a bare numeric PID. Linux requires a matching `/proc` starttime;
//! non-Linux uses unreaped parenthood (`waitid` + `WNOWAIT`) as the ownership
//! token. When identity cannot be verified, refuse signals and only best-effort
//! reap if we are still the parent.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// True if `/proc/<pid>` exists in a non-zombie state (running/sleeping/etc.).
///
/// Linux-only helper used as a secondary check alongside the portable heartbeat
/// assertion. Prefer heartbeat growth for cross-Unix coverage: zombies still
/// "exist" for kill(0) but are not live writers.
#[cfg(target_os = "linux")]
pub(super) fn live_non_zombie_process(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        // Missing /proc entry means not live — on Linux this is authoritative.
        return false;
    };
    // Format: pid (comm) state ppid ...
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return false;
    };
    let Some(state) = rest
        .split_whitespace()
        .next()
        .and_then(|s| s.chars().next())
    else {
        return false;
    };
    state != 'Z'
}

/// Portable: process exists (including zombies). Used to assert we did **not**
/// SIGKILL a still-owned or already-reaped identity when ownership is unknown.
pub(super) fn process_exists(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    // SAFETY: kill(pid, 0) is a pure existence/permission probe.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0
}

/// Bound for test-only leader reap after group SIGKILL. Never use unbounded
/// `waitpid(..., 0)` alone — that can hang the suite if the leader becomes
/// unreapable under abnormal kernel conditions.
pub(super) const FORCE_REAP_BOUND: Duration = Duration::from_secs(2);

/// Linux: `/proc/<pid>/stat` starttime (field 22) for PID-identity checks.
///
/// After ownership-unknown paths abandon the `Child` handle, the leader may be
/// reparented and later appear as a zombie (or a reused PID). Comparing starttime
/// distinguishes the original fixture from a reuse and treats zombies as cleaned.
#[cfg(target_os = "linux")]
pub(super) fn process_starttime(pid: u32) -> Option<u64> {
    if pid <= 1 {
        return None;
    }
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return None;
    };
    // Format: pid (comm) state ppid ... starttime (field 22).
    let rest = stat.rsplit_once(')').map(|(_, r)| r)?;
    let field = rest.split_whitespace().nth(19)?;
    field.parse().ok()
}

#[cfg(not(target_os = "linux"))]
pub(super) fn process_starttime(_pid: u32) -> Option<u64> {
    None
}

/// True when `pid` is still the original live (non-zombie) identity.
///
/// - Missing `/proc` entry → cleaned.
/// - Zombie (`Z`) → cleaned (not a live writer; init may reap slowly).
/// - Different starttime → PID reuse; original fixture is gone.
#[cfg(target_os = "linux")]
pub(super) fn same_live_identity(pid: u32, starttime: Option<u64>) -> bool {
    let Some(expected) = starttime.filter(|t| *t != 0) else {
        return false;
    };
    if !live_non_zombie_process(pid as i32) {
        return false;
    }
    process_starttime(pid) == Some(expected)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn same_live_identity(_pid: u32, _starttime: Option<u64>) -> bool {
    // Non-Linux identity is proven via heartbeat growth for descendants.
    false
}

/// True when the numeric PID still belongs to the original fixture identity
/// and is therefore safe to target with `kill(-pgid)` / exact-PID kill.
///
/// - Linux: saved starttime must be non-zero and match current `/proc` starttime
///   (zombie with the same starttime still anchors the PGID until reaped).
/// - Non-Linux: unreaped parenthood via `waitid(..., WNOWAIT)` is the ownership
///   token when starttime is unavailable.
fn may_signal_original_identity(pid: u32, expected_starttime: Option<u64>) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Some(expected) = expected_starttime.filter(|t| *t != 0) else {
            // Rule 5: refuse signals when starttime was never captured.
            return false;
        };
        process_starttime(pid) == Some(expected)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = expected_starttime;
        still_our_unreaped_child(pid)
    }
}

/// Parenthood without reaping: `waitid` + `WNOWAIT` keeps the leader as the
/// PGID anchor so a subsequent group SIGKILL can still reach descendants.
///
/// Used only when Linux starttime is unavailable (non-Linux ownership token).
#[cfg(not(target_os = "linux"))]
fn still_our_unreaped_child(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    loop {
        // SAFETY: zeroed siginfo is the documented no-state-change baseline.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: WNOWAIT leaves the child unreaped; WNOHANG never blocks.
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
            // ECHILD / other: not our waitable child — refuse PID/PGID signals.
            return false;
        }
        // Success: still our child (running si_pid==0, or exited-but-unreaped).
        return true;
    }
}

/// Bounded, EINTR-safe `waitpid(..., WNOHANG)` when we may still be the parent.
fn best_effort_waitpid(pid: u32) {
    if pid <= 1 {
        return;
    }
    let leader = pid as libc::pid_t;
    let deadline = Instant::now() + FORCE_REAP_BOUND;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid on our unreaped child if still parent; WNOHANG.
        let rc = unsafe { libc::waitpid(leader, &mut status, libc::WNOHANG) };
        if rc == leader {
            return;
        }
        if rc == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error();
            if errno == Some(libc::EINTR) {
                continue;
            }
            // ECHILD / other: already reaped, reparented, or not our child.
            return;
        }
        // rc == 0: still running (or unreaped zombie we still own)
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Test-only cleanup for hang-group fixtures.
///
/// 1. Verify original identity (starttime / unreaped parenthood) **before** any
///    signal — refuse `kill(-pgid)` and exact-PID kill when verification fails.
/// 2. SIGKILL the whole process group (`-leader`) while the numeric PGID still
///    matches the fixture leader (descendants such as `sleep` must not orphan).
/// 3. Exact-PID SIGKILL as a fallback if group delivery is denied.
/// 4. Bounded, EINTR-safe `waitpid(..., WNOHANG)` when we are still the parent —
///    after `abandon_without_signaling` this may return `ECHILD` (reparented);
///    that is fine when signals were identity-verified first.
pub(super) fn force_kill_and_reap(pid: u32, expected_starttime: Option<u64>) {
    if pid <= 1 {
        return;
    }
    if !may_signal_original_identity(pid, expected_starttime) {
        // Identity unverified or already gone — never spray -pgid at a reused PID.
        // Best-effort reap only if we are still the parent of an unreaped child.
        best_effort_waitpid(pid);
        return;
    }
    let leader = pid as libc::pid_t;
    // SAFETY: identity just verified (Linux starttime match, or unreaped parenthood).
    // Prefer group kill first so descendants die before the leader PID can be reused.
    unsafe {
        let _ = libc::kill(-leader, libc::SIGKILL);
        let _ = libc::kill(leader, libc::SIGKILL);
    }
    best_effort_waitpid(pid);
}

/// After `force_kill_and_reap`, prove the original fixture is no longer a live writer.
///
/// Do **not** require `kill(pid, 0)` to fail: zombies still "exist", and pure
/// numeric PIDs can be reused. Leader identity uses `/proc` starttime (Linux);
/// descendant cleanup is proven by a stopped-growing heartbeat (portable).
fn assert_group_cleaned(
    leader: u32,
    leader_starttime: Option<u64>,
    descendant: u32,
    descendant_starttime: Option<u64>,
    heartbeat: &Path,
) {
    // Portable proof: heartbeat must stop growing across several write intervals.
    let deadline = Instant::now() + FORCE_REAP_BOUND;
    let mut stopped = false;
    while Instant::now() < deadline {
        let size1 = fs::metadata(heartbeat).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(200));
        let size2 = fs::metadata(heartbeat).map(|m| m.len()).unwrap_or(0);
        if size1 == size2 {
            stopped = true;
            break;
        }
    }
    assert!(
        stopped,
        "descendant pid {descendant} heartbeat must stop growing after group SIGKILL \
         (leader={leader})"
    );

    // Linux: neither PID may remain the original live non-zombie identity.
    // Zombies and starttime mismatches count as cleaned.
    #[cfg(target_os = "linux")]
    {
        let id_deadline = Instant::now() + FORCE_REAP_BOUND;
        while Instant::now() < id_deadline
            && (same_live_identity(leader, leader_starttime)
                || same_live_identity(descendant, descendant_starttime))
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !same_live_identity(leader, leader_starttime),
            "leader pid {leader} must not remain a live original identity after group cleanup"
        );
        assert!(
            !same_live_identity(descendant, descendant_starttime),
            "descendant pid {descendant} must not remain a live original identity after group SIGKILL"
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (leader, leader_starttime, descendant, descendant_starttime);
    }
}

/// RAII cleanup for hang-group fixtures used by ownership-unknown tests.
///
/// Armed by default so any assertion panic still SIGKILLs the process group and
/// best-effort reaps the leader. Happy paths call [`HangGroupCleanup::force_kill_reap_and_assert`]
/// which disarms Drop after identity-safe verification.
pub(super) struct HangGroupCleanup {
    leader: u32,
    descendant: u32,
    leader_starttime: Option<u64>,
    descendant_starttime: Option<u64>,
    heartbeat: PathBuf,
    armed: bool,
}

impl HangGroupCleanup {
    pub(super) fn new(leader: u32, descendant: u32, heartbeat: PathBuf) -> Self {
        Self {
            leader,
            descendant,
            leader_starttime: process_starttime(leader),
            descendant_starttime: process_starttime(descendant),
            heartbeat,
            armed: true,
        }
    }

    pub(super) fn leader(&self) -> u32 {
        self.leader
    }

    pub(super) fn descendant(&self) -> u32 {
        self.descendant
    }

    pub(super) fn leader_starttime(&self) -> Option<u64> {
        self.leader_starttime
    }

    /// Record descendant after it appears; refresh starttime identities.
    pub(super) fn bind_descendant(&mut self, descendant: u32) {
        self.descendant = descendant;
        self.descendant_starttime = process_starttime(descendant);
        self.leader_starttime = process_starttime(self.leader);
    }

    /// Happy-path cleanup: group-kill, identity-safe assert, then disarm Drop.
    pub(super) fn force_kill_reap_and_assert(mut self) {
        force_kill_and_reap(self.leader, self.leader_starttime);
        assert_group_cleaned(
            self.leader,
            self.leader_starttime,
            self.descendant,
            self.descendant_starttime,
            &self.heartbeat,
        );
        self.armed = false;
    }
}

impl Drop for HangGroupCleanup {
    fn drop(&mut self) {
        if self.armed {
            // Panic/assert failure path: never leave infinite-loop fixtures behind.
            // Identity check inside force_kill_and_reap refuses reused PID/PGID.
            force_kill_and_reap(self.leader, self.leader_starttime);
        }
    }
}

/// RAII for a direct `Child` that is not handed to `VersionProbeSession`.
///
/// Arms immediately after spawn so assertion failures cannot leave long-lived
/// sleep/hang children behind. Uses exact-child `kill` + bounded `try_wait`
/// (no negative-PGID — the child is not necessarily a process-group leader).
pub(super) struct ArmedChildCleanup {
    child: Option<std::process::Child>,
}

impl ArmedChildCleanup {
    pub(super) fn new(child: std::process::Child) -> Self {
        Self { child: Some(child) }
    }

    pub(super) fn child_mut(&mut self) -> &mut std::process::Child {
        self.child
            .as_mut()
            .expect("ArmedChildCleanup child still present")
    }
}

impl Drop for ArmedChildCleanup {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let deadline = Instant::now() + FORCE_REAP_BOUND;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break,
            }
        }
    }
}

/// Spawn a hang fixture in its own process group that records a long-lived
/// descendant PID + append-heartbeat (not an ephemeral `sleep 60` alone).
///
/// Returns the leader `Child` (for hand-off into `VersionProbeSession`) and an
/// armed [`HangGroupCleanup`] guard. The guard kills the group on Drop unless
/// the test explicitly finishes with [`HangGroupCleanup::force_kill_reap_and_assert`].
pub(super) fn spawn_hang_group_with_descendant(
    temp: &Path,
) -> (std::process::Child, HangGroupCleanup) {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let pid_file = temp.join("descendant.pid");
    let hb = temp.join("descendant.hb");
    // Append heartbeat so cleanup can assert growth stopped (overwrite-only
    // timestamps keep a constant size and cannot prove the writer died).
    let shell = format!(
        "trap '' TERM\n\
         (while true; do printf 'x' >>'{hb}'; sleep 0.05; done) &\n\
         echo $! >'{pid}'\n\
         while true; do sleep 60; done\n",
        hb = hb.display(),
        pid = pid_file.display(),
    );

    let child = Command::new("sh")
        .args(["-c", &shell])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn hang fixture with descendant");
    let leader = child.id();

    // Arm cleanup immediately so setup assert failures cannot leak the leader.
    let mut cleanup = HangGroupCleanup::new(leader, 0, hb.clone());

    let deadline = Instant::now() + FORCE_REAP_BOUND;
    let mut descendant = 0u32;
    while Instant::now() < deadline {
        if let Ok(s) = fs::read_to_string(&pid_file)
            && let Ok(p) = s.trim().parse::<u32>()
            && p > 1
        {
            descendant = p;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        descendant > 1,
        "descendant pid must be recorded under {}",
        pid_file.display()
    );
    cleanup.bind_descendant(descendant);

    let hb_deadline = Instant::now() + FORCE_REAP_BOUND;
    while Instant::now() < hb_deadline {
        if let Ok(meta) = fs::metadata(&hb)
            && meta.len() > 0
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        fs::metadata(&hb).map(|m| m.len()).unwrap_or(0) > 0,
        "descendant must write heartbeat before ownership-unknown assertions"
    );
    assert!(
        process_exists(leader) && process_exists(descendant),
        "leader and descendant must both be alive before probe cleanup"
    );

    (child, cleanup)
}
