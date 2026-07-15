use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use tempfile::tempdir;

use crate::atomic_state_write::AtomicWriteFault;
use crate::convergence::{
    CampaignId, CampaignRecord, ConvergenceAppendError, ConvergenceEvent, ConvergenceLedger,
    ConvergenceLedgerStore, MAX_LEDGER_BYTES,
};

const CAMPAIGN_A: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const CAMPAIGN_B: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const CAMPAIGN_C: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
const CHILD_ROLE_ENV: &str = "CSA_CONVERGENCE_STORE_CHILD";
const CHILD_ROOT_ENV: &str = "CSA_CONVERGENCE_STORE_ROOT";
const CHILD_CAMPAIGN_ENV: &str = "CSA_CONVERGENCE_STORE_CAMPAIGN";
const CHILD_READY_ENV: &str = "CSA_CONVERGENCE_STORE_READY";

fn campaign(value: &str) -> CampaignId {
    CampaignId::parse(value).unwrap()
}

fn campaign_start(value: &str) -> (CampaignId, ConvergenceEvent) {
    let id = campaign(value);
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

fn ledger_path(root: &Path) -> PathBuf {
    convergence_dir(root).join("ledger.json")
}

fn lock_path(root: &Path) -> PathBuf {
    convergence_dir(root).join("ledger.lock")
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_ledger_bytes(root: &Path, bytes: &[u8]) {
    fs::create_dir_all(convergence_dir(root)).unwrap();
    set_mode(&convergence_dir(root), 0o700);
    fs::write(ledger_path(root), bytes).unwrap();
    set_mode(&ledger_path(root), 0o600);
}

fn valid_ledger_bytes(campaign_id: &str) -> Vec<u8> {
    let mut ledger = ConvergenceLedger::empty();
    let (campaign_id, event) = campaign_start(campaign_id);
    ledger.append(campaign_id, event).unwrap();
    let mut bytes = serde_json::to_vec_pretty(&ledger).unwrap();
    bytes.push(b'\n');
    bytes
}

fn assert_corrupt_append_preserves_bytes(root: &Path, bytes: &[u8]) {
    write_ledger_bytes(root, bytes);
    let store = store_at(root);
    assert!(store.load().is_err());
    let before = fs::read(ledger_path(root)).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    let error = store.append(campaign_id, event).unwrap_err();
    assert!(matches!(error, ConvergenceAppendError::NotPublished(_)));
    assert!(error.retry_is_safe());
    assert!(!error.may_have_been_published());
    assert!(error.attempted_entry().is_none());
    assert_eq!(fs::read(ledger_path(root)).unwrap(), before);
}

#[test]
fn convergence_store_missing_load_is_empty_without_filesystem_side_effects() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("missing-project-state");
    let store = store_at(&root);

    assert_eq!(store.load().unwrap(), ConvergenceLedger::empty());
    assert!(!root.exists());
}

struct EnvGuard {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = self.old.take() {
            unsafe { std::env::set_var(self.key, value) };
        } else {
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

#[test]
fn convergence_store_for_project_is_canonical_and_rejects_invalid_roots_before_writes() {
    let _env_lock = crate::test_env::TEST_ENV_LOCK.lock().unwrap();
    let temp = tempdir().unwrap();
    let state_home = temp.path().join("state-home");
    let _xdg = EnvGuard::set("XDG_STATE_HOME", &state_home);
    let missing = temp.path().join("missing-project");
    let non_directory = temp.path().join("project-file");
    fs::write(&non_directory, b"not a directory").unwrap();

    assert!(ConvergenceLedgerStore::for_project(&missing).is_err());
    assert!(ConvergenceLedgerStore::for_project(&non_directory).is_err());
    assert!(!state_home.exists());

    let project = temp.path().join("project");
    let alias = temp.path().join("project-alias");
    fs::create_dir(&project).unwrap();
    symlink(&project, &alias).unwrap();
    let canonical_store = ConvergenceLedgerStore::for_project(&project).unwrap();
    let alias_store = ConvergenceLedgerStore::for_project(&alias).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    canonical_store.append(campaign_id, event).unwrap();

    assert_eq!(alias_store.load().unwrap().entries().len(), 1);
    let expected_root =
        crate::manager::get_session_root(&fs::canonicalize(&project).unwrap()).unwrap();
    assert!(expected_root.join("convergence/ledger.json").is_file());
    assert!(!state_home.join("cli-sub-agent/project-alias").exists());
}

#[test]
fn convergence_store_append_round_trips_prefix_tail_modes_and_persistent_lock() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_a, event_a) = campaign_start(CAMPAIGN_A);
    let returned_a = store.append(campaign_a, event_a).unwrap();
    let before = store.load().unwrap();
    let (campaign_b, event_b) = campaign_start(CAMPAIGN_B);
    let returned_b = store.append(campaign_b, event_b).unwrap();
    let after = store.load().unwrap();

    assert_eq!(returned_a, before.entries()[0]);
    assert_eq!(&after.entries()[..before.entries().len()], before.entries());
    assert_eq!(after.entries().last(), Some(&returned_b));
    assert_eq!(after.entries()[0], returned_a);
    assert_eq!(after.entries()[1].sequence(), 2);
    after.validate().unwrap();
    assert!(lock_path(&root).is_file());
    assert_eq!(
        fs::metadata(convergence_dir(&root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(ledger_path(&root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(lock_path(&root)).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn convergence_store_load_and_append_fail_closed_for_corrupt_ledgers() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");

    assert_corrupt_append_preserves_bytes(&root, b"{not-json");
    assert_corrupt_append_preserves_bytes(&root, b"\xff\xfe");

    let mut unknown: serde_json::Value =
        serde_json::from_slice(&valid_ledger_bytes(CAMPAIGN_A)).unwrap();
    unknown["unknown"] = serde_json::json!(true);
    assert_corrupt_append_preserves_bytes(&root, &serde_json::to_vec(&unknown).unwrap());

    let mut future: serde_json::Value =
        serde_json::from_slice(&valid_ledger_bytes(CAMPAIGN_A)).unwrap();
    future["schema_version"] = serde_json::json!(u32::MAX);
    assert_corrupt_append_preserves_bytes(&root, &serde_json::to_vec(&future).unwrap());

    let mut tampered: serde_json::Value =
        serde_json::from_slice(&valid_ledger_bytes(CAMPAIGN_A)).unwrap();
    tampered["entries"][0]["sequence"] = serde_json::json!(2);
    assert_corrupt_append_preserves_bytes(&root, &serde_json::to_vec(&tampered).unwrap());

    write_ledger_bytes(&root, &valid_ledger_bytes(CAMPAIGN_A));
    set_mode(&ledger_path(&root), 0o640);
    let store = store_at(&root);
    assert!(store.load().is_err());
    let before = fs::read(ledger_path(&root)).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert_eq!(fs::read(ledger_path(&root)).unwrap(), before);
}

#[test]
fn convergence_store_load_rejects_oversized_symlink_and_non_regular_paths() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    fs::create_dir_all(convergence_dir(&root)).unwrap();
    set_mode(&convergence_dir(&root), 0o700);

    let sparse = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(ledger_path(&root))
        .unwrap();
    sparse.set_len(MAX_LEDGER_BYTES + 1).unwrap();
    drop(sparse);
    let store = store_at(&root);
    assert!(store.load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert_eq!(
        fs::metadata(ledger_path(&root)).unwrap().len(),
        MAX_LEDGER_BYTES + 1
    );

    fs::remove_file(ledger_path(&root)).unwrap();
    let target = temp.path().join("target-ledger.json");
    fs::write(&target, valid_ledger_bytes(CAMPAIGN_A)).unwrap();
    set_mode(&target, 0o600);
    symlink(&target, ledger_path(&root)).unwrap();
    assert!(store.load().is_err());
    let target_before = fs::read(&target).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert_eq!(fs::read(&target).unwrap(), target_before);

    fs::remove_file(ledger_path(&root)).unwrap();
    fs::create_dir(ledger_path(&root)).unwrap();
    assert!(store.load().is_err());
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    assert!(matches!(
        store.append(campaign_id, event),
        Err(ConvergenceAppendError::NotPublished(_))
    ));
    assert!(ledger_path(&root).is_dir());
}

#[test]
fn convergence_store_pre_rename_failure_is_not_published_and_preserves_old_bytes() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    let before = fs::read(ledger_path(&root)).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);

    let error = store
        .append_with_fault(campaign_id, event, AtomicWriteFault::BeforeRename)
        .unwrap_err();

    assert!(matches!(error, ConvergenceAppendError::NotPublished(_)));
    assert!(error.retry_is_safe());
    assert!(error.attempted_entry().is_none());
    assert_eq!(fs::read(ledger_path(&root)).unwrap(), before);
    let names = fs::read_dir(convergence_dir(&root))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        names,
        std::collections::HashSet::from(["ledger.json".into(), "ledger.lock".into()])
    );
}

#[test]
fn convergence_store_post_rename_failure_reports_publication_uncertainty() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    store.append(campaign_id, event).unwrap();
    let (campaign_id, event) = campaign_start(CAMPAIGN_B);

    let error = store
        .append_with_fault(campaign_id, event, AtomicWriteFault::AfterRename)
        .unwrap_err();

    assert!(matches!(
        error,
        ConvergenceAppendError::PublishedButDurabilityUnconfirmed { .. }
    ));
    assert!(error.may_have_been_published());
    assert!(!error.retry_is_safe());
    assert!(error.attempted_entry().is_some());
    let visible = store.load().unwrap();
    visible.validate().unwrap();
    assert_eq!(visible.entries().len(), 2);
    assert_eq!(visible.entries()[1].campaign_id(), &campaign(CAMPAIGN_B));
}

#[test]
fn convergence_store_holds_lock_through_publication() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    let mut observed_locked = false;

    store
        .append_with_before_publish(campaign_id, event, |path| {
            let probe_file = OpenOptions::new().read(true).write(true).open(path)?;
            let mut probe = fd_lock::RwLock::new(probe_file);
            observed_locked = probe.try_write().is_err();
            Ok(())
        })
        .unwrap();

    assert!(
        observed_locked,
        "write lock must remain held until publication completes"
    );
}

#[test]
fn convergence_store_child_append_helper() {
    if std::env::var_os(CHILD_ROLE_ENV).is_none() {
        return;
    }
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT_ENV).unwrap());
    let campaign_id = std::env::var(CHILD_CAMPAIGN_ENV).unwrap();
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(&campaign_id);
    fs::write(std::env::var_os(CHILD_READY_ENV).unwrap(), b"ready\n").unwrap();
    store.append(campaign_id, event).unwrap();
}

