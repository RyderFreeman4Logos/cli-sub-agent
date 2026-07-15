use std::path::PathBuf;

use anyhow::Result;
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource, Sha256Digest,
};

use super::clean_room::{
    AdmittedProviderSessionFactory, CleanRoomWorkspaceFactory, ProviderSessionDriver,
    ProviderSessionFactory, ProviderSessionOutcome, ProviderSessionRequest,
};
use super::clean_room_tests::{epoch, factory};
use crate::pipeline::{ParentSessionSource, SessionCreationMode};

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
