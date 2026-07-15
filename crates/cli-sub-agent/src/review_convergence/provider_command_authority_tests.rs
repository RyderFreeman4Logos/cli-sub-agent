use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use csa_executor::codex_runtime::{CodexRuntimeMetadata, CodexTransport};
use csa_session::convergence::{
    AdmittedModelIdentity, CommandAuthorityCatalogIdentity, CommandAuthorityPolicy,
    CommandAuthoritySnapshot, CommandAuthoritySource,
};

use super::provider_command_authority::{
    AbsoluteProviderProgram, AuditedProviderEnvironment, EnvValueOrigin, ProviderCommandAuthority,
    ProviderEnvironmentInputs, ProviderProgramResolver, SystemProviderProgramResolver,
};
use crate::pipeline::AdmittedExecutor;

fn authority(tool: &str, provider: &str, model: &str) -> CommandAuthoritySnapshot {
    CommandAuthoritySnapshot::new(
        CommandAuthoritySource::tier("review", "test").expect("source"),
        CommandAuthorityPolicy::new(false, vec![tool.to_string()], false, true).expect("policy"),
        CommandAuthorityCatalogIdentity::new("test catalog", "v1").expect("catalog"),
        vec![AdmittedModelIdentity::new(tool, provider, model, "xhigh").expect("identity")],
    )
    .expect("authority")
}

fn environment_inputs(path: &Path) -> ProviderEnvironmentInputs {
    ProviderEnvironmentInputs::new(
        BTreeMap::from([
            ("PATH".to_string(), path.display().to_string()),
            (
                "OPENAI_API_KEY".to_string(),
                "ambient-secret-value".to_string(),
            ),
            ("LANG".to_string(), "C".to_string()),
        ]),
        BTreeMap::from([
            (
                "OPENAI_API_KEY".to_string(),
                "configured-secret-value".to_string(),
            ),
            ("LANG".to_string(), "C.UTF-8".to_string()),
        ]),
    )
}

#[cfg(unix)]
fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, body).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("set executable permissions");
}

#[test]
fn audited_environment_is_fixed_precedence_checked_and_secret_safe() {
    let temp = tempfile::tempdir().expect("tempdir");
    let environment =
        AuditedProviderEnvironment::capture("opencode", "openai", environment_inputs(temp.path()))
            .expect("audited environment");

    assert_eq!(
        environment.origin("OPENAI_API_KEY"),
        Some(EnvValueOrigin::Configured)
    );
    assert_eq!(environment.origin("LANG"), Some(EnvValueOrigin::Configured));
    for derived in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
    ] {
        assert_eq!(environment.origin(derived), Some(EnvValueOrigin::Derived));
    }
    assert!(!environment.contains_key("NODE_OPTIONS"));
    assert!(!environment.contains_key("CSA_GIT_PUSH_ALLOWED"));
    assert!(!environment.provenance_digest().as_str().is_empty());

    let debug = format!("{environment:?}");
    assert!(!debug.contains("ambient-secret-value"));
    assert!(!debug.contains("configured-secret-value"));
    assert!(debug.contains("OPENAI_API_KEY"));
}

