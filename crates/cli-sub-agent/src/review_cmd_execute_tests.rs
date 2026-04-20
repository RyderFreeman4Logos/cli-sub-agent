use super::*;
use crate::review_cmd::tests::{
    ScopedEnvVarRestore, project_config_with_enabled_tools, setup_git_repo,
};
use crate::session_cmds_result::{StructuredOutputOpts, handle_session_result};
use crate::test_session_sandbox::ScopedSessionSandbox;
use csa_config::{GlobalConfig, ProjectProfile, ToolRestrictions, global::GlobalToolConfig};
use csa_core::types::ToolName;
use csa_executor::PeakMemoryContext;
use std::path::Path;

#[cfg(unix)]
#[tokio::test]
async fn execute_review_reclassifies_complete_review_after_edit_restriction() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(
        &fake_opencode,
        "#!/bin/sh\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'Review completed successfully.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Detailed review body.' \
'<!-- CSA:SECTION:details:END -->' \
'' \
'PASS'\n\
printf 'tool mutation\\n' >> tracked.txt\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_opencode).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_opencode, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let mut config = project_config_with_enabled_tools(&["opencode"]);
    config.tools.get_mut("opencode").unwrap().restrictions = Some(ToolRestrictions {
        allow_edit_existing_files: false,
        allow_write_new_files: false,
    });

    let global = GlobalConfig::default();
    let result = execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None, // tier_model_spec
        None, // tier_name
        None, // thinking
        "review: edit-restriction-regression".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,  // initial_response_timeout_seconds
        false, // force_override_user_config
        false, // force_ignore_tier_setting
        false, // no_failover
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    )
    .await
    .expect("review should succeed after reclassifying edit restriction");

    assert_eq!(result.execution.execution.exit_code, 0);
    assert_eq!(
        result.execution.execution.summary,
        "Review completed successfully."
    );

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .unwrap()
        .expect("result.toml");
    assert_eq!(persisted.status, "success");
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.summary, "Review completed successfully.");

    let session =
        csa_session::load_session(project_dir.path(), &result.execution.meta_session_id).unwrap();
    let tool_state = session.tools.get("opencode").expect("opencode tool state");
    assert_eq!(tool_state.last_exit_code, 0);
    assert_eq!(
        tool_state.last_action_summary,
        "Review completed successfully."
    );

    assert_eq!(
        std::fs::read_to_string(project_dir.path().join("tracked.txt")).unwrap(),
        "baseline\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_preserves_codex_default_target_when_project_target_exists() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let target_mount = project_dir.path().join("ssd-target");
    std::fs::create_dir_all(&target_mount).unwrap();
    symlink(&target_mount, project_dir.path().join("target")).unwrap();

    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_codex = bin_dir.join("codex");
    let cargo_target_log = project_dir.path().join("cargo-target-dir.log");
    std::fs::write(
        &fake_codex,
        format!(
            "#!/bin/sh\n\
printf '%s' \"${{CARGO_TARGET_DIR:-}}\" > \"{}\"\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'PASS' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Review used default cargo target behavior.' \
'<!-- CSA:SECTION:details:END -->'\n",
            cargo_target_log.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_codex).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_codex, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);
    let _bwrap_preflight_guard = ScopedEnvVarRestore::set("CSA_SKIP_BWRAP_PREFLIGHT", "1");

    let config = project_config_with_enabled_tools(&["codex"]);
    let global = GlobalConfig::default();
    let result = execute_review(
        ToolName::Codex,
        "scope=range:main...HEAD mode=review-only security=auto".to_string(),
        None,
        None,
        Some("codex/openai/gpt-5.4/medium".to_string()),
        None,
        None,
        "review: codex-target-default".to_string(),
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
        true,
        true,
        false,
        false,
        &[],
        &[],
    )
    .await
    .expect("codex review should honor project target");

    assert_eq!(result.execution.execution.exit_code, 0);
    let session_dir =
        csa_session::get_session_dir(project_dir.path(), &result.execution.meta_session_id)
            .unwrap();
    let observed_target_dir = std::fs::read_to_string(cargo_target_log).unwrap();
    assert_ne!(
        observed_target_dir,
        session_dir.join("target").display().to_string(),
        "review dispatch must not override CARGO_TARGET_DIR to the session-local target dir"
    );
    assert!(
        !session_dir.join("target").exists(),
        "review session should not create a session-local target dir"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_model_spec_bypasses_tier_enforcement_without_active_tier() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_opencode = bin_dir.join("opencode");
    std::fs::write(
        &fake_opencode,
        "#!/bin/sh\n\
printf '%s\\n' \
'<!-- CSA:SECTION:summary -->' \
'Explicit model spec review succeeded.' \
'<!-- CSA:SECTION:summary:END -->' \
'' \
'<!-- CSA:SECTION:details -->' \
'Explicit model spec bypassed tier enforcement.' \
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

    let mut config = project_config_with_enabled_tools(&["opencode", "gemini-cli"]);
    config.tiers.insert(
        "quality".to_string(),
        csa_config::config::TierConfig {
            description: "Test tier".to_string(),
            models: vec!["gemini-cli/google/default/xhigh".to_string()],
            strategy: csa_config::TierStrategy::default(),
            token_budget: None,
            max_turns: None,
        },
    );

    let global = GlobalConfig::default();
    let result = execute_review(
        ToolName::Opencode,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        Some("opencode/provider/model/medium".to_string()),
        None, // tier_name
        None, // thinking
        "review: model-spec-bypasses-tier-regression".to_string(),
        project_dir.path(),
        Some(&config),
        &global,
        ReviewRoutingMetadata {
            project_profile: ProjectProfile::Unknown,
            detection_method: "auto",
        },
        csa_process::StreamMode::BufferOnly,
        crate::pipeline::DEFAULT_IDLE_TIMEOUT_SECONDS,
        None,  // initial_response_timeout_seconds
        false, // force_override_user_config
        false, // force_ignore_tier_setting
        false, // no_failover
        false, // no_fs_sandbox
        false, // readonly_project_root
        &[],   // extra_writable
        &[],   // extra_readable
    )
    .await
    .expect("explicit review model spec should bypass tier enforcement");

    assert_eq!(result.execution.execution.exit_code, 0);
    assert!(
        result
            .execution
            .execution
            .output
            .contains("Explicit model spec review succeeded."),
        "expected structured review output, got: {}",
        result.execution.execution.output
    );
}

#[test]
fn synthesize_missing_review_result_makes_session_result_readable() {
    let td = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project_root = td.path();

    let session =
        csa_session::create_session(project_root, Some("review-failure"), None, Some("codex"))
            .expect("session creation");
    let session_id = session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(project_root, &session_id).unwrap();
    std::fs::write(
        session_dir.join("stderr.log"),
        "codex daemon fork-from failed: provider session bootstrap failed",
    )
    .unwrap();

    let started_at = Utc::now();
    let err = anyhow::anyhow!("codex daemon fork-from failed")
        .context(format!("meta_session_id={session_id}"));

    maybe_synthesize_missing_review_result(project_root, ToolName::Codex, started_at, &err);

    let result = csa_session::load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("synthetic result.toml should exist");
    assert_eq!(result.status, "failure");
    assert_eq!(result.exit_code, 1);
    assert!(
        result.summary.contains("spawn_fail"),
        "expected classified summary, got: {}",
        result.summary
    );

    handle_session_result(
        session.meta_session_id,
        false,
        Some(project_root.to_string_lossy().into_owned()),
        StructuredOutputOpts::default(),
    )
    .expect("session result should read the synthetic review failure");
}

#[test]
fn read_review_failure_excerpt_ignores_bytes_beyond_4kb() {
    let td = tempfile::tempdir().unwrap();
    let session_dir = td.path();
    std::fs::write(
        session_dir.join("stderr.log"),
        format!("{}late failure marker", " ".repeat(4096)),
    )
    .unwrap();

    assert_eq!(read_review_failure_excerpt(session_dir), None);
}

#[test]
fn synthesize_missing_review_result_propagates_peak_memory_from_error_chain() {
    let td = tempfile::tempdir().unwrap();
    let _sandbox = ScopedSessionSandbox::new_blocking(&td);
    let project_root = td.path();

    let session =
        csa_session::create_session(project_root, Some("review-failure"), None, Some("codex"))
            .expect("session creation");
    let session_id = session.meta_session_id.clone();
    let session_dir = csa_session::get_session_dir(project_root, &session_id).unwrap();
    std::fs::write(session_dir.join("stderr.log"), "tool terminated by signal").unwrap();

    let started_at = Utc::now();
    let err = PeakMemoryContext(Some(512))
        .into_anyhow("codex process exited")
        .context(format!("meta_session_id={session_id}"));

    maybe_synthesize_missing_review_result(project_root, ToolName::Codex, started_at, &err);

    let result = csa_session::load_result(project_root, &session.meta_session_id)
        .unwrap()
        .expect("synthetic result.toml should exist");
    assert_eq!(result.peak_memory_mb, Some(512));
}

#[test]
fn extract_meta_session_id_from_error_accepts_hyphen_and_underscore() {
    let err = anyhow::anyhow!("review failed").context("meta_session_id=session-1_abc");
    assert_eq!(
        extract_meta_session_id_from_error(&err).as_deref(),
        Some("session-1_abc")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_retries_gemini_with_api_key_after_oauth_prompt() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    let auth_log = project_dir.path().join("gemini-auth.log");
    std::fs::write(
            &fake_gemini,
            format!(
                "#!/bin/sh\n\
if [ \"$1\" = \"--version\" ]; then\n\
  printf 'gemini-cli 1.0.0\\n'\n\
  exit 0\n\
fi\n\
if [ -n \"${{GEMINI_API_KEY:-}}\" ]; then\n\
  printf 'api_key\\n' >> \"{}\"\n\
  printf '%s\\n' '<!-- CSA:SECTION:summary -->' 'PASS' '<!-- CSA:SECTION:summary:END -->'\n\
  printf '%s\\n' '<!-- CSA:SECTION:details -->' 'No issues found.' '<!-- CSA:SECTION:details:END -->'\n\
  printf 'output_tokens: 3\\n'\n\
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

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: gemini-auth-retry".to_string(),
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
        &[],
    )
    .await
    .expect("gemini auth retry should succeed");

    assert!(result.status_reason.is_none());
    assert_eq!(result.execution.execution.exit_code, 0);
    assert!(result.execution.execution.output.contains("PASS"));
    assert_eq!(
        std::fs::read_to_string(auth_log).unwrap(),
        "oauth\napi_key\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_classifies_gemini_oauth_prompt_without_api_key() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    std::fs::write(
        &fake_gemini,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'gemini-cli 1.0.0\\n'\n  exit 0\nfi\nprintf 'Opening authentication page\\nDo you want to continue? [Y/n]\\n'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&fake_gemini).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&fake_gemini, perms).unwrap();

    let inherited_path = std::env::var("PATH").unwrap_or_default();
    let patched_path = format!("{}:{inherited_path}", bin_dir.display());
    let _path_guard = ScopedEnvVarRestore::set("PATH", &patched_path);

    let config = project_config_with_enabled_tools(&["gemini-cli"]);
    let global = GlobalConfig::default();

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: gemini-auth-classified".to_string(),
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
        &[],
    )
    .await
    .expect("classified auth failure should return a result");

    assert_eq!(result.status_reason.as_deref(), Some("gemini_auth_prompt"));
    assert_eq!(result.execution.execution.exit_code, 1);
    assert!(
        result
            .execution
            .execution
            .summary
            .contains("no review verdict produced"),
        "unexpected summary: {}",
        result.execution.execution.summary
    );

    let persisted = csa_session::load_result(project_dir.path(), &result.execution.meta_session_id)
        .unwrap()
        .expect("result.toml");
    assert_eq!(persisted.exit_code, 1);
    assert!(persisted.summary.contains("no review verdict produced"));
}

#[cfg(unix)]
#[tokio::test]
async fn execute_review_does_not_retry_gemini_auth_prompt_when_no_failover_is_set() {
    use std::os::unix::fs::PermissionsExt;

    let project_dir = setup_git_repo();
    let _sandbox = ScopedSessionSandbox::new(&project_dir).await;
    let bin_dir = project_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let fake_gemini = bin_dir.join("gemini");
    let auth_log = project_dir.path().join("gemini-auth.log");
    std::fs::write(
        &fake_gemini,
        format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"--version\" ]; then\n\
  printf 'gemini-cli 1.0.0\\n'\n\
  exit 0\n\
fi\n\
printf 'oauth\\n' >> \"{}\"\n\
printf 'Opening authentication page\\nDo you want to continue? [Y/n]\\n'\n",
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

    let result = execute_review(
        ToolName::GeminiCli,
        "scope=uncommitted mode=review-only security=auto".to_string(),
        None,
        None,
        None,
        None,
        None,
        "review: gemini-auth-no-failover".to_string(),
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
        true,
        &[],
        &[],
    )
    .await
    .expect("classified auth failure should still return a result");

    assert_eq!(result.status_reason.as_deref(), Some("gemini_auth_prompt"));
    let auth_attempts = std::fs::read_to_string(auth_log).unwrap();
    assert!(
        !auth_attempts.contains("api_key\n"),
        "expected no api-key retry when --no-failover is set, got: {auth_attempts:?}"
    );
}

#[test]
fn csa_review_patterns_do_not_use_relative_output_paths() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let review_pattern_root = repo_root.join("patterns/csa-review");
    let mut offenders = Vec::new();

    fn visit(dir: &Path, offenders: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                visit(&path, offenders);
                continue;
            }
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(ext, "md" | "toml") {
                continue;
            }

            let content = std::fs::read_to_string(&path).expect("read pattern file");
            for (line_no, line) in content.lines().enumerate() {
                if line.contains("output/")
                    && !line.contains("$CSA_SESSION_DIR/output/")
                    && !line.contains("${CSA_SESSION_DIR}/output/")
                {
                    offenders.push(format!("{}:{}", path.display(), line_no + 1));
                }
            }
        }
    }

    visit(&review_pattern_root, &mut offenders);
    assert!(
        offenders.is_empty(),
        "found relative output/ paths in review pattern files:\n{}",
        offenders.join("\n")
    );
}
