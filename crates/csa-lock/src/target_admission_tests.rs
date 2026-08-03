use crate::target_admission::acquire_target_gc_admission_for_test;
use crate::{TargetGcAdmissionLease, acquire_target_gc_admission};
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

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
    // SAFETY: fork has no Rust-side work in the child except raw syscalls and immediate exit.
    let pid = unsafe { libc::fork() };
    assert_ne!(pid, -1, "fork should succeed");
    if pid == 0 {
        // SAFETY: this is the inherited lease fd; closing it leaves an independent probe.
        unsafe { libc::close(inherited_fd) };
        let file = OpenOptions::new()
            .read(true)
            .open(parent)
            .expect("child open parent");
        // SAFETY: `file` owns a valid directory fd.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        // SAFETY: exit immediately after raw syscall-only child work.
        unsafe { libc::_exit(if rc == 0 { 0 } else { 1 }) };
    }
    let mut status = 0;
    // SAFETY: waiting for the child PID returned by fork.
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 1
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
    super::assert_fd_cloexec(lease.file.as_raw_fd());
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