#[test]
fn audited_environment_rejects_unknown_prohibited_missing_and_malformed_inputs() {
    let temp = tempfile::tempdir().expect("tempdir");
    for key in [
        "UNKNOWN_SENTINEL",
        "NODE_OPTIONS",
        "LD_PRELOAD",
        "CSA_GIT_PUSH_ALLOWED",
    ] {
        let mut inputs = environment_inputs(temp.path());
        inputs.insert_configured(key, "forbidden");
        let error = AuditedProviderEnvironment::capture("opencode", "openai", inputs)
            .expect_err("unknown or prohibited keys must fail closed");
        assert!(error.to_string().contains(key));
        assert!(!error.to_string().contains("forbidden"));
    }

    let missing_path = ProviderEnvironmentInputs::new(
        BTreeMap::from([("OPENAI_API_KEY".to_string(), "secret".to_string())]),
        BTreeMap::new(),
    );
    assert!(
        AuditedProviderEnvironment::capture("opencode", "openai", missing_path)
            .expect_err("PATH required")
            .to_string()
            .contains("PATH")
    );

    let relative_path = ProviderEnvironmentInputs::new(
        BTreeMap::from([
            ("PATH".to_string(), "relative:/usr/bin".to_string()),
            ("OPENAI_API_KEY".to_string(), "secret".to_string()),
        ]),
        BTreeMap::new(),
    );
    assert!(
        AuditedProviderEnvironment::capture("opencode", "openai", relative_path)
            .expect_err("relative PATH component must fail")
            .to_string()
            .contains("absolute")
    );

    let missing_credential = ProviderEnvironmentInputs::new(
        BTreeMap::from([("PATH".to_string(), temp.path().display().to_string())]),
        BTreeMap::new(),
    );
    assert!(
        AuditedProviderEnvironment::capture("opencode", "openai", missing_credential)
            .expect_err("provider credential required")
            .to_string()
            .contains("OPENAI_API_KEY")
    );

    let error = AuditedProviderEnvironment::capture(
        "opencode",
        "custom-provider",
        environment_inputs(temp.path()),
    )
    .expect_err("custom provider matrix must fail closed");
    assert!(error.to_string().contains("unsupported"));
}

struct FixedResolver {
    expected_runtime: &'static str,
    path: PathBuf,
}

impl ProviderProgramResolver for FixedResolver {
    fn resolve(
        &self,
        _expected_runtime: &str,
        _audited_path: &str,
        _capture_cwd: &Path,
    ) -> anyhow::Result<AbsoluteProviderProgram> {
        AbsoluteProviderProgram::capture(self.expected_runtime, &self.path)
    }
}

#[cfg(unix)]
#[test]
fn provider_authority_binds_actual_admission_and_fingerprinted_absolute_program() {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("opencode");
    write_executable(&binary, "#!/bin/sh\nexit 0\n");
    let admitted = AdmittedExecutor::from_model_spec_for_test("opencode/openai/gpt-5.4/xhigh")
        .expect("admitted executor");
    let snapshot = authority("opencode", "openai", "gpt-5.4");

    let captured = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        environment_inputs(temp.path()),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("provider command authority");

    assert_eq!(
        captured.selected_identity(),
        snapshot.ordered_admitted().first().unwrap()
    );
    assert_eq!(captured.authority_digest(), &snapshot.digest());
    assert_eq!(captured.runtime_binary(), "opencode");
    assert_eq!(captured.program_path(), binary.as_path());
    assert_eq!(captured.tool().as_str(), "opencode");
    captured
        .verify_program_integrity()
        .expect("unchanged program fingerprint");

    let debug = format!("{captured:?}");
    assert!(!debug.contains("configured-secret-value"));
}

#[cfg(unix)]
#[test]
fn provider_program_capture_rejects_non_executable_basename_and_runtime_mismatch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let non_executable = temp.path().join("opencode");
    fs::write(&non_executable, "not executable").expect("write fixture");
    assert!(
        AbsoluteProviderProgram::capture("opencode", &non_executable)
            .expect_err("non-executable must fail")
            .to_string()
            .contains("executable")
    );

    let wrong_name = temp.path().join("wrong-name");
    write_executable(&wrong_name, "#!/bin/sh\nexit 0\n");
    assert!(
        AbsoluteProviderProgram::capture("opencode", &wrong_name)
            .expect_err("basename mismatch must fail")
            .to_string()
            .contains("basename")
    );

    let codex = temp.path().join("codex");
    write_executable(&codex, "#!/bin/sh\nexit 0\n");
    let admitted = AdmittedExecutor::from_model_spec_for_test("opencode/openai/gpt-5.4/xhigh")
        .expect("admitted executor");
    let resolver = FixedResolver {
        expected_runtime: "codex",
        path: codex,
    };
    let error = ProviderCommandAuthority::capture(
        &admitted,
        &authority("opencode", "openai", "gpt-5.4"),
        environment_inputs(temp.path()),
        temp.path(),
        &resolver,
    )
    .expect_err("runtime mismatch must fail");
    assert!(error.to_string().contains("runtime"));
}