fn spawn_child(root: &Path, campaign_id: &str, readiness: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("convergence_store_tests::convergence_store_child_append_helper")
        .arg("--nocapture")
        .env(CHILD_ROLE_ENV, "append")
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_CAMPAIGN_ENV, campaign_id)
        .env(CHILD_READY_ENV, readiness)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

struct ChildProcess {
    child: Child,
    status: Option<ExitStatus>,
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct ChildSet {
    children: Vec<ChildProcess>,
}

impl ChildSet {
    fn spawn(root: &Path, campaigns: [(&str, &Path); 2]) -> Self {
        Self {
            children: campaigns
                .into_iter()
                .map(|(campaign, readiness)| ChildProcess {
                    child: spawn_child(root, campaign, readiness),
                    status: None,
                })
                .collect(),
        }
    }

    fn poll(&mut self) {
        for child in &mut self.children {
            if child.status.is_none() {
                child.status = child.child.try_wait().unwrap();
            }
        }
    }

    fn all_running(&mut self) -> bool {
        self.poll();
        self.children.iter().all(|child| child.status.is_none())
    }

    fn wait_success(mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while self.children.iter().any(|child| child.status.is_none()) {
            self.poll();
            assert!(
                Instant::now() < deadline,
                "convergence store children timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        for (index, child) in self.children.iter_mut().enumerate() {
            let status = child.status.unwrap();
            let stderr = child
                .child
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
                "child {index} failed: {}",
                String::from_utf8_lossy(&stderr)
            );
        }
    }
}

fn wait_for_readiness(paths: &[PathBuf]) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while paths.iter().any(|path| !path.is_file()) {
        assert!(Instant::now() < deadline, "child readiness timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn convergence_store_forced_contention_does_not_lose_updates() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("state");
    let readiness = [
        temp.path().join("child-b.ready"),
        temp.path().join("child-c.ready"),
    ];
    let store = store_at(&root);
    let (campaign_id, event) = campaign_start(CAMPAIGN_A);
    let mut children = None;
    store
        .append_with_before_publish(campaign_id, event, |path| {
            let probe_file = OpenOptions::new().read(true).write(true).open(path)?;
            let mut probe = fd_lock::RwLock::new(probe_file);
            assert!(probe.try_write().is_err());
            let mut child_set = ChildSet::spawn(
                &root,
                [
                    (CAMPAIGN_B, readiness[0].as_path()),
                    (CAMPAIGN_C, readiness[1].as_path()),
                ],
            );
            wait_for_readiness(&readiness);
            assert!(
                child_set.all_running(),
                "children must remain blocked while the parent holds the ledger lock"
            );
            children = Some(child_set);
            Ok(())
        })
        .unwrap();
    children.unwrap().wait_success();

    let ledger = store_at(&root).load().unwrap();
    ledger.validate().unwrap();
    assert_eq!(ledger.entries().len(), 3);
    assert_eq!(ledger.entries()[0].sequence(), 1);
    assert_eq!(ledger.entries()[1].sequence(), 2);
    assert_eq!(ledger.entries()[2].sequence(), 3);
    let campaigns = ledger
        .entries()
        .iter()
        .map(|entry| entry.campaign_id().as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        campaigns,
        std::collections::HashSet::from([CAMPAIGN_A, CAMPAIGN_B, CAMPAIGN_C])
    );
    assert!(lock_path(&root).is_file());
}

#[test]
fn provider_evidence_bundle_is_private_content_addressed_and_verified() {
    let td = tempdir().expect("temp state root");
    let store = store_at(td.path());
    let published = store
        .publish_provider_evidence_bundle(b"immutable evidence")
        .expect("publish provider evidence bundle");

    assert_eq!(
        published.digest(),
        &crate::convergence::Sha256Digest::compute(b"immutable evidence")
    );
    assert_eq!(
        published.verify().expect("verify bundle"),
        b"immutable evidence"
    );
    assert_eq!(
        fs::metadata(published.root())
            .expect("bundle root metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(published.path())
            .expect("bundle metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let entries = fs::read_dir(published.root())
        .expect("list bundle root")
        .collect::<std::io::Result<Vec<_>>>()
        .expect("read bundle root entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), "provider-evidence.tar");
}

#[test]
fn provider_evidence_bundle_rejects_symlinked_storage_boundary() {
    let td = tempdir().expect("temp state root");
    let outside = tempdir().expect("outside directory");
    fs::create_dir_all(convergence_dir(td.path())).expect("create convergence directory");
    set_mode(&convergence_dir(td.path()), 0o700);
    symlink(
        outside.path(),
        convergence_dir(td.path()).join("provider-bundles"),
    )
    .expect("install malicious symlink");

    let error = store_at(td.path())
        .publish_provider_evidence_bundle(b"immutable evidence")
        .expect_err("symlinked bundle boundary must fail closed");
    assert!(error.to_string().contains("provider-bundles"));
    assert!(
        fs::read_dir(outside.path())
            .expect("list outside directory")
            .next()
            .is_none()
    );
}
