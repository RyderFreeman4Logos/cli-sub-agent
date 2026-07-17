use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use anyhow::Result;
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource, Sha256Digest,
};

use super::clean_room::{
    CleanRoomWorkspaceFactory, ProductionCleanRoomProvider, ProviderSessionFactory,
    ProviderSessionFuture, ProviderSessionOutcome, ProviderSessionRequest,
};
use super::clean_room_provider::{
    AdmittedProviderSessionFactory, ExactProviderPrompt, ProviderSessionDriver,
};
use super::clean_room_tests::{epoch, factory, lease_context};
use super::provider_command_authority::{
    ProviderCommandAuthority, ProviderEnvironmentInputs, SystemProviderProgramResolver,
};
use crate::pipeline::{ParentSessionSource, SessionCreationMode};
use crate::run_resource_overrides::RunResourceOverrides;

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

fn opencode_authority(source_key: &str, model: &str) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("review", source_key).expect("source"),
        CommandAuthorityPolicy::new(false, vec!["opencode".to_string()], false, true)
            .expect("policy"),
        CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("catalog"),
        vec![AdmittedModelIdentity::new("opencode", "openai", model, "xhigh").expect("identity")],
    )
    .expect("authority")
}

fn clean_limits() -> crate::pipeline::CleanRoomExecutionLimits {
    crate::pipeline::CleanRoomExecutionLimits::try_new(
        30,
        Some(10),
        Some(Duration::from_secs(30)),
        RunResourceOverrides::absent(),
        Some("quality".to_string()),
    )
    .expect("clean limits")
}

fn low_resource_config() -> csa_config::ProjectConfig {
    toml::from_str(
        r#"
[resources]
min_free_memory_mb = 1
idle_timeout_seconds = 30
initial_response_timeout_seconds = 10
"#,
    )
    .expect("clean-room test config")
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make executable");
}

#[test]
fn provider_request_preserves_exact_prompt_and_evidence_bundle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lease_context = lease_context(temp.path());
    let frozen = epoch();
    let (mut factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
            &lease_context,
        )
        .expect("guard");

    let prompt = ExactProviderPrompt::new(
        "\u{feff}fresh café review\r\n<prior-finding>literal fixture</prior-finding>  ",
    );
    let request = ProviderSessionRequest::from_authority(
        guard.workspace(),
        &authority(["xhigh", "high"]),
        prompt.clone(),
    )
    .expect("provider request");

    assert_eq!(request.selected_model().model(), "gpt-5.4");
    assert_eq!(request.selected_model().reasoning(), "xhigh");
    assert_eq!(request.cwd(), guard.workspace().root());
    assert_eq!(request.exact_prompt().as_bytes(), prompt.as_bytes());
    assert_eq!(request.evidence_bundle(), guard.workspace().bundle_path());
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
    let lease_context = lease_context(temp.path());
    let frozen = epoch();
    let (mut factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
            &lease_context,
        )
        .expect("guard");
    let error = ProviderSessionRequest::from_authority(
        guard.workspace(),
        &authority(["high", "xhigh"]),
        ExactProviderPrompt::new("strict prompt"),
    )
    .expect_err("strongest identity must be xhigh");
    assert!(error.to_string().contains("xhigh"));
}

struct NeverProviderDriver;

impl ProviderSessionDriver for NeverProviderDriver {
    fn run<'a>(
        &'a mut self,
        _admitted: &'a crate::pipeline::AdmittedExecutor,
        request: &'a ProviderSessionRequest,
    ) -> ProviderSessionFuture<'a> {
        Box::pin(async move {
            panic!(
                "provider driver must not execute in Slice 2 tests: {}",
                request.cwd().display()
            );
        })
    }
}

#[test]
fn provider_adapter_type_is_bound_to_existing_admitted_executor_without_live_execution() {
    fn assert_factory<T: ProviderSessionFactory>() {}
    assert_factory::<AdmittedProviderSessionFactory<'static, NeverProviderDriver>>();
}

struct RecordingProviderFactory {
    calls: Vec<PathBuf>,
    failure: Option<&'static str>,
}

impl ProviderSessionFactory for RecordingProviderFactory {
    fn run<'a>(
        &'a mut self,
        request: &'a ProviderSessionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderSessionOutcome>> + 'a>> {
        Box::pin(async move {
            self.calls.push(request.cwd().to_path_buf());
            if let Some(message) = self.failure {
                anyhow::bail!(message);
            }
            Ok(ProviderSessionOutcome::new(
                "01KCLEANROOMSESSION",
                b"provider artifact",
            ))
        })
    }
}