#[cfg(unix)]
#[test]
fn provider_program_revalidation_rejects_replacement_and_symlink_target_drift() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("tempdir");
    let admitted = AdmittedExecutor::from_model_spec_for_test("opencode/openai/gpt-5.4/xhigh")
        .expect("admitted executor");
    let snapshot = authority("opencode", "openai", "gpt-5.4");

    let direct_dir = temp.path().join("direct");
    fs::create_dir(&direct_dir).expect("direct dir");
    let direct = direct_dir.join("opencode");
    write_executable(&direct, "#!/bin/sh\nprintf old\n");
    let direct_authority = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        environment_inputs(&direct_dir),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("direct authority");
    write_executable(&direct, "#!/bin/sh\nprintf new\n");
    assert!(
        direct_authority
            .verify_program_integrity()
            .expect_err("content replacement must fail")
            .to_string()
            .contains("fingerprint")
    );

    let symlink_dir = temp.path().join("symlinked");
    fs::create_dir(&symlink_dir).expect("symlink dir");
    let first = temp.path().join("provider-one");
    let second = temp.path().join("provider-two");
    write_executable(&first, "#!/bin/sh\nprintf one\n");
    write_executable(&second, "#!/bin/sh\nprintf two\n");
    let link = symlink_dir.join("opencode");
    symlink(&first, &link).expect("initial symlink");
    let symlink_authority = ProviderCommandAuthority::capture(
        &admitted,
        &snapshot,
        environment_inputs(&symlink_dir),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("symlink authority");
    fs::remove_file(&link).expect("remove old symlink");
    symlink(&second, &link).expect("replacement symlink");
    assert!(
        symlink_authority
            .verify_program_integrity()
            .expect_err("symlink target drift must fail")
            .to_string()
            .contains("target")
    );
}

#[cfg(unix)]
#[test]
fn provider_authority_accepts_codex_cli_and_rejects_acp_or_tmux() {
    let temp = tempfile::tempdir().expect("tempdir");
    let binary = temp.path().join("codex");
    write_executable(&binary, "#!/bin/sh\nexit 0\n");
    let snapshot = authority("codex", "openai", "gpt-5.4");
    let cli = AdmittedExecutor::from_codex_model_spec_for_test(
        "codex/openai/gpt-5.4/xhigh",
        CodexRuntimeMetadata::from_transport(CodexTransport::Cli),
    )
    .expect("direct Codex executor");

    let captured = ProviderCommandAuthority::capture(
        &cli,
        &snapshot,
        environment_inputs(temp.path()),
        temp.path(),
        &SystemProviderProgramResolver,
    )
    .expect("direct Codex authority");
    assert_eq!(captured.runtime_binary(), "codex");

    for metadata in [
        CodexRuntimeMetadata::from_transport(CodexTransport::Acp),
        CodexRuntimeMetadata::from_transport(CodexTransport::Cli).with_tmux_mode(true),
    ] {
        let rejected = AdmittedExecutor::from_codex_model_spec_for_test(
            "codex/openai/gpt-5.4/xhigh",
            metadata,
        )
        .expect("Codex executor");
        let error = ProviderCommandAuthority::capture(
            &rejected,
            &snapshot,
            ProviderEnvironmentInputs::new(BTreeMap::new(), BTreeMap::new()),
            temp.path(),
            &SystemProviderProgramResolver,
        )
        .expect_err("non-direct Codex authority must fail");
        assert!(error.to_string().contains("direct CLI") || error.to_string().contains("tmux"));
    }
}
