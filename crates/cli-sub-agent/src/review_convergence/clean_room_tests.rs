use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource, EpochRecord, GitObjectId, Sha256Digest,
};

use super::clean_room::{
    AdmittedProviderSessionFactory, CleanRoomWorkspaceFactory, CleanupFailureLedger,
    DetachedWorkspaceDriver, DetachedWorkspacePlan, ExactOidWorkspaceFactory,
    MaterializedWorkspace, ProviderSessionDriver, ProviderSessionFactory, ProviderSessionOutcome,
    ProviderSessionRequest, WorkspaceCleanup,
};
use crate::pipeline::{ParentSessionSource, SessionCreationMode};

fn epoch() -> EpochRecord {
    EpochRecord::new(
        GitObjectId::parse(&"a".repeat(40)).expect("base oid"),
        GitObjectId::parse(&"b".repeat(40)).expect("head oid"),
        Sha256Digest::compute(b"immutable diff"),
    )
}

fn authority(reasoning: [&str; 2]) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("review", "test").expect("source"),
        CommandAuthorityPolicy::new(false, vec!["codex".to_string()], false, true).expect("policy"),
        CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("catalog"),
        vec![
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.4", reasoning[0])
                .expect("strongest identity"),
            AdmittedModelIdentity::new("codex", "openai", "gpt-5.3", reasoning[1])
                .expect("secondary identity"),
        ],
    )
    .expect("authority")
}

#[derive(Clone)]
struct FakeCleanup {
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

struct RecordingWorkspaceDriver {
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

type WorkspaceFactoryFixture = (
    ExactOidWorkspaceFactory<RecordingWorkspaceDriver>,
    Arc<Mutex<Vec<DetachedWorkspacePlan>>>,
    Arc<Mutex<Vec<Duration>>>,
    CleanupFailureLedger,
);

fn factory(observed_head: String, cleanup_fails: bool) -> WorkspaceFactoryFixture {
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
fn provider_request_selects_strongest_xhigh_identity_and_proves_clean_room_policy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
        )
        .expect("guard");

    let request =
        ProviderSessionRequest::from_authority(guard.workspace(), &authority(["xhigh", "high"]))
            .expect("provider request");

    assert_eq!(request.selected_model().model(), "gpt-5.4");
    assert_eq!(request.selected_model().reasoning(), "xhigh");
    assert_eq!(request.cwd(), guard.workspace().root());
    assert!(request.readonly_project_root());
    assert!(request.extra_writable().is_empty());
    assert_eq!(
        request.extra_readable(),
        vec![guard.workspace().bundle_path().to_path_buf()]
    );
    assert_eq!(
        request.parent_session_source(),
        ParentSessionSource::ExplicitOnly
    );
    assert_eq!(
        request.session_creation_mode(),
        SessionCreationMode::FreshChild
    );
    assert!(request.startup_env().to_child_env_vars().is_empty());
    assert!(request.parent().is_none());
    assert!(request.resume_session().is_none());
}

#[test]
fn provider_request_rejects_non_xhigh_strongest_identity() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
        )
        .expect("guard");
    let error =
        ProviderSessionRequest::from_authority(guard.workspace(), &authority(["high", "xhigh"]))
            .expect_err("strongest identity must be xhigh");
    assert!(error.to_string().contains("xhigh"));
}

struct NeverProviderDriver;

impl ProviderSessionDriver for NeverProviderDriver {
    fn run(
        &mut self,
        _admitted: &crate::pipeline::AdmittedExecutor,
        request: &ProviderSessionRequest,
    ) -> Result<ProviderSessionOutcome> {
        panic!(
            "provider driver must not execute in Slice 2 tests: {}",
            request.cwd().display()
        );
    }
}

#[test]
fn provider_adapter_type_is_bound_to_existing_admitted_executor_without_live_execution() {
    fn assert_factory<T: ProviderSessionFactory>() {}
    assert_factory::<AdmittedProviderSessionFactory<'static, NeverProviderDriver>>();
}

struct RecordingProviderFactory {
    calls: Vec<PathBuf>,
}

impl ProviderSessionFactory for RecordingProviderFactory {
    fn run(&mut self, request: &ProviderSessionRequest) -> Result<ProviderSessionOutcome> {
        self.calls.push(request.cwd().to_path_buf());
        Ok(ProviderSessionOutcome::new(
            "01KCLEANROOMSESSION",
            b"provider artifact",
        ))
    }
}

#[test]
fn provider_port_is_mechanically_tested_with_a_fake_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let frozen = epoch();
    let (mut workspace_factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = workspace_factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
        )
        .expect("guard");
    let request =
        ProviderSessionRequest::from_authority(guard.workspace(), &authority(["xhigh", "high"]))
            .expect("request");
    let mut fake = RecordingProviderFactory { calls: Vec::new() };

    let outcome = fake.run(&request).expect("fake provider outcome");

    assert_eq!(fake.calls, vec![guard.workspace().root().to_path_buf()]);
    assert_eq!(outcome.session_id(), "01KCLEANROOMSESSION");
    assert_eq!(outcome.artifact(), b"provider artifact");
    assert_eq!(
        outcome.artifact_digest(),
        &Sha256Digest::compute(b"provider artifact")
    );
    assert_eq!(request.epoch(), guard.workspace().epoch());
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