#[tokio::test]
async fn async_provider_port_propagates_success_and_error_with_a_fake_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lease_context = lease_context(temp.path());
    let frozen = epoch();
    let (mut workspace_factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let guard = workspace_factory
        .create(
            &temp.path().join("source"),
            &temp.path().join("room"),
            &temp.path().join("bundle"),
            frozen,
            &lease_context,
        )
        .expect("guard");
    let request = ProviderSessionRequest::from_authority(
        guard.workspace(),
        &authority(["xhigh", "high"]),
        ExactProviderPrompt::new("exact async prompt"),
    )
    .expect("request");
    let mut fake = RecordingProviderFactory {
        calls: Vec::new(),
        failure: None,
    };

    let outcome = fake.run(&request).await.expect("fake provider outcome");

    assert_eq!(fake.calls, vec![guard.workspace().root().to_path_buf()]);
    assert_eq!(outcome.session_id(), "01KCLEANROOMSESSION");
    assert_eq!(outcome.artifact(), b"provider artifact");
    assert_eq!(
        outcome.artifact_digest(),
        &Sha256Digest::compute(b"provider artifact")
    );
    assert_eq!(request.epoch(), guard.workspace().epoch());

    fake.failure = Some("fake provider failure");
    let error = fake
        .run(&request)
        .await
        .expect_err("async fake error must propagate");
    assert_eq!(error.to_string(), "fake provider failure");
}

