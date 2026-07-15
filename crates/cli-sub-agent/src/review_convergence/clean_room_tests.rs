use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::convergence::{EpochRecord, GitObjectId, Sha256Digest};

use super::clean_room::{
    CleanRoomWorkspaceFactory, CleanupFailureLedger, DetachedWorkspaceDriver,
    DetachedWorkspacePlan, ExactOidWorkspaceFactory, MaterializedWorkspace, WorkspaceCleanup,
};

pub(super) fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("base oid"),
        GitObjectId::parse(&"b".repeat(40)).expect("head oid"),
        Sha256Digest::compute(b"immutable diff"),
    )
}

#[derive(Clone)]
pub(super) struct FakeCleanup {
    calls: Arc<Mutex<Vec<Duration>>>,
    fail: bool,
}

impl WorkspaceCleanup for FakeCleanup {
    fn cleanup(&mut self, timeout: Duration) -> Result<()> {
        self.calls.lock().expect("cleanup calls").push(timeout);
        if self.fail {
            return Err(anyhow!("injected cleanup failure"));
        }
        Ok(())
    }
}

pub(super) struct RecordingWorkspaceDriver {
    plans: Arc<Mutex<Vec<DetachedWorkspacePlan>>>,
    cleanup_calls: Arc<Mutex<Vec<Duration>>>,
    observed_head: String,
    cleanup_fails: bool,
}

impl DetachedWorkspaceDriver for RecordingWorkspaceDriver {
    type Cleanup = FakeCleanup;

    fn materialize(
        &mut self,
        plan: &DetachedWorkspacePlan,
    ) -> Result<MaterializedWorkspace<Self::Cleanup>> {
        self.plans.lock().expect("plans").push(plan.clone());
        Ok(MaterializedWorkspace::new(
            self.observed_head.clone(),
            FakeCleanup {
                calls: Arc::clone(&self.cleanup_calls),
                fail: self.cleanup_fails,
            },
        ))
    }
}

pub(super) type WorkspaceFactoryFixture = (
    ExactOidWorkspaceFactory<RecordingWorkspaceDriver>,
    Arc<Mutex<Vec<DetachedWorkspacePlan>>>,
    Arc<Mutex<Vec<Duration>>>,
    CleanupFailureLedger,
);

pub(super) fn factory(observed_head: String, cleanup_fails: bool) -> WorkspaceFactoryFixture {
    let plans = Arc::new(Mutex::new(Vec::new()));
    let cleanup_calls = Arc::new(Mutex::new(Vec::new()));
    let ledger = CleanupFailureLedger::default();
    (
        ExactOidWorkspaceFactory::new(
            RecordingWorkspaceDriver {
                plans: Arc::clone(&plans),
                cleanup_calls: Arc::clone(&cleanup_calls),
                observed_head,
                cleanup_fails,
            },
            Duration::from_secs(7),
            ledger.clone(),
        ),
        plans,
        cleanup_calls,
        ledger,
    )
}

#[test]
fn exact_oid_factory_builds_detached_non_interactive_git_plan_without_executing_git() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source");
    let root = temp.path().join("clean-room");
    let bundle = temp.path().join("provider-evidence.tar");
    let frozen = epoch();
    let (mut factory, plans, cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);

    let guard = factory
        .create(&source, &root, &bundle, frozen.clone())
        .expect("create through recording driver");

    let plans = plans.lock().expect("plans");
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert_eq!(plan.create().program(), "git");
    assert_eq!(
        plan.create().args(),
        vec![
            "-c",
            "advice.detachedHead=false",
            "-C",
            source.to_str().unwrap(),
            "worktree",
            "add",
            "--detach",
            root.to_str().unwrap(),
            frozen.head_oid().as_str(),
        ]
    );
    assert_eq!(
        plan.create()
            .env()
            .get("GIT_TERMINAL_PROMPT")
            .map(String::as_str),
        Some("0")
    );
    assert_eq!(
        plan.create()
            .env()
            .get("GIT_CONFIG_NOSYSTEM")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(plan.cleanup().program(), "git");
    assert_eq!(plan.cleanup().env(), plan.create().env());
    assert_eq!(
        plan.cleanup().args(),
        vec![
            "-C",
            source.to_str().unwrap(),
            "worktree",
            "remove",
            "--force",
            root.to_str().unwrap(),
        ]
    );
    assert_eq!(guard.workspace().root(), root);
    assert_eq!(guard.workspace().bundle_path(), bundle);
    assert_eq!(guard.workspace().epoch(), &frozen);
    drop(plans);
    drop(guard);
    assert_eq!(
        cleanup_calls.lock().expect("cleanup").as_slice(),
        &[Duration::from_secs(7)]
    );
}

#[test]
fn workspace_guard_requests_bounded_cleanup_on_drop() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut factory, _plans, cleanup_calls, ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    {
        let _guard = factory
            .create(
                &temp.path().join("source"),
                &temp.path().join("room"),
                &temp.path().join("bundle"),
                frozen,
            )
            .expect("guard");
    }
    assert_eq!(cleanup_calls.lock().expect("cleanup").len(), 1);
    assert!(ledger.failures().is_empty());
}

#[test]
fn explicit_close_surfaces_cleanup_failure_and_drop_records_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut factory, _plans, cleanup_calls, ledger) =
        factory(frozen.head_oid().as_str().to_string(), true);
    let guard = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen.clone(),
        )
        .expect("guard");
    let error = guard
        .close()
        .expect_err("explicit cleanup must surface failure");
    assert!(
        format!("{error:#}").contains("injected cleanup failure"),
        "{error:#}"
    );

    let _dropped = factory
        .create(
            &temp.path().join("source-2"),
            &temp.path().join("room-2"),
            &temp.path().join("bundle-2"),
            frozen,
        )
        .expect("second guard");
    drop(_dropped);
    assert_eq!(cleanup_calls.lock().expect("cleanup").len(), 2);
    assert_eq!(ledger.failures().len(), 1);
}

#[test]
fn observed_head_mismatch_fails_closed_and_cleans_partial_workspace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut factory, _plans, cleanup_calls, _ledger) = factory("c".repeat(40), false);
    let error = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
        )
        .expect_err("mismatched materialization must fail");
    assert!(error.to_string().contains("exact frozen head"));
    assert_eq!(cleanup_calls.lock().expect("cleanup").len(), 1);
}

#[test]
fn workspace_factory_rejects_relative_boundaries_before_driver_invocation() {
    let frozen = epoch();
    let (mut factory, plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let error = factory
        .create(
            Path::new("relative-source"),
            Path::new("relative-room"),
            Path::new("relative-bundle"),
            frozen,
        )
        .expect_err("relative boundaries must fail closed");
    assert!(error.to_string().contains("absolute"));
    assert!(plans.lock().expect("plans").is_empty());
}
