use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use tempfile::tempdir;

use crate::atomic_state_write::AtomicWriteFault;
use crate::convergence::{
    CampaignId, CampaignRecord, ConvergenceAppendError, ConvergenceEvent, ConvergenceLedgerStore,
};

const CAMPAIGN_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const CAMPAIGN_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const CAMPAIGN_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const PUBLIC_CHILD_ENV: &str = "CSA_CONVERGENCE_PUBLIC_CHILD";
const PUBLIC_PROJECT_ENV: &str = "CSA_CONVERGENCE_PUBLIC_PROJECT";
const PUBLIC_STATE_ENV: &str = "CSA_CONVERGENCE_PUBLIC_STATE";

fn campaign_start(value: &str) -> (CampaignId, ConvergenceEvent) {
    let id = CampaignId::parse(value).unwrap();
    (
        id.clone(),
        ConvergenceEvent::CampaignStarted(CampaignRecord::for_test(id, Utc::now(), None)),
    )
}

fn store_at(root: &Path) -> ConvergenceLedgerStore {
    ConvergenceLedgerStore::for_project_state_root(root).unwrap()
}

fn convergence_dir(root: &Path) -> PathBuf {
    root.join("convergence")
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_valid_ledger(root: &Path) {
    let store = store_at(root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
}

#[test]
fn convergence_store_rejects_distinct_non_utf8_projects_without_state_writes() {
    let _env_lock = crate::test_env::TEST_ENV_LOCK.lock().unwrap();
    let temp = tempdir().unwrap();
    let state_home = temp.path().join("state-home");
    let old_state = std::env::var_os("XDG_STATE_HOME");
    unsafe { std::env::set_var("XDG_STATE_HOME", &state_home) };

    for suffix in [0x80, 0x81] {
        let mut name = b"project-".to_vec();
        name.push(suffix);
        let project = temp.path().join(OsString::from_vec(name));
        fs::create_dir(&project).unwrap();
        let error = ConvergenceLedgerStore::for_project(&project).unwrap_err();
        assert!(error.to_string().contains("UTF-8"));
    }

    assert!(!state_home.exists());
    if let Some(value) = old_state {
        unsafe { std::env::set_var("XDG_STATE_HOME", value) };
    } else {
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }
}

#[test]
fn convergence_store_rejects_existing_and_dangling_secure_boundary_symlinks() {
    let temp = tempdir().unwrap();
    let real_root = temp.path().join("real-state");
    fs::create_dir(&real_root).unwrap();
    set_mode(&real_root, 0o700);
    write_valid_ledger(&real_root);

    let linked_root = temp.path().join("linked-state");
    symlink(&real_root, &linked_root).unwrap();
    assert!(store_at(&linked_root).load().is_err());

    let dangling_root = temp.path().join("dangling-state");
    symlink(temp.path().join("missing-target"), &dangling_root).unwrap();
    assert!(store_at(&dangling_root).load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store_at(&dangling_root).append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
}

#[test]
fn convergence_store_rejects_existing_and_dangling_convergence_symlinks() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let target_root = temp.path().join("target-state");
    fs::create_dir(&root).unwrap();
    set_mode(&root, 0o700);
    write_valid_ledger(&target_root);

    symlink(convergence_dir(&target_root), convergence_dir(&root)).unwrap();
    assert!(store_at(&root).load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store_at(&root).append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));

    fs::remove_file(convergence_dir(&root)).unwrap();
    symlink(
        temp.path().join("missing-convergence"),
        convergence_dir(&root),
    )
    .unwrap();
    assert!(store_at(&root).load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store_at(&root).append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
}

#[test]
fn convergence_store_directory_replacement_before_publish_is_not_published() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    let original = convergence_dir(&root);
    let displaced = root.join("displaced-convergence");
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);

    let error = store
        .append_with_before_publish(campaign_id, event, |_| {
            fs::rename(&original, &displaced)?;
            fs::create_dir(&original)?;
            set_mode(&original, 0o700);
            Ok(())
        })
        .unwrap_err();

    assert!(matches!(error, ConvergenceAppendError::NotPublished(_)));
    assert!(!original.join("ledger.json").exists());
    let displaced_ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(displaced.join("ledger.json")).unwrap()).unwrap();
    assert_eq!(displaced_ledger["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn convergence_store_public_child_helper() {
    if std::env::var_os(PUBLIC_CHILD_ENV).is_none() {
        return;
    }
    let project = PathBuf::from(std::env::var_os(PUBLIC_PROJECT_ENV).unwrap());
    let state_home = PathBuf::from(std::env::var_os(PUBLIC_STATE_ENV).unwrap());
    unsafe { std::env::set_var("XDG_STATE_HOME", state_home) };
    // SAFETY: `umask` changes only this dedicated child process and accepts every mode value.
    let previous = unsafe { libc::umask(0) };
    let store = ConvergenceLedgerStore::for_project(&project).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    // SAFETY: restoring the mode returned by `umask` is valid in this dedicated child process.
    unsafe { libc::umask(previous) };
}

struct ReviewChild {
    child: Option<Child>,
}

impl ReviewChild {
    fn spawn_public(project: &Path, state_home: &Path) -> Self {
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("convergence_store_review_tests::convergence_store_public_child_helper")
            .arg("--nocapture")
            .env(PUBLIC_CHILD_ENV, "1")
            .env(PUBLIC_PROJECT_ENV, project)
            .env(PUBLIC_STATE_ENV, state_home)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self { child: Some(child) }
    }

    fn wait_success(mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let child = self.child.as_mut().unwrap();
            if let Some(status) = child.try_wait().unwrap() {
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut stderr| {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut stderr, &mut bytes).unwrap();
                        bytes
                    })
                    .unwrap_or_default();
                assert!(
                    status.success(),
                    "public child failed: {}",
                    String::from_utf8_lossy(&stderr)
                );
                self.child.take();
                return;
            }
            assert!(Instant::now() < deadline, "public child timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ReviewChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn convergence_store_umask_zero_creates_private_public_state_hierarchy() {
    let temp = tempdir().unwrap();
    let state_home = temp.path().join("missing-state-home");
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();

    ReviewChild::spawn_public(&project, &state_home).wait_success();

    let _env_lock = crate::test_env::TEST_ENV_LOCK.lock().unwrap();
    let old_state = std::env::var_os("XDG_STATE_HOME");
    unsafe { std::env::set_var("XDG_STATE_HOME", &state_home) };
    let project_root = crate::manager::get_session_root(&project).unwrap();
    if let Some(value) = old_state {
        unsafe { std::env::set_var("XDG_STATE_HOME", value) };
    } else {
        unsafe { std::env::remove_var("XDG_STATE_HOME") };
    }

    let euid = unsafe { libc::geteuid() };
    let directories = [
        state_home.clone(),
        state_home.join("cli-sub-agent"),
        project_root.clone(),
        convergence_dir(&project_root),
    ];
    for directory in directories {
        let metadata = fs::symlink_metadata(&directory).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.uid(), euid);
        assert_eq!(metadata.permissions().mode() & 0o022, 0);
    }
    assert_eq!(
        fs::metadata(convergence_dir(&project_root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for file in [
        convergence_dir(&project_root).join("ledger.lock"),
        convergence_dir(&project_root).join("ledger.json"),
    ] {
        let metadata = fs::metadata(file).unwrap();
        assert_eq!(metadata.uid(), euid);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn convergence_store_rejects_group_writable_secure_boundary() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    fs::create_dir(&root).unwrap();
    set_mode(&root, 0o720);
    let store = store_at(&root);
    assert!(store.load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o720
    );
}

#[test]
fn convergence_store_rejects_group_writable_convergence_without_tightening_it() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    write_valid_ledger(&root);
    set_mode(&convergence_dir(&root), 0o720);
    let store = store_at(&root);
    assert!(store.load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert_eq!(
        fs::metadata(convergence_dir(&root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o720
    );
}

#[test]
fn convergence_store_rejects_fifo_ledger_without_blocking() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    fs::create_dir_all(convergence_dir(&root)).unwrap();
    set_mode(&root, 0o700);
    set_mode(&convergence_dir(&root), 0o700);
    let path = convergence_dir(&root).join("ledger.json");
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let result = unsafe { libc::mkfifo(name.as_ptr(), 0o600) };
    assert_eq!(result, 0);
    assert!(store_at(&root).load().is_err());
}

#[test]
fn convergence_store_constructor_rejects_non_normalized_or_relative_roots() {
    let temp = tempdir().unwrap();
    assert!(ConvergenceLedgerStore::for_project_state_root(Path::new("relative")).is_err());
    assert!(
        ConvergenceLedgerStore::for_project_state_root(&temp.path().join("child/../state"))
            .is_err()
    );
}

#[test]
fn convergence_store_existing_ledger_owner_and_mode_are_verified() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    write_valid_ledger(&root);
    let ledger = convergence_dir(&root).join("ledger.json");
    let file = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&ledger)
        .unwrap();
    file.set_permissions(fs::Permissions::from_mode(0o660))
        .unwrap();
    assert!(store_at(&root).load().is_err());
}

#[test]
fn convergence_store_rejects_serialized_growth_past_limit_without_replacement() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    let ledger_path = convergence_dir(&root).join("ledger.json");
    let before = fs::read(&ledger_path).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);

    let error = store
        .append_with_max_for_test(campaign_id, event, before.len() as u64)
        .unwrap_err();

    assert!(matches!(error, ConvergenceAppendError::NotPublished(_)));
    assert!(error.retry_is_safe());
    assert!(error.attempted_entry().is_none());
    assert_eq!(fs::read(ledger_path).unwrap(), before);
}

#[test]
fn convergence_store_uncertain_error_exposes_exact_entry_for_reconciliation() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);

    let error = store
        .append_with_fault(campaign_id, event, AtomicWriteFault::AfterRename)
        .unwrap_err();
    let attempted = error.attempted_entry().unwrap().clone();
    assert!(error.may_have_been_published());
    assert!(!error.retry_is_safe());

    let (campaign_id, event) = campaign_start(CAMPAIGN_C);
    store.append(campaign_id, event).unwrap();
    let ledger = store.load().unwrap();
    ledger.validate().unwrap();
    assert_eq!(ledger.entries().len(), 3);
    assert_eq!(
        ledger
            .entries()
            .iter()
            .filter(|entry| entry.event_id() == attempted.event_id())
            .count(),
        1
    );
    assert_eq!(ledger.entries()[1], attempted);
}