#[cfg(unix)]
#[tokio::test]
async fn production_adapter_executes_only_fingerprinted_fake_with_exact_contract() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lease_context = lease_context(temp.path());
    let mut sandbox = crate::test_session_sandbox::ScopedSessionSandbox::new(&temp).await;
    sandbox.track_env("CSA_CLEAN_ROOM_PARENT_SENTINEL");
    unsafe {
        std::env::set_var("CSA_CLEAN_ROOM_PARENT_SENTINEL", "must-not-leak");
    }

    let frozen = epoch();
    let (mut workspace_factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let root = temp.path().join("clean-room");
    let bundle = temp.path().join("evidence.md");
    let guard = workspace_factory
        .create(
            &temp.path().join("source"),
            &root,
            &bundle,
            frozen,
            &lease_context,
        )
        .expect("guard");
    fs::create_dir_all(&root).expect("clean-room root");
    fs::write(&bundle, "frozen-evidence-marker\n").expect("evidence");
    let program = root.join("opencode");
    let script = format!(
        r#"#!/bin/sh
set -eu
[ "${{CSA_CLEAN_ROOM_PARENT_SENTINEL+x}}" != x ]
[ "$LANG" = "configured-provider-sentinel" ]
[ "$OPENAI_API_KEY" = "configured-provider-secret" ]
[ "$HOME" = "/tmp/csa-clean-room/home" ]
[ "$(pwd)" = "{}" ]
IFS= read -r evidence < "{}"
[ "$evidence" = "frozen-evidence-marker" ]
last=
for arg in "$@"; do last=$arg; done
printf '%s' "$last"
"#,
        root.display(),
        bundle.display(),
    );
    write_executable(&program, &script);

    let admitted = crate::pipeline::AdmittedExecutor::from_model_spec_for_test(
        "opencode/openai/gpt-5.4/xhigh",
    )
    .expect("admitted executor");
    let snapshot = opencode_authority("production-adapter", "gpt-5.4");
    let command_authority = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        ProviderEnvironmentInputs::new(
            BTreeMap::from([
                (
                    "PATH".to_string(),
                    format!("{}:/usr/bin:/bin", root.display()),
                ),
                (
                    "OPENAI_API_KEY".to_string(),
                    "ambient-provider-secret".to_string(),
                ),
            ]),
            BTreeMap::from([
                (
                    "OPENAI_API_KEY".to_string(),
                    "configured-provider-secret".to_string(),
                ),
                (
                    "LANG".to_string(),
                    "configured-provider-sentinel".to_string(),
                ),
            ]),
        ),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("provider authority");
    let prompt = ExactProviderPrompt::new(
        "\u{feff}exact production café\r\n<prior-finding>literal</prior-finding>\r\n  ",
    );
    let request =
        ProviderSessionRequest::from_authority(guard.workspace(), &snapshot, prompt.clone())
            .expect("request");
    let config = low_resource_config();
    let global = csa_config::GlobalConfig::default();
    let mut provider = ProductionCleanRoomProvider::new(
        &admitted,
        &command_authority,
        Some(&config),
        Some(&global),
        clean_limits(),
    )
    .expect("production provider");

    let outcome = provider.run(&request).await.expect("provider execution");

    assert_eq!(outcome.artifact(), prompt.as_bytes());
    assert!(!outcome.session_id().is_empty());
    assert_eq!(
        outcome.artifact_digest(),
        &Sha256Digest::compute(prompt.as_bytes())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn production_adapter_rejects_stale_executor_request_and_program_before_execution() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lease_context = lease_context(temp.path());
    let frozen = epoch();
    let (mut workspace_factory, _plans, _cleanup_calls, _ledger) =
        factory(frozen.head_oid().as_str().to_string(), false);
    let root = temp.path().join("clean-room");
    let bundle = temp.path().join("evidence.md");
    let guard = workspace_factory
        .create(
            &temp.path().join("source"),
            &root,
            &bundle,
            frozen,
            &lease_context,
        )
        .expect("guard");
    fs::create_dir_all(&root).expect("clean-room root");
    fs::write(&bundle, "evidence\n").expect("evidence");
    let program = root.join("opencode");
    let marker = root.join("must-not-run");
    write_executable(
        &program,
        &format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
    );
    let admitted = crate::pipeline::AdmittedExecutor::from_model_spec_for_test(
        "opencode/openai/gpt-5.4/xhigh",
    )
    .expect("admitted executor");
    let stale_admitted = crate::pipeline::AdmittedExecutor::from_model_spec_for_test(
        "opencode/openai/gpt-5.5/xhigh",
    )
    .expect("stale admitted executor");
    let snapshot = opencode_authority("captured", "gpt-5.4");
    let command_authority = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        ProviderEnvironmentInputs::new(
            BTreeMap::from([
                (
                    "PATH".to_string(),
                    format!("{}:/usr/bin:/bin", root.display()),
                ),
                ("OPENAI_API_KEY".to_string(), "provider-secret".to_string()),
            ]),
            BTreeMap::new(),
        ),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("provider authority");
    let config = low_resource_config();
    let global = csa_config::GlobalConfig::default();

    let stale_executor_error = ProductionCleanRoomProvider::new(
        &stale_admitted,
        &command_authority,
        Some(&config),
        Some(&global),
        clean_limits(),
    )
    .err()
    .expect("stale executor must fail");
    assert!(stale_executor_error.to_string().contains("admitted"));

    let stale_snapshot = opencode_authority("stale-request", "gpt-5.4");
    let stale_request = ProviderSessionRequest::from_authority(
        guard.workspace(),
        &stale_snapshot,
        ExactProviderPrompt::new("must not execute"),
    )
    .expect("stale request");
    let mut provider = ProductionCleanRoomProvider::new(
        &admitted,
        &command_authority,
        Some(&config),
        Some(&global),
        clean_limits(),
    )
    .expect("provider");
    assert!(
        provider
            .run(&stale_request)
            .await
            .expect_err("stale request must fail")
            .to_string()
            .contains("digest")
    );

    write_executable(&program, "#!/bin/sh\nexit 99\n");
    let request = ProviderSessionRequest::from_authority(
        guard.workspace(),
        &snapshot,
        ExactProviderPrompt::new("must not execute"),
    )
    .expect("request");
    assert!(
        provider
            .run(&request)
            .await
            .expect_err("replaced program must fail")
            .to_string()
            .contains("fingerprint")
    );

    let recaptured = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        ProviderEnvironmentInputs::new(
            BTreeMap::from([
                (
                    "PATH".to_string(),
                    format!("{}:/usr/bin:/bin", root.display()),
                ),
                ("OPENAI_API_KEY".to_string(), "provider-secret".to_string()),
            ]),
            BTreeMap::new(),
        ),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("recaptured provider authority");
    let mut failing_provider = ProductionCleanRoomProvider::new(
        &admitted,
        &recaptured,
        Some(&config),
        Some(&global),
        clean_limits(),
    )
    .expect("failing provider");
    assert!(
        failing_provider
            .run(&request)
            .await
            .expect_err("nonzero fake provider exit must propagate")
            .to_string()
            .contains("status 99")
    );
    assert!(!marker.exists());
}
