use super::*;
use crate::review_cmd::tests::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile, global::GlobalToolConfig};
use csa_core::types::ToolName;

#[cfg(unix)]
#[tokio::test]
async fn execute_review_fails_when_repo_root_output_artifact_is_created() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(
        &fake_opencode,
        "#!/bin/sh\n\
mkdir -p output\n\
printf 'repo-root leak\\n' > output/details.md\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'Review completed successfully.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Structured details from reviewer.' \
'<!-- CSA:SECTION:details:END -->' \
'' \
'PASS'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = project_config_with_enabled_tools(&["opencode"]);
    let global = GlobalConfig::default();
    let err = match execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: repo-root-output-contract-violation".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
    )
    .await
    {
        Ok(_) => panic!("expected review artifact contract violation"),
        Err(err) => err,
    };

    let message = format!("{err:#}");
    assert!(
        message.contains("contract violation"),
        "expected contract violation, got: {message}"
    );
    assert!(
        message.contains("output/details.md"),
        "expected leaked artifact path in error, got: {message}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_error_path_still_checks_artifact_contract() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(
        &fake_opencode,
        "#!/bin/sh\n\
mkdir -p output\n\
printf 'repo-root leak\\n' > output/details.md\n\
printf 'review tool failed\\n' >&2\n\
exit 7\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = project_config_with_enabled_tools(&["opencode"]);
    let global = GlobalConfig::default();
    let err = match execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: tool-error-output-contract-violation".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
    )
    .await
    {
        Ok(_) => panic!("expected review artifact contract violation"),
        Err(err) => err,
    };

    let message = format!("{err:#}");
    assert!(message.contains("contract violation"));
    assert!(
        message.contains("output/details.md"),
        "expected leaked artifact path in error, got: {message}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_retry_path_still_checks_artifact_contract() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    let auth_log = project_dir.path().join("gemini-auth.log");
    std::fs::write(
        &fake_gemini,
        format!(
            "#!/bin/sh\n\
if [ -n \"${{GEMINI_API_KEY:-}}\" ]; then\n\
  printf 'api_key\\n' >> \"{}\"\n\
  mkdir -p output\n\
  printf 'leak\\n' > output/details.md\n\
  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->'\n\
  printf '%s\\n' '<!-- CSA:SECTION:details -->' 'No issues found.' '<!-- CSA:SECTION:details:END -->'\n\
else\n\
  printf 'oauth\\n' >> \"{}\"\n\
  printf 'Opening authentication page\\nDo you want to continue? [Y/n]\\n'\n\
fi\n",
            auth_log.display(),
            auth_log.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_gemini).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_gemini, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = project_config_with_enabled_tools(&["gemini-cli"]);
    let mut global = GlobalConfig::default();
    global.tools.insert(
        "gemini-cli".to_string(),
        GlobalToolConfig {
            api_key: Some("fallback-key".to_string()),
            ..Default::default()
        },
    );

    let err = match execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: gemini-auth-retry-contract-violation".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
    )
    .await
    {
        Ok(_) => panic!("expected review artifact contract violation on retry path"),
        Err(err) => err,
    };

    let message = format!("{err:#}");
    assert!(message.contains("contract violation"));
    assert!(
        message.contains("output/details.md"),
        "expected leaked artifact path in error, got: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(auth_log).unwrap(),
        "oauth\napi_key\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_fails_when_repo_root_findings_artifact_is_created() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir);
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(
        &fake_opencode,
        "#!/bin/sh\n\
printf '{\"findings\":[]}\n' > review-findings.json\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'Review completed successfully.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Structured details from reviewer.' \
'<!-- CSA:SECTION:details:END -->' \
'' \
'PASS'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = project_config_with_enabled_tools(&["opencode"]);
    let global = GlobalConfig::default();
    let err = match execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: repo-root-findings-contract-violation".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,
        false,
        false,
        false,
        false,
        false,
        &[],
    )
    .await
    {
        Ok(_) => panic!("expected review artifact contract violation"),
        Err(err) => err,
    };

    let message = format!("{err:#}");
    assert!(message.contains("contract violation"));
    assert!(
        message.contains("review-findings.json"),
        "expected leaked artifact path in error, got: {message}"
    );
}
