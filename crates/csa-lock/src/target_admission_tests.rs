use crate::target_admission::acquire_target_gc_admission_for_test;
use crate::{TargetGcAdmissionLease, acquire_target_gc_admission};
use std::fs::{self, File, OpenOptions};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[cfg(target_os = "linux")]
const LEASE_CHILD_ENV: &str = "CSA_TARGET_ADMISSION_LEASE_CHILD";

#[cfg(target_os = "linux")]
struct ManagedTargetWorkspace {
    workspace: tempfile::TempDir,
    mirror_root: tempfile::TempDir,
    canonical_parent: PathBuf,
}

#[cfg(target_os = "linux")]
impl ManagedTargetWorkspace {
    fn new() -> Self {
        let workspace = tempdir().expect("workspace tempdir");
        let mirror_root = tempdir().expect("mirror root tempdir");
        let canonical_parent = mirror_root.path().join(
            workspace
                .path()
                .strip_prefix("/")
                .expect("absolute workspace"),
        );
        fs::create_dir_all(&canonical_parent).expect("create canonical parent fixture");
        std::os::unix::fs::symlink(
            canonical_parent.join("target"),
            workspace.path().join("target"),
        )
        .expect("create managed target symlink");
        Self {
            workspace,
            mirror_root,
            canonical_parent,
        }
    }

    fn acquire(&self) -> anyhow::Result<Option<TargetGcAdmissionLease>> {
        acquire_target_gc_admission_for_test(self.workspace.path(), self.mirror_root.path(), || {})
    }
}

#[cfg(target_os = "linux")]
impl Drop for ManagedTargetWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.canonical_parent);
    }
}

#[cfg(target_os = "linux")]
struct DirectoryFlock {
    file: File,
}

#[cfg(target_os = "linux")]
impl DirectoryFlock {
    fn exclusive(path: &Path) -> Self {
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .expect("open directory");
        // SAFETY: `file` owns the directory fd and the nonblocking flock result is checked.
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        Self { file }
    }
}

#[cfg(target_os = "linux")]
impl Drop for DirectoryFlock {
    fn drop(&mut self) {
        // SAFETY: `file` owns the directory fd.
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(target_os = "linux")]
fn child_exclusive_lock_is_blocked(parent: &Path, inherited_fd: std::os::unix::io::RawFd) -> bool {
    let probe = OpenOptions::new()
        .read(true)
        .open(parent)
        .expect("open independent probe before fork");
    let probe_fd = probe.as_raw_fd();
    // SAFETY: the child only closes inherited fds, flocks its pre-opened probe, and exits.
    let pid = unsafe { libc::fork() };
    assert_ne!(pid, -1, "fork should succeed");
    if pid == 0 {
        // SAFETY: these fds were opened before fork; only async-signal-safe syscalls run here.
        unsafe {
            libc::close(inherited_fd);
            let rc = libc::flock(probe_fd, libc::LOCK_EX | libc::LOCK_NB);
            libc::_exit(if rc == 0 { 0 } else { 1 });
        }
    }
    let mut status = 0;
    // SAFETY: waiting for the child PID returned by fork.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 1
}

#[cfg(target_os = "linux")]
fn exclusive_lock_is_available(parent: &Path) -> bool {
    let probe = OpenOptions::new()
        .read(true)
        .open(parent)
        .expect("open independent exclusive probe");
    // SAFETY: `probe` owns the directory fd and the nonblocking result is checked.
    let acquired = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if acquired {
        // SAFETY: the same live fd acquired the lock above.
        unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) };
    }
    acquired
}

#[cfg(target_os = "linux")]
fn assert_fd_not_cloexec(fd: std::os::unix::io::RawFd) {
    // SAFETY: `fd` is owned by a live lease guard in the calling test.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_ne!(flags, -1, "F_GETFD should succeed");
    assert_eq!(
        flags & libc::FD_CLOEXEC,
        0,
        "target GC admission lease fd must be inherited across exec"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_lease_owner_subprocess() {
    let Ok(workspace) = std::env::var("CSA_TARGET_ADMISSION_WORKSPACE") else {
        return;
    };
    if std::env::var_os(LEASE_CHILD_ENV).is_none() {
        return;
    }
    let mirror_root = std::env::var("CSA_TARGET_ADMISSION_MIRROR_ROOT").expect("mirror root");
    let pid_file = std::env::var("CSA_TARGET_ADMISSION_DESCENDANT_PID").expect("pid file");
    let lease =
        acquire_target_gc_admission_for_test(Path::new(&workspace), Path::new(&mirror_root), || {})
            .expect("subprocess admission")
            .expect("managed subprocess admission");
    Command::new("sh")
        .arg("-c")
        .arg("trap '' TERM; printf '%s' \"$$\" >\"$1\"; while :; do sleep 1; done")
        .arg("sh")
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn TERM-resistant descendant");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !Path::new(&pid_file).exists() {
        assert!(Instant::now() < deadline, "descendant did not publish pid");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::mem::forget(lease);
    std::process::exit(0);
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_lease_survives_owner_exit_until_descendant_termination() {
    let fixture = ManagedTargetWorkspace::new();
    let pid_file = fixture.workspace.path().join("lease-descendant.pid");
    let status = Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("tests::target_admission_tests::target_gc_admission_lease_owner_subprocess")
        .arg("--nocapture")
        .env(LEASE_CHILD_ENV, "1")
        .env("CSA_TARGET_ADMISSION_WORKSPACE", fixture.workspace.path())
        .env(
            "CSA_TARGET_ADMISSION_MIRROR_ROOT",
            fixture.mirror_root.path(),
        )
        .env("CSA_TARGET_ADMISSION_DESCENDANT_PID", &pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run lease-owning subprocess");
    assert!(status.success(), "lease-owning subprocess failed: {status}");
    let descendant_pid = fs::read_to_string(&pid_file)
        .expect("descendant pid")
        .parse::<i32>()
        .expect("numeric descendant pid");

    // SAFETY: `descendant_pid` was published by the owned child; TERM probes its resistance.
    assert_eq!(unsafe { libc::kill(descendant_pid, libc::SIGTERM) }, 0);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !exclusive_lock_is_available(&fixture.canonical_parent),
        "exclusive GC lock must remain blocked after the lease owner exits"
    );

    // SAFETY: the same owned descendant must be terminated to release its inherited lease fd.
    let _ = unsafe { libc::kill(descendant_pid, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while !exclusive_lock_is_available(&fixture.canonical_parent) {
        assert!(
            Instant::now() < deadline,
            "exclusive GC lock remained blocked after descendant termination"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_holds_shared_parent_inode_lock_without_marker() {
    let fixture = ManagedTargetWorkspace::new();
    let marker = fixture
        .canonical_parent
        .join(".rust-target-gc-admission-v1");
    assert!(!marker.exists());

    let lease = fixture
        .acquire()
        .expect("managed target should acquire admission")
        .expect("managed target should require admission");

    assert!(
        child_exclusive_lock_is_blocked(&fixture.canonical_parent, lease.file.as_raw_fd()),
        "independent GC exclusive lock must be blocked"
    );
    assert_fd_not_cloexec(lease.file.as_raw_fd());
    assert!(
        !marker.exists(),
        "Rust admission must not create wrapper marker"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_reports_busy_then_retries_after_gc_release() {
    let fixture = ManagedTargetWorkspace::new();
    let holder = DirectoryFlock::exclusive(&fixture.canonical_parent);

    let error = fixture
        .acquire()
        .expect_err("GC exclusive holder must fail closed")
        .to_string();
    assert!(
        error.contains("target GC admission busy"),
        "unexpected error: {error}"
    );

    drop(holder);
    assert!(
        fixture
            .acquire()
            .expect("released GC lock should allow retry")
            .is_some()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_retains_existing_parent_lease_for_later_managed_opt_in() {
    let fixture = ManagedTargetWorkspace::new();
    fs::remove_file(fixture.workspace.path().join("target")).expect("remove managed target");
    let holder = DirectoryFlock::exclusive(&fixture.canonical_parent);

    let error = fixture
        .acquire()
        .expect_err("existing parent must be shared-locked before target inspection")
        .to_string();
    assert!(
        error.contains("target GC admission busy"),
        "unexpected error: {error}"
    );

    drop(holder);
    let lease = fixture
        .acquire()
        .expect("existing canonical parent must retain admission")
        .expect("mutable workspace target must not release shared admission");
    assert!(
        child_exclusive_lock_is_blocked(&fixture.canonical_parent, lease.file.as_raw_fd()),
        "cooperating GC exclusive lock must remain blocked while workspace opts in"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_rejects_relative_workspace_path() {
    let error = acquire_target_gc_admission_for_test(
        Path::new("relative-workspace"),
        Path::new("/mirror"),
        || {},
    )
    .expect_err("relative workspace must fail")
    .to_string();
    assert!(
        error.contains("requires an absolute workspace path"),
        "unexpected error: {error}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_rejects_dangling_managed_target_without_creation() {
    let workspace = tempdir().expect("workspace tempdir");
    let mirror_root = tempdir().expect("mirror root tempdir");
    let canonical_parent = mirror_root.path().join(
        workspace
            .path()
            .strip_prefix("/")
            .expect("absolute workspace"),
    );
    std::os::unix::fs::symlink(
        canonical_parent.join("target"),
        workspace.path().join("target"),
    )
    .expect("create dangling managed symlink");

    let error = acquire_target_gc_admission_for_test(workspace.path(), mirror_root.path(), || {})
        .expect_err("dangling managed target must fail closed")
        .to_string();
    assert!(
        error.contains("failed to open canonical target parent"),
        "unexpected error: {error}"
    );
    assert!(
        !canonical_parent.exists(),
        "admission must not create the parent"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_returns_none_for_unmanaged_target() {
    let workspace = tempdir().expect("workspace tempdir");
    assert!(
        acquire_target_gc_admission(workspace.path())
            .expect("missing target is unmanaged")
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_rejects_parent_identity_replacement() {
    let fixture = ManagedTargetWorkspace::new();
    let replaced_parent = fixture.canonical_parent.with_extension("replaced");
    let _ = fs::remove_dir_all(&replaced_parent);

    let error = acquire_target_gc_admission_for_test(
        fixture.workspace.path(),
        fixture.mirror_root.path(),
        || {
            fs::rename(&fixture.canonical_parent, &replaced_parent).expect("move opened parent");
            fs::create_dir(&fixture.canonical_parent).expect("replace parent path");
        },
    )
    .expect_err("parent replacement must fail closed")
    .to_string();
    assert!(
        error.contains("identity changed"),
        "unexpected error: {error}"
    );

    fs::remove_dir(&fixture.canonical_parent).expect("remove replacement");
    fs::rename(&replaced_parent, &fixture.canonical_parent).expect("restore original parent");
}

#[cfg(target_os = "linux")]
#[test]
fn target_gc_admission_rejects_interior_nul_path_without_panicking() {
    let workspace = PathBuf::from(std::ffi::OsString::from_vec(b"/workspace\0suffix".to_vec()));
    let mirror_root = tempdir().expect("mirror root tempdir");
    let error = acquire_target_gc_admission_for_test(&workspace, mirror_root.path(), || {})
        .expect_err("interior NUL must be rejected")
        .context("expected invalid-input error");

    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::InvalidInput)
    }));
}
